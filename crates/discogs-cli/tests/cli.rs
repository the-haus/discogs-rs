//! The `discogs` command, end to end.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn tmp() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("discogs-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn profile(out: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_discogs"))
        .arg("profile")
        .arg("--input")
        .arg(fixtures().join("releases.xml"))
        .arg("--targets")
        .arg(fixtures().join("targets.txt"))
        .arg("--out")
        .arg(out)
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn profile_writes_jsonl_and_a_json_summary() {
    let out = tmp().join("r.jsonl");
    let o = profile(&out, &["--json", "--threads", "2"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let summary: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(summary["records"], 11);
    assert_eq!(summary["matched"], 5);
    assert_eq!(summary["rows"], 27);
    assert!(summary["seconds"]["write"].is_number());
    assert_eq!(std::fs::read_to_string(&out).unwrap().lines().count(), 27);
}

#[test]
fn checksum_mismatch_writes_nothing() {
    let out = tmp().join("mismatch.parquet");
    let o = profile(&out, &["--checksum", &"a".repeat(64)]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("checksum mismatch"));
    assert!(!out.exists());
    assert!(!PathBuf::from(format!("{}.partial", out.display())).exists());
}

#[test]
fn bench_runs_on_a_tiny_file() {
    let dir = tmp().join("bench");
    let o = Command::new(env!("CARGO_BIN_EXE_discogs"))
        .args([
            "bench",
            "--generate",
            "0.002",
            "--hit-rates",
            "0.1",
            "--threads",
            "2",
            "--json",
            "--dir",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let runs: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    // decompress-only + (plain, gzip) × (1 thread, 2 threads).
    assert_eq!(runs.as_array().unwrap().len(), 5);
}
