//! Integer parsing (`strconv.ParseInt`, `strconv.ParseUint`, `strconv.Atoi`) and `NumError`.
//!
//! Follows the behaviour of Go's standard library strconv package (The Go Authors, BSD-style licence).

use std::fmt;

/// The reason a conversion failed (Go's `NumError.Err`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumErrorKind {
    /// `strconv.ErrSyntax`: "invalid syntax".
    Syntax,
    /// `strconv.ErrRange`: "value out of range".
    Range,
    /// "invalid base N".
    InvalidBase(u32),
    /// "invalid bit size N".
    InvalidBitSize(u32),
}

/// A failed conversion, like Go's `*strconv.NumError`.
///
/// `Display` gives Go's `Error()` text, e.g.
/// `strconv.ParseInt: parsing "x": invalid syntax`, with the input quoted by `strconv.Quote`.
#[derive(Debug, Clone, PartialEq)]
pub struct NumError {
    /// The failing function: "ParseInt", "ParseUint", "Atoi" or "ParseFloat".
    pub func: &'static str,
    /// The input.
    pub num: Vec<u8>,
    /// The reason.
    pub kind: NumErrorKind,
    /// For `ParseFloat` range errors, the value Go returns alongside the error (±Inf);
    /// 0 for every other error. For integer range errors Go returns the clamped value
    /// (the maximum or minimum for the bit size), which callers can derive from the sign.
    pub value_on_range: f64,
}

impl NumError {
    pub(super) fn new(func: &'static str, num: &[u8], kind: NumErrorKind) -> NumError {
        NumError { func, num: num.to_vec(), kind, value_on_range: 0.0 }
    }

    /// Go's `ErrSyntax` for `num`.
    pub(super) fn syntax(func: &'static str, num: &[u8]) -> NumError {
        NumError::new(func, num, NumErrorKind::Syntax)
    }

    /// Go's `ErrRange` for `num`.
    pub(super) fn range(func: &'static str, num: &[u8]) -> NumError {
        NumError::new(func, num, NumErrorKind::Range)
    }
}

impl fmt::Display for NumErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumErrorKind::Syntax => f.write_str("invalid syntax"),
            NumErrorKind::Range => f.write_str("value out of range"),
            NumErrorKind::InvalidBase(b) => write!(f, "invalid base {b}"),
            NumErrorKind::InvalidBitSize(b) => write!(f, "invalid bit size {b}"),
        }
    }
}

impl fmt::Display for NumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "strconv.{}: parsing {}: {}", self.func, super::quote(&self.num), self.kind)
    }
}

impl std::error::Error for NumError {}

fn lower(c: u8) -> u8 {
    c | (b'x' - b'X')
}

/// Like `parse_int` but for unsigned numbers; a sign prefix is not permitted.
///
/// Base 0 infers the base from the prefix (`0b`, `0o`, `0x`, or a bare `0` for octal) and then
/// permits underscores between digits. Bit size 0 means 64. On a range error Go returns the
/// maximum value of the bit size along with the error.
pub fn parse_uint(s: &[u8], base: u32, bit_size: u32) -> Result<u64, NumError> {
    const FN: &str = "ParseUint";
    if s.is_empty() {
        return Err(NumError::syntax(FN, s));
    }
    let base0 = base == 0;
    let s0 = s;
    let mut s = s;
    let mut base = base;
    if base == 0 {
        base = 10;
        if s[0] == b'0' {
            if s.len() >= 3 && lower(s[1]) == b'b' {
                base = 2;
                s = &s[2..];
            } else if s.len() >= 3 && lower(s[1]) == b'o' {
                base = 8;
                s = &s[2..];
            } else if s.len() >= 3 && lower(s[1]) == b'x' {
                base = 16;
                s = &s[2..];
            } else {
                base = 8;
                s = &s[1..];
            }
        }
    } else if !(2..=36).contains(&base) {
        return Err(NumError::new(FN, s0, NumErrorKind::InvalidBase(base)));
    }
    let bit_size = match bit_size {
        0 => 64,
        1..=64 => bit_size,
        _ => return Err(NumError::new(FN, s0, NumErrorKind::InvalidBitSize(bit_size))),
    };

    let base64 = u64::from(base);
    let cutoff = u64::MAX / base64 + 1;
    let max_val = if bit_size == 64 { u64::MAX } else { (1u64 << bit_size) - 1 };
    let mut underscores = false;
    let mut n: u64 = 0;
    for &c in s {
        let d = match c {
            b'_' if base0 => {
                underscores = true;
                continue;
            }
            b'0'..=b'9' => c - b'0',
            _ if lower(c).is_ascii_lowercase() => lower(c) - b'a' + 10,
            _ => return Err(NumError::syntax(FN, s0)),
        };
        if u32::from(d) >= base {
            return Err(NumError::syntax(FN, s0));
        }
        if n >= cutoff {
            return Err(NumError::range(FN, s0));
        }
        n *= base64;
        let n1 = n.wrapping_add(u64::from(d));
        if n1 < n || n1 > max_val {
            return Err(NumError::range(FN, s0));
        }
        n = n1;
    }
    if underscores && !underscore_ok(s0) {
        return Err(NumError::syntax(FN, s0));
    }
    Ok(n)
}

/// Interprets `s` in the given base (0, or 2 to 36) and bit size (0 means 64, else 1 to 64),
/// like Go's `strconv.ParseInt`. An optional leading sign is accepted. On a range error Go
/// returns the maximum or minimum value of the bit size along with the error.
pub fn parse_int(s: &[u8], base: u32, bit_size: u32) -> Result<i64, NumError> {
    const FN: &str = "ParseInt";
    if s.is_empty() {
        return Err(NumError::syntax(FN, s));
    }
    let s0 = s;
    let (neg, rest) = match s[0] {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    let bit_size_or_64 = if bit_size == 0 { 64 } else { bit_size };
    let un = match parse_uint(rest, base, bit_size) {
        Ok(v) => v,
        // ParseUint returns the bit size's maximum on overflow; Go carries on with it, so
        // "-2" with bit size 1 parses as -1 without an error.
        Err(e) if e.kind == NumErrorKind::Range => u64::MAX >> (64 - bit_size_or_64),
        Err(e) => return Err(NumError::new(FN, s0, e.kind)),
    };
    let bit_size = bit_size_or_64;
    let cutoff = 1u64 << (bit_size - 1);
    if !neg && un >= cutoff {
        return Err(NumError::range(FN, s0));
    }
    if neg && un > cutoff {
        return Err(NumError::range(FN, s0));
    }
    // The magnitude fits: up to 2^63 for negative numbers.
    let n = un as i64;
    Ok(if neg { n.wrapping_neg() } else { n })
}

/// Equivalent to `parse_int(s, 10, 0)` but reports errors with the function name "Atoi"
/// (Go's `strconv.Atoi` on a 64-bit platform, including its fast path for short inputs).
pub fn atoi(s: &[u8]) -> Result<i64, NumError> {
    const FN: &str = "Atoi";
    if !s.is_empty() && s.len() < 19 {
        let s0 = s;
        let mut digits = s;
        if s[0] == b'-' || s[0] == b'+' {
            digits = &s[1..];
            if digits.is_empty() {
                return Err(NumError::syntax(FN, s0));
            }
        }
        let mut n: i64 = 0;
        for &ch in digits {
            let d = ch.wrapping_sub(b'0');
            if d > 9 {
                return Err(NumError::syntax(FN, s0));
            }
            n = n * 10 + i64::from(d);
        }
        if s0[0] == b'-' {
            n = -n;
        }
        return Ok(n);
    }
    parse_int(s, 10, 0).map_err(|mut e| {
        e.func = FN;
        e
    })
}

/// Reports whether the underscores in `s` are allowed: each must separate digits (a base
/// prefix counts as a digit).
pub(super) fn underscore_ok(s: &[u8]) -> bool {
    // saw tracks the last character class: '^' start, '0' digit, '_' underscore, '!' other.
    let mut saw = b'^';
    let mut i = 0;
    let mut s = s;
    if !s.is_empty() && (s[0] == b'-' || s[0] == b'+') {
        s = &s[1..];
    }
    let mut hex = false;
    if s.len() >= 2 && s[0] == b'0' && matches!(lower(s[1]), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = lower(s[1]) == b'x';
    }
    while i < s.len() {
        let c = s[i];
        i += 1;
        if c.is_ascii_digit() || hex && (b'a'..=b'f').contains(&lower(c)) {
            saw = b'0';
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
    }
    saw != b'_'
}
