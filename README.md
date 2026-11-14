# db-conformance: one contract for every ce-db language

The kit that keeps ce-db-rs and ce-db-ts (and later Python, Swift, Go) behaving the same, in the style of
`ce-conformance`: a language-neutral `SCENARIOS.md`, one thin runner per language that drives only that SDK's public
API and prints `CONF <id> PASS|FAIL`, and a driver that builds the cross-language matrix and fails on any FAIL.

```
npm install && npm run build          # the TypeScript runner (dist/runners/ts/run.js), @ce-net/db from its repo
cargo build                           # conf-rs (the Rust runner) and db-conformance (the driver)
target/debug/db-conformance           # fresh dev stores (ce-db-dev, loopback, in memory): D1..D10, D8 both ways
target/debug/db-conformance --store HOST:PORT --cap CAP --cap-ro CAP   # a granted db-conformance app elsewhere
```

| path | what |
|---|---|
| `SCENARIOS.md` | the contract, D1 to D10 |
| `schema/` | `conf-schema`: the `#[derive(Record)]` source of the records and the golden values |
| `golden/items.json` | the exact stored text of the two golden records (D1) |
| `sdk/ts/db-conformance.ts`, `schemas/db-conformance.json` | generated: `ce-wire-gen --records schema/src/lib.rs --lang ts -o sdk/ts` and `--lang json -o schemas`; never edit |
| `runners/rs/` | `conf-rs` on `ce-db` |
| `runners/ts/run.ts` | the TypeScript runner on `@ce-net/db` |
| `driver/` | `db-conformance`: starts dev stores, runs each runner, D8 across languages, prints the matrix |

The live run against a real replica needs an app of its own: `repo:state-store-cell/AGENTS.md` allows live tests on
`repl-probe/ping` only, so the store's owners must register and grant `db-conformance` first. Until then the kit runs
against ce-db-dev, which speaks the same wire and schema hash as state.store and copies the engine's rules.
