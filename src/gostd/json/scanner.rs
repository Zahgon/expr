//! The JSON scanner state machine, ported from Go's `encoding/json/scanner.go`.
//!
//! Follows the behaviour of Go's standard library encoding/json package (The Go Authors, BSD-style licence).
//!
//! The scanner is fed one byte at a time and reports what each byte means through an [`Op`]. Syntax errors
//! carry exactly the message Go's `SyntaxError` would carry.

/// Maximum nesting depth of arrays and objects accepted by Go's scanner.
const MAX_NESTING_DEPTH: usize = 10000;

/// The result of feeding one byte to the scanner (Go's `scanContinue` .. `scanError`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// Uninteresting byte (inside a literal).
    Continue,
    /// Start of a literal; its end is implied by the next result that is not `Continue`.
    BeginLiteral,
    /// `{`.
    BeginObject,
    /// Just finished an object key (the `:`).
    ObjectKey,
    /// Just finished a non-last object value (the `,`).
    ObjectValue,
    /// `}`.
    EndObject,
    /// `[`.
    BeginArray,
    /// Just finished a non-last array value (the `,`).
    ArrayValue,
    /// `]`.
    EndArray,
    /// Whitespace that may be skipped.
    SkipSpace,
    /// The top-level value ended before this byte.
    End,
    /// A syntax error; see [`Scanner::err`].
    Error,
}

/// What the scanner is in the middle of parsing (Go's `parseObjectKey` and friends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parse {
    ObjectKey,
    ObjectValue,
    ArrayValue,
}

/// The current step function of Go's scanner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    BeginValueOrEmpty,
    BeginValue,
    BeginStringOrEmpty,
    BeginString,
    EndValue,
    EndTop,
    InString,
    InStringEsc,
    InStringEscU,
    InStringEscU1,
    InStringEscU12,
    InStringEscU123,
    Neg,
    One,
    Zero,
    Dot,
    Dot0,
    E,
    ESign,
    E0,
    T,
    Tr,
    Tru,
    F,
    Fa,
    Fal,
    Fals,
    N,
    Nu,
    Nul,
    Error,
}

/// Go's JSON scanner.
#[derive(Clone, Debug)]
pub(crate) struct Scanner {
    step: State,
    end_top: bool,
    parse_state: Vec<Parse>,
    err: Option<String>,
    /// Number of bytes consumed so far (kept for parity with Go; error texts do not use it).
    pub(crate) bytes: i64,
}

/// Reports whether `c` is JSON whitespace.
pub(crate) fn is_space(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\r' || c == b'\n'
}

fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

impl Scanner {
    /// A fresh scanner positioned before the top-level value.
    pub(crate) fn new() -> Scanner {
        Scanner { step: State::BeginValue, end_top: false, parse_state: Vec::new(), err: None, bytes: 0 }
    }

    /// The syntax error message, once [`Op::Error`] has been returned.
    pub(crate) fn err(&self) -> Option<&str> {
        self.err.as_deref()
    }

    /// Go's `scanner.eof`: tells the scanner the input has ended.
    pub(crate) fn eof(&mut self) -> Op {
        if self.err.is_some() {
            return Op::Error;
        }
        if self.end_top {
            return Op::End;
        }
        self.step(b' ');
        if self.end_top {
            return Op::End;
        }
        if self.err.is_none() {
            self.err = Some("unexpected end of JSON input".to_string());
        }
        Op::Error
    }

    /// Feeds one byte to the current step function.
    pub(crate) fn step(&mut self, c: u8) -> Op {
        match self.step {
            State::BeginValueOrEmpty => self.state_begin_value_or_empty(c),
            State::BeginValue => self.state_begin_value(c),
            State::BeginStringOrEmpty => self.state_begin_string_or_empty(c),
            State::BeginString => self.state_begin_string(c),
            State::EndValue => self.state_end_value(c),
            State::EndTop => self.state_end_top(c),
            State::InString => self.state_in_string(c),
            State::InStringEsc => self.state_in_string_esc(c),
            State::InStringEscU => self.hex_digit(c, State::InStringEscU1),
            State::InStringEscU1 => self.hex_digit(c, State::InStringEscU12),
            State::InStringEscU12 => self.hex_digit(c, State::InStringEscU123),
            State::InStringEscU123 => self.hex_digit(c, State::InString),
            State::Neg => self.state_neg(c),
            State::One => self.state1(c),
            State::Zero => self.state0(c),
            State::Dot => self.state_dot(c),
            State::Dot0 => self.state_dot0(c),
            State::E => self.state_e(c),
            State::ESign => self.state_e_sign(c),
            State::E0 => self.state_e0(c),
            State::T => self.expect(c, b'r', State::Tr, "in literal true (expecting 'r')"),
            State::Tr => self.expect(c, b'u', State::Tru, "in literal true (expecting 'u')"),
            State::Tru => self.expect(c, b'e', State::EndValue, "in literal true (expecting 'e')"),
            State::F => self.expect(c, b'a', State::Fa, "in literal false (expecting 'a')"),
            State::Fa => self.expect(c, b'l', State::Fal, "in literal false (expecting 'l')"),
            State::Fal => self.expect(c, b's', State::Fals, "in literal false (expecting 's')"),
            State::Fals => self.expect(c, b'e', State::EndValue, "in literal false (expecting 'e')"),
            State::N => self.expect(c, b'u', State::Nu, "in literal null (expecting 'u')"),
            State::Nu => self.expect(c, b'l', State::Nul, "in literal null (expecting 'l')"),
            State::Nul => self.expect(c, b'l', State::EndValue, "in literal null (expecting 'l')"),
            State::Error => Op::Error,
        }
    }

    /// Go's `stateEndValue` called directly (as the Decoder does with a space).
    pub(crate) fn end_value(&mut self, c: u8) -> Op {
        self.state_end_value(c)
    }

    fn push_parse_state(&mut self, c: u8, state: Parse, success: Op) -> Op {
        self.parse_state.push(state);
        if self.parse_state.len() <= MAX_NESTING_DEPTH {
            return success;
        }
        self.error(c, "exceeded max depth")
    }

    fn pop_parse_state(&mut self) {
        self.parse_state.pop();
        if self.parse_state.is_empty() {
            self.step = State::EndTop;
            self.end_top = true;
        } else {
            self.step = State::EndValue;
        }
    }

    fn state_begin_value_or_empty(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::SkipSpace;
        }
        if c == b']' {
            return self.state_end_value(c);
        }
        self.state_begin_value(c)
    }

    fn state_begin_value(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::SkipSpace;
        }
        let next = match c {
            b'{' => {
                self.step = State::BeginStringOrEmpty;
                return self.push_parse_state(c, Parse::ObjectKey, Op::BeginObject);
            }
            b'[' => {
                self.step = State::BeginValueOrEmpty;
                return self.push_parse_state(c, Parse::ArrayValue, Op::BeginArray);
            }
            b'"' => State::InString,
            b'-' => State::Neg,
            b'0' => State::Zero,
            b't' => State::T,
            b'f' => State::F,
            b'n' => State::N,
            b'1'..=b'9' => State::One,
            _ => return self.error(c, "looking for beginning of value"),
        };
        self.step = next;
        Op::BeginLiteral
    }

    fn state_begin_string_or_empty(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::SkipSpace;
        }
        if c == b'}' {
            if let Some(top) = self.parse_state.last_mut() {
                *top = Parse::ObjectValue;
            }
            return self.state_end_value(c);
        }
        self.state_begin_string(c)
    }

    fn state_begin_string(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::SkipSpace;
        }
        if c == b'"' {
            self.step = State::InString;
            return Op::BeginLiteral;
        }
        self.error(c, "looking for beginning of object key string")
    }

    fn state_end_value(&mut self, c: u8) -> Op {
        let Some(&ps) = self.parse_state.last() else {
            self.step = State::EndTop;
            self.end_top = true;
            return self.state_end_top(c);
        };
        if is_space(c) {
            self.step = State::EndValue;
            return Op::SkipSpace;
        }
        match ps {
            Parse::ObjectKey => {
                if c == b':' {
                    self.set_top(Parse::ObjectValue);
                    self.step = State::BeginValue;
                    return Op::ObjectKey;
                }
                self.error(c, "after object key")
            }
            Parse::ObjectValue => {
                if c == b',' {
                    self.set_top(Parse::ObjectKey);
                    self.step = State::BeginString;
                    return Op::ObjectValue;
                }
                if c == b'}' {
                    self.pop_parse_state();
                    return Op::EndObject;
                }
                self.error(c, "after object key:value pair")
            }
            Parse::ArrayValue => {
                if c == b',' {
                    self.step = State::BeginValue;
                    return Op::ArrayValue;
                }
                if c == b']' {
                    self.pop_parse_state();
                    return Op::EndArray;
                }
                self.error(c, "after array element")
            }
        }
    }

    fn set_top(&mut self, state: Parse) {
        if let Some(top) = self.parse_state.last_mut() {
            *top = state;
        }
    }

    fn state_end_top(&mut self, c: u8) -> Op {
        if !is_space(c) {
            // Go records the error but still reports scanEnd for this byte.
            self.error(c, "after top-level value");
        }
        Op::End
    }

    fn state_in_string(&mut self, c: u8) -> Op {
        if c == b'"' {
            self.step = State::EndValue;
            return Op::Continue;
        }
        if c == b'\\' {
            self.step = State::InStringEsc;
            return Op::Continue;
        }
        if c < 0x20 {
            return self.error(c, "in string literal");
        }
        Op::Continue
    }

    fn state_in_string_esc(&mut self, c: u8) -> Op {
        match c {
            b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => {
                self.step = State::InString;
                Op::Continue
            }
            b'u' => {
                self.step = State::InStringEscU;
                Op::Continue
            }
            _ => self.error(c, "in string escape code"),
        }
    }

    fn hex_digit(&mut self, c: u8, next: State) -> Op {
        if is_hex(c) {
            self.step = next;
            return Op::Continue;
        }
        self.error(c, "in \\u hexadecimal character escape")
    }

    fn state_neg(&mut self, c: u8) -> Op {
        match c {
            b'0' => self.step = State::Zero,
            b'1'..=b'9' => self.step = State::One,
            _ => return self.error(c, "in numeric literal"),
        }
        Op::Continue
    }

    fn state1(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            self.step = State::One;
            return Op::Continue;
        }
        self.state0(c)
    }

    fn state0(&mut self, c: u8) -> Op {
        match c {
            b'.' => self.step = State::Dot,
            b'e' | b'E' => self.step = State::E,
            _ => return self.state_end_value(c),
        }
        Op::Continue
    }

    fn state_dot(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            self.step = State::Dot0;
            return Op::Continue;
        }
        self.error(c, "after decimal point in numeric literal")
    }

    fn state_dot0(&mut self, c: u8) -> Op {
        match c {
            b'0'..=b'9' => {}
            b'e' | b'E' => self.step = State::E,
            _ => return self.state_end_value(c),
        }
        Op::Continue
    }

    fn state_e(&mut self, c: u8) -> Op {
        if c == b'+' || c == b'-' {
            self.step = State::ESign;
            return Op::Continue;
        }
        self.state_e_sign(c)
    }

    fn state_e_sign(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            self.step = State::E0;
            return Op::Continue;
        }
        self.error(c, "in exponent of numeric literal")
    }

    fn state_e0(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            return Op::Continue;
        }
        self.state_end_value(c)
    }

    fn expect(&mut self, c: u8, want: u8, next: State, context: &str) -> Op {
        if c == want {
            self.step = next;
            return Op::Continue;
        }
        self.error(c, context)
    }

    fn error(&mut self, c: u8, context: &str) -> Op {
        self.step = State::Error;
        self.err = Some(format!("invalid character {} {}", quote_char(c), context));
        Op::Error
    }
}

/// Go's `quoteChar`: the byte is treated as the rune U+0000..U+00FF and quoted like `strconv.Quote`.
fn quote_char(c: u8) -> String {
    match c {
        b'\'' => return "'\\''".to_string(),
        b'"' => return "'\"'".to_string(),
        _ => {}
    }
    let body = match c {
        0x07 => "\\a".to_string(),
        0x08 => "\\b".to_string(),
        0x0c => "\\f".to_string(),
        b'\n' => "\\n".to_string(),
        b'\r' => "\\r".to_string(),
        b'\t' => "\\t".to_string(),
        0x0b => "\\v".to_string(),
        b'\\' => "\\\\".to_string(),
        0x00..=0x1f | 0x7f => format!("\\x{c:02x}"),
        // C1 controls, NO-BREAK SPACE and SOFT HYPHEN are not printable for strconv.
        0x80..=0xa0 | 0xad => format!("\\u{c:04x}"),
        _ => char::from(c).to_string(),
    };
    format!("'{body}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(data: &[u8]) -> Result<(), String> {
        let mut s = Scanner::new();
        for &c in data {
            s.bytes += 1;
            if s.step(c) == Op::Error {
                return Err(s.err().unwrap_or_default().to_string());
            }
        }
        if s.eof() == Op::Error {
            return Err(s.err().unwrap_or_default().to_string());
        }
        Ok(())
    }

    #[test]
    fn quote_chars() {
        assert_eq!(quote_char(b'x'), "'x'");
        assert_eq!(quote_char(b'\''), "'\\''");
        assert_eq!(quote_char(b'"'), "'\"'");
        assert_eq!(quote_char(b'\\'), "'\\\\'");
        assert_eq!(quote_char(0), "'\\x00'");
        assert_eq!(quote_char(0x0b), "'\\v'");
        assert_eq!(quote_char(0x7f), "'\\x7f'");
        assert_eq!(quote_char(0x80), "'\\u0080'");
        assert_eq!(quote_char(0xa0), "'\\u00a0'");
        assert_eq!(quote_char(0xad), "'\\u00ad'");
        assert_eq!(quote_char(0xff), "'\u{ff}'");
    }

    #[test]
    fn valid_documents() {
        for doc in [&b"null"[..], b" [1, 2.5e+3, -0, \"a\\u00e9\", {\"k\": true}] ", b"{}", b"[]", b"-0.0E-0"] {
            assert_eq!(check(doc), Ok(()), "{doc:?}");
        }
    }

    #[test]
    fn errors() {
        let cases: &[(&[u8], &str)] = &[
            (b"", "unexpected end of JSON input"),
            (b"  ", "unexpected end of JSON input"),
            (b"x", "invalid character 'x' looking for beginning of value"),
            (b"{\"a\" 1}", "invalid character '1' after object key"),
            (b"{\"a\"}", "invalid character '}' after object key"),
            (b"tx", "invalid character 'x' in literal true (expecting 'r')"),
            (b"nul", "invalid character ' ' in literal null (expecting 'l')"),
            (b"\"abc", "unexpected end of JSON input"),
            (b"[1", "unexpected end of JSON input"),
            (b"[1 2]", "invalid character '2' after array element"),
            (b"{\"a\":1 2}", "invalid character '2' after object key:value pair"),
            (b"{1:2}", "invalid character '1' looking for beginning of object key string"),
            (b"1 x", "invalid character 'x' after top-level value"),
            (b"01", "invalid character '1' after top-level value"),
            (b"1.", "invalid character ' ' after decimal point in numeric literal"),
            (b"1.x", "invalid character 'x' after decimal point in numeric literal"),
            (b"1ex", "invalid character 'x' in exponent of numeric literal"),
            (b"-x", "invalid character 'x' in numeric literal"),
            (b"\"\\x\"", "invalid character 'x' in string escape code"),
            (b"\"\\u12g\"", "invalid character 'g' in \\u hexadecimal character escape"),
            (b"\"\n\"", "invalid character '\\n' in string literal"),
            (b"[,]", "invalid character ',' looking for beginning of value"),
            (b"[1,]", "invalid character ']' looking for beginning of value"),
        ];
        for (doc, want) in cases {
            assert_eq!(check(doc), Err(want.to_string()), "{doc:?}");
        }
    }

    #[test]
    fn max_depth() {
        let ok = vec![b'['; MAX_NESTING_DEPTH];
        let mut s = Scanner::new();
        assert!(ok.iter().all(|&c| s.step(c) == Op::BeginArray));
        assert_eq!(s.step(b'['), Op::Error);
        assert_eq!(s.err(), Some("invalid character '[' exceeded max depth"));
    }
}
