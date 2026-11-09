# Contributing

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude discogs-node
corepack yarn install && cd crates/discogs-node && corepack yarn build && corepack yarn test
```

- The tests use `fixtures/releases.xml` and `fixtures/masters.xml`,
  hand-written in the shape of the 2026-11-01 dump (main, extra and track
  artists, Various, entities, missing years, several artists, releases with
  and without a master, Entirely Incorrect), and the first 40 records of
  the real dump (`fixtures/real-*-20261101-head.xml`, CC0). Expected rows
  are asserted in `crates/discogs-core/tests/fixtures.rs`; the same
  fixtures are meant to be asserted by Festimap's TypeScript reader.
- Behaviour that follows the dumps' conventions should say so in a
  comment, with an example (e.g. why `<master_id>0</master_id>` is no
  master).
- Changes to the scanner or splitter: run the fuzzer for a few minutes
  (`cargo +nightly fuzz run scan -- -dict=fuzz/discogs.dict`; on macOS
  add `--sanitizer none`: AddressSanitizer's start-up hangs there in
  `InitializeShadowMemory`, before the target runs).
- Commits follow Conventional Commits (`feat:`, `fix:`, `perf:`, …).

## Releasing

See [RELEASING.md](RELEASING.md).
