#![deny(clippy::all)]

//! Node.js binding of discogs-rs: `profileArtists()` runs a whole profile
//! pass (read, sha256, gunzip, split, scan, merge, write) off the JS
//! thread and resolves with its summary.

use std::path::Path;
use std::time::Instant;

use discogs_core::output::write_rows;
use discogs_core::{profile_path, ProfileOptions, RecordKind, Summary, TargetSet};
use napi::bindgen_prelude::{AsyncTask, Uint32Array};
use napi::{Env, Error, Result, Task};
use napi_derive::napi;

/// Options of `profileArtists`.
#[napi(object)]
pub struct ProfileArtistsOptions {
    /// The dump: a `.xml.gz` or `.xml` file path. (HTTPS URLs: M2.)
    pub source: String,
    /// The source's expected SHA-256 (64 hex digits). On a mismatch the
    /// promise rejects and nothing is written.
    pub checksum: Option<String>,
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
}

pub struct ProfileTask {
    source: String,
    out: String,
    targets: TargetSet,
    options: ProfileOptions,
}

impl Task for ProfileTask {
    type Output = (Summary, f64);
    type JsValue = ProfileSummary;

    fn compute(&mut self) -> Result<Self::Output> {
        let fail = |e: discogs_core::Error| Error::from_reason(e.to_string());
        let profile = profile_path(Path::new(&self.source), &self.targets, &self.options).map_err(fail)?;
        let t = Instant::now();
        write_rows(&profile.rows, &self.out).map_err(fail)?;
        Ok((profile.summary, t.elapsed().as_secs_f64()))
    }

    fn resolve(&mut self, _env: Env, (s, write): Self::Output) -> Result<Self::JsValue> {
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
        source: options.source,
        out: options.out,
        targets,
        options: ProfileOptions {
            threads: options.threads.unwrap_or(0) as usize,
            block_size: options.block_size.unwrap_or(0) as usize,
            checksum: options.checksum,
            kind,
        },
    }))
}

/// The package version.
#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
