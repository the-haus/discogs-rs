//! A synthetic dump generator for benchmarks: releases or masters shaped
//! like the 2026-11-01 dump's (checked against its first records): main
//! artists, labels, extra artists, formats, genres, styles, release date,
//! notes, data quality, a `<master_id>` on every release (`0` when it has
//! no master), a tracklist with track artists, identifiers, videos,
//! companies; about 4 KB per release, as in the real file.
//!
//! Main artist ids are uniform over `1..=max_artist_id`, so the share of
//! records that hit a target set is chosen by the targets, not the file:
//! [`targets_for_hit_rate`] picks every m-th id.

use std::io::{self, Write};

use crate::scan::RecordKind;

/// What to generate.
#[derive(Clone, Debug)]
pub struct GenerateOptions {
    pub kind: RecordKind,
    /// Stop after about this many bytes of XML.
    pub bytes: u64,
    pub seed: u64,
    /// Main artist ids are drawn from `1..=max_artist_id`.
    pub max_artist_id: u32,
    /// The share of releases with a `<master_id>`.
    pub master_share: f64,
    /// The share of records crediting "Various" (194).
    pub various_share: f64,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            kind: RecordKind::Releases,
            bytes: 64 << 20,
            seed: 1,
            max_artist_id: 10_000_000,
            master_share: 0.8,
            various_share: 0.03,
        }
    }
}

/// The target ids that make about `rate` of the records hit: every m-th
/// id, m = round(1 / rate). With 1.3 main artists a record on average, the
/// real hit rate comes out a little higher; the run reports it.
pub fn targets_for_hit_rate(rate: f64, max_artist_id: u32) -> Vec<u32> {
    let m = (1.0 / rate.clamp(1e-9, 1.0)).round().max(1.0) as u32;
    (1..=max_artist_id).filter(|id| id % m == 0).collect()
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        ((self.next() >> 32) * n) >> 32
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
    fn pick<'a>(&mut self, list: &[&'a str]) -> &'a str {
        list[self.below(list.len() as u64) as usize]
    }
}

const WORDS: &[&str] = &[
    "night",
    "city",
    "dream",
    "love",
    "machine",
    "sound",
    "deep",
    "light",
    "fire",
    "water",
    "electric",
    "soul",
    "groove",
    "space",
    "time",
    "heart",
    "dance",
    "shadow",
    "storm",
    "echo",
    "signal",
    "river",
    "gold",
    "silver",
    "rhythm",
    "motion",
    "system",
    "future",
    "blue",
    "red",
    "black",
    "summer",
    "winter",
    "Café",
    "Østergaard",
    "Rock &amp; Roll",
    "Ünter",
    "the",
    "of",
    "in",
    "and",
];
const GENRES: &[&str] = &[
    "Electronic",
    "Electronic",
    "Electronic",
    "Rock",
    "Rock",
    "Pop",
    "Hip Hop",
    "Jazz",
    "Funk / Soul",
    "Reggae",
    "Latin",
    "Folk, World, &amp; Country",
    "Blues",
    "Classical",
    "Stage &amp; Screen",
];
const STYLES: &[&str] = &[
    "House",
    "Techno",
    "Deep House",
    "Electro",
    "Dub",
    "Rock &amp; Roll",
    "Drum n Bass",
    "Ambient",
    "Synth-pop",
    "Disco",
    "Indie Rock",
    "Punk",
    "Pop Rock",
    "Downtempo",
    "Trance",
    "Hardcore",
    "Experimental",
    "Soul",
    "Boom Bap",
    "Dubstep",
    "Minimal",
    "Tech House",
    "Breakbeat",
    "Garage Rock",
];
const FORMATS: &[(&str, &[&str])] = &[
    ("Vinyl", &["12&quot;", "33 ⅓ RPM", "EP"]),
    ("Vinyl", &["LP", "Album"]),
    ("Vinyl", &["7&quot;", "45 RPM", "Single"]),
    ("CD", &["Album"]),
    ("CD", &["Compilation"]),
    ("CD", &["Mixed", "Compilation"]),
    ("File", &["MP3", "EP"]),
    ("Vinyl", &["12&quot;", "33 ⅓ RPM"]),
    ("Cassette", &["Album", "Promo"]),
];
const COUNTRIES: &[&str] = &[
    "US",
    "UK",
    "Germany",
    "Netherlands",
    "France",
    "Japan",
    "Belgium",
    "Italy",
    "Sweden",
];
const QUALITY: &[&str] = &[
    "Needs Vote",
    "Needs Vote",
    "Correct",
    "Complete and Correct",
    "Needs Minor Changes",
];
const ROLES: &[&str] = &[
    "Producer",
    "Mastered By",
    "Written-By",
    "Remix",
    "Design",
    "Mixed By",
    "Vocals",
];

struct Gen {
    rng: Rng,
    o: GenerateOptions,
    buf: Vec<u8>,
}

impl Gen {
    fn words(&mut self, n: u64) {
        let n = 1 + self.rng.below(n);
        for i in 0..n {
            if i > 0 {
                self.buf.push(b' ');
            }
            let w = self.rng.pick(WORDS);
            self.buf.extend_from_slice(w.as_bytes());
        }
    }

    fn put(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }

    fn num(&mut self, n: u64) {
        self.buf.extend_from_slice(n.to_string().as_bytes());
    }

    fn artist(&mut self, id: u64, role: Option<&str>) {
        self.put("<artist><id>");
        self.num(id);
        self.put("</id><name>");
        self.words(3);
        self.put("</name>");
        if let Some(r) = role {
            self.put("<role>");
            self.put(r);
            self.put("</role>");
        }
        self.put("</artist>");
    }

    fn main_artists(&mut self) {
        self.put("<artists>");
        let max = self.o.max_artist_id as u64;
        if self.rng.chance(self.o.various_share) {
            self.artist(194, None);
        } else {
            let n = match self.rng.below(10) {
                0..=7 => 1,
                8 => 2,
                _ => 3,
            };
            for _ in 0..n {
                let id = 1 + self.rng.below(max);
                self.artist(id, None);
            }
        }
        self.put("</artists>");
    }

    fn genres_styles(&mut self) {
        self.put("<genres>");
        for _ in 0..1 + self.rng.below(2) {
            self.put("<genre>");
            let g = self.rng.pick(GENRES);
            self.put(g);
            self.put("</genre>");
        }
        self.put("</genres><styles>");
        for _ in 0..1 + self.rng.below(3) {
            self.put("<style>");
            let s = self.rng.pick(STYLES);
            self.put(s);
            self.put("</style>");
        }
        self.put("</styles>");
    }

    fn videos(&mut self) {
        self.put("<videos>");
        for _ in 0..self.rng.below(3) {
            self.put("<video src=\"https://www.youtube.com/watch?v=");
            let v = self.rng.next();
            self.num(v % 100_000_000_000);
            self.put("\" duration=\"");
            let d = 60 + self.rng.below(500);
            self.num(d);
            self.put("\" embed=\"true\"><title>");
            self.words(5);
            self.put("</title><description>");
            self.words(8);
            self.put("</description></video>");
        }
        self.put("</videos>");
    }

    fn release(&mut self, id: u64) {
        let max = self.o.max_artist_id as u64;
        self.put("<release id=\"");
        self.num(id);
        self.put("\">");
        self.main_artists();
        self.put("<title>");
        self.words(4);
        self.put("</title><labels><label name=\"");
        self.words(2);
        self.put("\" catno=\"CAT-");
        let c = self.rng.below(1000);
        self.num(c);
        self.put("\" id=\"");
        let l = self.rng.below(2_000_000);
        self.num(l);
        self.put("\"/></labels><extraartists>");
        for _ in 0..self.rng.below(5) {
            let a = 1 + self.rng.below(max);
            let role = self.rng.pick(ROLES);
            self.artist(a, Some(role));
        }
        self.put("</extraartists><formats>");
        let (name, descriptions) = FORMATS[self.rng.below(FORMATS.len() as u64) as usize];
        self.put("<format name=\"");
        self.put(name);
        self.put("\" qty=\"1\" text=\"\"><descriptions>");
        for d in descriptions {
            self.put("<description>");
            self.put(d);
            self.put("</description>");
        }
        self.put("</descriptions></format></formats>");
        self.genres_styles();
        self.put("<country>");
        let c = self.rng.pick(COUNTRIES);
        self.put(c);
        self.put("</country>");
        if !self.rng.chance(0.1) {
            self.put("<released>");
            let y = 1950 + self.rng.below(76);
            self.num(y);
            if self.rng.chance(0.6) {
                self.put("-03-00");
            }
            self.put("</released>");
        }
        if self.rng.chance(0.4) {
            self.put("<notes>");
            for _ in 0..1 + self.rng.below(6) {
                self.words(12);
                self.put(".&#13;\n");
            }
            self.put("</notes>");
        }
        self.put("<data_quality>");
        let q = self.rng.pick(QUALITY);
        self.put(q);
        self.put("</data_quality>");
        if self.rng.chance(self.o.master_share) {
            self.put("<master_id is_main_release=\"");
            let main = if self.rng.chance(0.3) { "true" } else { "false" };
            self.put(main);
            self.put("\">");
            let m = 1 + self.rng.below(3_500_000);
            self.num(m);
            self.put("</master_id>");
        } else {
            self.put("<master_id is_main_release=\"false\">0</master_id>");
        }
        self.put("<tracklist>");
        for t in 0..2 + self.rng.below(11) {
            self.put("<track><position>");
            self.num(t + 1);
            self.put("</position><title>");
            self.words(4);
            self.put("</title><duration>");
            let m = 2 + self.rng.below(8);
            self.num(m);
            self.put(":");
            let s = 10 + self.rng.below(50);
            self.num(s);
            self.put("</duration>");
            if self.rng.chance(0.2) {
                self.put("<artists>");
                let a = 1 + self.rng.below(max);
                self.artist(a, None);
                self.put("</artists>");
            }
            if self.rng.chance(0.2) {
                self.put("<extraartists>");
                let a = 1 + self.rng.below(max);
                let role = self.rng.pick(ROLES);
                self.artist(a, Some(role));
                self.put("</extraartists>");
            }
            self.put("</track>");
        }
        self.put("</tracklist><identifiers><identifier type=\"Barcode\" value=\"");
        let b = self.rng.next() % 10_000_000_000_000;
        self.num(b);
        self.put("\"/><identifier type=\"Matrix / Runout\" description=\"Side A\" value=\"");
        self.words(2);
        self.put("\"/></identifiers>");
        self.videos();
        self.put("<companies><company><id>");
        let c = self.rng.below(2_000_000);
        self.num(c);
        self.put("</id><name>");
        self.words(3);
        self.put("</name><catno></catno><entity_type>13</entity_type><entity_type_name>Phonographic Copyright (p)</entity_type_name><resource_url>https://api.discogs.com/labels/");
        self.num(c);
        self.put("</resource_url></company></companies></release>\n");
    }

    fn master(&mut self, id: u64) {
        self.put("<master id=\"");
        self.num(id);
        self.put("\"><main_release>");
        let r = self.rng.below(30_000_000);
        self.num(r);
        self.put("</main_release>");
        self.main_artists();
        self.genres_styles();
        self.put("<year>");
        let y = if self.rng.chance(0.08) {
            0
        } else {
            1950 + self.rng.below(76)
        };
        self.num(y);
        self.put("</year><title>");
        self.words(4);
        self.put("</title><data_quality>");
        let q = self.rng.pick(QUALITY);
        self.put(q);
        self.put("</data_quality>");
        self.videos();
        self.put("</master>\n");
    }
}

/// Writes a synthetic dump of about `options.bytes` bytes of XML to `out`;
/// returns the bytes and records written.
pub fn generate<W: Write>(options: &GenerateOptions, mut out: W) -> io::Result<(u64, u64)> {
    let mut g = Gen {
        rng: Rng(options.seed),
        o: options.clone(),
        buf: Vec::with_capacity(2 << 20),
    };
    let (root, end) = match options.kind {
        RecordKind::Releases => ("<releases>\n", "</releases>\n"),
        RecordKind::Masters => ("<masters>\n", "</masters>\n"),
    };
    g.put(root);
    let mut written = 0u64;
    let mut records = 0u64;
    while written + (g.buf.len() as u64) < options.bytes {
        records += 1;
        match options.kind {
            RecordKind::Releases => g.release(records),
            RecordKind::Masters => g.master(records),
        }
        if g.buf.len() >= 1 << 20 {
            out.write_all(&g.buf)?;
            written += g.buf.len() as u64;
            g.buf.clear();
        }
    }
    g.put(end);
    out.write_all(&g.buf)?;
    written += g.buf.len() as u64;
    out.flush()?;
    Ok((written, records))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_sized() {
        let o = GenerateOptions {
            bytes: 200_000,
            ..Default::default()
        };
        let mut a = Vec::new();
        let (n, records) = generate(&o, &mut a).unwrap();
        let mut b = Vec::new();
        generate(&o, &mut b).unwrap();
        assert_eq!(a, b);
        assert_eq!(n, a.len() as u64);
        assert!(a.len() >= 200_000 && a.len() < 220_000);
        assert!(records > 20);
        let m = GenerateOptions {
            kind: RecordKind::Masters,
            bytes: 50_000,
            ..Default::default()
        };
        let mut c = Vec::new();
        generate(&m, &mut c).unwrap();
        assert!(c.starts_with(b"<masters>"));
    }

    #[test]
    fn hit_rate_targets() {
        let t = targets_for_hit_rate(0.1, 1000);
        assert_eq!(t.len(), 100);
        assert_eq!(t[0], 10);
    }
}
