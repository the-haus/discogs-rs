//! Records by id: the raw XML of the `masters`, `artists` and `labels`
//! records whose ids are in a set (or of every record), in file order.
//!
//! The same pipeline as a profile (reader + sha256 ─▶ gunzip + splitter ─▶
//! N scanners), but the scanners copy each wanted record's bytes out
//! instead of counting, and a collector hands the blocks' records to the
//! caller **in file order** (blocks finish out of order; a bounded number
//! of tickets keeps the blocks in flight, and so memory, flat).
//!
//! A record is what Festimap's TypeScript scan (`dump.ts`) cuts out, byte
//! for byte, so one parser reads both:
//!
//! - it starts at a match of the file's start pattern — `<master id="N">`,
//!   `<artist><id>N</id>`, `<label><id>N</id>` (top-level records only: a
//!   nested artist or label carries its id as an attribute);
//! - its body runs from that match to the first end tag after it
//!   (`</master>`, `</artist>`, `</label>`), the end tag left out. A label's
//!   body stops at its first sub-label's `</label>`, after its id, name,
//!   profile, URLs and parent;
//! - the search for the next record resumes after that end tag.
//!
//! The splitter cuts blocks right after an end tag, so a block scan finds
//! exactly the records a scan of the whole input finds (no start match can
//! hold an end tag, nor end inside one; the fuzz target `records` checks
//! it on arbitrary bytes). A record with no end tag before the input ends
//! is dropped, as the TypeScript scan drops it.
//!
//! The dumps are written in id order. A record whose id is below the one
//! before it fails the run ([`Error::Dump`]), as it fails the TypeScript
//! merge.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, unbounded};
use memchr::memmem::Finder;
use serde::Serialize;

use crate::pipeline::{decompress_and_split, parse_checksum, read_and_hash, Failure, CHUNKS_IN_FLIGHT};
use crate::split::DEFAULT_BLOCK_SIZE;
use crate::targets::{parse_digits, TargetSet};
use crate::{Error, Result};

/// A dump file read by records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecordFile {
    /// `discogs_*_masters.xml.gz`: `<master id="N">…</master>`.
    Masters,
    /// `discogs_*_artists.xml.gz`: `<artist><id>N</id>…</artist>`.
    Artists,
    /// `discogs_*_labels.xml.gz`: `<label><id>N</id>…</label>`.
    Labels,
}

impl RecordFile {
    /// Every file records reads.
    pub const ALL: [RecordFile; 3] = [RecordFile::Masters, RecordFile::Artists, RecordFile::Labels];

    /// `masters`, `artists` or `labels`.
    pub fn as_str(self) -> &'static str {
        match self {
            RecordFile::Masters => "masters",
            RecordFile::Artists => "artists",
            RecordFile::Labels => "labels",
        }
    }

    /// The start pattern: these bytes, ASCII digits (the id), then
    /// [`RecordFile::after_id`].
    pub fn before_id(self) -> &'static [u8] {
        match self {
            RecordFile::Masters => b"<master id=\"",
            RecordFile::Artists => b"<artist><id>",
            RecordFile::Labels => b"<label><id>",
        }
    }

    /// What follows the id in the start pattern.
    pub fn after_id(self) -> &'static [u8] {
        match self {
            RecordFile::Masters => b"\">",
            RecordFile::Artists | RecordFile::Labels => b"</id>",
        }
    }

    /// The record's end tag.
    pub fn close_tag(self) -> &'static [u8] {
        match self {
            RecordFile::Masters => b"</master>",
            RecordFile::Artists => b"</artist>",
            RecordFile::Labels => b"</label>",
        }
    }
}

impl std::str::FromStr for RecordFile {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "masters" | "master" => Ok(RecordFile::Masters),
            "artists" | "artist" => Ok(RecordFile::Artists),
            "labels" | "label" => Ok(RecordFile::Labels),
            _ => Err(format!("unknown records file {s:?} (masters, artists or labels)")),
        }
    }
}

impl std::fmt::Display for RecordFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A record found in a block: its id, its body (`start..end`, from the
/// start match to the end tag, the end tag left out), and where the search
/// for the next one resumes (`next`, after the end tag).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub id: u64,
    pub start: usize,
    pub end: usize,
    pub next: usize,
}

/// Finds a file's records in decompressed bytes.
pub struct RecordScanner {
    file: RecordFile,
    open: Finder<'static>,
    close: Finder<'static>,
}

impl RecordScanner {
    pub fn new(file: RecordFile) -> Self {
        Self {
            file,
            open: Finder::new(file.before_id()),
            close: Finder::new(file.close_tag()),
        }
    }

    pub fn file(&self) -> RecordFile {
        self.file
    }

    /// The first whole record starting at or after `from`, or `None` (no
    /// start match, or none with an end tag after it).
    pub fn next_record(&self, block: &[u8], mut from: usize) -> Option<Found> {
        let before = self.file.before_id();
        let after = self.file.after_id();
        while from < block.len() {
            let p = from + self.open.find(&block[from..])?;
            let d = p + before.len();
            if let Some((id, n)) = parse_digits(&block[d..]) {
                if block[d + n..].starts_with(after) {
                    let body = d + n + after.len();
                    let c = body + self.close.find(&block[body..])?;
                    return Some(Found {
                        id,
                        start: p,
                        end: c,
                        next: c + self.file.close_tag().len(),
                    });
                }
            }
            from = p + 1;
        }
        None
    }

    /// Every whole record of a block, in order.
    pub fn records<'a>(&'a self, block: &'a [u8]) -> impl Iterator<Item = Found> + 'a {
        let mut from = 0;
        std::iter::from_fn(move || {
            let f = self.next_record(block, from)?;
            from = f.next;
            Some(f)
        })
    }
}

/// Records handed over together: `ids[i]`'s body is
/// `xml[offsets[i]..offsets[i + 1]]` (`offsets` has one more entry than
/// `ids`). In file order, within and across batches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecordBatch {
    pub ids: Vec<u32>,
    pub offsets: Vec<u32>,
    pub xml: Vec<u8>,
}

impl RecordBatch {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The `i`th record: its id and body.
    pub fn get(&self, i: usize) -> (u32, &[u8]) {
        (
            self.ids[i],
            &self.xml[self.offsets[i] as usize..self.offsets[i + 1] as usize],
        )
    }

    /// The records, in order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &[u8])> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }

    fn push(&mut self, id: u32, body: &[u8]) -> Result<()> {
        if self.offsets.is_empty() {
            self.offsets.push(0);
        }
        self.xml.extend_from_slice(body);
        let end = u32::try_from(self.xml.len()).map_err(|_| Error::Dump("a block's records exceed 4 GB".into()))?;
        self.ids.push(id);
        self.offsets.push(end);
        Ok(())
    }
}

/// What a block's scan found.
#[derive(Default)]
struct BlockOut {
    batch: RecordBatch,
    records: u64,
    first: Option<u64>,
    last: Option<u64>,
}

fn out_of_order(file: RecordFile, id: u64, after: u64) -> Error {
    Error::Dump(format!(
        "Discogs {file}: the dump is not in id order ({id} after {after})"
    ))
}

/// Scans one block: every record counted and order-checked, the wanted
/// ones (in `ids`, or all without) copied out.
fn scan_block(scanner: &RecordScanner, block: &[u8], ids: Option<&TargetSet>) -> Result<BlockOut> {
    let mut out = BlockOut::default();
    for f in scanner.records(block) {
        out.records += 1;
        if let Some(last) = out.last {
            if f.id < last {
                return Err(out_of_order(scanner.file(), f.id, last));
            }
        }
        out.first.get_or_insert(f.id);
        out.last = Some(f.id);
        let wanted = match ids {
            Some(set) => set.contains(f.id),
            None => true,
        };
        if wanted {
            let id = u32::try_from(f.id)
                .map_err(|_| Error::Dump(format!("Discogs {}: id {} is above 2^32", scanner.file(), f.id)))?;
            out.batch.push(id, &block[f.start..f.end])?;
        }
    }
    Ok(out)
}

/// How to read records.
#[derive(Clone, Debug)]
pub struct RecordsOptions {
    pub file: RecordFile,
    /// The ids wanted; `None` is every record.
    pub ids: Option<TargetSet>,
    /// Scanner threads; 0 (the default) is the cores less one, at least 1.
    pub threads: usize,
    /// Block size in bytes; 0 is [`DEFAULT_BLOCK_SIZE`].
    pub block_size: usize,
    /// The input's expected SHA-256. It is known only once the input is
    /// read, after every batch was handed over: on a mismatch the run
    /// fails at the end, and a caller that kept the batches must drop them.
    pub checksum: Option<String>,
    /// Skip the SHA-256 (ignored when `checksum` is set).
    pub no_hash: bool,
    /// Set from another thread to stop the run: the input reads as ended,
    /// and the run returns `cancelled: true` (whatever the half-read input
    /// made of the gunzip), without checking the checksum.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl RecordsOptions {
    pub fn new(file: RecordFile) -> Self {
        Self {
            file,
            ids: None,
            threads: 0,
            block_size: 0,
            checksum: None,
            no_hash: false,
            cancel: None,
        }
    }

    /// The scanner threads this run uses.
    pub fn scanner_threads(&self) -> usize {
        if self.threads > 0 {
            return self.threads;
        }
        std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1))
    }
}

/// Seconds per stage. `scan` is CPU seconds over the scanner threads;
/// `deliver` the time the caller took with the batches; `total` wall time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct RecordStages {
    pub read: f64,
    pub hash: f64,
    pub decompress: f64,
    pub split: f64,
    pub scan: f64,
    pub deliver: f64,
    pub total: f64,
}

/// What a records run did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RecordsSummary {
    /// `masters`, `artists` or `labels`.
    pub file: String,
    /// Records scanned.
    pub records: u64,
    /// Records handed over.
    pub matched: u64,
    /// Batches handed over.
    pub batches: u64,
    /// Ids asked for (0 for every record).
    pub ids: u64,
    pub bytes_in: u64,
    pub bytes_decompressed: u64,
    /// The input's SHA-256 (empty with `no_hash`, or when cancelled).
    pub sha256: String,
    /// `zlib-rs`, or `none` for plain XML.
    pub decompressor: String,
    pub threads: usize,
    pub block_size: usize,
    pub seconds: RecordStages,
    /// MB (10^6 bytes) of XML per wall second.
    pub mb_per_s: f64,
    /// The caller stopped the run (its sink said so): the rest of the
    /// input was not read and the checksum not checked.
    pub cancelled: bool,
}

/// Reads records from a file (`.xml.gz` or `.xml`).
pub fn records_path(
    path: impl AsRef<Path>,
    options: &RecordsOptions,
    sink: &mut dyn FnMut(RecordBatch) -> bool,
) -> Result<RecordsSummary> {
    let file =
        File::open(path.as_ref()).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.as_ref().display())))?;
    records_reader(file, options, sink)
}

/// Records as `(id, body)` pairs.
pub type OwnedRecords = Vec<(u32, Vec<u8>)>;

/// Every record of a file in one `Vec` (tests, small files).
pub fn collect_records(path: impl AsRef<Path>, options: &RecordsOptions) -> Result<(OwnedRecords, RecordsSummary)> {
    let mut out = Vec::new();
    let summary = records_path(path, options, &mut |b| {
        out.extend(b.iter().map(|(id, xml)| (id, xml.to_vec())));
        true
    })?;
    Ok((out, summary))
}

/// An input that reads as ended once its flag is set.
struct Cancellable<R> {
    inner: R,
    flag: Arc<AtomicBool>,
}

impl<R: Read> Read for Cancellable<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.flag.load(Ordering::Relaxed) {
            return Ok(0);
        }
        self.inner.read(buf)
    }
}

/// Reads records from `input`, handing each non-empty batch to `sink` in
/// file order (on the calling thread). `sink` returns `false` to stop: the
/// run then ends early with `cancelled: true`.
pub fn records_reader<R: Read + Send>(
    input: R,
    options: &RecordsOptions,
    sink: &mut dyn FnMut(RecordBatch) -> bool,
) -> Result<RecordsSummary> {
    let expected = options.checksum.as_deref().map(parse_checksum).transpose()?;
    let threads = options.scanner_threads();
    let block_size = if options.block_size == 0 {
        DEFAULT_BLOCK_SIZE
    } else {
        options.block_size
    };
    let file = options.file;
    let ids = options.ids.as_ref();
    let started = Instant::now();
    let failure = Failure::default();
    let flag = options.cancel.clone().unwrap_or_default();
    let input = Cancellable {
        inner: input,
        flag: flag.clone(),
    };

    let (chunk_tx, chunk_rx) = bounded::<Vec<u8>>(CHUNKS_IN_FLIGHT);
    let (chunk_back_tx, chunk_back_rx) = unbounded::<Vec<u8>>();
    let (block_tx, block_rx) = bounded::<(u64, Vec<u8>)>(threads);
    let (block_back_tx, block_back_rx) = unbounded::<Vec<u8>>();
    let (out_tx, out_rx) = unbounded::<(u64, Result<BlockOut>)>();
    // Blocks in flight, from the cut to the hand-over: the splitter takes a
    // ticket per block, the collector gives it back once the block's
    // records are handed over in order.
    let tickets = threads * 2 + 2;
    let (ticket_tx, ticket_rx) = bounded::<()>(tickets);
    for _ in 0..tickets {
        let _ = ticket_tx.send(());
    }

    let mut records = 0u64;
    let mut matched = 0u64;
    let mut batches = 0u64;
    let mut deliver = Duration::ZERO;
    let mut cancelled = false;

    let (read_stats, split_stats, scanned) = std::thread::scope(|s| {
        let failure = &failure;
        let hash = expected.is_some() || !options.no_hash;
        let reader = s.spawn(move || read_and_hash(input, hash, chunk_tx, chunk_back_rx, failure));
        let splitter = s.spawn(move || {
            let mut seq = 0u64;
            decompress_and_split(
                chunk_rx,
                chunk_back_tx,
                block_back_rx,
                block_size,
                failure,
                |_| Ok(file.close_tag()),
                |block| {
                    if ticket_rx.recv().is_err() || failure.stopped() {
                        return false;
                    }
                    let ok = block_tx.send((seq, block)).is_ok();
                    seq += 1;
                    ok
                },
            )
        });
        let scanners: Vec<_> = (0..threads)
            .map(|_| {
                let rx = block_rx.clone();
                let back = block_back_tx.clone();
                let out = out_tx.clone();
                s.spawn(move || {
                    let scanner = RecordScanner::new(file);
                    let mut busy = Duration::ZERO;
                    for (seq, block) in rx {
                        if failure.stopped() {
                            continue;
                        }
                        let t = Instant::now();
                        let found = scan_block(&scanner, &block, ids);
                        busy += t.elapsed();
                        let _ = back.send(block);
                        if out.send((seq, found)).is_err() {
                            break;
                        }
                    }
                    busy
                })
            })
            .collect();
        drop(block_rx);
        drop(block_back_tx);
        drop(out_tx);

        // The collector: blocks back in file order, handed over.
        let mut pending: BTreeMap<u64, BlockOut> = BTreeMap::new();
        let mut next = 0u64;
        let mut last_id: Option<u64> = None;
        'collect: for (seq, found) in &out_rx {
            match found {
                Ok(out) => {
                    pending.insert(seq, out);
                }
                Err(e) => {
                    failure.set(e);
                    break;
                }
            }
            while let Some(out) = pending.remove(&next) {
                next += 1;
                if let (Some(last), Some(first)) = (last_id, out.first) {
                    if first < last {
                        failure.set(out_of_order(file, first, last));
                        break 'collect;
                    }
                }
                if out.last.is_some() {
                    last_id = out.last;
                }
                records += out.records;
                if !out.batch.is_empty() {
                    matched += out.batch.len() as u64;
                    batches += 1;
                    let t = Instant::now();
                    let go_on = sink(out.batch);
                    deliver += t.elapsed();
                    if !go_on || flag.load(Ordering::Relaxed) {
                        cancelled = true;
                        flag.store(true, Ordering::Relaxed);
                        failure.halt();
                        break 'collect;
                    }
                }
                let _ = ticket_tx.send(());
            }
        }
        // Unblock the splitter (waiting on a ticket) and the scanners.
        drop(ticket_tx);
        drop(out_rx);

        let read_stats = reader.join().map_err(|_| Error::Panic);
        let split_stats = splitter.join().map_err(|_| Error::Panic);
        let scanned: Result<Vec<Duration>> = scanners
            .into_iter()
            .map(|h| h.join().map_err(|_| Error::Panic))
            .collect();
        (read_stats, split_stats, scanned)
    });
    let read_stats = read_stats?;
    let split_stats = split_stats?;
    let scan: Duration = scanned?.into_iter().sum();
    // Cancelled from outside: the input ended early, so a gunzip error
    // after it is no failure.
    cancelled |= flag.load(Ordering::Relaxed);
    if let Some(e) = failure.error.into_inner().unwrap_or_else(|p| p.into_inner()) {
        if !cancelled {
            return Err(e);
        }
    }
    let mut sha256 = String::new();
    if !cancelled {
        if let Some(expected) = expected {
            if expected != read_stats.sha256 {
                return Err(Error::ChecksumMismatch {
                    expected,
                    actual: read_stats.sha256,
                });
            }
        }
        sha256 = read_stats.sha256;
    }
    let total = started.elapsed().as_secs_f64();
    Ok(RecordsSummary {
        file: file.as_str().to_string(),
        records,
        matched,
        batches,
        ids: ids.map_or(0, |s| s.len() as u64),
        bytes_in: read_stats.bytes,
        bytes_decompressed: split_stats.bytes,
        sha256,
        decompressor: if split_stats.gzip { "zlib-rs" } else { "none" }.to_string(),
        threads,
        block_size,
        seconds: RecordStages {
            read: read_stats.read.as_secs_f64(),
            hash: read_stats.hash.as_secs_f64(),
            decompress: split_stats.decompress.as_secs_f64(),
            split: split_stats.split.as_secs_f64(),
            scan: scan.as_secs_f64(),
            deliver: deliver.as_secs_f64(),
            total,
        },
        mb_per_s: if total > 0.0 {
            split_stats.bytes as f64 / 1e6 / total
        } else {
            0.0
        },
        cancelled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(file: RecordFile, input: &[u8]) -> Vec<(u64, String)> {
        let s = RecordScanner::new(file);
        s.records(input)
            .map(|f| (f.id, String::from_utf8_lossy(&input[f.start..f.end]).into_owned()))
            .collect()
    }

    #[test]
    fn start_patterns_and_bodies() {
        let x = b"<masters><master id=\"7\"><title>A</title></master>\n<master id=\"x\"></master><master id=\"9\">B</master><master id=\"10\">open";
        assert_eq!(
            all(RecordFile::Masters, x),
            [
                (7, "<master id=\"7\"><title>A</title>".to_string()),
                (9, "<master id=\"9\">B".to_string())
            ]
        );
        // A label's body stops at its first sub-label's end; the next
        // sub-labels are no records.
        let l = b"<label><id>1</id><name>P</name><sublabels><label id=\"2\">S</label><label id=\"3\">T</label></sublabels></label><label><id>4</id></label>";
        assert_eq!(
            all(RecordFile::Labels, l),
            [
                (
                    1,
                    "<label><id>1</id><name>P</name><sublabels><label id=\"2\">S".to_string()
                ),
                (4, "<label><id>4</id>".to_string())
            ]
        );
        let a = b"<artists><artist><id>12</id><name>N</name><aliases><name id=\"3\">Q</name></aliases></artist><artist><id></id></artist></artists>";
        assert_eq!(
            all(RecordFile::Artists, a),
            [(
                12,
                "<artist><id>12</id><name>N</name><aliases><name id=\"3\">Q</name></aliases>".to_string()
            )]
        );
    }

    #[test]
    fn names() {
        for f in RecordFile::ALL {
            assert_eq!(f.as_str().parse::<RecordFile>(), Ok(f));
        }
        assert!("releases".parse::<RecordFile>().is_err());
    }
}
