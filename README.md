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

- the file is read in 4 MB chunks and hashed (SHA-256) on the way; given the
  checksum from Discogs' `CHECKSUM.txt`, a mismatch fails the run and
  writes nothing;
- gzip is decoded by `flate2` on the `zlib-rs` backend, on its own thread,
  straight into ~16 MB blocks cut after their last whole record;
- scanner threads take the blocks from a bounded channel and find each
  record's main artists with `memchr::memmem` and a few digit reads, tested
  against a bitset of your target artist ids; only a hit has its genres,
  styles, year, formats and data quality read;
- per-thread sums are merged into rows, written as Parquet (zstd) or JSON
  lines.

It is built for [Festimap](https://festimap.com), which maps the counts to
its own genre tree and weighs them (recency, format) itself, so the
weighting can be retuned without reading 80 GB again. It is a sibling of
[wikidata-rs](https://github.com/the-haus/wikidata-rs) and follows the same
layout.

## Contents

- [Quick start](#quick-start)
- [What counts](#what-counts)
- [The output](#the-output)
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
//   mbPerSecond }
```

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

**Decompression is the ceiling.** One scanner thread keeps up with zlib-rs
with room to spare (the scan costs about 0.2 CPU-seconds per GB), so more
threads change nothing end to end; they matter only for plain XML input.

## Project layout

| path | |
|---|---|
| `crates/discogs-core` | the engine: targets, splitter, scanner, format classes, aggregation, pipeline, output, synthetic generator |
| `crates/discogs-cli` | the `discogs` command: `profile`, `bench` |
| `crates/discogs-node` | the napi-rs binding, published as `@discogs-rs/node` |
| `fixtures/` | hand-written releases and masters, targets, and the first 40 records of the 2026-11-01 dumps |
| `fuzz/` | cargo-fuzz target for the scanner and splitter |

## Development

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude discogs-node
corepack yarn install && cd crates/discogs-node && corepack yarn build && corepack yarn test
cargo +nightly fuzz run scan -- -dict=fuzz/discogs.dict   # needs cargo-fuzz
```

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Limitations

- Local files only (`.xml.gz` or `.xml`). Reading a URL, `CHECKSUM.txt`
  and skipping an unchanged dump are next (0.2).
- One gzip thread: about 1 GB/s of XML on an M1, so the decompressor bounds
  the run. A parallel single-member gunzip is planned only if a real run
  measures below 400 MB/s.
- Only `profile`: reading `artists`, `labels` and `masters` records by id
  comes later.
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
