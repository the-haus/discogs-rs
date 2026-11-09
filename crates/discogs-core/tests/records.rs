//! Records by id over the fixtures: the same records as a port of
//! Festimap's TypeScript scan (`dump.ts`: chunks appended to a buffer, a
//! record carried whole into the next chunk), whatever the block size,
//! the thread count or the compression; ids filtered; order, checksum and
//! cancelling.

use std::io::{Cursor, Write};
use std::path::PathBuf;

use discogs_core::{
    collect_records, records_reader, Error, RecordBatch, RecordFile, RecordScanner, RecordsOptions, TargetSet,
};
use proptest::prelude::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

const FIXTURES: [(RecordFile, &str); 6] = [
    (RecordFile::Artists, "artists.xml"),
    (RecordFile::Artists, "real-artists-20261001-head.xml"),
    (RecordFile::Labels, "labels.xml"),
    (RecordFile::Labels, "real-labels-20261001-head.xml"),
    (RecordFile::Masters, "masters.xml"),
    (RecordFile::Masters, "real-masters-20261101-head.xml"),
];

/// `dump.ts`'s `scan`, ported as written: the start pattern searched from
/// the buffer's start after each chunk, the body up to the first end tag
/// after the match, what follows the last whole record kept for the next
/// chunk.
fn ts_scan(file: RecordFile, input: &[u8], chunk: usize) -> Vec<(u32, Vec<u8>)> {
    let before = file.before_id();
    let after = file.after_id();
    let end = file.close_tag();
    let find = |hay: &[u8], needle: &[u8], from: usize| -> Option<usize> {
        (from..=hay.len().saturating_sub(needle.len())).find(|&i| hay[i..].starts_with(needle))
    };
    // The regex `<before>(\d+)<after>`: its leftmost match at or after `from`.
    let start = |buf: &[u8], from: usize| -> Option<(usize, usize, u32)> {
        let mut at = from;
        while let Some(p) = find(buf, before, at) {
            let d = p + before.len();
            let n = buf[d..].iter().take_while(|b| b.is_ascii_digit()).count();
            if n > 0 && buf[d + n..].starts_with(after) {
                let id: u32 = std::str::from_utf8(&buf[d..d + n]).unwrap().parse().unwrap();
                return Some((p, d + n + after.len(), id));
            }
            at = p + 1;
        }
        None
    };
    let mut out = Vec::new();
    let mut buffer: Vec<u8> = Vec::new();
    for piece in input.chunks(chunk.max(1)) {
        buffer.extend_from_slice(piece);
        let mut consumed = 0;
        let mut last_index = 0;
        while let Some((p, match_end, id)) = start(&buffer, last_index) {
            let Some(close) = find(&buffer, end, match_end) else {
                consumed = p;
                break;
            };
            out.push((id, buffer[p..close].to_vec()));
            consumed = close + end.len();
            last_index = consumed;
        }
        buffer.drain(..consumed);
    }
    out
}

fn options(file: RecordFile) -> RecordsOptions {
    RecordsOptions {
        threads: 3,
        ..RecordsOptions::new(file)
    }
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn read_all(input: &[u8], options: &RecordsOptions) -> Result<Vec<(u32, Vec<u8>)>, Error> {
    let mut out = Vec::new();
    records_reader(Cursor::new(input.to_vec()), options, &mut |b: RecordBatch| {
        out.extend(b.iter().map(|(id, x)| (id, x.to_vec())));
        true
    })?;
    Ok(out)
}

#[test]
fn every_fixture_matches_the_typescript_scan() {
    for (file, name) in FIXTURES {
        let bytes = std::fs::read(fixture(name)).unwrap();
        let expected = ts_scan(file, &bytes, 1 << 20);
        assert!(expected.len() >= 4, "{name}: {}", expected.len());
        assert_eq!(ts_scan(file, &bytes, 7), expected, "{name}: the port is chunk-blind");

        let (got, summary) = collect_records(fixture(name), &options(file)).unwrap();
        assert_eq!(got, expected, "{name}");
        assert_eq!(summary.records, expected.len() as u64);
        assert_eq!(summary.matched, expected.len() as u64);
        assert_eq!(summary.decompressor, "none");
        assert_eq!(summary.file, file.as_str());

        for block_size in [1, 13, 100, 4096] {
            let o = RecordsOptions {
                block_size,
                ..options(file)
            };
            assert_eq!(read_all(&bytes, &o).unwrap(), expected, "{name} block {block_size}");
        }
        let gz = RecordsOptions {
            block_size: 1000,
            threads: 1,
            ..options(file)
        };
        assert_eq!(read_all(&gzip(&bytes), &gz).unwrap(), expected, "{name} gzip");
    }
}

#[test]
fn ids_filter_the_records() {
    let bytes = std::fs::read(fixture("real-artists-20261001-head.xml")).unwrap();
    let every = ts_scan(RecordFile::Artists, &bytes, 1 << 20);
    let wanted: Vec<u32> = every.iter().map(|r| r.0).filter(|id| id % 3 == 0).collect();
    let o = RecordsOptions {
        ids: Some(TargetSet::from_ids(wanted.iter().copied().chain([999_999_999]))),
        block_size: 2000,
        ..options(RecordFile::Artists)
    };
    let got = read_all(&bytes, &o).unwrap();
    assert_eq!(got.iter().map(|r| r.0).collect::<Vec<_>>(), wanted);
    let expected: Vec<_> = every.into_iter().filter(|r| r.0 % 3 == 0).collect();
    assert_eq!(got, expected);

    let none = RecordsOptions {
        ids: Some(TargetSet::default()),
        ..options(RecordFile::Artists)
    };
    let mut batches = 0;
    let s = records_reader(Cursor::new(bytes), &none, &mut |_| {
        batches += 1;
        true
    })
    .unwrap();
    assert_eq!((batches, s.matched, s.batches), (0, 0, 0));
    assert_eq!(s.records, 40);
}

#[test]
fn known_bodies() {
    let (got, _) = collect_records(fixture("labels.xml"), &options(RecordFile::Labels)).unwrap();
    let (id, body) = &got[0];
    assert_eq!(*id, 1);
    let body = String::from_utf8_lossy(body);
    assert!(body.starts_with("<label><id>1</id><name>Planet E</name>"));
    // Up to the first sub-label's end tag, left out.
    assert!(body.ends_with("<label id=\"31405\">I Ner Zon Sounds"), "{body}");

    let (got, _) = collect_records(fixture("masters.xml"), &options(RecordFile::Masters)).unwrap();
    assert!(got
        .iter()
        .all(|(id, b)| b.starts_with(format!("<master id=\"{id}\">").as_bytes())));
    assert!(got.iter().all(|(_, b)| !b.ends_with(b"</master>")));
}

#[test]
fn out_of_order_fails() {
    let x = b"<artists><artist><id>5</id></artist><artist><id>5</id></artist><artist><id>4</id></artist></artists>";
    let e = read_all(x, &options(RecordFile::Artists)).unwrap_err();
    assert!(matches!(&e, Error::Dump(m) if m.contains("(4 after 5)")), "{e}");
    // Across blocks too.
    let o = RecordsOptions {
        block_size: 1,
        ..options(RecordFile::Artists)
    };
    assert!(matches!(read_all(x, &o), Err(Error::Dump(_))));
}

#[test]
fn checksum_is_checked_at_the_end() {
    let bytes = std::fs::read(fixture("masters.xml")).unwrap();
    let good = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let o = RecordsOptions {
        checksum: Some(good.clone()),
        ..options(RecordFile::Masters)
    };
    let s = records_reader(Cursor::new(bytes.clone()), &o, &mut |_| true).unwrap();
    assert_eq!(s.sha256, good);
    let bad = RecordsOptions {
        checksum: Some("0".repeat(64)),
        ..options(RecordFile::Masters)
    };
    assert!(matches!(
        records_reader(Cursor::new(bytes), &bad, &mut |_| true),
        Err(Error::ChecksumMismatch { .. })
    ));
}

#[test]
fn the_sink_can_stop_the_run() {
    let bytes = std::fs::read(fixture("real-masters-20261101-head.xml")).unwrap();
    let o = RecordsOptions {
        block_size: 512,
        ..options(RecordFile::Masters)
    };
    let mut seen = 0;
    let s = records_reader(Cursor::new(bytes), &o, &mut |b| {
        seen += b.len();
        false
    })
    .unwrap();
    assert!(s.cancelled);
    assert_eq!(s.batches, 1);
    assert_eq!(s.matched as usize, seen);
    assert!(s.sha256.is_empty());
}

fn arb_records() -> impl Strategy<Value = Vec<u8>> {
    // Records (ids ascending), sub-labels, broken starts and noise.
    let piece = prop_oneof![
        (0u8..1, "[a-z <>/&;]{0,20}"),
        (1u8..2, Just(String::new())),
        (2u8..3, Just(String::new())),
        (3u8..4, Just(String::new())),
        (4u8..5, "[a-z<>/]{0,8}"),
    ];
    prop::collection::vec(piece, 0..40).prop_map(|v| {
        let mut id = 0;
        let mut out = String::new();
        for (kind, t) in v {
            id += 1;
            out += &match kind {
                0 => format!("<label><id>{id}</id>{t}</label>"),
                1 => format!("<label id=\"{id}\">S</label>"),
                2 => "<label><id></id>".to_string(),
                3 => format!("<label><id>{id}"),
                _ => t,
            };
        }
        out.into_bytes()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Any block size gives the records of one scan of the whole input
    #[test]
    fn blocks_equal_one_scan(input in arb_records(), block_size in 1usize..64) {
        let scanner = RecordScanner::new(RecordFile::Labels);
        prop_assert_eq!(scanner.records(&input).count(), ts_scan(RecordFile::Labels, &input, 1 << 20).len());
        let expected = ts_scan(RecordFile::Labels, &input, 5);
        let o = RecordsOptions { block_size, threads: 2, ..RecordsOptions::new(RecordFile::Labels) };
        prop_assert_eq!(read_all(&input, &o).unwrap(), expected);
    }
}

#[test]
fn a_cancel_flag_stops_the_run() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let bytes = gzip(&std::fs::read(fixture("real-masters-20261101-head.xml")).unwrap());
    let flag = Arc::new(AtomicBool::new(true));
    let o = RecordsOptions {
        cancel: Some(flag.clone()),
        checksum: Some("0".repeat(64)),
        ..options(RecordFile::Masters)
    };
    let s = records_reader(Cursor::new(bytes), &o, &mut |_| true).unwrap();
    assert!(s.cancelled);
    assert_eq!(s.matched, 0);
    assert!(flag.load(Ordering::Relaxed));
}
