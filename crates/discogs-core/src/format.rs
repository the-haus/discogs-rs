//! A release's format class, from its `<formats>` block.
//!
//! Discogs describes a release's formats as
//! `<format name="Vinyl" qty="1" text=""><descriptions><description>LP</description>
//! <description>Album</description></descriptions></format>`, with one
//! `<format>` per medium. The class reads every `<description>` of every
//! format, and picks the first that applies:
//!
//! | class | when a description is |
//! |---|---|
//! | `mix` | `Mixed`, `Mixtape` (a DJ mix, even when also a compilation) |
//! | `compilation` | `Compilation` |
//! | `album` | `Album` |
//! | `ep` | `EP`, `Mini-Album` |
//! | `album` | `LP` (a long-player not described further) |
//! | `single` | `Single`, `Maxi-Single`, or else a `7"` |
//! | `other` | anything else, or no description |
//!
//! Masters carry no formats: their rows have no class (`null`).
//!
//! Apart from the class, [`Formats::promo`] says whether the release is a
//! `Promo`, a `Test Pressing` or an `Unofficial Release`: Festimap weighs
//! those like compilations.

use memchr::memmem;

use crate::xml;

/// The class of a release's formats. See the module docs for the rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum FormatClass {
    Album = 0,
    Ep = 1,
    Single = 2,
    Compilation = 3,
    Mix = 4,
    Other = 5,
}

impl FormatClass {
    /// Every class, in code order.
    pub const ALL: [FormatClass; 6] = [
        FormatClass::Album,
        FormatClass::Ep,
        FormatClass::Single,
        FormatClass::Compilation,
        FormatClass::Mix,
        FormatClass::Other,
    ];

    /// The class's name in the output: `album`, `ep`, `single`,
    /// `compilation`, `mix` or `other`.
    pub fn as_str(self) -> &'static str {
        match self {
            FormatClass::Album => "album",
            FormatClass::Ep => "ep",
            FormatClass::Single => "single",
            FormatClass::Compilation => "compilation",
            FormatClass::Mix => "mix",
            FormatClass::Other => "other",
        }
    }

    /// The class of a code from [`FormatClass as u8`].
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(code as usize).copied()
    }
}

/// What a release's `<formats>` block says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Formats {
    pub class: FormatClass,
    /// A promo, test pressing or unofficial release.
    pub promo: bool,
}

/// Classifies the descriptions inside a `<formats>…</formats>` slice (or
/// any slice holding `<description>` elements).
pub fn classify(formats: &[u8]) -> Formats {
    let open = memmem::Finder::new(b"<description>");
    let close = memmem::Finder::new(b"</description>");
    let mut descriptions = Vec::new();
    xml::texts(formats, &open, &close, &mut descriptions);
    classify_descriptions(descriptions.iter().copied())
}

/// Classifies a list of raw description texts.
pub fn classify_descriptions<'a, I: IntoIterator<Item = &'a [u8]>>(descriptions: I) -> Formats {
    const MIX: u8 = 1;
    const COMPILATION: u8 = 2;
    const ALBUM: u8 = 4;
    const EP: u8 = 8;
    const LP: u8 = 16;
    const SINGLE: u8 = 32;
    const SEVEN: u8 = 64;
    let mut seen = 0u8;
    let mut promo = false;
    for d in descriptions {
        seen |= match d {
            b"Mixed" | b"Mixtape" => MIX,
            b"Compilation" => COMPILATION,
            b"Album" => ALBUM,
            b"EP" | b"Mini-Album" => EP,
            b"LP" => LP,
            b"Single" | b"Maxi-Single" => SINGLE,
            b"7\"" | b"7&quot;" => SEVEN,
            b"Promo" | b"Test Pressing" | b"Unofficial Release" => {
                promo = true;
                0
            }
            _ => 0,
        };
    }
    let class = if seen & MIX != 0 {
        FormatClass::Mix
    } else if seen & COMPILATION != 0 {
        FormatClass::Compilation
    } else if seen & ALBUM != 0 {
        FormatClass::Album
    } else if seen & EP != 0 {
        FormatClass::Ep
    } else if seen & LP != 0 {
        FormatClass::Album
    } else if seen & (SINGLE | SEVEN) != 0 {
        FormatClass::Single
    } else {
        FormatClass::Other
    };
    Formats { class, promo }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(descriptions: &[&str]) -> FormatClass {
        classify_descriptions(descriptions.iter().map(|d| d.as_bytes())).class
    }

    #[test]
    fn classes() {
        assert_eq!(class(&["LP", "Album"]), FormatClass::Album);
        assert_eq!(class(&["LP"]), FormatClass::Album);
        assert_eq!(class(&["12\"", "33 ⅓ RPM", "EP"]), FormatClass::Ep);
        assert_eq!(class(&["Mini-Album"]), FormatClass::Ep);
        assert_eq!(class(&["LP", "EP"]), FormatClass::Ep);
        assert_eq!(class(&["7\"", "45 RPM"]), FormatClass::Single);
        assert_eq!(class(&["7&quot;"]), FormatClass::Single);
        assert_eq!(class(&["CD", "Maxi-Single"]), FormatClass::Single);
        assert_eq!(class(&["Album", "Compilation"]), FormatClass::Compilation);
        assert_eq!(class(&["Compilation", "Mixed"]), FormatClass::Mix);
        assert_eq!(class(&["Mixtape"]), FormatClass::Mix);
        assert_eq!(class(&["12\"", "33 ⅓ RPM"]), FormatClass::Other);
        assert_eq!(class(&[]), FormatClass::Other);
    }

    #[test]
    fn promo() {
        assert!(classify_descriptions([&b"Promo"[..], b"EP"]).promo);
        assert!(classify_descriptions([&b"Test Pressing"[..]]).promo);
        assert!(classify_descriptions([&b"Unofficial Release"[..]]).promo);
        assert!(!classify_descriptions([&b"Album"[..]]).promo);
    }

    #[test]
    fn from_xml() {
        let f = br#"<formats><format name="Vinyl" qty="2" text=""><descriptions><description>LP</description><description>Compilation</description></descriptions></format><format name="CD" qty="1" text=""><descriptions><description>Promo</description></descriptions></format></formats>"#;
        assert_eq!(
            classify(f),
            Formats {
                class: FormatClass::Compilation,
                promo: true
            }
        );
        assert_eq!(
            classify(br#"<formats><format name="File" qty="1" text="MP3"/></formats>"#).class,
            FormatClass::Other
        );
    }

    #[test]
    fn codes_round_trip() {
        for c in FormatClass::ALL {
            assert_eq!(FormatClass::from_code(c as u8), Some(c));
        }
        assert_eq!(FormatClass::from_code(6), None);
    }
}
