# @discogs-rs/node

Per-artist genre and style profiles, and the raw XML of records by id,
from the [Discogs data dumps](https://data.discogs.com/), in Node.js, on
every core (Rust, N-API). See the [discogs-rs README](https://github.com/the-haus/discogs-rs#readme)
for what counts and the row schema.

```js
import { profileArtists } from "@discogs-rs/node";

const s = await profileArtists({
  source: "discogs_20261101_releases.xml.gz",
  checksum: "…",                  // optional sha256; a mismatch rejects and writes nothing
  targets: Uint32Array.from([1, 2, 3]),
  out: "releases.parquet",        // or .jsonl
});
console.log(s.records, s.matched, s.mbPerSecond);
```

Straight from data.discogs.com, streamed (nothing stored unless `keep`),
checked against the dump's `CHECKSUM.txt`, and skipped when unchanged:

```js
import { latestDump, profileArtists } from "@discogs-rs/node";

const dump = await latestDump(); // "20261101"
const s = await profileArtists({
  source: `https://data.discogs.com/?download=data%2F${dump.slice(0, 4)}%2Fdiscogs_${dump}_releases.xml.gz`,
  checksum: "auto",
  state: "state.json",
  targets,
  out: "releases.parquet",
});
console.log(s.skipped, s.downloadSeconds, s.attempts);
```

`dumpChecksums(dump)` returns a dump's `CHECKSUM.txt` as an object.

## Records by id

The raw XML of a `masters`, `artists` or `labels` file's records whose ids
you give (every record without), in file order, as batches:

```js
import { records, recordsOf } from "@discogs-rs/node";

const run = records({
  source: "discogs_20261101_artists.xml.gz", // or an https:// URL
  file: "artists",
  ids: Uint32Array.from([1, 2, 3]),          // optional
  checksum: "auto",                          // checked at the end: a mismatch throws after the last batch
});
for await (const batch of run) {
  // batch: { ids: Uint32Array, offsets: Uint32Array, xml: Buffer }
  for (const { id, xml } of recordsOf(batch)) console.log(id, xml.slice(0, 40));
}
console.log(run.summary());
```

A body runs from the record's start tag (`<artist><id>1</id>`) to its first
end tag, the end tag left out (see the repository README). The run works on
its own threads and stays two batches ahead of the loop; leaving the loop
stops it. **Why an async iterator of batches**, not a callback per record:
napi-rs crosses into JS cheaply once per block (a Buffer and two typed
arrays, no copy), the loop sets the pace (a callback could not await), and
`break` cancels. `openRecords(options)` is the native reader underneath:
`next()` resolves a batch or null, `close()`, `summary()`.

Prebuilt for linux-x64-gnu, linux-arm64-gnu and darwin-arm64.
`DISCOGS_RS_NODE_BINARY` names a binary to load instead.
