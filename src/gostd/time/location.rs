//! Locations (time zones): zone lookup, POSIX TZ rule strings, UTC and the Local zone.
//!
//! Follows the behaviour of Go's standard library time package (The Go Authors, BSD-style licence).

use std::cell::OnceCell;
use std::rc::Rc;

use super::zoneinfo::{
    load_location_from, load_location_from_tz_data, load_tzinfo_from_dir_or_zip, LoadError, PLATFORM_ZONE_SOURCES,
};
use super::{
    days_before, days_in, is_leap, AbsSeconds, SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_MINUTE, UNIX_TO_ABSOLUTE,
};

/// The beginning of time for zone transitions.
pub(crate) const ALPHA: i64 = i64::MIN;
/// The end of time for zone transitions.
pub(crate) const OMEGA: i64 = i64::MAX;

/// A single time zone such as CET.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Zone {
    /// Abbreviated name, "CET".
    pub(crate) name: Vec<u8>,
    /// Seconds east of UTC.
    pub(crate) offset: i64,
    /// Whether this zone is daylight saving time.
    pub(crate) is_dst: bool,
}

/// A single time zone transition.
#[derive(Clone, Debug)]
pub(crate) struct ZoneTrans {
    /// Transition time in seconds since 1970 UTC.
    pub(crate) when: i64,
    /// Index of the zone that goes into effect at that time.
    pub(crate) index: u8,
}

/// A Go `time.Location`: maps time instants to the zone in use at that time.
#[derive(Clone, Debug)]
pub struct Location {
    pub(crate) name: Vec<u8>,
    pub(crate) zone: Vec<Zone>,
    pub(crate) tx: Vec<ZoneTrans>,
    /// POSIX TZ string describing transitions after the last recorded one.
    pub(crate) extend: Vec<u8>,
    /// One-element cache for the zone in effect when the location was created.
    pub(crate) cache_start: i64,
    pub(crate) cache_end: i64,
    pub(crate) cache_zone: Option<Zone>,
}

/// A shared reference to a [`Location`] (Go's `*time.Location`).
pub type Loc = Rc<Location>;

/// The result of a zone lookup (Go's `Location.lookup` results).
pub(crate) struct ZoneInfo<'a> {
    pub(crate) name: &'a [u8],
    pub(crate) offset: i64,
    pub(crate) start: i64,
    pub(crate) end: i64,
    pub(crate) is_dst: bool,
}

thread_local! {
    static UTC_LOC: Loc = Rc::new(Location::empty(b"UTC".to_vec()));
    static LOCAL_LOC: OnceCell<Loc> = const { OnceCell::new() };
    static ZONEINFO: OnceCell<Vec<u8>> = const { OnceCell::new() };
}

/// Go's `time.UTC`.
pub fn utc() -> Loc {
    UTC_LOC.with(Rc::clone)
}

/// Go's `time.Local`, initialised once (per thread) from `$TZ` like Go's `initLocal` on Unix.
///
/// TZ unset uses `/etc/localtime` (named "Local"); `TZ=""` or `TZ="UTC"` gives UTC; `TZ=":x"`
/// or `TZ="x"` loads `x` (an absolute path, or a name in the system zoneinfo directories, keeping
/// that name); on failure the zone is UTC.
pub fn local() -> Loc {
    LOCAL_LOC.with(|cell| Rc::clone(cell.get_or_init(|| Rc::new(init_local()))))
}

pub(crate) fn is_utc(loc: &Loc) -> bool {
    UTC_LOC.with(|u| Rc::ptr_eq(u, loc))
}

pub(crate) fn is_local(loc: &Loc) -> bool {
    LOCAL_LOC.with(|cell| cell.get().is_some_and(|l| Rc::ptr_eq(l, loc)))
}

fn init_local() -> Location {
    use std::os::unix::ffi::OsStrExt;
    match std::env::var_os("TZ") {
        None => {
            if let Ok(mut z) = load_location_from(b"localtime", &[b"/etc"]) {
                z.name = b"Local".to_vec();
                return z;
            }
        }
        Some(tz) => {
            let mut tz = tz.as_bytes();
            if !tz.is_empty() {
                if tz[0] == b':' {
                    tz = &tz[1..];
                }
                if !tz.is_empty() && tz[0] == b'/' {
                    if let Ok(mut z) = load_location_from(tz, &[b""]) {
                        z.name = if tz == b"/etc/localtime" { b"Local".to_vec() } else { tz.to_vec() };
                        return z;
                    }
                } else if !tz.is_empty() && tz != b"UTC" {
                    if let Ok(z) = load_location_from(tz, PLATFORM_ZONE_SOURCES) {
                        return z;
                    }
                }
            }
        }
    }
    Location::empty(b"UTC".to_vec())
}

/// Go's `time.FixedZone`: a location that always uses the given zone name and offset
/// (seconds east of UTC).
pub fn fixed_zone(name: &[u8], offset: i64) -> Loc {
    let z = Zone { name: name.to_vec(), offset, is_dst: false };
    Rc::new(Location {
        name: name.to_vec(),
        zone: vec![z.clone()],
        tx: vec![ZoneTrans { when: ALPHA, index: 0 }],
        extend: Vec::new(),
        cache_start: ALPHA,
        cache_end: OMEGA,
        cache_zone: Some(z),
    })
}

/// Go's `time.LoadLocation`.
///
/// `""` and `"UTC"` give UTC, `"Local"` gives [`local()`]; otherwise the name is looked up in
/// `$ZONEINFO` and then the system zoneinfo directories. Errors carry Go's text, e.g.
/// `unknown time zone Foo/Bar` or `time: invalid location name`.
pub fn load_location(name: &[u8]) -> Result<Loc, String> {
    if name.is_empty() || name == b"UTC" {
        return Ok(utc());
    }
    if name == b"Local" {
        return Ok(local());
    }
    if name.windows(2).any(|w| w == b"..") || name[0] == b'/' || name[0] == b'\\' {
        // No valid IANA Time Zone name contains a single dot, much less dot dot.
        return Err("time: invalid location name".to_string());
    }
    // Like Go, $ZONEINFO is consulted once.
    let zoneinfo = ZONEINFO.with(|cell| {
        cell.get_or_init(|| {
            let env = std::env::var_os("ZONEINFO").unwrap_or_default();
            std::os::unix::ffi::OsStrExt::as_bytes(env.as_os_str()).to_vec()
        })
        .clone()
    });
    let mut first_err: Option<String> = None;
    if !zoneinfo.is_empty() {
        match load_tzinfo_from_dir_or_zip(&zoneinfo, name) {
            Ok(data) => {
                // Go discards a parse error here (it reports the nil read error instead).
                if let Ok(z) = load_location_from_tz_data(name, &data) {
                    return Ok(Rc::new(z));
                }
            }
            Err(LoadError::NotFound) => {}
            Err(LoadError::Other(e)) => first_err = Some(e),
        }
    }
    match load_location_from(name, PLATFORM_ZONE_SOURCES) {
        Ok(z) => Ok(Rc::new(z)),
        Err(e) => Err(first_err.unwrap_or(e)),
    }
}

impl Location {
    /// A location with no zones, which behaves as UTC.
    pub(crate) fn empty(name: Vec<u8>) -> Location {
        Location {
            name,
            zone: Vec::new(),
            tx: Vec::new(),
            extend: Vec::new(),
            cache_start: 0,
            cache_end: 0,
            cache_zone: None,
        }
    }

    /// Go's `Location.String`: the name given to `LoadLocation` or `FixedZone`
    /// ("UTC", "Local", "America/New_York", ...).
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// Zone information at the instant `sec` (seconds since 1970 UTC).
    pub(crate) fn lookup(&self, sec: i64) -> ZoneInfo<'_> {
        if self.zone.is_empty() {
            return ZoneInfo { name: b"UTC", offset: 0, start: ALPHA, end: OMEGA, is_dst: false };
        }
        if let Some(zone) = &self.cache_zone {
            if self.cache_start <= sec && sec < self.cache_end {
                return ZoneInfo {
                    name: &zone.name,
                    offset: zone.offset,
                    start: self.cache_start,
                    end: self.cache_end,
                    is_dst: zone.is_dst,
                };
            }
        }
        if self.tx.is_empty() || sec < self.tx[0].when {
            let zone = &self.zone[self.lookup_first_zone()];
            return ZoneInfo {
                name: &zone.name,
                offset: zone.offset,
                start: ALPHA,
                end: self.tx.first().map_or(OMEGA, |t| t.when),
                is_dst: zone.is_dst,
            };
        }
        // Binary search for the entry with the largest time <= sec.
        let tx = &self.tx;
        let mut end = OMEGA;
        let mut lo = 0usize;
        let mut hi = tx.len();
        while hi - lo > 1 {
            let m = (lo + hi) / 2;
            let lim = tx[m].when;
            if sec < lim {
                end = lim;
                hi = m;
            } else {
                lo = m;
            }
        }
        let zone = &self.zone[usize::from(tx[lo].index)];
        let start = tx[lo].when;
        if lo == tx.len() - 1 && !self.extend.is_empty() {
            if let Some(info) = tzset(&self.extend, start, sec) {
                return info;
            }
        }
        ZoneInfo { name: &zone.name, offset: zone.offset, start, end, is_dst: zone.is_dst }
    }

    /// The index of the zone used before the first transition (Go's `lookupFirstZone`).
    fn lookup_first_zone(&self) -> usize {
        // Case 1: the first zone is unused by the transitions.
        if !self.tx.iter().any(|t| t.index == 0) {
            return 0;
        }
        // Case 2: the first transition is to DST; use the closest earlier non-DST zone.
        if let Some(first) = self.tx.first() {
            if self.zone[usize::from(first.index)].is_dst {
                for zi in (0..usize::from(first.index)).rev() {
                    if !self.zone[zi].is_dst {
                        return zi;
                    }
                }
            }
        }
        // Case 3: the first non-DST zone; case 4: the first zone.
        self.zone.iter().position(|z| !z.is_dst).unwrap_or(0)
    }

    /// The offset of the zone named `name` at the pseudo-Unix time `unix` (Go's `lookupName`).
    pub(crate) fn lookup_name(&self, name: &[u8], unix: i64) -> Option<i64> {
        // First try a zone with the right name that was actually in effect at the given time.
        for zone in &self.zone {
            if zone.name == name {
                let info = self.lookup(unix.wrapping_sub(zone.offset));
                if info.name == zone.name.as_slice() {
                    return Some(info.offset);
                }
            }
        }
        // Otherwise fall back to an ordinary name match.
        self.zone.iter().find(|z| z.name == name).map(|z| z.offset)
    }
}

/// Evaluates a POSIX TZ string `s` at instant `sec`, given the last transition `last_tx_sec`
/// (Go's `tzset`). Returns `None` if the string does not parse.
pub(crate) fn tzset(s: &[u8], last_tx_sec: i64, sec: i64) -> Option<ZoneInfo<'_>> {
    let (mut std_name, s) = tzset_name(s)?;
    let (std_offset, s) = tzset_offset(s)?;
    // The TZ offsets are added to local time to get UTC; ours are the other way round.
    let mut std_offset = -std_offset;
    if s.is_empty() || s[0] == b',' {
        // No daylight saving time.
        return Some(ZoneInfo { name: std_name, offset: std_offset, start: last_tx_sec, end: OMEGA, is_dst: false });
    }
    let (mut dst_name, mut s) = tzset_name(s)?;
    let mut dst_offset;
    if s.is_empty() || s[0] == b',' {
        dst_offset = std_offset + SECONDS_PER_HOUR;
    } else {
        let (off, rest) = tzset_offset(s)?;
        dst_offset = -off;
        s = rest;
    }
    if s.is_empty() {
        // Default DST rules per tzcode.
        s = b",M3.2.0,M11.1.0";
    }
    // The TZ definition does not mention ';' here but tzcode accepts it.
    if s[0] != b',' && s[0] != b';' {
        return None;
    }
    let (start_rule, s) = tzset_rule(&s[1..])?;
    if s.first() != Some(&b',') {
        return None;
    }
    let (end_rule, s) = tzset_rule(&s[1..])?;
    if !s.is_empty() {
        return None;
    }

    let (year, yday) = AbsSeconds(sec.wrapping_add(UNIX_TO_ABSOLUTE) as u64).days().year_yday();
    // Go's truncated remainder: negative for instants before 1970.
    let ysec = (yday - 1).wrapping_mul(SECONDS_PER_DAY).wrapping_add(sec % SECONDS_PER_DAY);
    let ystart = sec.wrapping_sub(ysec);

    let mut start_sec = tzrule_time(year, &start_rule, std_offset);
    let mut end_sec = tzrule_time(year, &end_rule, dst_offset);
    let (mut dst_is_dst, mut std_is_dst) = (true, false);
    // In the southern hemisphere the labels flip.
    if end_sec < start_sec {
        std::mem::swap(&mut start_sec, &mut end_sec);
        std::mem::swap(&mut std_name, &mut dst_name);
        std::mem::swap(&mut std_offset, &mut dst_offset);
        std::mem::swap(&mut std_is_dst, &mut dst_is_dst);
    }
    // The bounds are accurate near a transition and otherwise just the year bounds.
    Some(if ysec < start_sec {
        ZoneInfo {
            name: std_name,
            offset: std_offset,
            start: ystart,
            end: start_sec.wrapping_add(ystart),
            is_dst: std_is_dst,
        }
    } else if ysec >= end_sec {
        ZoneInfo {
            name: std_name,
            offset: std_offset,
            start: end_sec.wrapping_add(ystart),
            end: ystart.wrapping_add(365 * SECONDS_PER_DAY),
            is_dst: std_is_dst,
        }
    } else {
        ZoneInfo {
            name: dst_name,
            offset: dst_offset,
            start: start_sec.wrapping_add(ystart),
            end: end_sec.wrapping_add(ystart),
            is_dst: dst_is_dst,
        }
    })
}

/// Parses a zone name at the start of a TZ string.
fn tzset_name(s: &[u8]) -> Option<(&[u8], &[u8])> {
    if s.is_empty() {
        return None;
    }
    if s[0] != b'<' {
        for (i, &c) in s.iter().enumerate() {
            if matches!(c, b'0'..=b'9' | b',' | b'-' | b'+') {
                if i < 3 {
                    return None;
                }
                return Some((&s[..i], &s[i..]));
            }
        }
        if s.len() < 3 {
            return None;
        }
        return Some((s, &[]));
    }
    let i = s.iter().position(|&c| c == b'>')?;
    Some((&s[1..i], &s[i + 1..]))
}

/// Parses a `[+-]hh[:mm[:ss]]` offset in seconds.
fn tzset_offset(s: &[u8]) -> Option<(i64, &[u8])> {
    if s.is_empty() {
        return None;
    }
    let mut s = s;
    let mut neg = false;
    if s[0] == b'+' {
        s = &s[1..];
    } else if s[0] == b'-' {
        s = &s[1..];
        neg = true;
    }
    // The tzdata code permits values up to 24 * 7 here, although POSIX does not.
    let (hours, rest) = tzset_num(s, 0, 24 * 7)?;
    s = rest;
    let mut off = hours * SECONDS_PER_HOUR;
    if s.first() == Some(&b':') {
        let (mins, rest) = tzset_num(&s[1..], 0, 59)?;
        s = rest;
        off += mins * SECONDS_PER_MINUTE;
        if s.first() == Some(&b':') {
            let (secs, rest) = tzset_num(&s[1..], 0, 59)?;
            s = rest;
            off += secs;
        }
    }
    Some((if neg { -off } else { off }, s))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    Julian,
    Doy,
    MonthWeekDay,
}

/// A transition rule read from a TZ string.
struct Rule {
    kind: RuleKind,
    day: i64,
    week: i64,
    mon: i64,
    time: i64,
}

fn tzset_rule(s: &[u8]) -> Option<(Rule, &[u8])> {
    if s.is_empty() {
        return None;
    }
    let mut r = Rule { kind: RuleKind::Doy, day: 0, week: 0, mon: 0, time: 0 };
    let mut s = s;
    if s[0] == b'J' {
        let (jday, rest) = tzset_num(&s[1..], 1, 365)?;
        s = rest;
        r.kind = RuleKind::Julian;
        r.day = jday;
    } else if s[0] == b'M' {
        let (mon, rest) = tzset_num(&s[1..], 1, 12)?;
        if rest.first() != Some(&b'.') {
            return None;
        }
        let (week, rest) = tzset_num(&rest[1..], 1, 5)?;
        if rest.first() != Some(&b'.') {
            return None;
        }
        let (day, rest) = tzset_num(&rest[1..], 0, 6)?;
        s = rest;
        r.kind = RuleKind::MonthWeekDay;
        r.day = day;
        r.week = week;
        r.mon = mon;
    } else {
        let (day, rest) = tzset_num(s, 0, 365)?;
        s = rest;
        r.kind = RuleKind::Doy;
        r.day = day;
    }
    if s.first() != Some(&b'/') {
        // 2am is the default.
        r.time = 2 * SECONDS_PER_HOUR;
        return Some((r, s));
    }
    let (offset, rest) = tzset_offset(&s[1..])?;
    r.time = offset;
    Some((r, rest))
}

/// Parses a number in `[min, max]` from a TZ string.
fn tzset_num(s: &[u8], min: i64, max: i64) -> Option<(i64, &[u8])> {
    if s.is_empty() {
        return None;
    }
    let mut num = 0i64;
    for (i, &c) in s.iter().enumerate() {
        if !c.is_ascii_digit() {
            if i == 0 || num < min {
                return None;
            }
            return Some((num, &s[i..]));
        }
        num = num * 10 + i64::from(c - b'0');
        if num > max {
            return None;
        }
    }
    if num < min {
        return None;
    }
    Some((num, &[]))
}

/// Seconds since the start of `year` at which rule `r` takes effect, for zone offset `off`.
fn tzrule_time(year: i64, r: &Rule, off: i64) -> i64 {
    let s = match r.kind {
        RuleKind::Julian => {
            let mut s = (r.day - 1) * SECONDS_PER_DAY;
            if is_leap(year) && r.day >= 60 {
                s += SECONDS_PER_DAY;
            }
            s
        }
        RuleKind::Doy => r.day * SECONDS_PER_DAY,
        RuleKind::MonthWeekDay => {
            // Zeller's Congruence.
            let m1 = (r.mon + 9) % 12 + 1;
            let mut yy0 = year;
            if r.mon <= 2 {
                yy0 = yy0.wrapping_sub(1);
            }
            let yy1 = yy0 / 100;
            let yy2 = yy0 % 100;
            let mut dow = ((26 * m1 - 2) / 10 + 1)
                .wrapping_add(yy2)
                .wrapping_add(yy2 / 4)
                .wrapping_add(yy1 / 4)
                .wrapping_sub(yy1.wrapping_mul(2))
                % 7;
            if dow < 0 {
                dow += 7;
            }
            // dow is the day of week of the first day of r.mon; find the first "r.day" day.
            let mut d = r.day - dow;
            if d < 0 {
                d += 7;
            }
            for _ in 1..r.week {
                if d + 7 >= days_in(r.mon, year) {
                    break;
                }
                d += 7;
            }
            d += days_before(r.mon);
            if is_leap(year) && r.mon > 2 {
                d += 1;
            }
            d * SECONDS_PER_DAY
        }
    };
    s + r.time - off
}

impl Location {
    /// Go's `reflect.DeepEqual` on two locations: equal names, zones and transitions.
    pub fn deep_eq(&self, other: &Location) -> bool {
        self.name == other.name
            && self.extend == other.extend
            && format!("{:?}{:?}", self.zone, self.tx) == format!("{:?}{:?}", other.zone, other.tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tzset_rules() {
        // America/Los_Angeles extend string; values checked against Go's tzset test table.
        let s = b"PST8PDT,M3.2.0,M11.1.0";
        let i = tzset(s, 0, 2159200800).unwrap();
        assert_eq!(
            (i.name, i.offset, i.start, i.end, i.is_dst),
            (&b"PDT"[..], -7 * 3600, 2152173600, 2172733200, true)
        );
        let i = tzset(s, 0, 2172733200).unwrap();
        assert_eq!(
            (i.name, i.offset, i.start, i.end, i.is_dst),
            (&b"PST"[..], -8 * 3600, 2172733200, 2177452800, false)
        );
        let i = tzset(b"<+03>-3", 0, 1).unwrap();
        assert_eq!((i.name, i.offset), (&b"+03"[..], 3 * 3600));
        assert!(tzset(b"PS", 0, 1).is_none());
        assert!(tzset(b"PST8PDT,M3.2.0", 0, 1).is_none());
    }

    #[test]
    fn fixed_and_utc() {
        let f = fixed_zone(b"ABC", 3600);
        let i = f.lookup(12345);
        assert_eq!((i.name, i.offset, i.start, i.end), (&b"ABC"[..], 3600, ALPHA, OMEGA));
        assert_eq!(utc().name(), b"UTC");
        assert!(is_utc(&utc()));
        assert!(!is_utc(&f));
    }

    #[test]
    fn load_errors() {
        assert_eq!(load_location(b"../x").unwrap_err(), "time: invalid location name");
        assert_eq!(load_location(b"/etc/x").unwrap_err(), "time: invalid location name");
        assert_eq!(load_location(b"Foo/Bar").unwrap_err(), "unknown time zone Foo/Bar");
        assert!(is_utc(&load_location(b"UTC").unwrap()));
        assert!(is_utc(&load_location(b"").unwrap()));
    }
}
