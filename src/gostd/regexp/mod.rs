//! Emulation of Go's `regexp.Compile` followed by `MatchString`/`Match` (boolean matching only).
//!
//! The pattern is parsed by a port of Go's regexp/syntax parser (module `parse`), which yields Go's
//! exact errors, and the resulting syntax tree is translated (module `translate`) into a pattern for
//! the `regex` crate that matches exactly what Go matches.
//!
//! Go reads each byte of invalid UTF-8 in the subject as U+FFFD of width 1. A boolean match does
//! not depend on widths, and U+FFFD is neither a word character nor a newline, so the subject is
//! rewritten with each such byte replaced by U+FFFD before matching. (Go's literal prefix search
//! deliberately stops at U+FFFD, so this holds for literal U+FFFD in patterns too.)
//!
//! Matching uses the `regex` crate's UTF-8 `Regex`, whose searches only start at (and, for empty
//! matches, only report) code point boundaries; this keeps Go's ASCII `\B`, written
//! `(?-u:\B)`, from matching between the bytes of a multi-byte character. The crate's
//! earliest-match searches (`is_match`) mishandle such split empty matches (for example
//! `(?-u:\B)|\x{FFFD}` reports no match in "9\u{FFFD}B"), so patterns containing `\B` are
//! matched with a leftmost search (`find`) instead, which skips them correctly.
//!
//! Known divergence: a pattern Go accepts can still exceed what the `regex` crate is willing to
//! build (its compiled program is limited to a few GiB); `compile` then returns an error whose
//! text starts with `regexp: pattern too large for the regex engine`, which Go never produces.
//! Patterns near Go's size limits (say twenty thousand `\pL`) are accepted, but the `regex` crate
//! takes seconds to compile them.
//!
//! Follows the behaviour of Go's standard library regexp package (The Go Authors, BSD-style licence).

mod parse;
mod tables;
mod translate;
mod unicode;

use std::borrow::Cow;

/// A compiled regular expression with Go's matching semantics.
#[derive(Clone, Debug)]
pub struct Regexp {
    expr: String,
    re: regex::Regex,
    use_find: bool,
}

/// Prefix of the error returned when the `regex` crate cannot build a pattern Go accepts.
pub const ENGINE_LIMIT_ERROR: &str = "regexp: pattern too large for the regex engine";

/// Go's `regexp.Compile(pattern)`.
///
/// The error is Go's `err.Error()` text. Go's message for invalid UTF-8 quotes the raw pattern
/// bytes; those are converted lossily here (use [`compile_bytes`] for the exact bytes).
pub fn compile(pattern: &[u8]) -> Result<Regexp, String> {
    compile_bytes(pattern).map_err(|e| String::from_utf8_lossy(&e).into_owned())
}

/// Like [`compile`], but returns Go's error text as the exact bytes Go produces.
pub fn compile_bytes(pattern: &[u8]) -> Result<Regexp, Vec<u8>> {
    let tree = parse::parse(pattern).map_err(|e| e.message())?;
    let translated = translate::translate(&tree);
    let re = regex::RegexBuilder::new(&translated.pattern)
        .unicode(true)
        .size_limit(1 << 31)
        .dfa_size_limit(1 << 25)
        .nest_limit(100_000)
        .build()
        .map_err(|e| format!("{ENGINE_LIMIT_ERROR}: {e}").into_bytes())?;
    // A successfully parsed pattern has been fully decoded as UTF-8 by the parser.
    let expr = String::from_utf8_lossy(pattern).into_owned();
    Ok(Regexp { expr, re, use_find: translated.has_non_boundary })
}

impl Regexp {
    /// Go's `re.Match(subject)` / `re.MatchString(subject)`.
    pub fn is_match(&self, subject: &[u8]) -> bool {
        let subject = go_decode(subject);
        if self.use_find {
            self.re.find(&subject).is_some()
        } else {
            self.re.is_match(&subject)
        }
    }

    /// Go's `re.String()`: the source text of the pattern.
    pub fn as_str(&self) -> &str {
        &self.expr
    }

    /// The source text of the pattern as bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.expr.as_bytes()
    }
}

/// Decodes the subject the way Go does: every byte that does not start a valid UTF-8 sequence
/// is read as U+FFFD (of width 1, so a truncated sequence yields one U+FFFD per byte).
fn go_decode(subject: &[u8]) -> Cow<'_, str> {
    let mut rest = match std::str::from_utf8(subject) {
        Ok(s) => return Cow::Borrowed(s),
        Err(_) => subject,
    };
    let mut out = String::with_capacity(subject.len() + 16);
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                break;
            }
            Err(e) => {
                let (good, bad) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(good).unwrap_or_default());
                out.push('\u{FFFD}');
                rest = bad.get(1..).unwrap_or_default();
            }
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values below come from Go 1.25.5's regexp.Compile and MatchString.
    const ERRORS: &[(&[u8], &[u8])] = &[
        (b"(a", b"error parsing regexp: missing closing ): `(a`"),
        (b"a**", b"error parsing regexp: invalid nested repetition operator: `**`"),
        (b"*", b"error parsing regexp: missing argument to repetition operator: `*`"),
        (b"[z-a]", b"error parsing regexp: invalid character class range: `z-a`"),
        (b"\\8", b"error parsing regexp: invalid escape sequence: `\\8`"),
        (b"a{1001}", b"error parsing regexp: invalid repeat count: `{1001}`"),
        (b"(?P<n!>a)", b"error parsing regexp: invalid named capture: `(?P<n!>`"),
        (b"(?z)", b"error parsing regexp: invalid or unsupported Perl syntax: `(?z`"),
        (b"a\\", b"error parsing regexp: trailing backslash at end of expression: ``"),
        (b"a\xff", b"error parsing regexp: invalid UTF-8: `\xff`"),
        (b"[a", b"error parsing regexp: missing closing ]: `[a`"),
        (b"a)", b"error parsing regexp: unexpected ): `a)`"),
        (b"\\pX", b"error parsing regexp: invalid character class range: `\\pX`"),
        (b"\\p{Foo}", b"error parsing regexp: invalid character class range: `\\p{Foo}`"),
        (b"[[:foo:]]", b"error parsing regexp: invalid character class range: `[:foo:]`"),
        (b"\\x{110000}", b"error parsing regexp: invalid escape sequence: `\\x{110000`"),
        (b"(?i", b"error parsing regexp: invalid or unsupported Perl syntax: `(?i`"),
        (b"\\C", b"error parsing regexp: invalid escape sequence: `\\C`"),
        (b"x{2}{3}", b"error parsing regexp: invalid nested repetition operator: `{2}{3}`"),
        (b"a{2,1}", b"error parsing regexp: invalid repeat count: `{2,1}`"),
        (b"(?<>a)", b"error parsing regexp: invalid named capture: `(?<>`"),
        (b"((a{100}){100}){100}", b"error parsing regexp: invalid repeat count: `{100}`"),
        (b"\\1", b"error parsing regexp: invalid escape sequence: `\\1`"),
        (b"(?P<n>", b"error parsing regexp: missing closing ): `(?P<n>`"),
        (b"[\\", b"error parsing regexp: trailing backslash at end of expression: ``"),
        (b"\\x4", b"error parsing regexp: invalid escape sequence: `\\x4`"),
        (b"(?-)", b"error parsing regexp: invalid or unsupported Perl syntax: `(?-)`"),
        (b"(?--i)", b"error parsing regexp: invalid or unsupported Perl syntax: `(?--`"),
        (b"(?P=n)", b"error parsing regexp: invalid or unsupported Perl syntax: `(?P`"),
        (b"\\Z", b"error parsing regexp: invalid escape sequence: `\\Z`"),
        (b"\\p{^", b"error parsing regexp: invalid character class range: `\\p{^`"),
        (b"a|*", b"error parsing regexp: missing argument to repetition operator: `*`"),
        (b"()+*", b"error parsing regexp: invalid nested repetition operator: `+*`"),
    ];

    const MATCHES: &[(&[u8], &[u8], bool)] = &[
        (b"(?i)k", b"\xe2\x84\xaa", true),
        (b"(?i)stra\xc3\x9fe", b"STRASSE", false),
        (b"(?i)\xcf\x83", b"\xcf\x82", true),
        (b"\\w", b"\xc3\xa9", false),
        (b"\\bx\\b", b"\xc3\xa9x\xc3\xa9", true),
        (b"\\Bx", b"\xc3\xa9x", false),
        (b"^a$", b"a\n", false),
        (b"(?m)^a$", b"b\na\n", true),
        (b"a$", b"a\n", false),
        (b".", b"\n", false),
        (b"(?s).", b"\n", true),
        (b"[^a]", b"\n", true),
        (b"^.$", b"\xff", true),
        (b"^..$", b"\xe2\x82", true),
        (b"\\x{FFFD}", b"\xff", true),
        (b"^[^a]$", b"\xc0", true),
        (b"\\pL", b"\xe6\x97\xa5", true),
        (b"\\p{Greek}", b"\xce\xa9", true),
        (b"(?i)\\p{Lu}", b"a", true),
        (b"\\PL", b"a", false),
        (b"[[:^alpha:]]", b"\xc3\xa9", true),
        (b"(?i)[k]", b"\xe2\x84\xaa", true),
        (b"\\Qa.b\\E+", b"a.bb", true),
        (b"\\101", b"A", true),
        (b"(?U)a+", b"aa", true),
        (b"(?i:A)a", b"aA", false),
        (b"(?i)A(?-i)a", b"aA", false),
        (b"\\x{D800}", b"\xed\xa0\x80", false),
        (b"[\\x{D7FF}-\\x{E000}]", b"\xee\xb4\x80", false),
        (b"\\z", b"", true),
        (b"\\A\\z", b"x", false),
        (b"(?m)$", b"a", true),
        (b"x*", b"", true),
        (b"[^\\x00-\\x{10FFFF}]", b"a", false),
        (b"\\p{Assigned}", b"\xf3\xa0\x80\x80", false),
        (b"\\p{Any}", b"\xff", true),
        (b"(?i)\xc7\x85", b"\xc7\x86", true),
        (b"\\S", b"\xff", true),
        (b"(?i)\xc5\xbf", b"S", true),
        (b"[[:word:]]+$", b"abc_", true),
    ];

    #[test]
    fn compile_errors_match_go() {
        for &(pattern, want) in ERRORS {
            match compile_bytes(pattern) {
                Ok(_) => panic!("{:?} compiled, want {:?}", pattern, want),
                Err(got) => assert_eq!(
                    String::from_utf8_lossy(&got),
                    String::from_utf8_lossy(want),
                    "pattern {:?}",
                    String::from_utf8_lossy(pattern)
                ),
            }
        }
    }

    #[test]
    fn matches_match_go() {
        for &(pattern, subject, want) in MATCHES {
            let re = compile_bytes(pattern).expect("pattern compiles");
            assert_eq!(
                re.is_match(subject),
                want,
                "pattern {:?} subject {:?}",
                String::from_utf8_lossy(pattern),
                subject
            );
        }
    }

    #[test]
    fn nesting_limit() {
        let deep = format!("{}a{}", "(".repeat(1001), ")".repeat(1001));
        let err = compile(deep.as_bytes()).expect_err("too deep");
        assert_eq!(err, format!("error parsing regexp: expression nests too deeply: `{deep}`"));
        let ok = format!("{}a{}", "(".repeat(999), ")".repeat(999));
        assert!(compile(ok.as_bytes()).is_ok());
    }

    #[test]
    fn size_limit() {
        let big = format!("({}){{1000}}", "ab".repeat(1700));
        let err = compile(big.as_bytes()).expect_err("too large");
        assert_eq!(err, format!("error parsing regexp: expression too large: `{big}`"));
        let fits = format!("({}){{1000}}", "ab".repeat(1670));
        assert!(compile(fits.as_bytes()).is_ok());
    }

    #[test]
    fn lossy_error_text() {
        let err = compile(b"a\xffb").expect_err("invalid UTF-8");
        assert_eq!(err, "error parsing regexp: invalid UTF-8: `\u{FFFD}b`");
    }

    #[test]
    fn as_str_returns_pattern() {
        let re = compile("(?i)σ+".as_bytes()).expect("compiles");
        assert_eq!(re.as_str(), "(?i)σ+");
        assert_eq!(re.as_bytes(), "(?i)σ+".as_bytes());
    }

    #[test]
    fn go_decode_replaces_each_invalid_byte() {
        assert_eq!(go_decode(b"a\xe2\x82b"), "a\u{FFFD}\u{FFFD}b");
        assert_eq!(go_decode(b"\xed\xa0\x80"), "\u{FFFD}\u{FFFD}\u{FFFD}");
        assert_eq!(go_decode("é".as_bytes()), "é");
    }

    #[test]
    fn non_boundary_inside_multibyte_char() {
        // Go: no position in "K\u{212A}_" lies between two characters of the same kind.
        let re = compile(br"\B").expect("compiles");
        assert!(!re.is_match("K\u{212A}_".as_bytes()));
        let re = compile(br"\B|\x{FFFD}").expect("compiles");
        assert!(re.is_match(b"9\xc0B"));
    }
}
