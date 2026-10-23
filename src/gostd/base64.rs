//! Standard base64 encoding (RFC 4648, with padding), like Go's `base64.StdEncoding`.
//!
//! Follows the behaviour of Go's standard library encoding/base64 package (The Go Authors, BSD-style licence).

const ENCODE_STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const PAD: u8 = b'=';

/// Returns the standard base64 encoding of `src` (`base64.StdEncoding.EncodeToString`).
pub fn std_encode(src: &[u8]) -> String {
    let mut out = String::with_capacity(src.len().div_ceil(3) * 4);
    for chunk in src.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let val = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let sym = |shift: u32| char::from(ENCODE_STD[((val >> shift) & 0x3F) as usize]);
        out.push(sym(18));
        out.push(sym(12));
        out.push(if chunk.len() > 1 { sym(6) } else { char::from(PAD) });
        out.push(if chunk.len() > 2 { sym(0) } else { char::from(PAD) });
    }
    out
}

fn decode_byte(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn corrupt(offset: usize) -> String {
    format!("illegal base64 data at input byte {offset}")
}

fn is_newline(c: u8) -> bool {
    c == b'\n' || c == b'\r'
}

/// Decodes one quantum of up to four symbols starting at `si`, skipping '\r' and '\n'.
/// Returns the next input index, or Go's `CorruptInputError` text. A quantum ending in
/// padding must be the end of the input (apart from newlines).
fn decode_quantum(dst: &mut Vec<u8>, src: &[u8], mut si: usize) -> Result<usize, String> {
    let mut dbuf = [0u8; 4];
    let mut dlen = 4;
    let mut j = 0;
    let mut trailing_err = None;
    while j < 4 {
        if si == src.len() {
            if j == 0 {
                return Ok(si);
            }
            // Missing padding.
            return Err(corrupt(si - j));
        }
        let c = src[si];
        si += 1;
        if let Some(v) = decode_byte(c) {
            dbuf[j] = v;
            j += 1;
            continue;
        }
        if is_newline(c) {
            continue;
        }
        if c != PAD {
            return Err(corrupt(si - 1));
        }
        // We've reached the end and there's padding.
        match j {
            0 | 1 => return Err(corrupt(si - 1)), // incorrect padding
            2 => {
                // "==" is expected; skip over newlines first.
                while si < src.len() && is_newline(src[si]) {
                    si += 1;
                }
                if si == src.len() {
                    return Err(corrupt(src.len()));
                }
                if src[si] != PAD {
                    return Err(corrupt(si - 1));
                }
                si += 1;
            }
            _ => {}
        }
        // Skip over newlines; anything else left over is an error.
        while si < src.len() && is_newline(src[si]) {
            si += 1;
        }
        if si < src.len() {
            trailing_err = Some(corrupt(si));
        }
        dlen = j;
        break;
    }
    let val = (u32::from(dbuf[0]) << 18) | (u32::from(dbuf[1]) << 12) | (u32::from(dbuf[2]) << 6) | u32::from(dbuf[3]);
    let bytes = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
    dst.extend_from_slice(&bytes[..dlen - 1]);
    match trailing_err {
        Some(e) => Err(e),
        None => Ok(si),
    }
}

/// Decodes standard base64 (`base64.StdEncoding.DecodeString`). '\r' and '\n' are ignored
/// anywhere; padding is required. Errors carry Go's text `illegal base64 data at input byte N`.
pub fn std_decode(src: &[u8]) -> Result<Vec<u8>, String> {
    let mut dst = Vec::with_capacity(src.len() / 4 * 3);
    let mut si = 0;
    while si < src.len() {
        si = decode_quantum(&mut dst, src, si)?;
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode() {
        assert_eq!(std_encode(b""), "");
        assert_eq!(std_encode(b"f"), "Zg==");
        assert_eq!(std_encode(b"fo"), "Zm8=");
        assert_eq!(std_encode(b"foo"), "Zm9v");
        assert_eq!(std_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(std_encode(b"\xff\xfe\xfd"), "//79");
    }

    #[test]
    fn decode() {
        assert_eq!(std_decode(b"Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(std_decode(b"Zg==").unwrap(), b"f");
        assert_eq!(std_decode(b"Zm8=").unwrap(), b"fo");
        assert_eq!(std_decode(b"Zm\r\n9v").unwrap(), b"foo");
        assert_eq!(std_decode(b"Zg=\n=\n").unwrap(), b"f");
        assert_eq!(std_decode(b"").unwrap(), b"");
        assert_eq!(std_decode(b"\n\r").unwrap(), b"");
        assert_eq!(std_decode(b"Zh==").unwrap(), b"f");
    }

    #[test]
    fn decode_errors() {
        let err = |s: &[u8]| std_decode(s).unwrap_err();
        assert_eq!(err(b"Zg"), "illegal base64 data at input byte 0");
        assert_eq!(err(b"Zm9vY"), "illegal base64 data at input byte 4");
        assert_eq!(err(b"Zg="), "illegal base64 data at input byte 3");
        assert_eq!(err(b"Zg==Zg=="), "illegal base64 data at input byte 4");
        assert_eq!(err(b"Z==="), "illegal base64 data at input byte 1");
        assert_eq!(err(b"Zm9v!"), "illegal base64 data at input byte 4");
        assert_eq!(err(b"Zg=x"), "illegal base64 data at input byte 2");
        assert_eq!(err(b"Zm8=\nx"), "illegal base64 data at input byte 5");
    }
}
