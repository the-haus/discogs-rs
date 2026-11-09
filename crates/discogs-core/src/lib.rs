//! discogs-core: read the Discogs monthly data dumps fast.
//!
//! The pipeline ([`pipeline::profile_path`]):
//!
//! ```text
//! file ─▶ reader + sha256 ─▶ gunzip + splitter ─▶ N scanners ─▶ merge ─▶ rows
//! ```
//!
//! - a reader thread reads the file in 4 MB chunks and hashes them on the
//!   way (SHA-256, checked against an expected checksum at the end: on a
//!   mismatch the run fails and returns no rows);
//! - a decompression thread gunzips them (`flate2` on `zlib-rs`; plain XML
//!   passes through) straight into ~16 MB blocks, each cut after its last
//!   whole record ([`split`]);
//! - scanner threads take the blocks from a bounded channel (so memory
//!   stays flat), scan their records ([`scan`]) and sum rows per thread
//!   ([`aggregate`]);
//! - the per-thread sums are merged into sorted [`Row`]s, written by
//!   [`output`] as Parquet or JSON lines.
//!
//! [`run::run_profile`] wraps it for the CLI and the Node binding: a path or
//! an HTTPS URL ([`remote`], feature `http`, on by default), the expected
//! checksum looked up in the dump's `CHECKSUM.txt` ([`dumps`]), whole-file
//! retries, a copy of the download kept on disk, the output written, and a
//! state file that skips a dump already profiled with the same targets.

#![deny(clippy::all)]

pub mod aggregate;
pub mod dumps;
pub mod format;
pub mod generate;
pub mod output;
pub mod pipeline;
#[cfg(feature = "http")]
pub mod remote;
pub mod run;
pub mod scan;
pub mod split;
pub mod targets;
pub mod xml;

pub use aggregate::{Aggregator, Row, RowKind};
pub use format::FormatClass;
pub use pipeline::{profile_path, profile_reader, Profile, ProfileOptions, Stages, Summary};
pub use run::{run_profile, RunOptions, RunSummary};
pub use scan::{Counters, RecordKind, Scanner};
pub use targets::TargetSet;

/// Everything that can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("targets: {0}")]
    Targets(String),
    #[error("checksum mismatch: expected sha256 {expected}, the input has {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("not a sha256 checksum (64 hex digits): {0:?}")]
    BadChecksum(String),
    #[error("cannot tell a releases dump from a masters dump by its first bytes; give the kind")]
    UnknownKind,
    #[error("output: {0}")]
    Output(String),
    /// A transport failure (a dropped connection, a timeout, a 5xx): the
    /// file is tried again from the start, a few times.
    #[error("network: {0}")]
    Network(String),
    /// An HTTP answer that a retry will not change (a 404).
    #[error("http: {0}")]
    Http(String),
    /// No expected checksum could be found (`checksum: "auto"`).
    #[error("checksum: {0}")]
    Checksum(String),
    /// The state file (skip-unchanged) could not be read or written.
    #[error("state: {0}")]
    State(String),
    /// An option that makes no sense for this source.
    #[error("{0}")]
    Usage(String),
    #[error("a worker thread panicked")]
    Panic,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
