//! String and rune quoting (`strconv.Quote`, `strconv.QuoteRune`, `strconv.QuoteToASCII`).
//!
//! Follows the behaviour of Go's standard library strconv package (The Go Authors, BSD-style licence).

use super::super::unicode;
use super::super::utf8;

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

/// Reports whether the rune is printable, like Go's `strconv.IsPrint` (identical to
/// `unicode.IsPrint`: letters, marks, numbers, punctuation, symbols and U+0020).
pub fn is_print(r: u32) -> bool {
    unicode::is_print(r)
}

fn quote_with(s: &[u8], quote: u8, ascii_only: bool) -> String {
    let mut buf = Vec::with_capacity(s.len() + 2);
    buf.push(quote);
    let mut i = 0;
    while i < s.len() {
        let (r, width) = if s[i] < 0x80 { (u32::from(s[i]), 1) } else { utf8::decode_rune(&s[i..]) };
        if width == 1 && r == utf8::RUNE_ERROR {
            buf.extend_from_slice(b"\\x");
            buf.push(LOWER_HEX[usize::from(s[i] >> 4)]);
            buf.push(LOWER_HEX[usize::from(s[i] & 0xF)]);
        } else {
            append_escaped_rune(&mut buf, r, quote, ascii_only);
        }
        i += width;
    }
    buf.push(quote);
    // Only valid UTF-8 is ever appended.
    String::from_utf8(buf).unwrap_or_default()
}

fn append_escaped_rune(buf: &mut Vec<u8>, r: u32, quote: u8, ascii_only: bool) {
    if r == u32::from(quote) || r == u32::from(b'\\') {
        // Always backslashed.
        buf.push(b'\\');
        buf.push(r as u8);
        return;
    }
    if ascii_only {
        if r < utf8::RUNE_SELF && is_print(r) {
            buf.push(r as u8);
            return;
        }
    } else if is_print(r) {
        utf8::encode_rune(r, buf);
        return;
    }
    match r {
        0x07 => buf.extend_from_slice(b"\\a"),
        0x08 => buf.extend_from_slice(b"\\b"),
        0x0C => buf.extend_from_slice(b"\\f"),
        0x0A => buf.extend_from_slice(b"\\n"),
        0x0D => buf.extend_from_slice(b"\\r"),
        0x09 => buf.extend_from_slice(b"\\t"),
        0x0B => buf.extend_from_slice(b"\\v"),
        _ if r < u32::from(b' ') || r == 0x7F => {
            buf.extend_from_slice(b"\\x");
            buf.push(LOWER_HEX[(r >> 4) as usize & 0xF]);
            buf.push(LOWER_HEX[r as usize & 0xF]);
        }
        _ => {
            let r = if utf8::valid_rune(r) { r } else { 0xFFFD };
            if r < 0x10000 {
                buf.extend_from_slice(b"\\u");
                for s in (0..=12).rev().step_by(4) {
                    buf.push(LOWER_HEX[(r >> s) as usize & 0xF]);
                }
            } else {
                buf.extend_from_slice(b"\\U");
                for s in (0..=28).rev().step_by(4) {
                    buf.push(LOWER_HEX[(r >> s) as usize & 0xF]);
                }
            }
        }
    }
}

/// Returns a double-quoted Go string literal representing `s`, like Go's `strconv.Quote`.
/// Control characters and non-printable runes are escaped (`\t`, `\x00`, `­`, ...) and
/// each byte of invalid UTF-8 becomes `\xNN`.
pub fn quote(s: &[u8]) -> String {
    quote_with(s, b'"', false)
}

/// Like [`quote`] but escapes every non-ASCII rune too (`é`, `\U0001f600`), like Go's
/// `strconv.QuoteToASCII`.
pub fn quote_to_ascii(s: &[u8]) -> String {
    quote_with(s, b'"', true)
}

/// Returns a single-quoted Go character literal representing the rune, like Go's
/// `strconv.QuoteRune`; invalid runes are quoted as U+FFFD.
pub fn quote_rune(r: u32) -> String {
    let r = if utf8::valid_rune(r) { r } else { utf8::RUNE_ERROR };
    let mut buf = vec![b'\''];
    append_escaped_rune(&mut buf, r, b'\'', false);
    buf.push(b'\'');
    String::from_utf8(buf).unwrap_or_default()
}
