//! Runtime values.
//!
//! An expression evaluates to Go values: the original hands `any` values around and inspects them with type
//! switches and `reflect`. [`Value`] keeps the exact Go dynamic type of each value (an `int8` is not an `int`,
//! a `[]int` is not a `[]interface {}`), because results, error messages and `%T` all depend on it.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;

use crate::gostd::strconv;
use crate::gostd::time::{self, Loc, Time};
use crate::types::{Kind, Named, Struct, Type};

/// A Go string: an immutable byte string that may hold invalid UTF-8.
pub type Str = Rc<[u8]>;

/// Makes a [`Str`] from anything byte-like.
pub fn str_of(bytes: impl AsRef<[u8]>) -> Str {
    Rc::from(bytes.as_ref())
}

/// A slice or array value: its Go type and its elements.
#[derive(Clone, Debug)]
pub struct Seq {
    pub ty: Type,
    pub items: Vec<Value>,
    /// A nil slice: prints like an empty one but is nil for `==`, `??` and JSON.
    pub nil: bool,
}

/// A map value. Go maps are unordered; entries keep insertion order here, and printing sorts them like `fmt`.
#[derive(Clone, Debug)]
pub struct MapValue {
    pub ty: Type,
    entries: Vec<(Value, Value)>,
    index: HashMap<Key, usize>,
    nans: u64,
}

/// A function value: a method bound to its receiver.
#[derive(Clone, Debug)]
pub struct FuncValue {
    pub ty: Type,
    pub receiver: Value,
    pub method: &'static str,
}

/// A Go value with its dynamic type.
#[derive(Clone, Debug)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Uint(u64),
    Uint8(u8),
    Uint16(u16),
    Uint32(u32),
    Uint64(u64),
    Float32(f32),
    Float64(f64),
    String(Str),
    Slice(Rc<Seq>),
    Array(Rc<Seq>),
    Map(Rc<MapValue>),
    Time(Time),
    Duration(i64),
    Month(i64),
    Weekday(i64),
    Location(Loc),
    /// A `time.Location` struct, the result of dereferencing a `*time.Location`.
    LocationValue(Loc),
    Func(Rc<FuncValue>),
    EmptyStruct,
}

impl Value {
    pub fn string(bytes: impl AsRef<[u8]>) -> Value {
        Value::String(str_of(bytes))
    }

    pub fn slice(ty_elem: Type, items: Vec<Value>) -> Value {
        Value::Slice(Rc::new(Seq { ty: Type::slice(ty_elem), items, nil: false }))
    }

    /// A nil slice of the given element type.
    pub fn nil_slice(ty_elem: Type) -> Value {
        Value::Slice(Rc::new(Seq { ty: Type::slice(ty_elem), items: Vec::new(), nil: true }))
    }

    /// A Go array value (`[N]T`).
    pub fn array(ty_elem: Type, items: Vec<Value>) -> Value {
        Value::Array(Rc::new(Seq { ty: Type::array(items.len(), ty_elem), items, nil: false }))
    }

    /// A `[]interface {}`.
    pub fn any_slice(items: Vec<Value>) -> Value {
        Value::slice(Type::Any, items)
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// The dynamic type, `reflect.TypeOf(v)`; `None` for nil.
    pub fn type_of(&self) -> Option<Type> {
        Some(match self {
            Value::Nil => return None,
            Value::Bool(_) => Type::Bool,
            Value::Int(_) => Type::Int,
            Value::Int8(_) => Type::Int8,
            Value::Int16(_) => Type::Int16,
            Value::Int32(_) => Type::Int32,
            Value::Int64(_) => Type::Int64,
            Value::Uint(_) => Type::Uint,
            Value::Uint8(_) => Type::Uint8,
            Value::Uint16(_) => Type::Uint16,
            Value::Uint32(_) => Type::Uint32,
            Value::Uint64(_) => Type::Uint64,
            Value::Float32(_) => Type::Float32,
            Value::Float64(_) => Type::Float64,
            Value::String(_) => Type::String,
            Value::Slice(s) | Value::Array(s) => s.ty.clone(),
            Value::Map(m) => m.ty.clone(),
            Value::Time(_) => Type::time(),
            Value::Duration(_) => Type::duration(),
            Value::Month(_) => Type::Named(Named::Month),
            Value::Weekday(_) => Type::Named(Named::Weekday),
            Value::Location(_) => Type::location_ptr(),
            Value::LocationValue(_) => Type::Struct(Struct::Location),
            Value::Func(f) => f.ty.clone(),
            Value::EmptyStruct => Type::Struct(Struct::Empty),
        })
    }

    pub fn kind(&self) -> Kind {
        self.type_of().map_or(Kind::Invalid, |t| t.kind())
    }

    /// `%T`: the dynamic type, or `<nil>`.
    pub fn type_name(&self) -> String {
        self.type_of().map_or_else(|| "<nil>".to_string(), |t| t.to_string())
    }

    /// The elements of a slice or array.
    pub fn items(&self) -> Option<&[Value]> {
        match self {
            Value::Slice(s) | Value::Array(s) => Some(&s.items),
            _ => None,
        }
    }

    /// The value as a signed integer, for any integer kind (Go's `reflect.Value.Int` or a conversion).
    pub fn as_i64(&self) -> Option<i64> {
        Some(match self {
            Value::Int(v) | Value::Int64(v) | Value::Duration(v) | Value::Month(v) | Value::Weekday(v) => *v,
            Value::Int8(v) => i64::from(*v),
            Value::Int16(v) => i64::from(*v),
            Value::Int32(v) => i64::from(*v),
            Value::Uint(v) | Value::Uint64(v) => *v as i64,
            Value::Uint8(v) => i64::from(*v),
            Value::Uint16(v) => i64::from(*v),
            Value::Uint32(v) => i64::from(*v),
            _ => return None,
        })
    }

    /// Whether the value is one of the builtin integer or float types (what expr's type switches accept).
    pub fn is_builtin_number(&self) -> bool {
        matches!(
            self,
            Value::Int(_)
                | Value::Int8(_)
                | Value::Int16(_)
                | Value::Int32(_)
                | Value::Int64(_)
                | Value::Uint(_)
                | Value::Uint8(_)
                | Value::Uint16(_)
                | Value::Uint32(_)
                | Value::Uint64(_)
                | Value::Float32(_)
                | Value::Float64(_)
        )
    }

    /// Whether the value is an empty string, slice, array or map.
    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }

    /// Go's `reflect.Value.Len` for strings (bytes), slices, arrays and maps.
    pub fn len(&self) -> Option<usize> {
        match self {
            Value::String(s) => Some(s.len()),
            Value::Slice(s) | Value::Array(s) => Some(s.items.len()),
            Value::Map(m) => Some(m.len()),
            _ => None,
        }
    }
}

/// The hashable identity of a map key, following Go's `==` on interface values.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Nil,
    Bool(bool),
    Signed(Kind, i64),
    Unsigned(Kind, u64),
    Float(Kind, u64),
    Nan(u64),
    String(Str),
    Time(i64, i64, usize),
    Named(Named, i64),
    Location(usize),
    Array(Vec<Key>),
    EmptyStruct,
}

/// A key that cannot be hashed; carries the type Go names in its panic.
#[derive(Debug)]
pub struct Unhashable(pub Type);

impl MapValue {
    pub fn new(ty: Type) -> MapValue {
        MapValue { ty, entries: Vec::new(), index: HashMap::new(), nans: 0 }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[(Value, Value)] {
        &self.entries
    }

    fn key(&mut self, value: &Value, fresh_nan: bool) -> Result<Key, Unhashable> {
        let mut nans = self.nans;
        let key = key_of(value, &mut || {
            if fresh_nan {
                nans += 1;
            }
            nans
        })?;
        self.nans = nans;
        Ok(key)
    }

    /// Looks a key up; NaN keys never match, like Go.
    pub fn get(&self, key: &Value) -> Result<Option<&Value>, Unhashable> {
        let k = key_of(key, &mut || u64::MAX)?;
        if matches!(k, Key::Nan(_)) {
            return Ok(None);
        }
        Ok(self.index.get(&k).map(|&i| &self.entries[i].1))
    }

    /// Sets a key, replacing the value of an equal key in place.
    pub fn insert(&mut self, key: Value, value: Value) -> Result<(), Unhashable> {
        let k = self.key(&key, true)?;
        match self.index.get(&k) {
            Some(&i) => self.entries[i].1 = value,
            None => {
                self.index.insert(k, self.entries.len());
                self.entries.push((key, value));
            }
        }
        Ok(())
    }

    /// The value of a key as a mutable reference, for accumulating maps.
    pub fn get_mut(&mut self, key: &Value) -> Result<Option<&mut Value>, Unhashable> {
        let k = key_of(key, &mut || u64::MAX)?;
        if matches!(k, Key::Nan(_)) {
            return Ok(None);
        }
        Ok(self.index.get(&k).copied().map(move |i| &mut self.entries[i].1))
    }
}

fn key_of(value: &Value, nan: &mut dyn FnMut() -> u64) -> Result<Key, Unhashable> {
    Ok(match value {
        Value::Nil => Key::Nil,
        Value::Bool(b) => Key::Bool(*b),
        Value::Int(v) => Key::Signed(Kind::Int, *v),
        Value::Int8(v) => Key::Signed(Kind::Int8, i64::from(*v)),
        Value::Int16(v) => Key::Signed(Kind::Int16, i64::from(*v)),
        Value::Int32(v) => Key::Signed(Kind::Int32, i64::from(*v)),
        Value::Int64(v) => Key::Signed(Kind::Int64, *v),
        Value::Uint(v) => Key::Unsigned(Kind::Uint, *v),
        Value::Uint8(v) => Key::Unsigned(Kind::Uint8, u64::from(*v)),
        Value::Uint16(v) => Key::Unsigned(Kind::Uint16, u64::from(*v)),
        Value::Uint32(v) => Key::Unsigned(Kind::Uint32, u64::from(*v)),
        Value::Uint64(v) => Key::Unsigned(Kind::Uint64, *v),
        Value::Float32(v) => float_key(Kind::Float32, f64::from(*v), nan),
        Value::Float64(v) => float_key(Kind::Float64, *v, nan),
        Value::String(s) => Key::String(s.clone()),
        Value::Time(t) => {
            let (sec, nsec, loc) = t.identity();
            Key::Time(sec, i64::from(nsec), loc)
        }
        Value::Duration(v) => Key::Named(Named::Duration, *v),
        Value::Month(v) => Key::Named(Named::Month, *v),
        Value::Weekday(v) => Key::Named(Named::Weekday, *v),
        Value::Location(l) => Key::Location(Rc::as_ptr(l) as usize),
        Value::LocationValue(l) => Key::String(str_of(l.name())),
        Value::Array(s) => {
            let mut keys = Vec::with_capacity(s.items.len());
            for item in &s.items {
                keys.push(key_of(item, nan)?);
            }
            Key::Array(keys)
        }
        Value::EmptyStruct => Key::EmptyStruct,
        Value::Slice(s) => return Err(Unhashable(s.ty.clone())),
        Value::Map(m) => return Err(Unhashable(m.ty.clone())),
        Value::Func(f) => return Err(Unhashable(f.ty.clone())),
    })
}

fn float_key(kind: Kind, f: f64, nan: &mut dyn FnMut() -> u64) -> Key {
    if f.is_nan() {
        Key::Nan(nan())
    } else if f == 0.0 {
        Key::Float(kind, 0)
    } else {
        Key::Float(kind, f.to_bits())
    }
}

/// Go's `reflect.DeepEqual` for the values an expression can hold.
pub fn deep_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Float32(x), Value::Float32(y)) => x == y,
        (Value::Float64(x), Value::Float64(y)) => x == y,
        (Value::Slice(x), Value::Slice(y)) | (Value::Array(x), Value::Array(y)) => {
            x.ty == y.ty
                && x.items.len() == y.items.len()
                && (Rc::ptr_eq(x, y) && !matches!(a, Value::Array(_))
                    || x.items.iter().zip(&y.items).all(|(p, q)| deep_equal(p, q)))
        }
        (Value::Map(x), Value::Map(y)) => {
            x.ty == y.ty
                && x.len() == y.len()
                && (Rc::ptr_eq(x, y)
                    || x.entries.iter().all(|(k, v)| match y.get(k) {
                        Ok(Some(w)) => deep_equal(v, w),
                        _ => false,
                    }))
        }
        (Value::Time(x), Value::Time(y)) => x.identity() == y.identity(),
        (Value::Location(x), Value::Location(y)) | (Value::LocationValue(x), Value::LocationValue(y)) => {
            Rc::ptr_eq(x, y) || x.deep_eq(y)
        }
        (Value::Func(_), Value::Func(_)) => false,
        _ => match (a.type_of(), b.type_of()) {
            (Some(ta), Some(tb)) if ta == tb => a.as_i64() == b.as_i64() && a.as_i64().is_some(),
            _ => false,
        },
    }
}

/// Formats values like Go's `fmt` package for the verbs expr uses (`%v`, `%s`, `%d`, `%q`).
pub struct GoFmt<'a> {
    out: &'a mut Vec<u8>,
}

impl GoFmt<'_> {
    /// Prints `value` with `verb` the way `fmt.Sprintf("%<verb>", value)` does.
    pub fn print(out: &mut Vec<u8>, verb: u8, value: &Value) {
        GoFmt { out }.value(verb, value, 0);
    }

    fn put(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
    }

    fn value(&mut self, verb: u8, value: &Value, depth: usize) {
        if let Some(s) = stringer(value) {
            if matches!(verb, b'v' | b's') {
                self.out.extend_from_slice(&s);
                return;
            }
            if verb == b'q' {
                self.put(&strconv::quote(&s));
                return;
            }
        }
        match value {
            Value::Nil => {
                if depth == 0 && verb != b'v' {
                    self.bad_verb(verb, value);
                } else {
                    self.put("<nil>");
                }
            }
            Value::Bool(b) => match verb {
                b'v' | b't' => self.put(if *b { "true" } else { "false" }),
                _ => self.bad_verb(verb, value),
            },
            Value::String(s) => match verb {
                b'v' | b's' => self.out.extend_from_slice(s),
                b'q' => self.put(&strconv::quote(s)),
                _ => self.bad_verb(verb, value),
            },
            Value::Float32(f) => match verb {
                b'v' => self.put(&strconv::fmt_v_float(f64::from(*f), 32)),
                _ => self.bad_verb(verb, value),
            },
            Value::Float64(f) => match verb {
                b'v' => self.put(&strconv::fmt_v_float(*f, 64)),
                _ => self.bad_verb(verb, value),
            },
            Value::Slice(seq) | Value::Array(seq) => {
                if verb == b's' && seq.ty.elem() == Some(&Type::Uint8) && matches!(value, Value::Slice(_)) {
                    for item in &seq.items {
                        if let Value::Uint8(b) = item {
                            self.out.push(*b);
                        }
                    }
                    return;
                }
                self.out.push(b'[');
                for (i, item) in seq.items.iter().enumerate() {
                    if i > 0 {
                        self.out.push(b' ');
                    }
                    self.value(verb, item, depth + 1);
                }
                self.out.push(b']');
            }
            Value::Map(m) => {
                self.put("map[");
                for (i, (k, v)) in sorted_entries(m).into_iter().enumerate() {
                    if i > 0 {
                        self.out.push(b' ');
                    }
                    self.value(verb, k, depth + 1);
                    self.out.push(b':');
                    self.value(verb, v, depth + 1);
                }
                self.out.push(b']');
            }
            Value::Func(_) => match verb {
                b'v' => self.put("0xc000010000"),
                _ => self.bad_verb(verb, value),
            },
            Value::EmptyStruct => self.put("{}"),
            Value::LocationValue(l) => {
                self.out.push(b'{');
                self.out.extend_from_slice(l.name());
                self.out.extend_from_slice(b" ...}");
            }
            Value::Time(_) | Value::Location(_) => self.bad_verb(verb, value),
            _ => match verb {
                b'v' | b'd' => {
                    let mut s = String::new();
                    match value {
                        Value::Uint(v) | Value::Uint64(v) => {
                            let _ = write!(s, "{v}");
                        }
                        other => {
                            let _ = write!(s, "{}", other.as_i64().unwrap_or_default());
                        }
                    }
                    self.put(&s);
                }
                _ => self.bad_verb(verb, value),
            },
        }
    }

    fn bad_verb(&mut self, verb: u8, value: &Value) {
        self.put("%!");
        self.out.push(verb);
        self.out.push(b'(');
        match value {
            Value::Nil => self.put("<nil>"),
            _ => {
                self.put(&value.type_name());
                self.out.push(b'=');
                self.value(b'v', value, 1);
            }
        }
        self.out.push(b')');
    }
}

/// The `String()` result of the values that implement `fmt.Stringer`.
pub fn stringer(value: &Value) -> Option<Vec<u8>> {
    Some(match value {
        Value::Time(t) => t.string(),
        Value::Duration(d) => time::duration_string(*d).into_bytes(),
        Value::Month(m) => time::month_string(*m).into_bytes(),
        Value::Weekday(d) => time::weekday_string(*d).into_bytes(),
        Value::Location(l) => l.name().to_vec(),
        _ => return None,
    })
}

/// `fmt.Sprintf("%v", value)`.
pub fn sprint_v(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    GoFmt::print(&mut out, b'v', value);
    out
}

/// `fmt.Sprintf("%v", value)` as a (lossy) Rust string, for error messages.
pub fn sprint_v_lossy(value: &Value) -> String {
    String::from_utf8_lossy(&sprint_v(value)).into_owned()
}

/// `fmt.Sprintf("%<verb>", value)` as a lossy Rust string.
pub fn sprintf_lossy(verb: u8, value: &Value) -> String {
    let mut out = Vec::new();
    GoFmt::print(&mut out, verb, value);
    String::from_utf8_lossy(&out).into_owned()
}

/// Map entries in the order `fmt` prints them (Go's internal/fmtsort).
pub fn sorted_entries(m: &MapValue) -> Vec<(&Value, &Value)> {
    let mut entries: Vec<(&Value, &Value)> = m.entries.iter().map(|(k, v)| (k, v)).collect();
    entries.sort_by(|a, b| fmtsort_compare(a.0, b.0));
    entries
}

/// The order of different dynamic key types under fmtsort: the original compares the addresses of their type
/// descriptors, and this is the order those descriptors have in the original program.
fn type_rank(value: &Value) -> u8 {
    match value {
        Value::Nil => 0,
        Value::String(_) => 1,
        Value::Uint8(_) => 2,
        Value::Int64(_) => 3,
        Value::Int(_) => 4,
        Value::Float64(_) => 5,
        Value::Bool(_) => 6,
        Value::Duration(_) => 7,
        Value::Int8(_) => 8,
        Value::Int16(_) => 9,
        Value::Int32(_) => 10,
        Value::Uint(_) => 11,
        Value::Uint16(_) => 12,
        Value::Uint32(_) => 13,
        Value::Uint64(_) => 14,
        Value::Float32(_) => 15,
        Value::Month(_) => 16,
        Value::Weekday(_) => 17,
        Value::Time(_) => 18,
        Value::Location(_) | Value::LocationValue(_) => 19,
        Value::Array(_) => 20,
        Value::EmptyStruct => 21,
        Value::Slice(_) | Value::Map(_) | Value::Func(_) => 22,
    }
}

fn fmtsort_compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (ra, rb) = (type_rank(a), type_rank(b));
    if ra != rb || matches!(a, Value::Array(_)) && a.type_of() != b.type_of() {
        return ra.cmp(&rb).then_with(|| a.type_name().cmp(&b.type_name()));
    }
    match (a, b) {
        (Value::Nil, Value::Nil) => Ordering::Equal,
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Uint(x), Value::Uint(y)) | (Value::Uint64(x), Value::Uint64(y)) => x.cmp(y),
        (Value::Float32(x), Value::Float32(y)) => float_order(f64::from(*x), f64::from(*y)),
        (Value::Float64(x), Value::Float64(y)) => float_order(*x, *y),
        (Value::Time(x), Value::Time(y)) => x.identity().cmp(&y.identity()),
        (Value::Array(x), Value::Array(y)) => {
            for (p, q) in x.items.iter().zip(&y.items) {
                let c = fmtsort_compare(p, q);
                if c != Ordering::Equal {
                    return c;
                }
            }
            Ordering::Equal
        }
        _ => match (a.as_i64(), b.as_i64()) {
            (Some(x), Some(y)) => x.cmp(&y),
            _ => Ordering::Equal,
        },
    }
}

fn float_order(a: f64, b: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
    }
}
