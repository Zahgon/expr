//! Byte-string manipulation: case mapping, trimming, splitting, searching and replacing.
//!
//! Follows the behaviour of Go's standard library strings package (The Go Authors, BSD-style licence).
//! Strings are byte slices that may hold invalid UTF-8; functions that iterate over runes
//! treat each invalid byte as U+FFFD, as Go's `range` over a string does. Byte offsets are
//! returned as `i64`, with -1 meaning "not found".

use super::unicode;
use super::utf8;

/// Decodes the rune at the start of `s` the way Go's `range` loop does.
fn next_rune(s: &[u8]) -> (u32, usize) {
    match s.first() {
        Some(&b) if b < 0x80 => (u32::from(b), 1),
        _ => utf8::decode_rune(s),
    }
}

/// Go's `strings.Map` for mappings that never drop runes.
fn map_runes(s: &[u8], mapping: fn(u32) -> u32) -> Vec<u8> {
    // Go returns the input unchanged unless some rune changes or an invalid byte is seen;
    // once a change happens, every rune is re-encoded, so invalid bytes become U+FFFD.
    let mut i = 0;
    let mut changed = false;
    while i < s.len() {
        let (c, width) = next_rune(&s[i..]);
        if mapping(c) != c || (c == utf8::RUNE_ERROR && width == 1) {
            changed = true;
            break;
        }
        i += width;
    }
    if !changed {
        return s.to_vec();
    }
    let mut out = Vec::with_capacity(s.len() + utf8::UTF_MAX);
    out.extend_from_slice(&s[..i]);
    while i < s.len() {
        let (c, width) = next_rune(&s[i..]);
        utf8::encode_rune(mapping(c), &mut out);
        i += width;
    }
    out
}

/// Returns `s` with all Unicode letters mapped to their upper case (`unicode::to_upper`).
/// Invalid UTF-8 bytes become U+FFFD unless the input is returned unchanged.
pub fn to_upper(s: &[u8]) -> Vec<u8> {
    if s.is_ascii() {
        return s.to_ascii_uppercase();
    }
    map_runes(s, unicode::to_upper)
}

/// Returns `s` with all Unicode letters mapped to their lower case (`unicode::to_lower`).
/// Invalid UTF-8 bytes become U+FFFD unless the input is returned unchanged.
pub fn to_lower(s: &[u8]) -> Vec<u8> {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    map_runes(s, unicode::to_lower)
}

/// Returns `s` with all Unicode letters mapped to their title case (`unicode::to_title`).
pub fn to_title(s: &[u8]) -> Vec<u8> {
    map_runes(s, unicode::to_title)
}

fn index_func(s: &[u8], f: fn(u32) -> bool, truth: bool) -> Option<usize> {
    let mut i = 0;
    while i < s.len() {
        let (r, width) = next_rune(&s[i..]);
        if f(r) == truth {
            return Some(i);
        }
        i += width;
    }
    None
}

fn last_index_func(s: &[u8], f: fn(u32) -> bool, truth: bool) -> Option<usize> {
    let mut i = s.len();
    while i > 0 {
        let (r, size) = utf8::decode_last_rune(&s[..i]);
        i -= size;
        if f(r) == truth {
            return Some(i);
        }
    }
    None
}

fn trim_left_func(s: &[u8], f: fn(u32) -> bool) -> &[u8] {
    match index_func(s, f, false) {
        Some(i) => &s[i..],
        None => &[],
    }
}

fn trim_right_func(s: &[u8], f: fn(u32) -> bool) -> &[u8] {
    match last_index_func(s, f, false) {
        Some(i) if s[i] >= 0x80 => &s[..i + utf8::decode_rune(&s[i..]).1],
        Some(i) => &s[..i + 1],
        None => &[],
    }
}

fn is_ascii_space(c: u8) -> bool {
    matches!(c, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ')
}

/// Returns `s` without leading and trailing white space as defined by Unicode
/// (`unicode::is_space`).
pub fn trim_space(s: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < s.len() {
        let c = s[start];
        if c >= 0x80 {
            // Some Unicode: fall back to the rune-wise (slower) path.
            let rest = trim_left_func(&s[start..], unicode::is_space);
            return trim_right_func(rest, unicode::is_space);
        }
        if !is_ascii_space(c) {
            break;
        }
        start += 1;
    }
    let mut stop = s.len();
    while stop > start {
        let c = s[stop - 1];
        if c >= 0x80 {
            return trim_right_func(&s[start..stop], unicode::is_space);
        }
        if !is_ascii_space(c) {
            break;
        }
        stop -= 1;
    }
    &s[start..stop]
}

/// How a cutset is matched: as a set of ASCII bytes, or rune-wise.
enum Cutset<'a> {
    Ascii([bool; 128]),
    Runes(&'a [u8]),
}

impl<'a> Cutset<'a> {
    fn new(cutset: &'a [u8]) -> Cutset<'a> {
        if cutset.is_ascii() {
            let mut set = [false; 128];
            for &c in cutset {
                set[usize::from(c)] = true;
            }
            Cutset::Ascii(set)
        } else {
            Cutset::Runes(cutset)
        }
    }

    fn trim_left<'s>(&self, mut s: &'s [u8]) -> &'s [u8] {
        match self {
            Cutset::Ascii(set) => {
                while let Some((&c, rest)) = s.split_first() {
                    if c >= 0x80 || !set[usize::from(c)] {
                        break;
                    }
                    s = rest;
                }
            }
            Cutset::Runes(cutset) => {
                while !s.is_empty() {
                    let (r, n) = next_rune(s);
                    if !contains_rune(cutset, r) {
                        break;
                    }
                    s = &s[n..];
                }
            }
        }
        s
    }

    fn trim_right<'s>(&self, mut s: &'s [u8]) -> &'s [u8] {
        match self {
            Cutset::Ascii(set) => {
                while let Some((&c, rest)) = s.split_last() {
                    if c >= 0x80 || !set[usize::from(c)] {
                        break;
                    }
                    s = rest;
                }
            }
            Cutset::Runes(cutset) => {
                while let Some(&last) = s.last() {
                    let (r, n) = if last < 0x80 { (u32::from(last), 1) } else { utf8::decode_last_rune(s) };
                    if !contains_rune(cutset, r) {
                        break;
                    }
                    s = &s[..s.len() - n];
                }
            }
        }
        s
    }
}

/// Reports whether the rune occurs in `s` (Go's `strings.ContainsRune`); U+FFFD matches
/// invalid UTF-8 as well as an encoded U+FFFD.
fn contains_rune(s: &[u8], r: u32) -> bool {
    let mut i = 0;
    while i < s.len() {
        let (c, width) = next_rune(&s[i..]);
        if c == r {
            return true;
        }
        i += width;
    }
    false
}

/// Returns `s` with all leading and trailing runes contained in `cutset` removed.
pub fn trim<'s>(s: &'s [u8], cutset: &[u8]) -> &'s [u8] {
    if s.is_empty() || cutset.is_empty() {
        return s;
    }
    let set = Cutset::new(cutset);
    set.trim_left(set.trim_right(s))
}

/// Returns `s` with all leading runes contained in `cutset` removed.
pub fn trim_left<'s>(s: &'s [u8], cutset: &[u8]) -> &'s [u8] {
    if s.is_empty() || cutset.is_empty() {
        return s;
    }
    Cutset::new(cutset).trim_left(s)
}

/// Returns `s` with all trailing runes contained in `cutset` removed.
pub fn trim_right<'s>(s: &'s [u8], cutset: &[u8]) -> &'s [u8] {
    if s.is_empty() || cutset.is_empty() {
        return s;
    }
    Cutset::new(cutset).trim_right(s)
}

/// Returns `s` without the leading `prefix`; `s` is returned unchanged if it does not start
/// with `prefix`.
pub fn trim_prefix<'s>(s: &'s [u8], prefix: &[u8]) -> &'s [u8] {
    s.strip_prefix(prefix).unwrap_or(s)
}

/// Returns `s` without the trailing `suffix`; `s` is returned unchanged if it does not end
/// with `suffix`.
pub fn trim_suffix<'s>(s: &'s [u8], suffix: &[u8]) -> &'s [u8] {
    s.strip_suffix(suffix).unwrap_or(s)
}

fn find(s: &[u8], sep: &[u8]) -> Option<usize> {
    match sep.len() {
        0 => Some(0),
        1 => s.iter().position(|&c| c == sep[0]),
        n if n > s.len() => None,
        n => s.windows(n).position(|w| w == sep),
    }
}

fn rfind(s: &[u8], sep: &[u8]) -> Option<usize> {
    match sep.len() {
        0 => Some(s.len()),
        1 => s.iter().rposition(|&c| c == sep[0]),
        n if n > s.len() => None,
        n => s.windows(n).rposition(|w| w == sep),
    }
}

/// Returns the byte index of the first instance of `substr` in `s`, or -1 if absent.
/// An empty `substr` is found at index 0.
pub fn index(s: &[u8], substr: &[u8]) -> i64 {
    find(s, substr).map_or(-1, |i| i as i64)
}

/// Returns the byte index of the last instance of `substr` in `s`, or -1 if absent.
/// An empty `substr` is found at index `s.len()`.
pub fn last_index(s: &[u8], substr: &[u8]) -> i64 {
    rfind(s, substr).map_or(-1, |i| i as i64)
}

/// Reports whether `substr` is within `s`.
pub fn contains(s: &[u8], substr: &[u8]) -> bool {
    find(s, substr).is_some()
}

/// Reports whether `s` begins with `prefix`.
pub fn has_prefix(s: &[u8], prefix: &[u8]) -> bool {
    s.starts_with(prefix)
}

/// Reports whether `s` ends with `suffix`.
pub fn has_suffix(s: &[u8], suffix: &[u8]) -> bool {
    s.ends_with(suffix)
}

/// Counts the non-overlapping instances of `substr` in `s`; an empty `substr` counts
/// one more than the number of runes in `s`.
pub fn count(s: &[u8], substr: &[u8]) -> usize {
    if substr.is_empty() {
        return utf8::rune_count(s) + 1;
    }
    let mut n = 0;
    let mut s = s;
    while let Some(i) = find(s, substr) {
        n += 1;
        s = &s[i + substr.len()..];
    }
    n
}

/// Splits `s` into its UTF-8 sequences (one per rune, one per invalid byte), at most `n`
/// pieces when `n` is positive; the last piece holds the unsplit remainder.
fn explode(s: &[u8], n: i64) -> Vec<&[u8]> {
    let l = utf8::rune_count(s);
    let n = if n < 0 || n as u64 > l as u64 { l } else { n as usize };
    let mut a = Vec::with_capacity(n);
    let mut s = s;
    for _ in 1..n {
        let (_, size) = utf8::decode_rune(s);
        a.push(&s[..size]);
        s = &s[size..];
    }
    if n > 0 {
        a.push(s);
    }
    a
}

fn gen_split<'s>(s: &'s [u8], sep: &[u8], sep_save: usize, n: i64) -> Vec<&'s [u8]> {
    if n == 0 {
        return Vec::new();
    }
    if sep.is_empty() {
        return explode(s, n);
    }
    let mut n = if n < 0 { count(s, sep) as u64 + 1 } else { n as u64 };
    n = n.min(s.len() as u64 + 1);
    let mut a = Vec::new();
    let mut s = s;
    while (a.len() as u64) < n - 1 {
        let Some(m) = find(s, sep) else {
            break;
        };
        a.push(&s[..m + sep_save]);
        s = &s[m + sep.len()..];
    }
    a.push(s);
    a
}

/// Slices `s` into all substrings separated by `sep`. An empty `sep` splits after each
/// UTF-8 sequence (each invalid byte on its own).
pub fn split<'s>(s: &'s [u8], sep: &[u8]) -> Vec<&'s [u8]> {
    gen_split(s, sep, 0, -1)
}

/// Like [`split`] but returns at most `n` substrings when `n > 0` (the last one is the
/// unsplit remainder), none when `n == 0` and all of them when `n < 0`.
pub fn split_n<'s>(s: &'s [u8], sep: &[u8], n: i64) -> Vec<&'s [u8]> {
    gen_split(s, sep, 0, n)
}

/// Slices `s` into all substrings after each instance of `sep` (the separators are kept).
pub fn split_after<'s>(s: &'s [u8], sep: &[u8]) -> Vec<&'s [u8]> {
    gen_split(s, sep, sep.len(), -1)
}

/// Like [`split_after`] with the count rules of [`split_n`].
pub fn split_after_n<'s>(s: &'s [u8], sep: &[u8], n: i64) -> Vec<&'s [u8]> {
    gen_split(s, sep, sep.len(), n)
}

/// Returns a copy of `s` with the first `n` non-overlapping instances of `old` replaced by
/// `new` (all of them if `n < 0`). An empty `old` matches at the start and after each
/// UTF-8 sequence.
pub fn replace(s: &[u8], old: &[u8], new: &[u8], n: i64) -> Vec<u8> {
    if old == new || n == 0 {
        return s.to_vec();
    }
    let m = count(s, old) as u64;
    if m == 0 {
        return s.to_vec();
    }
    let n = if n < 0 || m < n as u64 { m } else { n as u64 };
    let mut b = Vec::with_capacity(s.len());
    let mut start = 0;
    if !old.is_empty() {
        for _ in 0..n {
            let j = start + find(&s[start..], old).unwrap_or(0);
            b.extend_from_slice(&s[start..j]);
            b.extend_from_slice(new);
            start = j + old.len();
        }
    } else {
        b.extend_from_slice(new);
        for _ in 1..n {
            let (_, wid) = utf8::decode_rune(&s[start..]);
            let j = start + wid;
            b.extend_from_slice(&s[start..j]);
            b.extend_from_slice(new);
            start = j;
        }
    }
    b.extend_from_slice(&s[start..]);
    b
}

/// Returns a copy of `s` with all non-overlapping instances of `old` replaced by `new`.
pub fn replace_all(s: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    replace(s, old, new, -1)
}

/// Returns `count` copies of `s`. Go panics on a negative count or an overflowing result;
/// callers check that before calling.
pub fn repeat(s: &[u8], count: usize) -> Vec<u8> {
    s.repeat(count)
}

/// Reports whether `s` and `t` are equal under simple Unicode case folding; invalid UTF-8
/// bytes compare as U+FFFD.
pub fn equal_fold(s: &[u8], t: &[u8]) -> bool {
    // ASCII fast path.
    let mut i = 0;
    let n = s.len().min(t.len());
    while i < n {
        let (sr, tr) = (s[i], t[i]);
        if (sr | tr) >= 0x80 {
            break;
        }
        if !sr.eq_ignore_ascii_case(&tr) {
            return false;
        }
        i += 1;
    }
    if i == n {
        return s.len() == t.len();
    }

    let (mut s, mut t) = (&s[i..], &t[i..]);
    while !s.is_empty() {
        if t.is_empty() {
            return false;
        }
        let (mut sr, ssize) = next_rune(s);
        let (mut tr, tsize) = next_rune(t);
        s = &s[ssize..];
        t = &t[tsize..];
        if tr == sr {
            continue;
        }
        // Make sr < tr to simplify what follows.
        if tr < sr {
            std::mem::swap(&mut tr, &mut sr);
        }
        if tr < utf8::RUNE_SELF {
            // ASCII only, sr/tr must be upper/lower case.
            if (u32::from(b'A')..=u32::from(b'Z')).contains(&sr) && tr == sr + 32 {
                continue;
            }
            return false;
        }
        // General case: walk sr's fold orbit looking for tr.
        let mut r = unicode::simple_fold(sr);
        while r != sr && r < tr {
            r = unicode::simple_fold(r);
        }
        if r != tr {
            return false;
        }
    }
    t.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_mapping() {
        assert_eq!(to_upper(b"hello, world"), b"HELLO, WORLD");
        assert_eq!(to_lower(b"HeLLo"), b"hello");
        assert_eq!(to_upper("straße".as_bytes()), "STRAßE".as_bytes());
        assert_eq!(to_upper("ǆ".as_bytes()), "Ǆ".as_bytes());
        assert_eq!(to_lower("ǅ".as_bytes()), "ǆ".as_bytes());
        assert_eq!(to_title("ǆ".as_bytes()), "ǅ".as_bytes());
        assert_eq!(to_lower("İ".as_bytes()), b"i");
        assert_eq!(to_upper("ı".as_bytes()), b"I");
        assert_eq!(to_upper(b"a\xffb\xc3\xa9"), "A\u{FFFD}BÉ".as_bytes());
        assert_eq!(to_upper(b"\xff\xc3\x89"), "\u{FFFD}É".as_bytes());
        assert_eq!(to_upper("É\u{FFFD}".as_bytes()), "É\u{FFFD}".as_bytes());
        assert_eq!(to_lower(b"\xffA"), "\u{FFFD}a".as_bytes());
    }

    #[test]
    fn trimming() {
        assert_eq!(trim_space(b" \t\n hi \r\n"), b"hi");
        assert_eq!(trim_space("\u{3000}hi\u{85}".as_bytes()), b"hi");
        assert_eq!(trim_space(b"\xff "), b"\xff");
        assert_eq!(trim(b"xxhixx", b"x"), b"hi");
        assert_eq!(trim(b"abhiba", b"ab"), b"hi");
        assert_eq!(trim("éhié".as_bytes(), "é".as_bytes()), b"hi");
        assert_eq!(trim(b"\xffhi\xff", "\u{FFFD}".as_bytes()), b"hi");
        assert_eq!(trim_left(b"aab", b"a"), b"b");
        assert_eq!(trim_right(b"baa", b"a"), b"b");
        assert_eq!(trim(b"abc", b""), b"abc");
        assert_eq!(trim_prefix(b"prefix-x", b"prefix-"), b"x");
        assert_eq!(trim_suffix(b"x.go", b".go"), b"x");
    }

    #[test]
    fn splitting() {
        let v: Vec<&[u8]> = vec![b"a", b"b", b"c"];
        assert_eq!(split(b"a,b,c", b","), v);
        assert_eq!(split(b"", b","), vec![b"" as &[u8]]);
        assert_eq!(split_n(b"a,b,c", b",", 2), vec![b"a" as &[u8], b"b,c"]);
        assert!(split_n(b"a,b,c", b",", 0).is_empty());
        assert_eq!(split_after(b"a,b,c", b","), vec![b"a," as &[u8], b"b,", b"c"]);
        assert_eq!(split("aé\u{FF}".as_bytes(), b""), vec![b"a" as &[u8], "é".as_bytes(), "\u{FF}".as_bytes()]);
        assert_eq!(split(b"a\xffb", b""), vec![b"a" as &[u8], b"\xff", b"b"]);
        assert_eq!(split_n(b"abc", b"", 2), vec![b"a" as &[u8], b"bc"]);
        assert!(split(b"", b"").is_empty());
        assert_eq!(split_after_n(b"a,b,c", b",", 10).len(), 3);
    }

    #[test]
    fn replacing() {
        assert_eq!(replace(b"oink oink oink", b"k", b"ky", 2), b"oinky oinky oink");
        assert_eq!(replace_all(b"oink oink", b"oink", b"moo"), b"moo moo");
        assert_eq!(replace(b"abc", b"", b"-", -1), b"-a-b-c-");
        assert_eq!(replace("aé".as_bytes(), b"", b"-", 2), "-a-é".as_bytes());
        assert_eq!(replace(b"abc", b"x", b"y", -1), b"abc");
        assert_eq!(repeat(b"ab", 3), b"ababab");
        assert_eq!(count(b"cheese", b"e"), 3);
        assert_eq!(count("five".as_bytes(), b""), 5);
    }

    #[test]
    fn searching() {
        assert_eq!(index(b"chicken", b"ken"), 4);
        assert_eq!(index(b"chicken", b"dmr"), -1);
        assert_eq!(index(b"abc", b""), 0);
        assert_eq!(last_index(b"go gopher", b"go"), 3);
        assert_eq!(last_index(b"abc", b""), 3);
        assert_eq!(last_index(b"abc", b"x"), -1);
        assert!(contains(b"seafood", b"foo"));
        assert!(contains(b"", b""));
        assert!(has_prefix(b"golang", b"go"));
        assert!(has_suffix(b"golang", b"ng"));
    }

    #[test]
    fn folding() {
        assert!(equal_fold(b"Go", b"GO"));
        assert!(equal_fold("σ".as_bytes(), "Σ".as_bytes()));
        assert!(equal_fold("ς".as_bytes(), "Σ".as_bytes()));
        assert!(equal_fold(b"k", "\u{212A}".as_bytes()));
        assert!(!equal_fold(b"a", b"ab"));
        assert!(equal_fold(b"\xff", b"\xfe"));
        assert!(!equal_fold("ß".as_bytes(), b"ss"));
    }
}
