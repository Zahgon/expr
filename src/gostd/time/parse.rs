//! Layout-based parsing of times (`Parse`, `ParseInLocation`) with Go's `ParseError` texts.
//!
//! Follows the behaviour of Go's standard library time package (The Go Authors, BSD-style licence).

use super::duration::leading_int;
use super::format::*;
use super::location::{fixed_zone, local, utc, Loc};
use super::{days_before, days_in, is_leap, Time};

/// Sign, hour, minute and second digits of a numeric zone offset, and the remaining value.
type OffsetFields<'a> = (u8, &'a [u8], &'a [u8], &'a [u8], &'a [u8]);

/// Go's `ParseError.Error()` text.
fn parse_error(layout: &[u8], value: &[u8], layout_elem: &[u8], value_elem: &[u8], message: &str) -> String {
    if message.is_empty() {
        return format!(
            "parsing time {} as {}: cannot parse {} as {}",
            quote(value),
            quote(layout),
            quote(value_elem),
            quote(layout_elem)
        );
    }
    format!("parsing time {}{}", quote(value), message)
}

/// Case-insensitive ASCII match of equal-length `s1` and `s2` (Go's `match`).
fn match_fold(s1: &[u8], s2: &[u8]) -> bool {
    s1.iter().zip(s2).all(|(&c1, &c2)| {
        if c1 == c2 {
            return true;
        }
        let (c1, c2) = (c1 | 0x20, c2 | 0x20);
        c1 == c2 && c1.is_ascii_lowercase()
    })
}

/// Finds the table entry that prefixes `val` case-insensitively (Go's `lookup`).
fn lookup_name<'a>(tab: &[&str], val: &'a [u8]) -> Option<(i64, &'a [u8])> {
    for (i, v) in tab.iter().enumerate() {
        let v = v.as_bytes();
        if val.len() >= v.len() && match_fold(&val[..v.len()], v) {
            return Some((i as i64, &val[v.len()..]));
        }
    }
    None
}

/// Go's `atoi`: an optionally signed decimal that must consume the whole input.
fn atoi(s: &[u8]) -> Option<i64> {
    let mut s = s;
    let mut neg = false;
    if let Some(&c) = s.first() {
        if c == b'-' || c == b'+' {
            neg = c == b'-';
            s = &s[1..];
        }
    }
    let (q, rem) = leading_int(s).ok()?;
    if !rem.is_empty() {
        return None;
    }
    let x = q as i64;
    Some(if neg { x.wrapping_neg() } else { x })
}

/// One or two digits (exactly two if `fixed`) (Go's `getnum`).
fn getnum(s: &[u8], fixed: bool) -> Option<(i64, &[u8])> {
    if !is_digit(s, 0) {
        return None;
    }
    if !is_digit(s, 1) {
        if fixed {
            return None;
        }
        return Some((i64::from(s[0] - b'0'), &s[1..]));
    }
    Some((i64::from(s[0] - b'0') * 10 + i64::from(s[1] - b'0'), &s[2..]))
}

/// One to three digits (exactly three if `fixed`) (Go's `getnum3`).
fn getnum3(s: &[u8], fixed: bool) -> Option<(i64, &[u8])> {
    let mut n = 0i64;
    let mut i = 0;
    while i < 3 && is_digit(s, i) {
        n = n * 10 + i64::from(s[i] - b'0');
        i += 1;
    }
    if i == 0 || fixed && i != 3 {
        return None;
    }
    Some((n, &s[i..]))
}

fn cutspace(mut s: &[u8]) -> &[u8] {
    while s.first() == Some(&b' ') {
        s = &s[1..];
    }
    s
}

/// Skips `prefix` in `value`, where a space in the prefix matches any run of spaces
/// (Go's `skip`). On failure returns the remaining value.
fn skip<'a>(mut value: &'a [u8], mut prefix: &[u8]) -> Result<&'a [u8], &'a [u8]> {
    while !prefix.is_empty() {
        if prefix[0] == b' ' {
            if !value.is_empty() && value[0] != b' ' {
                return Err(value);
            }
            prefix = cutspace(prefix);
            value = cutspace(value);
            continue;
        }
        if value.is_empty() || value[0] != prefix[0] {
            return Err(value);
        }
        prefix = &prefix[1..];
        value = &value[1..];
    }
    Ok(value)
}

fn comma_or_period(b: u8) -> bool {
    b == b'.' || b == b','
}

/// Parses the fraction `value[..nbytes]` (separator included) as nanoseconds
/// (Go's `parseNanoseconds`). `Err(Some(msg))` is a range error, `Err(None)` a syntax error.
fn parse_nanoseconds(value: &[u8], nbytes: usize) -> Result<i64, Option<&'static str>> {
    if !comma_or_period(value[0]) {
        return Err(None);
    }
    let nbytes = nbytes.min(10);
    let mut ns = atoi(&value[1..nbytes]).ok_or(None)?;
    if ns < 0 {
        return Err(Some("fractional second"));
    }
    // We need nanoseconds, which means scaling by the number of missing digits in the format.
    for _ in 0..10 - nbytes {
        ns *= 10;
    }
    Ok(ns)
}

/// Length of a signed `±hh` offset (hh <= 23) at the start of `value`, or 0 (Go's
/// `parseSignedOffset`).
fn parse_signed_offset(value: &[u8]) -> usize {
    let sign = value[0];
    if sign != b'-' && sign != b'+' {
        return 0;
    }
    let (x, rem) = match leading_int(&value[1..]) {
        Ok(v) => v,
        Err(()) => return 0,
    };
    if rem.len() == value.len() - 1 {
        return 0;
    }
    if x > 23 {
        return 0;
    }
    value.len() - rem.len()
}

/// Length of a time zone abbreviation at the start of `value` (Go's `parseTimeZone`).
fn parse_time_zone(value: &[u8]) -> Option<usize> {
    if value.len() < 3 {
        return None;
    }
    // Special case 1: ChST and MeST.
    if value.len() >= 4 && (&value[..4] == b"ChST" || &value[..4] == b"MeST") {
        return Some(4);
    }
    // Special case 2: GMT may have an hour offset; treat it specially.
    if &value[..3] == b"GMT" {
        let rest = &value[3..];
        if rest.is_empty() {
            return Some(3);
        }
        return Some(3 + parse_signed_offset(rest));
    }
    // Special case 3: some time zones are not named, but have +/-00 format.
    if value[0] == b'+' || value[0] == b'-' {
        let length = parse_signed_offset(value);
        return if length > 0 { Some(length) } else { None };
    }
    // How many upper-case letters are there? Need at least three, at most five.
    let n_upper = value.iter().take(6).take_while(|c| c.is_ascii_uppercase()).count();
    match n_upper {
        0 | 1 | 2 | 6 => None,
        5 if value[4] == b'T' => Some(5),
        4 if value[3] == b'T' || &value[..4] == b"WITA" => Some(4),
        3 => Some(3),
        _ => None,
    }
}

/// Go's `time.Parse`: parses `value` per `layout`. Without zone information the result is UTC;
/// a zone offset or abbreviation matching the Local zone yields a Local time.
///
/// Errors are Go's `ParseError.Error()` texts, e.g.
/// `parsing time "2020-13-01" as "2006-01-02": month out of range`.
pub fn parse(layout: &[u8], value: &[u8]) -> Result<Time, String> {
    let local = local();
    if layout == RFC3339 || layout == RFC3339_NANO {
        if let Some(t) = parse_rfc3339(value, &local) {
            return Ok(t);
        }
    }
    parse_impl(layout, value, &utc(), &local)
}

/// Go's `time.ParseInLocation`: like [`parse`] but interprets a time without zone information
/// in `loc`, and matches zone offsets and abbreviations against `loc` instead of Local.
pub fn parse_in_location(layout: &[u8], value: &[u8], loc: &Loc) -> Result<Time, String> {
    if layout == RFC3339 || layout == RFC3339_NANO {
        if let Some(t) = parse_rfc3339(value, loc) {
            return Ok(t);
        }
    }
    parse_impl(layout, value, loc, loc)
}

/// Go's fast-path `parseRFC3339`.
fn parse_rfc3339(s: &[u8], local: &Loc) -> Option<Time> {
    let ok = std::cell::Cell::new(true);
    let parse_uint = |s: &[u8], min: i64, max: i64| -> i64 {
        let mut x = 0i64;
        for &c in s {
            if !c.is_ascii_digit() {
                ok.set(false);
                return min;
            }
            x = x * 10 + i64::from(c - b'0');
        }
        if x < min || max < x {
            ok.set(false);
            return min;
        }
        x
    };
    if s.len() < b"2006-01-02T15:04:05".len() {
        return None;
    }
    let year = parse_uint(&s[0..4], 0, 9999);
    let month = parse_uint(&s[5..7], 1, 12);
    let day = parse_uint(&s[8..10], 1, days_in(month, year));
    let hour = parse_uint(&s[11..13], 0, 23);
    let min = parse_uint(&s[14..16], 0, 59);
    let sec = parse_uint(&s[17..19], 0, 59);
    if !(ok.get() && s[4] == b'-' && s[7] == b'-' && s[10] == b'T' && s[13] == b':' && s[16] == b':') {
        return None;
    }
    let mut s = &s[19..];
    let mut nsec = 0;
    if s.len() >= 2 && s[0] == b'.' && is_digit(s, 1) {
        let mut n = 2;
        while n < s.len() && is_digit(s, n) {
            n += 1;
        }
        nsec = parse_nanoseconds(s, n).unwrap_or(0);
        s = &s[n..];
    }
    let mut t = Time::date(year, month, day, hour, min, sec, nsec, &utc());
    if s.len() != 1 || s[0] != b'Z' {
        if s.len() != b"-07:00".len() {
            return None;
        }
        let hr = parse_uint(&s[1..3], 0, 23);
        let mm = parse_uint(&s[4..6], 0, 59);
        if !ok.get() || !((s[0] == b'-' || s[0] == b'+') && s[3] == b':') {
            return None;
        }
        let mut zone_offset = (hr * 60 + mm) * 60;
        if s[0] == b'-' {
            zone_offset = -zone_offset;
        }
        t.add_sec(-zone_offset);
        // Use the local zone with the given offset if possible.
        if local.lookup(t.unix_sec()).offset == zone_offset {
            t = t.with_loc(local);
        } else {
            t = t.with_loc(&fixed_zone(b"", zone_offset));
        }
    }
    Some(t)
}

fn parse_impl(layout_in: &[u8], value_in: &[u8], default_location: &Loc, local: &Loc) -> Result<Time, String> {
    let (alayout, avalue) = (layout_in, value_in);
    let mut layout = layout_in;
    let mut value = value_in;
    let mut range_err: &str = "";
    let mut am_set = false;
    let mut pm_set = false;

    let mut year: i64 = 0;
    let mut month: i64 = -1;
    let mut day: i64 = -1;
    let mut yday: i64 = -1;
    let mut hour: i64 = 0;
    let mut min: i64 = 0;
    let mut sec: i64 = 0;
    let mut nsec: i64 = 0;
    let mut z: Option<Loc> = None;
    let mut zone_offset: i64 = -1;
    let mut zone_name: &[u8] = b"";

    // Each iteration processes one std value.
    loop {
        let (pend, std, sstart) = next_std_chunk(layout);
        let prefix = &layout[..pend];
        let stdstr = &layout[pend..sstart];
        value = match skip(value, prefix) {
            Ok(v) => v,
            Err(v) => return Err(parse_error(alayout, avalue, prefix, v, "")),
        };
        if std == 0 {
            if !value.is_empty() {
                return Err(parse_error(alayout, avalue, b"", value, &format!(": extra text: {}", quote(value))));
            }
            break;
        }
        layout = &layout[sstart..];
        let hold = value;
        // `ok == false` is Go's errBad.
        let mut ok = true;
        match std & STD_MASK {
            STD_YEAR => {
                if value.len() < 2 {
                    ok = false;
                } else {
                    let p = &value[..2];
                    value = &value[2..];
                    match atoi(p) {
                        Some(y) => year = if y >= 69 { y + 1900 } else { y + 2000 },
                        None => ok = false,
                    }
                }
            }
            STD_LONG_YEAR => {
                if value.len() < 4 || !is_digit(value, 0) {
                    ok = false;
                } else {
                    let p = &value[..4];
                    value = &value[4..];
                    match atoi(p) {
                        Some(y) => year = y,
                        None => {
                            year = 0;
                            ok = false;
                        }
                    }
                }
            }
            STD_MONTH | STD_LONG_MONTH => {
                let tab: &[&str] = if std == STD_MONTH { &SHORT_MONTH_NAMES } else { &super::LONG_MONTH_NAMES };
                match lookup_name(tab, value) {
                    Some((m, rest)) => {
                        month = m + 1;
                        value = rest;
                    }
                    None => {
                        month = 0;
                        ok = false;
                    }
                }
            }
            STD_NUM_MONTH | STD_ZERO_MONTH => match getnum(value, std == STD_ZERO_MONTH) {
                Some((m, rest)) => {
                    month = m;
                    value = rest;
                    if month <= 0 || 12 < month {
                        range_err = "month";
                    }
                }
                None => {
                    month = 0;
                    ok = false;
                }
            },
            STD_WEEK_DAY | STD_LONG_WEEK_DAY => {
                let tab: &[&str] = if std == STD_WEEK_DAY { &SHORT_DAY_NAMES } else { &super::LONG_DAY_NAMES };
                match lookup_name(tab, value) {
                    Some((_, rest)) => value = rest,
                    None => ok = false,
                }
            }
            STD_DAY | STD_UNDER_DAY | STD_ZERO_DAY => {
                if std == STD_UNDER_DAY && value.first() == Some(&b' ') {
                    value = &value[1..];
                }
                match getnum(value, std == STD_ZERO_DAY) {
                    Some((d, rest)) => {
                        day = d;
                        value = rest;
                    }
                    None => {
                        day = 0;
                        ok = false;
                    }
                }
                // Note that we allow any one- or two-digit day here.
                // The month, day, year combination is validated after we've completed parsing.
            }
            STD_UNDER_YEAR_DAY | STD_ZERO_YEAR_DAY => {
                for _ in 0..2 {
                    if std == STD_UNDER_YEAR_DAY && value.first() == Some(&b' ') {
                        value = &value[1..];
                    }
                }
                match getnum3(value, std == STD_ZERO_YEAR_DAY) {
                    Some((d, rest)) => {
                        yday = d;
                        value = rest;
                    }
                    None => {
                        yday = 0;
                        ok = false;
                    }
                }
                // Note that we allow any one-, two-, or three-digit year-day here.
                // The year-day, year combination is validated after we've completed parsing.
            }
            STD_HOUR => {
                match getnum(value, false) {
                    Some((h, rest)) => {
                        hour = h;
                        value = rest;
                    }
                    None => {
                        hour = 0;
                        ok = false;
                    }
                }
                if !(0..24).contains(&hour) {
                    range_err = "hour";
                }
            }
            STD_HOUR12 | STD_ZERO_HOUR12 => {
                match getnum(value, std == STD_ZERO_HOUR12) {
                    Some((h, rest)) => {
                        hour = h;
                        value = rest;
                    }
                    None => {
                        hour = 0;
                        ok = false;
                    }
                }
                if !(0..=12).contains(&hour) {
                    range_err = "hour";
                }
            }
            STD_MINUTE | STD_ZERO_MINUTE => {
                match getnum(value, std == STD_ZERO_MINUTE) {
                    Some((m, rest)) => {
                        min = m;
                        value = rest;
                    }
                    None => {
                        min = 0;
                        ok = false;
                    }
                }
                if !(0..60).contains(&min) {
                    range_err = "minute";
                }
            }
            STD_SECOND | STD_ZERO_SECOND => match getnum(value, std == STD_ZERO_SECOND) {
                None => {
                    sec = 0;
                    ok = false;
                }
                Some((s, rest)) => {
                    sec = s;
                    value = rest;
                    if !(0..60).contains(&sec) {
                        range_err = "second";
                    } else if value.len() >= 2 && comma_or_period(value[0]) && is_digit(value, 1) {
                        // Special case: do we have a fractional second but no fractional second
                        // in the format?
                        let (_, next, _) = next_std_chunk(layout);
                        let next = next & STD_MASK;
                        if next != STD_FRAC_SECOND0 && next != STD_FRAC_SECOND9 {
                            // Fractional second in the layout; proceed normally.
                            let mut n = 2;
                            while n < value.len() && is_digit(value, n) {
                                n += 1;
                            }
                            match parse_nanoseconds(value, n) {
                                Ok(ns) => nsec = ns,
                                Err(Some(r)) => {
                                    nsec = 0;
                                    range_err = r;
                                }
                                Err(None) => {
                                    nsec = 0;
                                    ok = false;
                                }
                            }
                            value = &value[n..];
                        }
                    }
                }
            },
            STD_PM | STD_PM_LOWER => {
                if value.len() < 2 {
                    ok = false;
                } else {
                    let p = &value[..2];
                    value = &value[2..];
                    let (pm, am): (&[u8], &[u8]) = if std == STD_PM { (b"PM", b"AM") } else { (b"pm", b"am") };
                    if p == pm {
                        pm_set = true;
                    } else if p == am {
                        am_set = true;
                    } else {
                        ok = false;
                    }
                }
            }
            STD_ISO8601_TZ..=STD_NUM_COLON_SECONDS_TZ => {
                let iso = (STD_ISO8601_TZ..=STD_ISO8601_COLON_SECONDS_TZ).contains(&std);
                if iso && value.first() == Some(&b'Z') {
                    value = &value[1..];
                    z = Some(utc());
                } else {
                    let fields: Option<OffsetFields<'_>> = if std == STD_ISO8601_COLON_TZ || std == STD_NUM_COLON_TZ {
                        if value.len() < 6 || value[3] != b':' {
                            None
                        } else {
                            Some((value[0], &value[1..3], &value[4..6], b"00", &value[6..]))
                        }
                    } else if std == STD_NUM_SHORT_TZ || std == STD_ISO8601_SHORT_TZ {
                        if value.len() < 3 {
                            None
                        } else {
                            Some((value[0], &value[1..3], b"00", b"00", &value[3..]))
                        }
                    } else if std == STD_ISO8601_COLON_SECONDS_TZ || std == STD_NUM_COLON_SECONDS_TZ {
                        if value.len() < 9 || value[3] != b':' || value[6] != b':' {
                            None
                        } else {
                            Some((value[0], &value[1..3], &value[4..6], &value[7..9], &value[9..]))
                        }
                    } else if std == STD_ISO8601_SECONDS_TZ || std == STD_NUM_SECONDS_TZ {
                        if value.len() < 7 {
                            None
                        } else {
                            Some((value[0], &value[1..3], &value[3..5], &value[5..7], &value[7..]))
                        }
                    } else if value.len() < 5 {
                        None
                    } else {
                        Some((value[0], &value[1..3], &value[3..5], b"00", &value[5..]))
                    };
                    match fields {
                        None => ok = false,
                        Some((sign, h, m, s, rest)) => {
                            value = rest;
                            let mut hr = 0;
                            let mut mm = 0;
                            let mut ss = 0;
                            match getnum(h, true) {
                                Some((v, _)) => {
                                    hr = v;
                                    match getnum(m, true) {
                                        Some((v, _)) => {
                                            mm = v;
                                            match getnum(s, true) {
                                                Some((v, _)) => ss = v,
                                                None => ok = false,
                                            }
                                        }
                                        None => ok = false,
                                    }
                                }
                                None => ok = false,
                            }
                            // The range test only checks overflow of two digits.
                            if hr > 24 {
                                range_err = "time zone offset hour";
                            }
                            if mm > 60 {
                                range_err = "time zone offset minute";
                            }
                            if ss > 60 {
                                range_err = "time zone offset second";
                            }
                            zone_offset = (hr * 60 + mm) * 60 + ss;
                            match sign {
                                b'+' => {}
                                b'-' => zone_offset = -zone_offset,
                                _ => ok = false,
                            }
                        }
                    }
                }
            }
            STD_TZ => {
                // Does it look like a time zone?
                if value.len() >= 3 && &value[..3] == b"UTC" {
                    z = Some(utc());
                    value = &value[3..];
                } else {
                    match parse_time_zone(value) {
                        Some(n) => {
                            zone_name = &value[..n];
                            value = &value[n..];
                        }
                        None => ok = false,
                    }
                }
            }
            STD_FRAC_SECOND0 => {
                // stdFracSecond0 requires the exact number of digits as specified in the layout.
                let ndigit = 1 + digits_len(std);
                if value.len() < ndigit {
                    ok = false;
                } else {
                    match parse_nanoseconds(value, ndigit) {
                        Ok(ns) => nsec = ns,
                        Err(Some(r)) => {
                            nsec = 0;
                            range_err = r;
                        }
                        Err(None) => {
                            nsec = 0;
                            ok = false;
                        }
                    }
                    value = &value[ndigit..];
                }
            }
            STD_FRAC_SECOND9 => {
                if value.len() >= 2 && comma_or_period(value[0]) && value[1].is_ascii_digit() {
                    // Take any number of digits, even more than asked for, because it is what
                    // the stdSecond case would do.
                    let mut i = 0;
                    while i + 1 < value.len() && value[i + 1].is_ascii_digit() {
                        i += 1;
                    }
                    match parse_nanoseconds(value, 1 + i) {
                        Ok(ns) => nsec = ns,
                        Err(Some(r)) => {
                            nsec = 0;
                            range_err = r;
                        }
                        Err(None) => {
                            nsec = 0;
                            ok = false;
                        }
                    }
                    value = &value[1 + i..];
                }
            }
            _ => {}
        }
        if !range_err.is_empty() {
            return Err(parse_error(alayout, avalue, stdstr, value, &format!(": {range_err} out of range")));
        }
        if !ok {
            return Err(parse_error(alayout, avalue, stdstr, hold, ""));
        }
    }
    if pm_set && hour < 12 {
        hour += 12;
    } else if am_set && hour == 12 {
        hour = 0;
    }

    // Convert yday to day, month.
    if yday >= 0 {
        let mut d = 0;
        let mut m = 0;
        if is_leap(year) {
            if yday == 31 + 29 {
                m = 2;
                d = 29;
            } else if yday > 31 + 29 {
                yday -= 1;
            }
        }
        if !(1..=365).contains(&yday) {
            return Err(parse_error(alayout, avalue, b"", value, ": day-of-year out of range"));
        }
        if m == 0 {
            m = (yday - 1) / 31 + 1;
            if days_before(m + 1) < yday {
                m += 1;
            }
            d = yday - days_before(m);
        }
        // If month, day already seen, yday's m, d must match.
        if month >= 0 && month != m {
            return Err(parse_error(alayout, avalue, b"", value, ": day-of-year does not match month"));
        }
        month = m;
        if day >= 0 && day != d {
            return Err(parse_error(alayout, avalue, b"", value, ": day-of-year does not match day"));
        }
        day = d;
    } else {
        if month < 0 {
            month = 1;
        }
        if day < 0 {
            day = 1;
        }
    }

    // Validate the day of the month.
    if day < 1 || day > days_in(month, year) {
        return Err(parse_error(alayout, avalue, b"", value, ": day out of range"));
    }

    if let Some(z) = z {
        return Ok(Time::date(year, month, day, hour, min, sec, nsec, &z));
    }

    if zone_offset != -1 {
        let mut t = Time::date(year, month, day, hour, min, sec, nsec, &utc());
        t.add_sec(-zone_offset);
        // Look for local zone with the given offset. If that zone was in effect at the given
        // time, use it.
        let info = local.lookup(t.unix_sec());
        if info.offset == zone_offset && (zone_name.is_empty() || info.name == zone_name) {
            return Ok(t.with_loc(local));
        }
        // Otherwise create fake zone to record offset.
        return Ok(t.with_loc(&fixed_zone(zone_name, zone_offset)));
    }

    if !zone_name.is_empty() {
        let mut t = Time::date(year, month, day, hour, min, sec, nsec, &utc());
        // Look for local zone with the given offset. If that zone was in effect at the given
        // time, use it.
        if let Some(offset) = local.lookup_name(zone_name, t.unix_sec()) {
            t.add_sec(offset.wrapping_neg());
            return Ok(t.with_loc(local));
        }
        // Otherwise, create fake zone with unknown offset.
        let mut offset = 0;
        if zone_name.len() > 3 && &zone_name[..3] == b"GMT" {
            // Guaranteed OK by parseGMT.
            offset = atoi(&zone_name[3..]).unwrap_or(0).wrapping_mul(3600);
        }
        return Ok(t.with_loc(&fixed_zone(zone_name, offset)));
    }

    // Otherwise, fall back to default.
    Ok(Time::date(year, month, day, hour, min, sec, nsec, default_location))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(r: Result<Time, String>) -> String {
        match r {
            Ok(t) => String::from_utf8_lossy(&t.string()).into_owned(),
            Err(e) => e,
        }
    }

    #[test]
    fn valid() {
        assert_eq!(s(parse(DATE_ONLY, b"2020-02-29")), "2020-02-29 00:00:00 +0000 UTC");
        assert_eq!(s(parse(RFC3339, b"2006-01-02T15:04:05.5+07:00")), "2006-01-02 15:04:05.5 +0700 +0700");
        assert_eq!(s(parse(b"2006-01-02 15:04:05", b"2006-01-02 15:04:05.123")), "2006-01-02 15:04:05.123 +0000 UTC");
        assert_eq!(s(parse(b"Jan _2 2006 3:04pm", b"feb  3 2001 12:30am")), "2001-02-03 00:30:00 +0000 UTC");
        assert_eq!(s(parse(b"2006 002", b"2020 060")), "2020-02-29 00:00:00 +0000 UTC");
        assert_eq!(s(parse(RFC1123, b"Mon, 02 Jan 2006 15:04:05 PST")), "2006-01-02 15:04:05 +0000 PST");
        assert_eq!(s(parse(RFC1123, b"Mon, 02 Jan 2006 15:04:05 GMT+3")), "2006-01-02 18:04:05 +0300 GMT+3");
    }

    #[test]
    fn invalid() {
        assert_eq!(s(parse(DATE_ONLY, b"2020-13-01")), "parsing time \"2020-13-01\": month out of range");
        assert_eq!(s(parse(DATE_ONLY, b"x")), "parsing time \"x\" as \"2006-01-02\": cannot parse \"x\" as \"2006\"");
        assert_eq!(s(parse(DATE_ONLY, b"2021-02-29")), "parsing time \"2021-02-29\": day out of range");
        assert_eq!(s(parse(DATE_ONLY, b"2021-02-28x")), "parsing time \"2021-02-28x\": extra text: \"x\"");
        assert_eq!(s(parse(b"2006 002", b"2021 366")), "parsing time \"2021 366\": day-of-year out of range");
        assert_eq!(
            s(parse(b"15:04 -0700", b"10:00 +2500")),
            "parsing time \"10:00 +2500\": time zone offset hour out of range"
        );
        assert_eq!(s(parse(b"05.000", b"05.-12")), "parsing time \"05.-12\": fractional second out of range");
    }

    #[test]
    fn in_location() {
        let ny = super::super::load_location(b"America/New_York").unwrap();
        let syd = super::super::load_location(b"Australia/Sydney").unwrap();
        let p = |l: &[u8], v: &[u8], loc: &Loc| s(parse_in_location(l, v, loc));
        assert_eq!(p(b"2006-01-02 15:04", b"2021-03-14 02:30", &ny), "2021-03-14 01:30:00 -0500 EST");
        assert_eq!(p(b"2006-01-02 15:04", b"2021-11-07 01:30", &ny), "2021-11-07 01:30:00 -0400 EDT");
        assert_eq!(p(b"2006-01-02 15:04 MST", b"2021-07-01 12:00 EST", &ny), "2021-07-01 13:00:00 -0400 EDT");
        assert_eq!(p(b"2006-01-02 15:04 MST", b"2021-07-01 12:00 EDT", &ny), "2021-07-01 12:00:00 -0400 EDT");
        assert_eq!(p(b"2006-01-02 15:04 MST", b"2021-07-01 12:00 CEST", &ny), "2021-07-01 12:00:00 +0000 CEST");
        assert_eq!(p(b"2006-01-02 15:04 -0700", b"2021-07-01 12:00 -0400", &ny), "2021-07-01 12:00:00 -0400 EDT");
        assert_eq!(p(RFC3339, b"2021-07-01T12:00:00+10:00", &syd), "2021-07-01 12:00:00 +1000 AEST");
    }

    #[test]
    fn corners() {
        assert_eq!(s(parse(b"Mon Jan 2", b"Tue Feb 3")), "0000-02-03 00:00:00 +0000 UTC");
        assert_eq!(s(parse(RFC3339, b"2006-01-02T15:04:05+24:00")), "2006-01-02 15:04:05 +2400 +2400");
        assert_eq!(s(parse(RFC3339, b"2006-01-02T15:04:05,5Z")), "2006-01-02 15:04:05.5 +0000 UTC");
        assert_eq!(s(parse(b"2006 Jan", b"2006 Janx")), "parsing time \"2006 Janx\": extra text: \"x\"");
        assert_eq!(s(parse(b"_2 Jan", b"2 Jan")), "0000-01-02 00:00:00 +0000 UTC");
        assert_eq!(s(parse(b"Jan _2", b"Feb 31")), "parsing time \"Feb 31\": day out of range");
    }
}
