# @discogs-rs/node

Per-artist genre and style profiles from the
[Discogs data dumps](https://data.discogs.com/), in Node.js, on every core
(Rust, N-API). See the [discogs-rs README](https://github.com/the-haus/discogs-rs#readme)
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

Prebuilt for linux-x64-gnu, linux-arm64-gnu and darwin-arm64.
`DISCOGS_RS_NODE_BINARY` names a binary to load instead.
