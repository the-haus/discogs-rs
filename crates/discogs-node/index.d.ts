/** Options of {@link profileArtists}. */
export interface ProfileArtistsOptions {
  /** The dump: a `.xml.gz` or `.xml` file path (HTTPS URLs come in 0.2). */
  source: string
  /** The source's expected SHA-256 (64 hex digits). On a mismatch the promise rejects and nothing is written. */
  checksum?: string
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
}

/**
 * Per-artist Discogs genre and style counts from one dump file, written as
 * Parquet or JSON lines (see the README for the row schema). Runs on its
 * own threads; resolves with the summary, or rejects (writing nothing) on
 * an error or a checksum mismatch.
 */
export declare function profileArtists(options: ProfileArtistsOptions): Promise<ProfileSummary>

/** The package version. */
export declare function version(): string
