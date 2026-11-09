//! Criterion benches of the CPU stages after gunzip: the block splitter,
//! the record scanner, and records by id (split + scan + copy + in-order
//! hand-over, on plain XML in memory), on 32 MB of synthetic XML.
//!
//!     cargo bench -p discogs-core --bench stages
//!
//! Not run in CI. `discogs bench` times the whole pipeline end to end.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use discogs_core::generate::{generate, targets_for_hit_rate, GenerateOptions};
use discogs_core::split::{Splitter, DEFAULT_BLOCK_SIZE};
use discogs_core::{
    records_reader, Aggregator, Counters, RecordFile, RecordKind, RecordScanner, RecordsOptions, Scanner, TargetSet,
};

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

fn records(c: &mut Criterion) {
    let data = xml(RecordKind::Masters);
    let ids: Vec<u32> = RecordScanner::new(RecordFile::Masters)
        .records(&data)
        .map(|f| f.id as u32)
        .collect();
    let mut g = c.benchmark_group("records");
    g.sample_size(10);
    g.throughput(Throughput::Bytes(data.len() as u64));
    let many = RecordsOptions::new(RecordFile::Masters).scanner_threads();
    for (label, set) in [
        ("all", None),
        ("1% ids", Some(TargetSet::from_ids(ids.iter().copied().step_by(100)))),
    ] {
        for threads in [1, many] {
            let options = RecordsOptions {
                ids: set.clone(),
                threads,
                no_hash: true,
                ..RecordsOptions::new(RecordFile::Masters)
            };
            g.bench_with_input(
                BenchmarkId::new(label, format!("{threads} threads")),
                &options,
                |b, o| {
                    b.iter(|| {
                        let mut n = 0usize;
                        records_reader(&data[..], o, &mut |batch| {
                            n += batch.len();
                            true
                        })
                        .unwrap();
                        black_box(n)
                    });
                },
            );
        }
    }
    g.finish();
}

criterion_group!(benches, splitter, scan_block, records);
criterion_main!(benches);
