//! Source positions and errors with a source snippet.

use std::fmt;

use crate::gostd::utf8;

/// A rune range in the source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Location {
    pub from: usize,
    pub to: usize,
}

impl Location {
    pub fn new(from: usize, to: usize) -> Location {
        Location { from, to }
    }
}

/// The source text of an expression, as raw bytes.
#[derive(Clone, Debug, Default)]
pub struct Source {
    raw: Vec<u8>,
}

impl Source {
    pub fn new(raw: &[u8]) -> Source {
        Source { raw: raw.to_vec() }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.raw
    }
}

/// An error at a location of the source. Once bound to the source it prints the line, column and a snippet with
/// a caret under the offending rune. Message and snippet are byte strings: both may carry invalid UTF-8 taken
/// from the expression.
#[derive(Clone, Debug, Default)]
pub struct Error {
    pub location: Location,
    pub line: usize,
    pub column: usize,
    pub message: Vec<u8>,
    pub snippet: Vec<u8>,
}

impl Error {
    pub fn new(location: Location, message: impl Into<Vec<u8>>) -> Error {
        Error { location, message: message.into(), ..Error::default() }
    }

    /// An error without a position, printed as its bare message.
    pub fn plain(message: impl Into<Vec<u8>>) -> Error {
        Error::new(Location::default(), message)
    }

    /// Computes the line, column and snippet of the error in `source`.
    pub fn bind(mut self, source: &Source) -> Error {
        let src = source.bytes();
        let mut rune_count = 0;
        let mut line_start = 0;
        self.line = 1;
        self.column = 0;
        let mut i = 0;
        while i < src.len() {
            if rune_count == self.location.from {
                break;
            }
            let (r, size) = utf8::decode_rune(&src[i..]);
            if r == u32::from('\n') {
                line_start = i + size;
                self.line += 1;
                self.column = 0;
            } else {
                self.column += 1;
            }
            rune_count += 1;
            i += size;
        }

        let line_end = src[line_start..].iter().position(|&b| b == b'\n').map_or(src.len(), |p| line_start + p);
        if line_start == line_end {
            return self;
        }

        let mut snippet = Vec::with_capacity(8 + (line_end - line_start) + self.column + 1);
        snippet.extend_from_slice(b"\n | ");
        snippet.extend(src[line_start..line_end].iter().map(|&b| if b == b'\t' { b' ' } else { b }));
        snippet.extend_from_slice(b"\n | ");
        snippet.extend(std::iter::repeat_n(b'.', self.column));
        snippet.push(b'^');
        self.snippet = snippet;
        self
    }

    /// The error text exactly as the original prints it.
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.snippet.is_empty() {
            return self.message.clone();
        }
        let mut out = self.message.clone();
        out.extend_from_slice(format!(" ({}:{})", self.line, self.column + 1).as_bytes());
        out.extend_from_slice(&self.snippet);
        out
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.to_bytes()))
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(src: &str, from: usize) -> String {
        Error::new(Location::new(from, from + 1), "boom").bind(&Source::new(src.as_bytes())).to_string()
    }

    #[test]
    fn points_at_the_rune_column() {
        assert_eq!(bound("1 % 0", 2), "boom (1:3)\n | 1 % 0\n | ..^");
        assert_eq!(bound("héllo + x", 8), "boom (1:9)\n | héllo + x\n | ........^");
    }

    #[test]
    fn finds_the_line_and_replaces_tabs() {
        assert_eq!(bound("a +\n\tb", 5), "boom (2:2)\n |  b\n | .^");
    }

    #[test]
    fn empty_lines_have_no_snippet() {
        assert_eq!(bound("a\n\nb", 2), "boom");
        assert_eq!(bound("", 0), "boom");
    }
}
