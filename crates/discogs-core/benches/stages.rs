//! Criterion benches of the two CPU stages after gunzip: the block
//! splitter and the record scanner, on 32 MB of synthetic releases XML.
//!
//!     cargo bench -p discogs-core --bench stages
//!
//! Not run in CI. `discogs bench` times the whole pipeline end to end.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use discogs_core::generate::{generate, targets_for_hit_rate, GenerateOptions};
use discogs_core::split::{Splitter, DEFAULT_BLOCK_SIZE};
use discogs_core::{Aggregator, Counters, RecordKind, Scanner, TargetSet};

const BYTES: u64 = 32 << 20;

fn xml(kind: RecordKind) -> Vec<u8> {
    let mut out = Vec::with_capacity(BYTES as usize + (1 << 20));
    generate(
        &GenerateOptions {
            kind,
            bytes: BYTES,
            ..Default::default()
        },
        &mut out,
    )
    .unwrap();
    out
}

fn blocks(xml: &[u8], kind: RecordKind, size: usize) -> Vec<Vec<u8>> {
    let mut s = Splitter::new(xml, kind.close_tag(), size);
    let mut out = Vec::new();
    while let Some(b) = s.next_block(Vec::new()).unwrap() {
        out.push(b);
    }
    out
}

fn splitter(c: &mut Criterion) {
    let kind = RecordKind::Releases;
    let data = xml(kind);
    let mut g = c.benchmark_group("splitter");
    g.throughput(Throughput::Bytes(data.len() as u64));
    g.sample_size(20);
    for size in [1 << 20, DEFAULT_BLOCK_SIZE] {
        g.bench_with_input(
            BenchmarkId::new("block", format!("{}MB", size >> 20)),
            &size,
            |b, &size| {
                // Buffers recycled, as the pipeline does.
                let mut spare: Vec<Vec<u8>> = Vec::new();
                b.iter(|| {
                    let mut s = Splitter::new(&data[..], kind.close_tag(), size);
                    let mut n = 0usize;
                    while let Some(block) = s.next_block(spare.pop().unwrap_or_default()).unwrap() {
                        n += block.len();
                        spare.push(block);
                    }
                    black_box(n)
                });
            },
        );
    }
    g.finish();
}

fn scan_block(c: &mut Criterion) {
    let mut g = c.benchmark_group("scan_block");
    g.sample_size(20);
    for kind in [RecordKind::Releases, RecordKind::Masters] {
        let data = xml(kind);
        let blocks = blocks(&data, kind, DEFAULT_BLOCK_SIZE);
        g.throughput(Throughput::Bytes(data.len() as u64));
        for rate in [0.0, 0.01, 0.1] {
            let targets = if rate == 0.0 {
                TargetSet::default()
            } else {
                TargetSet::from_ids(targets_for_hit_rate(rate, GenerateOptions::default().max_artist_id))
            };
            let id = BenchmarkId::new(kind.as_str(), format!("{}% hits", rate * 100.0));
            g.bench_with_input(id, &targets, |b, targets| {
                b.iter(|| {
                    let mut scanner = Scanner::new(kind);
                    let mut agg = Aggregator::new();
                    let mut counters = Counters::default();
                    for block in &blocks {
                        scanner.scan_block(block, targets, &mut agg, &mut counters);
                    }
                    black_box((agg, counters.records))
                });
            });
        }
    }
    g.finish();
}

criterion_group!(benches, splitter, scan_block);
criterion_main!(benches);
