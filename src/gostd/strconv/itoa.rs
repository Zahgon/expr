//! Integer formatting (`strconv.FormatInt`, `strconv.FormatUint`, `strconv.Itoa`).
//!
//! Follows the behaviour of Go's standard library strconv package (The Go Authors, BSD-style licence).

const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Returns the string representation of `i` in the given base (2 to 36, lower-case letters
/// for digits >= 10). Go panics for other bases; this returns an empty string instead.
pub fn format_int(i: i64, base: u32) -> String {
    if !(2..=36).contains(&base) {
        return String::new();
    }
    let mut dst = Vec::new();
    format_bits(&mut dst, i.unsigned_abs(), base, i < 0);
    dst.into_iter().map(char::from).collect()
}

/// Returns the string representation of `u` in the given base (2 to 36, lower-case letters
/// for digits >= 10). Go panics for other bases; this returns an empty string instead.
pub fn format_uint(u: u64, base: u32) -> String {
    if !(2..=36).contains(&base) {
        return String::new();
    }
    let mut dst = Vec::new();
    format_bits(&mut dst, u, base, false);
    dst.into_iter().map(char::from).collect()
}

/// Equivalent to `format_int(i, 10)`.
pub fn itoa(i: i64) -> String {
    format_int(i, 10)
}

/// Appends the digits of `u` in `base` (2..=36), preceded by '-' if `neg`.
pub(super) fn format_bits(dst: &mut Vec<u8>, mut u: u64, base: u32, neg: bool) {
    let base = u64::from(base);
    let mut buf = [0u8; 65];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = DIGITS[(u % base) as usize];
        u /= base;
        if u == 0 {
            break;
        }
    }
    if neg {
        dst.push(b'-');
    }
    dst.extend_from_slice(&buf[i..]);
}
