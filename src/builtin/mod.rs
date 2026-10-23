//! The builtin functions: their names, signatures and argument validation.
//!
//! The order of [`NAMES`] is the original's builtin table order; the compiler refers to builtins by that index.

pub mod functions;
pub mod json;

use crate::types::{Kind, Type};

/// The builtins, in the original's order.
pub static NAMES: [&str; 71] = [
    "all",
    "none",
    "any",
    "one",
    "filter",
    "map",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "count",
    "sum",
    "groupBy",
    "sortBy",
    "reduce",
    "len",
    "type",
    "abs",
    "ceil",
    "floor",
    "round",
    "int",
    "float",
    "string",
    "trim",
    "trimPrefix",
    "trimSuffix",
    "upper",
    "lower",
    "split",
    "splitAfter",
    "replace",
    "repeat",
    "join",
    "indexOf",
    "lastIndexOf",
    "hasPrefix",
    "hasSuffix",
    "max",
    "min",
    "mean",
    "median",
    "toJSON",
    "fromJSON",
    "toBase64",
    "fromBase64",
    "now",
    "duration",
    "date",
    "timezone",
    "first",
    "last",
    "get",
    "take",
    "keys",
    "values",
    "toPairs",
    "fromPairs",
    "reverse",
    "uniq",
    "concat",
    "flatten",
    "sort",
    "bitand",
    "bitor",
    "bitxor",
    "bitnand",
    "bitshl",
    "bitshr",
    "bitushr",
    "bitnot",
];

/// The index of a builtin in [`NAMES`].
pub fn index(name: &str) -> Option<usize> {
    NAMES.iter().position(|n| *n == name)
}

/// How the compiler calls a builtin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    /// Compiled inline (the predicate builtins).
    Predicate,
    /// One argument, no error result.
    Fast,
    /// A function of its arguments that may fail.
    Func,
    /// Like `Func`, and reports the memory it allocated.
    Safe,
}

/// How the compiler calls the builtin `name`.
pub fn call_kind(name: &str) -> Call {
    match name {
        "all" | "none" | "any" | "one" | "filter" | "map" | "find" | "findIndex" | "findLast" | "findLastIndex"
        | "count" | "sum" | "groupBy" | "sortBy" | "reduce" => Call::Predicate,
        "len" | "type" | "abs" | "ceil" | "floor" | "round" | "int" | "float" | "string" | "upper" | "lower" => {
            Call::Fast
        }
        "repeat" | "reverse" | "concat" | "flatten" | "sort" => Call::Safe,
        _ => Call::Func,
    }
}

fn f(ins: Vec<Type>, outs: Vec<Type>) -> Type {
    Type::func(ins, outs, false)
}

fn s() -> Type {
    Type::String
}

/// `func(interface {}) bool`.
fn pred(out: Type) -> Type {
    f(vec![Type::Any], vec![out])
}

/// The declared signatures of a builtin, tried in order as overloads. Empty when the builtin validates its
/// arguments itself or is special-cased by the checker.
pub fn types(name: &str) -> Vec<Type> {
    let any_slice = Type::any_slice;
    let strings = || Type::slice(Type::String);
    match name {
        "all" | "none" | "any" | "one" => vec![f(vec![any_slice(), pred(Type::Bool)], vec![Type::Bool])],
        "filter" => vec![f(vec![any_slice(), pred(Type::Bool)], vec![any_slice()])],
        "map" => vec![f(vec![any_slice(), pred(Type::Any)], vec![any_slice()])],
        "find" | "findLast" => vec![f(vec![any_slice(), pred(Type::Bool)], vec![Type::Any])],
        "findIndex" | "findLastIndex" | "count" | "sum" => {
            vec![f(vec![any_slice(), pred(Type::Bool)], vec![Type::Int])]
        }
        "groupBy" => vec![f(vec![any_slice(), pred(Type::Any)], vec![Type::map(Type::Any, any_slice())])],
        "sortBy" => vec![f(vec![any_slice(), pred(Type::Bool), s()], vec![any_slice()])],
        "reduce" => {
            vec![f(vec![any_slice(), f(vec![Type::Any, Type::Any], vec![Type::Any]), Type::Any], vec![Type::Any])]
        }
        "type" | "string" => vec![f(vec![Type::Any], vec![s()])],
        "trim" => vec![f(vec![s()], vec![s()]), f(vec![s(), s()], vec![s()])],
        "trimPrefix" | "trimSuffix" => vec![f(vec![s(), s()], vec![s()]), f(vec![s()], vec![s()])],
        "upper" | "lower" | "toBase64" | "fromBase64" => vec![f(vec![s()], vec![s()])],
        "split" | "splitAfter" => {
            vec![f(vec![s(), s()], vec![strings()]), f(vec![s(), s(), Type::Int], vec![strings()])]
        }
        "replace" => vec![f(vec![s(), s(), s(), Type::Int], vec![s()]), f(vec![s(), s(), s()], vec![s()])],
        "repeat" => vec![f(vec![s(), Type::Int], vec![s()])],
        "join" => vec![
            f(vec![strings(), s()], vec![s()]),
            f(vec![any_slice(), s()], vec![s()]),
            f(vec![any_slice()], vec![s()]),
            f(vec![strings(), s()], vec![s()]),
            f(vec![strings()], vec![s()]),
        ],
        "indexOf" | "lastIndexOf" => vec![f(vec![s(), s()], vec![Type::Int])],
        "hasPrefix" | "hasSuffix" => vec![f(vec![s(), s()], vec![Type::Bool])],
        "toJSON" => vec![f(vec![Type::Any], vec![s()])],
        "fromJSON" => vec![f(vec![s()], vec![Type::Any])],
        "duration" => vec![f(vec![s()], vec![Type::duration(), Type::Error])],
        "timezone" => vec![f(vec![s()], vec![Type::location_ptr(), Type::Error])],
        "sort" => vec![
            f(vec![any_slice(), s()], vec![any_slice()]),
            f(vec![Type::slice(Type::Int), s()], vec![any_slice()]),
            f(vec![Type::slice(Type::Float64), s()], vec![any_slice()]),
            f(vec![strings(), s()], vec![any_slice()]),
            f(vec![any_slice()], vec![any_slice()]),
            f(vec![Type::slice(Type::Float64)], vec![any_slice()]),
            f(vec![strings()], vec![any_slice()]),
            f(vec![Type::slice(Type::Int)], vec![any_slice()]),
        ],
        "bitand" | "bitor" | "bitxor" | "bitnand" | "bitshl" | "bitshr" | "bitushr" => {
            vec![f(vec![Type::Int, Type::Int], vec![Type::Int])]
        }
        "bitnot" => vec![f(vec![Type::Int], vec![Type::Int])],
        _ => Vec::new(),
    }
}

/// The type of a builtin used as a value: its first signature, or the generic function type.
pub fn func_type(name: &str) -> Type {
    types(name)
        .into_iter()
        .next()
        .unwrap_or_else(|| Type::func(vec![Type::any_slice()], vec![Type::Any, Type::Error], true))
}

/// Formats a `reflect.Type` the way `%s` does, including a nil type.
fn ts(t: &Option<Type>) -> String {
    t.as_ref().map_or_else(|| "%!s(<nil>)".to_string(), |t| t.to_string())
}

fn kind(t: &Option<Type>) -> Kind {
    let mut t = match t {
        Some(t) => t,
        None => return Kind::Invalid,
    };
    while let Type::Ptr(e) = t {
        t = e;
    }
    t.kind()
}

fn is_number_kind(k: Kind) -> bool {
    k.is_integer() && k != Kind::Uintptr || k.is_float()
}

fn assignable_to_location(t: &Option<Type>) -> bool {
    t.as_ref().is_some_and(|t| t.assignable_to(&Type::location_ptr()))
}

fn validate_aggregate(name: &str, args: &[Option<Type>]) -> Result<Type, String> {
    if args.is_empty() {
        return Err(format!("not enough arguments to call {name}"));
    }
    for arg in args {
        match kind(arg) {
            Kind::Interface | Kind::Array | Kind::Slice => return Ok(Type::Any),
            k if is_number_kind(k) => {}
            _ => return Err(format!("invalid argument for {name} (type {})", ts(arg))),
        }
    }
    Ok(args[0].clone().unwrap_or(Type::Any))
}

fn validate_round(name: &str, args: &[Option<Type>]) -> Result<Type, String> {
    if args.len() != 1 {
        return Err(format!("invalid number of arguments (expected 1, got {})", args.len()));
    }
    match kind(&args[0]) {
        k if is_number_kind(k) || k == Kind::Interface => Ok(Type::Float64),
        _ => Err(format!("invalid argument for {name} (type {})", ts(&args[0]))),
    }
}

fn expect_one(args: &[Option<Type>]) -> Result<(), String> {
    if args.len() != 1 {
        return Err(format!("invalid number of arguments (expected 1, got {})", args.len()));
    }
    Ok(())
}

/// Runs the builtin's own argument validation; `None` when the builtin has none.
pub fn validate(name: &str, args: &[Option<Type>]) -> Option<Result<Type, String>> {
    let any = Type::Any;
    Some(match name {
        "len" => expect_one(args).and_then(|()| match kind(&args[0]) {
            Kind::Array | Kind::Map | Kind::Slice | Kind::String | Kind::Interface => Ok(Type::Int),
            _ => Err(format!("invalid argument for len (type {})", ts(&args[0]))),
        }),
        "abs" => expect_one(args).and_then(|()| match kind(&args[0]) {
            k if is_number_kind(k) || k == Kind::Interface => Ok(args[0].clone().unwrap_or(Type::Any)),
            _ => Err(format!("invalid argument for abs (type {})", ts(&args[0]))),
        }),
        "ceil" | "floor" | "round" => validate_round(name, args),
        "int" | "float" => expect_one(args).and_then(|()| match kind(&args[0]) {
            k if is_number_kind(k) || k == Kind::Interface || k == Kind::String => {
                Ok(if name == "int" { Type::Int } else { Type::Float64 })
            }
            _ => Err(format!("invalid argument for {name} (type {})", ts(&args[0]))),
        }),
        "max" | "min" | "mean" | "median" => validate_aggregate(name, args),
        "now" => {
            if args.is_empty() || args.len() == 1 && assignable_to_location(&args[0]) {
                Ok(Type::time())
            } else {
                Err(format!("invalid number of arguments (expected 0, got {})", args.len()))
            }
        }
        "date" => {
            if args.is_empty() {
                Err(format!("invalid number of arguments (expected at least 1, got {})", args.len()))
            } else {
                let rest = if assignable_to_location(&args[0]) { &args[1..] } else { args };
                if rest.len() > 3 {
                    Err(format!("invalid number of arguments (expected at most 3, got {})", rest.len()))
                } else {
                    Ok(Type::time())
                }
            }
        }
        "first" | "last" => expect_one(args).and_then(|()| match kind(&args[0]) {
            Kind::Interface => Ok(any),
            Kind::Slice | Kind::Array => Ok(args[0].as_ref().and_then(Type::elem).cloned().unwrap_or(Type::Any)),
            _ => Err(format!(
                "cannot get {} element from {}",
                if name == "first" { "first" } else { "last" },
                ts(&args[0])
            )),
        }),
        "take" => {
            if args.len() != 2 {
                Err(format!("invalid number of arguments (expected 2, got {})", args.len()))
            } else if !matches!(kind(&args[0]), Kind::Interface | Kind::Slice | Kind::Array) {
                Err(format!("cannot take from {}", ts(&args[0])))
            } else if !matches!(
                kind(&args[1]),
                Kind::Interface | Kind::Int | Kind::Int8 | Kind::Int16 | Kind::Int32 | Kind::Int64
            ) {
                Err(format!("cannot take {} elements", ts(&args[1])))
            } else {
                Ok(args[0].clone().unwrap_or(Type::Any))
            }
        }
        "keys" | "values" => expect_one(args).and_then(|()| match kind(&args[0]) {
            Kind::Interface | Kind::Map => Ok(Type::any_slice()),
            _ => Err(format!("cannot get {name} from {}", ts(&args[0]))),
        }),
        "toPairs" => expect_one(args).and_then(|()| match kind(&args[0]) {
            Kind::Interface | Kind::Map => Ok(Type::any_slice()),
            _ => Err(format!("cannot transform {} to pairs", ts(&args[0]))),
        }),
        "fromPairs" => expect_one(args).and_then(|()| match kind(&args[0]) {
            Kind::Interface | Kind::Slice | Kind::Array => Ok(Type::any_map()),
            _ => Err(format!("cannot transform {} from pairs", ts(&args[0]))),
        }),
        "reverse" | "uniq" => expect_one(args).and_then(|()| match kind(&args[0]) {
            Kind::Interface | Kind::Slice | Kind::Array => Ok(Type::any_slice()),
            _ => Err(format!("cannot {name} {}", ts(&args[0]))),
        }),
        "concat" => {
            if args.is_empty() {
                Err("invalid number of arguments (expected at least 1, got 0)".to_string())
            } else {
                match args.iter().find(|a| !matches!(kind(a), Kind::Interface | Kind::Slice | Kind::Array)) {
                    Some(bad) => Err(format!("cannot concat {}", ts(bad))),
                    None => Ok(Type::any_slice()),
                }
            }
        }
        "flatten" => expect_one(args).and_then(|()| match kind(&args[0]) {
            Kind::Interface | Kind::Slice | Kind::Array => Ok(Type::any_slice()),
            _ => Err(format!("cannot flatten {}", ts(&args[0]))),
        }),
        _ => return None,
    })
}

/// Whether the compiler dereferences argument `i` of type `arg` (only `now` and `date` decide this
/// themselves; `None` means the default rule applies).
pub fn deref_arg(name: &str, arg: &Type) -> Option<bool> {
    match name {
        "now" => Some(false),
        "date" => Some(!arg.assignable_to(&Type::location_ptr())),
        _ => None,
    }
}
