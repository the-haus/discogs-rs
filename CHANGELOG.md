# Changelog

## 0.2.0 — unreleased

### Added

- Streaming input: `discogs profile --input https://…` (and Node's
  `source`) reads a dump straight from data.discogs.com into the pass,
  never stored, with `reqwest` (blocking, its runtime on its own thread)
  over `rustls` (ring, Mozilla roots built in: no system CA store or
  OpenSSL). Redirects are followed; the User-Agent is `discogs-rs/<version>`.
  A dropped connection, a timeout, a 5xx or a 429 starts the file over
  (the site ignores `Range`), `--retries` times (default 2); then the run
  fails with nothing written. `--timeout` (default 120 s) bounds each wait.
- `--checksum auto`: the expected SHA-256 from the dump's `CHECKSUM.txt`
  (beside the URL; beside a local file, else on the data site).
- `--state <file.json>`: skip-unchanged. A dump whose checksum (known
  before reading) equals the last successful run's, with the same targets
  and options and the output still there, is not read: `skipped: true`.
- `--keep <path>`: writes the download to disk as it streams, renamed into
  place once the run succeeds.
- `discogs checksum --dump YYYYMMDD` and `discogs latest` (the newest dump
  whose `CHECKSUM.txt` is listed); `--base-url` / `DISCOGS_DATA_URL` for
  another site.
- Node: `profileArtists({ source: url, checksum: "auto", state, keep,
  retries, timeoutSeconds, baseUrl })`; the summary gains `skipped`,
  `source`, `expectedSha256`, `downloadSeconds`, `attempts`.
  `latestDump()` and `dumpChecksums(dump)`.
- Rust: `run_profile` / `RunOptions` / `RunSummary` (the whole run, as the
  CLI and Node do it), `dumps` (names, links, `CHECKSUM.txt` and listing
  parsing), `remote::Http` (feature `http`, on by default),
  `TargetSet::digest`, `Error::{Network, Http, Checksum, State, Usage}`.
- Criterion benches of the splitter and `scan_block`
  (`cargo bench -p discogs-core --bench stages`; not run in CI).

### Changed

- `discogs profile --input` takes a path or a URL; `--json` adds `skipped`,
  `source`, `expected_sha256`, `download_seconds`, `attempts` and
  `write_seconds`.

## 0.1.0 — unreleased

### Added

- `discogs profile`: per-artist genre, genre/style pair and total counts
  by year, format class and promo flag from a local releases or masters
  dump (`.xml.gz` or `.xml`), with the SHA-256 checked on the way; Parquet
  or JSON lines.
- `discogs bench --generate <GB>`: a synthetic dump and timed runs per
  stage.
- `@discogs-rs/node`: `profileArtists()`, off the JS thread.
