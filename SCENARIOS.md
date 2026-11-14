# db-conformance scenarios (the contract)

Every ce-db language passes these against a state.store, driving only its public API, and prints one line per
scenario: `CONF <id> PASS` or `CONF <id> FAIL <why>`. Source of the design: `repo:state-store-cell/docs/DB-SDK-DESIGN.md`
section 8. The records are `schema/src/lib.rs` (`Item`, and `ItemNext`, the same collection one version later); other
languages use the types `ce-wire-gen --records` generates from that file (`sdk/ts/db-conformance.ts`).

App `db-conformance`, collection `items`. Grants: `--cap` holds `state.dbconformance.*` (read, write, admin),
`--cap-ro` holds `state.dbconformance.items.read` only. Every key carries the run tag, so a run never meets another.

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

Known encoding limits, not in the golden values: non-integral floats below 1e-6 or at or above 1e21 (Rust's ryu and
JavaScript write them differently); JSON numbers above 2^53 in fields a JavaScript reader does not know (they are
kept as JavaScript numbers).
