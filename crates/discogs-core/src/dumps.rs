//! Discogs dump names, links, listings and `CHECKSUM.txt`, as text: no I/O
//! here (the HTTP side is [`crate::remote`]).
//!
//! - a dump is named by its date, `YYYYMMDD` (`20261101`);
//! - its files are `discogs_<dump>_<kind>.xml.gz` (`releases`, `masters`,
//!   `artists`, `labels`) and `discogs_<dump>_CHECKSUM.txt`;
//! - a file's link is `https://data.discogs.com/?download=data%2F<year>%2F<file>`
//!   (it redirects to the bucket);
//! - a year's listing is `https://data.discogs.com/?prefix=data/<year>/`, an
//!   HTML page linking every file. A dump counts once its checksum is listed:
//!   `CHECKSUM.txt` is the last file Discogs writes.

use std::collections::BTreeMap;

/// Discogs' data site.
pub const DISCOGS_DATA: &str = "https://data.discogs.com/";

/// Whether `dump` names a dump (`YYYYMMDD`).
pub fn is_dump(dump: &str) -> bool {
    dump.len() == 8 && dump.bytes().all(|b| b.is_ascii_digit())
}

/// A dump file's name: `discogs_20261101_releases.xml.gz`.
pub fn dump_file(dump: &str, kind: &str) -> String {
    format!("discogs_{dump}_{kind}.xml.gz")
}

/// A dump's checksum file name: `discogs_20261101_CHECKSUM.txt`.
pub fn checksum_file(dump: &str) -> String {
    format!("discogs_{dump}_CHECKSUM.txt")
}

fn with_slash(base: &str) -> String {
    if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    }
}

/// A file's download link under `base` ([`DISCOGS_DATA`]):
/// `<base>?download=data%2F<year>%2F<file>`.
pub fn download_url(base: &str, dump: &str, file: &str) -> String {
    format!(
        "{}?download=data%2F{}%2F{file}",
        with_slash(base),
        &dump[..4.min(dump.len())]
    )
}

/// A year's listing under `base`: `<base>?prefix=data/<year>/`.
pub fn listing_url(base: &str, year: i32) -> String {
    format!("{}?prefix=data/{year}/", with_slash(base))
}

/// A dump file name found in `text` (a path, a link, a bare name):
/// `(start, end, dump, kind)` of the first `discogs_<8 digits>_<kind>.xml.gz`.
fn find_dump_file(text: &str) -> Option<(usize, usize, &str, &str)> {
    let mut from = 0;
    while let Some(p) = text[from..].find("discogs_") {
        let start = from + p;
        let rest = &text[start + 8..];
        if rest.len() > 9 && is_dump(&rest[..8]) && rest.as_bytes()[8] == b'_' {
            let tail = &rest[9..];
            let kind_len = tail.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
            if kind_len > 0 && tail[kind_len..].starts_with(".xml.gz") {
                let end = start + 8 + 9 + kind_len + 7;
                return Some((start, end, &rest[..8], &tail[..kind_len]));
            }
        }
        from = start + 8;
    }
    None
}

/// The dump file a source names, as `(file name, dump, kind)`:
/// `discogs_20261101_releases.xml.gz` in a path or a
/// `?download=data%2F2026%2F…` link.
pub fn dump_file_in(source: &str) -> Option<(String, String, String)> {
    find_dump_file(source).map(|(s, e, dump, kind)| (source[s..e].to_string(), dump.to_string(), kind.to_string()))
}

/// The link of the `CHECKSUM.txt` beside a dump file's link: the file name
/// in it replaced by the checksum file's (works for a `?download=` link, a
/// bucket link and a plain path alike).
pub fn checksum_url_beside(source: &str) -> Option<String> {
    let (s, e, dump, _) = find_dump_file(source)?;
    Some(format!("{}{}{}", &source[..s], checksum_file(dump), &source[e..]))
}

/// A `CHECKSUM.txt`: `<sha256> <file>` per line (Discogs writes one space;
/// any whitespace and a `*` binary marker are accepted). File name →
/// lowercase digest; lines that are not a digest and a name are skipped.
pub fn parse_checksums(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(digest), Some(file)) = (parts.next(), parts.next()) else {
            continue;
        };
        if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            out.insert(
                file.strip_prefix('*').unwrap_or(file).to_string(),
                digest.to_ascii_lowercase(),
            );
        }
    }
    out
}

/// The dumps a year's listing names, oldest first: those whose
/// `CHECKSUM.txt` is listed (a dump without one is still being written).
pub fn dumps_in_listing(html: &str) -> Vec<String> {
    let mut dumps = Vec::new();
    let mut from = 0;
    while let Some(p) = html[from..].find("_CHECKSUM.txt") {
        let end = from + p;
        if end >= 16 && html.is_char_boundary(end - 16) && html[end - 16..].starts_with("discogs_") {
            let dump = &html[end - 8..end];
            if is_dump(dump) {
                dumps.push(dump.to_string());
            }
        }
        from = end + 13;
    }
    dumps.sort();
    dumps.dedup();
    dumps
}

/// Today's UTC date as `(year, month 1–12)`.
pub fn today_utc() -> (i32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, _) = civil_from_days((secs / 86_400) as i64);
    (y, m)
}

/// Days since 1970-01-01 to `(year, month, day)` (proleptic Gregorian;
/// Howard Hinnant's algorithm).
pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_links() {
        assert_eq!(dump_file("20261101", "releases"), "discogs_20261101_releases.xml.gz");
        assert_eq!(
            download_url(DISCOGS_DATA, "20261101", "discogs_20261101_CHECKSUM.txt"),
            "https://data.discogs.com/?download=data%2F2026%2Fdiscogs_20261101_CHECKSUM.txt"
        );
        assert_eq!(
            listing_url("http://127.0.0.1:9", 2026),
            "http://127.0.0.1:9/?prefix=data/2026/"
        );
    }

    #[test]
    fn dump_file_in_links_and_paths() {
        let link = "https://data.discogs.com/?download=data%2F2026%2Fdiscogs_20261101_releases.xml.gz";
        assert_eq!(
            dump_file_in(link),
            Some((
                "discogs_20261101_releases.xml.gz".into(),
                "20261101".into(),
                "releases".into()
            ))
        );
        assert_eq!(
            checksum_url_beside(link).unwrap(),
            "https://data.discogs.com/?download=data%2F2026%2Fdiscogs_20261101_CHECKSUM.txt"
        );
        assert_eq!(
            checksum_url_beside("/d/discogs_2026_x/discogs_20261101_masters.xml.gz").unwrap(),
            "/d/discogs_2026_x/discogs_20261101_CHECKSUM.txt"
        );
        assert_eq!(dump_file_in("releases.xml.gz"), None);
        assert_eq!(dump_file_in("discogs_2026110_releases.xml.gz"), None);
    }

    #[test]
    fn checksums_parse() {
        let a = "0b9886e30752b29781891bbaf0663e0876d8196784326cd23d561d69edea79c8";
        let b = "75EB03AEB1D03A07797FFDCB57A4B81C87CF10955353C800FA7B6BC86AAC1D67";
        let text = format!(
            "{a} discogs_20261101_labels.xml.gz\n{b}  *discogs_20261101_masters.xml.gz\r\n\nnot a line\nabc file\n"
        );
        let sums = parse_checksums(&text);
        assert_eq!(sums.len(), 2);
        assert_eq!(sums["discogs_20261101_labels.xml.gz"], a);
        assert_eq!(sums["discogs_20261101_masters.xml.gz"], b.to_ascii_lowercase());
    }

    #[test]
    fn listing_counts_dumps_with_a_checksum() {
        let html = r#"<a href="?download=data%2F2026%2Fdiscogs_20261001_CHECKSUM.txt">discogs_20261001_CHECKSUM.txt</a>
<a href="?download=data%2F2026%2Fdiscogs_20260901_CHECKSUM.txt">discogs_20260901_CHECKSUM.txt</a>
<a href="?download=data%2F2026%2Fdiscogs_20261101_releases.xml.gz">discogs_20261101_releases.xml.gz</a>"#;
        assert_eq!(dumps_in_listing(html), ["20260901", "20261001"]);
        assert!(dumps_in_listing("_CHECKSUM.txt é_CHECKSUM.txt").is_empty());
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_766), (2026, 11, 9));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
