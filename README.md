<div align="center">

# discogs-rs

**Reading the Discogs data dumps, fast, in Rust.**

One pass over a Discogs monthly dump (`releases`: 10.5 GB gzipped, about 80 GB of XML) turns into per-artist genre and style counts, on every core, in flat memory, with the file's SHA-256 checked on the way.

<p>
  <a href="https://github.com/the-haus/discogs-rs/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/the-haus/discogs-rs/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/the-haus/discogs-rs/actions/workflows/node.yml"><img alt="Node" src="https://github.com/the-haus/discogs-rs/actions/workflows/node.yml/badge.svg"></a>
  <a href="https://www.npmjs.com/package/@discogs-rs/node"><img alt="npm" src="https://img.shields.io/npm/v/@discogs-rs/node?label=npm&logo=npm"></a>
  <a href="#development"><img alt="Rust 1.87+" src="https://img.shields.io/badge/rust-1.87%2B-orange?logo=rust"></a>
  <a href="LICENCE.txt"><img alt="License: FTWPL" src="https://img.shields.io/badge/license-FTWPL-lightgrey"></a>
</p>

<sub>CLI · Rust crate · Node.js &nbsp;·&nbsp; zlib-rs gzip · sha256 on the fly &nbsp;·&nbsp; byte scanners, no XML parser &nbsp;·&nbsp; Parquet · JSON lines</sub>

</div>

---

## Overview

discogs-rs reads the [Discogs monthly data dumps](https://data.discogs.com/)
(CC0) and answers one question fast: for each artist you care about, what
genres and styles does it release under, by year and by format?

- the file, or the HTTPS download, is read in 4 MB chunks and hashed
  (SHA-256) on the way; given the checksum from Discogs' `CHECKSUM.txt`
  (or `auto`, looked up there), a mismatch fails the run and writes
  nothing;
- gzip is decoded by `flate2` on the `zlib-rs` backend, on its own thread,
  straight into ~16 MB blocks cut after their last whole record;
- scanner threads take the blocks from a bounded channel and find each
  record's main artists with `memchr::memmem` and a few digit reads, tested
  against a bitset of your target artist ids; only a hit has its genres,
  styles, year, formats and data quality read;
- per-thread sums are merged into rows, written as Parquet (zstd) or JSON
  lines.

It also hands over the **raw XML of records by id** from the `masters`,
`artists` and `labels` files (`discogs records`, Node `records()`), through
the same reader, gunzip, splitter and parallel scan, in file order: see
[Records by id](#records-by-id).

It is built for [Festimap](https://festimap.com), which maps the counts to
its own genre tree and weighs them (recency, format) itself, so the
weighting can be retuned without reading 80 GB again. It is a sibling of
[wikidata-rs](https://github.com/the-haus/wikidata-rs) and follows the same
layout.

## Contents

- [Quick start](#quick-start)
- [What counts](#what-counts)
- [The output](#the-output)
- [Records by id](#records-by-id)
- [Performance](#performance)
- [Project layout](#project-layout)
- [Development](#development)
- [Limitations](#limitations)
- [CI/CD](#cicd)
- [License](#license)

## Quick start

### Command line

```sh
cargo install --path crates/discogs-cli   # the `discogs` binary

# One Discogs artist id per line (blank lines and # comments are skipped).
printf '1\n2\n3\n' > targets.txt

discogs profile \
  --input discogs_20261101_releases.xml.gz \
  --targets targets.txt \
  --checksum 3f9c…e1 \
  --out releases.parquet          # or .jsonl
```

`--threads N` sets the scanner threads (default: the cores less one),
`--kind releases|masters` overrides what the root element says, `--json`
prints the summary as JSON, `--no-hash` skips the SHA-256 (benchmarks).

### Straight from data.discogs.com

```sh
discogs latest                      # 20261101: the newest dump whose CHECKSUM.txt is listed
discogs checksum --dump 20261101    # <sha256>  discogs_20261101_<kind>.xml.gz, per line (--json)

discogs profile \
  --input 'https://data.discogs.com/?download=data%2F2026%2Fdiscogs_20261101_releases.xml.gz' \
  --checksum auto \
  --state discogs-state.json \
  --keep /data/discogs/discogs_20261101_releases.xml.gz \
  --targets targets.txt --out releases.parquet
```

- The download streams into the pass: nothing is stored unless `--keep`
  (written as it streams to `<keep>.partial`, renamed once the run
  succeeds, so a second pass can read it locally).
- `--checksum auto` takes the expected SHA-256 from the dump's
  `CHECKSUM.txt` (the one beside the URL; for a local file, the one beside
  it, else the data site's), before anything is downloaded.
- data.discogs.com ignores `Range`: a dropped connection, a timeout, a 5xx
  or a 429 starts the file over, `--retries` times (default 2), then the run
  fails with nothing written. `--timeout` (seconds, default 120) bounds each
  wait for a connection or the next bytes. Redirects (to the bucket) are
  followed; the User-Agent is `discogs-rs/<version>`.
- `--state` is skip-unchanged: each successful run records, per (file,
  targets), the input's SHA-256 and a hash of the options that shape the
  output (discogs-rs version, kind, output path). A run whose checksum is
  known before reading (given or `auto`) and equal, with the same targets
  and options and its output still there, downloads nothing and reports
  `skipped: true` (the counts are then the last run's). The "file" is the
  dump file's kind (`releases`, `masters`), so an unchanged file in a later
  dump is skipped too. Runs sharing a state file must not overlap.
- `--base-url` (or `DISCOGS_DATA_URL`) points `latest`, `checksum` and
  `auto` at another site.
- TLS is `rustls` with the ring provider and the Mozilla roots compiled in:
  no OpenSSL, no system CA store needed.

```sh
discogs bench --generate 2        # 2 GB synthetic dump, then timed runs
```

### Node.js

```js
import { profileArtists } from "@discogs-rs/node";

const summary = await profileArtists({
  source: "/data/discogs/20261101/discogs_20261101_masters.xml.gz",
  checksum: "…",                     // optional; a mismatch rejects, nothing is written
  targets: Uint32Array.from(ids),    // Discogs artist ids
  threads: 7,                        // optional; default cores − 1
  out: "/data/discogs/20261101/masters.parquet",
});
// { kind, records, hits, matched, skippedVarious, skippedMaster, skippedStatus,
//   skippedQuality, bytes, bytesDecompressed, rows, sha256, decompressor,
//   threads, seconds, stages: { read, hash, decompress, split, scan, merge, write, total },
//   mbPerSecond, skipped, source, expectedSha256, downloadSeconds, attempts }
```

From the data site, skipping a dump already profiled:

```js
import { latestDump, dumpChecksums, profileArtists } from "@discogs-rs/node";

const dump = await latestDump();                     // "20261101"
const sums = await dumpChecksums(dump);              // { "discogs_20261101_releases.xml.gz": "…", … }
const s = await profileArtists({
  source: `https://data.discogs.com/?download=data%2F${dump.slice(0, 4)}%2Fdiscogs_${dump}_releases.xml.gz`,
  checksum: "auto",                  // from the dump's CHECKSUM.txt
  state: "/data/discogs/state.json", // skip-unchanged
  keep: `/data/discogs/discogs_${dump}_releases.xml.gz`, // optional copy, written as it streams
  retries: 2,                        // whole-file retries (default 2)
  targets,
  out: "/data/discogs/releases.parquet",
});
if (s.skipped) console.log("unchanged since the last run");
else console.log(`${s.downloadSeconds.toFixed(0)} s on the network, ${s.attempts} attempt(s)`);
```

`latestDump(options?)` and `dumpChecksums(dump, options?)` take
`{ baseUrl }` for another site.

The pass runs on its own threads (an napi `AsyncTask`); the JS thread stays
free. Prebuilt for linux-x64-gnu, linux-arm64-gnu and darwin-arm64.

### Rust

```rust
use discogs_core::{output::write_rows, profile_path, ProfileOptions, TargetSet};

let targets = TargetSet::from_ids([1u32, 2, 3]);
let profile = profile_path("discogs_20261101_releases.xml.gz", &targets, &ProfileOptions::default())?;
write_rows(&profile.rows, "releases.parquet")?;
println!("{} records, {:.0} MB/s", profile.summary.records, profile.summary.mb_per_s);
```

The whole run as the CLI does it (a URL or a path, `auto` checksum, state,
retries, keep, output):

```rust
use discogs_core::{run_profile, RunOptions};

let run = run_profile(&targets, &RunOptions {
    source: "https://data.discogs.com/?download=data%2F2026%2Fdiscogs_20261101_masters.xml.gz".into(),
    checksum: Some("auto".into()),
    state: Some("state.json".into()),
    out: "masters.parquet".into(),
    ..Default::default()
})?;
```

`discogs_core::remote::Http` (feature `http`, on by default) has
`latest_dump()`, `checksums(dump)` and `open(url)`; `discogs_core::dumps`
the file names, links and the `CHECKSUM.txt` and listing parsers.

## What counts

Records are `<release id="…">…</release>` in a releases file and
`<master id="…">…</master>` in a masters file. Run both: masters, plus the
releases that have no master, count every work once.

- **Main artists only**: the record's first `<artists>` element, and only
  if it comes before `<tracklist>`. `<extraartists>` (producers, remixers,
  mastering) and a track's own `<artists>` never count.
- Each main artist once; a record with *n* main artists gives **1/*n*** to
  each.
- Skipped: a record crediting **"Various"** (artist 194); a record whose
  `<data_quality>` is **Entirely Incorrect**; a release **with a master**
  (`<master_id>` above 0: the dumps write
  `<master_id is_main_release="false">0</master_id>` on every release that
  has none); a release whose `status` attribute is set and not `Accepted`
  (the 2026 dumps carry no status).
- The year: a master's `<year>`, a release's `<released>` (`YYYY` or
  `YYYY-MM-DD`); `0` or absent is no year.
- The format class of a release, from every `<description>` in its
  `<formats>`, the first that applies: `mix` (Mixed, Mixtape), `compilation`
  (Compilation), `album` (Album), `ep` (EP, Mini-Album), `album` (LP),
  `single` (Single, Maxi-Single, or a 7"), else `other`. Masters carry no
  formats: their class is null. `promo` is set for a Promo, Test Pressing
  or Unofficial Release.
- Genres and styles are entity-decoded (`Rock &amp; Roll` → `Rock & Roll`)
  and counted once per record.

## The output

One row per (artist, kind, name, year, format class, promo), summed over
the records:

| column | type | |
|---|---|---|
| `artist_id` | u32 | Discogs artist id |
| `kind` | string | `genre`, `style` or `total` |
| `name` | string | a genre (`Electronic`); a **genre/style pair** (`Electronic/Electro`); `""` for `total` |
| `year` | u16, nullable | the record's year |
| `format_class` | string, nullable | `album`, `ep`, `single`, `compilation`, `mix`, `other`; null for masters |
| `promo` | bool | Promo, Test Pressing or Unofficial Release |
| `weight` | f32 | genre and total rows: Σ 1/*n*; style rows: Σ 1/(*n*·*g*) |
| `share` | f32 | the weight split over the record's rows of this kind: genre Σ 1/(*n*·*g*), style Σ 1/(*n*·*g*·*s*), total Σ 1/*n* |
| `records` | u32 | records counted |

*n*: the record's main artists; *g*: its genres; *s*: its styles.

**Styles are tied to their genres.** A style means different things under
different genres (Electro under Electronic or under Hip Hop, Dub under
Reggae or Electronic), so a style row's name is `Genre/Style`, one row for
each (genre, style) combination on the record, the style's weight split
evenly across that record's genres. A record with genres Electronic and
Hip Hop and style Electro, by one artist, gives `Electronic/Electro` and
`Hip Hop/Electro`, each with weight 0.5. A record with styles but no genre
writes `/Style`. To split a pair, take the longest of Discogs' genres the
name starts with (`Funk / Soul` contains a slash), then the rest after the
`/`.

`total` rows count the records themselves, whatever their genres:
`Σ records` over an artist's total rows is its counted releases, and its
first and last years are there too.

No recency or format weighting is applied: the rows are raw counts, so a
consumer can weigh them as it likes (Festimap: `share × 0.5^(age/8) ×
0.5 for compilations, mixes and promos`) without re-reading the dump. The
same rows come out for any thread count and block size (weights are summed
in 32.32 fixed point, so the order of the sums does not matter).

JSON lines carry the same fields:

```json
{"artist_id":2,"kind":"style","name":"Hip Hop/Electro","year":1984,"format_class":"album","promo":false,"weight":0.25,"share":0.25,"records":1}
```

## Records by id

`discogs records` (Node: `records()` / `openRecords()`, Rust:
`run_records` / `records_reader`) reads a `masters`, `artists` or `labels`
file and hands over the raw XML of the records whose ids you give (or of
every record), **in file order**, with the file's SHA-256 checked on the
way. No field is parsed: the consumer parses the bodies, so a TypeScript
reader and this one can share one parser.

A record is cut exactly as Festimap's TypeScript scan cuts it:

| file | starts at | body ends before |
|---|---|---|
| `masters` | `<master id="N">` | the first `</master>` after it |
| `artists` | `<artist><id>N</id>` | the first `</artist>` after it |
| `labels` | `<label><id>N</id>` | the first `</label>` after it |

Only top-level records match (a nested artist or label carries its id as
an attribute). A label's body stops at its first sub-label's end, after its
id, name, profile, URLs and parent. The end tag is left out. The search for
the next record resumes after it. A record with no end tag before the end
of the input is dropped. A record whose id is below the one before it fails
the run (the dumps are written in id order).

```sh
discogs records --file artists --input discogs_20261101_artists.xml.gz \
  --ids artist-ids.txt --checksum auto --out artists.jsonl
# {"id":1,"file":"artists","xml":"<artist><id>1</id><name>The Persuader</name>…"}
```

Without `--ids` every record is written; without `--out` nothing is
(timing). The checksum is known only once the whole file is read: on a
mismatch the command fails and leaves no output (lines go to
`<out>.partial` first). Node streams the batches as they come, so there a
mismatch throws at the end of the loop, after the batches were handed over:

```js
import { records, recordsOf } from "@discogs-rs/node";

const run = records({ source: "discogs_20261101_artists.xml.gz", file: "artists", ids: Uint32Array.from(ids), checksum: "auto" });
for await (const batch of run) {
  for (const { id, xml } of recordsOf(batch)) parse(id, xml);
}
console.log(run.summary()); // { records, matched, mbPerSecond, sha256, … }
```

A batch is `{ ids: Uint32Array, offsets: Uint32Array, xml: Buffer }` (one
block's wanted records: record `i` is `xml[offsets[i], offsets[i + 1])`), so
crossing into JS costs one Buffer per block, not an object per record. The
run stays two batches ahead of the loop (backpressure); leaving the loop,
or `close()`, stops it. `openRecords()` is the native reader behind
`records()`: `next()` (a Promise of a batch, or null at the end), `close()`
and `summary()`.

How it works: the splitter cuts blocks after the file's end tag; scanners
find the start pattern with `memchr::memmem`, read the id, test it against
a bitset, and copy a wanted record's bytes out; a collector gives the
blocks back in order, with tickets bounding the blocks in flight. A cut
right after an end tag never splits a record (no start pattern holds an
end tag), so blocks find exactly the records one scan of the whole finds:
a property test and the `records` fuzz target check it.

## Performance

Apple M1 Max (8 performance cores), macOS, release build, a 2.0 GB
synthetic releases dump (`discogs bench --generate 2`: 824,134 releases
shaped like the 2026-11-01 dump's, 2.4 KB each, gzip -6 ratio 8.1):

| run | 1 scanner thread | 9 scanner threads |
|---|---|---|
| gunzip alone (zlib-rs, one thread) | 1,298 MB/s | — |
| split + scan, already-decompressed XML, 1% hits | 6,288 MB/s | 8,025 MB/s |
| split + scan, already-decompressed XML, 10% hits | 5,188 MB/s | 7,427 MB/s |
| end to end from .gz (sha256 included), 1% hits | 1,283 MB/s | 1,306 MB/s |
| end to end from .gz (sha256 included), 10% hits | 1,309 MB/s | 1,312 MB/s |

(MB of XML per wall second. "1% / 10% hits" select every 100th / 10th
artist id; with 1.3 main artists a release, 1.3% / 12.3% of the releases
hit.) A 530 MB file of real records (the dump's first records, repeated,
gzip -6, ratio 6.0) decodes at 990 MB/s. Peak RSS of an end-to-end run:
141 MB.

Per stage (criterion, `cargo bench -p discogs-core --bench stages`, 32 MB
of synthetic XML, one thread): the splitter cuts 16 MB blocks at about 12
GiB/s; `scan_block` runs at 6.6 GiB/s on releases with no hits, 5.5 GiB/s
at 1% and 4.6 GiB/s at 10%, and 3.6 / 3.2 / 1.9 GiB/s on masters.

From a URL, the network is the ceiling unless the link is faster than
about 1 GB/s of XML (some 160 MB/s of gzip).

**Records by id, on the real 2026-10-01 dumps** (same machine, files on an
external SSD, sha256 checked, `discogs records` with no output; Festimap's
TypeScript reader on Node 24 for comparison, the same records cut):

| file | XML | records | every record | 50,000 ids | TypeScript, every record |
|---|---|---|---|---|---|
| artists (499 MB .gz) | 2.26 GB | 10,243,337 | 3.4 s (664 MB/s) | 4.4 s (512 MB/s) | 23.0 s (98 MB/s) |
| labels (91 MB .gz) | 0.45 GB | 2,426,075 | 0.6 s (699 MB/s) | 0.7 s (682 MB/s) | 5.7 s (79 MB/s) |
| masters (630 MB .gz) | 3.18 GB | 2,598,586 | 4.1 s (769 MB/s) | 4.9 s (645 MB/s) | 28.3 s (112 MB/s) |

Every record decoded into JS strings through `openRecords` takes 4.9 s
(artists), 1.0 s (labels) and 4.1 s (masters). Peak RSS of the CLI: 320 to
400 MB. Criterion (`records`, 32 MB of synthetic masters XML in memory):
every record 1.5 GiB/s on one thread, 2.3 GiB/s on nine; 1% of the ids
3.7 GiB/s.

**Decompression is the ceiling.** One scanner thread keeps up with zlib-rs
with room to spare (the scan costs about 0.2 CPU-seconds per GB), so more
threads change nothing end to end; they matter only for plain XML input.

## Project layout

| path | |
|---|---|
| `crates/discogs-core` | the engine: targets, splitter, scanner, format classes, aggregation, pipeline, output, synthetic generator; `records` (records by id); `dumps` (names, links, CHECKSUM.txt), `remote` (HTTPS), `run` (checksum lookup, state, retries, keep) |
| `crates/discogs-cli` | the `discogs` command: `profile`, `records`, `checksum`, `latest`, `bench` |
| `crates/discogs-node` | the napi-rs binding, published as `@discogs-rs/node` |
| `fixtures/` | hand-written releases and masters, targets, the first 40 records of the 2026-11-01 releases and masters, Festimap's artists and labels samples and the first 40 records of the 2026-10-01 artists and labels |
| `fuzz/` | cargo-fuzz targets: the profile scanner and splitter (`scan`), records (`records`) |

## Development

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude discogs-node      # the network tests use a local server
cargo test -p discogs-core --test http -- --ignored # reads the real data.discogs.com listing (never a dump)
cargo bench -p discogs-core --bench stages          # criterion: splitter, scan_block, records
corepack yarn install && cd crates/discogs-node && corepack yarn build && corepack yarn test
cargo +nightly fuzz run scan -- -dict=fuzz/discogs.dict   # needs cargo-fuzz; macOS: add --sanitizer none
cargo +nightly fuzz run -s none records -- -dict=fuzz/discogs.dict
```

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Limitations

- No resume: data.discogs.com ignores `Range`, so a broken download starts
  over (the 10.5 GB releases file included). Use `--keep` to read a dump
  twice without downloading it twice.
- One gzip thread: about 1 GB/s of XML on an M1, so the decompressor bounds
  the run. A parallel single-member gunzip is planned only if a real run
  measures below 400 MB/s.
- Records by id hand over raw XML: parsing the fields is the consumer's.
  A records checksum mismatch is known only after the last batch.
- Masters have no format class (the dump gives masters no formats).

## CI/CD

- `ci.yml`: rustfmt, clippy `-D warnings`, `cargo test` on Linux x64,
  Linux arm64 and macOS; two minutes of fuzzing; version and workflow
  checks.
- `node.yml`: the binding built and tested on the same three platforms.
- `release.yml` (on a `v*` tag): checks, then `release-node.yml` (prebuilt
  binaries for linux-x64-gnu, linux-arm64-gnu, darwin-arm64, installed and
  used from the packed tarballs, then published to npm), the `discogs`
  binaries, and a GitHub release. See [RELEASING.md](RELEASING.md).

## License

[FTWPL](LICENCE.txt). The Discogs data is CC0; the fixtures' real records
come from the 2026-11-01 dump.
