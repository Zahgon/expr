//! Emulation of the parts of Go's standard library whose observable behaviour the language inherits: number
//! parsing and printing, string functions, Unicode tables, JSON, base64, regular expressions, dates and time
//! zones, sorting and `fmt` formatting.

pub mod base64;
pub mod fmt;
pub mod json;
pub mod math;
pub mod regexp;
pub mod sort;
pub mod strconv;
pub mod strings;
pub mod time;
pub mod unicode;
pub mod utf8;
