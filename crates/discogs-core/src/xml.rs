//! The little XML the scanner needs: element texts inside a slice, and
//! entity decoding for those texts only.

use std::borrow::Cow;

use memchr::memmem;

/// Decodes the five named XML entities and numeric character references
/// (`&#38;`, `&#x26;`). An unknown or malformed reference is kept as it
/// is. Invalid UTF-8 is replaced (U+FFFD), never an error.
pub fn unescape(bytes: &[u8]) -> Cow<'_, str> {
    if memchr::memchr(b'&', bytes).is_none() {
        return String::from_utf8_lossy(bytes);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'&' {
            out.push(b);
            i += 1;
            continue;
        }
        let Some(semi) = bytes[i + 1..].iter().take(12).position(|&c| c == b';') else {
            out.push(b);
            i += 1;
            continue;
        };
        let name = &bytes[i + 1..i + 1 + semi];
        let decoded: Option<char> = match name {
            b"amp" => Some('&'),
            b"lt" => Some('<'),
            b"gt" => Some('>'),
            b"quot" => Some('"'),
            b"apos" => Some('\''),
            [b'#', b'x' | b'X', hex @ ..] if !hex.is_empty() => std::str::from_utf8(hex)
                .ok()
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .and_then(char::from_u32),
            [b'#', dec @ ..] if !dec.is_empty() => std::str::from_utf8(dec)
                .ok()
                .and_then(|d| d.parse::<u32>().ok())
                .and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                i += semi + 2;
            }
            None => {
                out.push(b);
                i += 1;
            }
        }
    }
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

/// The texts of every `<tag>…</tag>` in `hay`, raw (not unescaped), each
/// trimmed of ASCII whitespace. `open` is `<tag>` and `close` `</tag>`.
pub fn texts<'a>(hay: &'a [u8], open: &memmem::Finder<'_>, close: &memmem::Finder<'_>, out: &mut Vec<&'a [u8]>) {
    let mut at = 0;
    while let Some(o) = open.find(&hay[at..]) {
        let start = at + o + open.needle().len();
        let Some(e) = close.find(&hay[start..]) else { break };
        out.push(hay[start..start + e].trim_ascii());
        at = start + e + close.needle().len();
    }
}

/// The text of the first `<tag>…</tag>` in `hay`, raw and trimmed.
pub fn text<'a>(hay: &'a [u8], open: &memmem::Finder<'_>, close: &memmem::Finder<'_>) -> Option<&'a [u8]> {
    let o = open.find(hay)?;
    let start = o + open.needle().len();
    let e = close.find(&hay[start..])?;
    Some(hay[start..start + e].trim_ascii())
}

/// The value of attribute `name` (with its `="`, e.g. `status="`) in a start
/// tag `tag` (the bytes up to its `>`), raw.
pub fn attr<'a>(tag: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let at = memmem::find(tag, name)?;
    // A whole attribute name: preceded by whitespace.
    if at == 0 || !tag[at - 1].is_ascii_whitespace() {
        return None;
    }
    let start = at + name.len();
    let end = memchr::memchr(b'"', &tag[start..])?;
    Some(&tag[start..start + end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities() {
        assert_eq!(unescape(b"Drum n Bass"), "Drum n Bass");
        assert_eq!(unescape(b"Rock &amp; Roll"), "Rock & Roll");
        assert_eq!(unescape(b"&lt;&gt;&quot;&apos;"), "<>\"'");
        assert_eq!(unescape(b"Caf&#233; &#xE9;&#XE9;"), "Café éé");
        assert_eq!(unescape(b"a & b &bogus; &#xZZ; &#;"), "a & b &bogus; &#xZZ; &#;");
        assert_eq!(unescape(b"&amp"), "&amp");
        assert_eq!(unescape(b"&#1114112;"), "&#1114112;");
    }

    #[test]
    fn element_texts() {
        let open = memmem::Finder::new(b"<style>");
        let close = memmem::Finder::new(b"</style>");
        let mut out = Vec::new();
        texts(
            b"<styles><style> House </style><style>Techno</style><style>x",
            &open,
            &close,
            &mut out,
        );
        assert_eq!(out, vec![&b"House"[..], b"Techno"]);
        assert_eq!(text(b"<style>A</style>", &open, &close), Some(&b"A"[..]));
    }

    #[test]
    fn attributes() {
        let tag = br#"<release id="12" status="Accepted""#;
        assert_eq!(attr(tag, b"status=\""), Some(&b"Accepted"[..]));
        assert_eq!(attr(tag, b"id=\""), Some(&b"12"[..]));
        assert_eq!(attr(br#"<release xid="1""#, b"id=\""), None);
    }
}
