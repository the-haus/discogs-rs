//! The records scanner on arbitrary bytes: it must never panic, and
//! splitting the input into blocks after the file's end tag, at any size,
//! must find the same records (ids and bodies) as one scan of the whole.
//!
//! The first byte picks the file (its value mod 3) and the block size.
//!
//!   cargo +nightly fuzz run records -- -dict=fuzz/discogs.dict

#![no_main]

use discogs_core::split::Splitter;
use discogs_core::{RecordFile, RecordScanner};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&ctl, input)) = data.split_first() else {
        return;
    };
    let file = RecordFile::ALL[(ctl % 3) as usize];
    let block_size = 1 + (ctl / 3) as usize;
    let scanner = RecordScanner::new(file);

    let whole: Vec<(u64, &[u8])> = scanner.records(input).map(|f| (f.id, &input[f.start..f.end])).collect();

    let mut splitter = Splitter::new(input, file.close_tag(), block_size);
    let mut blocks = Vec::new();
    while let Some(block) = splitter.next_block(Vec::new()).unwrap() {
        blocks.push(block);
    }
    assert_eq!(blocks.concat(), input);
    let split: Vec<(u64, &[u8])> = blocks
        .iter()
        .flat_map(|b| scanner.records(b).map(move |f| (f.id, &b[f.start..f.end])))
        .collect();
    assert_eq!(whole, split);
});
