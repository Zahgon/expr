//! Unicode character classes and simple case mappings over `u32` runes.
//!
//! Follows the behaviour of Go's standard library unicode package (The Go Authors, BSD-style licence).
//! The range tables in `tables.rs` are generated from Go 1.25.5's unicode package (Unicode 15.0.0).
//! Runes above [`MAX_RUNE`] behave like Go runes that are not valid code points: every
//! predicate is false and every mapping returns the rune unchanged.

#[rustfmt::skip]
mod tables;

pub use tables::VERSION;

/// Maximum valid Unicode code point.
pub const MAX_RUNE: u32 = 0x10FFFF;
/// Represents invalid code points.
pub const REPLACEMENT_CHAR: u32 = 0xFFFD;
/// Maximum ASCII value.
pub const MAX_ASCII: u32 = 0x7F;
/// Maximum Latin-1 value.
pub const MAX_LATIN1: u32 = 0xFF;

/// Index of the upper-case delta in a [`CaseRange`].
pub const UPPER_CASE: usize = 0;
/// Index of the lower-case delta in a [`CaseRange`].
pub const LOWER_CASE: usize = 1;
/// Index of the title-case delta in a [`CaseRange`].
pub const TITLE_CASE: usize = 2;

/// Delta marking alternating upper/lower sequences (an otherwise impossible delta).
pub const UPPER_LOWER: i32 = MAX_RUNE as i32 + 1;

/// A set of code points: (lo, hi, stride) ranges, like Go's `RangeTable`.
#[derive(Debug)]
pub struct RangeTable {
    /// Ranges whose bounds fit in 16 bits, sorted.
    pub r16: &'static [(u16, u16, u16)],
    /// Ranges at or above 0x10000, sorted.
    pub r32: &'static [(u32, u32, u32)],
    /// Number of entries in `r16` with `hi <= MAX_LATIN1`.
    pub latin_offset: usize,
}

/// A simple case-mapping range: (lo, hi, [upper, lower, title] deltas), like Go's `CaseRange`.
pub type CaseRange = (u32, u32, [i32; 3]);

// Property bits of Go's Latin-1 `properties` table.
const P_C: u8 = 1;
const P_P: u8 = 1 << 1;
const P_N: u8 = 1 << 2;
const P_S: u8 = 1 << 3;
const P_Z: u8 = 1 << 4;
const P_LU: u8 = 1 << 5;
const P_LL: u8 = 1 << 6;
const P_PRINT: u8 = 1 << 7;
const P_GRAPHIC: u8 = P_PRINT | P_Z;
const P_LMASK: u8 = P_LU | P_LL;

/// Go's `linearMax`: tables up to this size are searched linearly.
const LINEAR_MAX: usize = 18;

fn latin1_props(r: u32) -> Option<u8> {
    if r <= MAX_LATIN1 {
        Some(tables::PROPERTIES[r as usize])
    } else {
        None
    }
}

fn in_range<T: Into<u32> + Copy>(lo: T, hi: T, stride: T, r: u32) -> Option<bool> {
    let (lo, hi, stride) = (lo.into(), hi.into(), stride.into());
    if r < lo {
        return Some(false);
    }
    if r <= hi {
        return Some(stride == 1 || (r - lo).is_multiple_of(stride));
    }
    None
}

fn search<T: Into<u32> + Copy>(ranges: &[(T, T, T)], r: u32, linear: bool) -> bool {
    if linear {
        for &(lo, hi, stride) in ranges {
            if let Some(found) = in_range(lo, hi, stride, r) {
                return found;
            }
        }
        return false;
    }
    let (mut lo, mut hi) = (0, ranges.len());
    while lo < hi {
        let m = (lo + hi) / 2;
        let (rlo, rhi, stride) = ranges[m];
        let (rlo, rhi, stride) = (rlo.into(), rhi.into(), stride.into());
        if rlo <= r && r <= rhi {
            return stride == 1 || (r - rlo).is_multiple_of(stride);
        }
        if r < rlo {
            hi = m;
        } else {
            lo = m + 1;
        }
    }
    false
}

fn is_from(table: &RangeTable, r: u32, offset: usize) -> bool {
    let r16 = &table.r16[offset.min(table.r16.len())..];
    if let Some(last) = r16.last() {
        if r <= u32::from(last.1) {
            return search(r16, r, r16.len() <= LINEAR_MAX || r <= MAX_LATIN1);
        }
    }
    if let Some(first) = table.r32.first() {
        if r >= first.0 {
            return search(table.r32, r, table.r32.len() <= LINEAR_MAX);
        }
    }
    false
}

/// Reports whether the rune is in the specified table of ranges (Go's `unicode.Is`).
pub fn is(table: &RangeTable, r: u32) -> bool {
    is_from(table, r, 0)
}

fn is_excluding_latin(table: &RangeTable, r: u32) -> bool {
    is_from(table, r, table.latin_offset)
}

/// Reports whether the rune is a letter (category L).
pub fn is_letter(r: u32) -> bool {
    match latin1_props(r) {
        Some(p) => p & P_LMASK != 0,
        None => is_excluding_latin(&tables::LETTER, r),
    }
}

/// Reports whether the rune is a decimal digit (category Nd).
pub fn is_digit(r: u32) -> bool {
    if r <= MAX_LATIN1 {
        return (u32::from(b'0')..=u32::from(b'9')).contains(&r);
    }
    is_excluding_latin(&tables::DIGIT, r)
}

/// Reports whether the rune is a number (category N).
pub fn is_number(r: u32) -> bool {
    match latin1_props(r) {
        Some(p) => p & P_N != 0,
        None => is_excluding_latin(&tables::NUMBER, r),
    }
}

/// Reports whether the rune is a mark character (category M).
pub fn is_mark(r: u32) -> bool {
    is_excluding_latin(&tables::MARK, r)
}

/// Reports whether the rune is a space character as defined by Unicode's White Space
/// property; in the Latin-1 space this is '\t', '\n', '\v', '\f', '\r', ' ', U+0085 and U+00A0.
pub fn is_space(r: u32) -> bool {
    if r <= MAX_LATIN1 {
        return matches!(r, 0x09..=0x0D | 0x20 | 0x85 | 0xA0);
    }
    is_excluding_latin(&tables::WHITE_SPACE, r)
}

/// Reports whether the rune is printable by Go's definition: letters, marks, numbers,
/// punctuation, symbols and the ASCII space U+0020 (other spaces are not printable).
pub fn is_print(r: u32) -> bool {
    match latin1_props(r) {
        Some(p) => p & P_PRINT != 0,
        None => {
            [&tables::LETTER, &tables::MARK, &tables::NUMBER, &tables::PUNCT, &tables::SYMBOL].iter().any(|t| is(t, r))
        }
    }
}

/// Reports whether the rune is a graphic character: letters, marks, numbers, punctuation,
/// symbols and spaces (category Zs).
pub fn is_graphic(r: u32) -> bool {
    match latin1_props(r) {
        Some(p) => p & P_GRAPHIC != 0,
        None => [&tables::LETTER, &tables::MARK, &tables::NUMBER, &tables::PUNCT, &tables::SYMBOL, &tables::ZS]
            .iter()
            .any(|t| is(t, r)),
    }
}

/// Reports whether the rune is a control character (only Latin-1 runes can be, as in Go).
pub fn is_control(r: u32) -> bool {
    latin1_props(r).is_some_and(|p| p & P_C != 0)
}

/// Reports whether the rune is a symbolic character (category S).
pub fn is_symbol(r: u32) -> bool {
    match latin1_props(r) {
        Some(p) => p & P_S != 0,
        None => is_excluding_latin(&tables::SYMBOL, r),
    }
}

/// Reports whether the rune is an upper case letter.
pub fn is_upper(r: u32) -> bool {
    match latin1_props(r) {
        Some(p) => p & P_LMASK == P_LU,
        None => is_excluding_latin(&tables::UPPER, r),
    }
}

/// Reports whether the rune is a lower case letter.
pub fn is_lower(r: u32) -> bool {
    match latin1_props(r) {
        Some(p) => p & P_LMASK == P_LL,
        None => is_excluding_latin(&tables::LOWER, r),
    }
}

/// Reports whether the rune is a title case letter.
pub fn is_title(r: u32) -> bool {
    if r <= MAX_LATIN1 {
        return false;
    }
    is_excluding_latin(&tables::TITLE, r)
}

/// Reports whether the rune is a punctuation character (category P).
pub fn is_punct(r: u32) -> bool {
    match latin1_props(r) {
        Some(p) => p & P_P != 0,
        None => is(&tables::PUNCT, r),
    }
}

fn lookup_case_range(r: u32, ranges: &'static [CaseRange]) -> Option<&'static CaseRange> {
    let (mut lo, mut hi) = (0, ranges.len());
    while lo < hi {
        let m = (lo + hi) / 2;
        let cr = &ranges[m];
        if cr.0 <= r && r <= cr.1 {
            return Some(cr);
        }
        if r < cr.0 {
            hi = m;
        } else {
            lo = m + 1;
        }
    }
    None
}

fn convert_case(case: usize, r: u32, cr: &CaseRange) -> u32 {
    let delta = cr.2[case];
    if delta == UPPER_LOWER {
        // Alternating Upper/Lower sequence starting with an upper-case letter: even offsets
        // are upper case, odd ones lower; take the low bit from the case index.
        return cr.0 + (((r - cr.0) & !1) | (case as u32 & 1));
    }
    r.wrapping_add_signed(delta)
}

/// Maps the rune to the given case ([`UPPER_CASE`], [`LOWER_CASE`] or [`TITLE_CASE`]);
/// any other case index yields U+FFFD, as in Go's `unicode.To`.
pub fn to(case: usize, r: u32) -> u32 {
    if case > TITLE_CASE {
        return REPLACEMENT_CHAR;
    }
    match lookup_case_range(r, tables::CASE_RANGES) {
        Some(cr) => convert_case(case, r, cr),
        None => r,
    }
}

/// Maps the rune to upper case.
pub fn to_upper(r: u32) -> u32 {
    if r <= MAX_ASCII {
        return u32::from((r as u8).to_ascii_uppercase());
    }
    to(UPPER_CASE, r)
}

/// Maps the rune to lower case.
pub fn to_lower(r: u32) -> u32 {
    if r <= MAX_ASCII {
        return u32::from((r as u8).to_ascii_lowercase());
    }
    to(LOWER_CASE, r)
}

/// Maps the rune to title case.
pub fn to_title(r: u32) -> u32 {
    if r <= MAX_ASCII {
        return u32::from((r as u8).to_ascii_uppercase());
    }
    to(TITLE_CASE, r)
}

/// Iterates over code points equivalent under simple case folding: returns the smallest
/// equivalent rune greater than `r` if one exists, else the smallest equivalent rune.
/// Runes above [`MAX_RUNE`] are returned unchanged.
pub fn simple_fold(r: u32) -> u32 {
    if r > MAX_RUNE {
        return r;
    }
    if let Some(&f) = tables::ASCII_FOLD.get(r as usize) {
        return u32::from(f);
    }
    let orbit = tables::CASE_ORBIT;
    if let Ok(i) = orbit.binary_search_by_key(&r, |&(from, _)| from) {
        return orbit[i].1;
    }
    match lookup_case_range(r, tables::CASE_RANGES) {
        Some(cr) => {
            let l = convert_case(LOWER_CASE, r, cr);
            if l != r {
                l
            } else {
                convert_case(UPPER_CASE, r, cr)
            }
        }
        None => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version() {
        assert_eq!(VERSION, "15.0.0");
    }

    #[test]
    fn letters_and_digits() {
        assert!(is_letter('a' as u32));
        assert!(is_letter(0xAA));
        assert!(!is_letter(0xD7));
        assert!(is_letter(0x4E16));
        assert!(is_digit('7' as u32));
        assert!(!is_digit(0xB2));
        assert!(is_number(0xB2));
        assert!(is_digit(0x0663));
        assert!(is_number(0x2163));
        assert!(!is_digit(0x2163));
    }

    #[test]
    fn spaces_and_print() {
        assert!(is_space(0x85));
        assert!(is_space(0x2028));
        assert!(!is_space(0x200B));
        assert!(is_print(' ' as u32));
        assert!(!is_print(0xA0));
        assert!(is_graphic(0xA0));
        assert!(!is_print(0x3000));
        assert!(is_graphic(0x3000));
        assert!(!is_print(0xAD));
        assert!(!is_print(0x7F));
        assert!(is_print(0x1F600));
        assert!(!is_print(0x10FFFF));
        assert!(!is_print(0x110000));
    }

    #[test]
    fn cases() {
        assert!(is_upper('A' as u32));
        assert!(is_lower(0xDF));
        assert!(!is_upper(0xAA));
        assert!(is_title(0x1C5));
        assert!(!is_title('A' as u32));
        assert!(is_punct('!' as u32));
        assert!(!is_punct('+' as u32));
        assert!(is_symbol('+' as u32));
    }

    #[test]
    fn mappings() {
        assert_eq!(to_upper('a' as u32), 'A' as u32);
        assert_eq!(to_upper(0xDF), 0xDF);
        assert_eq!(to_upper(0xFF), 0x178);
        assert_eq!(to_upper(0x1C6), 0x1C4);
        assert_eq!(to_title(0x1C6), 0x1C5);
        assert_eq!(to_lower(0x1C5), 0x1C6);
        assert_eq!(to_lower(0x130), 'i' as u32);
        assert_eq!(to_upper(0x131), 'I' as u32);
        assert_eq!(to_upper(0x101), 0x100);
        assert_eq!(to_lower(0x100), 0x101);
        assert_eq!(to_lower(0x1E9E), 0xDF);
        assert_eq!(to(3, 'a' as u32), 0xFFFD);
        assert_eq!(to_upper(0x110000), 0x110000);
    }

    #[test]
    fn folding() {
        assert_eq!(simple_fold('A' as u32), 'a' as u32);
        assert_eq!(simple_fold('a' as u32), 'A' as u32);
        assert_eq!(simple_fold('K' as u32), 'k' as u32);
        assert_eq!(simple_fold('k' as u32), 0x212A);
        assert_eq!(simple_fold(0x212A), 'K' as u32);
        assert_eq!(simple_fold('1' as u32), '1' as u32);
        assert_eq!(simple_fold(0x130), 0x130);
        assert_eq!(simple_fold(0xDF), 0x1E9E);
        assert_eq!(simple_fold(0x1E9E), 0xDF);
        assert_eq!(simple_fold(0x110000), 0x110000);
    }
}
