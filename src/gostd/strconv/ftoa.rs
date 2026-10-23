//! Float formatting (`strconv.FormatFloat`).
//!
//! Follows the behaviour of Go's standard library strconv package (The Go Authors, BSD-style licence).
//! Go formats most values with Ryū; this port always takes Go's exact multiprecision path
//! (`bigFtoa`), which Go's own tests check to produce identical output.

use super::decimal::Decimal;
use super::itoa::format_bits;

/// Layout of an IEEE 754 binary format.
pub(super) struct FloatInfo {
    pub mantbits: u32,
    pub expbits: u32,
    pub bias: i32,
}

pub(super) const FLOAT32_INFO: FloatInfo = FloatInfo { mantbits: 23, expbits: 8, bias: -127 };
pub(super) const FLOAT64_INFO: FloatInfo = FloatInfo { mantbits: 52, expbits: 11, bias: -1023 };

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";
const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Converts the floating-point number `f` to a string, like Go's `strconv.FormatFloat`.
///
/// `fmt` is one of `b'b'`, `b'e'`, `b'E'`, `b'f'`, `b'g'`, `b'G'`, `b'x'`, `b'X'`; any other
/// byte yields `%` followed by that byte, as in Go. `prec` is the number of digits (for `e`, `f`)
/// or significant digits (for `g`); -1 (any negative value) selects the shortest representation
/// that round-trips. `bit_size` 32 rounds `f` to `f32` first; any other value is treated as 64
/// (Go panics on sizes other than 32 and 64). The output is always ASCII.
pub fn format_float(f: f64, fmt: u8, prec: i32, bit_size: u32) -> String {
    let mut dst = Vec::with_capacity(24);
    generic_ftoa(&mut dst, f, fmt, i64::from(prec), bit_size);
    dst.into_iter().map(char::from).collect()
}

fn generic_ftoa(dst: &mut Vec<u8>, val: f64, fmt: u8, prec: i64, bit_size: u32) {
    let (bits, flt) = if bit_size == 32 {
        (u64::from((val as f32).to_bits()), &FLOAT32_INFO)
    } else {
        (val.to_bits(), &FLOAT64_INFO)
    };
    let neg = bits >> (flt.expbits + flt.mantbits) != 0;
    let mut exp = ((bits >> flt.mantbits) & ((1 << flt.expbits) - 1)) as i32;
    let mut mant = bits & ((1u64 << flt.mantbits) - 1);

    if exp == (1 << flt.expbits) - 1 {
        let s: &[u8] = if mant != 0 {
            b"NaN"
        } else if neg {
            b"-Inf"
        } else {
            b"+Inf"
        };
        dst.extend_from_slice(s);
        return;
    } else if exp == 0 {
        // Denormal.
        exp += 1;
    } else {
        // Add implicit top bit.
        mant |= 1u64 << flt.mantbits;
    }
    exp += flt.bias;

    match fmt {
        b'b' => fmt_b(dst, neg, mant, exp, flt),
        b'x' | b'X' => fmt_x(dst, prec, fmt, neg, mant, exp, flt),
        _ => big_ftoa(dst, prec, fmt, neg, mant, exp, flt),
    }
}

fn big_ftoa(dst: &mut Vec<u8>, mut prec: i64, fmt: u8, neg: bool, mant: u64, exp: i32, flt: &FloatInfo) {
    let mut d = Decimal::new();
    d.assign(mant);
    d.shift(exp - flt.mantbits as i32);
    let shortest = prec < 0;
    if shortest {
        round_shortest(&mut d, mant, exp, flt);
        let (nd, dp) = (d.nd as i64, i64::from(d.dp));
        match fmt {
            b'e' | b'E' => prec = (nd - 1).max(0),
            b'f' => prec = (nd - dp).max(0),
            b'g' | b'G' => prec = nd,
            _ => {}
        }
    } else {
        match fmt {
            b'e' | b'E' => d.round(prec.saturating_add(1)),
            b'f' => d.round(i64::from(d.dp).saturating_add(prec)),
            b'g' | b'G' => {
                if prec == 0 {
                    prec = 1;
                }
                d.round(prec);
            }
            _ => {}
        }
    }
    let digs = Digits { d: &d.d[..d.nd], dp: i64::from(d.dp) };
    format_digits(dst, shortest, neg, &digs, prec, fmt);
}

/// A decimal digit string `0.d * 10^dp`.
struct Digits<'a> {
    d: &'a [u8],
    dp: i64,
}

impl Digits<'_> {
    fn nd(&self) -> i64 {
        self.d.len() as i64
    }
}

fn format_digits(dst: &mut Vec<u8>, shortest: bool, neg: bool, digs: &Digits, prec: i64, fmt: u8) {
    match fmt {
        b'e' | b'E' => fmt_e(dst, neg, digs, prec, fmt),
        b'f' => fmt_f(dst, neg, digs, prec),
        b'g' | b'G' => {
            let mut eprec = prec;
            if eprec > digs.nd() && digs.nd() >= digs.dp {
                eprec = digs.nd();
            }
            // %e is used if the exponent from the conversion is less than -4 or greater than
            // or equal to the precision; shortest output uses precision 6 for this decision.
            if shortest {
                eprec = 6;
            }
            let exp = digs.dp - 1;
            if exp < -4 || exp >= eprec {
                let prec = prec.min(digs.nd());
                fmt_e(dst, neg, digs, prec - 1, fmt + b'e' - b'g');
                return;
            }
            let prec = if prec > digs.dp { digs.nd() } else { prec };
            fmt_f(dst, neg, digs, (prec - digs.dp).max(0));
        }
        _ => {
            dst.push(b'%');
            dst.push(fmt);
        }
    }
}

/// Rounds `d` (the exact value of `mant * 2^(exp - mantbits)`) to the shortest decimal that
/// still uniquely identifies the float.
fn round_shortest(d: &mut Decimal, mant: u64, exp: i32, flt: &FloatInfo) {
    if mant == 0 {
        d.nd = 0;
        return;
    }
    // If the exact value has fewer digits than any shorter output could, it is already shortest.
    let minexp = flt.bias + 1;
    if exp > minexp && 332 * (d.dp - d.nd as i32) >= 100 * (exp - flt.mantbits as i32) {
        return;
    }

    // Halfway point to the next higher float.
    let mut upper = Decimal::new();
    upper.assign(mant * 2 + 1);
    upper.shift(exp - flt.mantbits as i32 - 1);

    // Halfway point to the next lower float; the spacing below is halved at a power of two.
    let (mantlo, explo) =
        if mant > 1 << flt.mantbits || exp == minexp { (mant - 1, exp) } else { (mant * 2 - 1, exp - 1) };
    let mut lower = Decimal::new();
    lower.assign(mantlo * 2 + 1);
    lower.shift(explo - flt.mantbits as i32 - 1);

    // The upper and lower bounds are possible outputs only if the mantissa is even, so that
    // round-to-even on input picks this float.
    let inclusive = mant.is_multiple_of(2);

    let mut upperdelta = 0u8;
    let mut ui: i64 = 0;
    loop {
        let mi = ui - i64::from(upper.dp) + i64::from(d.dp);
        if mi >= d.nd as i64 {
            break;
        }
        let li = ui - i64::from(upper.dp) + i64::from(lower.dp);
        let l = if li >= 0 && li < lower.nd as i64 { lower.d[li as usize] } else { b'0' };
        let m = if mi >= 0 { d.d[mi as usize] } else { b'0' };
        let u = if ui < upper.nd as i64 { upper.d[ui as usize] } else { b'0' };

        let okdown = l != m || inclusive && li + 1 == lower.nd as i64;

        if upperdelta == 0 && m + 1 < u {
            upperdelta = 2;
        } else if upperdelta == 0 && m != u {
            upperdelta = 1;
        } else if upperdelta == 1 && (m != b'9' || u != b'0') {
            upperdelta = 2;
        }
        let okup = upperdelta > 0 && (inclusive || upperdelta > 1 || ui + 1 < upper.nd as i64);

        if okdown && okup {
            d.round(mi + 1);
            return;
        } else if okdown {
            d.round_down(mi + 1);
            return;
        } else if okup {
            d.round_up(mi + 1);
            return;
        }
        ui += 1;
    }
}

fn fmt_e(dst: &mut Vec<u8>, neg: bool, d: &Digits, prec: i64, fmt: u8) {
    if neg {
        dst.push(b'-');
    }
    dst.push(d.d.first().copied().unwrap_or(b'0'));
    if prec > 0 {
        dst.push(b'.');
        let m = d.nd().min(prec + 1);
        let mut i = 1;
        if i < m {
            dst.extend_from_slice(&d.d[1..m as usize]);
            i = m;
        }
        while i <= prec {
            dst.push(b'0');
            i += 1;
        }
    }
    dst.push(fmt);
    // Zero has exponent 0.
    let mut exp = if d.nd() == 0 { 0 } else { d.dp - 1 };
    if exp < 0 {
        dst.push(b'-');
        exp = -exp;
    } else {
        dst.push(b'+');
    }
    if exp < 10 {
        dst.extend_from_slice(&[b'0', exp as u8 + b'0']);
    } else if exp < 100 {
        dst.extend_from_slice(&[(exp / 10) as u8 + b'0', (exp % 10) as u8 + b'0']);
    } else {
        dst.extend_from_slice(&[(exp / 100) as u8 + b'0', (exp / 10 % 10) as u8 + b'0', (exp % 10) as u8 + b'0']);
    }
}

fn fmt_f(dst: &mut Vec<u8>, neg: bool, d: &Digits, prec: i64) {
    if neg {
        dst.push(b'-');
    }
    if d.dp > 0 {
        let m = d.nd().min(d.dp);
        dst.extend_from_slice(&d.d[..m as usize]);
        dst.resize(dst.len() + (d.dp - m) as usize, b'0');
    } else {
        dst.push(b'0');
    }
    if prec > 0 {
        dst.push(b'.');
        for i in 0..prec {
            let j = d.dp + i;
            let ch = if 0 <= j && j < d.nd() { d.d[j as usize] } else { b'0' };
            dst.push(ch);
        }
    }
}

/// `%b`: `-ddddp±ddd`, a decimal mantissa and binary exponent.
fn fmt_b(dst: &mut Vec<u8>, neg: bool, mant: u64, exp: i32, flt: &FloatInfo) {
    if neg {
        dst.push(b'-');
    }
    format_bits(dst, mant, 10, false);
    dst.push(b'p');
    let exp = exp - flt.mantbits as i32;
    if exp >= 0 {
        dst.push(b'+');
    }
    format_bits(dst, u64::from(exp.unsigned_abs()), 10, exp < 0);
}

/// `%x`: `-0x1.yyyyyp±ddd`, a hexadecimal mantissa and binary exponent.
fn fmt_x(dst: &mut Vec<u8>, prec: i64, fmt: u8, neg: bool, mut mant: u64, mut exp: i32, flt: &FloatInfo) {
    if mant == 0 {
        exp = 0;
    }
    // Shift digits so leading 1 (if any) is at bit 1<<60.
    mant <<= 60 - flt.mantbits;
    while mant != 0 && mant & (1 << 60) == 0 {
        mant <<= 1;
        exp -= 1;
    }
    // Round if requested.
    if (0..15).contains(&prec) {
        let shift = (prec * 4) as u32;
        let extra = (mant << shift) & ((1 << 60) - 1);
        mant >>= 60 - shift;
        if extra | (mant & 1) > 1 << 59 {
            mant += 1;
        }
        mant <<= 60 - shift;
        if mant & (1 << 61) != 0 {
            // Wrapped around.
            mant >>= 1;
            exp += 1;
        }
    }
    let hex = if fmt == b'X' { UPPER_HEX } else { LOWER_HEX };
    if neg {
        dst.push(b'-');
    }
    dst.extend_from_slice(&[b'0', fmt, b'0' + ((mant >> 60) & 1) as u8]);
    mant <<= 4; // remove leading 0 or 1
    if prec < 0 && mant != 0 {
        dst.push(b'.');
        while mant != 0 {
            dst.push(hex[((mant >> 60) & 15) as usize]);
            mant <<= 4;
        }
    } else if prec > 0 {
        dst.push(b'.');
        for _ in 0..prec {
            dst.push(hex[((mant >> 60) & 15) as usize]);
            mant <<= 4;
        }
    }
    dst.push(if fmt == b'X' { b'P' } else { b'p' });
    if exp < 0 {
        dst.push(b'-');
        exp = -exp;
    } else {
        dst.push(b'+');
    }
    let exp = exp as u32;
    if exp < 100 {
        dst.extend_from_slice(&[(exp / 10) as u8 + b'0', (exp % 10) as u8 + b'0']);
    } else if exp < 1000 {
        dst.extend_from_slice(&[(exp / 100) as u8 + b'0', (exp / 10 % 10) as u8 + b'0', (exp % 10) as u8 + b'0']);
    } else {
        dst.extend_from_slice(&[
            (exp / 1000) as u8 + b'0',
            (exp / 100 % 10) as u8 + b'0',
            (exp / 10 % 10) as u8 + b'0',
            (exp % 10) as u8 + b'0',
        ]);
    }
}
