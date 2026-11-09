//! The record scanner on arbitrary bytes: it must never panic, and
//! splitting the input into blocks at any size must give the same rows and
//! counts as one scan of the whole.
//!
//! The first byte picks the dump kind (bit 0) and the block size (the rest).
//!
//!   cargo +nightly fuzz run scan -- -dict=fuzz/discogs.dict

#![no_main]

use discogs_core::split::Splitter;
use discogs_core::{format, xml, Aggregator, Counters, RecordKind, Scanner, TargetSet};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&ctl, input)) = data.split_first() else {
        return;
    };
    let kind = if ctl & 1 == 0 {
        RecordKind::Releases
    } else {
        RecordKind::Masters
    };
    let block_size = 1 + (ctl >> 1) as usize * 3;
    let targets = TargetSet::from_ids([0, 1, 2, 3, 10, 194, 4_000_000_000]);

    let mut scanner = Scanner::new(kind);
    let mut whole = Aggregator::new();
    let mut whole_c = Counters::default();
    scanner.scan_block(input, &targets, &mut whole, &mut whole_c);

    let mut split = Aggregator::new();
    let mut split_c = Counters::default();
    let mut splitter = Splitter::new(input, kind.close_tag(), block_size);
    let mut joined = Vec::new();
    while let Some(block) = splitter.next_block(Vec::new()).unwrap() {
        scanner.scan_block(&block, &targets, &mut split, &mut split_c);
        joined.extend_from_slice(&block);
    }
    assert_eq!(joined, input);
    assert_eq!(whole_c, split_c);
    assert_eq!(whole.into_rows(), split.into_rows());

    // The whole input as one (possibly truncated or malformed) record.
    let mut agg = Aggregator::new();
    scanner.scan_record(input, &targets, &mut agg, &mut Counters::default());
    let _ = agg.into_rows();
    let _ = xml::unescape(input);
    let _ = format::classify(input);
});
