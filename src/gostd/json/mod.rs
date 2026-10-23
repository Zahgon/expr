//! Emulation of Go's `encoding/json` for untyped values (`any`), number formatting, string quoting and
//! indentation.
//!
//! Follows the behaviour of Go's standard library encoding/json package (The Go Authors, BSD-style licence).
//!
//! All string data are byte strings: Go strings may hold invalid UTF-8, so inputs are `&[u8]` and outputs
//! `Vec<u8>`. Error values are `String`s holding exactly the text of Go's `error.Error()`.

mod decode;
mod encode;
mod indent;
mod scanner;

pub use decode::{decode_use_number, only_whitespace_follows, unmarshal, valid};
pub use encode::{encode_float, encode_string};
pub use indent::indent;

/// A decoded JSON value, the equivalent of what Go stores in an `any`.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null` (Go `nil`).
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, holding its literal text exactly as it appeared in the input (for example `-0.50e+3`).
    /// [`unmarshal`] only produces numbers whose text converts to a finite float64.
    Number(Vec<u8>),
    /// A string after unescaping: invalid UTF-8 bytes and lone surrogate escapes have become U+FFFD.
    String(Vec<u8>),
    /// An array.
    Array(Vec<Json>),
    /// An object, one entry per distinct (unescaped) key. When a key repeats, the value of its last
    /// occurrence wins (as with Go maps) and the entry sits at the key's first position. Callers should treat
    /// the order as unspecified.
    Object(Vec<(Vec<u8>, Json)>),
}

/// Go's `utf8.DecodeRune`: the decoded char and its width, or `None` with width 1 for invalid input
/// (width 0 for empty input).
pub(crate) fn decode_rune(s: &[u8]) -> (Option<char>, usize) {
    let Some(&b0) = s.first() else {
        return (None, 0);
    };
    let width = match b0 {
        0x00..=0x7f => return (Some(char::from(b0)), 1),
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    match s.get(..width).map(std::str::from_utf8) {
        Some(Ok(text)) => match text.chars().next() {
            Some(c) => (Some(c), width),
            None => (None, 1),
        },
        _ => (None, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rune_matches_go() {
        assert_eq!(decode_rune(b""), (None, 0));
        assert_eq!(decode_rune(b"a"), (Some('a'), 1));
        assert_eq!(decode_rune("é".as_bytes()), (Some('é'), 2));
        assert_eq!(decode_rune(b"\xe2\x82"), (None, 1));
        assert_eq!(decode_rune(b"\xed\xa0\x80"), (None, 1));
        assert_eq!(decode_rune(b"\xc0\x80"), (None, 1));
        assert_eq!(decode_rune(b"\xf4\x90\x80\x80"), (None, 1));
        assert_eq!(decode_rune("\u{10ffff}".as_bytes()), (Some('\u{10ffff}'), 4));
    }
}
