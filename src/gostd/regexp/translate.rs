//! Translation of a Go syntax tree into an equivalent pattern for the `regex` crate.
//!
//! Every construct is spelled out explicitly so that no flag of the `regex` crate changes its
//! meaning: classes (including Go's case folding and Unicode tables) become explicit code point
//! ranges, Go's ASCII-only `\b` becomes `(?-u:\b)`, and `^`/`$` become `\A`, `\z`, `(?m:^)` or
//! `(?m:$)` according to the flags Go recorded while parsing.
//!
//! Follows the behaviour of Go's standard library regexp package (The Go Authors, BSD-style licence).

use std::fmt::Write;

use super::parse::{Node, Op, Tree, FOLD_CASE};
use super::unicode::simple_fold;

/// A pattern that never matches: an ASCII word boundary that is also a non-boundary.
const NO_MATCH: &str = r"(?:(?-u:\b)(?-u:\B))";

const SURROGATE_LO: i32 = 0xD800;
const SURROGATE_HI: i32 = 0xDFFF;

/// The translation of a Go syntax tree.
pub(super) struct Translation {
    /// The pattern in `regex` crate syntax.
    pub(super) pattern: String,
    /// Whether the pattern contains Go's `\B`, the only construct whose empty matches can fall
    /// between the bytes of a multi-byte character.
    pub(super) has_non_boundary: bool,
}

/// Translates the tree into `regex` crate syntax.
pub(super) fn translate(tree: &Tree) -> Translation {
    let mut pattern = String::new();
    emit(&tree.nodes, tree.root, &mut pattern);
    let has_non_boundary = tree.nodes.iter().any(|n| n.op == Op::NoWordBoundary);
    Translation { pattern, has_non_boundary }
}

fn push_char(out: &mut String, c: i32) {
    match u8::try_from(c) {
        Ok(b) if b.is_ascii_alphanumeric() => out.push(char::from(b)),
        _ => {
            // Writing to a String cannot fail.
            let _ = write!(out, "\\x{{{c:X}}}");
        }
    }
}

/// Appends a class matching the given (lo, hi) ranges; surrogates cannot be matched by Go
/// (decoding never yields them) and cannot be written for the `regex` crate, so they are dropped.
fn push_class(out: &mut String, ranges: &[(i32, i32)]) {
    let mut parts = Vec::new();
    for &(lo, hi) in ranges {
        if lo < SURROGATE_LO {
            parts.push((lo, hi.min(SURROGATE_LO - 1)));
        }
        if hi > SURROGATE_HI {
            parts.push((lo.max(SURROGATE_HI + 1), hi));
        }
    }
    if parts.is_empty() {
        out.push_str(NO_MATCH);
        return;
    }
    out.push('[');
    for (lo, hi) in parts {
        push_char(out, lo);
        if hi != lo {
            out.push('-');
            push_char(out, hi);
        }
    }
    out.push(']');
}

fn push_literal_rune(out: &mut String, r: i32, fold: bool) {
    if fold {
        let mut orbit = vec![(r, r)];
        let mut f = simple_fold(r);
        while f != r {
            orbit.push((f, f));
            f = simple_fold(f);
        }
        if orbit.len() > 1 {
            orbit.sort_unstable();
            push_class(out, &orbit);
            return;
        }
    }
    if (SURROGATE_LO..=SURROGATE_HI).contains(&r) {
        out.push_str(NO_MATCH);
    } else {
        push_char(out, r);
    }
}

fn emit(nodes: &[Node], i: usize, out: &mut String) {
    let n = &nodes[i];
    match n.op {
        Op::NoMatch => out.push_str(NO_MATCH),
        Op::EmptyMatch => out.push_str("(?:)"),
        Op::Literal => {
            let fold = n.flags & FOLD_CASE != 0;
            for &r in &n.runes {
                push_literal_rune(out, r, fold);
            }
        }
        Op::CharClass => {
            let ranges: Vec<(i32, i32)> = n.runes.chunks_exact(2).map(|p| (p[0], p[1])).collect();
            push_class(out, &ranges);
        }
        Op::AnyCharNotNL => out.push_str(r"[^\n]"),
        Op::AnyChar => out.push_str("(?s:.)"),
        Op::BeginLine => out.push_str("(?m:^)"),
        Op::EndLine => out.push_str("(?m:$)"),
        Op::BeginText => out.push_str(r"\A"),
        Op::EndText => out.push_str(r"\z"),
        Op::WordBoundary => out.push_str(r"(?-u:\b)"),
        Op::NoWordBoundary => out.push_str(r"(?-u:\B)"),
        Op::Capture => {
            out.push_str("(?:");
            emit(nodes, n.sub[0], out);
            out.push(')');
        }
        Op::Star | Op::Plus | Op::Quest | Op::Repeat => {
            out.push_str("(?:");
            emit(nodes, n.sub[0], out);
            out.push(')');
            match n.op {
                Op::Star => out.push('*'),
                Op::Plus => out.push('+'),
                Op::Quest => out.push('?'),
                _ if n.max == -1 => {
                    let _ = write!(out, "{{{},}}", n.min);
                }
                _ => {
                    let _ = write!(out, "{{{},{}}}", n.min, n.max);
                }
            }
        }
        Op::Concat => {
            out.push_str("(?:");
            for &s in &n.sub {
                emit(nodes, s, out);
            }
            out.push(')');
        }
        Op::Alternate => {
            out.push_str("(?:");
            for (k, &s) in n.sub.iter().enumerate() {
                if k > 0 {
                    out.push('|');
                }
                emit(nodes, s, out);
            }
            out.push(')');
        }
        // Pseudo-ops never survive a successful parse.
        Op::LeftParen | Op::VerticalBar => out.push_str(NO_MATCH),
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse::parse;
    use super::*;

    fn tr(p: &str) -> String {
        translate(&parse(p.as_bytes()).expect("parses")).pattern
    }

    #[test]
    fn literals_and_folding() {
        assert_eq!(tr("ab"), "ab");
        assert_eq!(tr("(?i)k"), r"[Kk\x{212A}]");
        assert_eq!(tr("a.b"), r"(?:a[^\n]b)");
    }

    #[test]
    fn assertions() {
        assert_eq!(tr(r"^\b$"), r"(?:\A(?-u:\b)\z)");
        assert_eq!(tr(r"(?m)^\B$"), r"(?:(?m:^)(?-u:\B)(?m:$))");
    }

    #[test]
    fn repeats() {
        assert_eq!(tr("a{2,}"), "(?:a){2,}");
        assert_eq!(tr("(a|bc)*?"), "(?:(?:(?:a|bc)))*");
    }

    #[test]
    fn surrogates_never_match() {
        assert_eq!(tr(r"\x{D800}"), NO_MATCH);
        assert_eq!(tr(r"[\x{D7FF}-\x{E000}]"), r"[\x{D7FF}\x{E000}]");
    }

    #[test]
    fn empty_class_never_matches() {
        assert_eq!(tr(r"[^\x00-\x{10FFFF}]"), NO_MATCH);
    }
}
