//! Reading TZif zoneinfo files from directories and uncompressed zip archives.
//!
//! Follows the behaviour of Go's standard library time package (The Go Authors, BSD-style licence).

use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::ffi::OsStrExt;

use super::location::{tzset, Location, Zone, ZoneTrans, ALPHA, OMEGA};

/// The system zoneinfo directories searched on Unix (Go's `platformZoneSources`).
pub(crate) const PLATFORM_ZONE_SOURCES: &[&[u8]] =
    &[b"/usr/share/zoneinfo/", b"/usr/share/lib/zoneinfo/", b"/usr/lib/locale/TZ/", b"/etc/zoneinfo"];

/// Maximum size of a zoneinfo file accepted by Go's `readFile`.
const MAX_FILE_SIZE: u64 = 10 << 20;

const ERR_BAD_DATA: &str = "malformed time zone information";

/// A failure to read zone data: "not found" (ENOENT) is skipped silently by the callers.
pub(crate) enum LoadError {
    NotFound,
    Other(String),
}

/// Go's `syscall.Errno.Error` text for the errors a zoneinfo lookup can plausibly meet.
fn errno_text(e: &std::io::Error) -> LoadError {
    let text = match e.raw_os_error() {
        Some(2) => return LoadError::NotFound,
        Some(1) => "operation not permitted",
        Some(5) => "input/output error",
        Some(13) => "permission denied",
        Some(20) => "not a directory",
        Some(21) => "is a directory",
        Some(22) => "invalid argument",
        Some(24) => "too many open files",
        Some(36) => "file name too long",
        Some(40) => "too many levels of symbolic links",
        Some(_) => return LoadError::Other(e.to_string()),
        // Rust rejects paths with NUL bytes itself; Go's syscall layer reports EINVAL.
        None => "invalid argument",
    };
    LoadError::Other(text.to_string())
}

/// Go's `readFile`: the whole file, at most 10 MiB.
fn read_file(name: &[u8]) -> Result<Vec<u8>, LoadError> {
    let f = std::fs::File::open(OsStr::from_bytes(name)).map_err(|e| errno_text(&e))?;
    let mut ret = Vec::new();
    f.take(MAX_FILE_SIZE + 1).read_to_end(&mut ret).map_err(|e| errno_text(&e))?;
    if ret.len() as u64 > MAX_FILE_SIZE {
        return Err(LoadError::Other(format!("time: file {} is too large", String::from_utf8_lossy(name))));
    }
    Ok(ret)
}

/// The contents of `name` inside `dir`, which is a directory or an uncompressed zip file.
pub(crate) fn load_tzinfo_from_dir_or_zip(dir: &[u8], name: &[u8]) -> Result<Vec<u8>, LoadError> {
    if dir.len() > 4 && dir.ends_with(b".zip") {
        return load_tzinfo_from_zip(dir, name);
    }
    if dir.is_empty() {
        return read_file(name);
    }
    let mut path = dir.to_vec();
    path.push(b'/');
    path.extend_from_slice(name);
    read_file(&path)
}

fn get4(b: &[u8]) -> usize {
    if b.len() < 4 {
        return 0;
    }
    usize::from(b[0]) | usize::from(b[1]) << 8 | usize::from(b[2]) << 16 | usize::from(b[3]) << 24
}

fn get2(b: &[u8]) -> usize {
    if b.len() < 2 {
        return 0;
    }
    usize::from(b[0]) | usize::from(b[1]) << 8
}

/// Reads exactly `buf.len()` bytes at `off` (negative: from the end), like Go's `preadn`.
fn preadn(f: &mut std::fs::File, buf: &mut [u8], off: i64) -> bool {
    let pos = if off < 0 { SeekFrom::End(off) } else { SeekFrom::Start(off as u64) };
    f.seek(pos).is_ok() && f.read_exact(buf).is_ok()
}

/// Go's `loadTzinfoFromZip`: a stored (uncompressed) member of a zip archive.
fn load_tzinfo_from_zip(zipfile: &[u8], name: &[u8]) -> Result<Vec<u8>, LoadError> {
    const ZECHEADER: usize = 0x0605_4b50;
    const ZCHEADER: usize = 0x0201_4b50;
    const ZTAILSIZE: usize = 22;
    const ZHEADERSIZE: usize = 30;
    const ZHEADER: usize = 0x0403_4b50;
    let zname_str = String::from_utf8_lossy(zipfile).into_owned();
    let corrupt = || LoadError::Other(format!("corrupt zip file {zname_str}"));

    let mut fd = std::fs::File::open(OsStr::from_bytes(zipfile)).map_err(|e| errno_text(&e))?;
    let mut tail = [0u8; ZTAILSIZE];
    if !preadn(&mut fd, &mut tail, -(ZTAILSIZE as i64)) || get4(&tail) != ZECHEADER {
        return Err(corrupt());
    }
    let n = get2(&tail[10..]);
    let size = get4(&tail[12..]);
    let off = get4(&tail[16..]);
    let mut dir = vec![0u8; size];
    if !preadn(&mut fd, &mut dir, off as i64) {
        return Err(corrupt());
    }
    let mut buf: &[u8] = &dir;
    for _ in 0..n {
        if get4(buf) != ZCHEADER {
            break;
        }
        let meth = get2(buf.get(10..).unwrap_or_default());
        let size = get4(buf.get(24..).unwrap_or_default());
        let namelen = get2(buf.get(28..).unwrap_or_default());
        let xlen = get2(buf.get(30..).unwrap_or_default());
        let fclen = get2(buf.get(32..).unwrap_or_default());
        let off = get4(buf.get(42..).unwrap_or_default());
        let zname = buf.get(46..46 + namelen).ok_or_else(corrupt)?;
        let matched = zname == name;
        buf = buf.get(46 + namelen + xlen + fclen..).ok_or_else(corrupt)?;
        if !matched {
            continue;
        }
        if meth != 0 {
            return Err(LoadError::Other(format!(
                "unsupported compression for {} in {}",
                String::from_utf8_lossy(name),
                zname_str
            )));
        }
        let mut hdr = vec![0u8; ZHEADERSIZE + namelen];
        if !preadn(&mut fd, &mut hdr, off as i64)
            || get4(&hdr) != ZHEADER
            || get2(&hdr[8..]) != meth
            || get2(&hdr[26..]) != namelen
            || &hdr[30..30 + namelen] != name
        {
            return Err(corrupt());
        }
        let xlen = get2(&hdr[28..]);
        let mut data = vec![0u8; size];
        if !preadn(&mut fd, &mut data, (off + 30 + namelen + xlen) as i64) {
            return Err(corrupt());
        }
        return Ok(data);
    }
    Err(LoadError::NotFound)
}

/// Go's `loadLocation`: the first source holding valid data for `name`, then
/// `$GOROOT/lib/time/zoneinfo.zip` when the `GOROOT` environment variable is set.
pub(crate) fn load_location_from(name: &[u8], sources: &[&[u8]]) -> Result<Location, String> {
    let mut first_err: Option<String> = None;
    let try_source = |source: &[u8], first_err: &mut Option<String>| -> Option<Location> {
        let err = match load_tzinfo_from_dir_or_zip(source, name) {
            Ok(data) => match load_location_from_tz_data(name, &data) {
                Ok(z) => return Some(z),
                Err(e) => e,
            },
            Err(LoadError::NotFound) => return None,
            Err(LoadError::Other(e)) => e,
        };
        if first_err.is_none() {
            *first_err = Some(err);
        }
        None
    };
    for source in sources {
        if let Some(z) = try_source(source, &mut first_err) {
            return Ok(z);
        }
    }
    if let Some(goroot) = std::env::var_os("GOROOT").filter(|g| !g.is_empty()) {
        let mut zip = goroot.as_bytes().to_vec();
        zip.extend_from_slice(b"/lib/time/zoneinfo.zip");
        if let Some(z) = try_source(&zip, &mut first_err) {
            return Ok(z);
        }
    }
    Err(first_err.unwrap_or_else(|| format!("unknown time zone {}", String::from_utf8_lossy(name))))
}

/// A cursor over a byte slice that latches an error flag on short reads (Go's `dataIO`).
struct DataIo<'a> {
    p: &'a [u8],
    error: bool,
}

impl<'a> DataIo<'a> {
    fn read(&mut self, n: usize) -> &'a [u8] {
        if self.p.len() < n {
            self.p = &[];
            self.error = true;
            return &[];
        }
        let (a, b) = self.p.split_at(n);
        self.p = b;
        a
    }

    fn big4(&mut self) -> Option<u32> {
        let p = self.read(4);
        if p.len() < 4 {
            self.error = true;
            return None;
        }
        Some(u32::from_be_bytes([p[0], p[1], p[2], p[3]]))
    }

    fn big8(&mut self) -> Option<u64> {
        let n1 = self.big4();
        let n2 = self.big4();
        match (n1, n2) {
            (Some(a), Some(b)) => Some(u64::from(a) << 32 | u64::from(b)),
            _ => {
                self.error = true;
                None
            }
        }
    }

    fn byte(&mut self) -> Option<u8> {
        let p = self.read(1);
        p.first().copied().or_else(|| {
            self.error = true;
            None
        })
    }
}

/// The current Unix time in seconds, used to prime the location cache.
fn now_unix() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// Go's `LoadLocationFromTZData`: parses TZif (v1, v2, v3) data into a location named `name`.
pub(crate) fn load_location_from_tz_data(name: &[u8], data: &[u8]) -> Result<Location, String> {
    let bad = || ERR_BAD_DATA.to_string();
    let mut d = DataIo { p: data, error: false };
    if d.read(4) != b"TZif" {
        return Err(bad());
    }
    let p = d.read(16);
    if p.len() != 16 {
        return Err(bad());
    }
    let version = match p[0] {
        0 => 1,
        b'2' => 2,
        b'3' => 3,
        _ => return Err(bad()),
    };
    // Counts: UTC/local indicators, std/wall indicators, leap seconds, transition times,
    // local time zones, abbreviation characters.
    const NUTCLOCAL: usize = 0;
    const NSTDWALL: usize = 1;
    const NLEAP: usize = 2;
    const NTIME: usize = 3;
    const NZONE: usize = 4;
    const NCHAR: usize = 5;
    let mut n = [0usize; 6];
    for v in n.iter_mut() {
        *v = d.big4().ok_or_else(bad)? as usize;
    }
    let mut is64 = false;
    if version > 1 {
        // Skip the 32-bit data and the second header, then read the 64-bit counts.
        let skip =
            n[NTIME] * 4 + n[NTIME] + n[NZONE] * 6 + n[NCHAR] + n[NLEAP] * 8 + n[NSTDWALL] + n[NUTCLOCAL] + 4 + 16;
        d.read(skip);
        is64 = true;
        for v in n.iter_mut() {
            *v = d.big4().ok_or_else(bad)? as usize;
        }
    }
    let size = if is64 { 8 } else { 4 };
    let mut txtimes = DataIo { p: d.read(n[NTIME] * size), error: false };
    let txzones = d.read(n[NTIME]);
    let mut zonedata = DataIo { p: d.read(n[NZONE] * 6), error: false };
    let abbrev = d.read(n[NCHAR]);
    d.read(n[NLEAP] * (size + 4));
    d.read(n[NSTDWALL]);
    d.read(n[NUTCLOCAL]);
    if d.error {
        return Err(bad());
    }
    let rest = std::mem::take(&mut d.p);
    let extend = if rest.len() > 2 && rest[0] == b'\n' && rest[rest.len() - 1] == b'\n' {
        rest[1..rest.len() - 1].to_vec()
    } else {
        Vec::new()
    };

    let nzone = n[NZONE];
    if nzone == 0 {
        return Err(bad());
    }
    let mut zones = Vec::with_capacity(nzone);
    for _ in 0..nzone {
        let off = zonedata.big4().ok_or_else(bad)?;
        let isdst = zonedata.byte().ok_or_else(bad)?;
        let b = usize::from(zonedata.byte().ok_or_else(bad)?);
        if b >= abbrev.len() {
            return Err(bad());
        }
        let ab = &abbrev[b..];
        let end = ab.iter().position(|&c| c == 0).unwrap_or(ab.len());
        zones.push(Zone { name: ab[..end].to_vec(), offset: i64::from(off as i32), is_dst: isdst != 0 });
    }
    let mut tx = Vec::with_capacity(n[NTIME]);
    for &zi in txzones.iter().take(n[NTIME]) {
        let when = if is64 {
            txtimes.big8().ok_or_else(bad)? as i64
        } else {
            i64::from(txtimes.big4().ok_or_else(bad)? as i32)
        };
        if usize::from(zi) >= zones.len() {
            return Err(bad());
        }
        tx.push(ZoneTrans { when, index: zi });
    }
    if tx.is_empty() {
        // Build a fake transition to cover all time (fixed locations like "Etc/GMT0").
        tx.push(ZoneTrans { when: ALPHA, index: 0 });
    }
    let mut l =
        Location { name: name.to_vec(), zone: zones, tx, extend, cache_start: 0, cache_end: 0, cache_zone: None };
    fill_cache(&mut l, now_unix());
    Ok(l)
}

/// Fills the location's one-element cache with the zone in effect at `sec`.
fn fill_cache(l: &mut Location, sec: i64) {
    let tx = &l.tx;
    for i in 0..tx.len() {
        if tx[i].when <= sec && (i + 1 == tx.len() || sec < tx[i + 1].when) {
            let mut start = tx[i].when;
            let mut end = OMEGA;
            let mut zone = l.zone[usize::from(tx[i].index)].clone();
            if i + 1 < tx.len() {
                end = tx[i + 1].when;
            } else if !l.extend.is_empty() {
                if let Some(info) = tzset(&l.extend, start, sec) {
                    start = info.start;
                    end = info.end;
                    zone = Zone { name: info.name.to_vec(), offset: info.offset, is_dst: info.is_dst };
                }
            }
            l.cache_start = start;
            l.cache_end = end;
            l.cache_zone = Some(zone);
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_data() {
        assert_eq!(load_location_from_tz_data(b"x", b"TZif").unwrap_err(), ERR_BAD_DATA);
        assert_eq!(load_location_from_tz_data(b"x", b"nope").unwrap_err(), ERR_BAD_DATA);
    }

    #[test]
    fn loads_system_zone() {
        let l = load_location_from(b"America/New_York", PLATFORM_ZONE_SOURCES).unwrap();
        assert_eq!(l.name(), b"America/New_York");
        let i = l.lookup(1_700_000_000);
        assert_eq!((i.name, i.offset), (&b"EST"[..], -5 * 3600));
        let i = l.lookup(4_000_000_000);
        assert_eq!((i.name, i.offset, i.is_dst), (&b"EDT"[..], -4 * 3600, true));
    }

    #[test]
    fn directory_error() {
        assert_eq!(load_location_from(b"America", PLATFORM_ZONE_SOURCES).unwrap_err(), "is a directory");
        assert_eq!(load_location_from(b"zone.tab", PLATFORM_ZONE_SOURCES).unwrap_err(), ERR_BAD_DATA);
    }
}
