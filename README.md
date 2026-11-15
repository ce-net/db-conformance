# db-conformance: one contract for every ce-db language

The kit that keeps ce-db-rs and ce-db-ts (and later Python, Swift, Go) behaving the same, in the style of
`ce-conformance`: a language-neutral `SCENARIOS.md`, one thin runner per language that drives only that SDK's public
API and prints `CONF <id> PASS|FAIL`, and a driver that builds the cross-language matrix and fails on any FAIL.

```
npm install && npm run build          # the TypeScript runner (dist/runners/ts/run.js), @ce-net/db from its repo
cargo build                           # conf-rs (the Rust runner) and db-conformance (the driver)
target/debug/db-conformance           # fresh dev stores (ce-db-dev, loopback): D1..D13, D8 both ways, D11 kills one
target/debug/db-conformance --store HOST:PORT --cap CAP --cap-ro CAP --cap-items CAP --cap-other CAP   # a granted replica
```

```
== D11 rs: the store killed mid-watch and restarted (a dev store in a child process, never --store)
    d11: killed the store at 127.0.0.1:45485 mid-watch
    d11: wrote the missed changes through a second instance at 127.0.0.1:37789, stopped it
    d11: restarted the store at 127.0.0.1:45485 on the same data
d11: connecting -> offline -> resyncing -> change removed d11-.../c -> change added d11-.../a -> change changed d11-.../b -> live
    CONF D11 PASS
...
       D1   D2   D3   D4   D5   D6   D7   D8   D9   D10  D11  D12  D13
rs     PASS PASS PASS PASS PASS PASS PASS PASS PASS PASS PASS PASS PASS
ts     PASS PASS PASS PASS PASS PASS PASS PASS PASS PASS PASS PASS PASS

26 of 26 PASS
```

| path | what |
|---|---|
| `SCENARIOS.md` | the contract, D1 to D13, and M1 to M5 (offline merge) |
| `schema/` | `conf-schema`: the `#[derive(Record)]` source of the records and the golden values |
| `golden/items.json` | the exact stored text of the two golden records (D1) |
| `golden/merge/` | M1 to M5, offline merge: the scripted partitions and their expected state hash, rows, resolutions and refusals; run by [ce-net/demo-demo-offline-merge](https://github.com/ce-net/demo-demo-offline-merge) (Rust and TypeScript `db.replica`) |
| `sdk/ts/db-conformance.ts`, `schemas/db-conformance.json` | generated: `ce-wire-gen --records schema/src/lib.rs --lang ts -o sdk/ts` and `--lang json -o schemas`; never edit |
| `runners/rs/` | `conf-rs` on `ce-db` |
| `runners/ts/run.ts` | the TypeScript runner on `@ce-net/db` |
| `driver/` | `db-conformance`: starts dev stores, runs each runner, D8 across languages, D11 (kill and restart a dev store child, `db-conformance dev-store`), prints the matrix |
| `.github/workflows/ci.yml` | CI on every push and pull request: the matrix on dev stores, and the generated files checked for drift |

The live run against a real replica needs a granted app of its own (four caps: full `state.dbconformance.*`,
`items.read`, `items.read`+`items.write`, and another app's grant as `--cap-other`). D11 never runs on `--store`:
it kills a dev store. Without grants the kit runs against ce-db-dev, which speaks the same wire and schema hash as
state.store and copies the engine's rules.

Dependencies not yet public: the ce-db SDKs build on the ce-net wire crates, which are not open yet (see the
ce-db-rs README); CI reads them with read-only deploy keys.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE)). See [NOTICE](NOTICE).
