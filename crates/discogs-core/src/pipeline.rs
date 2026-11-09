//! The profile pipeline: reader + sha256 ─▶ gunzip + splitter ─▶ N
//! scanners ─▶ merge.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, BufRead, Read};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::aggregate::{Aggregator, Row};
use crate::scan::{Counters, RecordKind, Scanner};
use crate::split::{Splitter, DEFAULT_BLOCK_SIZE};
use crate::targets::TargetSet;
use crate::{Error, Result};

/// The reader's chunk size.
const CHUNK: usize = 4 << 20;
/// Chunks in flight between the reader and the decompressor.
const CHUNKS_IN_FLIGHT: usize = 8;

/// How to run a profile.
#[derive(Clone, Debug, Default)]
pub struct ProfileOptions {
    /// Scanner threads; 0 (the default) is the cores less one, at least 1.
    pub threads: usize,
    /// Block size in bytes; 0 is [`DEFAULT_BLOCK_SIZE`].
    pub block_size: usize,
    /// The input's expected SHA-256 (64 hex digits, any case). On a
    /// mismatch the run fails and returns no rows.
    pub checksum: Option<String>,
    /// The dump's kind; `None` reads it from the root element.
    pub kind: Option<RecordKind>,
    /// Skip the SHA-256 (the summary's `sha256` is then empty). Ignored
    /// when `checksum` is set. For benchmarks of the later stages.
    pub no_hash: bool,
}

impl ProfileOptions {
    /// The scanner threads this run uses.
    pub fn scanner_threads(&self) -> usize {
        if self.threads > 0 {
            return self.threads;
        }
        std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1))
    }
}

/// Seconds spent per stage. `scan` is CPU seconds summed over the scanner
/// threads; the others are one thread's busy time; `total` is wall time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Stages {
    pub read: f64,
    pub hash: f64,
    pub decompress: f64,
    pub split: f64,
    pub scan: f64,
    pub merge: f64,
    pub total: f64,
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Summary {
    /// `releases` or `masters`.
    pub kind: String,
    /// Records scanned.
    pub records: u64,
    /// Records whose main artists include a target.
    pub hits: u64,
    /// Hits counted.
    pub matched: u64,
    pub skipped_various: u64,
    pub skipped_master: u64,
    pub skipped_status: u64,
    pub skipped_quality: u64,
    /// Bytes read from the input (compressed, for a .gz).
    pub bytes_in: u64,
    /// Bytes of XML.
    pub bytes_decompressed: u64,
    /// Output rows.
    pub rows: u64,
    /// The input's SHA-256, lowercase hex (empty with `no_hash`).
    pub sha256: String,
    /// `zlib-rs` for gzip input, `none` for plain XML.
    pub decompressor: String,
    pub threads: usize,
    pub block_size: usize,
    pub seconds: Stages,
    /// MB (10^6 bytes) of XML per second of wall time.
    pub mb_per_s: f64,
}

/// The rows of a run and what it did.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub rows: Vec<Row>,
    pub summary: Summary,
}

/// Profiles a dump file (`.xml.gz` or `.xml`, told apart by their first
/// bytes).
pub fn profile_path(path: impl AsRef<Path>, targets: &TargetSet, options: &ProfileOptions) -> Result<Profile> {
    let file =
        File::open(path.as_ref()).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.as_ref().display())))?;
    profile_reader(file, targets, options)
}

fn parse_checksum(s: &str) -> Result<String> {
    let s = s.trim().to_ascii_lowercase();
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(s)
    } else {
        Err(Error::BadChecksum(s))
    }
}

/// The first error any thread hit, and a flag that tells the others to stop.
#[derive(Default)]
struct Failure {
    error: Mutex<Option<Error>>,
    stop: AtomicBool,
}

impl Failure {
    fn set(&self, e: Error) {
        let mut slot = self.error.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() {
            *slot = Some(e);
        }
        self.stop.store(true, Ordering::Relaxed);
    }
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

struct ReadStats {
    bytes: u64,
    read: Duration,
    hash: Duration,
    sha256: String,
}

struct SplitStats {
    bytes: u64,
    decompress: Duration,
    split: Duration,
    gzip: bool,
}

/// Profiles a dump read from `input`.
pub fn profile_reader<R: Read + Send>(input: R, targets: &TargetSet, options: &ProfileOptions) -> Result<Profile> {
    let expected = options.checksum.as_deref().map(parse_checksum).transpose()?;
    let threads = options.scanner_threads();
    let block_size = if options.block_size == 0 {
        DEFAULT_BLOCK_SIZE
    } else {
        options.block_size
    };
    let started = Instant::now();
    let failure = Failure::default();
    let kind_cell: OnceLock<RecordKind> = OnceLock::new();
    if let Some(k) = options.kind {
        let _ = kind_cell.set(k);
    }

    let (chunk_tx, chunk_rx) = bounded::<Vec<u8>>(CHUNKS_IN_FLIGHT);
    let (chunk_back_tx, chunk_back_rx) = unbounded::<Vec<u8>>();
    let (block_tx, block_rx) = bounded::<Vec<u8>>(threads);
    let (block_back_tx, block_back_rx) = unbounded::<Vec<u8>>();

    let (read_stats, split_stats, scanned) = std::thread::scope(|s| {
        let failure = &failure;
        let kind_cell = &kind_cell;
        let hash = expected.is_some() || !options.no_hash;
        let reader = s.spawn(move || read_and_hash(input, hash, chunk_tx, chunk_back_rx, failure));
        let splitter = s.spawn(move || {
            decompress_and_split(
                chunk_rx,
                chunk_back_tx,
                block_tx,
                block_back_rx,
                block_size,
                kind_cell,
                failure,
            )
        });
        let scanners: Vec<_> = (0..threads)
            .map(|_| {
                let rx = block_rx.clone();
                let back = block_back_tx.clone();
                s.spawn(move || scan_blocks(rx, back, targets, kind_cell, failure))
            })
            .collect();
        drop(block_rx);
        drop(block_back_tx);
        let read_stats = reader.join().map_err(|_| Error::Panic);
        let split_stats = splitter.join().map_err(|_| Error::Panic);
        let scanned: Result<Vec<_>> = scanners
            .into_iter()
            .map(|h| h.join().map_err(|_| Error::Panic))
            .collect();
        (read_stats, split_stats, scanned)
    });
    let read_stats = read_stats?;
    let split_stats = split_stats?;
    let scanned = scanned?;
    if let Some(e) = failure.error.into_inner().unwrap_or_else(|p| p.into_inner()) {
        return Err(e);
    }
    if let Some(expected) = expected {
        if expected != read_stats.sha256 {
            return Err(Error::ChecksumMismatch {
                expected,
                actual: read_stats.sha256,
            });
        }
    }

    let merge_started = Instant::now();
    let mut counters = Counters::default();
    let mut scan_time = Duration::ZERO;
    let mut agg = Aggregator::new();
    for (a, c, t) in scanned {
        counters.add(&c);
        scan_time += t;
        agg.merge(a);
    }
    let rows = agg.into_rows();
    let merge = merge_started.elapsed();
    let total = started.elapsed().as_secs_f64();

    let summary = Summary {
        kind: kind_cell.get().map_or("unknown", |k| k.as_str()).to_string(),
        records: counters.records,
        hits: counters.hits,
        matched: counters.matched,
        skipped_various: counters.skipped_various,
        skipped_master: counters.skipped_master,
        skipped_status: counters.skipped_status,
        skipped_quality: counters.skipped_quality,
        bytes_in: read_stats.bytes,
        bytes_decompressed: split_stats.bytes,
        rows: rows.len() as u64,
        sha256: read_stats.sha256,
        decompressor: if split_stats.gzip { "zlib-rs" } else { "none" }.to_string(),
        threads,
        block_size,
        seconds: Stages {
            read: read_stats.read.as_secs_f64(),
            hash: read_stats.hash.as_secs_f64(),
            decompress: split_stats.decompress.as_secs_f64(),
            split: split_stats.split.as_secs_f64(),
            scan: scan_time.as_secs_f64(),
            merge: merge.as_secs_f64(),
            total,
        },
        mb_per_s: if total > 0.0 {
            split_stats.bytes as f64 / 1e6 / total
        } else {
            0.0
        },
    };
    Ok(Profile { rows, summary })
}

/// The reader thread: chunks of the input, hashed on the way.
fn read_and_hash<R: Read>(
    mut input: R,
    hash: bool,
    tx: Sender<Vec<u8>>,
    back: Receiver<Vec<u8>>,
    failure: &Failure,
) -> ReadStats {
    let mut hasher = Sha256::new();
    let mut stats = ReadStats {
        bytes: 0,
        read: Duration::ZERO,
        hash: Duration::ZERO,
        sha256: String::new(),
    };
    loop {
        if failure.stopped() {
            break;
        }
        let mut chunk = back.try_recv().unwrap_or_else(|_| Vec::with_capacity(CHUNK));
        chunk.clear();
        let t = Instant::now();
        let got = match (&mut input).take(CHUNK as u64).read_to_end(&mut chunk) {
            Ok(n) => n,
            Err(e) => {
                failure.set(input_error(e));
                break;
            }
        };
        stats.read += t.elapsed();
        if got == 0 {
            break;
        }
        stats.bytes += got as u64;
        if hash {
            let t = Instant::now();
            hasher.update(&chunk);
            stats.hash += t.elapsed();
        }
        if tx.send(chunk).is_err() {
            break;
        }
    }
    if hash {
        stats.sha256 = hex(&hasher.finalize());
    }
    stats
}

/// A read error of the input: [`Error::Network`] when it came from an
/// HTTP body (worth a retry), else [`Error::Io`].
fn input_error(e: io::Error) -> Error {
    #[cfg(feature = "http")]
    {
        crate::remote::classify(e)
    }
    #[cfg(not(feature = "http"))]
    {
        Error::Io(e)
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

/// The reader's chunks as one `BufRead`, timing how long it waits for them.
struct ChunkReader {
    rx: Receiver<Vec<u8>>,
    back: Sender<Vec<u8>>,
    cur: Vec<u8>,
    pos: usize,
    waited: Rc<Cell<Duration>>,
}

impl Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let avail = self.fill_buf()?;
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for ChunkReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.pos >= self.cur.len() {
            let t = Instant::now();
            let next = self.rx.recv();
            self.waited.set(self.waited.get() + t.elapsed());
            match next {
                Ok(chunk) => {
                    let old = std::mem::replace(&mut self.cur, chunk);
                    let _ = self.back.send(old);
                    self.pos = 0;
                }
                Err(_) => return Ok(&[]),
            }
        }
        Ok(&self.cur[self.pos..])
    }
    fn consume(&mut self, n: usize) {
        self.pos += n;
    }
}

/// A reader that times its reads (less the time spent waiting for input).
struct Timed<R> {
    inner: R,
    busy: Duration,
}

impl<R: Read> Read for Timed<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let t = Instant::now();
        let r = self.inner.read(buf);
        self.busy += t.elapsed();
        r
    }
}

/// The decompression thread: gunzip (or pass through) and cut into blocks.
#[allow(clippy::too_many_arguments)]
fn decompress_and_split(
    rx: Receiver<Vec<u8>>,
    back: Sender<Vec<u8>>,
    blocks: Sender<Vec<u8>>,
    blocks_back: Receiver<Vec<u8>>,
    block_size: usize,
    kind_cell: &OnceLock<RecordKind>,
    failure: &Failure,
) -> SplitStats {
    let waited = Rc::new(Cell::new(Duration::ZERO));
    let mut chunks = ChunkReader {
        rx,
        back,
        cur: Vec::new(),
        pos: 0,
        waited: waited.clone(),
    };
    let gzip = match chunks.fill_buf() {
        Ok(head) => head.starts_with(&[0x1f, 0x8b]),
        Err(e) => {
            failure.set(Error::Io(e));
            return SplitStats {
                bytes: 0,
                decompress: Duration::ZERO,
                split: Duration::ZERO,
                gzip: false,
            };
        }
    };
    let source: Box<dyn Read> = if gzip {
        Box::new(flate2::bufread::MultiGzDecoder::new(chunks))
    } else {
        Box::new(chunks)
    };
    let mut timed = Timed {
        inner: source,
        busy: Duration::ZERO,
    };

    // The kind decides the end tag: read it from the first bytes when not given.
    let mut head = Vec::new();
    if kind_cell.get().is_none() {
        if let Err(e) = (&mut timed).take(64 << 10).read_to_end(&mut head) {
            failure.set(Error::Io(e));
        } else {
            match RecordKind::sniff(&head) {
                Some(k) => {
                    let _ = kind_cell.set(k);
                }
                None => failure.set(Error::UnknownKind),
            }
        }
    }
    let kind = *kind_cell.get().unwrap_or(&RecordKind::Releases);
    let mut splitter = Splitter::new(io::Cursor::new(head).chain(&mut timed), kind.close_tag(), block_size);

    let mut split = Duration::ZERO;
    while !failure.stopped() {
        let buf = blocks_back
            .try_recv()
            .unwrap_or_else(|_| Vec::with_capacity(block_size + (1 << 20)));
        let t = Instant::now();
        match splitter.next_block(buf) {
            Ok(Some(block)) => {
                split += t.elapsed();
                if blocks.send(block).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                failure.set(Error::Io(e));
                break;
            }
        }
    }
    let bytes = splitter.bytes;
    drop(splitter);
    let decompress = timed.busy.saturating_sub(waited.get());
    // `split` included the reads; keep only the cutting.
    let split = split.saturating_sub(timed.busy);
    SplitStats {
        bytes,
        decompress,
        split,
        gzip,
    }
}

/// A scanner thread: scan blocks until the channel closes.
fn scan_blocks(
    rx: Receiver<Vec<u8>>,
    back: Sender<Vec<u8>>,
    targets: &TargetSet,
    kind_cell: &OnceLock<RecordKind>,
    failure: &Failure,
) -> (Aggregator, Counters, Duration) {
    let mut agg = Aggregator::new();
    let mut counters = Counters::default();
    let mut busy = Duration::ZERO;
    let mut scanner: Option<Scanner> = None;
    for block in rx {
        if failure.stopped() {
            continue;
        }
        let t = Instant::now();
        // The kind is set before the first block is sent.
        let s = scanner.get_or_insert_with(|| Scanner::new(*kind_cell.get().unwrap_or(&RecordKind::Releases)));
        s.scan_block(&block, targets, &mut agg, &mut counters);
        busy += t.elapsed();
        let _ = back.send(block);
    }
    (agg, counters, busy)
}
