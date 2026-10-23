//! `json.Indent`, as used by `json.MarshalIndent`.
//!
//! Follows the behaviour of Go's standard library encoding/json package (The Go Authors, BSD-style licence).

use super::scanner::{Op, Scanner};

/// Go's `appendIndent` on an empty destination: each array element and object member starts on a new line
/// beginning with `prefix` followed by one copy of `indent` per nesting level. Empty arrays and objects stay
/// `[]` and `{}`, a space follows each `:`, whitespace outside strings is dropped, and no prefix is put
/// before the first line (exactly like `MarshalIndent`).
///
/// If `src` is not valid JSON, Go returns an error and leaves the destination untouched; this returns an
/// empty vector then.
pub fn indent(src: &[u8], prefix: &[u8], indent: &[u8]) -> Vec<u8> {
    let mut dst = Vec::with_capacity(src.len() * 2);
    let mut scan = Scanner::new();
    let mut need_indent = false;
    let mut depth = 0usize;
    for &c in src {
        scan.bytes += 1;
        let op = scan.step(c);
        if op == Op::SkipSpace {
            continue;
        }
        if op == Op::Error {
            break;
        }
        if need_indent && op != Op::EndObject && op != Op::EndArray {
            need_indent = false;
            depth += 1;
            newline(&mut dst, prefix, indent, depth);
        }
        if op == Op::Continue {
            dst.push(c);
            continue;
        }
        match c {
            b'{' | b'[' => {
                need_indent = true;
                dst.push(c);
            }
            b',' => {
                dst.push(c);
                newline(&mut dst, prefix, indent, depth);
            }
            b':' => dst.extend_from_slice(b": "),
            b'}' | b']' => {
                if need_indent {
                    need_indent = false;
                } else {
                    depth = depth.saturating_sub(1);
                    newline(&mut dst, prefix, indent, depth);
                }
                dst.push(c);
            }
            _ => dst.push(c),
        }
    }
    if scan.eof() == Op::Error {
        return Vec::new();
    }
    dst
}

fn newline(dst: &mut Vec<u8>, prefix: &[u8], indent: &[u8], depth: usize) {
    dst.push(b'\n');
    dst.extend_from_slice(prefix);
    for _ in 0..depth {
        dst.extend_from_slice(indent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ind(src: &str, prefix: &str, ind: &str) -> String {
        String::from_utf8(indent(src.as_bytes(), prefix.as_bytes(), ind.as_bytes())).unwrap_or_default()
    }

    #[test]
    fn scalars() {
        assert_eq!(ind("1", "", "  "), "1");
        assert_eq!(ind("\"a b\"", ">", "  "), "\"a b\"");
    }

    #[test]
    fn empty_containers() {
        assert_eq!(ind("[]", "", "  "), "[]");
        assert_eq!(ind("{}", "", "  "), "{}");
        assert_eq!(ind("[{},[]]", "", "\t"), "[\n\t{},\n\t[]\n]");
    }

    #[test]
    fn nested() {
        assert_eq!(
            ind("{\"a\":[1,2],\"b\":{\"c\":null}}", "", "  "),
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {\n    \"c\": null\n  }\n}"
        );
        assert_eq!(ind("[1]", "P", "I"), "[\nPI1\nP]");
    }

    #[test]
    fn invalid() {
        assert_eq!(ind("[1,", "", "  "), "");
        assert_eq!(ind("", "", "  "), "");
    }

    #[test]
    fn brackets_inside_strings() {
        assert_eq!(
            ind("{\"a\":\"[1,{}]\",\"b\":[[]],\"c\":{\"d\":{}}}", "", "\t"),
            "{\n\t\"a\": \"[1,{}]\",\n\t\"b\": [\n\t\t[]\n\t],\n\t\"c\": {\n\t\t\"d\": {}\n\t}\n}"
        );
    }
}
