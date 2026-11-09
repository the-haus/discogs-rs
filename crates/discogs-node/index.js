// Native module loader. Picks the platform-specific prebuilt binary if
// present, otherwise the dev build produced by `napi build` at the crate
// root.
//
// napi-rs appends an ABI suffix on some platforms:
//   - Linux GNU:   discogs-node.linux-x64-gnu.node
//   - Linux musl:  discogs-node.linux-x64-musl.node
//   - Windows:     discogs-node.win32-x64-msvc.node
//   - macOS:       discogs-node.darwin-arm64.node  (no ABI suffix)
//
// Published installs resolve the binary through an optional platform
// subpackage named `@discogs-rs/node-<triple>`. `DISCOGS_RS_NODE_BINARY`
// names a binary to load instead (for bundled apps that ship it apart).
//

"use strict";

// Intentional CJS: `.node` addons are loaded with require().
/* eslint-disable @typescript-eslint/no-require-imports */

const { platform, arch } = process;
const { existsSync } = require("node:fs");
const { join } = require("node:path");

// A glibc build of Node reports the glibc it runs on; a musl build does not.
// (`ldd --version` is no help: musl's prints to stderr and exits 1.)
function isMuslLibc() {
  try {
    const header = process.report?.getReport()?.header;
    if (header) return !header.glibcVersionRuntime;
  } catch {
    /* ignore */
  }
  try {
    return require("node:fs").readFileSync("/usr/bin/ldd", "utf8").includes("musl");
  } catch {
    return false;
  }
}

function candidateTriples() {
  if (platform === "linux") {
    const libc = isMuslLibc() ? "musl" : "gnu";
    return [`${platform}-${arch}-${libc}`, `${platform}-${arch}`];
  }
  if (platform === "win32") {
    return [`${platform}-${arch}-msvc`, `${platform}-${arch}`];
  }
  return [`${platform}-${arch}`];
}

function loadNative() {
  const override = process.env.DISCOGS_RS_NODE_BINARY;
  // An override naming this file (a consumer told to load the package
  // from its index.js) would load itself: look further.
  if (override && require.resolve(override) !== __filename) return require(override);
  const triples = candidateTriples();
  const localCandidates = [...triples.map((t) => `discogs-node.${t}.node`), "discogs-node.node"];
  for (const name of localCandidates) {
    const candidate = join(__dirname, name);
    if (existsSync(candidate)) {
      return require(candidate);
    }
  }
  const subpackageNames = triples.map((t) => `@discogs-rs/node-${t}`);
  for (const name of subpackageNames) {
    try {
      return require(name);
    } catch (err) {
      if (err && err.code !== "MODULE_NOT_FOUND") throw err;
    }
  }
  throw new Error(
    `@discogs-rs/node: no native binary found for ${platform}-${arch}. ` +
      `Tried local: ${localCandidates.join(", ")}. ` +
      `Tried npm: ${subpackageNames.join(", ")}. ` +
      "Set DISCOGS_RS_NODE_BINARY to the binary's path, or, for local development, run `yarn build` in crates/discogs-node.",
  );
}

const native = loadNative();

// Named one by one, so that ESM `import { profileArtists }` finds them.
module.exports.profileArtists = native.profileArtists;
module.exports.latestDump = native.latestDump;
module.exports.dumpChecksums = native.dumpChecksums;
module.exports.version = native.version;
module.exports.openRecords = native.openRecords;
module.exports.RecordsReader = native.RecordsReader;

/**
 * The records of a batch as `{ id, xml }`, each body decoded from UTF-8
 * (invalid bytes become U+FFFD, as `TextDecoder` does).
 */
function recordsOf(batch) {
  const out = new Array(batch.ids.length);
  for (let i = 0; i < batch.ids.length; i++) {
    out[i] = { id: batch.ids[i], xml: batch.xml.toString("utf8", batch.offsets[i], batch.offsets[i + 1]) };
  }
  return out;
}

/**
 * The raw XML of a masters, artists or labels file's records whose ids
 * are in `options.ids` (every record without), as an async iterable of
 * batches in file order. The run starts at once on its own threads and
 * stays a couple of batches ahead of the loop. Leaving the loop (break,
 * return, throw) stops it; `summary()` says what it did once the loop
 * ended normally.
 */
function records(options) {
  const reader = native.openRecords(options);
  let done = false;
  const iterator = {
    async next() {
      if (done) return { done: true, value: undefined };
      let batch;
      try {
        batch = await reader.next();
      } catch (error) {
        done = true;
        reader.close();
        throw error;
      }
      if (batch === null) {
        done = true;
        return { done: true, value: undefined };
      }
      return { done: false, value: batch };
    },
    async return(value) {
      done = true;
      reader.close();
      return { done: true, value };
    },
    [Symbol.asyncIterator]() {
      return iterator;
    },
    summary: () => reader.summary(),
    close() {
      done = true;
      reader.close();
    },
  };
  return iterator;
}

module.exports.records = records;
module.exports.recordsOf = recordsOf;
