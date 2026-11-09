//! Parquet and JSON lines output, read back.

use std::path::PathBuf;

use arrow_array::{Array, Float32Array, StringArray, UInt16Array, UInt32Array};
use discogs_core::output::{write_jsonl, write_parquet, write_rows};
use discogs_core::{profile_path, ProfileOptions, TargetSet};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

fn rows() -> Vec<discogs_core::Row> {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/releases.xml");
    profile_path(fixture, &TargetSet::from_ids([1, 2, 3]), &ProfileOptions::default())
        .unwrap()
        .rows
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("discogs-core-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn parquet_round_trip() {
    let rows = rows();
    let path = tmp("p.parquet");
    write_parquet(&rows, &path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap())
        .unwrap()
        .build()
        .unwrap();
    let mut n = 0;
    for batch in reader {
        let batch = batch.unwrap();
        let names: Vec<_> = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
        assert_eq!(
            names,
            [
                "artist_id",
                "kind",
                "name",
                "year",
                "format_class",
                "promo",
                "weight",
                "share",
                "records"
            ]
        );
        let artist = batch.column(0).as_any().downcast_ref::<UInt32Array>().unwrap();
        let name = batch.column(2).as_any().downcast_ref::<StringArray>().unwrap();
        let year = batch.column(3).as_any().downcast_ref::<UInt16Array>().unwrap();
        let weight = batch.column(6).as_any().downcast_ref::<Float32Array>().unwrap();
        for i in 0..batch.num_rows() {
            let r = &rows[n + i];
            assert_eq!(artist.value(i), r.artist_id);
            assert_eq!(name.value(i), r.name);
            assert_eq!(year.is_null(i), r.year.is_none());
            assert_eq!(weight.value(i), r.weight);
        }
        n += batch.num_rows();
    }
    assert_eq!(n, rows.len());
    assert!(!PathBuf::from(format!("{}.partial", path.display())).exists());
}

#[test]
fn jsonl_lines() {
    let rows = rows();
    let path = tmp("p.jsonl");
    write_jsonl(&rows, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), rows.len());
    let promo = lines.iter().find(|l| l["name"] == "Rock/Rock & Roll").unwrap();
    assert_eq!(promo["kind"], "style");
    assert_eq!(promo["year"], serde_json::Value::Null);
    assert_eq!(promo["format_class"], "single");
    assert_eq!(promo["promo"], true);
    assert_eq!(promo["weight"], 0.5);
    assert_eq!(promo["share"], 0.25);
    assert_eq!(promo["records"], 1);
}

#[test]
fn unknown_extension_is_refused() {
    assert!(write_rows(&rows(), tmp("p.csv")).is_err());
}
