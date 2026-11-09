//! The hand-written fixtures (`fixtures/releases.xml`, `masters.xml`), row
//! by row, and the real head of the 2026-11-01 dump.

use std::path::PathBuf;

use discogs_core::{profile_path, FormatClass, ProfileOptions, Row, RowKind, TargetSet};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn targets() -> TargetSet {
    TargetSet::parse(&std::fs::read(fixture("targets.txt")).unwrap()).unwrap()
}

fn run(name: &str) -> discogs_core::Profile {
    profile_path(
        fixture(name),
        &targets(),
        &ProfileOptions {
            threads: 2,
            ..Default::default()
        },
    )
    .unwrap()
}

fn row<'a>(rows: &'a [Row], artist: u32, kind: RowKind, name: &str, year: Option<u16>) -> &'a Row {
    let found: Vec<_> = rows
        .iter()
        .filter(|r| r.artist_id == artist && r.kind == kind && r.name == name && r.year == year)
        .collect();
    assert_eq!(found.len(), 1, "{artist} {kind:?} {name:?} {year:?}: {found:?}");
    found[0]
}

fn close(a: f32, b: f64) -> bool {
    (a as f64 - b).abs() < 1e-6
}

#[test]
fn releases() {
    let p = run("releases.xml");
    let s = &p.summary;
    assert_eq!(s.kind, "releases");
    assert_eq!((s.records, s.hits, s.matched), (11, 9, 5));
    assert_eq!(
        (s.skipped_various, s.skipped_master, s.skipped_status, s.skipped_quality),
        (1, 1, 1, 1)
    );
    assert_eq!(s.decompressor, "none");
    let rows = &p.rows;
    assert_eq!(rows.len(), 27);

    // Only main artists: never 4, 7 (track artists) or 239 (extra artist).
    assert!(rows.iter().all(|r| [1, 2, 3].contains(&r.artist_id)));

    // Release 1: one artist, one genre, two styles.
    let r = row(rows, 1, RowKind::Style, "Electronic/Deep House", Some(1999));
    assert_eq!((r.format_class, r.promo, r.records), (Some(FormatClass::Ep), false, 1));
    assert!(close(r.weight, 1.0) && close(r.share, 0.5));
    let r = row(rows, 1, RowKind::Total, "", Some(1999));
    assert!(close(r.weight, 1.0) && close(r.share, 1.0));

    // Release 2: two artists, two genres, one style: Electro under each genre.
    for artist in [1, 2] {
        let t = row(rows, artist, RowKind::Total, "", Some(1984));
        assert!(close(t.weight, 0.5));
        assert_eq!(t.format_class, Some(FormatClass::Album));
        let g = row(rows, artist, RowKind::Genre, "Hip Hop", Some(1984));
        assert!(close(g.weight, 0.5) && close(g.share, 0.25));
        for pair in ["Electronic/Electro", "Hip Hop/Electro"] {
            let st = row(rows, artist, RowKind::Style, pair, Some(1984));
            assert!(close(st.weight, 0.25) && close(st.share, 0.25), "{pair}: {st:?}");
        }
    }

    // Release 6: entities decoded, no release date, a promo single, two
    // genres × two styles.
    let r = row(rows, 3, RowKind::Style, "Rock/Rock & Roll", None);
    assert_eq!((r.format_class, r.promo), (Some(FormatClass::Single), true));
    assert!(close(r.weight, 0.5) && close(r.share, 0.25));
    row(rows, 3, RowKind::Style, "Reggae/Dub", None);
    row(rows, 3, RowKind::Style, "Rock/Dub", None);
    row(rows, 3, RowKind::Style, "Reggae/Rock & Roll", None);

    // Release 8: `<released>0</released>` is no year; a DJ mix.
    let dub = rows
        .iter()
        .find(|r| r.artist_id == 3 && r.name == "Electronic/Dub")
        .expect("Electronic/Dub");
    assert_eq!((dub.year, dub.format_class), (None, Some(FormatClass::Mix)));

    // Release 9: artist 2 credited twice counts once; n = 2 (2 and 10).
    let r = row(rows, 2, RowKind::Total, "", Some(2001));
    assert_eq!(r.format_class, Some(FormatClass::Other));
    assert!(close(r.weight, 0.5) && r.records == 1);

    // Skipped: 3 (has a master), 4 (Various), 5 (Entirely Incorrect), 11 (Draft).
    assert!(!rows
        .iter()
        .any(|r| r.year == Some(2005) || r.year == Some(1985) || r.year == Some(1990)));
    assert!(!rows.iter().any(|r| r.year == Some(2020)));
}

#[test]
fn masters() {
    let p = run("masters.xml");
    let s = &p.summary;
    assert_eq!(s.kind, "masters");
    assert_eq!((s.records, s.hits, s.matched), (6, 5, 3));
    assert_eq!((s.skipped_various, s.skipped_quality, s.skipped_master), (1, 1, 0));
    let rows = &p.rows;
    assert!(rows.iter().all(|r| r.format_class.is_none() && !r.promo));

    let r = row(rows, 1, RowKind::Style, "Electronic/Deep House", Some(1999));
    assert!(close(r.weight, 1.0));
    // Master 101: three artists, two genres, two styles; year 0 is none.
    let st = row(rows, 3, RowKind::Style, "Hip Hop/Breakbeat", None);
    assert!(close(st.weight, 1.0 / 6.0) && close(st.share, 1.0 / 12.0));
    let g = row(rows, 3, RowKind::Genre, "Electronic", None);
    assert!(close(g.weight, 1.0 / 3.0) && close(g.share, 1.0 / 6.0));
    // Artist 2: master 101 and master 104 (no year element) share a row key.
    let t = row(rows, 2, RowKind::Total, "", None);
    assert_eq!(t.records, 2);
    assert!(close(t.weight, 1.0 / 3.0 + 1.0));
    // A repeated style counts once.
    let d = row(rows, 2, RowKind::Style, "Funk / Soul/Disco", None);
    assert!(close(d.weight, 1.0) && close(d.share, 1.0) && d.records == 1);
}

#[test]
fn gzip_gives_the_same_rows() {
    use std::io::Write;
    let xml = std::fs::read(fixture("releases.xml")).unwrap();
    let dir = std::env::temp_dir().join(format!("discogs-core-gz-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let gz = dir.join("releases.xml.gz");
    // Two gzip members, as a concatenated file would have.
    let mut out = Vec::new();
    for half in [&xml[..xml.len() / 2], &xml[xml.len() / 2..]] {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(half).unwrap();
        out.extend(e.finish().unwrap());
    }
    std::fs::write(&gz, out).unwrap();
    let a = run("releases.xml");
    let b = profile_path(&gz, &targets(), &ProfileOptions::default()).unwrap();
    assert_eq!(a.rows, b.rows);
    assert_eq!(b.summary.decompressor, "zlib-rs");
    assert_eq!(b.summary.bytes_decompressed, xml.len() as u64);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn real_dump_head() {
    // The first 40 records of the 2026-11-01 releases and masters files.
    let all = TargetSet::from_ids(1..=2_000_000);
    for (name, kind) in [
        ("real-releases-20261101-head.xml", "releases"),
        ("real-masters-20261101-head.xml", "masters"),
    ] {
        let p = profile_path(fixture(name), &all, &ProfileOptions::default()).unwrap();
        let s = &p.summary;
        assert_eq!(s.kind, kind);
        assert_eq!(s.records, 40, "{name}");
        assert_eq!(
            s.hits,
            s.matched + s.skipped_various + s.skipped_master + s.skipped_status + s.skipped_quality
        );
        assert!(s.matched > 0, "{name}");
        if kind == "releases" {
            // Most of these early releases have a master; the rest carry
            // `<master_id …>0</master_id>` and must count.
            assert!(s.skipped_master > 0 && s.skipped_master < s.hits, "{s:?}");
        }
    }
}
