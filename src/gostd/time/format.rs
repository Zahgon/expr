//! Layout-based formatting of times, `String`/`GoString`, RFC 3339 and JSON/text marshalling.
//!
//! Follows the behaviour of Go's standard library time package (The Go Authors, BSD-style licence).

use super::location::is_local;
use super::{AbsDays, Time, LONG_DAY_NAMES, LONG_MONTH_NAMES};

/// The reference time layout, in numerical order.
pub const LAYOUT: &[u8] = b"01/02 03:04:05PM '06 -0700";
/// Go's `time.ANSIC`.
pub const ANSIC: &[u8] = b"Mon Jan _2 15:04:05 2006";
/// Go's `time.UnixDate`.
pub const UNIX_DATE: &[u8] = b"Mon Jan _2 15:04:05 MST 2006";
/// Go's `time.RubyDate`.
pub const RUBY_DATE: &[u8] = b"Mon Jan 02 15:04:05 -0700 2006";
/// Go's `time.RFC822`.
pub const RFC822: &[u8] = b"02 Jan 06 15:04 MST";
/// Go's `time.RFC822Z` (RFC 822 with a numeric zone).
pub const RFC822Z: &[u8] = b"02 Jan 06 15:04 -0700";
/// Go's `time.RFC850`.
pub const RFC850: &[u8] = b"Monday, 02-Jan-06 15:04:05 MST";
/// Go's `time.RFC1123`.
pub const RFC1123: &[u8] = b"Mon, 02 Jan 2006 15:04:05 MST";
/// Go's `time.RFC1123Z` (RFC 1123 with a numeric zone).
pub const RFC1123Z: &[u8] = b"Mon, 02 Jan 2006 15:04:05 -0700";
/// Go's `time.RFC3339`.
pub const RFC3339: &[u8] = b"2006-01-02T15:04:05Z07:00";
/// Go's `time.RFC3339Nano`.
pub const RFC3339_NANO: &[u8] = b"2006-01-02T15:04:05.999999999Z07:00";
/// Go's `time.Kitchen`.
pub const KITCHEN: &[u8] = b"3:04PM";
/// Go's `time.Stamp`.
pub const STAMP: &[u8] = b"Jan _2 15:04:05";
/// Go's `time.StampMilli`.
pub const STAMP_MILLI: &[u8] = b"Jan _2 15:04:05.000";
/// Go's `time.StampMicro`.
pub const STAMP_MICRO: &[u8] = b"Jan _2 15:04:05.000000";
/// Go's `time.StampNano`.
pub const STAMP_NANO: &[u8] = b"Jan _2 15:04:05.000000000";
/// Go's `time.DateTime`.
pub const DATE_TIME: &[u8] = b"2006-01-02 15:04:05";
/// Go's `time.DateOnly`.
pub const DATE_ONLY: &[u8] = b"2006-01-02";
/// Go's `time.TimeOnly`.
pub const TIME_ONLY: &[u8] = b"15:04:05";

// Layout chunk codes, numerically identical to Go's std* constants.
pub(crate) const STD_NEED_DATE: i32 = 1 << 8;
pub(crate) const STD_NEED_YDAY: i32 = 1 << 9;
pub(crate) const STD_NEED_CLOCK: i32 = 1 << 10;
pub(crate) const STD_ARG_SHIFT: i32 = 16;
pub(crate) const STD_SEPARATOR_SHIFT: i32 = 28;
pub(crate) const STD_MASK: i32 = (1 << STD_ARG_SHIFT) - 1;

pub(crate) const STD_LONG_MONTH: i32 = 1 + STD_NEED_DATE; // "January"
pub(crate) const STD_MONTH: i32 = 2 + STD_NEED_DATE; // "Jan"
pub(crate) const STD_NUM_MONTH: i32 = 3 + STD_NEED_DATE; // "1"
pub(crate) const STD_ZERO_MONTH: i32 = 4 + STD_NEED_DATE; // "01"
pub(crate) const STD_LONG_WEEK_DAY: i32 = 5 + STD_NEED_DATE; // "Monday"
pub(crate) const STD_WEEK_DAY: i32 = 6 + STD_NEED_DATE; // "Mon"
pub(crate) const STD_DAY: i32 = 7 + STD_NEED_DATE; // "2"
pub(crate) const STD_UNDER_DAY: i32 = 8 + STD_NEED_DATE; // "_2"
pub(crate) const STD_ZERO_DAY: i32 = 9 + STD_NEED_DATE; // "02"
pub(crate) const STD_UNDER_YEAR_DAY: i32 = 10 + STD_NEED_YDAY; // "__2"
pub(crate) const STD_ZERO_YEAR_DAY: i32 = 11 + STD_NEED_YDAY; // "002"
pub(crate) const STD_HOUR: i32 = 12 + STD_NEED_CLOCK; // "15"
pub(crate) const STD_HOUR12: i32 = 13 + STD_NEED_CLOCK; // "3"
pub(crate) const STD_ZERO_HOUR12: i32 = 14 + STD_NEED_CLOCK; // "03"
pub(crate) const STD_MINUTE: i32 = 15 + STD_NEED_CLOCK; // "4"
pub(crate) const STD_ZERO_MINUTE: i32 = 16 + STD_NEED_CLOCK; // "04"
pub(crate) const STD_SECOND: i32 = 17 + STD_NEED_CLOCK; // "5"
pub(crate) const STD_ZERO_SECOND: i32 = 18 + STD_NEED_CLOCK; // "05"
pub(crate) const STD_LONG_YEAR: i32 = 19 + STD_NEED_DATE; // "2006"
pub(crate) const STD_YEAR: i32 = 20 + STD_NEED_DATE; // "06"
pub(crate) const STD_PM: i32 = 21 + STD_NEED_CLOCK; // "PM"
pub(crate) const STD_PM_LOWER: i32 = 22 + STD_NEED_CLOCK; // "pm"
pub(crate) const STD_TZ: i32 = 23; // "MST"
pub(crate) const STD_ISO8601_TZ: i32 = 24; // "Z0700"
pub(crate) const STD_ISO8601_SECONDS_TZ: i32 = 25; // "Z070000"
pub(crate) const STD_ISO8601_SHORT_TZ: i32 = 26; // "Z07"
pub(crate) const STD_ISO8601_COLON_TZ: i32 = 27; // "Z07:00"
pub(crate) const STD_ISO8601_COLON_SECONDS_TZ: i32 = 28; // "Z07:00:00"
pub(crate) const STD_NUM_TZ: i32 = 29; // "-0700"
pub(crate) const STD_NUM_SECONDS_TZ: i32 = 30; // "-070000"
pub(crate) const STD_NUM_SHORT_TZ: i32 = 31; // "-07"
pub(crate) const STD_NUM_COLON_TZ: i32 = 32; // "-07:00"
pub(crate) const STD_NUM_COLON_SECONDS_TZ: i32 = 33; // "-07:00:00"
pub(crate) const STD_FRAC_SECOND0: i32 = 34; // ".0", ".00", ... trailing zeros included
pub(crate) const STD_FRAC_SECOND9: i32 = 35; // ".9", ".99", ... trailing zeros omitted

const STD0X: [i32; 6] = [STD_ZERO_MONTH, STD_ZERO_DAY, STD_ZERO_HOUR12, STD_ZERO_MINUTE, STD_ZERO_SECOND, STD_YEAR];

pub(crate) const SHORT_DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub(crate) const SHORT_MONTH_NAMES: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn starts_with_lower_case(s: &[u8]) -> bool {
    s.first().is_some_and(u8::is_ascii_lowercase)
}

pub(crate) fn is_digit(s: &[u8], i: usize) -> bool {
    s.get(i).is_some_and(u8::is_ascii_digit)
}

fn std_frac_second(code: i32, n: usize, c: u8) -> i32 {
    let n = ((n & 0xfff) as i32) << STD_ARG_SHIFT;
    if c == b'.' {
        return code | n;
    }
    code | n | 1 << STD_SEPARATOR_SHIFT
}

pub(crate) fn digits_len(std: i32) -> usize {
    ((std >> STD_ARG_SHIFT) & 0xfff) as usize
}

fn separator(std: i32) -> u8 {
    if std >> STD_SEPARATOR_SHIFT == 0 {
        b'.'
    } else {
        b','
    }
}

/// Finds the first layout chunk: returns `(prefix_end, std, suffix_start)`, so the prefix is
/// `layout[..prefix_end]` and the suffix `layout[suffix_start..]` (Go's `nextStdChunk`).
pub(crate) fn next_std_chunk(layout: &[u8]) -> (usize, i32, usize) {
    let l = layout;
    let has = |i: usize, pat: &[u8]| l.len() >= i + pat.len() && &l[i..i + pat.len()] == pat;
    for i in 0..l.len() {
        match l[i] {
            b'J' => {
                if has(i, b"Jan") {
                    if has(i, b"January") {
                        return (i, STD_LONG_MONTH, i + 7);
                    }
                    if !starts_with_lower_case(&l[i + 3..]) {
                        return (i, STD_MONTH, i + 3);
                    }
                }
            }
            b'M' => {
                if l.len() >= i + 3 {
                    if has(i, b"Mon") {
                        if has(i, b"Monday") {
                            return (i, STD_LONG_WEEK_DAY, i + 6);
                        }
                        if !starts_with_lower_case(&l[i + 3..]) {
                            return (i, STD_WEEK_DAY, i + 3);
                        }
                    }
                    if has(i, b"MST") {
                        return (i, STD_TZ, i + 3);
                    }
                }
            }
            b'0' => {
                if l.len() >= i + 2 && (b'1'..=b'6').contains(&l[i + 1]) {
                    return (i, STD0X[usize::from(l[i + 1] - b'1')], i + 2);
                }
                if has(i, b"002") {
                    return (i, STD_ZERO_YEAR_DAY, i + 3);
                }
            }
            b'1' => {
                if l.len() >= i + 2 && l[i + 1] == b'5' {
                    return (i, STD_HOUR, i + 2);
                }
                return (i, STD_NUM_MONTH, i + 1);
            }
            b'2' => {
                if has(i, b"2006") {
                    return (i, STD_LONG_YEAR, i + 4);
                }
                return (i, STD_DAY, i + 1);
            }
            b'_' => {
                if l.len() >= i + 2 && l[i + 1] == b'2' {
                    // _2006 is really a literal _, followed by stdLongYear.
                    if has(i + 1, b"2006") {
                        return (i + 1, STD_LONG_YEAR, i + 5);
                    }
                    return (i, STD_UNDER_DAY, i + 2);
                }
                if has(i, b"__2") {
                    return (i, STD_UNDER_YEAR_DAY, i + 3);
                }
            }
            b'3' => return (i, STD_HOUR12, i + 1),
            b'4' => return (i, STD_MINUTE, i + 1),
            b'5' => return (i, STD_SECOND, i + 1),
            b'P' => {
                if l.len() >= i + 2 && l[i + 1] == b'M' {
                    return (i, STD_PM, i + 2);
                }
            }
            b'p' => {
                if l.len() >= i + 2 && l[i + 1] == b'm' {
                    return (i, STD_PM_LOWER, i + 2);
                }
            }
            b'-' => {
                if has(i, b"-070000") {
                    return (i, STD_NUM_SECONDS_TZ, i + 7);
                }
                if has(i, b"-07:00:00") {
                    return (i, STD_NUM_COLON_SECONDS_TZ, i + 9);
                }
                if has(i, b"-0700") {
                    return (i, STD_NUM_TZ, i + 5);
                }
                if has(i, b"-07:00") {
                    return (i, STD_NUM_COLON_TZ, i + 6);
                }
                if has(i, b"-07") {
                    return (i, STD_NUM_SHORT_TZ, i + 3);
                }
            }
            b'Z' => {
                if has(i, b"Z070000") {
                    return (i, STD_ISO8601_SECONDS_TZ, i + 7);
                }
                if has(i, b"Z07:00:00") {
                    return (i, STD_ISO8601_COLON_SECONDS_TZ, i + 9);
                }
                if has(i, b"Z0700") {
                    return (i, STD_ISO8601_TZ, i + 5);
                }
                if has(i, b"Z07:00") {
                    return (i, STD_ISO8601_COLON_TZ, i + 6);
                }
                if has(i, b"Z07") {
                    return (i, STD_ISO8601_SHORT_TZ, i + 3);
                }
            }
            c @ (b'.' | b',') => {
                // ,000 or .000 or ,999 or .999: repeated digits for fractional seconds.
                if i + 1 < l.len() && (l[i + 1] == b'0' || l[i + 1] == b'9') {
                    let ch = l[i + 1];
                    let mut j = i + 1;
                    while j < l.len() && l[j] == ch {
                        j += 1;
                    }
                    // String of digits must end here - only fractional second if all digits.
                    if !is_digit(l, j) {
                        let code = if ch == b'9' { STD_FRAC_SECOND9 } else { STD_FRAC_SECOND0 };
                        return (i, std_frac_second(code, j - (i + 1), c), j);
                    }
                }
            }
            _ => {}
        }
    }
    (l.len(), 0, l.len())
}

/// Appends the decimal `x`, zero-padded to `width` digits (Go's `appendInt`).
pub(crate) fn append_int(b: &mut Vec<u8>, x: i64, width: usize) {
    let mut u = x as u64;
    if x < 0 {
        b.push(b'-');
        u = x.wrapping_neg() as u64;
    }
    let digits = u.to_string();
    for _ in digits.len()..width {
        b.push(b'0');
    }
    b.extend_from_slice(digits.as_bytes());
}

/// Appends the fractional second `nanosec` in the format chunk `std` (Go's `appendNano`).
fn append_nano(b: &mut Vec<u8>, nanosec: i64, std: i32) {
    let trim = std & STD_MASK == STD_FRAC_SECOND9;
    let n = digits_len(std);
    if trim && (n == 0 || nanosec == 0) {
        return;
    }
    let dot = separator(std);
    let start = b.len();
    b.push(dot);
    append_int(b, nanosec, 9);
    if n < 9 {
        let keep = b.len() - 9 + n;
        b.truncate(keep);
    }
    if trim {
        while b.len() > start && b.last() == Some(&b'0') {
            b.pop();
        }
        if b.last() == Some(&dot) && b.len() > start {
            b.pop();
        }
    }
}

/// Decodes one UTF-8 sequence like Go's `range` over a string: invalid input yields
/// `(U+FFFD, 1)`.
pub(crate) fn decode_rune(s: &[u8]) -> (u32, usize) {
    let b0 = match s.first() {
        Some(&b) => b,
        None => return (0xFFFD, 0),
    };
    if b0 < 0x80 {
        return (u32::from(b0), 1);
    }
    let (n, lo, hi) = match b0 {
        0xC2..=0xDF => (2, 0x80, 0xBF),
        0xE0 => (3, 0xA0, 0xBF),
        0xE1..=0xEC | 0xEE..=0xEF => (3, 0x80, 0xBF),
        0xED => (3, 0x80, 0x9F),
        0xF0 => (4, 0x90, 0xBF),
        0xF1..=0xF3 => (4, 0x80, 0xBF),
        0xF4 => (4, 0x80, 0x8F),
        _ => return (0xFFFD, 1),
    };
    if s.len() < n || !(lo..=hi).contains(&s[1]) || s[2..n].iter().any(|&c| !(0x80..=0xBF).contains(&c)) {
        return (0xFFFD, 1);
    }
    let mut r = u32::from(b0) & (0x7F >> n);
    for &c in &s[1..n] {
        r = r << 6 | u32::from(c & 0x3F);
    }
    (r, n)
}

/// Go's `time` package `quote` helper: double-quotes `s`, escaping `"` and `\`, and writing
/// every byte of control and non-ASCII characters as `\xNN`. The result is ASCII.
pub fn quote(s: &[u8]) -> String {
    const LOWERHEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf = String::with_capacity(s.len() + 2);
    buf.push('"');
    let mut i = 0;
    while i < s.len() {
        let (c, width) = decode_rune(&s[i..]);
        if c >= 0x80 || c < u32::from(b' ') {
            for &byte in &s[i..i + width] {
                buf.push_str("\\x");
                buf.push(char::from(LOWERHEX[usize::from(byte >> 4)]));
                buf.push(char::from(LOWERHEX[usize::from(byte & 0xF)]));
            }
        } else {
            if c == u32::from(b'"') || c == u32::from(b'\\') {
                buf.push('\\');
            }
            buf.push(char::from(s[i]));
        }
        i += width;
    }
    buf.push('"');
    buf
}

/// Appends a zone offset as `±hh[:]mm`.
fn append_offset_hm(b: &mut Vec<u8>, offset: i64, colon: bool) {
    let mut zone = offset / 60;
    if zone < 0 {
        b.push(b'-');
        zone = -zone;
    } else {
        b.push(b'+');
    }
    append_int(b, zone / 60, 2);
    if colon {
        b.push(b':');
    }
    append_int(b, zone % 60, 2);
}

impl Time {
    /// Go's `Time.Format`: the time formatted according to `layout` (see Go's layout rules).
    pub fn format(&self, layout: &[u8]) -> Vec<u8> {
        self.append_format(Vec::with_capacity(layout.len() + 10), layout)
    }

    /// Go's `Time.AppendFormat`: like [`Time::format`] but appends to `b`.
    pub fn append_format(&self, mut b: Vec<u8>, layout: &[u8]) -> Vec<u8> {
        if layout == RFC3339 {
            self.append_format_rfc3339(&mut b, false);
        } else if layout == RFC3339_NANO {
            self.append_format_rfc3339(&mut b, true);
        } else {
            self.append_format_layout(&mut b, layout);
        }
        b
    }

    fn append_format_layout(&self, b: &mut Vec<u8>, mut layout: &[u8]) {
        let (name, offset, abs) = self.locabs();
        let days: AbsDays = abs.days();
        let mut date: Option<(i64, i64, i64)> = None;
        let mut yday: Option<i64> = None;
        let mut clock: Option<(i64, i64, i64)> = None;

        while !layout.is_empty() {
            let (pend, std, sstart) = next_std_chunk(layout);
            b.extend_from_slice(&layout[..pend]);
            if std == 0 {
                break;
            }
            layout = &layout[sstart..];

            if std & STD_NEED_DATE != 0 && date.is_none() {
                date = Some(days.date());
            }
            if std & STD_NEED_YDAY != 0 && yday.is_none() {
                yday = Some(days.year_yday().1);
            }
            if std & STD_NEED_CLOCK != 0 && clock.is_none() {
                clock = Some(abs.clock());
            }
            let (year, month, day) = date.unwrap_or((0, 1, 0));
            let yday = yday.unwrap_or(0);
            let (hour, min, sec) = clock.unwrap_or((0, 0, 0));

            match std & STD_MASK {
                STD_YEAR => append_int(b, year.wrapping_abs() % 100, 2),
                STD_LONG_YEAR => append_int(b, year, 4),
                STD_MONTH => b.extend_from_slice(&LONG_MONTH_NAMES[(month - 1) as usize].as_bytes()[..3]),
                STD_LONG_MONTH => b.extend_from_slice(LONG_MONTH_NAMES[(month - 1) as usize].as_bytes()),
                STD_NUM_MONTH => append_int(b, month, 0),
                STD_ZERO_MONTH => append_int(b, month, 2),
                STD_WEEK_DAY => b.extend_from_slice(&LONG_DAY_NAMES[days.weekday() as usize].as_bytes()[..3]),
                STD_LONG_WEEK_DAY => b.extend_from_slice(LONG_DAY_NAMES[days.weekday() as usize].as_bytes()),
                STD_DAY => append_int(b, day, 0),
                STD_UNDER_DAY => {
                    if day < 10 {
                        b.push(b' ');
                    }
                    append_int(b, day, 0);
                }
                STD_ZERO_DAY => append_int(b, day, 2),
                STD_UNDER_YEAR_DAY => {
                    if yday < 100 {
                        b.push(b' ');
                        if yday < 10 {
                            b.push(b' ');
                        }
                    }
                    append_int(b, yday, 0);
                }
                STD_ZERO_YEAR_DAY => append_int(b, yday, 3),
                STD_HOUR => append_int(b, hour, 2),
                STD_HOUR12 | STD_ZERO_HOUR12 => {
                    let hr = match hour % 12 {
                        0 => 12,
                        h => h,
                    };
                    append_int(b, hr, if std == STD_HOUR12 { 0 } else { 2 });
                }
                STD_MINUTE => append_int(b, min, 0),
                STD_ZERO_MINUTE => append_int(b, min, 2),
                STD_SECOND => append_int(b, sec, 0),
                STD_ZERO_SECOND => append_int(b, sec, 2),
                STD_PM => b.extend_from_slice(if hour >= 12 { b"PM" } else { b"AM" }),
                STD_PM_LOWER => b.extend_from_slice(if hour >= 12 { b"pm" } else { b"am" }),
                STD_ISO8601_TZ..=STD_NUM_COLON_SECONDS_TZ => {
                    let iso = (STD_ISO8601_TZ..=STD_ISO8601_COLON_SECONDS_TZ).contains(&std);
                    if offset == 0 && iso {
                        b.push(b'Z');
                        continue;
                    }
                    let mut zone = offset / 60;
                    let mut absoffset = offset;
                    if zone < 0 {
                        b.push(b'-');
                        zone = -zone;
                        absoffset = absoffset.wrapping_neg();
                    } else {
                        b.push(b'+');
                    }
                    append_int(b, zone / 60, 2);
                    if matches!(
                        std,
                        STD_ISO8601_COLON_TZ
                            | STD_NUM_COLON_TZ
                            | STD_ISO8601_COLON_SECONDS_TZ
                            | STD_NUM_COLON_SECONDS_TZ
                    ) {
                        b.push(b':');
                    }
                    if std != STD_NUM_SHORT_TZ && std != STD_ISO8601_SHORT_TZ {
                        append_int(b, zone % 60, 2);
                    }
                    // Append seconds if appropriate.
                    if matches!(
                        std,
                        STD_ISO8601_SECONDS_TZ
                            | STD_NUM_SECONDS_TZ
                            | STD_NUM_COLON_SECONDS_TZ
                            | STD_ISO8601_COLON_SECONDS_TZ
                    ) {
                        if std == STD_NUM_COLON_SECONDS_TZ || std == STD_ISO8601_COLON_SECONDS_TZ {
                            b.push(b':');
                        }
                        append_int(b, absoffset % 60, 2);
                    }
                }
                STD_TZ => {
                    // Ugly special case: we cheat and take the "MST" to mean "the zone name".
                    if !name.is_empty() {
                        b.extend_from_slice(&name);
                    } else {
                        append_offset_hm(b, offset, false);
                    }
                }
                STD_FRAC_SECOND0 | STD_FRAC_SECOND9 => append_nano(b, i64::from(self.nsec), std),
                _ => {}
            }
        }
    }

    /// Go's `appendFormatRFC3339`.
    fn append_format_rfc3339(&self, b: &mut Vec<u8>, nanos: bool) {
        let (_, offset, abs) = self.locabs();
        let (year, month, day) = abs.days().date();
        append_int(b, year, 4);
        b.push(b'-');
        append_int(b, month, 2);
        b.push(b'-');
        append_int(b, day, 2);
        b.push(b'T');
        let (hour, min, sec) = abs.clock();
        append_int(b, hour, 2);
        b.push(b':');
        append_int(b, min, 2);
        b.push(b':');
        append_int(b, sec, 2);
        if nanos {
            append_nano(b, i64::from(self.nsec), std_frac_second(STD_FRAC_SECOND9, 9, b'.'));
        }
        if offset == 0 {
            b.push(b'Z');
            return;
        }
        append_offset_hm(b, offset, true);
    }

    /// Go's `appendStrictRFC3339`: RFC 3339 with nanoseconds, rejecting years outside
    /// [0,9999] and zone hours outside [0,23].
    fn append_strict_rfc3339(&self, b: &mut Vec<u8>) -> Result<(), &'static str> {
        let n0 = b.len();
        self.append_format_rfc3339(b, true);
        let num2 = |s: &[u8]| (s[0].wrapping_sub(b'0')).wrapping_mul(10).wrapping_add(s[1].wrapping_sub(b'0'));
        if b[n0 + 4] != b'-' {
            return Err("year outside of range [0,9999]");
        }
        if b[b.len() - 1] != b'Z' {
            let c = b[b.len() - 6];
            if c.is_ascii_digit() || num2(&b[b.len() - 5..]) >= 24 {
                return Err("timezone hour outside of range [0,23]");
            }
        }
        Ok(())
    }

    /// Go's `Time.MarshalJSON`: the RFC 3339 (nano) time in double quotes, or an error such as
    /// `Time.MarshalJSON: year outside of range [0,9999]`.
    pub fn marshal_json(&self) -> Result<Vec<u8>, String> {
        let mut b = vec![b'"'];
        let r = self.append_strict_rfc3339(&mut b);
        b.push(b'"');
        match r {
            Ok(()) => Ok(b),
            Err(e) => Err(format!("Time.MarshalJSON: {e}")),
        }
    }

    /// Go's `Time.MarshalText`: the RFC 3339 (nano) time, or an error such as
    /// `Time.MarshalText: year outside of range [0,9999]`.
    pub fn marshal_text(&self) -> Result<Vec<u8>, String> {
        let mut b = Vec::new();
        match self.append_strict_rfc3339(&mut b) {
            Ok(()) => Ok(b),
            Err(e) => Err(format!("Time.MarshalText: {e}")),
        }
    }

    /// Go's `Time.String` (without a monotonic clock suffix):
    /// `2006-01-02 15:04:05.999999999 -0700 MST`.
    pub fn string(&self) -> Vec<u8> {
        self.format(b"2006-01-02 15:04:05.999999999 -0700 MST")
    }

    /// Go's `Time.GoString`, e.g. `time.Date(2009, time.November, 10, 23, 0, 0, 0, time.UTC)`.
    pub fn go_string(&self) -> Vec<u8> {
        let abs = self.locabs().2;
        let (year, month, day) = abs.days().date();
        let (hour, minute, second) = abs.clock();
        let mut buf = b"time.Date(".to_vec();
        append_int(&mut buf, year, 0);
        buf.extend_from_slice(b", time.");
        buf.extend_from_slice(LONG_MONTH_NAMES[(month - 1) as usize].as_bytes());
        for v in [day, hour, minute, second, i64::from(self.nsec)] {
            buf.extend_from_slice(b", ");
            append_int(&mut buf, v, 0);
        }
        buf.extend_from_slice(b", ");
        match &self.loc {
            None => buf.extend_from_slice(b"time.UTC"),
            Some(l) if is_local(l) => buf.extend_from_slice(b"time.Local"),
            Some(l) => {
                buf.extend_from_slice(b"time.Location(");
                buf.extend_from_slice(quote(&l.name).as_bytes());
                buf.push(b')');
            }
        }
        buf.push(b')');
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::super::{fixed_zone, load_location, unix, utc};
    use super::*;

    fn t0() -> Time {
        Time::date(2009, 2, 4, 21, 0, 57, 12_345_600, &load_location(b"America/Los_Angeles").unwrap())
    }

    #[test]
    fn layouts() {
        let t = t0();
        assert_eq!(t.format(ANSIC), b"Wed Feb  4 21:00:57 2009".to_vec());
        assert_eq!(t.format(UNIX_DATE), b"Wed Feb  4 21:00:57 PST 2009".to_vec());
        assert_eq!(t.format(RUBY_DATE), b"Wed Feb 04 21:00:57 -0800 2009".to_vec());
        assert_eq!(t.format(RFC822), b"04 Feb 09 21:00 PST".to_vec());
        assert_eq!(t.format(RFC850), b"Wednesday, 04-Feb-09 21:00:57 PST".to_vec());
        assert_eq!(t.format(RFC3339), b"2009-02-04T21:00:57-08:00".to_vec());
        assert_eq!(t.format(RFC3339_NANO), b"2009-02-04T21:00:57.0123456-08:00".to_vec());
        assert_eq!(t.format(KITCHEN), b"9:00PM".to_vec());
        assert_eq!(t.format(STAMP_MICRO), b"Feb  4 21:00:57.012345".to_vec());
        assert_eq!(t.format(b"__2 002 _2006 Z07 -07:00:00 ,999"), b" 35 035 _2009 -08 -08:00:00 ,012".to_vec());
    }

    #[test]
    fn odd_values() {
        let t = Time::date(-12345, 1, 1, 0, 0, 0, 0, &utc());
        assert_eq!(t.format(b"2006 06"), b"-12345 45".to_vec());
        let t = unix(0, 0).in_location(&fixed_zone(b"", 5 * 3600 + 30 * 60 + 15));
        assert_eq!(t.format(b"MST Z070000"), b"+0530 +053015".to_vec());
        assert_eq!(t.marshal_json(), Ok(b"\"1970-01-01T05:30:15+05:30\"".to_vec()));
        let far = Time::date(10000, 1, 1, 0, 0, 0, 0, &utc());
        assert_eq!(far.marshal_json(), Err("Time.MarshalJSON: year outside of range [0,9999]".into()));
        let tz = Time::default().in_location(&fixed_zone(b"X", 24 * 3600));
        assert_eq!(tz.marshal_text(), Err("Time.MarshalText: timezone hour outside of range [0,23]".into()));
    }

    #[test]
    fn go_strings() {
        assert_eq!(
            t0().go_string(),
            b"time.Date(2009, time.February, 4, 21, 0, 57, 12345600, time.Location(\"America/Los_Angeles\"))".to_vec()
        );
        assert_eq!(Time::default().go_string(), b"time.Date(1, time.January, 1, 0, 0, 0, 0, time.UTC)".to_vec());
    }

    #[test]
    fn quoting() {
        assert_eq!(quote(b"a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(quote(b"\xff\xc3\xa9\n"), "\"\\xff\\xc3\\xa9\\x0a\"");
    }
}
