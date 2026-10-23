//! The builtin functions at run time.
//!
//! Arguments arrive as dynamically typed values. The checker rejects most wrong calls, but values of unknown
//! type reach these functions unchecked, so each one fails the way the original does (type assertions, index
//! errors and its own messages).

use std::rc::Rc;

use crate::builtin::json;
use crate::gostd::{base64, sort, strconv, strings, time, utf8};
use crate::types::{Kind, Type};
use crate::value::{self, sprint_v, sprintf_lossy, MapValue, Seq, Value};
use crate::vm::runtime::{self, as_str, Panic, RtResult};

/// The deepest nesting `flatten`, `min`, `max`, `mean` and `median` follow.
const MAX_DEPTH: usize = 10_000;

fn arg(args: &[Value], i: usize) -> RtResult<&Value> {
    args.get(i).ok_or_else(|| format!("runtime error: index out of range [{i}] with length {}", args.len()).into())
}

fn str_arg(args: &[Value], i: usize) -> RtResult<&[u8]> {
    as_str(arg(args, i)?)
}

fn strings_value(parts: Vec<&[u8]>) -> Value {
    Value::slice(Type::String, parts.into_iter().map(Value::string).collect())
}

/// `%s` of a `reflect.Kind`, or of a `reflect.Type` of a possibly nil value.
fn kind_name(v: &Value) -> String {
    v.kind().name().to_string()
}

fn type_or_nil(v: &Value) -> String {
    v.type_of().map_or_else(|| "%!s(<nil>)".to_string(), |t| t.to_string())
}

fn expect_args(args: &[Value], n: usize) -> RtResult<()> {
    if args.len() != n {
        return Err(format!("invalid number of arguments (expected {n}, got {})", args.len()).into());
    }
    Ok(())
}

/// The elements of a slice or array, or `None` for other kinds.
fn elements(v: &Value) -> Option<&[Value]> {
    v.items()
}

/// Calls a builtin compiled as a plain function.
pub fn call(name: &str, args: &[Value]) -> RtResult<Value> {
    match name {
        "trim" => match args.len() {
            1 => Ok(Value::string(strings::trim_space(str_arg(args, 0)?))),
            2 => {
                let s = str_arg(args, 0)?;
                Ok(Value::string(strings::trim(s, str_arg(args, 1)?)))
            }
            n => Err(format!("invalid number of arguments for trim (expected 1 or 2, got {n})").into()),
        },
        "trimPrefix" | "trimSuffix" => {
            let cut: &[u8] = if args.len() == 2 { str_arg(args, 1)? } else { b" " };
            let s = str_arg(args, 0)?;
            Ok(Value::string(if name == "trimPrefix" {
                strings::trim_prefix(s, cut)
            } else {
                strings::trim_suffix(s, cut)
            }))
        }
        "split" | "splitAfter" => {
            let after = name == "splitAfter";
            match args.len() {
                2 => {
                    let (s, sep) = (str_arg(args, 0)?, str_arg(args, 1)?);
                    Ok(strings_value(if after { strings::split_after(s, sep) } else { strings::split(s, sep) }))
                }
                3 => {
                    let (s, sep) = (str_arg(args, 0)?, str_arg(args, 1)?);
                    let n = runtime::to_int(arg(args, 2)?)?;
                    let parts = if after { strings::split_after_n(s, sep, n) } else { strings::split_n(s, sep, n) };
                    if n == 0 {
                        return Ok(Value::nil_slice(Type::String));
                    }
                    Ok(strings_value(parts))
                }
                n => Err(format!("invalid number of arguments for {name} (expected 2 or 3, got {n})").into()),
            }
        }
        "replace" => match args.len() {
            4 => {
                let (s, old, new) = (str_arg(args, 0)?, str_arg(args, 1)?, str_arg(args, 2)?);
                let n = runtime::to_int(arg(args, 3)?)?;
                Ok(Value::string(strings::replace(s, old, new, n)))
            }
            3 => Ok(Value::string(strings::replace_all(str_arg(args, 0)?, str_arg(args, 1)?, str_arg(args, 2)?))),
            n => Err(format!("invalid number of arguments for replace (expected 3 or 4, got {n})").into()),
        },
        "join" => {
            let glue: &[u8] = if args.len() == 2 { str_arg(args, 1)? } else { b"" };
            let first = arg(args, 0)?;
            match first {
                Value::Slice(seq) if seq.ty == Type::slice(Type::String) || seq.ty == Type::any_slice() => {
                    let mut out = Vec::new();
                    for (i, item) in seq.items.iter().enumerate() {
                        let s = as_str(item)?;
                        if i > 0 {
                            out.extend_from_slice(glue);
                        }
                        out.extend_from_slice(s);
                    }
                    Ok(Value::string(out))
                }
                other => Err(format!("invalid argument for join (type {})", type_or_nil(other)).into()),
            }
        }
        "indexOf" => {
            let s = str_arg(args, 0)?;
            Ok(Value::Int(strings::index(s, str_arg(args, 1)?)))
        }
        "lastIndexOf" => {
            let s = str_arg(args, 0)?;
            Ok(Value::Int(strings::last_index(s, str_arg(args, 1)?)))
        }
        "hasPrefix" => {
            let s = str_arg(args, 0)?;
            Ok(Value::Bool(strings::has_prefix(s, str_arg(args, 1)?)))
        }
        "hasSuffix" => {
            let s = str_arg(args, 0)?;
            Ok(Value::Bool(strings::has_suffix(s, str_arg(args, 1)?)))
        }
        "max" => min_max("max", true, 0, args).map(|v| v.unwrap_or(Value::Nil)),
        "min" => min_max("min", false, 0, args).map(|v| v.unwrap_or(Value::Nil)),
        "mean" => {
            let (count, sum) = mean(0, args)?;
            Ok(Value::Float64(if count == 0 { 0.0 } else { sum / count as f64 }))
        }
        "median" => {
            let mut values = Vec::new();
            median(0, args, &mut values)?;
            let n = values.len();
            if n == 0 {
                return Ok(Value::Float64(0.0));
            }
            sort::sort_floats(&mut values);
            Ok(Value::Float64(if n % 2 == 1 { values[n / 2] } else { (values[n / 2 - 1] + values[n / 2]) / 2.0 }))
        }
        "toJSON" => {
            let compact = json::marshal(arg(args, 0)?).map_err(Panic::from)?;
            Ok(Value::string(crate::gostd::json::indent(&compact, b"", b"  ")))
        }
        "fromJSON" => json::unmarshal(str_arg(args, 0)?).map_err(Panic::from),
        "toBase64" => Ok(Value::string(base64::std_encode(str_arg(args, 0)?))),
        "fromBase64" => base64::std_decode(str_arg(args, 0)?).map(Value::string).map_err(Panic::from),
        "now" => match args {
            [] => Ok(Value::Time(time::now())),
            [Value::Location(l)] => Ok(Value::Time(time::now().in_location(l))),
            _ => Err(format!("invalid number of arguments (expected 0, got {})", args.len()).into()),
        },
        "duration" => time::parse_duration(str_arg(args, 0)?).map(Value::Duration).map_err(Panic::from),
        "date" => date(args),
        "timezone" => time::load_location(str_arg(args, 0)?).map(Value::Location).map_err(Panic::from),
        "first" => Ok(runtime::fetch(arg(args, 0).unwrap_or(&Value::Nil), &Value::Int(0)).unwrap_or(Value::Nil)),
        "last" => Ok(runtime::fetch(arg(args, 0).unwrap_or(&Value::Nil), &Value::Int(-1)).unwrap_or(Value::Nil)),
        "get" => get(args),
        "take" => take(args),
        "keys" | "values" => {
            expect_args(args, 1)?;
            match &args[0] {
                Value::Map(m) => Ok(Value::any_slice(
                    m.entries().iter().map(|(k, v)| if name == "keys" { k.clone() } else { v.clone() }).collect(),
                )),
                other => Err(format!("cannot get {name} from {}", kind_name(other)).into()),
            }
        }
        "toPairs" => {
            expect_args(args, 1)?;
            match &args[0] {
                Value::Map(m) => Ok(Value::slice(
                    Type::array(2, Type::Any),
                    m.entries().iter().map(|(k, v)| Value::array(Type::Any, vec![k.clone(), v.clone()])).collect(),
                )),
                other => Err(format!("cannot transform {} to pairs", kind_name(other)).into()),
            }
        }
        "fromPairs" => from_pairs(args),
        "uniq" => {
            expect_args(args, 1)?;
            let items =
                elements(&args[0]).ok_or_else(|| Panic::from(format!("cannot uniq {}", kind_name(&args[0]))))?;
            let mut out: Vec<Value> = Vec::new();
            for item in items {
                if !out.iter().any(|r| runtime::equal(item, r)) {
                    out.push(item.clone());
                }
            }
            Ok(Value::any_slice(out))
        }
        "bitand" | "bitor" | "bitxor" | "bitnand" | "bitshl" | "bitshr" | "bitushr" => bit_func(name, args),
        "bitnot" => {
            if args.len() != 1 {
                return Err(format!("invalid number of arguments for bitnot (expected 1, got {})", args.len()).into());
            }
            let x = to_int_strict(&args[0]).map_err(|e| Panic::from(format!("{e} to call bitnot")))?;
            Ok(Value::Int(!x))
        }
        other => Err(format!("unknown builtin {other}").into()),
    }
}

/// Calls a builtin compiled as a "safe" function; also returns the memory it allocated.
pub fn call_safe(name: &str, args: &[Value]) -> RtResult<(Value, u64)> {
    match name {
        "repeat" => {
            let s = str_arg(args, 0)?;
            let n = runtime::to_int(arg(args, 1)?)?;
            if n < 0 {
                return Err(format!("invalid argument for repeat (expected positive integer, got {n})").into());
            }
            if n > 1_000_000 {
                return Err("memory budget exceeded".into());
            }
            Ok((Value::string(strings::repeat(s, n as usize)), (s.len() as u64).saturating_mul(n as u64)))
        }
        "reverse" => {
            expect_args(args, 1)?;
            let items =
                elements(&args[0]).ok_or_else(|| Panic::from(format!("cannot reverse {}", kind_name(&args[0]))))?;
            let out: Vec<Value> = items.iter().rev().cloned().collect();
            let n = out.len() as u64;
            Ok((Value::any_slice(out), n))
        }
        "concat" => {
            if args.is_empty() {
                return Err("invalid number of arguments (expected at least 1, got 0)".into());
            }
            let mut out = Vec::new();
            for a in args {
                let items = elements(a).ok_or_else(|| Panic::from(format!("cannot concat {}", kind_name(a))))?;
                out.extend(items.iter().cloned());
            }
            let n = out.len() as u64;
            Ok((if out.is_empty() { Value::nil_slice(Type::Any) } else { Value::any_slice(out) }, n))
        }
        "flatten" => {
            expect_args(args, 1)?;
            let items =
                elements(&args[0]).ok_or_else(|| Panic::from(format!("cannot flatten {}", kind_name(&args[0]))))?;
            let mut out = Vec::new();
            flatten(items, 0, &mut out)?;
            let n = out.len() as u64;
            Ok((Value::any_slice(out), n))
        }
        "sort" => {
            if args.is_empty() || args.len() > 2 {
                return Err(format!("invalid number of arguments (expected 1 or 2, got {})", args.len()).into());
            }
            let mut array: Vec<Value> = match &args[0] {
                Value::Slice(seq)
                    if [Type::Any, Type::Int, Type::Float64, Type::String].iter().any(|t| seq.ty.elem() == Some(t)) =>
                {
                    seq.items.clone()
                }
                _ => Vec::new(),
            };
            let mut desc = false;
            if args.len() == 2 {
                match &args[1] {
                    Value::String(s) if &s[..] == b"asc" => {}
                    Value::String(s) if &s[..] == b"desc" => desc = true,
                    Value::String(s) => {
                        return Err(format!("invalid order {}, expected asc or desc", String::from_utf8_lossy(s)).into())
                    }
                    other => {
                        return Err(format!("sort order argument must be a string (got {})", other.type_name()).into())
                    }
                }
            }
            crate::vm::sort_values(&mut array, None, desc)?;
            let n = array.len() as u64;
            let is_nil = !matches!(&args[0], Value::Slice(seq) if [Type::Any, Type::Int, Type::Float64, Type::String].iter().any(|t| seq.ty.elem() == Some(t)));
            Ok((if is_nil { Value::nil_slice(Type::Any) } else { Value::any_slice(array) }, n))
        }
        other => Err(format!("unknown builtin {other}").into()),
    }
}

/// Calls a builtin compiled for one argument without an error result.
pub fn call_fast(name: &str, x: &Value) -> RtResult<Value> {
    match name {
        "len" => match x {
            Value::String(s) => Ok(Value::Int(utf8::rune_count(s) as i64)),
            Value::Slice(_) | Value::Array(_) | Value::Map(_) => Ok(Value::Int(x.len().unwrap_or_default() as i64)),
            other => Err(format!("invalid argument for len (type {})", other.type_name()).into()),
        },
        "type" => Ok(Value::string(type_of(x))),
        "abs" => abs(x),
        "ceil" | "floor" | "round" => match x {
            Value::Float32(_) | Value::Float64(_) => {
                let f = runtime::to_float64(x)?;
                Ok(Value::Float64(match name {
                    "ceil" => f.ceil(),
                    "floor" => f.floor(),
                    _ => f.round(),
                }))
            }
            v if v.is_builtin_number() => Ok(Value::Float64(runtime::to_float64(v)?)),
            other => Err(format!("invalid argument for {name} (type {})", other.type_name()).into()),
        },
        "int" => match x {
            Value::String(s) => strconv::atoi(s)
                .map(Value::Int)
                .map_err(|_| format!("invalid operation: int({})", String::from_utf8_lossy(s)).into()),
            v if v.is_builtin_number() => Ok(Value::Int(runtime::to_int(v)?)),
            Value::Duration(d) | Value::Month(d) | Value::Weekday(d) => Ok(Value::Int(*d)),
            Value::Nil => Err("reflect: call of reflect.Value.Type on zero Value".into()),
            other => Err(format!("invalid operation: int({})", other.type_name()).into()),
        },
        "float" => match x {
            Value::String(s) => strconv::parse_float(s, 64)
                .map(Value::Float64)
                .map_err(|_| format!("invalid operation: float({})", String::from_utf8_lossy(s)).into()),
            v if v.is_builtin_number() => Ok(Value::Float64(runtime::to_float64(v)?)),
            other => Err(format!("invalid operation: float({})", other.type_name()).into()),
        },
        "string" => Ok(Value::string(sprint_v(x))),
        "upper" => Ok(Value::string(strings::to_upper(as_str(x)?))),
        "lower" => Ok(Value::string(strings::to_lower(as_str(x)?))),
        other => Err(format!("unknown builtin {other}").into()),
    }
}

/// The original's `type()` names.
fn type_of(x: &Value) -> String {
    let Some(t) = x.type_of() else { return "nil".to_string() };
    if !t.name().is_empty() && !t.pkg_path().is_empty() {
        return format!("{}.{}", t.pkg_path(), t.name());
    }
    match t.kind() {
        Kind::Bool => "bool",
        Kind::Int | Kind::Int8 | Kind::Int16 | Kind::Int32 | Kind::Int64 => "int",
        Kind::Uint | Kind::Uint8 | Kind::Uint16 | Kind::Uint32 | Kind::Uint64 => "uint",
        Kind::Float32 | Kind::Float64 => "float",
        Kind::String => "string",
        Kind::Array | Kind::Slice => "array",
        Kind::Map => "map",
        Kind::Func => "func",
        Kind::Struct => "struct",
        _ => "unknown",
    }
    .to_string()
}

fn abs(x: &Value) -> RtResult<Value> {
    Ok(match x {
        Value::Float32(f) => Value::Float32(if *f < 0.0 { -f } else { *f }),
        Value::Float64(f) => Value::Float64(if *f < 0.0 { -f } else { *f }),
        Value::Int(i) => Value::Int(if *i < 0 { i.wrapping_neg() } else { *i }),
        Value::Int8(i) => Value::Int8(if *i < 0 { i.wrapping_neg() } else { *i }),
        Value::Int16(i) => Value::Int16(if *i < 0 { i.wrapping_neg() } else { *i }),
        Value::Int32(i) => Value::Int32(if *i < 0 { i.wrapping_neg() } else { *i }),
        Value::Int64(i) => Value::Int64(if *i < 0 { i.wrapping_neg() } else { *i }),
        Value::Uint(_) | Value::Uint8(_) | Value::Uint16(_) | Value::Uint32(_) | Value::Uint64(_) => x.clone(),
        other => return Err(format!("invalid argument for abs (type {})", other.type_name()).into()),
    })
}

/// The bit functions' integer conversion: integers only, as Go's `int`.
fn to_int_strict(v: &Value) -> Result<i64, String> {
    match v {
        Value::Float32(_) | Value::Float64(_) => Err(format!("cannot use {} as argument (type int)", v.type_name())),
        v if v.is_builtin_number() => Ok(v.as_i64().unwrap_or_default()),
        other => Err(format!("cannot use {} as argument (type int)", other.type_name())),
    }
}

fn bit_func(name: &str, args: &[Value]) -> RtResult<Value> {
    if args.len() != 2 {
        return Err(format!("invalid number of arguments for {name} (expected 2, got {})", args.len()).into());
    }
    let x = to_int_strict(&args[0]).map_err(|e| Panic::from(format!("{e} to call {name}")))?;
    let y = to_int_strict(&args[1]).map_err(|e| Panic::from(format!("{e} to call {name}")))?;
    if matches!(name, "bitshl" | "bitshr" | "bitushr") && y < 0 {
        return Err(format!("invalid operation: negative shift count {y} (type int)").into());
    }
    Ok(Value::Int(match name {
        "bitand" => x & y,
        "bitor" => x | y,
        "bitxor" => x ^ y,
        "bitnand" => x & !y,
        "bitshl" => {
            if y >= 64 {
                0
            } else {
                x << y
            }
        }
        "bitshr" => {
            if y >= 64 {
                if x < 0 {
                    -1
                } else {
                    0
                }
            } else {
                x >> y
            }
        }
        _ => {
            if y >= 64 {
                0
            } else {
                ((x as u64) >> y) as i64
            }
        }
    }))
}

fn date(args: &[Value]) -> RtResult<Value> {
    let (tz, args) = match args.first() {
        Some(Value::Location(l)) => (Some(l.clone()), &args[1..]),
        _ => (None, args),
    };
    let input = str_arg(args, 0)?;
    let result = match args.len() {
        2 => {
            let layout = str_arg(args, 1)?;
            match &tz {
                Some(loc) => time::parse_in_location(layout, input, loc),
                None => time::parse(layout, input),
            }
        }
        3 => {
            let layout = str_arg(args, 1)?;
            let zone = str_arg(args, 2)?;
            let loc = time::load_location(zone).map_err(Panic::from)?;
            time::parse_in_location(layout, input, &loc)
        }
        _ => {
            let layouts: [&[u8]; 7] = [
                b"2006-01-02",
                b"15:04:05",
                b"2006-01-02 15:04:05",
                time::RFC3339,
                time::RFC822,
                time::RFC850,
                time::RFC1123,
            ];
            for layout in layouts {
                let parsed = match &tz {
                    Some(loc) => time::parse_in_location(layout, input, loc),
                    None => time::parse(layout, input),
                };
                if let Ok(t) = parsed {
                    return Ok(Value::Time(t));
                }
            }
            return Err(format!("invalid date {}", String::from_utf8_lossy(input)).into());
        }
    };
    result.map(Value::Time).map_err(Panic::from)
}

fn get(args: &[Value]) -> RtResult<Value> {
    if args.len() < 2 {
        return Err(format!("invalid number of arguments (expected 2, got {})", args.len()).into());
    }
    let (from, i) = (&args[0], &args[1]);
    if from.is_nil() {
        return Ok(Value::Nil);
    }
    if let Value::String(name) = i {
        if let Some(m) = std::str::from_utf8(name).ok().and_then(|n| runtime::bound_method(from, n)) {
            return Ok(m);
        }
    }
    match from {
        Value::Slice(_) | Value::Array(_) | Value::String(_) => {
            let mut index = runtime::to_int(i)?;
            let l = from.len().unwrap_or_default() as i64;
            if index < 0 {
                index += l;
            }
            if 0 <= index && index < l {
                return Ok(match from {
                    Value::String(s) => Value::Uint8(s[index as usize]),
                    other => other.items().map_or(Value::Nil, |items| items[index as usize].clone()),
                });
            }
            Ok(Value::Nil)
        }
        Value::Map(m) => Ok(runtime::map_index(m, i)?.cloned().unwrap_or(Value::Nil)),
        Value::Time(_) | Value::LocationValue(_) | Value::EmptyStruct => {
            as_str(i)?;
            Ok(Value::Nil)
        }
        _ => Ok(Value::Nil),
    }
}

fn take(args: &[Value]) -> RtResult<Value> {
    expect_args(args, 2)?;
    let (v, n) = (&args[0], &args[1]);
    let items = match v {
        Value::Slice(_) | Value::Array(_) => v.items().unwrap_or(&[]),
        other => return Err(format!("cannot take from {}", kind_name(other)).into()),
    };
    let count = match n {
        Value::Int(_) | Value::Int8(_) | Value::Int16(_) | Value::Int32(_) | Value::Int64(_) | Value::Duration(_) => {
            n.as_i64().unwrap_or_default()
        }
        other => return Err(format!("cannot take {} elements", kind_name(other)).into()),
    };
    if let Value::Array(_) = v {
        return Err("reflect.Value.Slice: slice of unaddressable array".into());
    }
    let to = if count > items.len() as i64 { items.len() as i64 } else { count };
    if to < 0 {
        return Err("reflect.Value.Slice: slice index out of bounds".into());
    }
    let Value::Slice(seq) = v else { return Ok(Value::Nil) };
    Ok(Value::Slice(Rc::new(Seq { ty: seq.ty.clone(), items: items[..to as usize].to_vec(), nil: seq.nil && to == 0 })))
}

fn from_pairs(args: &[Value]) -> RtResult<Value> {
    expect_args(args, 1)?;
    let pairs = match &args[0] {
        Value::Slice(_) | Value::Array(_) => args[0].items().unwrap_or(&[]),
        Value::Nil => return Err("cannot transform <invalid reflect.Value> from pairs".into()),
        other => return Err(format!("cannot transform {} from pairs", sprintf_lossy(b's', other)).into()),
    };
    let mut out = MapValue::new(Type::any_map());
    for pair in pairs {
        let items = match pair {
            Value::Slice(_) | Value::Array(_) => pair.items().unwrap_or(&[]),
            other => return Err(format!("invalid pair {}", value::sprint_v_lossy(other)).into()),
        };
        if items.len() != 2 {
            return Err(format!("invalid pair length {}", value::sprint_v_lossy(pair)).into());
        }
        out.insert(items[0].clone(), items[1].clone())
            .map_err(|value::Unhashable(t)| Panic::from(format!("runtime error: hash of unhashable type {t}")))?;
    }
    Ok(Value::Map(Rc::new(out)))
}

fn flatten(items: &[Value], depth: usize, out: &mut Vec<Value>) -> RtResult<()> {
    if depth > MAX_DEPTH {
        return Err("recursion depth exceeded".into());
    }
    for item in items {
        match item {
            Value::Slice(_) | Value::Array(_) => flatten(item.items().unwrap_or(&[]), depth + 1, out)?,
            other => out.push(other.clone()),
        }
    }
    Ok(())
}

fn better(is_max: bool, val: &Value, candidate: &Value) -> RtResult<bool> {
    if is_max {
        runtime::less(val, candidate)
    } else {
        runtime::more(val, candidate)
    }
}

/// The original's `minMax`; `None` is a nil result.
fn min_max(name: &str, is_max: bool, depth: usize, args: &[Value]) -> RtResult<Option<Value>> {
    if depth > MAX_DEPTH {
        return Err("recursion depth exceeded".into());
    }
    let mut val: Option<Value> = None;
    let consider = |val: &mut Option<Value>, e: Value| -> RtResult<()> {
        let replace = match val {
            None => true,
            Some(v) => better(is_max, v, &e)?,
        };
        if replace {
            *val = Some(e);
        }
        Ok(())
    };
    for a in args {
        if let Value::Slice(seq) = a {
            let elem = seq.ty.elem().cloned().unwrap_or(Type::Any);
            if elem == Type::Int || elem == Type::Float64 {
                if seq.items.is_empty() {
                    continue;
                }
                let mut m = seq.items[0].clone();
                for item in &seq.items[1..] {
                    if better(is_max, &m, item)? {
                        m = item.clone();
                    }
                }
                consider(&mut val, m)?;
                continue;
            }
            if elem == Type::Any {
                for e in &seq.items {
                    if e.is_builtin_number() {
                        consider(&mut val, e.clone())?;
                    } else if matches!(e, Value::Slice(_) | Value::Array(_)) {
                        if let Some(nested) = min_max(name, is_max, depth + 1, std::slice::from_ref(e))? {
                            consider(&mut val, nested)?;
                        }
                    } else {
                        return Err(format!("invalid argument for {name} (type {})", e.type_name()).into());
                    }
                }
                continue;
            }
        }
        match a {
            Value::Slice(_) | Value::Array(_) => {
                for item in a.items().unwrap_or(&[]) {
                    let e = min_max(name, is_max, depth + 1, std::slice::from_ref(item))?;
                    match e {
                        Some(e) if e.is_builtin_number() => consider(&mut val, e)?,
                        other => {
                            let t = other.map_or_else(|| "<nil>".to_string(), |v| v.type_name());
                            return Err(format!("invalid argument for {name} (type {t})").into());
                        }
                    }
                }
            }
            v if v.is_builtin_number() => consider(&mut val, v.clone())?,
            other => {
                if args.len() == 1 {
                    return Ok(if other.is_nil() { None } else { Some(other.clone()) });
                }
                return Err(format!("invalid argument for {name} (type {})", other.type_name()).into());
            }
        }
    }
    Ok(val)
}

fn number_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Uint(u) | Value::Uint64(u) => Some(*u as f64),
        v if v.is_builtin_number() => runtime::to_float64(v).ok(),
        _ => None,
    }
}

fn mean(depth: usize, args: &[Value]) -> RtResult<(i64, f64)> {
    if depth > MAX_DEPTH {
        return Err("recursion depth exceeded".into());
    }
    let (mut count, mut total) = (0i64, 0.0f64);
    for a in args {
        match a {
            Value::Slice(_) | Value::Array(_) => {
                for e in a.items().unwrap_or(&[]) {
                    if let Some(f) = number_as_f64(e) {
                        total += f;
                        count += 1;
                    } else if matches!(e, Value::Slice(_) | Value::Array(_)) {
                        let (c, s) = mean(depth + 1, std::slice::from_ref(e))?;
                        total += s;
                        count += c;
                    } else {
                        return Err(format!("invalid argument for mean (type {})", e.type_name()).into());
                    }
                }
            }
            v => match number_as_f64(v) {
                Some(f) => {
                    total += f;
                    count += 1;
                }
                None => return Err(format!("invalid argument for mean (type {})", v.type_name()).into()),
            },
        }
    }
    Ok((count, total))
}

fn median(depth: usize, args: &[Value], values: &mut Vec<f64>) -> RtResult<()> {
    if depth > MAX_DEPTH {
        return Err("recursion depth exceeded".into());
    }
    for a in args {
        match a {
            Value::Slice(_) | Value::Array(_) => {
                for e in a.items().unwrap_or(&[]) {
                    if let Some(f) = number_as_f64(e) {
                        values.push(f);
                    } else if matches!(e, Value::Slice(_) | Value::Array(_)) {
                        median(depth + 1, std::slice::from_ref(e), values)?;
                    } else {
                        return Err(format!("invalid argument for median (type {})", e.type_name()).into());
                    }
                }
            }
            v => match number_as_f64(v) {
                Some(f) => values.push(f),
                None => return Err(format!("invalid argument for median (type {})", v.type_name()).into()),
            },
        }
    }
    Ok(())
}
