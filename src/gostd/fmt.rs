//! Follows the behaviour of Go's standard library fmt package (The Go Authors, BSD-style licence).
//!
//! Only what the language needs beyond value printing (see `crate::value`): `fmt.Sprintf` called with a
//! format string and no arguments, which is how the original reports some messages.

/// `fmt.Sprintf(format)` with no arguments: `%%` prints `%`, every other directive prints `%!v(MISSING)`
/// (with its verb), and a trailing `%` prints `%!(NOVERB)`.
pub fn sprintf_no_args(format: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(format.len());
    let mut i = 0;
    while i < format.len() {
        if format[i] != b'%' {
            out.push(format[i]);
            i += 1;
            continue;
        }
        i += 1;
        // Flags.
        while i < format.len() && matches!(format[i], b'#' | b'0' | b'+' | b'-' | b' ') {
            i += 1;
        }
        // Argument index, width and precision.
        let mut bad_index = false;
        if i < format.len() && format[i] == b'[' {
            match format[i..].iter().position(|&b| b == b']') {
                Some(end) => i += end + 1,
                None => {
                    bad_index = true;
                    i = format.len();
                }
            }
        }
        while i < format.len() && (format[i].is_ascii_digit() || format[i] == b'*') {
            i += 1;
        }
        if i < format.len() && format[i] == b'.' {
            i += 1;
            while i < format.len() && (format[i].is_ascii_digit() || format[i] == b'*') {
                i += 1;
            }
        }
        if bad_index {
            out.extend_from_slice(b"%!(BADINDEX)");
            continue;
        }
        if i >= format.len() {
            out.extend_from_slice(b"%!(NOVERB)");
            break;
        }
        let (verb, size) = super::utf8::decode_rune(&format[i..]);
        i += size;
        if verb == u32::from('%') {
            out.push(b'%');
            continue;
        }
        out.extend_from_slice(b"%!");
        super::utf8::encode_rune(verb, &mut out);
        out.extend_from_slice(b"(MISSING)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sprintf_no_args;

    #[test]
    fn reports_missing_arguments_like_go() {
        assert_eq!(sprintf_no_args(b"plain"), b"plain");
        assert_eq!(sprintf_no_args(b"100%% sure"), b"100% sure");
        assert_eq!(sprintf_no_args(b"`(%`"), b"`(%!`(MISSING)");
        assert_eq!(sprintf_no_args(b"a%5.2fb"), b"a%!f(MISSING)b");
        assert_eq!(sprintf_no_args(b"end%"), b"end%!(NOVERB)");
    }
}
