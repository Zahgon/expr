//! UTF-8 decoding and encoding of byte strings that may hold invalid UTF-8.
//!
//! Follows the behaviour of Go's standard library unicode/utf8 package (The Go Authors, BSD-style licence).

/// The "error" rune, U+FFFD, returned for invalid input.
pub const RUNE_ERROR: u32 = 0xFFFD;
/// Runes below this value are represented as themselves in a single byte.
pub const RUNE_SELF: u32 = 0x80;
/// Maximum valid Unicode code point.
pub const MAX_RUNE: u32 = 0x10FFFF;
/// Maximum number of bytes of a UTF-8 encoded rune.
pub const UTF_MAX: usize = 4;

const SURROGATE_MIN: u32 = 0xD800;
const SURROGATE_MAX: u32 = 0xDFFF;

/// Returns the encoded length and the accepted range of the second byte for a leading byte,
/// or `None` when the byte cannot start a multi-byte sequence.
fn lead_info(b: u8) -> Option<(usize, u8, u8)> {
    match b {
        0xC2..=0xDF => Some((2, 0x80, 0xBF)),
        0xE0 => Some((3, 0xA0, 0xBF)),
        0xE1..=0xEC | 0xEE..=0xEF => Some((3, 0x80, 0xBF)),
        0xED => Some((3, 0x80, 0x9F)),
        0xF0 => Some((4, 0x90, 0xBF)),
        0xF1..=0xF3 => Some((4, 0x80, 0xBF)),
        0xF4 => Some((4, 0x80, 0x8F)),
        _ => None,
    }
}

fn is_cont(b: u8) -> bool {
    (0x80..=0xBF).contains(&b)
}

/// Unpacks the first UTF-8 encoding in `p` and returns the rune and its width in bytes.
/// Returns `(RUNE_ERROR, 0)` for empty input and `(RUNE_ERROR, 1)` for an invalid encoding.
pub fn decode_rune(p: &[u8]) -> (u32, usize) {
    let Some(&b0) = p.first() else {
        return (RUNE_ERROR, 0);
    };
    if b0 < 0x80 {
        return (u32::from(b0), 1);
    }
    let Some((size, lo, hi)) = lead_info(b0) else {
        return (RUNE_ERROR, 1);
    };
    if p.len() < size {
        return (RUNE_ERROR, 1);
    }
    let b1 = p[1];
    if b1 < lo || hi < b1 {
        return (RUNE_ERROR, 1);
    }
    let c1 = u32::from(b1 & 0x3F);
    if size == 2 {
        return ((u32::from(b0 & 0x1F) << 6) | c1, 2);
    }
    let b2 = p[2];
    if !is_cont(b2) {
        return (RUNE_ERROR, 1);
    }
    let c2 = u32::from(b2 & 0x3F);
    if size == 3 {
        return ((u32::from(b0 & 0x0F) << 12) | (c1 << 6) | c2, 3);
    }
    let b3 = p[3];
    if !is_cont(b3) {
        return (RUNE_ERROR, 1);
    }
    let c3 = u32::from(b3 & 0x3F);
    ((u32::from(b0 & 0x07) << 18) | (c1 << 12) | (c2 << 6) | c3, 4)
}

/// Unpacks the last UTF-8 encoding in `p` and returns the rune and its width in bytes.
/// Returns `(RUNE_ERROR, 0)` for empty input and `(RUNE_ERROR, 1)` for an invalid encoding.
pub fn decode_last_rune(p: &[u8]) -> (u32, usize) {
    let end = p.len();
    if end == 0 {
        return (RUNE_ERROR, 0);
    }
    let last = p[end - 1];
    if u32::from(last) < RUNE_SELF {
        return (u32::from(last), 1);
    }
    // Back up to the start of the rune, scanning at most UTF_MAX bytes.
    let lim = end.saturating_sub(UTF_MAX);
    let mut start = end - 1;
    while start > lim {
        start -= 1;
        if !is_cont(p[start]) {
            break;
        }
    }
    let (r, size) = decode_rune(&p[start..end]);
    if start + size != end {
        return (RUNE_ERROR, 1);
    }
    (r, size)
}

/// Returns the number of runes in `p`; invalid and short encodings count as one rune per byte.
pub fn rune_count(p: &[u8]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < p.len() {
        if p[i] < 0x80 {
            i += 1;
        } else {
            i += decode_rune(&p[i..]).1;
        }
        n += 1;
    }
    n
}

/// Returns the number of bytes needed to encode the rune, or -1 if it is not a valid
/// value to encode in UTF-8 (a surrogate or above [`MAX_RUNE`]).
pub fn rune_len(r: u32) -> i32 {
    match r {
        0..=0x7F => 1,
        0x80..=0x7FF => 2,
        SURROGATE_MIN..=SURROGATE_MAX => -1,
        0x800..=0xFFFF => 3,
        0x10000..=MAX_RUNE => 4,
        _ => -1,
    }
}

/// Appends the UTF-8 encoding of the rune to `out` and returns the number of bytes written.
/// Invalid runes (surrogates, values above [`MAX_RUNE`]) are written as U+FFFD.
pub fn encode_rune(r: u32, out: &mut Vec<u8>) -> usize {
    let r = if valid_rune(r) { r } else { RUNE_ERROR };
    match r {
        0..=0x7F => {
            out.push(r as u8);
            1
        }
        0x80..=0x7FF => {
            out.extend_from_slice(&[0xC0 | (r >> 6) as u8, 0x80 | (r & 0x3F) as u8]);
            2
        }
        0x800..=0xFFFF => {
            out.extend_from_slice(&[0xE0 | (r >> 12) as u8, 0x80 | ((r >> 6) & 0x3F) as u8, 0x80 | (r & 0x3F) as u8]);
            3
        }
        _ => {
            out.extend_from_slice(&[
                0xF0 | (r >> 18) as u8,
                0x80 | ((r >> 12) & 0x3F) as u8,
                0x80 | ((r >> 6) & 0x3F) as u8,
                0x80 | (r & 0x3F) as u8,
            ]);
            4
        }
    }
}

/// Reports whether `p` consists entirely of valid UTF-8 encoded runes.
pub fn valid(p: &[u8]) -> bool {
    let mut i = 0;
    while i < p.len() {
        if p[i] < 0x80 {
            i += 1;
            continue;
        }
        let (r, size) = decode_rune(&p[i..]);
        if r == RUNE_ERROR && size == 1 {
            return false;
        }
        i += size;
    }
    true
}

/// Reports whether the rune can be legally encoded as UTF-8.
pub fn valid_rune(r: u32) -> bool {
    r < SURROGATE_MIN || (SURROGATE_MAX < r && r <= MAX_RUNE)
}

/// Reports whether the byte could be the first byte of an encoded, possibly invalid rune.
pub fn rune_start(b: u8) -> bool {
    !is_cont(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode() {
        assert_eq!(decode_rune(b""), (RUNE_ERROR, 0));
        assert_eq!(decode_rune(b"a"), (0x61, 1));
        assert_eq!(decode_rune("é".as_bytes()), (0xE9, 2));
        assert_eq!(decode_rune("€x".as_bytes()), (0x20AC, 3));
        assert_eq!(decode_rune("😀".as_bytes()), (0x1F600, 4));
        assert_eq!(decode_rune(b"\xef\xbf\xbd"), (0xFFFD, 3));
        assert_eq!(decode_rune(b"\xc0\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune(b"\xed\xa0\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune(b"\xf4\x90\x80\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune(b"\xe2\x82"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune(b"\x80"), (RUNE_ERROR, 1));
    }

    #[test]
    fn decode_last() {
        assert_eq!(decode_last_rune(b""), (RUNE_ERROR, 0));
        assert_eq!(decode_last_rune("a€".as_bytes()), (0x20AC, 3));
        assert_eq!(decode_last_rune(b"a\x82"), (RUNE_ERROR, 1));
        assert_eq!(decode_last_rune(b"\xe2\x82\xac\xac"), (RUNE_ERROR, 1));
        assert_eq!(decode_last_rune(b"\x80\x80\x80\x80\x80"), (RUNE_ERROR, 1));
    }

    #[test]
    fn counts() {
        assert_eq!(rune_count("héllo, 世界".as_bytes()), 9);
        assert_eq!(rune_count(b"\xe2\x82"), 2);
        assert_eq!(rune_len('a' as u32), 1);
        assert_eq!(rune_len(0xD800), -1);
        assert_eq!(rune_len(0x10FFFF), 4);
        assert_eq!(rune_len(0x110000), -1);
    }

    #[test]
    fn encode() {
        let mut v = Vec::new();
        assert_eq!(encode_rune(0x20AC, &mut v), 3);
        assert_eq!(encode_rune(0xD800, &mut v), 3);
        assert_eq!(encode_rune(0x110000, &mut v), 3);
        assert_eq!(v, b"\xe2\x82\xac\xef\xbf\xbd\xef\xbf\xbd");
    }

    #[test]
    fn validity() {
        assert!(valid("a€😀".as_bytes()));
        assert!(!valid(b"a\xff"));
        assert!(!valid(b"\xed\xbf\xbf"));
        assert!(valid_rune(0));
        assert!(!valid_rune(0xDFFF));
        assert!(valid_rune(0xE000));
        assert!(!valid_rune(0x110000));
    }
}
