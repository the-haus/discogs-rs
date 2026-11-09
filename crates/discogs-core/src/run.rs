//! A whole profile run, as the CLI and the Node binding do it: the source
//! (a path or an HTTPS URL), the expected checksum (given, or `auto` from
//! the dump's `CHECKSUM.txt`), skip-unchanged against a state file, whole
//! file retries, a copy of the download kept on disk, and the output.
//!
//! **Skip-unchanged.** With a state file, every successful run records, per
//! (file, targets), the input's SHA-256 and a hash of the options that
//! shape the output (the discogs-rs version, the kind, the output path). A
//! later run whose expected checksum is known before reading (given, or
//! `auto`), with the same targets and options, and whose output is still
//! there, reads nothing and returns `skipped: true`. The "file" is the dump
//! file's kind (`releases`, `masters`) when the source names a Discogs dump
//! file, so an unchanged file in a later dump is skipped too; else the
//! source itself. Runs sharing one state file must not run at the same
//! time.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dumps::{self, checksum_file, dump_file_in, parse_checksums};
use crate::output::write_rows;
use crate::pipeline::{hex, profile_reader, Profile, ProfileOptions, Summary};
use crate::records::{records_reader, RecordBatch, RecordsOptions, RecordsSummary};
use crate::targets::TargetSet;
use crate::{Error, Result};

/// The `checksum` value that looks the expected sum up in `CHECKSUM.txt`.
pub const AUTO: &str = "auto";

/// A run: [`ProfileOptions`] plus where the input comes from and where the
/// rows go.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// A `.xml.gz` / `.xml` path, or an `http(s)://` URL (feature `http`).
    pub source: String,
    /// Output: `.parquet` or `.jsonl`, written atomically.
    pub out: PathBuf,
    /// The expected SHA-256, or `"auto"`: the sum `CHECKSUM.txt` gives for
    /// the source's file name (`discogs_<dump>_<kind>.xml.gz`). For a URL,
    /// the `CHECKSUM.txt` beside it; for a path, the one beside the file
    /// when there is one, else the data site's. Overrides
    /// `profile.checksum`.
    pub checksum: Option<String>,
    /// The skip-unchanged state file (JSON; created when missing).
    pub state: Option<PathBuf>,
    /// URL sources: also write the downloaded bytes here, as they stream
    /// (to `<keep>.partial`, renamed once the run succeeds).
    pub keep: Option<PathBuf>,
    /// URL sources: extra whole tries after a dropped connection, a
    /// timeout, a 5xx or a 429. Default 2.
    pub retries: u32,
    /// The data site (default `https://data.discogs.com/`): where `auto`
    /// finds a local file's `CHECKSUM.txt` when none is beside it.
    pub base_url: Option<String>,
    /// URL sources: the longest wait for a connection, a response or the
    /// next bytes. Default 120 s.
    pub timeout: Option<Duration>,
    /// Threads, block size, kind, `no_hash`.
    pub profile: ProfileOptions,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            source: String::new(),
            out: PathBuf::new(),
            checksum: None,
            state: None,
            keep: None,
            retries: 2,
            base_url: None,
            timeout: None,
            profile: ProfileOptions::default(),
        }
    }
}

/// What a run did: the profile's [`Summary`] and the run around it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RunSummary {
    #[serde(flatten)]
    pub profile: Summary,
    /// Nothing was read: the state file has this checksum, these targets
    /// and these options, and the output is there. The counts are then the
    /// last run's.
    pub skipped: bool,
    /// The source as given.
    pub source: String,
    /// The expected checksum this run used (given or looked up), if any.
    pub expected_sha256: Option<String>,
    /// Seconds waiting on the network (requests and body reads), over all
    /// attempts; 0 for a file.
    pub download_seconds: f64,
    /// Tries of the source (1 unless a transfer broke off; 0 when skipped).
    pub attempts: u32,
    /// Seconds writing the output.
    pub write_seconds: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    version: u32,
    entries: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    /// The source's file name (or the source) last run.
    file: String,
    sha256: String,
    /// The options hash.
    options: String,
    /// The targets' digest.
    targets: String,
    kind: String,
    records: u64,
    matched: u64,
    rows: u64,
    out: String,
    /// Unix seconds.
    finished_at: u64,
}

impl State {
    fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| Error::State(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                version: 1,
                ..Default::default()
            }),
            Err(e) => Err(Error::State(format!("{}: {e}", path.display()))),
        }
    }

    fn save(&self, path: &Path) -> Result<()> {
        let fail = |e: &dyn std::fmt::Display| Error::State(format!("{}: {e}", path.display()));
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".partial");
        let tmp = PathBuf::from(tmp);
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| fail(&e))?;
        std::fs::write(&tmp, bytes).map_err(|e| fail(&e))?;
        std::fs::rename(&tmp, path).map_err(|e| fail(&e))
    }
}

fn file_name(source: &str) -> String {
    let tail = source.rsplit(['/', '\\']).next().unwrap_or(source);
    if tail.is_empty() {
        source.to_string()
    } else {
        tail.to_string()
    }
}

/// The state key of a source: its dump kind, or the source.
fn state_file_key(source: &str) -> (String, String) {
    match dump_file_in(source) {
        Some((name, _, kind)) => (kind, name),
        None => (source.to_string(), file_name(source)),
    }
}

fn options_hash(options: &RunOptions) -> String {
    let text = format!(
        "discogs-rs {}\nkind {}\nout {}\n",
        env!("CARGO_PKG_VERSION"),
        options.profile.kind.map_or("auto", |k| k.as_str()),
        options.out.display()
    );
    hex(&Sha256::digest(text.as_bytes()))
}

fn is_url(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

#[cfg(feature = "http")]
fn http(options: &RunOptions) -> Result<crate::remote::Http> {
    let mut o = crate::remote::HttpOptions {
        retries: options.retries,
        ..Default::default()
    };
    if let Some(base) = &options.base_url {
        o.base_url = base.clone();
    }
    if let Some(t) = options.timeout {
        o.timeout = t;
    }
    crate::remote::Http::new(o)
}

#[cfg(not(feature = "http"))]
fn no_http(source: &str) -> Error {
    Error::Usage(format!("{source}: built without the `http` feature"))
}

/// The expected checksum: given, or looked up (`auto`).
fn expected_checksum(options: &RunOptions) -> Result<Option<String>> {
    let given = options.checksum.as_deref().or(options.profile.checksum.as_deref());
    let Some(given) = given else { return Ok(None) };
    if !given.trim().eq_ignore_ascii_case(AUTO) {
        return Ok(Some(given.trim().to_ascii_lowercase()));
    }
    let source = &options.source;
    let Some((name, dump, _)) = dump_file_in(source) else {
        return Err(Error::Checksum(format!(
            "{source}: `auto` needs a Discogs dump file name (discogs_YYYYMMDD_<kind>.xml.gz)"
        )));
    };
    let (sums, from) = if is_url(source) {
        let url = dumps::checksum_url_beside(source).expect("the source names a dump file");
        #[cfg(feature = "http")]
        {
            (http(options)?.checksums_at(&url)?, url)
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = url;
            return Err(no_http(source));
        }
    } else {
        let beside = Path::new(source)
            .parent()
            .unwrap_or(Path::new(""))
            .join(checksum_file(&dump));
        if beside.is_file() {
            let text = std::fs::read_to_string(&beside)?;
            (parse_checksums(&text), beside.display().to_string())
        } else {
            #[cfg(feature = "http")]
            {
                let h = http(options)?;
                let url = dumps::download_url(&h.options().base_url, &dump, &checksum_file(&dump));
                (h.checksums_at(&url)?, url)
            }
            #[cfg(not(feature = "http"))]
            {
                return Err(Error::Checksum(format!("{}: not found", beside.display())));
            }
        }
    };
    match sums.get(&name) {
        Some(sum) => Ok(Some(sum.clone())),
        None => Err(Error::Checksum(format!("{from} lists no {name}"))),
    }
}

/// Reads through to a `<keep>.partial` file; [`Keep::finish`] renames it
/// to `<keep>`, and dropping it unfinished removes it.
#[cfg_attr(not(feature = "http"), allow(dead_code))]
struct Keep<R> {
    inner: R,
    file: Option<BufWriter<File>>,
    partial: PathBuf,
    path: PathBuf,
}

#[cfg_attr(not(feature = "http"), allow(dead_code))]
impl<R> Keep<R> {
    fn new(inner: R, path: &Path) -> Result<Self> {
        let mut partial = path.as_os_str().to_owned();
        partial.push(".partial");
        let partial = PathBuf::from(partial);
        let file =
            File::create(&partial).map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", partial.display())))?;
        Ok(Self {
            inner,
            file: Some(BufWriter::with_capacity(4 << 20, file)),
            partial,
            path: path.to_path_buf(),
        })
    }

    fn finish(mut self) -> Result<()> {
        let file = self.file.take().expect("finished once");
        file.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&self.partial, &self.path)?;
        Ok(())
    }
}

impl<R: Read> Read for Keep<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(f) = &mut self.file {
            f.write_all(&buf[..n])?;
        }
        Ok(n)
    }
}

impl<R> Drop for Keep<R> {
    fn drop(&mut self) {
        if self.file.take().is_some() {
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}

/// One try of the source.
fn attempt(
    options: &RunOptions,
    targets: &TargetSet,
    profile: &ProfileOptions,
    clock: &Arc<Mutex<Duration>>,
) -> Result<Profile> {
    let source = &options.source;
    if !is_url(source) {
        if options.keep.is_some() {
            return Err(Error::Usage(
                "keep is for URL sources: the file is already on disk".into(),
            ));
        }
        let file = File::open(source).map_err(|e| std::io::Error::new(e.kind(), format!("{source}: {e}")))?;
        return profile_reader(file, targets, profile);
    }
    #[cfg(feature = "http")]
    {
        let body = http(options)?.open(source)?.timed(clock.clone());
        match &options.keep {
            Some(path) => {
                let mut keep = Keep::new(body, path)?;
                let p = profile_reader(&mut keep, targets, profile)?;
                keep.finish()?;
                Ok(p)
            }
            None => profile_reader(body, targets, profile),
        }
    }
    #[cfg(not(feature = "http"))]
    {
        let _ = clock;
        Err(no_http(source))
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Runs a profile: skip-unchanged, the input (retried whole on a transport
/// failure), the output, the state. On any error nothing is written (no
/// output, no kept file, no state change).
pub fn run_profile(targets: &TargetSet, options: &RunOptions) -> Result<RunSummary> {
    let started = Instant::now();
    let expected = expected_checksum(options)?;
    let (key_file, file) = state_file_key(&options.source);
    let key = format!("{key_file}|{}", targets.digest());
    let opts_hash = options_hash(options);

    let mut state = match &options.state {
        Some(path) => Some(State::load(path)?),
        None => None,
    };
    if let (Some(state), Some(expected)) = (&state, &expected) {
        if let Some(e) = state.entries.get(&key) {
            if &e.sha256 == expected && e.options == opts_hash && options.out.exists() {
                return Ok(RunSummary {
                    profile: Summary {
                        kind: e.kind.clone(),
                        records: e.records,
                        matched: e.matched,
                        rows: e.rows,
                        sha256: e.sha256.clone(),
                        seconds: crate::Stages {
                            total: started.elapsed().as_secs_f64(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    skipped: true,
                    source: options.source.clone(),
                    expected_sha256: Some(expected.clone()),
                    download_seconds: 0.0,
                    attempts: 0,
                    write_seconds: 0.0,
                });
            }
        }
    }

    let profile_options = ProfileOptions {
        checksum: expected.clone(),
        ..options.profile.clone()
    };
    let retries = if is_url(&options.source) { options.retries } else { 0 };
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let mut attempts = 0;
    let profile = loop {
        attempts += 1;
        match attempt(options, targets, &profile_options, &clock) {
            Err(Error::Network(_)) if attempts <= retries => continue,
            other => break other?,
        }
    };

    let t = Instant::now();
    write_rows(&profile.rows, &options.out)?;
    let write_seconds = t.elapsed().as_secs_f64();

    let s = &profile.summary;
    if let (Some(state), Some(path)) = (&mut state, &options.state) {
        if !s.sha256.is_empty() {
            state.version = 1;
            state.entries.insert(
                key,
                Entry {
                    file,
                    sha256: s.sha256.clone(),
                    options: opts_hash,
                    targets: targets.digest(),
                    kind: s.kind.clone(),
                    records: s.records,
                    matched: s.matched,
                    rows: s.rows,
                    out: options.out.display().to_string(),
                    finished_at: now_unix(),
                },
            );
            state.save(path)?;
        }
    }
    let download_seconds = if is_url(&options.source) {
        clock.lock().unwrap_or_else(|p| p.into_inner()).as_secs_f64()
    } else {
        0.0
    };
    Ok(RunSummary {
        profile: profile.summary,
        skipped: false,
        source: options.source.clone(),
        expected_sha256: expected,
        download_seconds,
        attempts,
        write_seconds,
    })
}

/// A records run ([`crate::records`]): where the input comes from.
#[derive(Clone, Debug)]
pub struct RecordsRunOptions {
    /// A `.xml.gz` / `.xml` path, or an `http(s)://` URL (feature `http`).
    pub source: String,
    /// The expected SHA-256, or `"auto"` (as [`RunOptions::checksum`]).
    /// Checked once the whole input is read: on a mismatch the run fails
    /// after the batches were handed over.
    pub checksum: Option<String>,
    /// URL sources: extra whole tries after a transport failure, only while
    /// no batch has been handed over yet. Default 2.
    pub retries: u32,
    /// The data site, for `auto` on a file with no `CHECKSUM.txt` beside it.
    pub base_url: Option<String>,
    /// URL sources: the longest wait for a connection or the next bytes.
    pub timeout: Option<Duration>,
    pub records: RecordsOptions,
}

impl RecordsRunOptions {
    pub fn new(source: impl Into<String>, records: RecordsOptions) -> Self {
        Self {
            source: source.into(),
            checksum: None,
            retries: 2,
            base_url: None,
            timeout: None,
            records,
        }
    }

    /// The profile run options that carry the same source settings.
    fn as_run(&self) -> RunOptions {
        RunOptions {
            source: self.source.clone(),
            checksum: self.checksum.clone(),
            retries: self.retries,
            base_url: self.base_url.clone(),
            timeout: self.timeout,
            ..Default::default()
        }
    }
}

/// What a records run did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RecordsRunSummary {
    #[serde(flatten)]
    pub records: RecordsSummary,
    pub source: String,
    pub expected_sha256: Option<String>,
    /// Seconds waiting on the network; 0 for a file.
    pub download_seconds: f64,
    pub attempts: u32,
}

/// Reads records from a path or URL, the expected checksum given or looked
/// up (`auto`), handing batches to `sink` in file order (`false` stops).
pub fn run_records(
    options: &RecordsRunOptions,
    sink: &mut dyn FnMut(RecordBatch) -> bool,
) -> Result<RecordsRunSummary> {
    let run = options.as_run();
    let expected = expected_checksum(&run)?;
    let records = RecordsOptions {
        checksum: expected.clone(),
        ..options.records.clone()
    };
    let url = is_url(&options.source);
    let retries = if url { options.retries } else { 0 };
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let mut attempts = 0;
    let mut delivered = false;
    let summary = loop {
        attempts += 1;
        let mut counted = |b: RecordBatch| {
            delivered = true;
            sink(b)
        };
        let tried = if url {
            #[cfg(feature = "http")]
            {
                http(&run)?
                    .open(&options.source)
                    .and_then(|body| records_reader(body.timed(clock.clone()), &records, &mut counted))
            }
            #[cfg(not(feature = "http"))]
            {
                Err(no_http(&options.source))
            }
        } else {
            File::open(&options.source)
                .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", options.source)).into())
                .and_then(|f| records_reader(f, &records, &mut counted))
        };
        match tried {
            Err(Error::Network(_)) if attempts <= retries && !delivered => continue,
            other => break other?,
        }
    };
    let download_seconds = if url {
        clock.lock().unwrap_or_else(|p| p.into_inner()).as_secs_f64()
    } else {
        0.0
    };
    Ok(RecordsRunSummary {
        records: summary,
        source: options.source.clone(),
        expected_sha256: expected,
        download_seconds,
        attempts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_keys() {
        assert_eq!(
            state_file_key("https://data.discogs.com/?download=data%2F2026%2Fdiscogs_20261101_masters.xml.gz"),
            ("masters".into(), "discogs_20261101_masters.xml.gz".into())
        );
        assert_eq!(
            state_file_key("/data/releases.xml"),
            ("/data/releases.xml".into(), "releases.xml".into())
        );
    }
}
