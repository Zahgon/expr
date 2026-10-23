//! Float parsing (`strconv.ParseFloat`).
//!
//! Follows the behaviour of Go's standard library strconv package (The Go Authors, BSD-style licence).
//! Go tries exact float arithmetic, then the Eisel-Lemire algorithm, then multiprecision
//! decimal conversion; all three are correctly rounded, so this port keeps the first and the
//! last and gives the same results.

use super::atoi::{underscore_ok, NumError};
use super::decimal::Decimal;
use super::ftoa::{FloatInfo, FLOAT32_INFO, FLOAT64_INFO};

const FN_PARSE_FLOAT: &str = "ParseFloat";

fn lower(c: u8) -> u8 {
    c | (b'x' - b'X')
}

/// Length of the common prefix of `s` and the lower-case ASCII `prefix`, ignoring case in `s`.
fn common_prefix_len_ignore_case(s: &[u8], prefix: &[u8]) -> usize {
    s.iter().zip(prefix).take_while(|(&c, &p)| c.to_ascii_lowercase() == p).count()
}

/// Parses "inf", "infinity" (optionally signed) or "nan" at the start of `s`, in any case.
/// Returns the value and the number of bytes consumed.
fn special(s: &[u8]) -> Option<(f64, usize)> {
    let (&first, rest) = s.split_first()?;
    let (sign, nsign, body) = match first {
        b'+' => (1.0, 1, rest),
        b'-' => (-1.0, 1, rest),
        b'i' | b'I' => (1.0, 0, s),
        b'n' | b'N' => {
            return (common_prefix_len_ignore_case(s, b"nan") == 3).then_some((f64::NAN, 3));
        }
        _ => return None,
    };
    let mut n = common_prefix_len_ignore_case(body, b"infinity");
    // Anything longer than "inf" but shorter than "infinity" parses as "inf".
    if 3 < n && n < 8 {
        n = 3;
    }
    (n == 3 || n == 8).then_some((sign * f64::INFINITY, nsign + n))
}

/// The result of scanning a float literal (Go's `readFloat`).
struct Scanned {
    mantissa: u64,
    exp: i64,
    neg: bool,
    trunc: bool,
    hex: bool,
}

/// Reads a decimal or hexadecimal mantissa and exponent from the start of `s`.
/// Returns the scan (if it is a valid prefix) and the number of bytes consumed.
fn read_float(s: &[u8]) -> (Option<Scanned>, usize) {
    let mut underscores = false;
    let mut i = 0;
    let mut r = Scanned { mantissa: 0, exp: 0, neg: false, trunc: false, hex: false };
    if s.is_empty() {
        return (None, 0);
    }
    match s[0] {
        b'+' => i += 1,
        b'-' => {
            i += 1;
            r.neg = true;
        }
        _ => {}
    }

    let mut base: u64 = 10;
    let mut max_mant_digits = 19; // 10^19 fits in u64
    let mut exp_char = b'e';
    if i + 2 < s.len() && s[i] == b'0' && lower(s[i + 1]) == b'x' {
        base = 16;
        max_mant_digits = 16; // 16^16 fits in u64
        i += 2;
        exp_char = b'p';
        r.hex = true;
    }
    let mut sawdot = false;
    let mut sawdigits = false;
    let mut nd: i64 = 0;
    let mut nd_mant: i64 = 0;
    let mut dp: i64 = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'_' {
            underscores = true;
        } else if c == b'.' {
            if sawdot {
                break;
            }
            sawdot = true;
            dp = nd;
        } else if c.is_ascii_digit() {
            sawdigits = true;
            if c == b'0' && nd == 0 {
                // Ignore leading zeros.
                dp -= 1;
            } else {
                nd += 1;
                if nd_mant < max_mant_digits {
                    r.mantissa = r.mantissa * base + u64::from(c - b'0');
                    nd_mant += 1;
                } else if c != b'0' {
                    r.trunc = true;
                }
            }
        } else if base == 16 && (b'a'..=b'f').contains(&lower(c)) {
            sawdigits = true;
            nd += 1;
            if nd_mant < max_mant_digits {
                r.mantissa = r.mantissa * 16 + u64::from(lower(c) - b'a' + 10);
                nd_mant += 1;
            } else {
                r.trunc = true;
            }
        } else {
            break;
        }
        i += 1;
    }
    if !sawdigits {
        return (None, i);
    }
    if !sawdot {
        dp = nd;
    }
    if base == 16 {
        dp *= 4;
        nd_mant *= 4;
    }

    if i < s.len() && lower(s[i]) == exp_char {
        i += 1;
        if i >= s.len() {
            return (None, i);
        }
        let mut esign = 1;
        match s[i] {
            b'+' => i += 1,
            b'-' => {
                i += 1;
                esign = -1;
            }
            _ => {}
        }
        if i >= s.len() || !s[i].is_ascii_digit() {
            return (None, i);
        }
        let mut e: i64 = 0;
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
            if s[i] == b'_' {
                underscores = true;
            } else if e < 10000 {
                e = e * 10 + i64::from(s[i] - b'0');
            }
            i += 1;
        }
        dp += e * esign;
    } else if base == 16 {
        // Must have exponent.
        return (None, i);
    }

    if r.mantissa != 0 {
        r.exp = dp - nd_mant;
    }
    if underscores && !underscore_ok(&s[..i]) {
        return (None, i);
    }
    (Some(r), i)
}

impl Decimal {
    /// Parses a decimal literal (already validated by `read_float`); Go's `decimal.set`.
    fn set(&mut self, s: &[u8]) -> bool {
        let mut i = 0;
        self.neg = false;
        self.trunc = false;
        if s.is_empty() {
            return false;
        }
        match s[0] {
            b'+' => i += 1,
            b'-' => {
                i += 1;
                self.neg = true;
            }
            _ => {}
        }
        let mut sawdot = false;
        let mut sawdigits = false;
        while i < s.len() {
            let c = s[i];
            if c == b'_' {
                // Underscores were already validated by read_float.
            } else if c == b'.' {
                if sawdot {
                    return false;
                }
                sawdot = true;
                self.dp = self.nd as i32;
            } else if c.is_ascii_digit() {
                sawdigits = true;
                if c == b'0' && self.nd == 0 {
                    // Ignore leading zeros.
                    self.dp -= 1;
                } else if self.nd < self.d.len() {
                    self.d[self.nd] = c;
                    self.nd += 1;
                } else if c != b'0' {
                    self.trunc = true;
                }
            } else {
                break;
            }
            i += 1;
        }
        if !sawdigits {
            return false;
        }
        if !sawdot {
            self.dp = self.nd as i32;
        }
        if i < s.len() && lower(s[i]) == b'e' {
            i += 1;
            if i >= s.len() {
                return false;
            }
            let mut esign = 1;
            match s[i] {
                b'+' => i += 1,
                b'-' => {
                    i += 1;
                    esign = -1;
                }
                _ => {}
            }
            if i >= s.len() || !s[i].is_ascii_digit() {
                return false;
            }
            let mut e = 0;
            while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
                if s[i] != b'_' && e < 10000 {
                    e = e * 10 + i32::from(s[i] - b'0');
                }
                i += 1;
            }
            self.dp += e * esign;
        }
        i == s.len()
    }

    /// Converts to the bits of the nearest float; also reports overflow.
    fn float_bits(&mut self, flt: &FloatInfo) -> (u64, bool) {
        let (mant, exp, overflow) = self.float_parts(flt);
        let mut bits = mant & ((1u64 << flt.mantbits) - 1);
        bits |= (((exp - flt.bias) & ((1 << flt.expbits) - 1)) as u64) << flt.mantbits;
        if self.neg {
            bits |= 1u64 << flt.mantbits << flt.expbits;
        }
        (bits, overflow)
    }

    fn float_parts(&mut self, flt: &FloatInfo) -> (u64, i32, bool) {
        const POWTAB: [i32; 9] = [1, 3, 6, 9, 13, 16, 19, 23, 26];
        let overflow = (0, (1 << flt.expbits) - 1 + flt.bias, true);
        let zero = (0, flt.bias, false);
        if self.nd == 0 {
            return zero;
        }
        // Obvious overflow and underflow; these bounds are for f64 but work for f32 too.
        if self.dp > 310 {
            return overflow;
        }
        if self.dp < -330 {
            return zero;
        }
        // Scale by powers of two until in range [0.5, 1.0).
        let mut exp = 0;
        while self.dp > 0 {
            let n = POWTAB.get(self.dp as usize).copied().unwrap_or(27);
            self.shift(-n);
            exp += n;
        }
        while self.dp < 0 || self.dp == 0 && self.d[0] < b'5' {
            let n = POWTAB.get((-self.dp) as usize).copied().unwrap_or(27);
            self.shift(n);
            exp -= n;
        }
        // Our range is [0.5,1) but floating point range is [1,2).
        exp -= 1;
        // Minimum representable exponent is flt.bias+1; denormalize if lower.
        if exp < flt.bias + 1 {
            let n = flt.bias + 1 - exp;
            self.shift(-n);
            exp += n;
        }
        if exp - flt.bias >= (1 << flt.expbits) - 1 {
            return overflow;
        }
        // Extract 1+flt.mantbits bits.
        self.shift(1 + flt.mantbits as i32);
        let mut mant = self.rounded_integer();
        // Rounding might have added a bit; shift down.
        if mant == 2 << flt.mantbits {
            mant >>= 1;
            exp += 1;
            if exp - flt.bias >= (1 << flt.expbits) - 1 {
                return overflow;
            }
        }
        // Denormalized?
        if mant & (1 << flt.mantbits) == 0 {
            exp = flt.bias;
        }
        (mant, exp, false)
    }
}

const FLOAT64_POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20,
    1e21, 1e22,
];
const FLOAT32_POW10: [f32; 11] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10];

/// Exact conversion when both the mantissa and the power of ten are exact floats.
fn atof64_exact(mantissa: u64, mut exp: i64, neg: bool) -> Option<f64> {
    if mantissa >> FLOAT64_INFO.mantbits != 0 {
        return None;
    }
    let mut f = mantissa as f64;
    if neg {
        f = -f;
    }
    if exp == 0 {
        return Some(f);
    }
    if exp > 0 && exp <= 15 + 22 {
        // int * 10^k: if exponent is big but number of digits is not, move zeros into the int.
        if exp > 22 {
            f *= FLOAT64_POW10[(exp - 22) as usize];
            exp = 22;
        }
        if !(-1e15..=1e15).contains(&f) {
            // The exponent was really too large.
            return None;
        }
        return Some(f * FLOAT64_POW10[exp as usize]);
    }
    if (-22..0).contains(&exp) {
        return Some(f / FLOAT64_POW10[(-exp) as usize]);
    }
    None
}

fn atof32_exact(mantissa: u64, mut exp: i64, neg: bool) -> Option<f32> {
    if mantissa >> FLOAT32_INFO.mantbits != 0 {
        return None;
    }
    let mut f = mantissa as f32;
    if neg {
        f = -f;
    }
    if exp == 0 {
        return Some(f);
    }
    if exp > 0 && exp <= 7 + 10 {
        if exp > 10 {
            f *= FLOAT32_POW10[(exp - 10) as usize];
            exp = 10;
        }
        if !(-1e7..=1e7).contains(&f) {
            return None;
        }
        return Some(f * FLOAT32_POW10[exp as usize]);
    }
    if (-10..0).contains(&exp) {
        return Some(f / FLOAT32_POW10[(-exp) as usize]);
    }
    None
}

/// Converts a hexadecimal mantissa and binary exponent, rounding to even.
/// Returns the value and whether it overflowed to infinity.
fn atof_hex(flt: &FloatInfo, mut mantissa: u64, exp: i64, neg: bool, trunc: bool) -> (f64, bool) {
    let max_exp = i64::from((1 << flt.expbits) + flt.bias - 2);
    let min_exp = i64::from(flt.bias + 1);
    // The mantissa is now implicitly divided by 2^mantbits.
    let mut exp = exp + i64::from(flt.mantbits);

    // Shift mantissa and exponent to bring representation into float range.
    // Eventually we want a mantissa with a leading 1-bit followed by mantbits other bits.
    // For rounding, we need two more, where the bottom bit represents
    // whether that bit or any later bit was non-zero.
    while mantissa != 0 && mantissa >> (flt.mantbits + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + flt.mantbits + 2) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // If exponent is too negative, denormalize in hopes of making it representable.
    while mantissa > 1 && exp < min_exp - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // Round using two bottom bits.
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1; // round to even (round up if mantissa is odd)
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + flt.mantbits) {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if mantissa >> flt.mantbits == 0 {
        // Denormal or zero.
        exp = i64::from(flt.bias);
    }
    let mut overflow = false;
    if exp > max_exp {
        // Infinity and range error.
        mantissa = 1 << flt.mantbits;
        exp = max_exp + 1;
        overflow = true;
    }
    let mut bits = mantissa & ((1u64 << flt.mantbits) - 1);
    bits |= (((exp - i64::from(flt.bias)) & ((1 << flt.expbits) - 1)) as u64) << flt.mantbits;
    if neg {
        bits |= 1u64 << flt.mantbits << flt.expbits;
    }
    if flt.mantbits == FLOAT32_INFO.mantbits {
        (f64::from(f32::from_bits(bits as u32)), overflow)
    } else {
        (f64::from_bits(bits), overflow)
    }
}

/// Parses the longest float prefix of `s`; returns the value, the bytes consumed and whether
/// the value is out of range, or `Err(consumed)` for a syntax error.
fn atof(s: &[u8], bit_size: u32) -> Result<(f64, usize, bool), usize> {
    let is32 = bit_size == 32;
    if let Some((val, n)) = special(s) {
        return Ok((val, n, false));
    }
    let (scan, n) = read_float(s);
    let Some(scan) = scan else {
        return Err(n);
    };
    let flt = if is32 { &FLOAT32_INFO } else { &FLOAT64_INFO };
    if scan.hex {
        let (f, ovf) = atof_hex(flt, scan.mantissa, scan.exp, scan.neg, scan.trunc);
        return Ok((f, n, ovf));
    }
    if !scan.trunc {
        let exact = if is32 {
            atof32_exact(scan.mantissa, scan.exp, scan.neg).map(f64::from)
        } else {
            atof64_exact(scan.mantissa, scan.exp, scan.neg)
        };
        if let Some(f) = exact {
            return Ok((f, n, false));
        }
    }
    let mut d = Decimal::new();
    if !d.set(&s[..n]) {
        return Err(n);
    }
    let (b, ovf) = d.float_bits(flt);
    let f = if is32 { f64::from(f32::from_bits(b as u32)) } else { f64::from_bits(b) };
    Ok((f, n, ovf))
}

/// Converts `s` to a float with the precision of `bit_size` (32, else 64), like Go's
/// `strconv.ParseFloat`.
///
/// Accepts decimal and hexadecimal (`0x1.8p3`) literals with an optional sign, underscores
/// only where Go allows them, and "inf", "infinity" (optionally signed) and "nan" in any case.
/// The result is the nearest float, rounded half to even. A syntax error carries the value 0.
/// When the value is too large Go returns ±Inf together with `ErrRange`; this returns
/// `Err` with `kind == NumErrorKind::Range` and `value_on_range` set to that infinity.
pub fn parse_float(s: &[u8], bit_size: u32) -> Result<f64, NumError> {
    match atof(s, bit_size) {
        Ok((_, n, _)) if n != s.len() => Err(NumError::syntax(FN_PARSE_FLOAT, s)),
        Ok((f, _, true)) => {
            let mut e = NumError::range(FN_PARSE_FLOAT, s);
            e.value_on_range = f;
            Err(e)
        }
        Ok((f, _, false)) => Ok(f),
        Err(_) => Err(NumError::syntax(FN_PARSE_FLOAT, s)),
    }
}
