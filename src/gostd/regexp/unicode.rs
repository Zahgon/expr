//! Lookups into Go's Unicode tables: `unicode.SimpleFold` and the named range tables used by
//! `\p{...}` classes.
//!
//! Follows the behaviour of Go's standard library unicode package (The Go Authors, BSD-style licence).

use std::sync::OnceLock;

use super::tables::{CATEGORIES, FOLD_CATEGORY, FOLD_RUNS, FOLD_SCRIPT, SCRIPTS};

/// Go's `unicode.MaxRune`.
pub(super) const MAX_RUNE: i32 = 0x10FFFF;

/// A Go `unicode.RangeTable` flattened into (lo, hi, stride) triples, R16 entries first.
pub(super) type Table = &'static [u32];

/// Every rune changed by `unicode.SimpleFold`, paired with its fold, sorted by rune.
fn fold_pairs() -> &'static [(i32, i32)] {
    static PAIRS: OnceLock<Vec<(i32, i32)>> = OnceLock::new();
    PAIRS.get_or_init(|| {
        let mut pairs = Vec::new();
        for &(lo, hi, stride, delta) in FOLD_RUNS {
            let mut c = lo;
            while c <= hi {
                // The table only holds code points, so these conversions are lossless.
                let r = c as i32;
                pairs.push((r, r + delta));
                c += stride;
            }
        }
        pairs.sort_unstable();
        pairs
    })
}

/// Go's `unicode.SimpleFold`: the next rune in the case-folding orbit of `r`.
pub(super) fn simple_fold(r: i32) -> i32 {
    if !(0..=MAX_RUNE).contains(&r) {
        return r;
    }
    let pairs = fold_pairs();
    match pairs.binary_search_by_key(&r, |p| p.0) {
        Ok(i) => pairs[i].1,
        Err(_) => r,
    }
}

fn lookup(map: &'static [(&'static str, Table)], name: &[u8]) -> Option<Table> {
    map.binary_search_by(|(k, _)| k.as_bytes().cmp(name)).ok().map(|i| map[i].1)
}

/// `unicode.Categories[name]`.
pub(super) fn category(name: &[u8]) -> Option<Table> {
    lookup(CATEGORIES, name)
}

/// `unicode.Scripts[name]`.
pub(super) fn script(name: &[u8]) -> Option<Table> {
    lookup(SCRIPTS, name)
}

/// `unicode.FoldCategory[name]`.
pub(super) fn fold_category(name: &[u8]) -> Option<Table> {
    lookup(FOLD_CATEGORY, name)
}

/// `unicode.FoldScript[name]`.
pub(super) fn fold_script(name: &[u8]) -> Option<Table> {
    lookup(FOLD_SCRIPT, name)
}

/// Iterates the (lo, hi, stride) triples of a table.
pub(super) fn triples(t: Table) -> impl Iterator<Item = (i32, i32, i32)> {
    // Table values are code points or small strides, so they fit in i32.
    t.chunks_exact(3).map(|c| (c[0] as i32, c[1] as i32, c[2] as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_fold_orbits() {
        assert_eq!(simple_fold('A' as i32), 'a' as i32);
        assert_eq!(simple_fold('a' as i32), 'A' as i32);
        assert_eq!(simple_fold('K' as i32), 'k' as i32);
        assert_eq!(simple_fold('k' as i32), 0x212A);
        assert_eq!(simple_fold(0x212A), 'K' as i32);
        assert_eq!(simple_fold('1' as i32), '1' as i32);
        assert_eq!(simple_fold(-1), -1);
        assert_eq!(simple_fold(0x110000), 0x110000);
    }

    #[test]
    fn table_lookup() {
        assert!(category(b"Lu").is_some());
        assert!(category(b"LC").is_some());
        assert!(script(b"Greek").is_some());
        assert!(script(b"Old_Italic").is_some());
        assert!(script(b"Nope").is_none());
    }
}
