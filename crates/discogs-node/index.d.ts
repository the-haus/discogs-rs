/** Options of {@link profileArtists}. */
export interface ProfileArtistsOptions {
  /**
   * The dump: a `.xml.gz` or `.xml` file path, or an `https://` URL, e.g.
   * `https://data.discogs.com/?download=data%2F2026%2Fdiscogs_20261101_releases.xml.gz`
   * (streamed straight into the pass; stored only with `keep`).
   */
  source: string
  /**
   * The source's expected SHA-256 (64 hex digits), or `"auto"`: the sum the
   * dump's `CHECKSUM.txt` gives for the source's file name (for a URL, the
   * one beside it; for a path, the one beside the file, else the data
   * site's). On a mismatch the promise rejects and nothing is written.
   */
  checksum?: string | 'auto'
  /**
   * Skip-unchanged state file (JSON, created when missing). A dump already
   * profiled with the same checksum (known before reading: given or
   * `"auto"`), targets and options, whose output is still there, is not
   * read again: the summary says `skipped: true`.
   */
  state?: string
  /** URL sources: also write the download to this path as it streams (renamed into place once the run succeeds). */
  keep?: string
  /** URL sources: extra whole tries after a dropped connection, a timeout, a 5xx or a 429 (default 2). */
  retries?: number
  /** URL sources: seconds to wait for a connection or the next bytes (default 120). */
  timeoutSeconds?: number
  /** The data site (default `https://data.discogs.com/`). */
  baseUrl?: string
  /** Target Discogs artist ids. */
  targets: Uint32Array
  /** `"releases"` or `"masters"`; read from the file when absent. */
  kind?: 'releases' | 'masters'
  /** Scanner threads; default: the cores less one. */
  threads?: number
  /** Block size in bytes; default 16 MB. */
  blockSize?: number
  /** Output path: `.parquet` or `.jsonl`. */
  out: string
}

/** Seconds per stage. `scan` is CPU seconds over all scanner threads. */
export interface ProfileStages {
  read: number
  hash: number
  decompress: number
  split: number
  scan: number
  merge: number
  write: number
  total: number
}

/** What {@link profileArtists} did. */
export interface ProfileSummary {
  kind: 'releases' | 'masters'
  /** Records scanned. */
  records: number
  /** Records whose main artists include a target. */
  hits: number
  /** Hits counted (not Various, not with a master, not Entirely Incorrect). */
  matched: number
  skippedVarious: number
  skippedMaster: number
  skippedStatus: number
  skippedQuality: number
  /** Bytes read from the source. */
  bytes: number
  /** Bytes of XML after gunzip. */
  bytesDecompressed: number
  /** Rows written. */
  rows: number
  /** The source's SHA-256 (lowercase hex). */
  sha256: string
  /** `zlib-rs`, or `none` for plain XML. */
  decompressor: string
  threads: number
  /** Wall seconds, writing included. */
  seconds: number
  stages: ProfileStages
  /** MB (10^6 bytes) of XML per wall second. */
  mbPerSecond: number
  /** Nothing was read: unchanged since the run the state file recorded (the counts are that run's, the timings 0). */
  skipped: boolean
  /** The source as given. */
  source: string
  /** The expected checksum used (given or from `CHECKSUM.txt`), if any. */
  expectedSha256?: string
  /** Seconds waiting on the network (requests and body reads), over all attempts; 0 for a file. */
  downloadSeconds: number
  /** Tries of the source (more than 1 after a dropped connection; 0 when skipped). */
  attempts: number
}

/** Options of {@link latestDump} and {@link dumpChecksums}. */
export interface SiteOptions {
  /** The data site (default `https://data.discogs.com/`). */
  baseUrl?: string
}

/**
 * Per-artist Discogs genre and style counts from one dump file, written as
 * Parquet or JSON lines (see the README for the row schema). Runs on its
 * own threads; resolves with the summary, or rejects (writing nothing) on
 * an error or a checksum mismatch.
 */
export declare function profileArtists(options: ProfileArtistsOptions): Promise<ProfileSummary>

/**
 * The newest complete Discogs dump, e.g. `"20261101"`: the newest whose
 * `CHECKSUM.txt` is listed (Discogs writes it last). Reads this year's
 * listing, and last year's in January or while this year lists none.
 */
export declare function latestDump(options?: SiteOptions): Promise<string>

/** A dump's `CHECKSUM.txt`: file name (`discogs_20261101_releases.xml.gz`) → SHA-256. */
export declare function dumpChecksums(dump: string, options?: SiteOptions): Promise<Record<string, string>>

/** Options of {@link records} and {@link openRecords}. */
export interface RecordsOptions {
  /** The dump file: a `.xml.gz` or `.xml` path, or an `https://` URL (streamed). */
  source: string
  /** Which file it is: the record's start and end tags follow from it. */
  file: 'masters' | 'artists' | 'labels'
  /** The ids wanted; every record when absent. */
  ids?: Uint32Array
  /**
   * The source's expected SHA-256 (64 hex digits), or `"auto"` (the dump's
   * `CHECKSUM.txt`). Known only once the whole file is read: on a mismatch
   * the iteration throws at its end, after every batch was handed over, so
   * a caller that wrote batches as they came must treat the run as failed.
   */
  checksum?: string | 'auto'
  /** URL sources: extra whole tries after a transport failure, while no batch has been handed over yet (default 2). */
  retries?: number
  /** URL sources: seconds to wait for a connection or the next bytes (default 120). */
  timeoutSeconds?: number
  /** The data site (default `https://data.discogs.com/`). */
  baseUrl?: string
  /** Scanner threads; default: the cores less one. */
  threads?: number
  /** Block size in bytes; default 16 MB. A batch holds the wanted records of one block. */
  blockSize?: number
}

/**
 * Records handed over together, in file order: record `i` is `ids[i]`,
 * its body `xml.subarray(offsets[i], offsets[i + 1])`, UTF-8: from the
 * start tag (`<master id="…">`, `<artist><id>…`, `<label><id>…`) up to the
 * first end tag after it, the end tag left out. A label's body stops at
 * its first sub-label's end, as Festimap's TypeScript scan cuts it.
 */
export interface RecordBatch {
  ids: Uint32Array
  /** `ids.length + 1` entries, the first 0. */
  offsets: Uint32Array
  xml: Buffer
}

/** Seconds per stage of a records run; `deliver` is time waiting for JS to take batches. */
export interface RecordsStages {
  read: number
  hash: number
  decompress: number
  split: number
  scan: number
  deliver: number
  total: number
}

/** What a records run did. */
export interface RecordsSummary {
  file: 'masters' | 'artists' | 'labels'
  /** Records scanned. */
  records: number
  /** Records handed over. */
  matched: number
  batches: number
  /** Ids asked for (0: every record). */
  ids: number
  /** Bytes read from the source. */
  bytes: number
  bytesDecompressed: number
  /** The source's SHA-256 (empty when cancelled). */
  sha256: string
  decompressor: string
  threads: number
  /** Wall seconds. */
  seconds: number
  stages: RecordsStages
  /** MB (10^6 bytes) of XML per wall second. */
  mbPerSecond: number
  /** Stopped before the end (`close()`, or the loop left early). */
  cancelled: boolean
  source: string
  expectedSha256?: string
  downloadSeconds: number
  attempts: number
}

/** A records run: batches one {@link RecordsReader.next} at a time. */
export declare class RecordsReader {
  /** The next batch, or null at the end. Rejects on an error (a dump out of id order, a checksum mismatch at the end). Call one at a time. */
  next(): Promise<RecordBatch | null>
  /** Stops the run: the file is read no further. */
  close(): void
  /** What the run did, once `next()` resolved null; else null. */
  summary(): RecordsSummary | null
}

/** The iterable {@link records} returns. */
export interface Records extends AsyncIterableIterator<RecordBatch> {
  /** What the run did, once the loop ended at the end of the file; else null. */
  summary(): RecordsSummary | null
  /** Stops the run (leaving a `for await` loop does too). */
  close(): void
}

/**
 * The raw XML of a masters, artists or labels file's records whose ids are
 * in `options.ids` (every record without), as batches in file order. The
 * run starts at once on its own threads (gunzip, SHA-256, split, parallel
 * scan) and stays a couple of batches ahead of the loop; leaving the loop
 * stops it.
 *
 * ```js
 * for await (const batch of records({ source, file: 'artists', ids })) {
 *   for (const { id, xml } of recordsOf(batch)) parse(id, xml)
 * }
 * ```
 */
export declare function records(options: RecordsOptions): Records

/** The lower-level run behind {@link records}. */
export declare function openRecords(options: RecordsOptions): RecordsReader

/** A batch's records as `{ id, xml }`, each body decoded from UTF-8. */
export declare function recordsOf(batch: RecordBatch): { id: number; xml: string }[]

/** The package version. */
export declare function version(): string
