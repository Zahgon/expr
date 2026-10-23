//! Encoding of strings and floats as `json.Marshal` does.
//!
//! Follows the behaviour of Go's standard library encoding/json package (The Go Authors, BSD-style licence).

use super::decode_rune;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Go's `appendString`: `s` as a quoted JSON string.
///
/// `"` and `\` are backslash-escaped, `\b \f \n \r \t` use their short escapes, other bytes below 0x20 become
/// `\u00XX`, `<`, `>` and `&` become `<`, `>`, `&` when `escape_html` is set (as with
/// `json.Marshal`; an Encoder with `SetEscapeHTML(false)` passes `false`), U+2028 and U+2029 are always
/// escaped, and each invalid UTF-8 byte becomes `�`.
pub fn encode_string(s: &[u8], escape_html: bool) -> Vec<u8> {
    let mut dst = Vec::with_capacity(s.len() + 2);
    dst.push(b'"');
    let mut start = 0;
    let mut i = 0;
    while let Some(&b) = s.get(i) {
        if b < 0x80 {
            if is_safe(b, escape_html) {
                i += 1;
                continue;
            }
            dst.extend_from_slice(&s[start..i]);
            match b {
                b'\\' | b'"' => dst.extend_from_slice(&[b'\\', b]),
                0x08 => dst.extend_from_slice(b"\\b"),
                0x0c => dst.extend_from_slice(b"\\f"),
                b'\n' => dst.extend_from_slice(b"\\n"),
                b'\r' => dst.extend_from_slice(b"\\r"),
                b'\t' => dst.extend_from_slice(b"\\t"),
                _ => dst.extend_from_slice(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(b >> 4)],
                    HEX[usize::from(b & 0xf)],
                ]),
            }
            i += 1;
            start = i;
            continue;
        }
        let (c, size) = decode_rune(&s[i..]);
        match c {
            None => {
                dst.extend_from_slice(&s[start..i]);
                dst.extend_from_slice(b"\\ufffd");
                i += 1;
                start = i;
            }
            Some(c @ ('\u{2028}' | '\u{2029}')) => {
                dst.extend_from_slice(&s[start..i]);
                dst.extend_from_slice(b"\\u202");
                dst.push(HEX[(c as usize) & 0xf]);
                i += size;
                start = i;
            }
            Some(_) => i += size,
        }
    }
    dst.extend_from_slice(&s[start..]);
    dst.push(b'"');
    dst
}

/// Go's `safeSet` / `htmlSafeSet` for an ASCII byte.
fn is_safe(b: u8, escape_html: bool) -> bool {
    match b {
        b'"' | b'\\' => false,
        b'<' | b'>' | b'&' => !escape_html,
        _ => b >= 0x20,
    }
}

/// Go's `floatEncoder` for `bits` 64 (float64) or 32 (float32; `f` is rounded to float32 first, as
/// `strconv.FormatFloat` does). Any other `bits` value is treated as 64.
///
/// Uses the shortest representation that round-trips, in `%f` style unless the magnitude is below 1e-6 or
/// at least 1e21, where `%e` style is used with a negative two-digit exponent shortened (`1e-07` becomes
/// `1e-7`). NaN and infinities fail with Go's `UnsupportedValueError` text, for example
/// `json: unsupported value: NaN` or `json: unsupported value: -Inf`. The output is ASCII.
pub fn encode_float(f: f64, bits: u32) -> Result<Vec<u8>, String> {
    if f.is_nan() {
        return Err("json: unsupported value: NaN".to_string());
    }
    if f.is_infinite() {
        let sign = if f > 0.0 { "+" } else { "-" };
        return Err(format!("json: unsupported value: {sign}Inf"));
    }
    let abs = f.abs();
    let mut exp_format = false;
    if abs != 0.0 {
        exp_format = if bits == 32 {
            // Go compares in float32 to get the cutoffs right for float32 values.
            let a = abs as f32;
            !(1e-6..1e21).contains(&a)
        } else {
            !(1e-6..1e21).contains(&abs)
        };
    }
    let (neg, digits, exp) = if bits == 32 {
        let v = f as f32;
        if v.is_infinite() {
            // A float64 beyond the float32 range: FormatFloat prints the rounded value.
            return Ok(if v > 0.0 { b"+Inf".to_vec() } else { b"-Inf".to_vec() });
        }
        go_shortest(&format!("{v:e}"), |prec| format!("{v:.prec$e}"), |text| text.parse::<f32>().ok() == Some(v.abs()))
    } else {
        go_shortest(&format!("{f:e}"), |prec| format!("{f:.prec$e}"), |text| text.parse::<f64>().ok() == Some(f.abs()))
    };
    let mut out = Vec::new();
    if neg {
        out.push(b'-');
    }
    if exp_format {
        format_e(&mut out, &digits, exp);
    } else {
        format_f(&mut out, &digits, exp);
    }
    Ok(out)
}

/// The shortest digits Go's `strconv` picks, starting from Rust's shortest `{:e}` text `sci`.
///
/// Both pick the shortest digit string that round-trips and is closest to the exact value, but when the
/// exact value lies exactly halfway between two such strings Go rounds half to even while Rust rounds half
/// up. `exact(prec)` formats the value with `prec` digits after the point (correctly rounded), and
/// `round_trips` reports whether an unsigned decimal text parses back to the magnitude of the value.
fn go_shortest(sci: &str, exact: impl Fn(usize) -> String, round_trips: impl Fn(&str) -> bool) -> (bool, Vec<u8>, i32) {
    let (neg, digits, exp) = shortest(sci);
    let n = digits.len();
    // A tie needs an exact expansion of n + 1 significant digits ending in 5.
    let (_, longer, _) = shortest(&exact(n));
    if longer.last() != Some(&b'5') {
        return (neg, digits, exp);
    }
    // A float's exact decimal expansion has at most 767 significant digits.
    let (_, full, full_exp) = shortest(&exact(800));
    let significant = full.len() - full.iter().rev().take_while(|&&d| d == b'0').count();
    if significant != n + 1 {
        return (neg, digits, exp);
    }
    let floor = &full[..n];
    let mut candidate = floor.to_vec();
    if floor.last().is_some_and(|d| (d - b'0') % 2 == 1) {
        // Round up to the even neighbour; a carry may lengthen the digits.
        let mut i = candidate.len();
        let mut carry = true;
        while carry && i > 0 {
            i -= 1;
            if candidate[i] == b'9' {
                candidate[i] = b'0';
            } else {
                candidate[i] += 1;
                carry = false;
            }
        }
        if carry {
            return (neg, digits, exp);
        }
    }
    while candidate.len() > 1 && candidate.last() == Some(&b'0') {
        candidate.pop();
    }
    let last_exp = full_exp - i32::try_from(candidate.len()).unwrap_or(0) + 1;
    let text = format!("{}e{last_exp}", String::from_utf8_lossy(&candidate));
    if candidate.len() <= n && round_trips(&text) {
        return (neg, candidate, full_exp);
    }
    (neg, digits, exp)
}

/// Splits Rust's shortest `{:e}` output (`-1.25e-7`) into sign, digits (`125`) and the decimal exponent
/// of the first digit (`-7`).
fn shortest(text: &str) -> (bool, Vec<u8>, i32) {
    let (neg, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (mantissa, exp) = rest.split_once('e').unwrap_or((rest, "0"));
    let digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    (neg, digits, exp.parse().unwrap_or(0))
}

/// `%e` with shortest digits, as `strconv.FormatFloat(f, 'e', -1, bits)` followed by the exponent cleanup.
fn format_e(out: &mut Vec<u8>, digits: &[u8], exp: i32) {
    let (first, rest) = digits.split_first().unwrap_or((&b'0', &[]));
    out.push(*first);
    if !rest.is_empty() {
        out.push(b'.');
        out.extend_from_slice(rest);
    }
    out.push(b'e');
    if exp < 0 {
        // Go would print e-07; json strips the padding zero.
        out.push(b'-');
        out.extend_from_slice(exp.unsigned_abs().to_string().as_bytes());
    } else {
        out.push(b'+');
        out.extend_from_slice(format!("{exp:02}").as_bytes());
    }
}

/// `%f` with shortest digits, as `strconv.FormatFloat(f, 'f', -1, bits)`.
fn format_f(out: &mut Vec<u8>, digits: &[u8], exp: i32) {
    let point = exp.saturating_add(1);
    if point <= 0 {
        out.extend_from_slice(b"0.");
        out.extend(std::iter::repeat_n(b'0', point.unsigned_abs() as usize));
        out.extend_from_slice(digits);
        return;
    }
    let point = point.unsigned_abs() as usize;
    if digits.len() <= point {
        out.extend_from_slice(digits);
        out.extend(std::iter::repeat_n(b'0', point - digits.len()));
    } else {
        out.extend_from_slice(&digits[..point]);
        out.push(b'.');
        out.extend_from_slice(&digits[point..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(input: &[u8], html: bool) -> String {
        String::from_utf8(encode_string(input, html)).unwrap_or_default()
    }

    fn f64s(f: f64) -> String {
        String::from_utf8(encode_float(f, 64).unwrap_or_default()).unwrap_or_default()
    }

    fn f32s(f: f32) -> String {
        String::from_utf8(encode_float(f64::from(f), 32).unwrap_or_default()).unwrap_or_default()
    }

    #[test]
    fn strings() {
        assert_eq!(s(b"abc", true), "\"abc\"");
        assert_eq!(s(b"a\"b\\c", true), "\"a\\\"b\\\\c\"");
        assert_eq!(s(b"\n\r\t\x08\x0c\x01\x1f\x7f", true), "\"\\n\\r\\t\\b\\f\\u0001\\u001f\x7f\"");
        assert_eq!(s(b"<a&b>", true), "\"\\u003ca\\u0026b\\u003e\"");
        assert_eq!(s(b"<a&b>", false), "\"<a&b>\"");
        assert_eq!(s("\u{2028}x\u{2029}".as_bytes(), false), "\"\\u2028x\\u2029\"");
        assert_eq!(s(b"a\xffb\xe2\x82", true), "\"a\\ufffdb\\ufffd\\ufffd\"");
        assert_eq!(s("héllo😀".as_bytes(), true), "\"héllo😀\"");
        assert_eq!(s(b"", true), "\"\"");
    }

    #[test]
    fn floats64() {
        assert_eq!(f64s(0.0), "0");
        assert_eq!(f64s(-0.0), "-0");
        assert_eq!(f64s(1.0), "1");
        assert_eq!(f64s(0.1), "0.1");
        assert_eq!(f64s(123456789.0), "123456789");
        assert_eq!(f64s(1e20), "100000000000000000000");
        assert_eq!(f64s(1e21), "1e+21");
        assert_eq!(f64s(1.5e300), "1.5e+300");
        assert_eq!(f64s(0.000001), "0.000001");
        assert_eq!(f64s(1e-7), "1e-7");
        assert_eq!(f64s(-1.25e-10), "-1.25e-10");
        assert_eq!(f64s(5e-324), "5e-324");
        assert_eq!(f64s(f64::MAX), "1.7976931348623157e+308");
        assert_eq!(f64s(0.30000000000000004), "0.30000000000000004");
    }

    #[test]
    fn floats32() {
        assert_eq!(f32s(0.1), "0.1");
        assert_eq!(f32s(3.4028235e38), "3.4028235e+38");
        assert_eq!(f32s(1e-7), "1e-7");
        assert_eq!(f32s(16777216.0), "16777216");
        assert_eq!(f32s(1e21), "1e+21");
        assert_eq!(f32s(1e-45), "1e-45");
    }

    #[test]
    fn unsupported() {
        assert_eq!(encode_float(f64::NAN, 64), Err("json: unsupported value: NaN".to_string()));
        assert_eq!(encode_float(f64::INFINITY, 32), Err("json: unsupported value: +Inf".to_string()));
        assert_eq!(encode_float(f64::NEG_INFINITY, 64), Err("json: unsupported value: -Inf".to_string()));
    }

    #[test]
    fn float_cutoffs_and_ties() {
        assert_eq!(f64s(1e-6), "0.000001");
        assert_eq!(f64s(999999999999999900000.0), "999999999999999900000");
        assert_eq!(f32s(999999999999999900000.0), "1e+21");
        assert_eq!(f64s(2538483.25), "2538483.25");
        // Exactly halfway between 2538483.2 and 2538483.3: Go rounds half to even.
        assert_eq!(f32s(2_538_483.0 + 0.25), "2538483.2");
        assert_eq!(f64s(1e100), "1e+100");
        assert_eq!(f64s(-1234.5678e-20), "-1.2345678e-17");
        assert_eq!(f32s(-1234.5678e-20), "-1.2345678e-17");
    }
}
