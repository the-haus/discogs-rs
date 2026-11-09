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

Prebuilt for linux-x64-gnu, linux-arm64-gnu and darwin-arm64.
`DISCOGS_RS_NODE_BINARY` names a binary to load instead.
