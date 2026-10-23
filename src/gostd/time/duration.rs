//! Durations: formatting, parsing, rounding and unit conversions.
//!
//! Follows the behaviour of Go's standard library time package (The Go Authors, BSD-style licence).

use super::format::quote;

/// A Go `time.Duration`: an elapsed time in nanoseconds.
pub type Duration = i64;

/// One nanosecond.
pub const NANOSECOND: Duration = 1;
/// One microsecond.
pub const MICROSECOND: Duration = 1000 * NANOSECOND;
/// One millisecond.
pub const MILLISECOND: Duration = 1000 * MICROSECOND;
/// One second.
pub const SECOND: Duration = 1000 * MILLISECOND;
/// One minute.
pub const MINUTE: Duration = 60 * SECOND;
/// One hour.
pub const HOUR: Duration = 60 * MINUTE;

pub(crate) const MIN_DURATION: Duration = i64::MIN;
pub(crate) const MAX_DURATION: Duration = i64::MAX;

/// Go's `Duration.String`, e.g. `72h3m0.5s`, `1.5µs`, `0s`.
///
/// The result is always valid UTF-8 (`µ` is the only non-ASCII character).
pub fn duration_string(d: Duration) -> String {
    let mut buf = [0u8; 32];
    let w = format_duration(&mut buf, d);
    String::from_utf8_lossy(&buf[w..]).into_owned()
}

/// Writes `d` right-aligned into `buf` and returns the start index (Go's `Duration.format`).
fn format_duration(buf: &mut [u8; 32], d: Duration) -> usize {
    let mut w = buf.len();
    let mut u = d as u64;
    let neg = d < 0;
    if neg {
        u = u.wrapping_neg();
    }
    if u < SECOND as u64 {
        // Special case: if duration is smaller than a second, use smaller units, like 1.2ms.
        let prec;
        w -= 1;
        buf[w] = b's';
        w -= 1;
        if u == 0 {
            buf[w] = b'0';
            return w;
        } else if u < MICROSECOND as u64 {
            prec = 0;
            buf[w] = b'n';
        } else if u < MILLISECOND as u64 {
            prec = 3;
            // U+00B5 'µ' micro sign takes two bytes.
            w -= 1;
            buf[w] = 0xC2;
            buf[w + 1] = 0xB5;
        } else {
            prec = 6;
            buf[w] = b'm';
        }
        let (nw, nu) = fmt_frac(&mut buf[..w], u, prec);
        w = fmt_int(&mut buf[..nw], nu);
    } else {
        w -= 1;
        buf[w] = b's';
        let (nw, nu) = fmt_frac(&mut buf[..w], u, 9);
        u = nu;
        w = fmt_int(&mut buf[..nw], u % 60);
        u /= 60;
        if u > 0 {
            w -= 1;
            buf[w] = b'm';
            w = fmt_int(&mut buf[..w], u % 60);
            u /= 60;
            if u > 0 {
                w -= 1;
                buf[w] = b'h';
                w = fmt_int(&mut buf[..w], u);
            }
        }
    }
    if neg {
        w -= 1;
        buf[w] = b'-';
    }
    w
}

/// Formats the fraction of `v / 10**prec` (e.g. ".12345") into the tail of `buf`, omitting
/// trailing zeros and the dot when the fraction is zero. Returns the new start and `v / 10**prec`.
fn fmt_frac(buf: &mut [u8], mut v: u64, prec: usize) -> (usize, u64) {
    let mut w = buf.len();
    let mut print = false;
    for _ in 0..prec {
        let digit = v % 10;
        print = print || digit != 0;
        if print {
            w -= 1;
            buf[w] = digit as u8 + b'0';
        }
        v /= 10;
    }
    if print {
        w -= 1;
        buf[w] = b'.';
    }
    (w, v)
}

/// Formats `v` into the tail of `buf` and returns the start index.
fn fmt_int(buf: &mut [u8], mut v: u64) -> usize {
    let mut w = buf.len();
    if v == 0 {
        w -= 1;
        buf[w] = b'0';
    } else {
        while v > 0 {
            w -= 1;
            buf[w] = (v % 10) as u8 + b'0';
            v /= 10;
        }
    }
    w
}

/// Formats `v` as unsigned decimal (Go's `fmtInt` into a 20-byte buffer).
pub(crate) fn fmt_uint(v: u64) -> String {
    let mut buf = [0u8; 20];
    let w = fmt_int(&mut buf, v);
    String::from_utf8_lossy(&buf[w..]).into_owned()
}

/// Go's `Duration.Microseconds`: the duration as an integer microsecond count.
pub fn duration_microseconds(d: Duration) -> i64 {
    d / 1000
}

/// Go's `Duration.Milliseconds`: the duration as an integer millisecond count.
pub fn duration_milliseconds(d: Duration) -> i64 {
    d / 1_000_000
}

/// Go's `Duration.Seconds`: the duration as a floating point number of seconds.
pub fn duration_seconds(d: Duration) -> f64 {
    let sec = d / SECOND;
    let nsec = d % SECOND;
    sec as f64 + nsec as f64 / 1e9
}

/// Go's `Duration.Minutes`: the duration as a floating point number of minutes.
pub fn duration_minutes(d: Duration) -> f64 {
    let min = d / MINUTE;
    let nsec = d % MINUTE;
    min as f64 + nsec as f64 / (60.0 * 1e9)
}

/// Go's `Duration.Hours`: the duration as a floating point number of hours.
pub fn duration_hours(d: Duration) -> f64 {
    let hour = d / HOUR;
    let nsec = d % HOUR;
    hour as f64 + nsec as f64 / (60.0 * 60.0 * 1e9)
}

/// Go's `Duration.Truncate`: rounds `d` toward zero to a multiple of `m` (unchanged if `m <= 0`).
pub fn duration_truncate(d: Duration, m: Duration) -> Duration {
    if m <= 0 {
        return d;
    }
    d - d % m
}

/// Reports whether `x+x < y` without overflow, assuming `x, y` are non-negative.
pub(crate) fn less_than_half(x: Duration, y: Duration) -> bool {
    (x as u64).wrapping_add(x as u64) < y as u64
}

/// Go's `Duration.Round`: rounds `d` to the nearest multiple of `m`, halfway values away from
/// zero, saturating at the minimum or maximum duration on overflow (unchanged if `m <= 0`).
pub fn duration_round(d: Duration, m: Duration) -> Duration {
    if m <= 0 {
        return d;
    }
    let mut r = d % m;
    if d < 0 {
        r = -r;
        if less_than_half(r, m) {
            return d + r;
        }
        let d1 = d.wrapping_sub(m).wrapping_add(r);
        if d1 < d {
            return d1;
        }
        return MIN_DURATION;
    }
    if less_than_half(r, m) {
        return d - r;
    }
    let d1 = d.wrapping_add(m).wrapping_sub(r);
    if d1 > d {
        return d1;
    }
    MAX_DURATION
}

/// Go's `Duration.Abs`: the absolute value, with the minimum duration mapped to the maximum.
pub fn duration_abs(d: Duration) -> Duration {
    if d >= 0 {
        d
    } else if d == MIN_DURATION {
        MAX_DURATION
    } else {
        -d
    }
}

/// Consumes leading decimal digits, failing when the value exceeds 1<<63 (Go's `leadingInt`).
pub(crate) fn leading_int(s: &[u8]) -> Result<(u64, &[u8]), ()> {
    let mut x: u64 = 0;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if !c.is_ascii_digit() {
            break;
        }
        if x > (1u64 << 63) / 10 {
            return Err(());
        }
        x = x * 10 + u64::from(c - b'0');
        if x > 1u64 << 63 {
            return Err(());
        }
        i += 1;
    }
    Ok((x, &s[i..]))
}

/// Consumes leading decimal digits as a fraction; digits past the precision limit are skipped.
fn leading_fraction(s: &[u8]) -> (u64, f64, &[u8]) {
    let mut i = 0;
    let mut x: u64 = 0;
    let mut scale = 1.0f64;
    let mut overflow = false;
    while i < s.len() {
        let c = s[i];
        if !c.is_ascii_digit() {
            break;
        }
        i += 1;
        if overflow {
            continue;
        }
        if x > (i64::MAX as u64) / 10 {
            overflow = true;
            continue;
        }
        let y = x * 10 + u64::from(c - b'0');
        if y > 1u64 << 63 {
            overflow = true;
            continue;
        }
        x = y;
        scale *= 10.0;
    }
    (x, scale, &s[i..])
}

fn unit_value(u: &[u8]) -> Option<u64> {
    Some(match u {
        b"ns" => NANOSECOND as u64,
        b"us" => MICROSECOND as u64,
        // U+00B5 micro sign and U+03BC Greek small letter mu.
        b"\xc2\xb5s" | b"\xce\xbcs" => MICROSECOND as u64,
        b"ms" => MILLISECOND as u64,
        b"s" => SECOND as u64,
        b"m" => MINUTE as u64,
        b"h" => HOUR as u64,
        _ => return None,
    })
}

/// Go's `time.ParseDuration`: parses strings such as `300ms`, `-1.5h` or `2h45m`.
///
/// Errors carry Go's exact text, e.g. `time: invalid duration "1.2.3"`.
pub fn parse_duration(input: &[u8]) -> Result<Duration, String> {
    let invalid = || format!("time: invalid duration {}", quote(input));
    let mut s = input;
    let mut d: u64 = 0;
    let mut neg = false;
    if let Some(&c) = s.first() {
        if c == b'-' || c == b'+' {
            neg = c == b'-';
            s = &s[1..];
        }
    }
    if s == b"0" {
        return Ok(0);
    }
    if s.is_empty() {
        return Err(invalid());
    }
    while !s.is_empty() {
        let mut f: u64 = 0;
        let mut scale = 1.0f64;
        if !(s[0] == b'.' || s[0].is_ascii_digit()) {
            return Err(invalid());
        }
        let pl = s.len();
        let (mut v, rest) = leading_int(s).map_err(|_| invalid())?;
        s = rest;
        let pre = pl != s.len();
        let mut post = false;
        if s.first() == Some(&b'.') {
            s = &s[1..];
            let pl = s.len();
            let (ff, sc, rest) = leading_fraction(s);
            f = ff;
            scale = sc;
            s = rest;
            post = pl != s.len();
        }
        if !pre && !post {
            return Err(invalid());
        }
        let i = s.iter().position(|&c| c == b'.' || c.is_ascii_digit()).unwrap_or(s.len());
        if i == 0 {
            return Err(format!("time: missing unit in duration {}", quote(input)));
        }
        let u = &s[..i];
        s = &s[i..];
        let unit = match unit_value(u) {
            Some(unit) => unit,
            None => return Err(format!("time: unknown unit {} in duration {}", quote(u), quote(input))),
        };
        if v > (1u64 << 63) / unit {
            return Err(invalid());
        }
        v *= unit;
        if f > 0 {
            // float64 is needed to be nanosecond accurate for fractions of hours.
            v = v.wrapping_add((f as f64 * (unit as f64 / scale)) as u64);
            if v > 1u64 << 63 {
                return Err(invalid());
            }
        }
        d = d.wrapping_add(v);
        if d > 1u64 << 63 {
            return Err(invalid());
        }
    }
    if neg {
        return Ok((d as i64).wrapping_neg());
    }
    if d > i64::MAX as u64 {
        return Err(invalid());
    }
    Ok(d as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings() {
        assert_eq!(duration_string(0), "0s");
        assert_eq!(duration_string(1), "1ns");
        assert_eq!(duration_string(1100), "1.1µs");
        assert_eq!(duration_string(2_200_000), "2.2ms");
        assert_eq!(duration_string(3_300_000_000), "3.3s");
        assert_eq!(duration_string(-(4 * MINUTE + 5 * SECOND)), "-4m5s");
        assert_eq!(duration_string(4 * MINUTE + 5_001 * MILLISECOND), "4m5.001s");
        assert_eq!(duration_string(5 * HOUR + 6 * MINUTE + 7_001 * MILLISECOND), "5h6m7.001s");
        assert_eq!(duration_string(8 * MINUTE + 1), "8m0.000000001s");
        assert_eq!(duration_string(i64::MAX), "2562047h47m16.854775807s");
        assert_eq!(duration_string(i64::MIN), "-2562047h47m16.854775808s");
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_duration(b"0"), Ok(0));
        assert_eq!(parse_duration(b"-1.5h"), Ok(-90 * MINUTE));
        assert_eq!(parse_duration(b"1h2m3.4s"), Ok(3_723_400_000_000));
        assert_eq!(parse_duration(b".5s"), Ok(500_000_000));
        assert_eq!(parse_duration(b"1\xc2\xb5s"), Ok(1000));
        assert_eq!(parse_duration(b"-9223372036854775808ns"), Ok(i64::MIN));
        assert_eq!(
            parse_duration(b"9223372036854775808ns"),
            Err("time: invalid duration \"9223372036854775808ns\"".into())
        );
        assert_eq!(parse_duration(b"1.2.3"), Err("time: missing unit in duration \"1.2.3\"".into()));
        assert_eq!(parse_duration(b"1x"), Err("time: unknown unit \"x\" in duration \"1x\"".into()));
        assert_eq!(parse_duration(b"1"), Err("time: missing unit in duration \"1\"".into()));
        assert_eq!(parse_duration(b""), Err("time: invalid duration \"\"".into()));
        assert_eq!(parse_duration(b"0.3333333333333333333h"), Ok(1_200_000_000_000));
    }

    #[test]
    fn rounding() {
        assert_eq!(duration_round(15, 10), 20);
        assert_eq!(duration_round(-15, 10), -20);
        assert_eq!(duration_round(i64::MAX, 2 * HOUR), i64::MAX);
        assert_eq!(duration_round(i64::MIN, 2 * HOUR), i64::MIN);
        assert_eq!(duration_truncate(-15, 10), -10);
        assert_eq!(duration_truncate(15, 0), 15);
        assert_eq!(duration_abs(i64::MIN), i64::MAX);
        assert_eq!(duration_hours(90 * MINUTE), 1.5);
        assert_eq!(duration_milliseconds(-1_999_999), -1);
    }
}
