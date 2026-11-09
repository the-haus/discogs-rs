//! Checksums and broken input: a failed run returns no rows.

use std::io::Write;
use std::path::PathBuf;

use discogs_core::{profile_path, profile_reader, Error, ProfileOptions, TargetSet};

fn fixture() -> Vec<u8> {
    std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/releases.xml")).unwrap()
}

fn gz(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn targets() -> TargetSet {
    TargetSet::from_ids([1, 2, 3])
}

#[test]
fn matching_checksum_passes_and_is_reported() {
    let data = gz(&fixture());
    let first = profile_reader(&data[..], &targets(), &ProfileOptions::default()).unwrap();
    let sha = first.summary.sha256.clone();
    assert_eq!(sha.len(), 64);
    let again = ProfileOptions {
        checksum: Some(sha.to_uppercase()),
        ..Default::default()
    };
    let second = profile_reader(&data[..], &targets(), &again).unwrap();
    assert_eq!(first.rows, second.rows);
    assert!(!second.rows.is_empty());
}

#[test]
fn mismatch_is_an_error_with_no_rows() {
    let data = gz(&fixture());
    let wrong = ProfileOptions {
        checksum: Some("0".repeat(64)),
        ..Default::default()
    };
    match profile_reader(&data[..], &targets(), &wrong) {
        Err(Error::ChecksumMismatch { expected, actual }) => {
            assert_eq!(expected, "0".repeat(64));
            assert_eq!(actual.len(), 64);
        }
        other => panic!("expected a checksum mismatch, got {other:?}"),
    }
}

#[test]
fn bad_checksum_text_is_refused() {
    let bad = ProfileOptions {
        checksum: Some("abc".into()),
        ..Default::default()
    };
    assert!(matches!(
        profile_reader(&b""[..], &targets(), &bad),
        Err(Error::BadChecksum(_))
    ));
}

#[test]
fn truncated_gzip_is_an_error() {
    let data = gz(&fixture());
    let cut = &data[..data.len() - 20];
    let r = profile_reader(cut, &targets(), &ProfileOptions::default());
    assert!(matches!(r, Err(Error::Io(_))), "{r:?}");
}

#[test]
fn unknown_kind_is_an_error() {
    let r = profile_reader(
        &b"<artists><artist><id>1</id></artist></artists>"[..],
        &targets(),
        &ProfileOptions::default(),
    );
    assert!(matches!(r, Err(Error::UnknownKind)), "{r:?}");
}

#[test]
fn missing_file_names_the_path() {
    let e = profile_path("/nonexistent/discogs.xml.gz", &targets(), &ProfileOptions::default()).unwrap_err();
    assert!(e.to_string().contains("/nonexistent/discogs.xml.gz"));
}
