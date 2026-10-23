//! Operator precedence and associativity.

/// How operators of equal precedence group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Associativity {
    None,
    Left,
    Right,
}

/// The precedence and associativity of an operator.
#[derive(Clone, Copy, Debug)]
pub struct Operator {
    pub precedence: i32,
    pub associativity: Associativity,
}

const UNKNOWN: Operator = Operator { precedence: 0, associativity: Associativity::None };

const fn left(precedence: i32) -> Operator {
    Operator { precedence, associativity: Associativity::Left }
}

/// The unary operator table; unknown operators have precedence 0.
pub fn unary(op: &str) -> Operator {
    match op {
        "not" | "!" => left(50),
        "-" | "+" => left(90),
        _ => UNKNOWN,
    }
}

/// Looks a binary operator up.
pub fn lookup_binary(op: &[u8]) -> Option<Operator> {
    Some(match op {
        b"|" => left(0),
        b"or" | b"||" => left(10),
        b"and" | b"&&" => left(15),
        b"==" | b"!=" | b"<" | b">" | b">=" | b"<=" | b"in" | b"matches" | b"contains" | b"startsWith"
        | b"endsWith" => left(20),
        b".." => left(25),
        b"+" | b"-" => left(30),
        b"*" | b"/" | b"%" => left(60),
        b"**" | b"^" => Operator { precedence: 100, associativity: Associativity::Right },
        b"??" => left(500),
        _ => return None,
    })
}

/// The binary operator table; unknown operators have precedence 0.
pub fn binary(op: &str) -> Operator {
    lookup_binary(op.as_bytes()).unwrap_or(UNKNOWN)
}

pub fn is_boolean(op: &str) -> bool {
    matches!(op, "and" | "or" | "&&" | "||")
}

pub fn allowed_negate_suffix(op: &[u8]) -> bool {
    matches!(op, b"contains" | b"matches" | b"startsWith" | b"endsWith" | b"in")
}

pub fn is_comparison(op: &[u8]) -> bool {
    matches!(op, b"<" | b">" | b">=" | b"<=")
}
