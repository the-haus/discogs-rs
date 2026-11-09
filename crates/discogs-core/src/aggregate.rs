//! Per-thread aggregation of profile rows, and the merge at the end.
//!
//! Weights are summed in 32.32 fixed point, not in floats: every record
//! adds the same rounded integer whatever thread or block it lands in, and
//! integer sums do not depend on their order, so the output is identical
//! for any number of threads and any block size (a property test checks
//! this). They become `f32` only in the output rows.

use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::format::FormatClass;

/// One in 32.32 fixed point.
pub const FIXED_ONE: u64 = 1 << 32;

/// `1 / d` in 32.32 fixed point, rounded to nearest.
#[inline]
pub fn fixed_reciprocal(d: u64) -> u64 {
    let d = d.max(1);
    (FIXED_ONE + d / 2) / d
}

/// What a row counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum RowKind {
    /// A Discogs genre (`Electronic`).
    Genre = 0,
    /// A Discogs style under one of the record's genres (`Electronic/Techno`).
    Style = 1,
    /// The records themselves, whatever their genres (name `""`).
    Total = 2,
}

impl RowKind {
    /// `genre`, `style` or `total`.
    pub fn as_str(self) -> &'static str {
        match self {
            RowKind::Genre => "genre",
            RowKind::Style => "style",
            RowKind::Total => "total",
        }
    }
}

/// One output row: everything one artist's counted records add up to for
/// one (kind, name, year, format class, promo) key.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    pub artist_id: u32,
    pub kind: RowKind,
    pub name: String,
    pub year: Option<u16>,
    #[serde(serialize_with = "ser_format")]
    pub format_class: Option<FormatClass>,
    pub promo: bool,
    /// Σ 1/n over the records (n: the record's main artists); for a style
    /// row, Σ 1/(n·g) (g: the record's genres).
    pub weight: f32,
    /// The weight split over the record's rows of this kind: Σ 1/(n·g) for
    /// a genre row, Σ 1/(n·g·s) for a style row (s: the record's styles),
    /// Σ 1/n for a total row.
    pub share: f32,
    /// The records counted.
    pub records: u32,
}

fn ser_format<S: serde::Serializer>(f: &Option<FormatClass>, s: S) -> Result<S::Ok, S::Error> {
    match f {
        Some(c) => s.serialize_str(c.as_str()),
        None => s.serialize_none(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    artist: u32,
    name: u32,
    year: u16,
    kind: u8,
    /// [`FormatClass`] code, or `NO_FORMAT`.
    format: u8,
    promo: bool,
}

const NO_FORMAT: u8 = u8::MAX;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Acc {
    weight: u64,
    share: u64,
    records: u32,
}

/// A thread's running sums: rows by key, with names interned.
#[derive(Debug, Default)]
pub struct Aggregator {
    names: FxHashMap<Box<str>, u32>,
    name_list: Vec<Box<str>>,
    sums: FxHashMap<Key, Acc>,
}

/// The year, format class and promo flag of a record: shared by all its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordFacts {
    pub year: Option<u16>,
    pub format_class: Option<FormatClass>,
    pub promo: bool,
}

impl Aggregator {
    pub fn new() -> Self {
        Self::default()
    }

    fn intern(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.names.get(name) {
            return id;
        }
        let id = self.name_list.len() as u32;
        let boxed: Box<str> = name.into();
        self.name_list.push(boxed.clone());
        self.names.insert(boxed, id);
        id
    }

    /// Adds one record's part to one row: `weight` and `share` in 32.32
    /// fixed point.
    pub fn add(&mut self, artist: u32, kind: RowKind, name: &str, facts: RecordFacts, weight: u64, share: u64) {
        let name = self.intern(name);
        let key = Key {
            artist,
            name,
            year: facts.year.unwrap_or(0),
            kind: kind as u8,
            format: facts.format_class.map_or(NO_FORMAT, |f| f as u8),
            promo: facts.promo,
        };
        let acc = self.sums.entry(key).or_default();
        acc.weight += weight;
        acc.share += share;
        acc.records += 1;
    }

    /// The number of distinct rows so far.
    pub fn len(&self) -> usize {
        self.sums.len()
    }

    /// Whether there are no rows.
    pub fn is_empty(&self) -> bool {
        self.sums.is_empty()
    }

    /// Adds another aggregator's sums into this one.
    pub fn merge(&mut self, other: Aggregator) {
        if self.sums.is_empty() && self.name_list.is_empty() {
            *self = other;
            return;
        }
        let remap: Vec<u32> = other.name_list.iter().map(|n| self.intern(n)).collect();
        self.sums.reserve(other.sums.len());
        for (mut key, acc) in other.sums {
            key.name = remap[key.name as usize];
            let into = self.sums.entry(key).or_default();
            into.weight += acc.weight;
            into.share += acc.share;
            into.records += acc.records;
        }
    }

    /// The rows, sorted by artist, kind, name, year (none first), format
    /// class (none first) and promo.
    pub fn into_rows(self) -> Vec<Row> {
        let to_f32 = |x: u64| (x as f64 / FIXED_ONE as f64) as f32;
        let mut rows: Vec<Row> = self
            .sums
            .into_iter()
            .map(|(k, acc)| Row {
                artist_id: k.artist,
                kind: match k.kind {
                    0 => RowKind::Genre,
                    1 => RowKind::Style,
                    _ => RowKind::Total,
                },
                name: self.name_list[k.name as usize].to_string(),
                year: (k.year != 0).then_some(k.year),
                format_class: FormatClass::from_code(k.format),
                promo: k.promo,
                weight: to_f32(acc.weight),
                share: to_f32(acc.share),
                records: acc.records,
            })
            .collect();
        rows.sort_by(|a, b| {
            (a.artist_id, a.kind, &a.name, a.year, a.format_class, a.promo).cmp(&(
                b.artist_id,
                b.kind,
                &b.name,
                b.year,
                b.format_class,
                b.promo,
            ))
        });
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACTS: RecordFacts = RecordFacts {
        year: Some(1999),
        format_class: Some(FormatClass::Ep),
        promo: false,
    };

    #[test]
    fn reciprocals() {
        assert_eq!(fixed_reciprocal(1), FIXED_ONE);
        assert_eq!(fixed_reciprocal(2), FIXED_ONE / 2);
        assert_eq!(fixed_reciprocal(0), FIXED_ONE);
    }

    #[test]
    fn sums_and_merges() {
        let mut a = Aggregator::new();
        a.add(7, RowKind::Genre, "Electronic", FACTS, FIXED_ONE, FIXED_ONE);
        a.add(7, RowKind::Genre, "Electronic", FACTS, FIXED_ONE / 2, FIXED_ONE / 4);
        let mut b = Aggregator::new();
        b.add(7, RowKind::Style, "Electronic/House", FACTS, FIXED_ONE, FIXED_ONE);
        b.add(7, RowKind::Genre, "Electronic", FACTS, FIXED_ONE, FIXED_ONE);
        b.add(
            3,
            RowKind::Total,
            "",
            RecordFacts {
                year: None,
                format_class: None,
                promo: true,
            },
            1,
            1,
        );
        a.merge(b);
        let rows = a.into_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].artist_id, 3);
        assert_eq!(rows[0].year, None);
        assert_eq!(rows[1].name, "Electronic");
        assert_eq!(rows[1].weight, 2.5);
        assert_eq!(rows[1].share, 2.25);
        assert_eq!(rows[1].records, 3);
        assert_eq!(rows[2].kind, RowKind::Style);
    }
}
