#![deny(clippy::all)]

//! Node.js binding of discogs-rs: `profileArtists()` runs a whole profile
//! pass (read or download, sha256, gunzip, split, scan, merge, write) off
//! the JS thread and resolves with its summary; `latestDump()` and
//! `dumpChecksums()` read the data site. Every call is an napi
//! `AsyncTask`: the work, HTTP included (a blocking client with its own
//! runtime thread), runs off the event loop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use discogs_core::remote::{Http, HttpOptions};
use discogs_core::{run_profile, ProfileOptions, RecordKind, RunOptions, RunSummary, TargetSet};
use napi::bindgen_prelude::{AsyncTask, Uint32Array};
use napi::{Env, Error, Result, Task};
use napi_derive::napi;

/// Options of `profileArtists`.
#[napi(object)]
pub struct ProfileArtistsOptions {
    /// The dump: a `.xml.gz` or `.xml` file path, or an `https://` URL
    /// (streamed; stored only with `keep`).
    pub source: String,
    /// The source's expected SHA-256 (64 hex digits), or `"auto"`: the sum
    /// the dump's `CHECKSUM.txt` gives for the source's file name. On a
    /// mismatch the promise rejects and nothing is written.
    pub checksum: Option<String>,
    /// Skip-unchanged state file (JSON). A dump already profiled with the
    /// same checksum (known before reading: given or `"auto"`), targets and
    /// options, whose output is still there, is not read again: the summary
    /// says `skipped: true`.
    pub state: Option<String>,
    /// URL sources: also write the download to this path as it streams
    /// (renamed into place once the run succeeds).
    pub keep: Option<String>,
    /// URL sources: extra whole tries after a dropped connection, a
    /// timeout, a 5xx or a 429 (default 2; data.discogs.com cannot resume).
    pub retries: Option<u32>,
    /// URL sources: seconds to wait for a connection or the next bytes
    /// (default 120).
    pub timeout_seconds: Option<u32>,
    /// The data site (default `https://data.discogs.com/`).
    pub base_url: Option<String>,
    /// Target Discogs artist ids.
    pub targets: Uint32Array,
    /// `"releases"` or `"masters"`; read from the file when absent.
    pub kind: Option<String>,
    /// Scanner threads; default: the cores less one.
    pub threads: Option<u32>,
    /// Block size in bytes; default 16 MB.
    pub block_size: Option<u32>,
    /// Output path: `.parquet` or `.jsonl`.
    pub out: String,
}

/// Seconds per stage. `scan` is CPU seconds over all scanner threads.
#[napi(object)]
pub struct ProfileStages {
    pub read: f64,
    pub hash: f64,
    pub decompress: f64,
    pub split: f64,
    pub scan: f64,
    pub merge: f64,
    pub write: f64,
    pub total: f64,
}

/// What `profileArtists` did.
#[napi(object)]
pub struct ProfileSummary {
    /// `releases` or `masters`.
    pub kind: String,
    /// Records scanned.
    pub records: f64,
    /// Records whose main artists include a target.
    pub hits: f64,
    /// Hits counted (not Various, not with a master, not Entirely Incorrect).
    pub matched: f64,
    pub skipped_various: f64,
    pub skipped_master: f64,
    pub skipped_status: f64,
    pub skipped_quality: f64,
    /// Bytes read from the source.
    pub bytes: f64,
    /// Bytes of XML after gunzip.
    pub bytes_decompressed: f64,
    /// Rows written.
    pub rows: f64,
    /// The source's SHA-256.
    pub sha256: String,
    /// `zlib-rs`, or `none` for plain XML.
    pub decompressor: String,
    pub threads: u32,
    /// Wall seconds, writing included.
    pub seconds: f64,
    pub stages: ProfileStages,
    /// MB (10^6 bytes) of XML per wall second.
    pub mb_per_second: f64,
    /// Nothing was read: unchanged since the run the state file recorded
    /// (the counts are that run's).
    pub skipped: bool,
    /// The source as given.
    pub source: String,
    /// The expected checksum used (given or from `CHECKSUM.txt`), if any.
    pub expected_sha256: Option<String>,
    /// Seconds waiting on the network (requests and body reads), over all
    /// attempts; 0 for a file.
    pub download_seconds: f64,
    /// Tries of the source (more than 1 after a dropped connection; 0 when
    /// skipped).
    pub attempts: u32,
}

pub struct ProfileTask {
    targets: TargetSet,
    options: RunOptions,
}

impl Task for ProfileTask {
    type Output = RunSummary;
    type JsValue = ProfileSummary;

    fn compute(&mut self) -> Result<Self::Output> {
        run_profile(&self.targets, &self.options).map_err(|e| Error::from_reason(e.to_string()))
    }

    fn resolve(&mut self, _env: Env, run: Self::Output) -> Result<Self::JsValue> {
        let write = run.write_seconds;
        let s = run.profile;
        let st = s.seconds;
        Ok(ProfileSummary {
            kind: s.kind,
            records: s.records as f64,
            hits: s.hits as f64,
            matched: s.matched as f64,
            skipped_various: s.skipped_various as f64,
            skipped_master: s.skipped_master as f64,
            skipped_status: s.skipped_status as f64,
            skipped_quality: s.skipped_quality as f64,
            bytes: s.bytes_in as f64,
            bytes_decompressed: s.bytes_decompressed as f64,
            rows: s.rows as f64,
            sha256: s.sha256,
            decompressor: s.decompressor,
            threads: s.threads as u32,
            seconds: st.total + write,
            stages: ProfileStages {
                read: st.read,
                hash: st.hash,
                decompress: st.decompress,
                split: st.split,
                scan: st.scan,
                merge: st.merge,
                write,
                total: st.total + write,
            },
            mb_per_second: s.mb_per_s,
            skipped: run.skipped,
            source: run.source,
            expected_sha256: run.expected_sha256,
            download_seconds: run.download_seconds,
            attempts: run.attempts,
        })
    }
}

/// Per-artist Discogs genre and style counts from one dump file, written
/// as Parquet or JSON lines. Runs on its own threads; the promise resolves
/// with the summary, or rejects (writing nothing) on an error or a
/// checksum mismatch.
#[napi(ts_return_type = "Promise<ProfileSummary>")]
pub fn profile_artists(options: ProfileArtistsOptions) -> Result<AsyncTask<ProfileTask>> {
    let kind = options
        .kind
        .as_deref()
        .map(|k| k.parse::<RecordKind>())
        .transpose()
        .map_err(Error::from_reason)?;
    let targets = TargetSet::from_ids(options.targets.iter().copied());
    Ok(AsyncTask::new(ProfileTask {
        targets,
        options: RunOptions {
            source: options.source,
            out: PathBuf::from(options.out),
            checksum: options.checksum,
            state: options.state.map(PathBuf::from),
            keep: options.keep.map(PathBuf::from),
            retries: options.retries.unwrap_or(2),
            base_url: options.base_url,
            timeout: options.timeout_seconds.map(|s| Duration::from_secs(s.into())),
            profile: ProfileOptions {
                threads: options.threads.unwrap_or(0) as usize,
                block_size: options.block_size.unwrap_or(0) as usize,
                checksum: None,
                kind,
                no_hash: false,
            },
        },
    }))
}

/// Options of `latestDump` and `dumpChecksums`.
#[napi(object)]
pub struct SiteOptions {
    /// The data site (default `https://data.discogs.com/`).
    pub base_url: Option<String>,
}

fn http(site: Option<SiteOptions>) -> Result<Http> {
    let mut o = HttpOptions::default();
    if let Some(base) = site.and_then(|s| s.base_url) {
        o.base_url = base;
    }
    Http::new(o).map_err(|e| Error::from_reason(e.to_string()))
}

pub struct LatestTask(Option<Http>);

impl Task for LatestTask {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<String> {
        let http = self.0.take().expect("computed once");
        http.latest_dump().map_err(|e| Error::from_reason(e.to_string()))
    }
    fn resolve(&mut self, _env: Env, dump: String) -> Result<String> {
        Ok(dump)
    }
}

/// The newest complete Discogs dump (`"20261101"`): the newest whose
/// `CHECKSUM.txt` is listed on the data site.
#[napi(ts_return_type = "Promise<string>")]
pub fn latest_dump(options: Option<SiteOptions>) -> Result<AsyncTask<LatestTask>> {
    Ok(AsyncTask::new(LatestTask(Some(http(options)?))))
}

pub struct ChecksumsTask(Option<Http>, String);

impl Task for ChecksumsTask {
    type Output = HashMap<String, String>;
    type JsValue = HashMap<String, String>;
    fn compute(&mut self) -> Result<Self::Output> {
        let http = self.0.take().expect("computed once");
        let sums = http.checksums(&self.1).map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(sums.into_iter().collect())
    }
    fn resolve(&mut self, _env: Env, sums: Self::Output) -> Result<Self::JsValue> {
        Ok(sums)
    }
}

/// A dump's `CHECKSUM.txt`: file name → SHA-256.
#[napi(ts_return_type = "Promise<Record<string, string>>")]
pub fn dump_checksums(dump: String, options: Option<SiteOptions>) -> Result<AsyncTask<ChecksumsTask>> {
    Ok(AsyncTask::new(ChecksumsTask(Some(http(options)?), dump)))
}

/// The package version.
#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
