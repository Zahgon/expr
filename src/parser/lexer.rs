//! The lexer: turns source text into tokens, one state function at a time.

use std::collections::VecDeque;
use std::fmt;

use crate::file::{Error, Location, Source};
use crate::gostd::{strconv, unicode, utf8};

/// The kind of a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Identifier,
    Number,
    String,
    Bytes,
    Operator,
    Bracket,
    Eof,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Identifier => "Identifier",
            Kind::Number => "Number",
            Kind::String => "String",
            Kind::Bytes => "Bytes",
            Kind::Operator => "Operator",
            Kind::Bracket => "Bracket",
            Kind::Eof => "EOF",
        }
    }
}

/// A token with its rune location. The value is a byte string: string literals may hold any bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub location: Location,
    pub kind: Kind,
    pub value: Vec<u8>,
}

impl Default for Token {
    fn default() -> Token {
        Token { location: Location::default(), kind: Kind::Eof, value: Vec::new() }
    }
}

impl Token {
    /// Whether the token has the kind and, when values are given, one of the values.
    pub fn is(&self, kind: Kind, values: &[&str]) -> bool {
        kind == self.kind && (values.is_empty() || values.iter().any(|v| v.as_bytes() == self.value))
    }

    pub fn value_str(&self) -> String {
        String::from_utf8_lossy(&self.value).into_owned()
    }
}

impl fmt::Display for Token {
    /// `Kind("value")`, the value quoted like Go's `%#v`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.value.is_empty() {
            f.write_str(self.kind.name())
        } else {
            write!(f, "{}({})", self.kind.name(), strconv::quote(&self.value))
        }
    }
}

/// A rune, or [`EOF`] past the end of the input.
type Rune = i32;
const EOF: Rune = -1;

#[derive(Clone, Copy, Default)]
struct Pos {
    byte: usize,
    rune: usize,
}

#[derive(Clone, Copy)]
enum State {
    Root,
    Number,
    Dot,
    Identifier,
    Not,
    QuestionMark,
    Slash,
    SingleLineComment,
    MultiLineComment,
    Pointer,
}

/// The lexer. Tokens are produced on demand by [`Lexer::next_token`].
pub struct Lexer {
    state: Option<State>,
    source: Source,
    tokens: VecDeque<Token>,
    err: Option<Error>,
    start: Pos,
    end: Pos,
    eof: bool,
    pub disable_if_operator: bool,
}

/// Why the lexer produced no token.
pub enum Stop {
    /// All tokens, including the EOF token, were produced.
    Eof,
    /// A lexing error, bound to the source.
    Error(Error),
}

fn is_space(r: Rune) -> bool {
    r >= 0 && unicode::is_space(r as u32)
}

fn is_alphabetic(r: Rune) -> bool {
    r == '_' as Rune || r == '$' as Rune || r >= 0 && unicode::is_letter(r as u32)
}

pub(crate) fn is_alpha_numeric(r: Rune) -> bool {
    is_alphabetic(r) || r >= 0 && unicode::is_digit(r as u32)
}

/// Whether the string is a valid identifier: a letter, `_` or `$`, then letters, digits, `_` or `$`.
pub fn is_valid_identifier(s: &[u8]) -> bool {
    if s.is_empty() {
        return false;
    }
    let (h, w) = utf8::decode_rune(s);
    if !is_alphabetic(h as Rune) {
        return false;
    }
    let mut i = w;
    while i < s.len() {
        let (r, size) = utf8::decode_rune(&s[i..]);
        if !is_alpha_numeric(r as Rune) {
            return false;
        }
        i += size;
    }
    true
}

fn contains_rune(valid: &str, r: Rune) -> bool {
    r >= 0 && valid.bytes().any(|b| Rune::from(b) == r)
}

impl Lexer {
    pub fn new(source: Source) -> Lexer {
        Lexer {
            state: Some(State::Root),
            source,
            tokens: VecDeque::new(),
            err: None,
            start: Pos::default(),
            end: Pos::default(),
            eof: false,
            disable_if_operator: false,
        }
    }

    /// The next token, or why there is none.
    pub fn next_token(&mut self) -> Result<Token, Stop> {
        while let Some(state) = self.state {
            if self.err.is_some() || !self.tokens.is_empty() {
                break;
            }
            self.state = self.step(state);
        }
        if let Some(err) = &self.err {
            return Err(Stop::Error(err.clone().bind(&self.source)));
        }
        self.tokens.pop_front().ok_or(Stop::Eof)
    }

    fn src(&self) -> &[u8] {
        self.source.bytes()
    }

    fn commit(&mut self) {
        self.start = self.end;
    }

    fn next(&mut self) -> Rune {
        if self.end.byte >= self.src().len() {
            self.eof = true;
            return EOF;
        }
        let (r, size) = utf8::decode_rune(&self.src()[self.end.byte..]);
        self.end.rune += 1;
        self.end.byte += size;
        r as Rune
    }

    fn peek(&self) -> Rune {
        if self.end.byte < self.src().len() {
            utf8::decode_rune(&self.src()[self.end.byte..]).0 as Rune
        } else {
            EOF
        }
    }

    fn backup(&mut self) {
        if self.eof {
            self.eof = false;
        } else if self.end.rune > 0 {
            let (_, size) = utf8::decode_last_rune(&self.src()[..self.end.byte]);
            self.end.byte -= size;
            self.end.rune -= 1;
        }
    }

    fn word(&self) -> Vec<u8> {
        self.src()[self.start.byte..self.end.byte].to_vec()
    }

    fn emit(&mut self, kind: Kind) {
        let word = self.word();
        self.emit_value(kind, word);
    }

    fn emit_value(&mut self, kind: Kind, value: Vec<u8>) {
        self.tokens.push_back(Token { location: Location::new(self.start.rune, self.end.rune), kind, value });
        self.commit();
    }

    fn emit_eof(&mut self) {
        let from = self.end.rune.saturating_sub(1);
        self.tokens.push_back(Token {
            location: Location::new(from, self.end.rune),
            kind: Kind::Eof,
            value: Vec::new(),
        });
        self.commit();
    }

    fn skip(&mut self) {
        self.commit();
    }

    fn accept(&mut self, valid: &str) -> bool {
        if contains_rune(valid, self.peek()) {
            self.next();
            true
        } else {
            false
        }
    }

    fn accept_run(&mut self, valid: &str) {
        while self.accept(valid) {}
    }

    fn skip_spaces(&mut self) {
        self.accept_run(" ");
        self.skip();
    }

    /// Records the first lexing error at the current position.
    fn error(&mut self, message: impl Into<Vec<u8>>) -> Option<State> {
        if self.err.is_none() {
            let mut end = self.end.rune;
            if self.eof {
                end += 1;
            }
            self.err = Some(Error::new(Location::new(end.wrapping_sub(1), end), message));
        }
        None
    }

    fn step(&mut self, state: State) -> Option<State> {
        match state {
            State::Root => self.root(),
            State::Number => self.number(),
            State::Dot => self.dot(),
            State::Identifier => self.identifier(),
            State::Not => self.not(),
            State::QuestionMark => {
                self.accept(".?");
                self.emit(Kind::Operator);
                Some(State::Root)
            }
            State::Slash => {
                if self.accept("/") {
                    return Some(State::SingleLineComment);
                }
                if self.accept("*") {
                    return Some(State::MultiLineComment);
                }
                self.emit(Kind::Operator);
                Some(State::Root)
            }
            State::SingleLineComment => {
                loop {
                    let r = self.next();
                    if r == EOF || r == '\n' as Rune {
                        break;
                    }
                }
                self.skip();
                Some(State::Root)
            }
            State::MultiLineComment => {
                loop {
                    let r = self.next();
                    if r == EOF {
                        return self.error("unclosed comment");
                    }
                    if r == '*' as Rune && self.accept("/") {
                        break;
                    }
                }
                self.skip();
                Some(State::Root)
            }
            State::Pointer => {
                self.accept("#");
                self.emit(Kind::Operator);
                loop {
                    let r = self.next();
                    if !is_alpha_numeric(r) {
                        self.backup();
                        if !self.word().is_empty() {
                            self.emit(Kind::Identifier);
                        }
                        return Some(State::Root);
                    }
                }
            }
        }
    }

    fn root(&mut self) -> Option<State> {
        let r = self.next();
        let ch = if r >= 0 { char::from_u32(r as u32) } else { None };
        match ch {
            _ if r == EOF => {
                self.emit_eof();
                return None;
            }
            _ if is_space(r) => {
                self.skip();
                return Some(State::Root);
            }
            Some(q @ ('\'' | '"')) => {
                self.scan_string(q as Rune);
                match unescape(&self.word()) {
                    Ok(s) => self.emit_value(Kind::String, s),
                    Err(e) => {
                        self.error(e);
                        let word = self.word();
                        self.emit_value(Kind::String, word);
                    }
                }
            }
            Some('`') => self.scan_raw_string('`' as Rune),
            Some('b' | 'B') if self.peek() == '\'' as Rune || self.peek() == '"' as Rune => {
                let quote = self.next();
                self.scan_string(quote);
                let word = self.word();
                match unescape_bytes(&word[1..]) {
                    Ok(s) => self.emit_value(Kind::Bytes, s),
                    Err(e) => {
                        self.error(e);
                        self.emit_value(Kind::Bytes, word[1..].to_vec());
                    }
                }
            }
            Some('0'..='9') => {
                self.backup();
                return Some(State::Number);
            }
            Some('?') => return Some(State::QuestionMark),
            Some('/') => return Some(State::Slash),
            Some('#') => return Some(State::Pointer),
            Some('|') => {
                self.accept("|");
                self.emit(Kind::Operator);
            }
            Some(':') => {
                self.accept(":");
                self.emit(Kind::Operator);
            }
            Some('(' | '[' | '{' | ')' | ']' | '}') => self.emit(Kind::Bracket),
            Some(',' | ';' | '%' | '+' | '-' | '^') => self.emit(Kind::Operator),
            Some('&' | '!' | '=' | '*' | '<' | '>') => {
                self.accept("&=*");
                self.emit(Kind::Operator);
            }
            Some('.') => {
                self.backup();
                return Some(State::Dot);
            }
            _ if is_alpha_numeric(r) => {
                self.backup();
                return Some(State::Identifier);
            }
            _ => return self.error(format!("unrecognized character: {}", strconv::fmt_sharp_u(r as u32))),
        }
        Some(State::Root)
    }

    fn number(&mut self) -> Option<State> {
        if !self.scan_number() {
            let word = self.word();
            return self.error(format!("bad number syntax: {}", strconv::quote(&word)));
        }
        self.emit(Kind::Number);
        Some(State::Root)
    }

    fn scan_number(&mut self) -> bool {
        let mut digits = "0123456789_";
        if self.accept("0") {
            if self.accept("xX") {
                digits = "0123456789abcdefABCDEF_";
            } else if self.accept("oO") {
                digits = "01234567_";
            } else if self.accept("bB") {
                digits = "01_";
            }
        }
        self.accept_run(digits);
        let end = self.end;
        if self.accept(".") {
            if self.peek() == '.' as Rune {
                self.end = end;
                return true;
            }
            self.accept_run(digits);
        }
        if self.accept("eE") {
            self.accept("+-");
            self.accept_run(digits);
        }
        if is_alpha_numeric(self.peek()) {
            self.next();
            return false;
        }
        true
    }

    fn dot(&mut self) -> Option<State> {
        self.next();
        if self.accept("0123456789") {
            self.backup();
            return Some(State::Number);
        }
        self.accept(".");
        self.emit(Kind::Operator);
        Some(State::Root)
    }

    fn identifier(&mut self) -> Option<State> {
        loop {
            let r = self.next();
            if !is_alpha_numeric(r) {
                self.backup();
                match self.word().as_slice() {
                    b"not" => return Some(State::Not),
                    b"in" | b"or" | b"and" | b"matches" | b"contains" | b"startsWith" | b"endsWith" | b"let" => {
                        self.emit(Kind::Operator)
                    }
                    b"if" | b"else" if !self.disable_if_operator => self.emit(Kind::Operator),
                    _ => self.emit(Kind::Identifier),
                }
                return Some(State::Root);
            }
        }
    }

    fn not(&mut self) -> Option<State> {
        self.emit(Kind::Operator);
        self.skip_spaces();
        let end = self.end;
        loop {
            let r = self.next();
            if !is_alpha_numeric(r) {
                self.backup();
                break;
            }
        }
        match self.word().as_slice() {
            b"in" | b"matches" | b"contains" | b"startsWith" | b"endsWith" => self.emit(Kind::Operator),
            _ => self.end = end,
        }
        Some(State::Root)
    }

    fn scan_digits(&mut self, mut ch: Rune, base: u32, mut n: usize) -> Rune {
        while n > 0 && digit_val(ch) < base {
            ch = self.next();
            n -= 1;
        }
        if n > 0 {
            self.error("invalid char escape");
        }
        ch
    }

    fn scan_escape(&mut self, quote: Rune) -> Rune {
        let ch = self.next();
        match ch {
            _ if ch == quote => self.next(),
            0x61 | 0x62 | 0x66 | 0x6e | 0x72 | 0x74 | 0x76 | 0x5c => self.next(),
            0x30..=0x37 => self.scan_digits(ch, 8, 3),
            0x78 => {
                let c = self.next();
                self.scan_digits(c, 16, 2)
            }
            0x75 => {
                if self.peek() == '{' as Rune {
                    self.next();
                    let mut digits = 0;
                    loop {
                        let p = self.peek();
                        if p == '}' as Rune {
                            break;
                        }
                        if digit_val(p) >= 16 || digits >= 6 {
                            self.error("invalid char escape");
                            return EOF;
                        }
                        self.next();
                        digits += 1;
                    }
                    if self.peek() != '}' as Rune || digits == 0 {
                        self.error("invalid char escape");
                        return EOF;
                    }
                    self.next();
                    return self.next();
                }
                let c = self.next();
                self.scan_digits(c, 16, 4)
            }
            0x55 => {
                let c = self.next();
                self.scan_digits(c, 16, 8)
            }
            _ => {
                self.error("invalid char escape");
                ch
            }
        }
    }

    fn scan_string(&mut self, quote: Rune) {
        let mut ch = self.next();
        while ch != quote {
            if ch == '\n' as Rune || ch == EOF {
                self.error("literal not terminated");
                return;
            }
            if ch == '\\' as Rune {
                ch = self.scan_escape(quote);
            } else {
                ch = self.next();
            }
        }
    }

    fn scan_raw_string(&mut self, quote: Rune) {
        let mut escaped_quotes = 0;
        loop {
            let mut ch = self.next();
            while ch == quote && self.peek() == quote {
                self.next();
                ch = self.next();
                escaped_quotes += 1;
            }
            if ch == quote {
                break;
            }
            if ch == EOF {
                self.error("literal not terminated");
                return;
            }
        }
        let s = self.src()[self.start.byte + 1..self.end.byte - 1].to_vec();
        if escaped_quotes == 0 {
            self.emit_value(Kind::String, s);
            return;
        }
        let mut out = Vec::with_capacity(s.len());
        let mut skipped = false;
        let mut i = 0;
        while i < s.len() {
            let (r, size) = utf8::decode_rune(&s[i..]);
            if r as Rune == quote {
                if !skipped {
                    skipped = true;
                    i += size;
                    continue;
                }
                skipped = false;
            }
            utf8::encode_rune(r, &mut out);
            i += size;
        }
        self.emit_value(Kind::String, out);
    }
}

fn digit_val(ch: Rune) -> u32 {
    let lower = ch | 0x20;
    if (0x30..=0x39).contains(&ch) {
        (ch - 0x30) as u32
    } else if (0x61..=0x66).contains(&lower) {
        (lower - 0x61 + 10) as u32
    } else {
        16
    }
}

fn normalize_newlines(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    let mut i = 0;
    while i < value.len() {
        if value[i] == b'\r' {
            out.push(b'\n');
            if value.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
        } else {
            out.push(value[i]);
        }
        i += 1;
    }
    out
}

fn unhex(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some(u32::from(b - b'0')),
        b'a'..=b'f' => Some(u32::from(b - b'a' + 10)),
        b'A'..=b'F' => Some(u32::from(b - b'A' + 10)),
        _ => None,
    }
}

const UNESCAPE_ERROR: &str = "unable to unescape string";
const OCTAL_ERROR: &str = "unable to unescape octal sequence in string";
const TRAILING_ERROR: &str = "unable to unescape string, found '\\' as last character";

/// Unescapes a quoted string literal (quotes included) like the original's `unescape`.
pub fn unescape(value: &[u8]) -> Result<Vec<u8>, String> {
    let value = normalize_newlines(value);
    let n = value.len();
    if n < 2 || value[0] != value[n - 1] || (value[0] != b'"' && value[0] != b'\'') {
        return Err(UNESCAPE_ERROR.to_string());
    }
    let mut s = &value[1..n - 1];
    let mut out = Vec::with_capacity(3 * n / 2);
    while !s.is_empty() {
        let (c, multibyte, rest) = unescape_char(s)?;
        s = rest;
        if c < 0x80 || !multibyte {
            out.push(c as u8);
        } else {
            utf8::encode_rune(c, &mut out);
        }
    }
    Ok(out)
}

fn unescape_char(s: &[u8]) -> Result<(u32, bool, &[u8]), String> {
    let c = s[0];
    if c >= 0x80 {
        let (r, size) = utf8::decode_rune(s);
        return Ok((r, true, &s[size..]));
    }
    if c != b'\\' {
        return Ok((u32::from(c), false, &s[1..]));
    }
    if s.len() <= 1 {
        return Err(TRAILING_ERROR.to_string());
    }
    let c = s[1];
    let mut s = &s[2..];
    let value = match c {
        b'a' => 7,
        b'b' => 8,
        b'f' => 12,
        b'n' => 10,
        b'r' => 13,
        b't' => 9,
        b'v' => 11,
        b'\\' => u32::from(b'\\'),
        b'\'' => u32::from(b'\''),
        b'"' => u32::from(b'"'),
        b'`' => u32::from(b'`'),
        b'?' => u32::from(b'?'),
        b'x' | b'X' | b'u' | b'U' => {
            if c == b'u' && s.first() == Some(&b'{') {
                s = &s[1..];
                let mut v: u32 = 0;
                let mut digits = 0;
                while let Some(&b) = s.first() {
                    if b == b'}' {
                        break;
                    }
                    let x = unhex(b).ok_or_else(|| UNESCAPE_ERROR.to_string())?;
                    if digits >= 6 {
                        return Err(UNESCAPE_ERROR.to_string());
                    }
                    v = (v << 4) | x;
                    s = &s[1..];
                    digits += 1;
                }
                if s.first() != Some(&b'}') || digits == 0 {
                    return Err(UNESCAPE_ERROR.to_string());
                }
                s = &s[1..];
                if v > utf8::MAX_RUNE {
                    return Err(UNESCAPE_ERROR.to_string());
                }
                return Ok((v, true, s));
            }
            let n = match c {
                b'x' | b'X' => 2,
                b'u' => 4,
                _ => 8,
            };
            if s.len() < n {
                return Err(UNESCAPE_ERROR.to_string());
            }
            let mut v: u32 = 0;
            for &b in &s[..n] {
                let x = unhex(b).ok_or_else(|| UNESCAPE_ERROR.to_string())?;
                v = (v << 4) | x;
            }
            s = &s[n..];
            if v > utf8::MAX_RUNE {
                return Err(UNESCAPE_ERROR.to_string());
            }
            return Ok((v, true, s));
        }
        b'0'..=b'3' => {
            if s.len() < 2 {
                return Err(OCTAL_ERROR.to_string());
            }
            let mut v = u32::from(c - b'0');
            for &x in &s[..2] {
                if !(b'0'..=b'7').contains(&x) {
                    return Err(OCTAL_ERROR.to_string());
                }
                v = v * 8 + u32::from(x - b'0');
            }
            return Ok((v, true, &s[2..]));
        }
        _ => return Err(UNESCAPE_ERROR.to_string()),
    };
    Ok((value, false, s))
}

/// Unescapes a quoted bytes literal (quotes included, without the `b` prefix).
pub fn unescape_bytes(value: &[u8]) -> Result<Vec<u8>, String> {
    let value = normalize_newlines(value);
    let n = value.len();
    if n < 2 || value[0] != value[n - 1] || (value[0] != b'"' && value[0] != b'\'') {
        return Err(UNESCAPE_ERROR.to_string());
    }
    let mut s = &value[1..n - 1];
    let mut out = Vec::with_capacity(3 * n / 2);
    while !s.is_empty() {
        let (c, multibyte, rest) = unescape_byte_char(s)?;
        s = rest;
        if c < 0x80 || !multibyte {
            out.push(c as u8);
        } else {
            utf8::encode_rune(c, &mut out);
        }
    }
    Ok(out)
}

fn unescape_byte_char(s: &[u8]) -> Result<(u32, bool, &[u8]), String> {
    let c = s[0];
    if c != b'\\' {
        if c >= 0x80 {
            let (r, size) = utf8::decode_rune(s);
            return Ok((r, true, &s[size..]));
        }
        return Ok((u32::from(c), false, &s[1..]));
    }
    if s.len() <= 1 {
        return Err(TRAILING_ERROR.to_string());
    }
    let c = s[1];
    let s = &s[2..];
    let simple = |v: u8| Ok((u32::from(v), false, s));
    match c {
        b'a' => simple(7),
        b'b' => simple(8),
        b'f' => simple(12),
        b'n' => simple(10),
        b'r' => simple(13),
        b't' => simple(9),
        b'v' => simple(11),
        b'\\' | b'\'' | b'"' | b'`' | b'?' => simple(c),
        b'x' | b'X' => {
            if s.len() < 2 {
                return Err(UNESCAPE_ERROR.to_string());
            }
            match (unhex(s[0]), unhex(s[1])) {
                (Some(hi), Some(lo)) => Ok(((hi << 4) | lo, false, &s[2..])),
                _ => Err(UNESCAPE_ERROR.to_string()),
            }
        }
        b'0'..=b'3' => {
            if s.len() < 2 {
                return Err(OCTAL_ERROR.to_string());
            }
            if !(b'0'..=b'7').contains(&s[0]) || !(b'0'..=b'7').contains(&s[1]) {
                return Err(OCTAL_ERROR.to_string());
            }
            let v = u32::from(c - b'0') * 64 + u32::from(s[0] - b'0') * 8 + u32::from(s[1] - b'0');
            if v > 255 {
                return Err(UNESCAPE_ERROR.to_string());
            }
            Ok((v, false, &s[2..]))
        }
        _ => Err(UNESCAPE_ERROR.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(input: &str) -> Result<Vec<String>, String> {
        let mut lexer = Lexer::new(Source::new(input.as_bytes()));
        let mut out = Vec::new();
        loop {
            match lexer.next_token() {
                Ok(t) => out.push(t.to_string()),
                Err(Stop::Eof) => return Ok(out),
                Err(Stop::Error(e)) => return Err(e.to_string()),
            }
        }
    }

    #[test]
    fn lexes_operators_and_literals() {
        assert_eq!(
            lex("a.b ?? 1_000 not in [0x1F]").unwrap(),
            vec![
                "Identifier(\"a\")",
                "Operator(\".\")",
                "Identifier(\"b\")",
                "Operator(\"??\")",
                "Number(\"1_000\")",
                "Operator(\"not\")",
                "Operator(\"in\")",
                "Bracket(\"[\")",
                "Number(\"0x1F\")",
                "Bracket(\"]\")",
                "EOF",
            ]
        );
    }

    #[test]
    fn ranges_are_not_floats() {
        assert_eq!(lex("1..3").unwrap(), vec!["Number(\"1\")", "Operator(\"..\")", "Number(\"3\")", "EOF"]);
    }

    #[test]
    fn unescapes_strings() {
        assert_eq!(lex(r#""é\x41\101\n""#).unwrap()[0], "String(\"éAA\\n\")");
        assert_eq!(lex("`a``b`").unwrap()[0], "String(\"a`b\")");
    }

    #[test]
    fn reports_errors_with_positions() {
        assert_eq!(lex("\"abc").unwrap_err(), "literal not terminated (1:5)\n | \"abc\n | ....^");
        assert_eq!(lex("1 @ 2").unwrap_err(), "unrecognized character: U+0040 '@' (1:3)\n | 1 @ 2\n | ..^");
        assert_eq!(lex("12a").unwrap_err(), "bad number syntax: \"12a\" (1:3)\n | 12a\n | ..^");
        assert_eq!(lex("/* x").unwrap_err(), "unclosed comment (1:5)\n | /* x\n | ....^");
    }
}
