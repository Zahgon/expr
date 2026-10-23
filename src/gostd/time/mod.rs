//! Instants in time (`Time`), calendar computations, locations, formatting, parsing and durations.
//!
//! Follows the behaviour of Go's standard library time package (The Go Authors, BSD-style licence).
//!
//! A [`Time`] stores wall-clock seconds since January 1, year 1 UTC, nanoseconds and an optional
//! location (`None` meaning UTC). There is no monotonic clock reading.

mod duration;
mod format;
mod location;
mod parse;
mod zoneinfo;

pub use duration::{
    duration_abs, duration_hours, duration_microseconds, duration_milliseconds, duration_minutes, duration_round,
    duration_seconds, duration_string, duration_truncate, parse_duration, Duration, HOUR, MICROSECOND, MILLISECOND,
    MINUTE, NANOSECOND, SECOND,
};
pub use format::{
    quote, ANSIC, DATE_ONLY, DATE_TIME, KITCHEN, LAYOUT, RFC1123, RFC1123Z, RFC3339, RFC3339_NANO, RFC822, RFC822Z,
    RFC850, RUBY_DATE, STAMP, STAMP_MICRO, STAMP_MILLI, STAMP_NANO, TIME_ONLY, UNIX_DATE,
};
pub use location::{fixed_zone, load_location, local, utc, Loc, Location};
pub use parse::{parse, parse_in_location};

use duration::{less_than_half, MAX_DURATION, MIN_DURATION};
use location::{is_utc, ALPHA, OMEGA};
use std::rc::Rc;

pub(crate) const SECONDS_PER_MINUTE: i64 = 60;
pub(crate) const SECONDS_PER_HOUR: i64 = 60 * SECONDS_PER_MINUTE;
pub(crate) const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;

/// Days from March 1 to the end of the year.
const MARCH_THRU_DECEMBER: u32 = 31 + 30 + 31 + 30 + 31 + 31 + 30 + 31 + 30 + 31;
/// Years between the absolute epoch (March 1, -292277022399) and the year 0.
const ABSOLUTE_YEARS: u64 = 292_277_022_400;
/// Offset from absolute seconds to internal seconds (seconds since January 1, year 1).
const ABSOLUTE_TO_INTERNAL: i64 =
    -((ABSOLUTE_YEARS as i64 * 3_652_425 / 10_000 + MARCH_THRU_DECEMBER as i64) * SECONDS_PER_DAY);
const INTERNAL_TO_ABSOLUTE: i64 = -ABSOLUTE_TO_INTERNAL;
const UNIX_TO_INTERNAL: i64 = (1969 * 365 + 1969 / 4 - 1969 / 100 + 1969 / 400) * SECONDS_PER_DAY;
const INTERNAL_TO_UNIX: i64 = -UNIX_TO_INTERNAL;
const ABSOLUTE_TO_UNIX: i64 = ABSOLUTE_TO_INTERNAL + INTERNAL_TO_UNIX;
pub(crate) const UNIX_TO_ABSOLUTE: i64 = UNIX_TO_INTERNAL + INTERNAL_TO_ABSOLUTE;

/// Long English month names, January first.
pub(crate) const LONG_MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Long English weekday names, Sunday first.
pub(crate) const LONG_DAY_NAMES: [&str; 7] =
    ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// Go's `Month.String`: "January".."December", or `%!Month(N)` with N printed as unsigned.
pub fn month_string(m: i64) -> String {
    if (1..=12).contains(&m) {
        return LONG_MONTH_NAMES[(m - 1) as usize].to_string();
    }
    format!("%!Month({})", duration::fmt_uint(m as u64))
}

/// Go's `Weekday.String`: "Sunday".."Saturday", or `%!Weekday(N)` with N printed as unsigned.
pub fn weekday_string(d: i64) -> String {
    if (0..=6).contains(&d) {
        return LONG_DAY_NAMES[d as usize].to_string();
    }
    format!("%!Weekday({})", duration::fmt_uint(d as u64))
}

/// Seconds since the absolute epoch.
#[derive(Clone, Copy)]
pub(crate) struct AbsSeconds(pub(crate) u64);

/// Days since the absolute epoch.
#[derive(Clone, Copy)]
pub(crate) struct AbsDays(pub(crate) u64);

impl AbsSeconds {
    pub(crate) fn days(self) -> AbsDays {
        AbsDays(self.0 / SECONDS_PER_DAY as u64)
    }

    /// Hour, minute and second within the day.
    pub(crate) fn clock(self) -> (i64, i64, i64) {
        let mut sec = (self.0 % SECONDS_PER_DAY as u64) as i64;
        let hour = sec / SECONDS_PER_HOUR;
        sec -= hour * SECONDS_PER_HOUR;
        let min = sec / SECONDS_PER_MINUTE;
        sec -= min * SECONDS_PER_MINUTE;
        (hour, min, sec)
    }
}

/// Century, year within the century and day within the March-based year.
struct Split {
    century: u64,
    cyear: u64,
    ayday: u32,
}

impl Split {
    fn jan_feb(&self) -> u64 {
        u64::from(self.ayday >= MARCH_THRU_DECEMBER)
    }

    fn year(&self) -> i64 {
        (self.century.wrapping_mul(100).wrapping_sub(ABSOLUTE_YEARS) as i64)
            .wrapping_add(self.cyear as i64)
            .wrapping_add(self.jan_feb() as i64)
    }

    /// Month and day of month.
    fn month_day(&self) -> (i64, i64) {
        let d = 2141 * self.ayday + 197_913;
        let m = i64::from(d >> 16) - self.jan_feb() as i64 * 12;
        (m, 1 + i64::from((d & 0xFFFF) / 2141))
    }

    fn leap(&self) -> i64 {
        let y4ok = i64::from(self.cyear.is_multiple_of(4));
        let y100ok = i64::from(self.cyear != 0);
        let y400ok = i64::from(self.century.is_multiple_of(4));
        y4ok & (y100ok | y400ok)
    }

    fn yday(&self) -> i64 {
        let jf = self.jan_feb() as i64;
        i64::from(self.ayday) + (1 + 31 + 28) + (self.leap() & !jf) - 365 * jf
    }
}

impl AbsDays {
    fn split(self) -> Split {
        let d = 4u64.wrapping_mul(self.0).wrapping_add(3);
        let century = d / 146_097;
        // Divide by 4 at the end; the |3 keeps the multiplication exact (Neri-Schneider).
        let cd = (d % 146_097) as u32 | 3;
        let prod = 2_939_745u64 * u64::from(cd);
        let hi = prod >> 32;
        let lo = prod as u32;
        Split { century, cyear: hi, ayday: lo / 2_939_745 / 4 }
    }

    /// Year, month and day.
    pub(crate) fn date(self) -> (i64, i64, i64) {
        let s = self.split();
        let (m, d) = s.month_day();
        (s.year(), m, d)
    }

    /// Year and 1-based day of the year.
    pub(crate) fn year_yday(self) -> (i64, i64) {
        let s = self.split();
        (s.year(), s.yday())
    }

    /// Day of the week, Sunday = 0.
    pub(crate) fn weekday(self) -> i64 {
        (self.0.wrapping_add(3) % 7) as i64
    }
}

/// Days since the absolute epoch of the given date (month in 1..=12).
fn date_to_abs_days(year: i64, month: i64, day: i64) -> AbsDays {
    let mut amonth = month as u32;
    let jan_feb = u32::from(amonth < 3);
    amonth = amonth.wrapping_add(12 * jan_feb);
    let y = (year as u64).wrapping_sub(u64::from(jan_feb)).wrapping_add(ABSOLUTE_YEARS);
    let ayday = 979u32.wrapping_mul(amonth).wrapping_sub(2919) >> 5;
    let century = y / 100;
    let cyear = (y % 100) as u32;
    let cday = 1461 * cyear / 4;
    let centurydays = 146_097u64.wrapping_mul(century) / 4;
    AbsDays(centurydays.wrapping_add((i64::from(cday.wrapping_add(ayday)).wrapping_add(day).wrapping_sub(1)) as u64))
}

/// Go's `isLeap`.
pub(crate) fn is_leap(year: i64) -> bool {
    let mask = if year % 25 != 0 { 3 } else { 15 };
    year & mask == 0
}

/// Days before the first of month `m` in a non-leap year (Go's `daysBefore`).
pub(crate) fn days_before(m: i64) -> i64 {
    let adj = if m >= 3 { -2 } else { 0 };
    (214 * m - 211) / 7 + adj
}

/// Days in month `m` of `year` (Go's `daysIn`).
pub(crate) fn days_in(m: i64, year: i64) -> i64 {
    if m == 2 {
        return if is_leap(year) { 29 } else { 28 };
    }
    30 + ((m + (m >> 3)) & 1)
}

/// Go's `norm`: moves `lo` into `[0, base)` by carrying into `hi` (wrapping like Go).
fn norm(mut hi: i64, mut lo: i64, base: i64) -> (i64, i64) {
    if lo < 0 {
        let n = (lo.wrapping_neg().wrapping_sub(1)) / base + 1;
        hi = hi.wrapping_sub(n);
        lo = lo.wrapping_add(n.wrapping_mul(base));
    }
    if lo >= base {
        let n = lo / base;
        hi = hi.wrapping_add(n);
        lo = lo.wrapping_sub(n.wrapping_mul(base));
    }
    (hi, lo)
}

/// A Go `time.Time` without a monotonic clock reading.
#[derive(Clone, Debug, Default)]
pub struct Time {
    /// Seconds since January 1, year 1 00:00:00 UTC.
    sec: i64,
    /// Nanoseconds within the second, in `[0, 999999999]`.
    nsec: i32,
    /// The location; `None` means UTC.
    loc: Option<Loc>,
}

/// Go's `time.Unix`: the local Time for `sec` seconds and `nsec` nanoseconds since 1970 UTC
/// (`nsec` may be outside `[0, 999999999]`).
pub fn unix(mut sec: i64, mut nsec: i64) -> Time {
    if !(0..1_000_000_000).contains(&nsec) {
        let n = nsec / 1_000_000_000;
        sec = sec.wrapping_add(n);
        nsec -= n * 1_000_000_000;
        if nsec < 0 {
            nsec += 1_000_000_000;
            sec = sec.wrapping_sub(1);
        }
    }
    unix_time(sec, nsec as i32).with_loc(&local())
}

/// Go's `time.UnixMilli`: the local Time for `msec` milliseconds since 1970 UTC.
pub fn unix_milli(msec: i64) -> Time {
    unix(msec / 1000, (msec % 1000) * 1_000_000)
}

/// Go's `time.UnixMicro`: the local Time for `usec` microseconds since 1970 UTC.
pub fn unix_micro(usec: i64) -> Time {
    unix(usec / 1_000_000, (usec % 1_000_000) * 1000)
}

/// Go's `time.Now` in the Local location (without a monotonic reading).
pub fn now() -> Time {
    let (sec, nsec) = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos() as i64),
        Err(e) => {
            let d = e.duration();
            (-(d.as_secs() as i64), -(d.subsec_nanos() as i64))
        }
    };
    unix(sec, nsec)
}

/// A UTC Time at the given Unix second and nanosecond (Go's `unixTime` before `setLoc`).
fn unix_time(sec: i64, nsec: i32) -> Time {
    Time { sec: sec.wrapping_add(UNIX_TO_INTERNAL), nsec, loc: None }
}

impl Time {
    /// Go's `time.Date`: the Time for the given wall clock in `loc`, normalising out-of-range
    /// fields (month 13 is January of the next year, etc.). In a DST gap or overlap the result
    /// is correct in one of the two zones involved, as in Go.
    #[allow(clippy::too_many_arguments)]
    pub fn date(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64, nsec: i64, loc: &Loc) -> Time {
        // Normalize month, overflowing into year.
        let (year, m) = norm(year, month.wrapping_sub(1), 12);
        let month = m + 1;
        // Normalize nsec, sec, min, hour, overflowing into day.
        let (sec, nsec) = norm(sec, nsec, 1_000_000_000);
        let (min, sec) = norm(min, sec, 60);
        let (hour, min) = norm(hour, min, 60);
        let (day, hour) = norm(day, hour, 24);

        let mut unix = (date_to_abs_days(year, month, day).0 as i64)
            .wrapping_mul(SECONDS_PER_DAY)
            .wrapping_add(
                hour.wrapping_mul(SECONDS_PER_HOUR)
                    .wrapping_add(min.wrapping_mul(SECONDS_PER_MINUTE))
                    .wrapping_add(sec),
            )
            .wrapping_add(ABSOLUTE_TO_UNIX);

        // Look for the zone offset for the expected time, then adjust if the result is
        // outside that zone's validity range.
        let info = loc.lookup(unix);
        let (mut offset, start, end) = (info.offset, info.start, info.end);
        if offset != 0 {
            let utc = unix.wrapping_sub(offset);
            if utc < start || utc >= end {
                offset = loc.lookup(utc).offset;
            }
            unix = unix.wrapping_sub(offset);
        }
        unix_time(unix, nsec as i32).with_loc(loc)
    }

    /// Go's `setLoc`: UTC is stored as `None`.
    fn with_loc(mut self, loc: &Loc) -> Time {
        self.loc = if is_utc(loc) { None } else { Some(Rc::clone(loc)) };
        self
    }

    fn unix_sec(&self) -> i64 {
        self.sec.wrapping_add(INTERNAL_TO_UNIX)
    }

    /// Saturating second addition (Go's `addSec`).
    fn add_sec(&mut self, d: i64) {
        let sum = self.sec.wrapping_add(d);
        if (sum > self.sec) == (d > 0) {
            self.sec = sum;
        } else if d > 0 {
            self.sec = i64::MAX;
        } else {
            self.sec = -i64::MAX;
        }
    }

    fn lookup_loc(&self) -> Loc {
        self.loc.clone().unwrap_or_else(utc)
    }

    /// Zone name, offset and absolute seconds in the time's location (Go's `locabs`).
    pub(crate) fn locabs(&self) -> (Vec<u8>, i64, AbsSeconds) {
        let mut sec = self.unix_sec();
        let (name, offset) = match &self.loc {
            None => (b"UTC".to_vec(), 0),
            Some(l) => {
                let info = l.lookup(sec);
                (info.name.to_vec(), info.offset)
            }
        };
        sec = sec.wrapping_add(offset);
        (name, offset, AbsSeconds(sec.wrapping_add(UNIX_TO_ABSOLUTE) as u64))
    }

    fn abs_sec(&self) -> AbsSeconds {
        let mut sec = self.unix_sec();
        if let Some(l) = &self.loc {
            sec = sec.wrapping_add(l.lookup(sec).offset);
        }
        AbsSeconds(sec.wrapping_add(UNIX_TO_ABSOLUTE) as u64)
    }

    /// Go's `Time.Date`: year, month (1..=12) and day.
    pub fn date_parts(&self) -> (i64, i64, i64) {
        self.abs_sec().days().date()
    }

    /// Go's `Time.Year`.
    pub fn year(&self) -> i64 {
        self.abs_sec().days().split().year()
    }

    /// Go's `Time.Month` (1..=12).
    pub fn month(&self) -> i64 {
        self.abs_sec().days().split().month_day().0
    }

    /// Go's `Time.Day`.
    pub fn day(&self) -> i64 {
        self.abs_sec().days().split().month_day().1
    }

    /// Go's `Time.Weekday` (Sunday = 0).
    pub fn weekday(&self) -> i64 {
        self.abs_sec().days().weekday()
    }

    /// Go's `Time.ISOWeek`: the ISO 8601 year and week number (1..=53).
    pub fn iso_week(&self) -> (i64, i64) {
        // Weeks start on Monday; the first week of a year contains its first Thursday.
        let days = self.abs_sec().days();
        let wd = AbsDays(days.0.wrapping_sub(1)).weekday() + 1;
        let thu = AbsDays(days.0.wrapping_add((4 - wd) as u64));
        let (year, yday) = thu.year_yday();
        (year, (yday - 1) / 7 + 1)
    }

    /// Go's `Time.Clock`: hour, minute and second.
    pub fn clock(&self) -> (i64, i64, i64) {
        self.abs_sec().clock()
    }

    /// Go's `Time.Hour`.
    pub fn hour(&self) -> i64 {
        (self.abs_sec().0 % SECONDS_PER_DAY as u64) as i64 / SECONDS_PER_HOUR
    }

    /// Go's `Time.Minute`.
    pub fn minute(&self) -> i64 {
        (self.abs_sec().0 % SECONDS_PER_HOUR as u64) as i64 / SECONDS_PER_MINUTE
    }

    /// Go's `Time.Second`.
    pub fn second(&self) -> i64 {
        (self.abs_sec().0 % SECONDS_PER_MINUTE as u64) as i64
    }

    /// Go's `Time.Nanosecond`.
    pub fn nanosecond(&self) -> i64 {
        i64::from(self.nsec)
    }

    /// Go's `Time.YearDay` (1..=366).
    pub fn year_day(&self) -> i64 {
        self.abs_sec().days().year_yday().1
    }

    /// Go's `Time.Zone`: the abbreviated zone name and its offset in seconds east of UTC.
    pub fn zone(&self) -> (Vec<u8>, i64) {
        let l = self.lookup_loc();
        let info = l.lookup(self.unix_sec());
        (info.name.to_vec(), info.offset)
    }

    /// Go's `Time.ZoneBounds`: the bounds of the zone in effect; the zero Time stands for an
    /// unbounded side.
    pub fn zone_bounds(&self) -> (Time, Time) {
        let l = self.lookup_loc();
        let info = l.lookup(self.unix_sec());
        let mut start = Time::default();
        let mut end = Time::default();
        if info.start != ALPHA {
            start = unix_time(info.start, 0).with_loc(&l);
        }
        if info.end != OMEGA {
            end = unix_time(info.end, 0).with_loc(&l);
        }
        (start, end)
    }

    /// Go's `Time.IsDST`.
    pub fn is_dst(&self) -> bool {
        self.lookup_loc().lookup(self.unix_sec()).is_dst
    }

    /// Go's `Time.Location` (UTC when unset).
    pub fn location(&self) -> Loc {
        self.lookup_loc()
    }

    /// Go's `Time.Unix`: seconds since January 1, 1970 UTC.
    pub fn unix(&self) -> i64 {
        self.unix_sec()
    }

    /// Go's `Time.UnixMilli` (wraps on overflow like Go).
    pub fn unix_milli(&self) -> i64 {
        self.unix_sec().wrapping_mul(1000).wrapping_add(i64::from(self.nsec) / 1_000_000)
    }

    /// Go's `Time.UnixMicro` (wraps on overflow like Go).
    pub fn unix_micro(&self) -> i64 {
        self.unix_sec().wrapping_mul(1_000_000).wrapping_add(i64::from(self.nsec) / 1000)
    }

    /// Go's `Time.UnixNano` (wraps on overflow like Go).
    pub fn unix_nano(&self) -> i64 {
        self.unix_sec().wrapping_mul(1_000_000_000).wrapping_add(i64::from(self.nsec))
    }

    /// Go's `Time.Add`: the time plus `d`, saturating the seconds on overflow.
    pub fn add(&self, d: Duration) -> Time {
        let mut dsec = d / 1_000_000_000;
        let mut nsec = self.nsec + (d % 1_000_000_000) as i32;
        if nsec >= 1_000_000_000 {
            dsec += 1;
            nsec -= 1_000_000_000;
        } else if nsec < 0 {
            dsec -= 1;
            nsec += 1_000_000_000;
        }
        let mut t = self.clone();
        t.nsec = nsec;
        t.add_sec(dsec);
        t
    }

    /// Go's `Time.Sub`: `self - u`, saturating at the minimum or maximum Duration.
    pub fn sub(&self, u: &Time) -> Duration {
        let d = self.sec.wrapping_sub(u.sec).wrapping_mul(SECOND).wrapping_add(i64::from(self.nsec - u.nsec));
        if u.add(d).equal(self) {
            d
        } else if self.before(u) {
            MIN_DURATION
        } else {
            MAX_DURATION
        }
    }

    /// Go's `Time.AddDate`: adds years, months and days, normalising like [`Time::date`].
    pub fn add_date(&self, years: i64, months: i64, days: i64) -> Time {
        let (year, month, day) = self.date_parts();
        let (hour, min, sec) = self.clock();
        Time::date(
            year.wrapping_add(years),
            month.wrapping_add(months),
            day.wrapping_add(days),
            hour,
            min,
            sec,
            i64::from(self.nsec),
            &self.lookup_loc(),
        )
    }

    /// Go's `Time.Truncate`: rounds down to a multiple of `d` since the zero time
    /// (unchanged if `d <= 0`).
    pub fn truncate(&self, d: Duration) -> Time {
        if d <= 0 {
            return self.clone();
        }
        let (_, r) = div(self, d);
        self.add(r.wrapping_neg())
    }

    /// Go's `Time.Round`: rounds to the nearest multiple of `d` since the zero time, halfway
    /// values rounding up (unchanged if `d <= 0`).
    pub fn round(&self, d: Duration) -> Time {
        if d <= 0 {
            return self.clone();
        }
        let (_, r) = div(self, d);
        if less_than_half(r, d) {
            return self.add(r.wrapping_neg());
        }
        self.add(d.wrapping_sub(r))
    }

    /// Go's `Time.Before`.
    pub fn before(&self, u: &Time) -> bool {
        self.sec < u.sec || self.sec == u.sec && self.nsec < u.nsec
    }

    /// Go's `Time.After`.
    pub fn after(&self, u: &Time) -> bool {
        self.sec > u.sec || self.sec == u.sec && self.nsec > u.nsec
    }

    /// Go's `Time.Equal`: the same instant, regardless of location.
    pub fn equal(&self, u: &Time) -> bool {
        self.sec == u.sec && self.nsec == u.nsec
    }

    /// Go's `Time.Compare`: -1, 0 or +1.
    pub fn compare(&self, u: &Time) -> i64 {
        match (self.sec, self.nsec).cmp(&(u.sec, u.nsec)) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    /// Go's `Time.IsZero`: January 1, year 1, 00:00:00 UTC.
    pub fn is_zero(&self) -> bool {
        self.sec == 0 && self.nsec == 0
    }

    /// Go's `Time.In`: the same instant in location `loc`.
    pub fn in_location(&self, loc: &Loc) -> Time {
        self.clone().with_loc(loc)
    }

    /// Go's `Time.UTC`.
    pub fn utc(&self) -> Time {
        self.in_location(&utc())
    }

    /// Go's `Time.Local`.
    pub fn local(&self) -> Time {
        self.in_location(&local())
    }
}

/// Go's `div`: `t mod d` (and the parity of the quotient) for the time since the zero time.
fn div(t: &Time, d: Duration) -> (i64, Duration) {
    let mut neg = false;
    let mut nsec = t.nsec;
    let mut sec = t.sec;
    if sec < 0 {
        // Operate on absolute value.
        neg = true;
        sec = sec.wrapping_neg();
        nsec = -nsec;
        if nsec < 0 {
            nsec += 1_000_000_000;
            sec = sec.wrapping_sub(1);
        }
    }
    let mut qmod2;
    let mut r: Duration;
    if d < SECOND && SECOND % (d + d) == 0 {
        // Special case: 2d divides 1 second.
        qmod2 = i64::from(nsec / d as i32) & 1;
        r = Duration::from(nsec % d as i32);
    } else if d % SECOND == 0 {
        // Special case: d is a multiple of 1 second.
        let d1 = d / SECOND;
        qmod2 = (sec / d1) & 1;
        r = (sec % d1) * SECOND + Duration::from(nsec);
    } else {
        // General case: compute the 128-bit sec*1e9+nsec and divide by d.
        let sec = sec as u64;
        let mut tmp = (sec >> 32) * 1_000_000_000;
        let mut u1 = tmp >> 32;
        let mut u0 = tmp << 32;
        tmp = (sec & 0xFFFF_FFFF) * 1_000_000_000;
        let (s, c) = u0.overflowing_add(tmp);
        u0 = s;
        if c {
            u1 += 1;
        }
        let (s, c) = u0.overflowing_add(nsec as u64);
        u0 = s;
        if c {
            u1 += 1;
        }
        // Compute remainder by subtracting r<<k for decreasing k.
        let mut d1 = d as u64;
        while d1 >> 63 != 1 {
            d1 <<= 1;
        }
        let mut d0 = 0u64;
        loop {
            qmod2 = 0;
            if u1 > d1 || u1 == d1 && u0 >= d0 {
                // Subtract.
                qmod2 = 1;
                let (s, b) = u0.overflowing_sub(d0);
                u0 = s;
                if b {
                    u1 = u1.wrapping_sub(1);
                }
                u1 = u1.wrapping_sub(d1);
            }
            if d1 == 0 && d0 == d as u64 {
                break;
            }
            d0 >>= 1;
            d0 |= (d1 & 1) << 63;
            d1 >>= 1;
        }
        r = u0 as i64;
    }
    if neg && r != 0 {
        // If input was negative and not an exact multiple of d, we computed q, r such that
        // q*d + r = -t; we want q', r' such that q'*d + r' = t.
        qmod2 ^= 1;
        r = d - r;
    }
    (qmod2, r)
}

impl Time {
    /// The fields Go's `==` compares on two times (seconds, nanoseconds and the location pointer, nil for
    /// UTC); used for map keys.
    pub fn identity(&self) -> (i64, i32, usize) {
        let loc = match &self.loc {
            Some(l) if !Rc::ptr_eq(l, &utc()) => Rc::as_ptr(l) as usize,
            _ => 0,
        };
        (self.sec, self.nsec, loc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ny() -> Loc {
        load_location(b"America/New_York").unwrap()
    }

    #[test]
    fn date_normalisation() {
        let t = Time::date(2023, 13, 32, 25, 61, 61, 1_000_000_001, &utc());
        assert_eq!(t.string(), b"2024-02-02 02:02:02.000000001 +0000 UTC".to_vec());
        let t = Time::date(2020, 0, 0, 0, 0, 0, -1, &utc());
        assert_eq!(t.string(), b"2019-11-29 23:59:59.999999999 +0000 UTC".to_vec());
    }

    #[test]
    fn calendar_fields() {
        let t = Time::date(2021, 1, 3, 4, 5, 6, 7, &utc());
        assert_eq!(t.date_parts(), (2021, 1, 3));
        assert_eq!(t.clock(), (4, 5, 6));
        assert_eq!(t.weekday(), 0);
        assert_eq!(t.iso_week(), (2020, 53));
        assert_eq!(t.year_day(), 3);
        assert_eq!(t.unix(), 1_609_646_706);
        let z = Time::default();
        assert!(z.is_zero());
        assert_eq!(z.string(), b"0001-01-01 00:00:00 +0000 UTC".to_vec());
        assert_eq!(z.unix(), -62_135_596_800);
    }

    #[test]
    fn dst_gap_and_overlap() {
        let gap = Time::date(2021, 3, 14, 2, 30, 0, 0, &ny());
        assert_eq!(gap.string(), b"2021-03-14 01:30:00 -0500 EST".to_vec());
        let overlap = Time::date(2021, 11, 7, 1, 30, 0, 0, &ny());
        assert_eq!(overlap.string(), b"2021-11-07 01:30:00 -0400 EDT".to_vec());
        let (s, e) = overlap.zone_bounds();
        assert_eq!(s.string(), b"2021-03-14 03:00:00 -0400 EDT".to_vec());
        assert_eq!(e.string(), b"2021-11-07 01:00:00 -0500 EST".to_vec());
        assert!(overlap.is_dst());
    }

    #[test]
    fn arithmetic() {
        let t = Time::date(2020, 1, 31, 0, 0, 0, 0, &utc());
        assert_eq!(t.add_date(0, 1, 0).date_parts(), (2020, 3, 2));
        let max = t.add(i64::MAX).add(i64::MAX);
        let min = t.add(i64::MIN).add(i64::MIN);
        assert_eq!(max.sub(&min), i64::MAX);
        assert_eq!(min.sub(&max), i64::MIN);
        assert_eq!(t.add(HOUR).sub(&t), HOUR);
        let r = Time::date(2020, 1, 1, 12, 34, 56, 789_000_000, &utc());
        assert_eq!(r.truncate(HOUR).clock(), (12, 0, 0));
        assert_eq!(r.round(SECOND).clock(), (12, 34, 57));
        assert_eq!(r.round(7 * MINUTE).string(), b"2020-01-01 12:33:00 +0000 UTC".to_vec());
        assert_eq!(r.compare(&t), -1);
    }

    #[test]
    fn names() {
        assert_eq!(month_string(1), "January");
        assert_eq!(month_string(13), "%!Month(13)");
        assert_eq!(month_string(-1), "%!Month(18446744073709551615)");
        assert_eq!(weekday_string(6), "Saturday");
        assert_eq!(weekday_string(7), "%!Weekday(7)");
    }

    #[test]
    fn negative_years() {
        let t = Time::date(-1, 12, 31, 23, 59, 59, 999_999_999, &utc());
        let st = |t: Time| String::from_utf8(t.string()).unwrap();
        assert_eq!(st(t.truncate(HOUR)), "-0001-12-31 23:00:00 +0000 UTC");
        assert_eq!(st(t.round(HOUR)), "0000-01-01 00:00:00 +0000 UTC");
        assert_eq!(st(t.round(7 * SECOND)), "-0001-12-31 23:59:58 +0000 UTC");
        assert_eq!(st(t.truncate(1500 * MILLISECOND)), "-0001-12-31 23:59:58.5 +0000 UTC");
        assert_eq!((t.unix_milli(), t.unix_micro()), (-62_167_219_200_001, -62_167_219_200_000_001));
        assert_eq!((t.year_day(), t.weekday(), t.iso_week()), (365, 5, (-1, 52)));
    }

    #[test]
    fn dates_and_bounds() {
        let st = |t: Time| String::from_utf8(t.string()).unwrap();
        let leap = Time::date(2020, 2, 29, 0, 0, 0, 0, &utc());
        assert_eq!(st(leap.add_date(1, 0, 0)), "2021-03-01 00:00:00 +0000 UTC");
        let jan = Time::date(2020, 1, 31, 0, 0, 0, 0, &utc());
        assert_eq!(st(jan.add_date(0, -2, 0)), "2019-12-01 00:00:00 +0000 UTC");
        assert_eq!(st(unix(-1, -1).utc()), "1969-12-31 23:59:58.999999999 +0000 UTC");
        let syd = load_location(b"Australia/Sydney").unwrap();
        let w = Time::date(2021, 6, 1, 0, 0, 0, 0, &syd);
        assert_eq!(w.zone(), (b"AEST".to_vec(), 36000));
        let (a, b) = w.zone_bounds();
        assert_eq!((st(a), st(b)), ("2021-04-04 02:00:00 +1000 AEST".into(), "2021-10-03 03:00:00 +1100 AEDT".into()));
        // After the last transition the TZ footer rule gives year-start bounds.
        let (a, b) = Time::date(2050, 1, 1, 0, 0, 0, 0, &ny()).zone_bounds();
        assert_eq!((st(a), st(b)), ("2049-12-31 19:00:00 -0500 EST".into(), "2050-03-13 03:00:00 -0400 EDT".into()));
    }
}
