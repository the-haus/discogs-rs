//! Property tests: the block size and thread count never change the
//! output.

use discogs_core::generate::{generate, GenerateOptions};
use discogs_core::split::Splitter;
use discogs_core::{profile_reader, Aggregator, Counters, ProfileOptions, RecordKind, Row, Scanner, TargetSet};
use proptest::prelude::*;

/// The reference: one scan over the whole input, no splitting, no threads.
fn whole(xml: &[u8], kind: RecordKind, targets: &TargetSet) -> (Vec<Row>, Counters) {
    let mut agg = Aggregator::new();
    let mut c = Counters::default();
    Scanner::new(kind).scan_block(xml, targets, &mut agg, &mut c);
    (agg.into_rows(), c)
}

fn sample(seed: u64, kind: RecordKind, bytes: u64) -> Vec<u8> {
    // Few artist ids, so rows from different blocks share keys and merge.
    let o = GenerateOptions {
        kind,
        bytes,
        seed,
        max_artist_id: 60,
        ..Default::default()
    };
    let mut out = Vec::new();
    generate(&o, &mut out).unwrap();
    out
}

fn kinds() -> impl Strategy<Value = RecordKind> {
    prop_oneof![Just(RecordKind::Releases), Just(RecordKind::Masters)]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn any_block_size_same_aggregates(
        seed in any::<u64>(),
        kind in kinds(),
        block in 24usize..20_000,
        threads in 1usize..5,
        targets in proptest::collection::vec(1u32..61, 0..30),
    ) {
        let xml = sample(seed, kind, 60_000);
        let targets = TargetSet::from_ids(targets);
        let (rows, c) = whole(&xml, kind, &targets);
        let p = profile_reader(&xml[..], &targets, &ProfileOptions { threads, block_size: block, ..Default::default() }).unwrap();
        prop_assert_eq!(p.summary.records, c.records);
        prop_assert_eq!(p.summary.matched, c.matched);
        prop_assert_eq!(p.summary.hits, c.hits);
        prop_assert_eq!(p.rows, rows);
    }

    #[test]
    fn splitter_output_is_the_input(seed in any::<u64>(), kind in kinds(), block in 1usize..9_000, cut in 0usize..4_000) {
        // A generated dump, cut short anywhere: the blocks still concatenate
        // to the input, and every block but the last ends with an end tag.
        let mut xml = sample(seed, kind, 20_000);
        xml.truncate(xml.len().saturating_sub(cut));
        let mut s = Splitter::new(&xml[..], kind.close_tag(), block);
        let mut blocks = Vec::new();
        while let Some(b) = s.next_block(Vec::new()).unwrap() {
            blocks.push(b);
        }
        prop_assert_eq!(blocks.concat(), xml.clone());
        if let Some((_, init)) = blocks.split_last() {
            for b in init {
                prop_assert!(b.ends_with(kind.close_tag()));
            }
        }
    }
}

#[test]
fn hand_fixtures_at_every_small_block_size() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let targets = TargetSet::from_ids([1, 2, 3, 4, 7, 239]);
    for (name, kind) in [
        ("releases.xml", RecordKind::Releases),
        ("masters.xml", RecordKind::Masters),
    ] {
        let xml = std::fs::read(dir.join(name)).unwrap();
        let (rows, _) = whole(&xml, kind, &targets);
        for block in (1..xml.len() + 10).step_by(7) {
            let p = profile_reader(
                &xml[..],
                &targets,
                &ProfileOptions {
                    threads: 3,
                    block_size: block,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(p.rows, rows, "{name} at block size {block}");
        }
    }
}
