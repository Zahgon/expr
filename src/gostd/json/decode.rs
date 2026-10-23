//! Decoding: `json.Unmarshal` into `any`, `json.Decoder` with `UseNumber`, and `json.Valid`.
//!
//! Follows the behaviour of Go's standard library encoding/json package (The Go Authors, BSD-style licence).
//!
//! Values are built directly from the scanner's results, which is what Go's `decodeState` does for `any`.

use std::collections::HashMap;

use super::scanner::{is_space, Op, Scanner};
use super::{decode_rune, Json};

/// `json.Valid(data)`: reports whether `data` is one valid JSON value, optionally surrounded by whitespace.
pub fn valid(data: &[u8]) -> bool {
    let mut scan = Scanner::new();
    for &c in data {
        scan.bytes += 1;
        if scan.step(c) == Op::Error {
            return false;
        }
    }
    scan.eof() != Op::Error
}

/// `json.Unmarshal(data, &v)` with `var v any`.
///
/// The whole input is checked for syntax first (Go's `checkValid`), so a syntax error anywhere wins, with
/// Go's `SyntaxError` text. Numbers are kept as their literal text; a number that does not fit a float64
/// (for example `1e400`) makes the call fail with
/// `json: cannot unmarshal number 1e400 into Go value of type float64`. Go keeps decoding after such a
/// number and reports the first one in document order, which is what is returned here.
pub fn unmarshal(data: &[u8]) -> Result<Json, String> {
    let mut scan = Scanner::new();
    let mut builder = Builder::new(data, false);
    for (i, &c) in data.iter().enumerate() {
        scan.bytes += 1;
        let op = scan.step(c);
        if op == Op::Error {
            return Err(syntax_error(&scan));
        }
        builder.feed(op, i);
    }
    if scan.eof() == Op::Error {
        return Err(syntax_error(&scan));
    }
    builder.finish()
}

/// One `Decode(&v)` call (with `var v any`) on `json.NewDecoder(bytes.NewReader(data))` after `UseNumber()`.
///
/// Returns the value and the offset just past it (the Decoder's `InputOffset`). Numbers are always kept as
/// their literal text. Errors are the Decoder's texts: `EOF` when the input is empty or only whitespace,
/// `unexpected EOF` when it ends inside a value, a `SyntaxError` text when a byte is invalid before the
/// value ends. As in Go, bytes after a complete value are not examined (so `123x` decodes `123` and stops
/// at offset 3); a number or literal at the very end of the input is complete.
pub fn decode_use_number(data: &[u8]) -> Result<(Json, usize), String> {
    let n = read_value(data)?;
    let value = data.get(..n).unwrap_or(data);
    let mut scan = Scanner::new();
    let mut builder = Builder::new(value, true);
    for (i, &c) in value.iter().enumerate() {
        let op = scan.step(c);
        if op == Op::Error {
            return Err(syntax_error(&scan));
        }
        builder.feed(op, i);
    }
    if scan.eof() == Op::Error {
        return Err(syntax_error(&scan));
    }
    Ok((builder.finish()?, n))
}

/// Reports whether only whitespace follows `offset` in `data`: then a `decoder.Token()` call after
/// [`decode_use_number`] returns `io.EOF`.
pub fn only_whitespace_follows(data: &[u8], offset: usize) -> bool {
    data.get(offset..).is_none_or(|rest| rest.iter().all(|&c| is_space(c)))
}

fn syntax_error(scan: &Scanner) -> String {
    scan.err().unwrap_or("unexpected end of JSON input").to_string()
}

/// Go's `Decoder.readValue` over a reader that yields all of `data`: the length of the first value.
fn read_value(data: &[u8]) -> Result<usize, String> {
    let mut scan = Scanner::new();
    for (i, &c) in data.iter().enumerate() {
        scan.bytes += 1;
        match scan.step(c) {
            // The byte after the value is not part of it (and an error it caused is ignored).
            Op::End => return Ok(i),
            Op::EndObject | Op::EndArray if scan.end_value(b' ') == Op::End => return Ok(i + 1),
            Op::Error => return Err(syntax_error(&scan)),
            _ => {}
        }
    }
    if scan.step(b' ') == Op::End {
        return Ok(data.len());
    }
    if data.iter().any(|&c| !is_space(c)) {
        Err("unexpected EOF".to_string())
    } else {
        Err("EOF".to_string())
    }
}

/// An array or object under construction.
enum Frame {
    Array(Vec<Json>),
    Object { entries: Vec<(Vec<u8>, Json)>, index: HashMap<Vec<u8>, usize>, key: Option<Vec<u8>> },
}

/// Builds a [`Json`] from the scanner's results over syntactically valid input.
struct Builder<'a> {
    data: &'a [u8],
    use_number: bool,
    stack: Vec<Frame>,
    result: Option<Json>,
    literal_start: Option<usize>,
    number_err: Option<String>,
}

impl<'a> Builder<'a> {
    fn new(data: &'a [u8], use_number: bool) -> Builder<'a> {
        Builder { data, use_number, stack: Vec::new(), result: None, literal_start: None, number_err: None }
    }

    /// Handles the scanner result `op` for the byte at `i`.
    fn feed(&mut self, op: Op, i: usize) {
        if op == Op::Continue {
            return;
        }
        if let Some(start) = self.literal_start.take() {
            self.finish_literal(start, i);
        }
        match op {
            Op::BeginLiteral => self.literal_start = Some(i),
            Op::BeginArray => self.stack.push(Frame::Array(Vec::new())),
            Op::BeginObject => self.stack.push(Frame::Object { entries: Vec::new(), index: HashMap::new(), key: None }),
            Op::EndArray | Op::EndObject => {
                let value = match self.stack.pop() {
                    Some(Frame::Array(items)) => Json::Array(items),
                    Some(Frame::Object { entries, .. }) => Json::Object(entries),
                    None => return,
                };
                self.add_value(value);
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Result<Json, String> {
        if let Some(start) = self.literal_start.take() {
            self.finish_literal(start, self.data.len());
        }
        if let Some(err) = self.number_err {
            return Err(err);
        }
        self.result.ok_or_else(|| "unexpected end of JSON input".to_string())
    }

    fn finish_literal(&mut self, start: usize, end: usize) {
        let item = self.data.get(start..end).unwrap_or_default();
        if let Some(Frame::Object { key: key @ None, .. }) = self.stack.last_mut() {
            *key = Some(unquote(item));
            return;
        }
        let value = self.literal_value(item);
        self.add_value(value);
    }

    fn literal_value(&mut self, item: &[u8]) -> Json {
        match item.first() {
            Some(b'n') => Json::Null,
            Some(b't') => Json::Bool(true),
            Some(b'f') => Json::Bool(false),
            Some(b'"') => Json::String(unquote(item)),
            _ if self.use_number => Json::Number(item.to_vec()),
            _ => {
                let fits =
                    std::str::from_utf8(item).ok().and_then(|s| s.parse::<f64>().ok()).is_some_and(f64::is_finite);
                if fits {
                    return Json::Number(item.to_vec());
                }
                if self.number_err.is_none() {
                    self.number_err = Some(format!(
                        "json: cannot unmarshal number {} into Go value of type float64",
                        String::from_utf8_lossy(item)
                    ));
                }
                // Go leaves the interface nil.
                Json::Null
            }
        }
    }

    fn add_value(&mut self, value: Json) {
        match self.stack.last_mut() {
            None => self.result = Some(value),
            Some(Frame::Array(items)) => items.push(value),
            Some(Frame::Object { entries, index, key }) => {
                let key = key.take().unwrap_or_default();
                if let Some(&at) = index.get(&key) {
                    if let Some(entry) = entries.get_mut(at) {
                        entry.1 = value;
                    }
                } else {
                    index.insert(key.clone(), entries.len());
                    entries.push((key, value));
                }
            }
        }
    }
}

/// Go's `getu4`: the value of a `\uXXXX` escape at the start of `s`.
fn getu4(s: &[u8]) -> Option<u32> {
    if s.len() < 6 || s[0] != b'\\' || s[1] != b'u' {
        return None;
    }
    let mut r = 0u32;
    for &c in &s[2..6] {
        r = r * 16 + char::from(c).to_digit(16)?;
    }
    Some(r)
}

fn is_surrogate(r: u32) -> bool {
    (0xd800..0xe000).contains(&r)
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Go's `unquoteBytes` for a scanner-validated string literal (quotes included).
fn unquote(item: &[u8]) -> Vec<u8> {
    let s = match item {
        [b'"', inner @ .., b'"'] => inner,
        _ => item,
    };
    let mut out = Vec::with_capacity(s.len());
    let mut r = 0;
    while let Some(&c) = s.get(r) {
        if c == b'\\' {
            let Some(&e) = s.get(r + 1) else {
                break;
            };
            let simple = match e {
                b'"' | b'\\' | b'/' | b'\'' => Some(e),
                b'b' => Some(0x08),
                b'f' => Some(0x0c),
                b'n' => Some(b'\n'),
                b'r' => Some(b'\r'),
                b't' => Some(b'\t'),
                _ => None,
            };
            if let Some(b) = simple {
                out.push(b);
                r += 2;
                continue;
            }
            let Some(mut rr) = getu4(&s[r..]) else {
                break;
            };
            r += 6;
            if is_surrogate(rr) {
                let low = s.get(r..).and_then(getu4);
                match low {
                    Some(lo) if rr < 0xdc00 && (0xdc00..0xe000).contains(&lo) => {
                        rr = 0x10000 + ((rr - 0xd800) << 10) + (lo - 0xdc00);
                        r += 6;
                    }
                    _ => rr = 0xfffd,
                }
            }
            push_char(&mut out, char::from_u32(rr).unwrap_or('\u{fffd}'));
        } else if c < 0x80 {
            out.push(c);
            r += 1;
        } else {
            let (ch, size) = decode_rune(&s[r..]);
            push_char(&mut out, ch.unwrap_or('\u{fffd}'));
            r += size.max(1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(s: &str) -> Json {
        Json::Number(s.as_bytes().to_vec())
    }

    fn string(s: &[u8]) -> Json {
        Json::String(s.to_vec())
    }

    #[test]
    fn unmarshal_values() {
        assert_eq!(unmarshal(b" null "), Ok(Json::Null));
        assert_eq!(unmarshal(b"true"), Ok(Json::Bool(true)));
        assert_eq!(unmarshal(b"-0.5e+3"), Ok(num("-0.5e+3")));
        assert_eq!(
            unmarshal(b"[1,[],{},\"x\"]"),
            Ok(Json::Array(vec![num("1"), Json::Array(vec![]), Json::Object(vec![]), string(b"x")]))
        );
    }

    #[test]
    fn unmarshal_duplicate_keys() {
        assert_eq!(
            unmarshal(b"{\"a\":1,\"b\":2,\"\\u0061\":3}"),
            Ok(Json::Object(vec![(b"a".to_vec(), num("3")), (b"b".to_vec(), num("2"))]))
        );
    }

    #[test]
    fn unmarshal_strings() {
        assert_eq!(unmarshal(b"\"\\u0000\""), Ok(string(b"\0")));
        assert_eq!(unmarshal(b"\"\\ud83d\\ude00\""), Ok(string("😀".as_bytes())));
        assert_eq!(unmarshal(b"\"\\ud83dx\""), Ok(string("\u{fffd}x".as_bytes())));
        assert_eq!(unmarshal(b"\"\\ude00\\ud83d\""), Ok(string("\u{fffd}\u{fffd}".as_bytes())));
        assert_eq!(unmarshal(b"\"\\ud83d\\u0041\""), Ok(string("\u{fffd}A".as_bytes())));
        assert_eq!(unmarshal(b"\"a\xffb\xe2\x82\""), Ok(string("a\u{fffd}b\u{fffd}\u{fffd}".as_bytes())));
        assert_eq!(unmarshal(b"\"\\b\\f\\n\\r\\t\\/\\\\\\\"\""), Ok(string(b"\x08\x0c\n\r\t/\\\"")));
    }

    #[test]
    fn unmarshal_number_errors() {
        assert_eq!(
            unmarshal(b"[1e400, -1e999]"),
            Err("json: cannot unmarshal number 1e400 into Go value of type float64".to_string())
        );
        assert_eq!(unmarshal(b"1e-400"), Ok(num("1e-400")));
        assert_eq!(unmarshal(b"[1e400, x]"), Err("invalid character 'x' looking for beginning of value".to_string()));
    }

    #[test]
    fn unmarshal_syntax_errors() {
        assert_eq!(unmarshal(b""), Err("unexpected end of JSON input".to_string()));
        assert_eq!(unmarshal(b"{\"a\"}"), Err("invalid character '}' after object key".to_string()));
        assert_eq!(unmarshal(b"[1] 2"), Err("invalid character '2' after top-level value".to_string()));
    }

    #[test]
    fn decoder_values() {
        assert_eq!(decode_use_number(b" 12 "), Ok((num("12"), 3)));
        assert_eq!(decode_use_number(b"12"), Ok((num("12"), 2)));
        assert_eq!(decode_use_number(b"1e400"), Ok((num("1e400"), 5)));
        assert_eq!(decode_use_number(b"123x"), Ok((num("123"), 3)));
        assert_eq!(decode_use_number(b"[1]x"), Ok((Json::Array(vec![num("1")]), 3)));
        assert_eq!(decode_use_number(b"true"), Ok((Json::Bool(true), 4)));
        assert_eq!(decode_use_number(b"\"a\"\"b\""), Ok((string(b"a"), 3)));
    }

    #[test]
    fn decoder_errors() {
        assert_eq!(decode_use_number(b""), Err("EOF".to_string()));
        assert_eq!(decode_use_number(b" \n\t"), Err("EOF".to_string()));
        assert_eq!(decode_use_number(b"[1,"), Err("unexpected EOF".to_string()));
        assert_eq!(decode_use_number(b"tru"), Err("unexpected EOF".to_string()));
        assert_eq!(decode_use_number(b"-"), Err("unexpected EOF".to_string()));
        assert_eq!(decode_use_number(b"x1"), Err("invalid character 'x' looking for beginning of value".to_string()));
        assert_eq!(decode_use_number(b"[1 2]"), Err("invalid character '2' after array element".to_string()));
    }

    #[test]
    fn whitespace_follows() {
        assert!(only_whitespace_follows(b"1 \n", 1));
        assert!(only_whitespace_follows(b"1", 5));
        assert!(!only_whitespace_follows(b"1 2", 1));
    }

    #[test]
    fn valid_inputs() {
        assert!(valid(b" {\"a\":[1,2,{}]} "));
        assert!(!valid(b"{\"a\":[1,2,{}]"));
        assert!(!valid(b""));
        assert!(!valid(b"1 2"));
    }

    #[test]
    fn deep_nesting() {
        // Dropping a 10000-deep value recurses; give it more than the default test stack.
        let run = || {
            let mut doc = vec![b'['; 10000];
            doc.extend(vec![b']'; 10000]);
            assert!(unmarshal(&doc).is_ok());
            assert!(decode_use_number(&doc).is_ok());
            let mut doc = vec![b'['; 10001];
            doc.extend(vec![b']'; 10001]);
            assert_eq!(unmarshal(&doc), Err("invalid character '[' exceeded max depth".to_string()));
        };
        let handle = std::thread::Builder::new().stack_size(64 << 20).spawn(run);
        assert!(handle.is_ok_and(|h| h.join().is_ok()));
    }

    #[test]
    fn errors_at_end_of_input() {
        let cases: &[(&[u8], &str, &str)] = &[
            (b"\"\\u12", "invalid character ' ' in \\u hexadecimal character escape", "unexpected EOF"),
            (b"[1,2", "unexpected end of JSON input", "unexpected EOF"),
            (b"{\"a\":", "unexpected end of JSON input", "unexpected EOF"),
            (b"-", "invalid character ' ' in numeric literal", "unexpected EOF"),
            (b"1e+", "invalid character ' ' in exponent of numeric literal", "unexpected EOF"),
            (b"[\"\\", "invalid character ' ' in string escape code", "unexpected EOF"),
            (b"\t", "unexpected end of JSON input", "EOF"),
        ];
        for (doc, unmarshal_err, decode_err) in cases {
            assert_eq!(unmarshal(doc), Err(unmarshal_err.to_string()), "{doc:?}");
            assert_eq!(decode_use_number(doc), Err(decode_err.to_string()), "{doc:?}");
        }
    }

    #[test]
    fn decoder_keeps_huge_numbers() {
        let doc = b"{\"a\":1e999,\"b\":[2e999]}";
        assert_eq!(
            unmarshal(doc),
            Err("json: cannot unmarshal number 1e999 into Go value of type float64".to_string())
        );
        let (value, offset) = decode_use_number(doc).unwrap_or((Json::Null, 0));
        assert_eq!(offset, 23);
        assert_eq!(
            value,
            Json::Object(vec![(b"a".to_vec(), num("1e999")), (b"b".to_vec(), Json::Array(vec![num("2e999")]))])
        );
    }

    #[test]
    fn trailing_garbage() {
        assert_eq!(unmarshal(b"1x"), Err("invalid character 'x' after top-level value".to_string()));
        assert_eq!(decode_use_number(b"1x"), Ok((num("1"), 1)));
        assert!(!only_whitespace_follows(b"1x", 1));
    }
}
