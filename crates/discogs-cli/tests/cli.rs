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

#[path = "../../discogs-core/tests/support/server.rs"]
mod server;

mod network {
    use std::io::Write;
    use std::process::Command;

    use sha2::{Digest, Sha256};

    use super::server::{Reply, Server};
    use super::{fixtures, tmp};

    const LINK: &str = "/?download=data%2F2026%2Fdiscogs_20261101_releases.xml.gz";
    const SUMS: &str = "/?download=data%2F2026%2Fdiscogs_20261101_CHECKSUM.txt";

    fn served() -> (Server, Vec<u8>) {
        let xml = std::fs::read(fixtures().join("releases.xml")).unwrap();
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(&xml).unwrap();
        let gz = e.finish().unwrap();
        let sum: String = Sha256::digest(&gz).iter().map(|b| format!("{b:02x}")).collect();
        let s = Server::start();
        s.route(LINK, vec![Reply::Body(gz.clone())]);
        s.route(
            SUMS,
            vec![Reply::Body(
                format!(
                    "{sum} discogs_20261101_releases.xml.gz\n{} discogs_20261101_masters.xml.gz\n",
                    "b".repeat(64)
                )
                .into_bytes(),
            )],
        );
        // This year's listing (whatever year the test runs in) and last year's.
        let (year, _) = discogs_core::dumps::today_utc();
        s.route(
            &format!("/?prefix=data/{year}/"),
            vec![Reply::Body(
                format!("discogs_{year}0101_CHECKSUM.txt discogs_{year}0115_CHECKSUM.txt discogs_{year}0201_releases.xml.gz")
                    .into_bytes(),
            )],
        );
        s.route(
            &format!("/?prefix=data/{}/", year - 1),
            vec![Reply::Body(
                format!("discogs_{}1201_CHECKSUM.txt", year - 1).into_bytes(),
            )],
        );
        (s, gz)
    }

    fn discogs(s: &Server, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_discogs"))
            .args(args)
            .env("DISCOGS_DATA_URL", &s.base)
            .output()
            .unwrap()
    }

    #[test]
    fn checksum_prints_the_dump_checksums() {
        let (s, gz) = served();
        let o = discogs(&s, &["checksum", "--dump", "20261101"]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let text = String::from_utf8(o.stdout).unwrap();
        let sum: String = Sha256::digest(&gz).iter().map(|b| format!("{b:02x}")).collect();
        assert!(
            text.contains(&format!("{sum}  discogs_20261101_releases.xml.gz\n")),
            "{text}"
        );
        assert_eq!(text.lines().count(), 2);
        let o = discogs(&s, &["checksum", "--dump", "20261101", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["discogs_20261101_releases.xml.gz"], sum);
    }

    #[test]
    fn latest_prints_the_newest_complete_dump() {
        let (s, _) = served();
        let o = discogs(&s, &["latest"]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let dump = String::from_utf8(o.stdout).unwrap();
        let (year, _) = discogs_core::dumps::today_utc();
        assert_eq!(dump.trim(), format!("{year}0115"));
    }

    #[test]
    fn profile_a_url_with_auto_checksum_then_skip_it() {
        let (s, gz) = served();
        let dir = tmp().join("url");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("r.jsonl");
        let kept = dir.join("discogs_20261101_releases.xml.gz");
        let state = dir.join("state.json");
        let _ = std::fs::remove_file(&state);
        let url = s.url(LINK);
        let args = |extra: &[&str]| {
            let mut a = vec![
                "profile".to_string(),
                "--input".into(),
                url.clone(),
                "--targets".into(),
                fixtures().join("targets.txt").display().to_string(),
                "--out".into(),
                out.display().to_string(),
                "--checksum".into(),
                "auto".into(),
                "--state".into(),
                state.display().to_string(),
                "--json".into(),
            ];
            a.extend(extra.iter().map(|s| s.to_string()));
            a
        };
        let first = Command::new(env!("CARGO_BIN_EXE_discogs"))
            .args(args(&["--keep", &kept.display().to_string()]))
            .output()
            .unwrap();
        assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
        let v: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
        assert_eq!(v["skipped"], false);
        assert_eq!(v["records"], 11);
        assert_eq!(v["rows"], 27);
        assert_eq!(v["attempts"], 1);
        assert_eq!(v["source"], url);
        assert!(v["download_seconds"].is_number());
        assert_eq!(std::fs::read(&kept).unwrap(), gz);

        let second = Command::new(env!("CARGO_BIN_EXE_discogs"))
            .args(args(&[]))
            .output()
            .unwrap();
        assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
        let v: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
        assert_eq!(v["skipped"], true);
        assert_eq!(v["rows"], 27);
        assert_eq!(s.hits(LINK), 1);
    }
}
