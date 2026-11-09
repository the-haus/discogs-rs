//! HTTPS input without the network: a local server serves the fixtures.

#![cfg(feature = "http")]

mod support;

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use discogs_core::remote::{Http, HttpOptions, USER_AGENT};
use discogs_core::{profile_reader, run_profile, Error, ProfileOptions, RunOptions, TargetSet};
use sha2::{Digest, Sha256};
use support::server::{Reply, Server};

const DUMP: &str = "20261101";
const FILE: &str = "discogs_20261101_releases.xml.gz";
const LINK: &str = "/?download=data%2F2026%2Fdiscogs_20261101_releases.xml.gz";
const SUMS: &str = "/?download=data%2F2026%2Fdiscogs_20261101_CHECKSUM.txt";

fn gz() -> Vec<u8> {
    let xml = std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/releases.xml")).unwrap();
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(&xml).unwrap();
    e.finish().unwrap()
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn checksum_txt(sum: &str) -> Vec<u8> {
    format!(
        "{} discogs_{DUMP}_labels.xml.gz\n{sum} {FILE}\n{} discogs_{DUMP}_masters.xml.gz\n",
        "1".repeat(64),
        "2".repeat(64)
    )
    .into_bytes()
}

fn targets() -> TargetSet {
    TargetSet::from_ids([1, 2, 3, 4, 7, 239])
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("discogs-http-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn options(server: &Server, dir: &std::path::Path) -> RunOptions {
    RunOptions {
        source: server.url(LINK),
        out: dir.join("out.jsonl"),
        base_url: Some(server.base.clone()),
        timeout: Some(Duration::from_secs(10)),
        profile: ProfileOptions {
            threads: 2,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn rows_of_file() -> String {
    let dir = tmp(&format!("reference-{:?}", std::thread::current().id()));
    let p = profile_reader(&gz()[..], &targets(), &ProfileOptions::default()).unwrap();
    let out = dir.join("ref.jsonl");
    discogs_core::output::write_rows(&p.rows, &out).unwrap();
    std::fs::read_to_string(out).unwrap()
}

#[test]
fn a_gz_served_whole_gives_the_rows_of_the_file() {
    let s = Server::start();
    let data = gz();
    s.route(LINK, vec![Reply::Body(data.clone())]);
    let dir = tmp("whole");
    let o = options(&s, &dir);
    let r = run_profile(&targets(), &o).unwrap();
    assert!(!r.skipped);
    assert_eq!(r.attempts, 1);
    assert_eq!(r.source, s.url(LINK));
    assert_eq!(r.profile.sha256, sha(&data));
    assert_eq!(r.profile.bytes_in, data.len() as u64);
    assert_eq!(r.profile.records, 11);
    assert!(r.download_seconds >= 0.0);
    assert_eq!(std::fs::read_to_string(&o.out).unwrap(), rows_of_file());
    assert_eq!(s.user_agents(), [USER_AGENT]);
    assert!(USER_AGENT.starts_with("discogs-rs/"));
}

#[test]
fn a_dropped_connection_is_an_error_with_no_output() {
    let s = Server::start();
    let data = gz();
    let after = data.len() / 2;
    s.route(LINK, vec![Reply::Drop { body: data, after }]);
    let dir = tmp("drop");
    let o = RunOptions {
        keep: Some(dir.join(FILE)),
        ..options(&s, &dir)
    };
    let e = run_profile(&targets(), &o).unwrap_err();
    assert!(matches!(e, Error::Network(_)), "{e:?}");
    // The first try and two retries.
    assert_eq!(s.hits(LINK), 3);
    assert!(!o.out.exists());
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "no output, no kept file, no partials"
    );

    let none = RunOptions { retries: 0, ..o };
    s.route(LINK, vec![Reply::Status(503)]);
    let before = s.hits(LINK);
    assert!(matches!(run_profile(&targets(), &none), Err(Error::Network(_))));
    assert_eq!(s.hits(LINK) - before, 1);
}

#[test]
fn a_drop_then_a_whole_body_succeeds_on_the_retry() {
    let s = Server::start();
    let data = gz();
    s.route(
        LINK,
        vec![
            Reply::Drop {
                body: data.clone(),
                after: 100,
            },
            Reply::Status(502),
            Reply::Body(data),
        ],
    );
    let dir = tmp("retry");
    let o = options(&s, &dir);
    let r = run_profile(&targets(), &o).unwrap();
    assert_eq!(r.attempts, 3);
    assert_eq!(std::fs::read_to_string(&o.out).unwrap(), rows_of_file());
}

#[test]
fn a_404_is_not_retried() {
    let s = Server::start();
    let dir = tmp("404");
    let e = run_profile(&targets(), &options(&s, &dir)).unwrap_err();
    assert!(matches!(e, Error::Http(ref m) if m.contains("404")), "{e:?}");
    assert_eq!(s.hits(LINK), 1);
}

#[test]
fn a_wrong_checksum_is_an_error_with_no_output() {
    let s = Server::start();
    s.route(LINK, vec![Reply::Body(gz())]);
    let dir = tmp("wrong");
    let o = RunOptions {
        checksum: Some("0".repeat(64)),
        keep: Some(dir.join(FILE)),
        state: Some(dir.join("state.json")),
        ..options(&s, &dir)
    };
    let e = run_profile(&targets(), &o).unwrap_err();
    assert!(matches!(e, Error::ChecksumMismatch { .. }), "{e:?}");
    assert_eq!(s.hits(LINK), 1, "a mismatch is not retried");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "no output, kept file or state"
    );
}

#[test]
fn checksum_txt_is_fetched_and_parsed() {
    let s = Server::start();
    s.route(SUMS, vec![Reply::Body(checksum_txt(&"a".repeat(64)))]);
    let http = Http::new(HttpOptions {
        base_url: s.base.clone(),
        ..Default::default()
    })
    .unwrap();
    let sums = http.checksums(DUMP).unwrap();
    assert_eq!(sums.len(), 3);
    assert_eq!(sums[FILE], "a".repeat(64));
    assert!(matches!(http.checksums("2026"), Err(Error::Checksum(_))));
    assert!(matches!(http.checksums("20261001"), Err(Error::Http(_))));
}

#[test]
fn auto_looks_the_sum_up_beside_the_file() {
    let s = Server::start();
    let data = gz();
    s.route(LINK, vec![Reply::Body(data.clone())]);
    s.route(SUMS, vec![Reply::Body(checksum_txt(&sha(&data)))]);
    let dir = tmp("auto");
    let o = RunOptions {
        checksum: Some("auto".into()),
        ..options(&s, &dir)
    };
    let r = run_profile(&targets(), &o).unwrap();
    assert_eq!(r.expected_sha256.as_deref(), Some(sha(&data).as_str()));

    // A sum that does not match: no output.
    s.route(SUMS, vec![Reply::Body(checksum_txt(&"f".repeat(64)))]);
    std::fs::remove_file(&o.out).unwrap();
    assert!(matches!(
        run_profile(&targets(), &o),
        Err(Error::ChecksumMismatch { .. })
    ));
    assert!(!o.out.exists());

    // A CHECKSUM.txt that does not list the file: nothing is downloaded.
    s.route(SUMS, vec![Reply::Body(b"no sums here\n".to_vec())]);
    let before = s.hits(LINK);
    assert!(matches!(run_profile(&targets(), &o), Err(Error::Checksum(_))));
    assert_eq!(s.hits(LINK), before);
}

#[test]
fn auto_for_a_local_file_reads_the_checksum_beside_it_or_the_site() {
    let s = Server::start();
    let data = gz();
    let dir = tmp("auto-local");
    let path = dir.join(FILE);
    std::fs::write(&path, &data).unwrap();
    let o = RunOptions {
        source: path.display().to_string(),
        checksum: Some("auto".into()),
        out: dir.join("out.jsonl"),
        base_url: Some(s.base.clone()),
        ..Default::default()
    };
    // None beside it: the site's.
    s.route(SUMS, vec![Reply::Body(checksum_txt(&sha(&data)))]);
    assert!(run_profile(&targets(), &o).is_ok());
    assert_eq!(s.hits(SUMS), 1);
    // One beside it wins.
    std::fs::write(dir.join("discogs_20261101_CHECKSUM.txt"), checksum_txt(&"e".repeat(64))).unwrap();
    assert!(matches!(
        run_profile(&targets(), &o),
        Err(Error::ChecksumMismatch { .. })
    ));
    assert_eq!(s.hits(SUMS), 1);
}

#[test]
fn an_unchanged_dump_is_skipped_without_downloading() {
    let s = Server::start();
    let data = gz();
    s.route(LINK, vec![Reply::Body(data.clone())]);
    s.route(SUMS, vec![Reply::Body(checksum_txt(&sha(&data)))]);
    let dir = tmp("skip");
    let state = dir.join("state.json");
    let o = RunOptions {
        checksum: Some("auto".into()),
        state: Some(state.clone()),
        ..options(&s, &dir)
    };
    let first = run_profile(&targets(), &o).unwrap();
    assert!(!first.skipped);
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    let entry = &saved["entries"].as_object().unwrap().values().next().unwrap();
    assert_eq!(entry["sha256"], sha(&data));
    assert_eq!(entry["file"], FILE);

    let again = run_profile(&targets(), &o).unwrap();
    assert!(again.skipped);
    assert_eq!(again.attempts, 0);
    assert_eq!(again.profile.rows, first.profile.rows);
    assert_eq!(s.hits(LINK), 1, "the dump was not downloaded again");

    // Other targets: a run.
    let other = run_profile(&TargetSet::from_ids([1, 2]), &o).unwrap();
    assert!(!other.skipped);
    assert_eq!(s.hits(LINK), 2);
    // Both entries are kept: the first targets still skip.
    assert!(run_profile(&targets(), &o).unwrap().skipped);
    // Another output path: a run.
    let elsewhere = RunOptions {
        out: dir.join("other.jsonl"),
        ..o.clone()
    };
    assert!(!run_profile(&targets(), &elsewhere).unwrap().skipped);
    // A deleted output: a run.
    std::fs::remove_file(&o.out).unwrap();
    assert!(!run_profile(&targets(), &o).unwrap().skipped);
    // A new dump file, same kind: a run (another checksum).
    let changed = {
        let mut d = data.clone();
        // Another gzip header mtime: same XML, another sha256.
        d[4] ^= 1;
        d
    };
    s.route(LINK, vec![Reply::Body(changed.clone())]);
    s.route(SUMS, vec![Reply::Body(checksum_txt(&sha(&changed)))]);
    let next = run_profile(&targets(), &o).unwrap();
    assert!(!next.skipped);
    assert_eq!(next.profile.sha256, sha(&changed));
}

#[test]
fn redirects_are_followed() {
    let s = Server::start();
    let data = gz();
    let bucket = format!("/bucket/data/2026/{FILE}?X-Amz-Signature=abc");
    s.route(LINK, vec![Reply::Redirect(s.url(&bucket))]);
    s.route(&bucket, vec![Reply::Body(data.clone())]);
    s.route(SUMS, vec![Reply::Body(checksum_txt(&sha(&data)))]);
    let dir = tmp("redirect");
    let o = RunOptions {
        checksum: Some("auto".into()),
        ..options(&s, &dir)
    };
    let r = run_profile(&targets(), &o).unwrap();
    assert_eq!(r.profile.sha256, sha(&data));
    assert_eq!(s.hits(&bucket), 1);
    assert_eq!(std::fs::read_to_string(&o.out).unwrap(), rows_of_file());
}

#[test]
fn keep_writes_the_download_as_it_streams() {
    let s = Server::start();
    let data = gz();
    s.route(LINK, vec![Reply::Body(data.clone())]);
    let dir = tmp("keep");
    let kept = dir.join(FILE);
    let o = RunOptions {
        keep: Some(kept.clone()),
        ..options(&s, &dir)
    };
    run_profile(&targets(), &o).unwrap();
    assert_eq!(std::fs::read(&kept).unwrap(), data);
    assert!(!dir.join(format!("{FILE}.partial")).exists());

    // The kept file reads the same, locally.
    let local = RunOptions {
        source: kept.display().to_string(),
        out: dir.join("local.jsonl"),
        ..Default::default()
    };
    run_profile(&targets(), &local).unwrap();
    assert_eq!(
        std::fs::read_to_string(&local.out).unwrap(),
        std::fs::read_to_string(&o.out).unwrap()
    );
    // Keep is for URLs.
    let wrong = RunOptions {
        keep: Some(dir.join("x.gz")),
        ..local
    };
    assert!(matches!(run_profile(&targets(), &wrong), Err(Error::Usage(_))));
}

#[test]
fn the_latest_dump_is_the_newest_with_a_checksum() {
    let s = Server::start();
    let listing = |dumps: &[&str], partial: &str| {
        let mut html = String::from("<pre>");
        for d in dumps {
            html += &format!("<a href=\"?download=data%2F{y}%2Fdiscogs_{d}_CHECKSUM.txt\">discogs_{d}_CHECKSUM.txt</a>\n<a>discogs_{d}_releases.xml.gz</a>\n", y = &d[..4]);
        }
        html += &format!("<a>discogs_{partial}_releases.xml.gz</a></pre>");
        html.into_bytes()
    };
    s.route(
        "/?prefix=data/2026/",
        vec![Reply::Body(listing(&["20260101", "20261001", "20260901"], "20261101"))],
    );
    s.route(
        "/?prefix=data/2025/",
        vec![Reply::Body(listing(&["20251101", "20251201"], ""))],
    );
    s.route("/?prefix=data/2027/", vec![Reply::Body(listing(&[], "20270101"))]);
    let http = Http::new(HttpOptions {
        base_url: s.base.clone(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(http.latest_dump_on(2026, 11).unwrap(), "20261001");
    assert_eq!(s.hits("/?prefix=data/2025/"), 0);
    assert_eq!(http.latest_dump_on(2026, 1).unwrap(), "20261001");
    // January before its dump: last year's.
    assert_eq!(http.latest_dump_on(2027, 1).unwrap(), "20261001");
    assert_eq!(http.dumps_in_year(2025).unwrap(), ["20251101", "20251201"]);
}

/// The real site: its listing only (a few KB), never a dump.
#[test]
#[ignore = "network: data.discogs.com"]
fn the_real_listing_names_a_dump_with_checksums() {
    let http = Http::new(HttpOptions::default()).unwrap();
    let dump = http.latest_dump().unwrap();
    assert!(dump.len() == 8 && dump.as_str() >= "20261101", "{dump}");
    let sums = http.checksums(&dump).unwrap();
    for kind in ["releases", "masters", "artists", "labels"] {
        assert!(
            sums.contains_key(&format!("discogs_{dump}_{kind}.xml.gz")),
            "{kind}: {sums:?}"
        );
    }
}
