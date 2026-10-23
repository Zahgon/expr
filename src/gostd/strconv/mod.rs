//! Conversions between numbers and strings, and Go string quoting.
//!
//! Follows the behaviour of Go's standard library strconv package (The Go Authors, BSD-style licence).
//! Also provides the few `fmt` formatting rules that build on strconv ([`fmt_v_float`],
//! [`fmt_q`], [`fmt_sharp_u`]), following Go's fmt package.

mod atof;
mod atoi;
mod decimal;
mod ftoa;
mod itoa;
mod quote;

pub use atof::parse_float;
pub use atoi::{atoi, parse_int, parse_uint, NumError, NumErrorKind};
pub use ftoa::format_float;
pub use itoa::{format_int, format_uint, itoa};
pub use quote::{is_print, quote, quote_rune, quote_to_ascii};

/// What Go's fmt `%v` prints for a float64 (`bit_size` 64) or float32 (`bit_size` 32):
/// strconv's shortest `'g'` form (precision -1), giving e.g. `1e+06`, `1.234567e+06`,
/// `123456`, `0.0001`, `1e-05`, `-0`, `+Inf`, `-Inf` and `NaN`.
pub fn fmt_v_float(f: f64, bit_size: u32) -> String {
    // fmt keeps the sign of infinities ("+Inf") and drops '+' from numbers; strconv's
    // 'g' output already has exactly that shape.
    format_float(f, b'g', -1, bit_size)
}

/// What Go's fmt `%q` prints for a string: `strconv.Quote` of it.
pub fn fmt_q(s: &[u8]) -> String {
    quote(s)
}

/// What Go's fmt `%#U` prints for a rune: `U+0041 'A'`, at least four upper-case hex digits,
/// with the quoted character only when the rune is printable. The `u32` is read as a Go
/// `rune` (int32), so values from 0x8000_0000 print as negative runes do
/// (`U+FFFFFFFFFFFFFFFF` for -1).
pub fn fmt_sharp_u(rune: u32) -> String {
    // fmt converts the int32 rune to uint64 with sign extension.
    let u = i64::from(rune as i32) as u64;
    let mut s = format!("U+{u:04X}");
    if u <= u64::from(super::utf8::MAX_RUNE) && is_print(rune) {
        s.push_str(" '");
        let mut buf = Vec::new();
        super::utf8::encode_rune(rune, &mut buf);
        s.push_str(&String::from_utf8_lossy(&buf));
        s.push('\'');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_text(r: Result<impl std::fmt::Debug, NumError>) -> String {
        r.unwrap_err().to_string()
    }

    #[test]
    fn format_float_shortest() {
        assert_eq!(format_float(0.1, b'g', -1, 64), "0.1");
        assert_eq!(format_float(0.1, b'g', -1, 32), "0.1");
        assert_eq!(format_float(0.1, b'e', -1, 64), "1e-01");
        assert_eq!(format_float(1e21, b'f', -1, 64), "1000000000000000000000");
        assert_eq!(format_float(1e21, b'g', -1, 64), "1e+21");
        assert_eq!(format_float(123456.0, b'g', -1, 64), "123456");
        assert_eq!(format_float(1234567.0, b'G', -1, 64), "1.234567E+06");
        assert_eq!(format_float(5e-324, b'g', -1, 64), "5e-324");
        assert_eq!(format_float(f64::MAX, b'g', -1, 64), "1.7976931348623157e+308");
        assert_eq!(format_float(-0.0, b'g', -1, 64), "-0");
        assert_eq!(format_float(0.0, b'e', -1, 64), "0e+00");
        assert_eq!(format_float(0.1 + 0.2, b'g', -1, 64), "0.30000000000000004");
    }

    #[test]
    fn format_float_fixed() {
        assert_eq!(format_float(12.345678, b'f', 2, 64), "12.35");
        assert_eq!(format_float(2.5, b'f', 0, 64), "2");
        assert_eq!(format_float(3.5, b'f', 0, 64), "4");
        assert_eq!(format_float(0.125, b'f', 2, 64), "0.12");
        assert_eq!(format_float(1.0, b'e', 3, 64), "1.000e+00");
        assert_eq!(format_float(1e100, b'e', 0, 64), "1e+100");
        assert_eq!(format_float(0.1, b'f', 20, 64), "0.10000000000000000555");
        assert_eq!(format_float(100.0, b'g', 2, 64), "1e+02");
        assert_eq!(format_float(0.00001234, b'g', 3, 64), "1.23e-05");
        assert_eq!(format_float(1.0, b'b', -1, 64), "4503599627370496p-52");
        assert_eq!(format_float(1.0, b'x', -1, 64), "0x1p+00");
        assert_eq!(format_float(1.5, b'X', 3, 64), "0X1.800P+00");
    }

    #[test]
    fn format_float_specials() {
        assert_eq!(format_float(f64::INFINITY, b'g', -1, 64), "+Inf");
        assert_eq!(format_float(f64::NEG_INFINITY, b'f', 2, 32), "-Inf");
        assert_eq!(format_float(f64::NAN, b'e', 3, 64), "NaN");
        assert_eq!(format_float(1e40, b'g', -1, 32), "+Inf");
        assert_eq!(format_float(1.0, b'q', -1, 64), "%q");
    }

    #[test]
    fn fmt_helpers() {
        assert_eq!(fmt_v_float(1e6, 64), "1e+06");
        assert_eq!(fmt_v_float(1234567.0, 64), "1.234567e+06");
        assert_eq!(fmt_v_float(123456.0, 64), "123456");
        assert_eq!(fmt_v_float(0.0001, 64), "0.0001");
        assert_eq!(fmt_v_float(0.00001, 64), "1e-05");
        assert_eq!(fmt_v_float(f64::INFINITY, 64), "+Inf");
        assert_eq!(fmt_v_float(f64::NAN, 64), "NaN");
        assert_eq!(fmt_v_float(0.1, 32), "0.1");
        assert_eq!(fmt_q(b"a\"b\n\xff"), r#""a\"b\n\xff""#);
        assert_eq!(fmt_sharp_u(0x41), "U+0041 'A'");
        assert_eq!(fmt_sharp_u(0x1F600), "U+1F600 '\u{1F600}'");
        assert_eq!(fmt_sharp_u(0x0A), "U+000A");
        assert_eq!(fmt_sharp_u(0x110000), "U+110000");
        assert_eq!(fmt_sharp_u(u32::MAX), "U+FFFFFFFFFFFFFFFF");
    }

    #[test]
    fn integers() {
        assert_eq!(format_int(-255, 16), "-ff");
        assert_eq!(format_int(i64::MIN, 10), "-9223372036854775808");
        assert_eq!(format_uint(u64::MAX, 36), "3w5e11264sgsf");
        assert_eq!(format_uint(5, 2), "101");
        assert_eq!(itoa(42), "42");
    }

    #[test]
    fn quoting() {
        assert_eq!(quote(b"hello"), "\"hello\"");
        assert_eq!(quote("\u{263a}\t\u{7}".as_bytes()), "\"\u{263a}\\t\\a\"");
        assert_eq!(quote("\u{ad}\u{a0}\u{2028}".as_bytes()), r#""\u00ad\u00a0\u2028""#);
        assert_eq!(quote(b"\x00\x7f\\"), r#""\x00\x7f\\""#);
        assert_eq!(quote("\u{e0001}".as_bytes()), r#""\U000e0001""#);
        assert_eq!(quote_to_ascii("é\u{1F600}".as_bytes()), r#""\u00e9\U0001f600""#);
        assert_eq!(quote_rune('\'' as u32), r"'\''");
        assert_eq!(quote_rune('"' as u32), "'\"'");
        assert_eq!(quote_rune(0xD800), "'\u{FFFD}'");
        assert_eq!(quote_rune(0x110000), "'\u{FFFD}'");
        assert_eq!(quote_rune(0xFEFF), r"'\ufeff'");
    }

    #[test]
    fn parse_integers() {
        assert_eq!(parse_int(b"-0x_1F", 0, 64), Ok(-31));
        assert_eq!(parse_int(b"0b101", 0, 8), Ok(5));
        assert_eq!(parse_int(b"017", 0, 0), Ok(15));
        assert_eq!(parse_int(b"-128", 10, 8), Ok(-128));
        assert_eq!(parse_int(b"-2", 10, 1), Ok(-1));
        assert_eq!(parse_uint(b"18446744073709551615", 10, 64), Ok(u64::MAX));
        assert_eq!(parse_uint(b"zz", 36, 0), Ok(1295));
        assert_eq!(atoi(b"-123"), Ok(-123));
        assert_eq!(atoi(b"+7"), Ok(7));
        assert_eq!(err_text(parse_int(b"128", 10, 8)), "strconv.ParseInt: parsing \"128\": value out of range");
        assert_eq!(err_text(parse_int(b"1_0", 10, 64)), "strconv.ParseInt: parsing \"1_0\": invalid syntax");
        assert_eq!(err_text(parse_int(b"1", 1, 64)), "strconv.ParseInt: parsing \"1\": invalid base 1");
        assert_eq!(err_text(parse_uint(b"1", 10, 65)), "strconv.ParseUint: parsing \"1\": invalid bit size 65");
        assert_eq!(err_text(atoi(b"12a")), "strconv.Atoi: parsing \"12a\": invalid syntax");
        assert_eq!(
            err_text(atoi(b"99999999999999999999")),
            "strconv.Atoi: parsing \"99999999999999999999\": value out of range"
        );
        assert_eq!(err_text(atoi(b"\xff")), "strconv.Atoi: parsing \"\\xff\": invalid syntax");
        assert_eq!(err_text(atoi(b"")), "strconv.Atoi: parsing \"\": invalid syntax");
    }

    #[test]
    fn parse_floats() {
        assert_eq!(parse_float(b"1.5", 64), Ok(1.5));
        assert_eq!(parse_float(b"-1_000.5e-1", 64), Ok(-100.05));
        assert_eq!(parse_float(b"0x1.8p1", 64), Ok(3.0));
        assert_eq!(parse_float(b"0x_1p-2", 64), Ok(0.25));
        assert_eq!(parse_float(b"1e-400", 64), Ok(0.0));
        assert_eq!(parse_float(b"0.1", 32), Ok(f64::from(0.1f32)));
        assert_eq!(parse_float(b"-InFiNiTy", 64), Ok(f64::NEG_INFINITY));
        assert_eq!(parse_float(b"+inf", 64), Ok(f64::INFINITY));
        assert!(parse_float(b"nAn", 64).unwrap().is_nan());
        assert_eq!(parse_float(b"2.2250738585072011e-308", 64), Ok(f64::from_bits(0x000f_ffff_ffff_ffff)));
        assert_eq!(parse_float(b"9007199254740993", 64), Ok(9007199254740992.0));
        let e = parse_float(b"1e309", 64).unwrap_err();
        assert_eq!(e.kind, NumErrorKind::Range);
        assert_eq!(e.value_on_range, f64::INFINITY);
        assert_eq!(e.to_string(), "strconv.ParseFloat: parsing \"1e309\": value out of range");
        let e = parse_float(b"-3.5e38", 32).unwrap_err();
        assert_eq!(e.value_on_range, f64::NEG_INFINITY);
        assert_eq!(err_text(parse_float(b"+nan", 64)), "strconv.ParseFloat: parsing \"+nan\": invalid syntax");
        assert_eq!(err_text(parse_float(b"1e309x", 64)), "strconv.ParseFloat: parsing \"1e309x\": invalid syntax");
        assert!(parse_float(b"0x1p", 64).is_err());
        assert!(parse_float(b"0x1", 64).is_err());
        assert!(parse_float(b"1__0", 64).is_err());
        assert!(parse_float(b"infin", 64).is_err());
    }
}
