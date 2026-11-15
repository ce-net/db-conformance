# db-conformance scenarios (the contract)

Every ce-db language passes these against a state.store, driving only its public API, and prints one line per
scenario: `CONF <id> PASS` or `CONF <id> FAIL <why>`. Source of the design: `repo:state-store-cell/docs/DB-SDK-DESIGN.md`
section 8. The records are `schema/src/lib.rs` (`Item`, and `ItemNext`, the same collection one version later); other
languages use the types `ce-wire-gen --records` generates from that file (`sdk/ts/db-conformance.ts`).

App `db-conformance`, collections `items` and `notes`. Grants: `--cap` holds `state.dbconformance.*` (read, write,
admin), `--cap-ro` holds `state.dbconformance.items.read` only, `--cap-items` holds `state.dbconformance.items.read`
and `.write` only, `--cap-other` holds every right of another app (`parts-demo`). Every key carries the run tag, so a
run never meets another.

| id | checks |
|---|---|
| D1 encode | `golden_a("g-a")` and `golden_b("g-b")` encode to exactly the text in `golden/items.json` (keys sorted, no whitespace, `u64`/`i64` as decimal strings, bytes as hex, `Millis` as an integer, `100.0` as `100`, `None` absent, `_v`), and that text decodes back to the equal typed value with no unknown fields and writer version 2 |
| D2 crud | insert returns the key; get equals what was inserted; a second insert is `Conflict { held_version: 1 }`; update changes a field and get sees it; update of a missing key is `NotFound`; the version after insert+update is 2; a compare-and-set over version 1 is `Conflict { held_version: 2 }`; after remove, get is empty |
| D3 unknown-fields | a newer program (`ItemNext`, version 3) writes `colour`; the older program (`Item`, version 2) opens the collection registered at version 3, sees `colour` in the stored row's unknown fields, updates `qty`, and the newer program then reads `qty` changed and `colour` intact |
| D4 index-eq | of 12 rows, the 4 in one bin come back from `by_bin`, exactly and in key order; `count` is 4; an unused bin is empty |
| D5 index-range | 30 rows with `qty` 1000, 1001, 1002 (10 each): pages of 7 over `1000..=1002` walk all 30 in (qty, key) order with the page boundaries inside equal values, ascending and descending; `1001..1002` counts 10; `1003..` is empty |
| D6 pagination | 1,201 rows under one prefix walk in exactly 3 pages of at most 500 (op 4 List cursors), no duplicate, no gap |
| D7 watch | a prefix watch on an empty prefix sees `Synced`, then, written by a second connection, `Added` (the equal value), `Changed` (old and new), `Removed` (the key), then nothing more |
| D8 cross-language | language A's watcher on one key prints `READY` after `Synced`; language B inserts `golden_a(key)`; A sees `Added` with the equal typed value and get returns it. Run both ways for every pair |
| D9 too-large | an item whose encoding exceeds 2,000 bytes fails with `TooLarge { bytes > 2000, limit: 2000 }` before any call, and get finds nothing |
| D10 refused | with the read-only grant, insert fails with `Refused { need: "state.dbconformance.items.write" }`, and nothing is written |
| D11 reconnect | a prefix watch has seen b and c and `Synced`; the store is killed (a dev store in a child process, never a production replica); a second instance over the same data file takes the changes the watcher misses (insert a, update b's `qty` 1 to 2, remove c) and stops; the store restarts on the first address. The watch's link reports `Offline`, then `Resyncing`, then exactly `Added(a)`, `Changed(b)`, `Removed(c)` (no duplicate, nothing before `Resyncing`), then `Live` |
| D12 collection-scope | the `items`-only grant inserts into `items`; on `notes` insert fails with `Refused { need: "state.dbconformance.notes.write" }` and get with `Refused { need: "state.dbconformance.notes.read" }`; nothing lands in `notes` |
| D13 wrong-app | another app's grant (`--cap-other`) is refused on `db-conformance` with `Refused` naming a `state.dbconformance.` right (at open, the schema check or insert), and nothing is written |

Known encoding limits, not in the golden values: non-integral floats below 1e-6 or at or above 1e21 (Rust's ryu and
JavaScript write them differently); JSON numbers above 2^53 in fields a JavaScript reader does not know (they are
kept as JavaScript numbers).

## M1 to M5: offline merge (db.replica)

Two replicas of one typed collection keep working with no connection and merge when they meet. These scenarios are
data, not calls to a store: each file in `golden/merge/` holds the scripted steps and the expected result, and a
language's replica passes when it reproduces `expect` exactly. The reference replicas, Rust and TypeScript, are
`db.replica` in `repo:demo-offline-merge` (`crates/db-replica`, `ts/src/replica.ts`); its CI runs every file here
with two Rust processes, Rust and TypeScript processes, and two TypeScript processes, and all three must match.

The record is `Part` (app `offline-merge`, collection `parts`, version 1): `id` (key, string, max 8), `name`
(string, max 40), `qty` (u32), `bin` (string, max 6). Rows are stored in the D1 canonical encoding.

The rules:

- **Op log.** Every local change is an op `{by, seq, hlc, key, kind: set|delete, fields}` appended to the log;
  `seq` counts from 1 per origin. A `set` carries only the fields it writes, in their canonical encoding (an insert
  writes all of them). A local edit is checked against the record first (decode, then the `max` bounds); a refused
  edit gets ce-db's error text (`invalid qty: expected an integer in u32`, `invalid name: 44 long, the limit is
  40`) and never enters the log. An edit of a deleted key is refused with `not found: <key> was deleted`.
- **Hybrid logical clock.** `hlc = {w, c, n}`: physical ms, counter, node name, ordered by all three (the name by
  bytes). Local event at physical time pt: if pt > w then (pt, 0) else (w, c+1). Receiving an op with clock m at pt:
  w' = max(w, m.w, pt); c' = max(c, m.c)+1 if w' equals both w and m.w, c+1 if only w, m.c+1 if only m.w, else 0.
  A replica applies the clock receive once per op it takes, in (origin, seq) order.
- **Sync.** A replica sends its version vector `{node: highest seq seen}`; the peer answers with every op the vector
  lacks. Ops are taken in seq order per origin; a gap waits for a resend. Nothing is queued while apart.
- **Merge.** The state is a function of the set of ops: a key with any `delete` op is gone, whatever the clocks
  (delete beats edit, and stays gone); otherwise each field takes the value of the `set` with the highest hlc that
  wrote it (field-level last-writer-wins).
- **State hash.** SHA-256, lowercase hex, over `key<TAB>row<LF>` for every live row (the row's canonical text), then
  `-<TAB>key<LF>` for every deleted key, both sorted by key bytes.
- **Resolutions.** At each plug-in, for every key both sides touched while apart: a delete gives one entry (rule
  `delete beats edit`, or `both deleted it`) listing the edits it overrode; otherwise one entry per field, `later
  hybrid clock`, `same millisecond, higher counter`, `same clock reading, node name breaks the tie`, or `only one
  side wrote this field: both edits kept`.

Scenario steps: `clock` (initial physical clock per replica), then `{at, on, seed|set|delete}` (the replica's
physical clock becomes `at`), `{do: cut}`, `{do: plug}`, `{do: clock, on, at}`. While plugged in, the replicas sync
to the same version vector after every step. `expect` holds the hash, the rows, the deleted keys, per plug-in the
resolutions and each side's row diff (ce-db's `diff`), the refused edits and each log's length.

| id | file | checks | golden hash |
|---|---|---|---|
| M1 story | `golden/merge/partition-1.json` | apart, one side changes `qty` of p12 and the other renames it (both kept); one deletes p7 and the other edits it (deleted) | `2891c5de7869e5f5` |
| M2 same-field | `golden/merge/partition-2.json` | both change `qty` of p3 and p4: the later hlc wins per field, and p3 keeps the other side's `bin` | `68153f90ba85ed2e` |
| M3 slow-clock | `golden/merge/partition-3.json` | the phone's clock is 2 s behind; its edit after seeing the laptop's write carries the laptop's time and wins; a same-millisecond write is decided by the counter | `5b92fef13c51bd6b` |
| M4 ties-and-deletes | `golden/merge/partition-4.json` | equal (w, c) on both sides: the node name decides; both delete p9; a delete beats two edits of p10 | `bff13b80f1699d89` |
| M5 two-partitions | `golden/merge/partition-5.json` | new parts on each side, two refused typed edits not in the log, a second partition with delete against rename, edits of the deleted part refused on both sides | `71950bdf8db301d9` |
