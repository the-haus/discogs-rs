# Changelog

## 0.3.0 — unreleased

### Added

- Records by id: `discogs records --file masters|artists|labels --input
  <path or URL> [--ids ids.txt] [--checksum auto] [--out out.jsonl|-]`
  writes the raw XML body of each record whose id is in the set (every
  record without), in file order, as JSON lines `{id, file, xml}`. A body
  is cut exactly as Festimap's TypeScript scan cuts it (start match to the
  first end tag after it, end tag left out). Same reader, SHA-256, zlib-rs
  gunzip and splitter as `profile`, a parallel scan over a bitset, and a
  collector that hands blocks over in order with a bounded number in
  flight. A record out of id order fails the run (`Error::Dump`).
- Node: `records(options)`, an async iterable of batches
  (`{ ids: Uint32Array, offsets: Uint32Array, xml: Buffer }`) with
  `summary()` and `close()`; `recordsOf(batch)`; the native
  `openRecords(options)` → `RecordsReader` (`next()`, `close()`,
  `summary()`). Leaving the loop stops the run.
- Rust: `records` module (`RecordFile`, `RecordScanner`, `RecordBatch`,
  `RecordsOptions` with a `cancel` flag, `records_reader`, `records_path`,
  `collect_records`), `run_records` / `RecordsRunOptions` /
  `RecordsRunSummary` (URL, `auto` checksum, retries before the first
  batch).
- Fixtures: Festimap's artists and labels samples, the first 40 records of
  the 2026-10-01 artists and labels dumps. A property test and the
  `records` fuzz target (blocks find the records one scan finds); a
  criterion `records` bench.

### Changed

- The profile pipeline's decompress-and-split stage is shared with
  records (the head is always read first; the output is unchanged).

### Fixed

- Node: `DISCOGS_RS_NODE_BINARY` naming the package's own `index.js` no
  longer loads it into itself.

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
