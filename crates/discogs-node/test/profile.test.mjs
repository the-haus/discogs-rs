// The binding against the fixtures: node --test test/
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";

const require = createRequire(import.meta.url);
const { profileArtists, version } = require("../index.js");
const fixtures = new URL("../../../fixtures/", import.meta.url).pathname;
const dir = mkdtempSync(join(tmpdir(), "discogs-node-"));
const targets = Uint32Array.from([1, 2, 3, 4, 7, 239]);

test("version", () => {
  assert.equal(version(), JSON.parse(readFileSync(new URL("../package.json", import.meta.url))).version);
});

test("profileArtists: releases to JSON lines", async () => {
  const out = join(dir, "releases.jsonl");
  const s = await profileArtists({ source: join(fixtures, "releases.xml"), targets, threads: 2, out });
  assert.equal(s.kind, "releases");
  assert.equal(s.records, 11);
  assert.equal(s.matched, 5);
  assert.equal(s.rows, 27);
  assert.equal(s.sha256.length, 64);
  assert.ok(s.seconds >= 0 && s.stages.write >= 0);
  const rows = readFileSync(out, "utf8").trim().split("\n").map((l) => JSON.parse(l));
  assert.equal(rows.length, 27);
  const pair = rows.find((r) => r.artist_id === 2 && r.name === "Hip Hop/Electro" && r.year === 1984);
  assert.equal(pair.kind, "style");
  assert.equal(pair.weight, 0.25);
});

test("profileArtists: masters to Parquet", async () => {
  const out = join(dir, "masters.parquet");
  const s = await profileArtists({ source: join(fixtures, "masters.xml"), targets, kind: "masters", out });
  assert.equal(s.matched, 3);
  assert.equal(readFileSync(out).subarray(0, 4).toString(), "PAR1");
});

test("profileArtists: a checksum mismatch rejects and writes nothing", async () => {
  const out = join(dir, "mismatch.parquet");
  await assert.rejects(
    profileArtists({ source: join(fixtures, "releases.xml"), targets, checksum: "0".repeat(64), out }),
    /checksum mismatch/,
  );
  assert.equal(existsSync(out), false);
});

test("profileArtists: runs off the JS thread", async () => {
  // The event loop turns while the pass runs: a setImmediate queued after
  // the call fires before the promise settles.
  let turned = false;
  const run = profileArtists({ source: join(fixtures, "releases.xml"), targets, out: join(dir, "async.jsonl") });
  setImmediate(() => (turned = true));
  assert.ok(run instanceof Promise);
  await run;
  assert.equal(turned, true);
});
