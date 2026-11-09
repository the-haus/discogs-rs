//! The record scanner: Discogs records read straight from decompressed
//! bytes.
//!
//! A record costs a `memmem` for its end tag, one for its main `<artists>`
//! and a few digit reads. Only a record that credits a target artist (a
//! bit in a [`TargetSet`]) has its other fields read: master id, status,
//! data quality, genres, styles, year and formats, with entities decoded in
//! those slices only.
//!
//! What counts (the same rules as Festimap's TypeScript reader):
//!
//! - only the record's MAIN `<artists>`: the first `<artists>` element, and
//!   only when it comes before `<tracklist>` (a track's artists are inside
//!   the tracklist; `<extraartists>` is another element);
//! - each main artist once; with *n* of them, each gets 1/*n*;
//! - never a record crediting "Various" (194);
//! - never one whose `<data_quality>` is "Entirely Incorrect";
//! - releases: never one with a master (its master counts, once): a
//!   `<master_id>` above 0. The 2026 dumps write
//!   `<master_id is_main_release="false">0</master_id>` on every release
//!   that has none, so the element alone says nothing;
//! - releases: never one whose `status` attribute is set and not
//!   `Accepted` (the 2026 dumps carry no status: every release in them is
//!   accepted).

use memchr::memmem::Finder;

use crate::aggregate::{fixed_reciprocal, Aggregator, RecordFacts, RowKind};
use crate::format::{self, FormatClass};
use crate::targets::{parse_digits, TargetSet};
use crate::xml;

/// Discogs' "Various" artist, credited on compilations.
pub const VARIOUS: u64 = 194;

/// Which dump file the records come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecordKind {
    /// `discogs_*_releases.xml.gz`: `<release id="…" status="…">…</release>`.
    Releases,
    /// `discogs_*_masters.xml.gz`: `<master id="…">…</master>`.
    Masters,
}

impl RecordKind {
    /// The record's start tag up to its first attribute.
    pub fn open_tag(self) -> &'static [u8] {
        match self {
            RecordKind::Releases => b"<release ",
            RecordKind::Masters => b"<master ",
        }
    }

    /// The record's end tag.
    pub fn close_tag(self) -> &'static [u8] {
        match self {
            RecordKind::Releases => b"</release>",
            RecordKind::Masters => b"</master>",
        }
    }

    /// `releases` or `masters`.
    pub fn as_str(self) -> &'static str {
        match self {
            RecordKind::Releases => "releases",
            RecordKind::Masters => "masters",
        }
    }

    /// The kind of a dump from its first bytes (the root element), if they
    /// say.
    pub fn sniff(head: &[u8]) -> Option<Self> {
        let r = memchr::memmem::find(head, b"<releases");
        let m = memchr::memmem::find(head, b"<masters");
        match (r, m) {
            (Some(r), Some(m)) => Some(if r < m {
                RecordKind::Releases
            } else {
                RecordKind::Masters
            }),
            (Some(_), None) => Some(RecordKind::Releases),
            (None, Some(_)) => Some(RecordKind::Masters),
            (None, None) => None,
        }
    }
}

impl std::str::FromStr for RecordKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "releases" | "release" => Ok(RecordKind::Releases),
            "masters" | "master" => Ok(RecordKind::Masters),
            _ => Err(format!("unknown record kind {s:?} (releases or masters)")),
        }
    }
}

/// What a scan saw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Records seen.
    pub records: u64,
    /// Records whose main artists include a target.
    pub hits: u64,
    /// Hits counted (not skipped below).
    pub matched: u64,
    /// Hits that credit "Various".
    pub skipped_various: u64,
    /// Release hits that have a master.
    pub skipped_master: u64,
    /// Release hits whose status is not Accepted.
    pub skipped_status: u64,
    /// Hits whose data quality is "Entirely Incorrect".
    pub skipped_quality: u64,
}

impl Counters {
    /// Adds another scan's counts.
    pub fn add(&mut self, o: &Counters) {
        self.records += o.records;
        self.hits += o.hits;
        self.matched += o.matched;
        self.skipped_various += o.skipped_various;
        self.skipped_master += o.skipped_master;
        self.skipped_status += o.skipped_status;
        self.skipped_quality += o.skipped_quality;
    }
}

/// A record scanner for one dump kind. Holds its search tables and scratch
/// space: make one per thread.
#[derive(Clone)]
pub struct Scanner {
    kind: RecordKind,
    open: Finder<'static>,
    close: Finder<'static>,
    artists: Finder<'static>,
    artists_end: Finder<'static>,
    id: Finder<'static>,
    tracklist: Finder<'static>,
    master_id: Finder<'static>,
    quality: Finder<'static>,
    quality_end: Finder<'static>,
    genres: Finder<'static>,
    genres_end: Finder<'static>,
    genre: Finder<'static>,
    genre_end: Finder<'static>,
    styles: Finder<'static>,
    styles_end: Finder<'static>,
    style: Finder<'static>,
    style_end: Finder<'static>,
    year: Finder<'static>,
    year_end: Finder<'static>,
    formats: Finder<'static>,
    formats_end: Finder<'static>,
    ids: Vec<u64>,
}

impl Scanner {
    pub fn new(kind: RecordKind) -> Self {
        let (year, year_end): (&'static [u8], &'static [u8]) = match kind {
            RecordKind::Releases => (b"<released>", b"</released>"),
            RecordKind::Masters => (b"<year>", b"</year>"),
        };
        Self {
            kind,
            open: Finder::new(kind.open_tag()),
            close: Finder::new(kind.close_tag()),
            artists: Finder::new(b"<artists>"),
            artists_end: Finder::new(b"</artists>"),
            id: Finder::new(b"<id>"),
            tracklist: Finder::new(b"<tracklist"),
            master_id: Finder::new(b"<master_id"),
            quality: Finder::new(b"<data_quality>"),
            quality_end: Finder::new(b"</data_quality>"),
            genres: Finder::new(b"<genres>"),
            genres_end: Finder::new(b"</genres>"),
            genre: Finder::new(b"<genre>"),
            genre_end: Finder::new(b"</genre>"),
            styles: Finder::new(b"<styles>"),
            styles_end: Finder::new(b"</styles>"),
            style: Finder::new(b"<style>"),
            style_end: Finder::new(b"</style>"),
            year: Finder::new(year),
            year_end: Finder::new(year_end),
            formats: Finder::new(b"<formats>"),
            formats_end: Finder::new(b"</formats>"),
            ids: Vec::with_capacity(16),
        }
    }

    pub fn kind(&self) -> RecordKind {
        self.kind
    }

    /// Scans every whole record in `block` (records are found by their
    /// start tag and the first end tag after it; anything else, such as the
    /// root element or a cut record at the end, is ignored).
    pub fn scan_block(&mut self, block: &[u8], targets: &TargetSet, agg: &mut Aggregator, counters: &mut Counters) {
        let close_len = self.kind.close_tag().len();
        let mut at = 0;
        while let Some(o) = self.open.find(&block[at..]) {
            let start = at + o;
            let Some(e) = self.close.find(&block[start..]) else {
                break;
            };
            let end = start + e + close_len;
            self.scan_record(&block[start..end], targets, agg, counters);
            at = end;
        }
    }

    /// Scans one record: `rec` runs from its start tag to the end of its end
    /// tag.
    pub fn scan_record(&mut self, rec: &[u8], targets: &TargetSet, agg: &mut Aggregator, counters: &mut Counters) {
        counters.records += 1;
        // The main artists: the first <artists>, before any <tracklist>.
        let Some(a) = self.artists.find(rec) else { return };
        if self.tracklist.find(&rec[..a]).is_some() {
            return;
        }
        let a_start = a + b"<artists>".len();
        let Some(a_len) = self.artists_end.find(&rec[a_start..]) else {
            return;
        };
        let block = &rec[a_start..a_start + a_len];

        self.ids.clear();
        let mut hit = false;
        let mut at = 0;
        while let Some(i) = self.id.find(&block[at..]) {
            let digits = at + i + b"<id>".len();
            at = digits;
            let Some((id, used)) = parse_digits(&block[digits..]) else {
                continue;
            };
            at += used;
            if self.ids.contains(&id) {
                continue;
            }
            self.ids.push(id);
            hit |= targets.contains(id);
        }
        if !hit {
            return;
        }
        counters.hits += 1;

        if self.ids.contains(&VARIOUS) {
            counters.skipped_various += 1;
            return;
        }
        if self.kind == RecordKind::Releases {
            if self.master_of(rec).is_some_and(|m| m > 0) {
                counters.skipped_master += 1;
                return;
            }
            let tag_end = memchr::memchr(b'>', rec).unwrap_or(rec.len());
            if let Some(status) = xml::attr(&rec[..tag_end], b"status=\"") {
                if status != b"Accepted" {
                    counters.skipped_status += 1;
                    return;
                }
            }
        }
        if let Some(q) = xml::text(rec, &self.quality, &self.quality_end) {
            if q == b"Entirely Incorrect" {
                counters.skipped_quality += 1;
                return;
            }
        }
        counters.matched += 1;
        self.count(rec, targets, agg);
    }

    /// A release's `<master_id …>N</master_id>` value.
    fn master_of(&self, rec: &[u8]) -> Option<u64> {
        let at = self.master_id.find(rec)? + b"<master_id".len();
        let gt = memchr::memchr(b'>', &rec[at..])?;
        let text = &rec[at + gt + 1..];
        let start = text.iter().position(|b| !b.is_ascii_whitespace())?;
        parse_digits(&text[start..]).map(|(n, _)| n)
    }

    /// Adds a counted record's rows to `agg`.
    fn count(&self, rec: &[u8], targets: &TargetSet, agg: &mut Aggregator) {
        let mut raw = Vec::new();
        let genres = self.names(
            rec,
            &self.genres,
            &self.genres_end,
            &self.genre,
            &self.genre_end,
            &mut raw,
        );
        let styles = self.names(
            rec,
            &self.styles,
            &self.styles_end,
            &self.style,
            &self.style_end,
            &mut raw,
        );

        let year = xml::text(rec, &self.year, &self.year_end).and_then(year_of);
        let (format_class, promo) = match self.kind {
            RecordKind::Masters => (None, false),
            RecordKind::Releases => match section(rec, &self.formats, &self.formats_end) {
                Some(f) => {
                    let f = format::classify(f);
                    (Some(f.class), f.promo)
                }
                None => (Some(FormatClass::Other), false),
            },
        };
        let facts = RecordFacts {
            year,
            format_class,
            promo,
        };

        let n = self.ids.len() as u64;
        let g = genres.len().max(1) as u64;
        let s = styles.len().max(1) as u64;
        let per_artist = fixed_reciprocal(n);
        let per_genre = fixed_reciprocal(n * g);
        let per_style = fixed_reciprocal(n * g * s);
        let mut pair = String::new();
        for &id in &self.ids {
            if !targets.contains(id) {
                continue;
            }
            // A target fits in u32: TargetSet holds only u32 ids.
            let artist = id as u32;
            agg.add(artist, RowKind::Total, "", facts, per_artist, per_artist);
            for genre in &genres {
                agg.add(artist, RowKind::Genre, genre, facts, per_artist, per_genre);
            }
            for style in &styles {
                if genres.is_empty() {
                    pair.clear();
                    pair.push('/');
                    pair.push_str(style);
                    agg.add(artist, RowKind::Style, &pair, facts, per_artist, per_style);
                }
                for genre in &genres {
                    pair.clear();
                    pair.push_str(genre);
                    pair.push('/');
                    pair.push_str(style);
                    agg.add(artist, RowKind::Style, &pair, facts, per_genre, per_style);
                }
            }
        }
    }

    /// The distinct, unescaped, non-empty texts of `<item>`s inside the
    /// record's `<list>…</list>`, in order.
    fn names<'r>(
        &self,
        rec: &'r [u8],
        list: &Finder<'_>,
        list_end: &Finder<'_>,
        item: &Finder<'_>,
        item_end: &Finder<'_>,
        raw: &mut Vec<&'r [u8]>,
    ) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let Some(slice) = section(rec, list, list_end) else {
            return out;
        };
        raw.clear();
        xml::texts(slice, item, item_end, raw);
        for r in raw.iter() {
            if r.is_empty() {
                continue;
            }
            let name = xml::unescape(r);
            let name = name.trim();
            if !name.is_empty() && !out.iter().any(|o| o == name) {
                out.push(name.to_string());
            }
        }
        out
    }
}

/// The bytes between the first `open` and the next `close`.
fn section<'r>(rec: &'r [u8], open: &Finder<'_>, close: &Finder<'_>) -> Option<&'r [u8]> {
    let o = open.find(rec)? + open.needle().len();
    let e = close.find(&rec[o..])?;
    Some(&rec[o..o + e])
}

/// A year from `YYYY` or `YYYY-MM-DD` (or anything starting with four
/// digits); `0`, a short or out-of-range number is no year.
pub fn year_of(text: &[u8]) -> Option<u16> {
    let (y, used) = parse_digits(text)?;
    (used == 4 && (1000..=9999).contains(&y)).then_some(y as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::Row;

    fn scan(kind: RecordKind, xml: &str, targets: &[u32]) -> (Vec<Row>, Counters) {
        let mut s = Scanner::new(kind);
        let t = TargetSet::from_ids(targets.iter().copied());
        let mut agg = Aggregator::new();
        let mut c = Counters::default();
        s.scan_block(xml.as_bytes(), &t, &mut agg, &mut c);
        (agg.into_rows(), c)
    }

    fn release(attrs: &str, body: &str) -> String {
        format!("<release id=\"1\"{attrs}>{body}</release>")
    }

    const MAIN: &str =
        "<artists><artist><id>10</id><name>A</name></artist><artist><id>20</id><name>B</name></artist></artists>";

    #[test]
    fn years() {
        assert_eq!(year_of(b"1999"), Some(1999));
        assert_eq!(year_of(b"1999-03-00"), Some(1999));
        assert_eq!(year_of(b"0"), None);
        assert_eq!(year_of(b"0000"), None);
        assert_eq!(year_of(b"199"), None);
        assert_eq!(year_of(b"19999"), None);
        assert_eq!(year_of(b"Unknown"), None);
    }

    #[test]
    fn two_artists_split_and_pairs() {
        let x = release(
            " status=\"Accepted\"",
            &format!(
                "{MAIN}<formats><format name=\"Vinyl\" qty=\"1\" text=\"\"><descriptions><description>EP</description></descriptions></format></formats>\
                 <genres><genre>Electronic</genre><genre>Hip Hop</genre></genres><styles><style>Electro</style></styles>\
                 <released>1984-05-00</released><data_quality>Correct</data_quality>"
            ),
        );
        let (rows, c) = scan(RecordKind::Releases, &x, &[10]);
        assert_eq!((c.records, c.hits, c.matched), (1, 1, 1));
        let names: Vec<_> = rows
            .iter()
            .map(|r| (r.kind, r.name.as_str(), r.weight, r.share))
            .collect();
        assert_eq!(
            names,
            vec![
                (RowKind::Genre, "Electronic", 0.5, 0.25),
                (RowKind::Genre, "Hip Hop", 0.5, 0.25),
                (RowKind::Style, "Electronic/Electro", 0.25, 0.25),
                (RowKind::Style, "Hip Hop/Electro", 0.25, 0.25),
                (RowKind::Total, "", 0.5, 0.5),
            ]
        );
        assert!(rows.iter().all(|r| r.artist_id == 10
            && r.year == Some(1984)
            && r.format_class == Some(FormatClass::Ep)
            && r.records == 1));
    }

    #[test]
    fn extra_and_track_artists_do_not_count() {
        let x = release(
            "",
            "<artists><artist><id>10</id></artist></artists>\
             <extraartists><artist><id>30</id></artist></extraartists>\
             <genres><genre>Rock</genre></genres>\
             <tracklist><track><artists><artist><id>40</id></artist></artists></track></tracklist>",
        );
        let (rows, c) = scan(RecordKind::Releases, &x, &[30, 40]);
        assert_eq!((c.hits, rows.len()), (0, 0));
        // Weight 1: the extra and track artists are not main artists.
        let (rows, _) = scan(RecordKind::Releases, &x, &[10]);
        assert_eq!(rows[0].weight, 1.0);
    }

    #[test]
    fn no_main_artists_but_track_artists() {
        let x = release(
            "",
            "<tracklist><track><artists><artist><id>40</id></artist></artists></track></tracklist>",
        );
        let (_, c) = scan(RecordKind::Releases, &x, &[40]);
        assert_eq!(c.hits, 0);
    }

    #[test]
    fn skips() {
        let various = release(
            "",
            "<artists><artist><id>194</id></artist><artist><id>10</id></artist></artists>",
        );
        assert_eq!(scan(RecordKind::Releases, &various, &[10]).1.skipped_various, 1);
        let mastered = release("", &format!("{MAIN}<master_id is_main_release=\"true\">5</master_id>"));
        assert_eq!(scan(RecordKind::Releases, &mastered, &[10]).1.skipped_master, 1);
        let not_main = release("", &format!("{MAIN}<master_id is_main_release=\"false\">5</master_id>"));
        assert_eq!(scan(RecordKind::Releases, &not_main, &[10]).1.skipped_master, 1);
        // The 2026 dumps' "no master".
        let masterless = release("", &format!("{MAIN}<master_id is_main_release=\"false\">0</master_id>"));
        assert_eq!(scan(RecordKind::Releases, &masterless, &[10]).1.matched, 1);
        let draft = release(" status=\"Draft\"", MAIN);
        assert_eq!(scan(RecordKind::Releases, &draft, &[10]).1.skipped_status, 1);
        let wrong = release("", &format!("{MAIN}<data_quality>Entirely Incorrect</data_quality>"));
        assert_eq!(scan(RecordKind::Releases, &wrong, &[10]).1.skipped_quality, 1);
        let fine = release(
            " status=\"Accepted\"",
            &format!("{MAIN}<data_quality>Needs Vote</data_quality>"),
        );
        assert_eq!(scan(RecordKind::Releases, &fine, &[10]).1.matched, 1);
    }

    #[test]
    fn masters_have_years_no_formats() {
        let x = "<masters><master id=\"5\"><main_release>9</main_release><artists><artist><id>10</id></artist><artist><id>10</id></artist></artists>\
                 <genres><genre>Funk / Soul</genre></genres><styles><style>Disco</style><style>Disco</style><style>Boogie</style></styles>\
                 <year>0</year></master></masters>";
        let (rows, c) = scan(RecordKind::Masters, x, &[10]);
        assert_eq!(c.matched, 1);
        let got: Vec<_> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.weight, r.share, r.year, r.format_class))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Funk / Soul", 1.0, 1.0, None, None),
                ("Funk / Soul/Boogie", 1.0, 0.5, None, None),
                ("Funk / Soul/Disco", 1.0, 0.5, None, None),
                ("", 1.0, 1.0, None, None),
            ]
        );
    }

    #[test]
    fn entities_and_no_genres() {
        let x = release(
            "",
            "<artists><artist><id>10</id></artist></artists><styles><style>Rock &amp; Roll</style></styles>",
        );
        let (rows, _) = scan(RecordKind::Releases, &x, &[10]);
        assert_eq!(rows[0].name, "/Rock & Roll");
        assert_eq!(rows[0].format_class, Some(FormatClass::Other));
    }

    #[test]
    fn sniff() {
        assert_eq!(
            RecordKind::sniff(b"<?xml?><releases><release"),
            Some(RecordKind::Releases)
        );
        assert_eq!(RecordKind::sniff(b"<masters><master "), Some(RecordKind::Masters));
        assert_eq!(RecordKind::sniff(b"<artists>"), None);
    }
}
