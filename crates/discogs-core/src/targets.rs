//! The target filter: a bitset over Discogs artist ids.
//!
//! Discogs artist ids are dense integers (about 10 million in 2026), so a
//! bit per id up to the largest target costs at most a few megabytes and
//! answers "is this a target?" with one shift and one load.

use crate::Error;

/// A set of Discogs artist ids, as one bit per id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetSet {
    words: Vec<u64>,
    len: usize,
}

impl TargetSet {
    /// A set of the given ids.
    pub fn from_ids<I: IntoIterator<Item = u32>>(ids: I) -> Self {
        let mut set = Self::default();
        for id in ids {
            set.insert(id);
        }
        set
    }

    /// Adds `id`; the set grows to hold it (an id costs a bit, up to the
    /// largest: 4.3 million ids up to 10 million take 1.25 MB).
    pub fn insert(&mut self, id: u32) {
        let word = (id >> 6) as usize;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        let bit = 1u64 << (id & 63);
        if self.words[word] & bit == 0 {
            self.words[word] |= bit;
            self.len += 1;
        }
    }

    /// Whether `id` is in the set.
    #[inline]
    pub fn contains(&self, id: u64) -> bool {
        match self.words.get((id >> 6) as usize) {
            Some(w) => w & (1u64 << (id & 63)) != 0,
            None => false,
        }
    }

    /// The number of ids in the set.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bytes held by the bitset.
    pub fn bytes(&self) -> usize {
        self.words.len() * 8
    }

    /// Parses a targets file: one id per line, as ASCII digits. Blank lines
    /// and lines starting with `#` are skipped; surrounding whitespace and a
    /// trailing `\r` are ignored. Anything else is an error, with its line.
    pub fn parse(text: &[u8]) -> Result<Self, Error> {
        let mut set = Self::default();
        for (n, line) in text.split(|&b| b == b'\n').enumerate() {
            let line = line.trim_ascii();
            if line.is_empty() || line[0] == b'#' {
                continue;
            }
            match parse_digits(line) {
                Some((id, used)) if used == line.len() && id <= u32::MAX as u64 => set.insert(id as u32),
                _ => {
                    return Err(Error::Targets(format!(
                        "line {}: not a Discogs artist id: {:?}",
                        n + 1,
                        String::from_utf8_lossy(line)
                    )))
                }
            }
        }
        Ok(set)
    }
}

/// The ASCII digits at the start of `bytes` as a number, and how many bytes
/// they took. `None` when `bytes` does not start with a digit. More than 19
/// digits saturate (no Discogs id comes near).
#[inline]
pub fn parse_digits(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut n: u64 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let d = bytes[i].wrapping_sub(b'0');
        if d > 9 {
            break;
        }
        n = n.saturating_mul(10).saturating_add(d as u64);
        i += 1;
    }
    (i > 0).then_some((n, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits() {
        let s = TargetSet::from_ids([0, 1, 63, 64, 1_000_000, 64]);
        assert_eq!(s.len(), 5);
        for id in [0, 1, 63, 64, 1_000_000] {
            assert!(s.contains(id));
        }
        for id in [2, 62, 65, 999_999, 1_000_001, u32::MAX as u64 + 64, u64::MAX] {
            assert!(!s.contains(id));
        }
    }

    #[test]
    fn parse_file() {
        let s = TargetSet::parse(b"# targets\n1\n 22 \r\n\n333\r\n").unwrap();
        assert_eq!(s.len(), 3);
        assert!(s.contains(22) && s.contains(333));
        assert!(TargetSet::parse(b"1\nx2\n").is_err());
        assert!(TargetSet::parse(b"12a\n").is_err());
        assert!(TargetSet::parse(b"4294967296\n").is_err());
    }

    #[test]
    fn digits() {
        assert_eq!(parse_digits(b"123<"), Some((123, 3)));
        assert_eq!(parse_digits(b"<1"), None);
        assert_eq!(parse_digits(b"99999999999999999999999"), Some((u64::MAX, 23)));
    }
}
