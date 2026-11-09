#!/bin/sh
# Install the packed @discogs-rs/node from packages/ (see release-node.yml)
# into an empty project, with the platform package for one triple, and use
# it from ESM and CommonJS. Run from the repository root:
#
#   sh crates/discogs-node/scripts/install-check.sh linux-x64-gnu
set -eu
TRIPLE="$1"
rm -rf install-check
mkdir install-check
cd install-check
npm init -y >/dev/null
npm install --no-audit --no-fund ../packages/discogs-rs-node-[0-9]*.tgz "../packages/discogs-rs-node-$TRIPLE-"*.tgz
ls node_modules/@discogs-rs
cp ../fixtures/releases.xml .
cat > esm.mjs <<'JS'
import { profileArtists, version } from "@discogs-rs/node";
const s = await profileArtists({ source: "releases.xml", targets: Uint32Array.from([1, 2, 3]), out: "out.parquet" });
if (s.matched !== 5) throw new Error(`expected 5 matched releases, got ${s.matched}`);
console.log(`esm: ${version()} on ${process.platform}-${process.arch}, node ${process.version}: ${s.rows} rows`);
JS
cat > cjs.cjs <<'JS'
const { profileArtists } = require("@discogs-rs/node");
profileArtists({ source: "releases.xml", targets: Uint32Array.from([1]), out: "out.jsonl" }).then((s) => {
  if (s.records !== 11) throw new Error(`expected 11 records, got ${s.records}`);
  console.log(`cjs: ${s.rows} rows`);
});
JS
node esm.mjs
node cjs.cjs
