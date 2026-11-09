// records() against the fixtures: node --test test/
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";

const require = createRequire(import.meta.url);
const { records, recordsOf, openRecords } = require("../index.js");
const fixtures = new URL("../../../fixtures/", import.meta.url).pathname;
const dir = mkdtempSync(join(tmpdir(), "discogs-node-records-"));

/** dump.ts's cut: from the start match to the first end tag after it. */
function tsScan(text, start, end) {
  const out = [];
  let m;
  start.lastIndex = 0;
  while ((m = start.exec(text))) {
    const close = text.indexOf(end, start.lastIndex);
    if (close < 0) break;
    out.push({ id: Number(m[1]), xml: text.slice(m.index, close) });
    start.lastIndex = close + end.length;
  }
  return out;
}

const FILES = [
  ["artists", "real-artists-20261001-head.xml", /<artist><id>(\d+)<\/id>/g, "</artist>"],
  ["labels", "real-labels-20261001-head.xml", /<label><id>(\d+)<\/id>/g, "</label>"],
  ["masters", "real-masters-20261101-head.xml", /<master id="(\d+)">/g, "</master>"],
];

async function all(options) {
  const it = records(options);
  const out = [];
  for await (const batch of it) out.push(...recordsOf(batch));
  return { out, summary: it.summary() };
}

for (const [file, name, start, end] of FILES) {
  test(`records: ${file} as the TypeScript scan cuts them`, async () => {
    const text = readFileSync(join(fixtures, name), "utf8");
    const expected = tsScan(text, start, end);
    const { out, summary } = await all({ source: join(fixtures, name), file, threads: 2, blockSize: 4096 });
    assert.deepEqual(out, expected);
    assert.equal(summary.records, expected.length);
    assert.equal(summary.matched, expected.length);
    assert.equal(summary.sha256.length, 64);
    assert.equal(summary.cancelled, false);

    const gz = join(dir, `${file}.xml.gz`);
    writeFileSync(gz, gzipSync(readFileSync(join(fixtures, name))));
    const ids = Uint32Array.from(expected.filter((_, i) => i % 4 === 1).map((r) => r.id));
    const some = await all({ source: gz, file, ids });
    assert.deepEqual(some.out, expected.filter((_, i) => i % 4 === 1));
    assert.equal(some.summary.decompressor, "zlib-rs");
  });
}

test("records: leaving the loop stops the run", async () => {
  const it = records({ source: join(fixtures, "real-masters-20261101-head.xml"), file: "masters", blockSize: 512 });
  let n = 0;
  for await (const batch of it) {
    n += batch.ids.length;
    break;
  }
  assert.ok(n > 0);
  assert.deepEqual(await it.next(), { done: true, value: undefined });
});

test("records: a checksum mismatch throws at the end", async () => {
  await assert.rejects(
    all({ source: join(fixtures, "masters.xml"), file: "masters", checksum: "0".repeat(64) }),
    /checksum mismatch/,
  );
});

test("records: an unknown file is refused", () => {
  assert.throws(() => openRecords({ source: join(fixtures, "masters.xml"), file: "releases" }), /masters, artists or labels/);
});
