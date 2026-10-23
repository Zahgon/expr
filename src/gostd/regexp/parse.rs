//! A port of Go's regexp/syntax parser in Perl mode (`syntax.Perl`), producing Go's syntax tree
//! and Go's exact parse errors, including the size and nesting limits.
//!
//! Go's parser recycles nodes through a free list and caches sizes and heights per node; the
//! limit checks depend on that bookkeeping, so nodes live in an arena addressed by index and the
//! free list and caches are reproduced exactly.
//!
//! Follows the behaviour of Go's standard library regexp/syntax package (The Go Authors, BSD-style licence).

use std::collections::HashMap;

use super::unicode::{self as uni, simple_fold, Table, MAX_RUNE};

/// Go's `utf8.RuneError`.
pub(super) const RUNE_ERROR: i32 = 0xFFFD;

/// Parse flags (Go's `syntax.Flags`).
pub(super) const FOLD_CASE: u16 = 1;
const CLASS_NL: u16 = 1 << 2;
const DOT_NL: u16 = 1 << 3;
const ONE_LINE: u16 = 1 << 4;
const NON_GREEDY: u16 = 1 << 5;
const PERL_X: u16 = 1 << 6;
const UNICODE_GROUPS: u16 = 1 << 7;
const WAS_DOLLAR: u16 = 1 << 8;
/// `syntax.Perl`, the flags used by `regexp.Compile`.
const PERL: u16 = CLASS_NL | ONE_LINE | PERL_X | UNICODE_GROUPS;

const MAX_HEIGHT: usize = 1000;
const MAX_SIZE: i64 = (128 << 20) / (5 * 8);
const MAX_RUNES: usize = (128 << 20) / 4;
const MIN_FOLD: i32 = 0x0041;
const MAX_FOLD: i32 = 0x1e943;

/// Go's `syntax.Op`, in Go's order (the parser compares ops numerically).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Op {
    NoMatch = 1,
    EmptyMatch,
    Literal,
    CharClass,
    AnyCharNotNL,
    AnyChar,
    BeginLine,
    EndLine,
    BeginText,
    EndText,
    WordBoundary,
    NoWordBoundary,
    Capture,
    Star,
    Plus,
    Quest,
    Repeat,
    Concat,
    Alternate,
    LeftParen = 128,
    VerticalBar,
}

/// One node of Go's `syntax.Regexp` tree.
#[derive(Clone, Debug)]
pub(super) struct Node {
    pub(super) op: Op,
    pub(super) flags: u16,
    pub(super) sub: Vec<usize>,
    pub(super) runes: Vec<i32>,
    pub(super) min: i64,
    pub(super) max: i64,
    cap: i64,
    name: Vec<u8>,
}

impl Node {
    fn new(op: Op) -> Node {
        Node { op, flags: 0, sub: Vec::new(), runes: Vec::new(), min: 0, max: 0, cap: 0, name: Vec::new() }
    }
}

/// A parsed expression: the node arena and the index of the root.
pub(super) struct Tree {
    pub(super) nodes: Vec<Node>,
    pub(super) root: usize,
}

/// A parse error: Go's error code text and the offending expression bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Error {
    pub(super) code: &'static str,
    pub(super) expr: Vec<u8>,
}

impl Error {
    fn new(code: &'static str, expr: &[u8]) -> Error {
        Error { code, expr: expr.to_vec() }
    }

    /// Go's `(*syntax.Error).Error()` text.
    pub(super) fn message(&self) -> Vec<u8> {
        let mut m = b"error parsing regexp: ".to_vec();
        m.extend_from_slice(self.code.as_bytes());
        m.extend_from_slice(b": `");
        m.extend_from_slice(&self.expr);
        m.push(b'`');
        m
    }
}

const ERR_INVALID_CHAR_RANGE: &str = "invalid character class range";
const ERR_INVALID_ESCAPE: &str = "invalid escape sequence";
const ERR_INVALID_NAMED_CAPTURE: &str = "invalid named capture";
const ERR_INVALID_PERL_OP: &str = "invalid or unsupported Perl syntax";
const ERR_INVALID_REPEAT_OP: &str = "invalid nested repetition operator";
const ERR_INVALID_REPEAT_SIZE: &str = "invalid repeat count";
const ERR_INVALID_UTF8: &str = "invalid UTF-8";
const ERR_MISSING_BRACKET: &str = "missing closing ]";
const ERR_MISSING_PAREN: &str = "missing closing )";
const ERR_MISSING_REPEAT_ARGUMENT: &str = "missing argument to repetition operator";
const ERR_TRAILING_BACKSLASH: &str = "trailing backslash at end of expression";
const ERR_UNEXPECTED_PAREN: &str = "unexpected )";
const ERR_NESTING_DEPTH: &str = "expression nests too deeply";
const ERR_LARGE: &str = "expression too large";

type PResult<T> = Result<T, Error>;

/// Go's `utf8.DecodeRune`: returns (RuneError, 1) for invalid input and (RuneError, 0) for empty.
pub(super) fn decode_rune(s: &[u8]) -> (i32, usize) {
    let Some(&b0) = s.first() else {
        return (RUNE_ERROR, 0);
    };
    if b0 < 0x80 {
        return (i32::from(b0), 1);
    }
    let n = match b0 {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return (RUNE_ERROR, 1),
    };
    match s.get(..n).map(std::str::from_utf8) {
        Some(Ok(st)) => match st.chars().next() {
            Some(c) => (c as i32, n),
            None => (RUNE_ERROR, 1),
        },
        _ => (RUNE_ERROR, 1),
    }
}

/// Go's `nextRune`.
fn next_rune(s: &[u8]) -> PResult<(i32, &[u8])> {
    let (c, size) = decode_rune(s);
    if c == RUNE_ERROR && size == 1 {
        return Err(Error::new(ERR_INVALID_UTF8, s));
    }
    Ok((c, &s[size..]))
}

/// Go's `checkUTF8`.
fn check_utf8(mut s: &[u8]) -> PResult<()> {
    while !s.is_empty() {
        let (c, size) = decode_rune(s);
        if c == RUNE_ERROR && size == 1 {
            return Err(Error::new(ERR_INVALID_UTF8, s));
        }
        s = &s[size..];
    }
    Ok(())
}

fn is_alnum(c: i32) -> bool {
    (i32::from(b'0')..=i32::from(b'9')).contains(&c)
        || (i32::from(b'A')..=i32::from(b'Z')).contains(&c)
        || (i32::from(b'a')..=i32::from(b'z')).contains(&c)
}

fn unhex(c: i32) -> i32 {
    match u8::try_from(c) {
        Ok(b @ b'0'..=b'9') => i32::from(b - b'0'),
        Ok(b @ b'a'..=b'f') => i32::from(b - b'a') + 10,
        Ok(b @ b'A'..=b'F') => i32::from(b - b'A') + 10,
        _ => -1,
    }
}

/// The prefix of `whole` that remains once the suffix `rest` is removed.
fn cut<'a>(whole: &'a [u8], rest: &[u8]) -> &'a [u8] {
    &whole[..whole.len() - rest.len()]
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A Perl or POSIX class group: sign and ranges.
struct CharGroup {
    sign: i32,
    class: &'static [i32],
}

const CODE_DIGIT: &[i32] = &[0x30, 0x39];
const CODE_SPACE: &[i32] = &[0x9, 0xa, 0xc, 0xd, 0x20, 0x20];
const CODE_WORD: &[i32] = &[0x30, 0x39, 0x41, 0x5a, 0x5f, 0x5f, 0x61, 0x7a];

fn perl_group(s: &[u8]) -> Option<CharGroup> {
    let (sign, class) = match s {
        b"\\d" => (1, CODE_DIGIT),
        b"\\D" => (-1, CODE_DIGIT),
        b"\\s" => (1, CODE_SPACE),
        b"\\S" => (-1, CODE_SPACE),
        b"\\w" => (1, CODE_WORD),
        b"\\W" => (-1, CODE_WORD),
        _ => return None,
    };
    Some(CharGroup { sign, class })
}

fn posix_group(name: &[u8]) -> Option<CharGroup> {
    let inner = name.strip_prefix(b"[:")?.strip_suffix(b":]")?;
    let (sign, base) = match inner.strip_prefix(b"^") {
        Some(b) => (-1, b),
        None => (1, inner),
    };
    let class: &'static [i32] = match base {
        b"alnum" => &[0x30, 0x39, 0x41, 0x5a, 0x61, 0x7a],
        b"alpha" => &[0x41, 0x5a, 0x61, 0x7a],
        b"ascii" => &[0x0, 0x7f],
        b"blank" => &[0x9, 0x9, 0x20, 0x20],
        b"cntrl" => &[0x0, 0x1f, 0x7f, 0x7f],
        b"digit" => &[0x30, 0x39],
        b"graph" => &[0x21, 0x7e],
        b"lower" => &[0x61, 0x7a],
        b"print" => &[0x20, 0x7e],
        b"punct" => &[0x21, 0x2f, 0x3a, 0x40, 0x5b, 0x60, 0x7b, 0x7e],
        b"space" => &[0x9, 0xd, 0x20, 0x20],
        b"upper" => &[0x41, 0x5a],
        b"word" => &[0x30, 0x39, 0x41, 0x5a, 0x5f, 0x5f, 0x61, 0x7a],
        b"xdigit" => &[0x30, 0x39, 0x41, 0x46, 0x61, 0x66],
        _ => return None,
    };
    Some(CharGroup { sign, class })
}

/// A named Unicode table: either one of Go's generated tables or one of the parser's own.
#[derive(Clone, Copy)]
enum UniTable {
    Go(Table),
    Any,
    Ascii,
    AsciiFold,
}

impl UniTable {
    fn triples(self) -> Vec<(i32, i32, i32)> {
        match self {
            UniTable::Go(t) => uni::triples(t).collect(),
            UniTable::Any => vec![(0, 0xFFFF, 1), (0x10000, MAX_RUNE, 1)],
            UniTable::Ascii => vec![(0, 0x7F, 1)],
            UniTable::AsciiFold => vec![(0, 0x7F, 1), (0x017F, 0x017F, 1), (0x212A, 0x212A, 1)],
        }
    }
}

/// Go's `canonicalName`.
fn canonical_name(name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len());
    let mut first = true;
    for &b in name {
        let mut c = b;
        if c == b'_' || c == b'-' || c == b' ' {
            continue;
        }
        if first {
            c = c.to_ascii_uppercase();
            first = false;
        } else {
            c = c.to_ascii_lowercase();
        }
        out.push(c);
    }
    out
}

/// Go's `unicodeTable`: (table, fold table, sign).
fn unicode_table(name: &[u8]) -> Option<(UniTable, Option<UniTable>, i32)> {
    let name = canonical_name(name);
    match name.as_slice() {
        b"Any" => return Some((UniTable::Any, Some(UniTable::Any), 1)),
        b"Assigned" => {
            let cn = uni::category(b"Cn")?;
            return Some((UniTable::Go(cn), Some(UniTable::Go(cn)), -1));
        }
        b"Ascii" => return Some((UniTable::Ascii, Some(UniTable::AsciiFold), 1)),
        b"Lc" => {
            let t = uni::category(b"LC")?;
            return Some((UniTable::Go(t), uni::fold_category(b"LC").map(UniTable::Go), 1));
        }
        _ => {}
    }
    if let Some(t) = uni::category(&name) {
        return Some((UniTable::Go(t), uni::fold_category(&name).map(UniTable::Go), 1));
    }
    if let Some(t) = uni::script(&name) {
        return Some((UniTable::Go(t), uni::fold_script(&name).map(UniTable::Go), 1));
    }
    let actual = super::tables::CATEGORY_ALIASES
        .iter()
        .find(|(alias, _)| canonical_name(alias.as_bytes()) == name)
        .map(|(_, actual)| actual.as_bytes())?;
    let t = uni::category(actual)?;
    Some((UniTable::Go(t), uni::fold_category(actual).map(UniTable::Go), 1))
}

/// Go's `minFoldRune`.
fn min_fold_rune(r: i32) -> i32 {
    if !(MIN_FOLD..=MAX_FOLD).contains(&r) {
        return r;
    }
    let mut m = r;
    let mut f = simple_fold(r);
    while f != r {
        m = m.min(f);
        f = simple_fold(f);
    }
    m
}

/// Go's `cleanClass`: sorts and merges the ranges.
fn clean_class(r: &mut Vec<i32>) {
    let mut pairs: Vec<(i32, i32)> = r.chunks_exact(2).map(|c| (c[0], c[1])).collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    r.clear();
    for (lo, hi) in pairs {
        if let Some(last) = r.last_mut() {
            if lo <= last.saturating_add(1) {
                if hi > *last {
                    *last = hi;
                }
                continue;
            }
        }
        r.push(lo);
        r.push(hi);
    }
}

/// Go's `appendRange`.
fn append_range(r: &mut Vec<i32>, lo: i32, hi: i32) {
    let n = r.len();
    for i in [2, 4] {
        if n >= i {
            let (rlo, rhi) = (r[n - i], r[n - i + 1]);
            if lo <= rhi + 1 && rlo <= hi + 1 {
                if lo < rlo {
                    r[n - i] = lo;
                }
                if hi > rhi {
                    r[n - i + 1] = hi;
                }
                return;
            }
        }
    }
    r.push(lo);
    r.push(hi);
}

/// Go's `appendFoldedRange`.
fn append_folded_range(r: &mut Vec<i32>, mut lo: i32, mut hi: i32) {
    if lo <= MIN_FOLD && hi >= MAX_FOLD {
        return append_range(r, lo, hi);
    }
    if hi < MIN_FOLD || lo > MAX_FOLD {
        return append_range(r, lo, hi);
    }
    if lo < MIN_FOLD {
        append_range(r, lo, MIN_FOLD - 1);
        lo = MIN_FOLD;
    }
    if hi > MAX_FOLD {
        append_range(r, MAX_FOLD + 1, hi);
        hi = MAX_FOLD;
    }
    for c in lo..=hi {
        append_range(r, c, c);
        let mut f = simple_fold(c);
        while f != c {
            append_range(r, f, f);
            f = simple_fold(f);
        }
    }
}

fn append_literal(r: &mut Vec<i32>, x: i32, flags: u16) {
    if flags & FOLD_CASE != 0 {
        append_folded_range(r, x, x);
    } else {
        append_range(r, x, x);
    }
}

fn append_class(r: &mut Vec<i32>, x: &[i32]) {
    for p in x.chunks_exact(2) {
        append_range(r, p[0], p[1]);
    }
}

fn append_folded_class(r: &mut Vec<i32>, x: &[i32]) {
    for p in x.chunks_exact(2) {
        append_folded_range(r, p[0], p[1]);
    }
}

fn append_negated_class(r: &mut Vec<i32>, x: &[i32]) {
    let mut next_lo = 0;
    for p in x.chunks_exact(2) {
        if next_lo < p[0] {
            append_range(r, next_lo, p[0] - 1);
        }
        next_lo = p[1] + 1;
    }
    if next_lo <= MAX_RUNE {
        append_range(r, next_lo, MAX_RUNE);
    }
}

fn append_table(r: &mut Vec<i32>, t: UniTable) {
    for (lo, hi, stride) in t.triples() {
        if stride == 1 {
            append_range(r, lo, hi);
            continue;
        }
        let mut c = lo;
        while c <= hi {
            append_range(r, c, c);
            c += stride;
        }
    }
}

fn append_negated_table(r: &mut Vec<i32>, t: UniTable) {
    let mut next_lo = 0;
    for (lo, hi, stride) in t.triples() {
        if stride == 1 {
            if next_lo < lo {
                append_range(r, next_lo, lo - 1);
            }
            next_lo = hi + 1;
            continue;
        }
        let mut c = lo;
        while c <= hi {
            if next_lo < c {
                append_range(r, next_lo, c - 1);
            }
            next_lo = c + 1;
            c += stride;
        }
    }
    if next_lo <= MAX_RUNE {
        append_range(r, next_lo, MAX_RUNE);
    }
}

/// Go's `negateClass` (the class must be clean).
fn negate_class(r: &[i32]) -> Vec<i32> {
    let mut out = Vec::with_capacity(r.len() + 2);
    let mut next_lo = 0;
    for p in r.chunks_exact(2) {
        if next_lo < p[0] {
            out.push(next_lo);
            out.push(p[0] - 1);
        }
        next_lo = p[1] + 1;
    }
    if next_lo <= MAX_RUNE {
        out.push(next_lo);
        out.push(MAX_RUNE);
    }
    out
}

struct Parser<'a> {
    nodes: Vec<Node>,
    flags: u16,
    stack: Vec<usize>,
    free: Vec<usize>,
    num_cap: i64,
    whole: &'a [u8],
    num_regexp: usize,
    num_runes: usize,
    repeats: i64,
    height: Option<HashMap<usize, usize>>,
    size: Option<HashMap<usize, i64>>,
}

/// Parses `s` exactly as Go's `syntax.Parse(s, syntax.Perl)` does.
pub(super) fn parse(s: &[u8]) -> PResult<Tree> {
    let mut p = Parser {
        nodes: Vec::new(),
        flags: PERL,
        stack: Vec::new(),
        free: Vec::new(),
        num_cap: 0,
        whole: s,
        num_regexp: 0,
        num_runes: 0,
        repeats: 0,
        height: None,
        size: None,
    };
    let root = p.run(s)?;
    Ok(Tree { nodes: p.nodes, root })
}

impl<'a> Parser<'a> {
    fn n(&self, i: usize) -> &Node {
        &self.nodes[i]
    }

    fn nm(&mut self, i: usize) -> &mut Node {
        &mut self.nodes[i]
    }

    fn new_regexp(&mut self, op: Op) -> usize {
        if let Some(i) = self.free.pop() {
            self.nodes[i] = Node::new(op);
            i
        } else {
            self.nodes.push(Node::new(op));
            self.num_regexp += 1;
            self.nodes.len() - 1
        }
    }

    fn reuse(&mut self, re: usize) {
        if let Some(h) = self.height.as_mut() {
            h.remove(&re);
        }
        self.free.push(re);
    }

    /// Errors raised by Go's parser through panics report the whole expression.
    fn limit_error(&self, code: &'static str) -> Error {
        Error::new(code, self.whole)
    }

    fn check_limits(&mut self, re: usize) -> PResult<()> {
        if self.num_runes > MAX_RUNES {
            return Err(self.limit_error(ERR_LARGE));
        }
        self.check_size(re)?;
        self.check_height(re)
    }

    fn check_size(&mut self, re: usize) -> PResult<()> {
        if self.size.is_none() {
            if self.repeats == 0 {
                self.repeats = 1;
            }
            if self.n(re).op == Op::Repeat {
                let mut n = self.n(re).max;
                if n == -1 {
                    n = self.n(re).min;
                }
                if n <= 0 {
                    n = 1;
                }
                if n > MAX_SIZE / self.repeats {
                    self.repeats = MAX_SIZE;
                } else {
                    self.repeats *= n;
                }
            }
            if (self.num_regexp as i64) < MAX_SIZE / self.repeats {
                return Ok(());
            }
            self.size = Some(HashMap::new());
            for i in 0..self.stack.len() {
                let s = self.stack[i];
                self.check_size(s)?;
            }
        }
        if self.calc_size(re, true) > MAX_SIZE {
            return Err(self.limit_error(ERR_LARGE));
        }
        Ok(())
    }

    fn calc_size(&mut self, re: usize, force: bool) -> i64 {
        if !force {
            if let Some(&s) = self.size.as_ref().and_then(|m| m.get(&re)) {
                return s;
            }
        }
        let node = self.n(re);
        let size: i64 = match node.op {
            Op::Literal => node.runes.len() as i64,
            Op::Capture | Op::Star => {
                let s0 = node.sub[0];
                2i64.wrapping_add(self.calc_size(s0, false))
            }
            Op::Plus | Op::Quest => {
                let s0 = node.sub[0];
                1i64.wrapping_add(self.calc_size(s0, false))
            }
            Op::Concat | Op::Alternate => {
                let subs = node.sub.clone();
                let is_alt = node.op == Op::Alternate;
                let mut size: i64 = 0;
                for &s in &subs {
                    size = size.wrapping_add(self.calc_size(s, false));
                }
                if is_alt && subs.len() > 1 {
                    size = size.wrapping_add(subs.len() as i64 - 1);
                }
                size
            }
            Op::Repeat => {
                let (min, max, s0) = (node.min, node.max, node.sub[0]);
                let sub = self.calc_size(s0, false);
                if max == -1 {
                    if min == 0 {
                        2i64.wrapping_add(sub)
                    } else {
                        1i64.wrapping_add(min.wrapping_mul(sub))
                    }
                } else {
                    max.wrapping_mul(sub).wrapping_add(max - min)
                }
            }
            _ => 0,
        };
        let size = size.max(1);
        if let Some(m) = self.size.as_mut() {
            m.insert(re, size);
        }
        size
    }

    fn check_height(&mut self, re: usize) -> PResult<()> {
        if self.num_regexp < MAX_HEIGHT {
            return Ok(());
        }
        if self.height.is_none() {
            self.height = Some(HashMap::new());
            for i in 0..self.stack.len() {
                let s = self.stack[i];
                self.check_height(s)?;
            }
        }
        if self.calc_height(re, true) > MAX_HEIGHT {
            return Err(self.limit_error(ERR_NESTING_DEPTH));
        }
        Ok(())
    }

    fn calc_height(&mut self, re: usize, force: bool) -> usize {
        if !force {
            if let Some(&h) = self.height.as_ref().and_then(|m| m.get(&re)) {
                return h;
            }
        }
        let mut h = 1;
        let subs = self.n(re).sub.clone();
        for s in subs {
            let hs = self.calc_height(s, false);
            if h < 1 + hs {
                h = 1 + hs;
            }
        }
        if let Some(m) = self.height.as_mut() {
            m.insert(re, h);
        }
        h
    }

    /// Go's `push`; returns `None` when the node was folded into the previous literal.
    fn push(&mut self, re: usize) -> PResult<Option<usize>> {
        self.num_runes += self.n(re).runes.len();
        let node = self.n(re);
        let is_class = node.op == Op::CharClass;
        let r: Vec<i32> = if node.runes.len() <= 4 { node.runes.clone() } else { Vec::new() };
        if is_class && r.len() == 2 && r[0] == r[1] {
            let r0 = r[0];
            if self.maybe_concat(r0, self.flags & !FOLD_CASE) {
                return Ok(None);
            }
            let flags = self.flags & !FOLD_CASE;
            let n = self.nm(re);
            n.op = Op::Literal;
            n.runes.truncate(1);
            n.flags = flags;
        } else if is_class
            && ((r.len() == 4
                && r[0] == r[1]
                && r[2] == r[3]
                && simple_fold(r[0]) == r[2]
                && simple_fold(r[2]) == r[0])
                || (r.len() == 2 && r[0] + 1 == r[1] && simple_fold(r[0]) == r[1] && simple_fold(r[1]) == r[0]))
        {
            let r0 = r[0];
            if self.maybe_concat(r0, self.flags | FOLD_CASE) {
                return Ok(None);
            }
            let flags = self.flags | FOLD_CASE;
            let n = self.nm(re);
            n.op = Op::Literal;
            n.runes.truncate(1);
            n.flags = flags;
        } else {
            self.maybe_concat(-1, 0);
        }
        self.stack.push(re);
        self.check_limits(re)?;
        Ok(Some(re))
    }

    /// Go's `maybeConcat`.
    fn maybe_concat(&mut self, r: i32, flags: u16) -> bool {
        let n = self.stack.len();
        if n < 2 {
            return false;
        }
        let re1 = self.stack[n - 1];
        let re2 = self.stack[n - 2];
        if self.n(re1).op != Op::Literal
            || self.n(re2).op != Op::Literal
            || self.n(re1).flags & FOLD_CASE != self.n(re2).flags & FOLD_CASE
        {
            return false;
        }
        let moved = self.n(re1).runes.clone();
        self.nm(re2).runes.extend_from_slice(&moved);
        if r >= 0 {
            let n1 = self.nm(re1);
            n1.runes = vec![r];
            n1.flags = flags;
            return true;
        }
        self.stack.truncate(n - 1);
        self.reuse(re1);
        false
    }

    fn literal(&mut self, mut r: i32) -> PResult<()> {
        let re = self.new_regexp(Op::Literal);
        self.nm(re).flags = self.flags;
        if self.flags & FOLD_CASE != 0 {
            r = min_fold_rune(r);
        }
        self.nm(re).runes = vec![r];
        self.push(re)?;
        Ok(())
    }

    fn op(&mut self, op: Op) -> PResult<usize> {
        let re = self.new_regexp(op);
        self.nm(re).flags = self.flags;
        // push only drops character classes, never the ops created here.
        Ok(self.push(re)?.unwrap_or(re))
    }

    /// Go's `repeat`; `before`, `after` and `last_repeat` are suffixes of the pattern.
    fn repeat(
        &mut self,
        op: Op,
        min: i64,
        max: i64,
        before: &'a [u8],
        mut after: &'a [u8],
        last_repeat: &'a [u8],
    ) -> PResult<&'a [u8]> {
        let mut flags = self.flags;
        if self.flags & PERL_X != 0 {
            if after.first() == Some(&b'?') {
                after = &after[1..];
                flags ^= NON_GREEDY;
            }
            if !last_repeat.is_empty() {
                return Err(Error::new(ERR_INVALID_REPEAT_OP, cut(last_repeat, after)));
            }
        }
        let Some(&sub) = self.stack.last() else {
            return Err(Error::new(ERR_MISSING_REPEAT_ARGUMENT, cut(before, after)));
        };
        if self.n(sub).op >= Op::LeftParen {
            return Err(Error::new(ERR_MISSING_REPEAT_ARGUMENT, cut(before, after)));
        }
        let re = self.new_regexp(op);
        {
            let n = self.nm(re);
            n.min = min;
            n.max = max;
            n.flags = flags;
            n.sub = vec![sub];
        }
        let top = self.stack.len() - 1;
        self.stack[top] = re;
        self.check_limits(re)?;
        if op == Op::Repeat && (min >= 2 || max >= 2) && !self.repeat_is_valid(re, 1000) {
            return Err(Error::new(ERR_INVALID_REPEAT_SIZE, cut(before, after)));
        }
        Ok(after)
    }

    fn repeat_is_valid(&self, re: usize, mut n: i64) -> bool {
        let node = self.n(re);
        if node.op == Op::Repeat {
            let mut m = node.max;
            if m == 0 {
                return true;
            }
            if m < 0 {
                m = node.min;
            }
            if m > n {
                return false;
            }
            if m > 0 {
                n /= m;
            }
        }
        node.sub.iter().all(|&s| self.repeat_is_valid(s, n))
    }

    fn pseudo_start(&self) -> usize {
        let mut i = self.stack.len();
        while i > 0 && self.n(self.stack[i - 1]).op < Op::LeftParen {
            i -= 1;
        }
        i
    }

    fn concat(&mut self) -> PResult<()> {
        self.maybe_concat(-1, 0);
        let i = self.pseudo_start();
        let subs = self.stack.split_off(i);
        if subs.is_empty() {
            let re = self.new_regexp(Op::EmptyMatch);
            self.push(re)?;
            return Ok(());
        }
        let re = self.collapse(subs, Op::Concat)?;
        self.push(re)?;
        Ok(())
    }

    fn alternate(&mut self) -> PResult<()> {
        let i = self.pseudo_start();
        let subs = self.stack.split_off(i);
        if let Some(&last) = subs.last() {
            self.clean_alt(last);
        }
        if subs.is_empty() {
            let re = self.new_regexp(Op::NoMatch);
            self.push(re)?;
            return Ok(());
        }
        let re = self.collapse(subs, Op::Alternate)?;
        self.push(re)?;
        Ok(())
    }

    fn clean_alt(&mut self, re: usize) {
        let n = self.nm(re);
        if n.op != Op::CharClass {
            return;
        }
        clean_class(&mut n.runes);
        let r = &n.runes;
        if r.len() == 2 && r[0] == 0 && r[1] == MAX_RUNE {
            n.runes.clear();
            n.op = Op::AnyChar;
        } else if r.len() == 4 && r[0] == 0 && r[1] == 9 && r[2] == 11 && r[3] == MAX_RUNE {
            n.runes.clear();
            n.op = Op::AnyCharNotNL;
        }
    }

    fn collapse(&mut self, subs: Vec<usize>, op: Op) -> PResult<usize> {
        if subs.len() == 1 {
            return Ok(subs[0]);
        }
        let re = self.new_regexp(op);
        let mut list = Vec::new();
        for sub in subs {
            if self.n(sub).op == op {
                let inner = self.n(sub).sub.clone();
                list.extend(inner);
                self.reuse(sub);
            } else {
                list.push(sub);
            }
        }
        if op == Op::Alternate {
            let list = self.factor(list)?;
            if list.len() == 1 {
                let only = list[0];
                self.nm(re).sub = list;
                self.reuse(re);
                return Ok(only);
            }
            self.nm(re).sub = list;
        } else {
            self.nm(re).sub = list;
        }
        Ok(re)
    }

    fn leading_string(&self, mut re: usize) -> (Vec<i32>, u16) {
        if self.n(re).op == Op::Concat && !self.n(re).sub.is_empty() {
            re = self.n(re).sub[0];
        }
        let n = self.n(re);
        if n.op != Op::Literal {
            return (Vec::new(), 0);
        }
        (n.runes.clone(), n.flags & FOLD_CASE)
    }

    fn remove_leading_string(&mut self, mut re: usize, n: usize) -> usize {
        if self.n(re).op == Op::Concat && !self.n(re).sub.is_empty() {
            let sub0 = self.n(re).sub[0];
            let sub = self.remove_leading_string(sub0, n);
            self.nm(re).sub[0] = sub;
            if self.n(sub).op == Op::EmptyMatch {
                self.reuse(sub);
                match self.n(re).sub.len() {
                    0 | 1 => {
                        let node = self.nm(re);
                        node.op = Op::EmptyMatch;
                        node.sub.clear();
                    }
                    2 => {
                        let old = re;
                        re = self.n(re).sub[1];
                        self.reuse(old);
                    }
                    _ => {
                        self.nm(re).sub.remove(0);
                    }
                }
            }
            return re;
        }
        let node = self.nm(re);
        if node.op == Op::Literal {
            let k = n.min(node.runes.len());
            node.runes.drain(..k);
            if node.runes.is_empty() {
                node.op = Op::EmptyMatch;
            }
        }
        re
    }

    fn leading_regexp(&self, re: usize) -> Option<usize> {
        let n = self.n(re);
        if n.op == Op::EmptyMatch {
            return None;
        }
        if n.op == Op::Concat && !n.sub.is_empty() {
            let sub = n.sub[0];
            if self.n(sub).op == Op::EmptyMatch {
                return None;
            }
            return Some(sub);
        }
        Some(re)
    }

    fn remove_leading_regexp(&mut self, re: usize, reuse: bool) -> usize {
        if self.n(re).op == Op::Concat && !self.n(re).sub.is_empty() {
            if reuse {
                let s0 = self.n(re).sub[0];
                self.reuse(s0);
            }
            self.nm(re).sub.remove(0);
            match self.n(re).sub.len() {
                0 => {
                    let node = self.nm(re);
                    node.op = Op::EmptyMatch;
                    node.sub.clear();
                }
                1 => {
                    let only = self.n(re).sub[0];
                    self.reuse(re);
                    return only;
                }
                _ => {}
            }
            return re;
        }
        if reuse {
            self.reuse(re);
        }
        self.new_regexp(Op::EmptyMatch)
    }

    /// Go's `Regexp.Equal`. Like Go it ignores the fold-case flag of literals, so the factoring
    /// in `factor` can merge `(?i:a)x|A\b` into `(?i:A)(?:x|\b)`; Go then matches "a".
    fn equal(&self, x: usize, y: usize) -> bool {
        let (a, b) = (self.n(x), self.n(y));
        if a.op != b.op {
            return false;
        }
        match a.op {
            Op::EndText => a.flags & WAS_DOLLAR == b.flags & WAS_DOLLAR,
            Op::Literal | Op::CharClass => a.runes == b.runes,
            Op::Alternate | Op::Concat => {
                a.sub.len() == b.sub.len() && a.sub.iter().zip(&b.sub).all(|(&p, &q)| self.equal(p, q))
            }
            Op::Star | Op::Plus | Op::Quest => {
                a.flags & NON_GREEDY == b.flags & NON_GREEDY && self.equal(a.sub[0], b.sub[0])
            }
            Op::Repeat => {
                a.flags & NON_GREEDY == b.flags & NON_GREEDY
                    && a.min == b.min
                    && a.max == b.max
                    && self.equal(a.sub[0], b.sub[0])
            }
            Op::Capture => a.cap == b.cap && a.name == b.name && self.equal(a.sub[0], b.sub[0]),
            _ => true,
        }
    }

    fn is_char_class(&self, re: usize) -> bool {
        let n = self.n(re);
        (n.op == Op::Literal && n.runes.len() == 1) || matches!(n.op, Op::CharClass | Op::AnyCharNotNL | Op::AnyChar)
    }

    fn match_rune(&self, re: usize, r: i32) -> bool {
        let n = self.n(re);
        match n.op {
            Op::Literal => n.runes.len() == 1 && n.runes[0] == r,
            Op::CharClass => n.runes.chunks_exact(2).any(|p| p[0] <= r && r <= p[1]),
            Op::AnyCharNotNL => r != i32::from(b'\n'),
            Op::AnyChar => true,
            _ => false,
        }
    }

    fn merge_char_class(&mut self, dst: usize, src: usize) {
        match self.n(dst).op {
            Op::AnyChar => {}
            Op::AnyCharNotNL => {
                if self.match_rune(src, i32::from(b'\n')) {
                    self.nm(dst).op = Op::AnyChar;
                }
            }
            Op::CharClass => {
                let s = self.n(src).clone();
                let d = self.nm(dst);
                if s.op == Op::Literal {
                    append_literal(&mut d.runes, s.runes[0], s.flags);
                } else {
                    append_class(&mut d.runes, &s.runes);
                }
            }
            Op::Literal => {
                let s = self.n(src).clone();
                let d = self.nm(dst);
                if s.runes[0] == d.runes[0] && s.flags == d.flags {
                    return;
                }
                d.op = Op::CharClass;
                let x = d.runes[0];
                let mut r = Vec::new();
                append_literal(&mut r, x, d.flags);
                append_literal(&mut r, s.runes[0], s.flags);
                d.runes = r;
            }
            _ => {}
        }
    }

    fn factor(&mut self, mut sub: Vec<usize>) -> PResult<Vec<usize>> {
        if sub.len() < 2 {
            return Ok(sub);
        }

        // Round 1: factor out common literal prefixes.
        let mut str: Vec<i32> = Vec::new();
        let mut strflags = 0;
        let mut start = 0;
        let mut out = Vec::new();
        for i in 0..=sub.len() {
            let (mut istr, mut iflags) = (Vec::new(), 0);
            if i < sub.len() {
                (istr, iflags) = self.leading_string(sub[i]);
                if iflags == strflags {
                    let same = str.iter().zip(&istr).take_while(|(a, b)| a == b).count();
                    if same > 0 {
                        str.truncate(same);
                        continue;
                    }
                }
            }
            if i == start {
            } else if i == start + 1 {
                out.push(sub[start]);
            } else {
                let prefix = self.new_regexp(Op::Literal);
                self.nm(prefix).flags = strflags;
                self.nm(prefix).runes = str.clone();
                for item in &mut sub[start..i] {
                    *item = self.remove_leading_string(*item, str.len());
                    self.check_limits(*item)?;
                }
                let suffix = self.collapse(sub[start..i].to_vec(), Op::Alternate)?;
                let re = self.new_regexp(Op::Concat);
                self.nm(re).sub = vec![prefix, suffix];
                out.push(re);
            }
            start = i;
            str = istr;
            strflags = iflags;
        }
        sub = out;

        // Round 2: factor out common simple prefixes.
        start = 0;
        let mut out = Vec::new();
        let mut first: Option<usize> = None;
        for i in 0..=sub.len() {
            let mut ifirst = None;
            if i < sub.len() {
                ifirst = self.leading_regexp(sub[i]);
                if let (Some(f), Some(fi)) = (first, ifirst) {
                    let fnode = self.n(f);
                    if self.equal(f, fi)
                        && (self.is_char_class(f)
                            || (fnode.op == Op::Repeat && fnode.min == fnode.max && self.is_char_class(fnode.sub[0])))
                    {
                        continue;
                    }
                }
            }
            if i == start {
            } else if i == start + 1 {
                out.push(sub[start]);
            } else {
                // A run of length >= 2 only forms when first is set.
                let prefix = first.unwrap_or(sub[start]);
                for (k, item) in sub[start..i].iter_mut().enumerate() {
                    // The prefix node itself came from the first member of the run.
                    *item = self.remove_leading_regexp(*item, k != 0);
                    self.check_limits(*item)?;
                }
                let suffix = self.collapse(sub[start..i].to_vec(), Op::Alternate)?;
                let re = self.new_regexp(Op::Concat);
                self.nm(re).sub = vec![prefix, suffix];
                out.push(re);
            }
            start = i;
            first = ifirst;
        }
        sub = out;

        // Round 3: collapse runs of single literals into character classes.
        start = 0;
        let mut out = Vec::new();
        for i in 0..=sub.len() {
            if i < sub.len() && self.is_char_class(sub[i]) {
                continue;
            }
            if i == start {
            } else if i == start + 1 {
                out.push(sub[start]);
            } else {
                let mut max = start;
                for j in start + 1..i {
                    let (a, b) = (self.n(sub[max]), self.n(sub[j]));
                    if a.op < b.op || (a.op == b.op && a.runes.len() < b.runes.len()) {
                        max = j;
                    }
                }
                sub.swap(start, max);
                for j in start + 1..i {
                    self.merge_char_class(sub[start], sub[j]);
                    self.reuse(sub[j]);
                }
                self.clean_alt(sub[start]);
                out.push(sub[start]);
            }
            if i < sub.len() {
                out.push(sub[i]);
            }
            start = i + 1;
        }
        sub = out;

        // Round 4: collapse runs of empty matches into a single empty match.
        let mut out = Vec::new();
        for i in 0..sub.len() {
            if i + 1 < sub.len() && self.n(sub[i]).op == Op::EmptyMatch && self.n(sub[i + 1]).op == Op::EmptyMatch {
                continue;
            }
            out.push(sub[i]);
        }
        Ok(out)
    }

    fn swap_vertical_bar(&mut self) -> bool {
        let n = self.stack.len();
        if n >= 3
            && self.n(self.stack[n - 2]).op == Op::VerticalBar
            && self.is_char_class(self.stack[n - 1])
            && self.is_char_class(self.stack[n - 3])
        {
            let mut re1 = self.stack[n - 1];
            let mut re3 = self.stack[n - 3];
            if self.n(re1).op > self.n(re3).op {
                std::mem::swap(&mut re1, &mut re3);
                self.stack[n - 3] = re3;
            }
            self.merge_char_class(re3, re1);
            self.reuse(re1);
            self.stack.truncate(n - 1);
            return true;
        }
        if n >= 2 {
            let re1 = self.stack[n - 1];
            let re2 = self.stack[n - 2];
            if self.n(re2).op == Op::VerticalBar {
                if n >= 3 {
                    self.clean_alt(self.stack[n - 3]);
                }
                self.stack[n - 2] = re1;
                self.stack[n - 1] = re2;
                return true;
            }
        }
        false
    }

    fn parse_vertical_bar(&mut self) -> PResult<()> {
        self.concat()?;
        if !self.swap_vertical_bar() {
            self.op(Op::VerticalBar)?;
        }
        Ok(())
    }

    fn parse_right_paren(&mut self) -> PResult<()> {
        self.concat()?;
        if self.swap_vertical_bar() {
            self.stack.pop();
        }
        self.alternate()?;
        let n = self.stack.len();
        if n < 2 {
            return Err(Error::new(ERR_UNEXPECTED_PAREN, self.whole));
        }
        let re1 = self.stack[n - 1];
        let re2 = self.stack[n - 2];
        self.stack.truncate(n - 2);
        if self.n(re2).op != Op::LeftParen {
            return Err(Error::new(ERR_UNEXPECTED_PAREN, self.whole));
        }
        self.flags = self.n(re2).flags;
        if self.n(re2).cap == 0 {
            self.push(re1)?;
        } else {
            let node = self.nm(re2);
            node.op = Op::Capture;
            node.sub = vec![re1];
            self.push(re2)?;
        }
        Ok(())
    }

    fn run(&mut self, s: &'a [u8]) -> PResult<usize> {
        let mut t: &'a [u8] = s;
        let mut last_repeat: &'a [u8] = b"";
        while let Some(&b0) = t.first() {
            let mut repeat: &'a [u8] = b"";
            match b0 {
                b'(' => {
                    if self.flags & PERL_X != 0 && t.get(1) == Some(&b'?') {
                        t = self.parse_perl_flags(t)?;
                    } else {
                        self.num_cap += 1;
                        let re = self.op(Op::LeftParen)?;
                        self.nm(re).cap = self.num_cap;
                        t = &t[1..];
                    }
                }
                b'|' => {
                    self.parse_vertical_bar()?;
                    t = &t[1..];
                }
                b')' => {
                    self.parse_right_paren()?;
                    t = &t[1..];
                }
                b'^' => {
                    if self.flags & ONE_LINE != 0 {
                        self.op(Op::BeginText)?;
                    } else {
                        self.op(Op::BeginLine)?;
                    }
                    t = &t[1..];
                }
                b'$' => {
                    if self.flags & ONE_LINE != 0 {
                        let re = self.op(Op::EndText)?;
                        self.nm(re).flags |= WAS_DOLLAR;
                    } else {
                        self.op(Op::EndLine)?;
                    }
                    t = &t[1..];
                }
                b'.' => {
                    if self.flags & DOT_NL != 0 {
                        self.op(Op::AnyChar)?;
                    } else {
                        self.op(Op::AnyCharNotNL)?;
                    }
                    t = &t[1..];
                }
                b'[' => {
                    t = self.parse_class(t)?;
                }
                b'*' | b'+' | b'?' => {
                    let before = t;
                    let op = match b0 {
                        b'*' => Op::Star,
                        b'+' => Op::Plus,
                        _ => Op::Quest,
                    };
                    let after = self.repeat(op, 0, 0, before, &t[1..], last_repeat)?;
                    repeat = before;
                    t = after;
                }
                b'{' => {
                    let before = t;
                    match parse_repeat(t) {
                        None => {
                            // An unparsable repeat is a literal {.
                            self.literal(i32::from(b'{'))?;
                            t = &t[1..];
                        }
                        Some((min, max, after)) => {
                            if !(0..=1000).contains(&min) || max > 1000 || (max >= 0 && min > max) {
                                return Err(Error::new(ERR_INVALID_REPEAT_SIZE, cut(before, after)));
                            }
                            let after = self.repeat(Op::Repeat, min, max, before, after, last_repeat)?;
                            repeat = before;
                            t = after;
                        }
                    }
                }
                b'\\' => {
                    t = self.parse_backslash(t)?;
                }
                _ => {
                    let (c, rest) = next_rune(t)?;
                    self.literal(c)?;
                    t = rest;
                }
            }
            last_repeat = repeat;
        }

        self.concat()?;
        if self.swap_vertical_bar() {
            self.stack.pop();
        }
        self.alternate()?;
        if self.stack.len() != 1 {
            return Err(Error::new(ERR_MISSING_PAREN, s));
        }
        Ok(self.stack[0])
    }

    /// Handles a backslash at the start of `t` outside a class; returns the remaining input.
    fn parse_backslash(&mut self, t: &'a [u8]) -> PResult<&'a [u8]> {
        if self.flags & PERL_X != 0 && t.len() >= 2 {
            match t[1] {
                b'A' => {
                    self.op(Op::BeginText)?;
                    return Ok(&t[2..]);
                }
                b'b' => {
                    self.op(Op::WordBoundary)?;
                    return Ok(&t[2..]);
                }
                b'B' => {
                    self.op(Op::NoWordBoundary)?;
                    return Ok(&t[2..]);
                }
                b'C' => return Err(Error::new(ERR_INVALID_ESCAPE, &t[..2])),
                b'Q' => {
                    let body = &t[2..];
                    let (mut lit, rest): (&[u8], &'a [u8]) = match find(body, b"\\E") {
                        Some(i) => (&body[..i], &body[i + 2..]),
                        None => (body, b""),
                    };
                    while !lit.is_empty() {
                        let (c, r) = next_rune(lit)?;
                        self.literal(c)?;
                        lit = r;
                    }
                    return Ok(rest);
                }
                b'z' => {
                    self.op(Op::EndText)?;
                    return Ok(&t[2..]);
                }
                _ => {}
            }
        }

        let re = self.new_regexp(Op::CharClass);
        self.nm(re).flags = self.flags;

        if t.len() >= 2 && (t[1] == b'p' || t[1] == b'P') {
            let mut r = Vec::new();
            if let Some(rest) = self.parse_unicode_class(t, &mut r)? {
                self.nm(re).runes = r;
                self.push(re)?;
                return Ok(rest);
            }
        }
        let mut r = Vec::new();
        if let Some(rest) = self.parse_perl_class_escape(t, &mut r) {
            self.nm(re).runes = r;
            self.push(re)?;
            return Ok(rest);
        }
        self.reuse(re);

        let (c, rest) = parse_escape(t)?;
        self.literal(c)?;
        Ok(rest)
    }

    fn parse_perl_flags(&mut self, s: &'a [u8]) -> PResult<&'a [u8]> {
        let mut t = s;
        let starts_with_p = t.len() > 4 && t[2] == b'P' && t[3] == b'<';
        let starts_with_name = t.len() > 3 && t[2] == b'<';
        if starts_with_p || starts_with_name {
            let expr_start = if starts_with_name { 3 } else { 4 };
            let Some(end) = t.iter().position(|&b| b == b'>') else {
                check_utf8(t)?;
                return Err(Error::new(ERR_INVALID_NAMED_CAPTURE, s));
            };
            let capture = &t[..=end];
            let name = t.get(expr_start..end).unwrap_or(b"");
            check_utf8(name)?;
            if !is_valid_capture_name(name) {
                return Err(Error::new(ERR_INVALID_NAMED_CAPTURE, capture));
            }
            self.num_cap += 1;
            let re = self.op(Op::LeftParen)?;
            self.nm(re).cap = self.num_cap;
            self.nm(re).name = name.to_vec();
            return Ok(&t[end + 1..]);
        }

        t = &t[2..];
        let mut flags = self.flags;
        let mut sign = 1;
        let mut saw_flag = false;
        while !t.is_empty() {
            let (c, rest) = next_rune(t)?;
            t = rest;
            match u8::try_from(c).unwrap_or(0) {
                b'i' => {
                    flags |= FOLD_CASE;
                    saw_flag = true;
                }
                b'm' => {
                    flags &= !ONE_LINE;
                    saw_flag = true;
                }
                b's' => {
                    flags |= DOT_NL;
                    saw_flag = true;
                }
                b'U' => {
                    flags |= NON_GREEDY;
                    saw_flag = true;
                }
                b'-' => {
                    if sign < 0 {
                        break;
                    }
                    sign = -1;
                    // Invert so that the set operations above clear flags instead.
                    flags = !flags;
                    saw_flag = false;
                }
                b':' | b')' => {
                    if sign < 0 {
                        if !saw_flag {
                            break;
                        }
                        flags = !flags;
                    }
                    if c == i32::from(b':') {
                        self.op(Op::LeftParen)?;
                    }
                    self.flags = flags;
                    return Ok(t);
                }
                _ => break,
            }
        }
        Err(Error::new(ERR_INVALID_PERL_OP, cut(s, t)))
    }

    fn append_group(&self, r: &mut Vec<i32>, g: &CharGroup) {
        if self.flags & FOLD_CASE == 0 {
            if g.sign < 0 {
                append_negated_class(r, g.class);
            } else {
                append_class(r, g.class);
            }
        } else {
            let mut tmp = Vec::new();
            append_folded_class(&mut tmp, g.class);
            clean_class(&mut tmp);
            if g.sign < 0 {
                append_negated_class(r, &tmp);
            } else {
                append_class(r, &tmp);
            }
        }
    }

    fn parse_perl_class_escape(&self, s: &'a [u8], r: &mut Vec<i32>) -> Option<&'a [u8]> {
        if self.flags & PERL_X == 0 || s.len() < 2 || s[0] != b'\\' {
            return None;
        }
        let g = perl_group(&s[..2])?;
        self.append_group(r, &g);
        Some(&s[2..])
    }

    fn parse_named_class(&self, s: &'a [u8], r: &mut Vec<i32>) -> PResult<Option<&'a [u8]>> {
        if s.len() < 2 || s[0] != b'[' || s[1] != b':' {
            return Ok(None);
        }
        let Some(i) = find(&s[2..], b":]") else {
            return Ok(None);
        };
        let i = i + 2;
        let (name, rest) = (&s[..i + 2], &s[i + 2..]);
        let Some(g) = posix_group(name) else {
            return Err(Error::new(ERR_INVALID_CHAR_RANGE, name));
        };
        self.append_group(r, &g);
        Ok(Some(rest))
    }

    fn parse_unicode_class(&self, s: &'a [u8], r: &mut Vec<i32>) -> PResult<Option<&'a [u8]>> {
        if self.flags & UNICODE_GROUPS == 0 || s.len() < 2 || s[0] != b'\\' || (s[1] != b'p' && s[1] != b'P') {
            return Ok(None);
        }
        let mut sign = if s[1] == b'P' { -1 } else { 1 };
        let (c, mut t) = next_rune(&s[2..])?;
        let (seq, mut name): (&[u8], &[u8]);
        if c != i32::from(b'{') {
            seq = cut(s, t);
            name = &seq[2..];
        } else {
            let Some(end) = s.iter().position(|&b| b == b'}') else {
                check_utf8(s)?;
                return Err(Error::new(ERR_INVALID_CHAR_RANGE, s));
            };
            seq = &s[..=end];
            t = &s[end + 1..];
            name = &s[3..end];
            check_utf8(name)?;
        }
        if let Some(rest) = name.strip_prefix(b"^") {
            sign = -sign;
            name = rest;
        }
        let Some((tab, fold, tsign)) = unicode_table(name) else {
            return Err(Error::new(ERR_INVALID_CHAR_RANGE, seq));
        };
        if tsign < 0 {
            sign = -sign;
        }
        match fold {
            Some(fold) if self.flags & FOLD_CASE != 0 => {
                let mut tmp = Vec::new();
                append_table(&mut tmp, tab);
                append_table(&mut tmp, fold);
                clean_class(&mut tmp);
                if sign > 0 {
                    append_class(r, &tmp);
                } else {
                    append_negated_class(r, &tmp);
                }
            }
            _ => {
                if sign > 0 {
                    append_table(r, tab);
                } else {
                    append_negated_table(r, tab);
                }
            }
        }
        Ok(Some(t))
    }

    fn parse_class(&mut self, s: &'a [u8]) -> PResult<&'a [u8]> {
        let mut t = &s[1..];
        let re = self.new_regexp(Op::CharClass);
        self.nm(re).flags = self.flags;
        let mut class = Vec::new();

        let mut sign = 1;
        if t.first() == Some(&b'^') {
            sign = -1;
            t = &t[1..];
            // Add \n so that the negation below excludes it.
            if self.flags & CLASS_NL == 0 {
                class.push(i32::from(b'\n'));
                class.push(i32::from(b'\n'));
            }
        }

        let mut first = true;
        while t.first() != Some(&b']') || first {
            if t.first() == Some(&b'-') && self.flags & PERL_X == 0 && !first && (t.len() == 1 || t[1] != b']') {
                let (_, size) = decode_rune(&t[1..]);
                return Err(Error::new(ERR_INVALID_CHAR_RANGE, &t[..1 + size]));
            }
            first = false;

            if t.len() > 2 && t[0] == b'[' && t[1] == b':' {
                if let Some(nt) = self.parse_named_class(t, &mut class)? {
                    t = nt;
                    continue;
                }
            }

            if let Some(nt) = self.parse_unicode_class(t, &mut class)? {
                t = nt;
                continue;
            }

            if let Some(nt) = self.parse_perl_class_escape(t, &mut class) {
                t = nt;
                continue;
            }

            let rng = t;
            let (lo, rest) = parse_class_char(t, s)?;
            t = rest;
            let mut hi = lo;
            if t.len() >= 2 && t[0] == b'-' && t[1] != b']' {
                t = &t[1..];
                let (h, rest) = parse_class_char(t, s)?;
                hi = h;
                t = rest;
                if hi < lo {
                    return Err(Error::new(ERR_INVALID_CHAR_RANGE, cut(rng, t)));
                }
            }
            if self.flags & FOLD_CASE == 0 {
                append_range(&mut class, lo, hi);
            } else {
                append_folded_range(&mut class, lo, hi);
            }
        }
        t = &t[1..];

        clean_class(&mut class);
        if sign < 0 {
            class = negate_class(&class);
        }
        self.nm(re).runes = class;
        self.push(re)?;
        Ok(t)
    }
}

/// Go's `isValidCaptureName`: `[A-Za-z0-9_]+`.
fn is_valid_capture_name(name: &[u8]) -> bool {
    !name.is_empty() && name.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric())
}

/// Go's `parseInt`: `None` when there is no number, `-1` for numbers that are too large.
fn parse_int(s: &[u8]) -> Option<(i64, &[u8])> {
    if !s.first()?.is_ascii_digit() {
        return None;
    }
    if s.len() >= 2 && s[0] == b'0' && s[1].is_ascii_digit() {
        return None;
    }
    let len = s.iter().take_while(|b| b.is_ascii_digit()).count();
    let mut n: i64 = 0;
    for &d in &s[..len] {
        if n >= 100_000_000 {
            n = -1;
            break;
        }
        n = n * 10 + i64::from(d - b'0');
    }
    Some((n, &s[len..]))
}

/// Go's `parseRepeat`: (min, max, rest); max is -1 for `{n,}`, min is -1 when too large.
fn parse_repeat(s: &[u8]) -> Option<(i64, i64, &[u8])> {
    let s = s.strip_prefix(b"{")?;
    let (mut min, mut s) = parse_int(s)?;
    let max;
    if *s.first()? != b',' {
        max = min;
    } else {
        s = &s[1..];
        if *s.first()? == b'}' {
            max = -1;
        } else {
            let (m, rest) = parse_int(s)?;
            s = rest;
            max = m;
            if max < 0 {
                min = -1;
            }
        }
    }
    let rest = s.strip_prefix(b"}")?;
    Some((min, max, rest))
}

/// Go's `parseEscape`.
fn parse_escape(s: &[u8]) -> PResult<(i32, &[u8])> {
    let t = &s[1..];
    if t.is_empty() {
        return Err(Error::new(ERR_TRAILING_BACKSLASH, b""));
    }
    let (c, mut t) = next_rune(t)?;
    match u8::try_from(c).ok() {
        Some(b'1'..=b'7') | Some(b'0') => {
            // A single non-zero digit is a backreference; not supported.
            let octal_next = matches!(t.first(), Some(b'0'..=b'7'));
            if c != i32::from(b'0') && !octal_next {
                return Err(Error::new(ERR_INVALID_ESCAPE, cut(s, t)));
            }
            let mut r = c - i32::from(b'0');
            for _ in 1..3 {
                match t.first() {
                    Some(&d @ b'0'..=b'7') => {
                        r = r * 8 + i32::from(d - b'0');
                        t = &t[1..];
                    }
                    _ => break,
                }
            }
            return Ok((r, t));
        }
        Some(b'x') => {
            if t.is_empty() {
                return Err(Error::new(ERR_INVALID_ESCAPE, cut(s, t)));
            }
            let (c, rest) = next_rune(t)?;
            t = rest;
            if c == i32::from(b'{') {
                let mut nhex = 0;
                let mut r: i32 = 0;
                loop {
                    if t.is_empty() {
                        return Err(Error::new(ERR_INVALID_ESCAPE, cut(s, t)));
                    }
                    let (c, rest) = next_rune(t)?;
                    t = rest;
                    if c == i32::from(b'}') {
                        break;
                    }
                    let v = unhex(c);
                    if v < 0 {
                        return Err(Error::new(ERR_INVALID_ESCAPE, cut(s, t)));
                    }
                    r = r * 16 + v;
                    if r > MAX_RUNE {
                        return Err(Error::new(ERR_INVALID_ESCAPE, cut(s, t)));
                    }
                    nhex += 1;
                }
                if nhex == 0 {
                    return Err(Error::new(ERR_INVALID_ESCAPE, cut(s, t)));
                }
                return Ok((r, t));
            }
            let x = unhex(c);
            let (c, rest) = next_rune(t)?;
            t = rest;
            let y = unhex(c);
            if x < 0 || y < 0 {
                return Err(Error::new(ERR_INVALID_ESCAPE, cut(s, t)));
            }
            return Ok((x * 16 + y, t));
        }
        Some(b'a') => return Ok((7, t)),
        Some(b'f') => return Ok((12, t)),
        Some(b'n') => return Ok((10, t)),
        Some(b'r') => return Ok((13, t)),
        Some(b't') => return Ok((9, t)),
        Some(b'v') => return Ok((11, t)),
        _ => {
            if c < 0x80 && !is_alnum(c) {
                // Escaped punctuation is always itself.
                return Ok((c, t));
            }
        }
    }
    Err(Error::new(ERR_INVALID_ESCAPE, cut(s, t)))
}

/// Go's `parseClassChar`.
fn parse_class_char<'b>(s: &'b [u8], whole_class: &[u8]) -> PResult<(i32, &'b [u8])> {
    if s.is_empty() {
        return Err(Error::new(ERR_MISSING_BRACKET, whole_class));
    }
    if s[0] == b'\\' {
        return parse_escape(s);
    }
    next_rune(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(p: &str) -> (Tree, Node) {
        let t = parse(p.as_bytes()).expect("parses");
        let n = t.nodes[t.root].clone();
        (t, n)
    }

    #[test]
    fn literals_are_concatenated() {
        let (_, n) = root("abc");
        assert_eq!(n.op, Op::Literal);
        assert_eq!(n.runes, vec![97, 98, 99]);
    }

    #[test]
    fn fold_case_literal_uses_min_fold_rune() {
        let (_, n) = root("(?i)k");
        assert_eq!(n.op, Op::Literal);
        assert_eq!(n.runes, vec![i32::from(b'K')]);
        assert_ne!(n.flags & FOLD_CASE, 0);
    }

    #[test]
    fn case_pair_class_becomes_fold_literal() {
        let (_, n) = root("[Aa]");
        assert_eq!(n.op, Op::Literal);
        assert_ne!(n.flags & FOLD_CASE, 0);
    }

    #[test]
    fn alternation_factors_prefix_and_classes() {
        // Go: abc|abd parses to Concat(Literal "ab", CharClass [c-d]).
        let (t, n) = root("abc|abd");
        assert_eq!(n.op, Op::Concat);
        let a = &t.nodes[n.sub[0]];
        let b = &t.nodes[n.sub[1]];
        assert_eq!((a.op, a.runes.clone()), (Op::Literal, vec![97, 98]));
        assert_eq!((b.op, b.runes.clone()), (Op::CharClass, vec![99, 100]));
    }

    #[test]
    fn dot_and_negated_newline() {
        assert_eq!(root(".").1.op, Op::AnyCharNotNL);
        assert_eq!(root("(?s).").1.op, Op::AnyChar);
        assert_eq!(root("[^a]").1.runes, vec![0, 0x60, 0x62, MAX_RUNE]);
    }

    #[test]
    fn dollar_and_caret_depend_on_multiline() {
        assert_eq!(root("^").1.op, Op::BeginText);
        assert_eq!(root("$").1.op, Op::EndText);
        assert_eq!(root("(?m)^").1.op, Op::BeginLine);
        assert_eq!(root("(?m)$").1.op, Op::EndLine);
    }

    #[test]
    fn repeat_parsing() {
        assert_eq!(parse_repeat(b"{2,5}x"), Some((2, 5, &b"x"[..])));
        assert_eq!(parse_repeat(b"{2,}"), Some((2, -1, &b""[..])));
        assert_eq!(parse_repeat(b"{3}"), Some((3, 3, &b""[..])));
        assert_eq!(parse_repeat(b"{,3}"), None);
        assert_eq!(parse_repeat(b"{01}"), None);
        assert_eq!(parse_repeat(b"{1,1000000000}"), Some((-1, -1, &b""[..])));
    }

    #[test]
    fn escapes() {
        assert_eq!(parse_escape(b"\\101").map(|r| r.0), Ok(65));
        assert_eq!(parse_escape(b"\\0").map(|r| r.0), Ok(0));
        assert_eq!(parse_escape(b"\\x{10FFFF}").map(|r| r.0), Ok(MAX_RUNE));
        assert_eq!(parse_escape(b"\\x41").map(|r| r.0), Ok(65));
        assert_eq!(parse_escape(b"\\_").map(|r| r.0), Ok(95));
        assert_eq!(parse_escape(b"\\x{12"), Err(Error::new(ERR_INVALID_ESCAPE, b"\\x{12")));
    }

    #[test]
    fn canonical_names() {
        assert_eq!(canonical_name(b"cased_letter"), b"Casedletter");
        assert_eq!(canonical_name(b"GREEK"), b"Greek");
        assert_eq!(canonical_name(b"L"), b"L");
    }

    #[test]
    fn decode_like_go() {
        assert_eq!(decode_rune(b""), (RUNE_ERROR, 0));
        assert_eq!(decode_rune(b"\xff"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune(b"\xe2\x82"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune(b"\xed\xa0\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune("é".as_bytes()), (0xE9, 2));
    }

    #[test]
    fn unicode_classes() {
        let (_, n) = root(r"\p{Greek}");
        assert_eq!(n.op, Op::CharClass);
        assert!(n.runes.chunks(2).any(|p| p[0] <= 0x3C3 && 0x3C3 <= p[1]));
        assert!(parse(br"\p{Old_Italic}").is_err());
        assert!(parse(br"\p{old italic}").is_err());
        assert!(parse(br"\p{Cased_Letter}").is_ok());
    }
}
