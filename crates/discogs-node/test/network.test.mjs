// URL sources, CHECKSUM.txt, skip-unchanged and the latest dump, against a
// local HTTP server (no network): node --test test/
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

const require = createRequire(import.meta.url);
const { profileArtists, latestDump, dumpChecksums } = require("../index.js");
const fixtures = new URL("../../../fixtures/", import.meta.url).pathname;
const dir = mkdtempSync(join(tmpdir(), "discogs-node-net-"));
const targets = Uint32Array.from([1, 2, 3, 4, 7, 239]);

const gz = gzipSync(readFileSync(join(fixtures, "releases.xml")));
const sum = createHash("sha256").update(gz).digest("hex");
const FILE = "discogs_20261101_releases.xml.gz";
const LINK = `/?download=data%2F2026%2F${FILE}`;
const SUMS = "/?download=data%2F2026%2Fdiscogs_20261101_CHECKSUM.txt";
const year = new Date().getUTCFullYear();

const hits = new Map();
const userAgents = [];
const server = createServer((req, res) => {
  hits.set(req.url, (hits.get(req.url) ?? 0) + 1);
  userAgents.push(req.headers["user-agent"]);
  if (req.url === LINK) return res.writeHead(302, { location: `/bucket/${FILE}` }).end();
  if (req.url === `/bucket/${FILE}`) return res.writeHead(200, { "content-length": gz.length }).end(gz);
  if (req.url === "/drop") {
    res.writeHead(200, { "content-length": gz.length });
    res.write(gz.subarray(0, 50));
    return setTimeout(() => res.destroy(), 10);
  }
  if (req.url === SUMS) return res.end(`${sum} ${FILE}\n${"b".repeat(64)} discogs_20261101_masters.xml.gz\n`);
  if (req.url === `/?prefix=data/${year}/`)
    return res.end(`discogs_${year}0101_CHECKSUM.txt\ndiscogs_${year}0201_CHECKSUM.txt\ndiscogs_${year}0301_releases.xml.gz`);
  if (req.url === `/?prefix=data/${year - 1}/`) return res.end(`discogs_${year - 1}1201_CHECKSUM.txt`);
  res.writeHead(404).end();
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}/`;
after(() => server.close());

test("profileArtists: a URL, checksum auto, kept, then skipped as unchanged", async () => {
  const out = join(dir, "url.jsonl");
  const keep = join(dir, FILE);
  const state = join(dir, "state.json");
  const options = { source: base + LINK.slice(1), checksum: "auto", targets, out, state, baseUrl: base };
  const first = await profileArtists({ ...options, keep });
  assert.equal(first.skipped, false);
  assert.equal(first.source, options.source);
  assert.equal(first.sha256, sum);
  assert.equal(first.expectedSha256, sum);
  assert.equal(first.records, 11);
  assert.equal(first.rows, 27);
  assert.equal(first.attempts, 1);
  assert.ok(first.downloadSeconds >= 0);
  assert.deepEqual(readFileSync(keep), gz);
  assert.match(userAgents.at(-1), /^discogs-rs\/\d+\.\d+\.\d+/);

  const again = await profileArtists(options);
  assert.equal(again.skipped, true);
  assert.equal(again.rows, 27);
  assert.equal(again.attempts, 0);
  assert.equal(hits.get(`/bucket/${FILE}`), 1);
});

test("profileArtists: a dropped connection rejects, writes nothing", async () => {
  const out = join(dir, "drop.jsonl");
  const before = hits.get("/drop") ?? 0;
  await assert.rejects(
    profileArtists({ source: base + "drop", targets, out, retries: 1, keep: join(dir, "drop.gz") }),
    /network/,
  );
  assert.equal(hits.get("/drop") - before, 2);
  assert.equal(existsSync(out), false);
  assert.equal(existsSync(join(dir, "drop.gz")), false);
  assert.equal(existsSync(join(dir, "drop.gz.partial")), false);
});

test("dumpChecksums and latestDump", async () => {
  const sums = await dumpChecksums("20261101", { baseUrl: base });
  assert.equal(sums[FILE], sum);
  assert.equal(Object.keys(sums).length, 2);
  assert.equal(await latestDump({ baseUrl: base }), `${year}0201`);
  await assert.rejects(dumpChecksums("2026", { baseUrl: base }), /names no dump/);
});
