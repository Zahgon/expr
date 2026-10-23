//! Runtime operations on values: member access, slicing, `in`, comparison and arithmetic.
//!
//! Operands are dynamically typed like the original's `any` values. Mixed integer types meet as `int`, any
//! float operand turns the operation into `float64`, and unsupported combinations fail with the original's
//! message.

use std::rc::Rc;

use crate::types::{Struct, Type};
use crate::value::{self, deep_equal, sprint_v_lossy, FuncValue, MapValue, Seq, Value};

/// A runtime failure: the text the original's panic prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panic(pub Vec<u8>);

impl From<String> for Panic {
    fn from(s: String) -> Panic {
        Panic(s.into_bytes())
    }
}

impl From<&str> for Panic {
    fn from(s: &str) -> Panic {
        Panic(s.as_bytes().to_vec())
    }
}

pub type RtResult<T> = Result<T, Panic>;

/// The failure of a type assertion `v.(T)` on an `interface {}` value.
pub fn assertion(v: &Value, want: &str) -> Panic {
    match v {
        Value::Nil => format!("interface conversion: interface {{}} is nil, not {want}").into(),
        other => format!("interface conversion: interface {{}} is {}, not {want}", other.type_name()).into(),
    }
}

pub fn as_bool(v: &Value) -> RtResult<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        other => Err(assertion(other, "bool")),
    }
}

pub fn as_int(v: &Value) -> RtResult<i64> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(assertion(other, "int")),
    }
}

pub fn as_str(v: &Value) -> RtResult<&[u8]> {
    match v {
        Value::String(s) => Ok(s),
        other => Err(assertion(other, "string")),
    }
}

/// The failure of a `reflect.Value` method called on a value of the wrong kind.
pub fn reflect_kind_error(method: &str, v: &Value) -> Panic {
    match v {
        Value::Nil => format!("reflect: call of reflect.Value.{method} on zero Value").into(),
        other => format!("reflect: call of reflect.Value.{method} on {} Value", other.kind()).into(),
    }
}

/// Dereferences pointers (`*time.Location`) the way `deref.Interface` does.
pub fn deref(v: Value) -> Value {
    match v {
        Value::Location(l) => Value::LocationValue(l),
        other => other,
    }
}

/// Integer operands as Go's `int` (with wrapping conversion), `None` for non-integers.
fn as_go_int(v: &Value) -> Option<i64> {
    match v {
        Value::Int(_)
        | Value::Int8(_)
        | Value::Int16(_)
        | Value::Int32(_)
        | Value::Int64(_)
        | Value::Uint(_)
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Uint32(_)
        | Value::Uint64(_) => v.as_i64(),
        _ => None,
    }
}

fn as_go_float(v: &Value) -> Option<f64> {
    match v {
        Value::Float32(f) => Some(f64::from(*f)),
        Value::Float64(f) => Some(*f),
        _ => as_go_int(v).map(|i| match v {
            Value::Uint(u) | Value::Uint64(u) => *u as f64,
            _ => i as f64,
        }),
    }
}

/// Two numeric operands: both integers, or at least one float (both as `float64`).
enum Numbers {
    Ints(i64, i64),
    Floats(f64, f64),
}

fn numbers(a: &Value, b: &Value) -> Option<Numbers> {
    if !a.is_builtin_number() || !b.is_builtin_number() {
        return None;
    }
    if matches!(a, Value::Float32(_) | Value::Float64(_)) || matches!(b, Value::Float32(_) | Value::Float64(_)) {
        return Some(Numbers::Floats(as_go_float(a)?, as_go_float(b)?));
    }
    Some(Numbers::Ints(as_go_int(a)?, as_go_int(b)?))
}

fn invalid(a: &Value, op: &str, b: &Value) -> Panic {
    format!("invalid operation: {} {op} {}", a.type_name(), b.type_name()).into()
}

/// Whether a value is nil: a nil interface, or a nil slice.
pub fn is_nil(v: &Value) -> bool {
    match v {
        Value::Nil => true,
        Value::Slice(s) => s.nil,
        _ => false,
    }
}

/// The original's `Equal`: numbers across types, arrays element-wise, then deep equality.
pub fn equal(a: &Value, b: &Value) -> bool {
    if let Some(n) = numbers(a, b) {
        return match n {
            Numbers::Ints(x, y) => x == y,
            Numbers::Floats(x, y) => x == y,
        };
    }
    if a.is_builtin_number() {
        return false;
    }
    if let (Value::Slice(x), Value::Slice(y)) = (a, b) {
        if let Some(eq) = slice_equal(x, y) {
            return eq;
        }
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => return x == y,
        (Value::Time(x), Value::Time(y)) => return x.equal(y),
        (Value::Duration(x), Value::Duration(y)) => return x == y,
        (Value::Bool(x), Value::Bool(y)) => return x == y,
        (Value::String(_) | Value::Time(_) | Value::Duration(_) | Value::Bool(_), _) => {}
        _ => {}
    }
    if is_nil(a) && is_nil(b) {
        return true;
    }
    deep_equal(a, b)
}

/// The generated array cases of `Equal`: `[]interface {}` against the basic slice types, and same-typed basic
/// slices. `None` falls back to the generic comparison.
fn slice_equal(x: &Seq, y: &Seq) -> Option<bool> {
    let basic = |t: &Type| matches!(t.elem(), Some(e) if *e == Type::String || e.is_builtin_integer() && *e != Type::Uintptr || e.is_builtin_float());
    let x_any = x.ty == Type::any_slice();
    let y_any = y.ty == Type::any_slice();
    let elementwise = || x.items.len() == y.items.len() && x.items.iter().zip(&y.items).all(|(p, q)| equal(p, q));
    if x_any && (y_any || basic(&y.ty)) || y_any && basic(&x.ty) {
        return Some(elementwise());
    }
    if basic(&x.ty) && x.ty == y.ty {
        return Some(elementwise());
    }
    None
}

fn compare(a: &Value, b: &Value, op: &str) -> RtResult<bool> {
    use std::cmp::Ordering::*;
    let ord = match (numbers(a, b), a, b) {
        (Some(Numbers::Ints(x, y)), _, _) => Some(x.cmp(&y)),
        (Some(Numbers::Floats(x, y)), _, _) => x.partial_cmp(&y),
        (None, Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (None, Value::Time(x), Value::Time(y)) => Some(x.compare(y).cmp(&0)),
        (None, Value::Duration(x), Value::Duration(y)) => Some(x.cmp(y)),
        _ => return Err(invalid(a, op, b)),
    };
    Ok(match (op, ord) {
        (_, None) => false,
        ("<", Some(o)) => o == Less,
        (">", Some(o)) => o == Greater,
        ("<=", Some(o)) => o != Greater,
        (_, Some(o)) => o != Less,
    })
}

pub fn less(a: &Value, b: &Value) -> RtResult<bool> {
    compare(a, b, "<")
}

pub fn more(a: &Value, b: &Value) -> RtResult<bool> {
    compare(a, b, ">")
}

pub fn less_or_equal(a: &Value, b: &Value) -> RtResult<bool> {
    compare(a, b, "<=")
}

pub fn more_or_equal(a: &Value, b: &Value) -> RtResult<bool> {
    compare(a, b, ">=")
}

pub fn add(a: &Value, b: &Value) -> RtResult<Value> {
    match numbers(a, b) {
        Some(Numbers::Ints(x, y)) => return Ok(Value::Int(x.wrapping_add(y))),
        Some(Numbers::Floats(x, y)) => return Ok(Value::Float64(x + y)),
        None => {}
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => {
            let mut s = x.to_vec();
            s.extend_from_slice(y);
            Ok(Value::string(s))
        }
        (Value::Time(x), Value::Duration(y)) => Ok(Value::Time(x.add(*y))),
        (Value::Duration(x), Value::Time(y)) => Ok(Value::Time(y.add(*x))),
        (Value::Duration(x), Value::Duration(y)) => Ok(Value::Duration(x.wrapping_add(*y))),
        _ => Err(invalid(a, "+", b)),
    }
}

pub fn subtract(a: &Value, b: &Value) -> RtResult<Value> {
    match numbers(a, b) {
        Some(Numbers::Ints(x, y)) => return Ok(Value::Int(x.wrapping_sub(y))),
        Some(Numbers::Floats(x, y)) => return Ok(Value::Float64(x - y)),
        None => {}
    }
    match (a, b) {
        (Value::Time(x), Value::Time(y)) => Ok(Value::Duration(x.sub(y))),
        (Value::Time(x), Value::Duration(y)) => Ok(Value::Time(x.add(y.wrapping_neg()))),
        (Value::Duration(x), Value::Duration(y)) => Ok(Value::Duration(x.wrapping_sub(*y))),
        _ => Err(invalid(a, "-", b)),
    }
}

pub fn multiply(a: &Value, b: &Value) -> RtResult<Value> {
    let is_float = |v: &Value| matches!(v, Value::Float32(_) | Value::Float64(_));
    let a_ok = a.is_builtin_number() || matches!(a, Value::Duration(_));
    let b_ok = b.is_builtin_number() || matches!(b, Value::Duration(_));
    if !a_ok || !b_ok {
        return Err(invalid(a, "*", b));
    }
    if is_float(a) || is_float(b) {
        let f = |v: &Value| as_go_float(v).or_else(|| v.as_i64().map(|i| i as f64)).unwrap_or_default();
        return Ok(Value::Float64(f(a) * f(b)));
    }
    let x = a.as_i64().unwrap_or_default();
    let y = b.as_i64().unwrap_or_default();
    if matches!(a, Value::Duration(_)) || matches!(b, Value::Duration(_)) {
        return Ok(Value::Duration(x.wrapping_mul(y)));
    }
    let x = as_go_int(a).unwrap_or(x);
    let y = as_go_int(b).unwrap_or(y);
    Ok(Value::Int(x.wrapping_mul(y)))
}

pub fn divide(a: &Value, b: &Value) -> RtResult<Value> {
    match numbers(a, b) {
        Some(Numbers::Ints(x, y)) => {
            let fx = as_go_float(a).unwrap_or(x as f64);
            let fy = as_go_float(b).unwrap_or(y as f64);
            Ok(Value::Float64(fx / fy))
        }
        Some(Numbers::Floats(x, y)) => Ok(Value::Float64(x / y)),
        None => Err(invalid(a, "/", b)),
    }
}

pub fn modulo(a: &Value, b: &Value) -> RtResult<Value> {
    match (as_go_int(a), as_go_int(b)) {
        (Some(_), Some(0)) => Err("runtime error: integer divide by zero".into()),
        (Some(x), Some(y)) => Ok(Value::Int(x.wrapping_rem(y))),
        _ => Err(invalid(a, "%", b)),
    }
}

pub fn exponent(a: &Value, b: &Value) -> RtResult<Value> {
    Ok(Value::Float64(crate::gostd::math::pow(to_float64(a)?, to_float64(b)?)))
}

pub fn negate(v: &Value) -> RtResult<Value> {
    Ok(match v {
        Value::Float32(f) => Value::Float32(-f),
        Value::Float64(f) => Value::Float64(-f),
        Value::Int(i) => Value::Int(i.wrapping_neg()),
        Value::Int8(i) => Value::Int8(i.wrapping_neg()),
        Value::Int16(i) => Value::Int16(i.wrapping_neg()),
        Value::Int32(i) => Value::Int32(i.wrapping_neg()),
        Value::Int64(i) => Value::Int64(i.wrapping_neg()),
        Value::Uint(u) => Value::Uint(u.wrapping_neg()),
        Value::Uint8(u) => Value::Uint8(u.wrapping_neg()),
        Value::Uint16(u) => Value::Uint16(u.wrapping_neg()),
        Value::Uint32(u) => Value::Uint32(u.wrapping_neg()),
        Value::Uint64(u) => Value::Uint64(u.wrapping_neg()),
        other => return Err(format!("invalid operation: - {}", other.type_name()).into()),
    })
}

/// Go's conversion of a float to an integer, saturating like the original platform.
fn float_to_int(f: f64) -> i64 {
    f as i64
}

/// The original's `ToInt`.
pub fn to_int(v: &Value) -> RtResult<i64> {
    match v {
        Value::Float32(f) => Ok(float_to_int(f64::from(*f))),
        Value::Float64(f) => Ok(float_to_int(*f)),
        other => as_go_int(other).ok_or_else(|| format!("invalid operation: int({})", other.type_name()).into()),
    }
}

pub fn to_int64(v: &Value) -> RtResult<i64> {
    match v {
        Value::Float32(f) => Ok(float_to_int(f64::from(*f))),
        Value::Float64(f) => Ok(float_to_int(*f)),
        other => as_go_int(other).ok_or_else(|| format!("invalid operation: int64({})", other.type_name()).into()),
    }
}

pub fn to_float64(v: &Value) -> RtResult<f64> {
    as_go_float(v).ok_or_else(|| format!("invalid operation: float({})", v.type_name()).into())
}

pub fn to_bool(v: &Value) -> RtResult<bool> {
    match v {
        Value::Nil => Ok(false),
        Value::Bool(b) => Ok(*b),
        other => Err(format!("invalid operation: bool({})", other.type_name()).into()),
    }
}

/// The fields of the `time` structs; reflection finds them but cannot read them.
fn unexported_field(v: &Value, name: &[u8]) -> bool {
    match v {
        Value::Time(_) => matches!(name, b"wall" | b"ext" | b"loc"),
        Value::LocationValue(_) => {
            matches!(name, b"name" | b"zone" | b"tx" | b"extend" | b"cacheStart" | b"cacheEnd" | b"cacheZone")
        }
        _ => false,
    }
}

/// Looks a key up in a map the way `reflect.Value.MapIndex` does.
pub fn map_index<'m>(m: &'m MapValue, key: &Value) -> RtResult<Option<&'m Value>> {
    let key_type = m.ty.key().cloned().unwrap_or(Type::Any);
    let key = match key {
        Value::Nil => zero_value(&key_type),
        other => {
            let t = other.type_of().unwrap_or(Type::Any);
            if !t.assignable_to(&key_type) {
                return Err(
                    format!("reflect.Value.MapIndex: value of type {t} is not assignable to type {key_type}").into()
                );
            }
            other.clone()
        }
    };
    m.get(&key).map_err(|value::Unhashable(t)| format!("runtime error: hash of unhashable type {t}").into())
}

/// The zero value of a type (`reflect.Zero`).
pub fn zero_value(t: &Type) -> Value {
    match t {
        Type::Bool => Value::Bool(false),
        Type::Int => Value::Int(0),
        Type::Int8 => Value::Int8(0),
        Type::Int16 => Value::Int16(0),
        Type::Int32 => Value::Int32(0),
        Type::Int64 => Value::Int64(0),
        Type::Uint | Type::Uintptr => Value::Uint(0),
        Type::Uint8 => Value::Uint8(0),
        Type::Uint16 => Value::Uint16(0),
        Type::Uint32 => Value::Uint32(0),
        Type::Uint64 => Value::Uint64(0),
        Type::Float32 => Value::Float32(0.0),
        Type::Float64 => Value::Float64(0.0),
        Type::String => Value::string(""),
        Type::Slice(e) => Value::Slice(Rc::new(Seq { ty: Type::Slice(e.clone()), items: Vec::new(), nil: true })),
        Type::Array(n, e) => {
            Value::Array(Rc::new(Seq { ty: t.clone(), items: (0..*n).map(|_| zero_value(e)).collect(), nil: false }))
        }
        Type::Struct(Struct::Time) => Value::Time(Default::default()),
        Type::Struct(Struct::Empty) => Value::EmptyStruct,
        Type::Named(crate::types::Named::Duration) => Value::Duration(0),
        Type::Named(crate::types::Named::Month) => Value::Month(0),
        Type::Named(crate::types::Named::Weekday) => Value::Weekday(0),
        _ => Value::Nil,
    }
}

/// A method of a value with methods, bound to the value (`reflect.Value.MethodByName`).
pub fn bound_method(v: &Value, name: &str) -> Option<Value> {
    let t = v.type_of()?;
    let (_, m) = t.method_by_name(name)?;
    Some(Value::Func(Rc::new(FuncValue { ty: Type::bound_method_type(m), receiver: v.clone(), method: m.name })))
}

/// The original's `Fetch`: `from[i]` for arrays, strings, maps, struct fields and methods.
pub fn fetch(from: &Value, i: &Value) -> RtResult<Value> {
    let cannot = || -> Panic { format!("cannot fetch {} from {}", sprint_v_lossy(i), from.type_name()).into() };
    if from.is_nil() {
        return Err(cannot());
    }
    if let Value::String(name) = i {
        if let Some(m) = std::str::from_utf8(name).ok().and_then(|n| bound_method(from, n)) {
            return Ok(m);
        }
    }
    let target = deref(from.clone());
    match &target {
        Value::Slice(_) | Value::Array(_) | Value::String(_) => {
            let mut index = to_int(i)?;
            let len = target.len().unwrap_or_default() as i64;
            if index < 0 {
                index += len;
            }
            if index < 0 || index >= len {
                return Err(format!("index out of range: {index} (array length is {len})").into());
            }
            Ok(match &target {
                Value::String(s) => Value::Uint8(s[index as usize]),
                other => other.items().map(|items| items[index as usize].clone()).unwrap_or(Value::Nil),
            })
        }
        Value::Map(m) => match map_index(m, i)? {
            Some(v) => Ok(v.clone()),
            None => Ok(zero_value(m.ty.elem().unwrap_or(&Type::Any))),
        },
        Value::Time(_) | Value::LocationValue(_) | Value::EmptyStruct => {
            let name = as_str(i)?;
            if unexported_field(&target, name) {
                return Err(
                    "reflect.Value.Interface: cannot return value obtained from unexported field or method".into()
                );
            }
            Err(cannot())
        }
        _ => Err(cannot()),
    }
}

/// The original's `FetchMethod`: the method with the given index of the value's type.
pub fn fetch_method(from: &Value, name: &'static str) -> RtResult<Value> {
    if from.is_nil() {
        return Err(format!("cannot fetch {name} from {}", from.type_name()).into());
    }
    bound_method(from, name).ok_or_else(|| format!("cannot fetch {name} from {}", from.type_name()).into())
}

/// The original's `Slice`: `array[from:to]` with negative indices counted from the end and clamping.
pub fn slice(array: &Value, from: &Value, to: &Value) -> RtResult<Value> {
    let length = match array {
        Value::Slice(_) | Value::Array(_) | Value::String(_) => array.len().unwrap_or_default() as i64,
        _ => return Err(format!("cannot slice {}", sprint_v_lossy(from)).into()),
    };
    let mut a = to_int(from)?;
    let mut b = to_int(to)?;
    if a < 0 {
        a += length;
    }
    if a < 0 {
        a = 0;
    }
    if b < 0 {
        b += length;
    }
    if b < 0 {
        b = 0;
    }
    if b > length {
        b = length;
    }
    if a > b {
        a = b;
    }
    let (a, b) = (a as usize, b as usize);
    Ok(match array {
        Value::String(s) => Value::string(&s[a..b]),
        Value::Slice(seq) => {
            Value::Slice(Rc::new(Seq { ty: seq.ty.clone(), items: seq.items[a..b].to_vec(), nil: seq.nil && a == b }))
        }
        Value::Array(seq) => Value::Slice(Rc::new(Seq {
            ty: Type::slice(seq.ty.elem().cloned().unwrap_or(Type::Any)),
            items: seq.items[a..b].to_vec(),
            nil: false,
        })),
        _ => Value::Nil,
    })
}

/// The original's `In`: membership in arrays, map keys, and struct fields.
pub fn contains_in(needle: &Value, array: &Value) -> RtResult<bool> {
    match array {
        Value::Nil => Ok(false),
        Value::Slice(_) | Value::Array(_) => Ok(array.items().unwrap_or(&[]).iter().any(|v| equal(v, needle))),
        Value::Map(m) => Ok(map_index(m, needle)?.is_some()),
        Value::Time(_) | Value::LocationValue(_) | Value::EmptyStruct => match needle {
            Value::String(_) => Ok(false),
            other => Err(format!("cannot use {} as field name of {}", other.type_name(), array.type_name()).into()),
        },
        Value::Location(_) => contains_in(needle, &deref(array.clone())),
        other => Err(format!("operator \"in\" not defined on {}", other.type_name()).into()),
    }
}

/// The original's `Len` (bytes for strings).
pub fn len(v: &Value) -> RtResult<i64> {
    v.len().map(|l| l as i64).ok_or_else(|| format!("invalid argument for len (type {})", v.type_name()).into())
}

/// The original's `MakeRange`.
pub fn make_range(min: i64, max: i64) -> Value {
    let size = max.wrapping_sub(min).wrapping_add(1);
    let items = if size <= 0 { Vec::new() } else { (0..size).map(|i| Value::Int(min.wrapping_add(i))).collect() };
    Value::slice(Type::Int, items)
}
