//! The Go type model the language works with.
//!
//! expr reasons about values through Go's `reflect.Type`: the checker compares types, the error messages print
//! them, and `%T` shows them. [`Type`] covers every type an expression can produce from a JSON environment and
//! the builtins, with Go's spelling, kinds, method sets and assignability rules.

use std::fmt;
use std::rc::Rc;

/// The kind of a type, as Go's `reflect.Kind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    #[default]
    Invalid,
    Bool,
    Int,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Uintptr,
    Float32,
    Float64,
    Complex64,
    Complex128,
    Array,
    Chan,
    Func,
    Interface,
    Map,
    Ptr,
    Slice,
    String,
    Struct,
    UnsafePointer,
}

impl Kind {
    /// All kinds in `reflect.Kind` order, `Invalid` first.
    pub const ALL: [Kind; 27] = [
        Kind::Invalid,
        Kind::Bool,
        Kind::Int,
        Kind::Int8,
        Kind::Int16,
        Kind::Int32,
        Kind::Int64,
        Kind::Uint,
        Kind::Uint8,
        Kind::Uint16,
        Kind::Uint32,
        Kind::Uint64,
        Kind::Uintptr,
        Kind::Float32,
        Kind::Float64,
        Kind::Complex64,
        Kind::Complex128,
        Kind::Array,
        Kind::Chan,
        Kind::Func,
        Kind::Interface,
        Kind::Map,
        Kind::Ptr,
        Kind::Slice,
        Kind::String,
        Kind::Struct,
        Kind::UnsafePointer,
    ];

    /// The name `reflect.Kind.String` prints.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Invalid => "invalid",
            Kind::Bool => "bool",
            Kind::Int => "int",
            Kind::Int8 => "int8",
            Kind::Int16 => "int16",
            Kind::Int32 => "int32",
            Kind::Int64 => "int64",
            Kind::Uint => "uint",
            Kind::Uint8 => "uint8",
            Kind::Uint16 => "uint16",
            Kind::Uint32 => "uint32",
            Kind::Uint64 => "uint64",
            Kind::Uintptr => "uintptr",
            Kind::Float32 => "float32",
            Kind::Float64 => "float64",
            Kind::Complex64 => "complex64",
            Kind::Complex128 => "complex128",
            Kind::Array => "array",
            Kind::Chan => "chan",
            Kind::Func => "func",
            Kind::Interface => "interface",
            Kind::Map => "map",
            Kind::Ptr => "ptr",
            Kind::Slice => "slice",
            Kind::String => "string",
            Kind::Struct => "struct",
            Kind::UnsafePointer => "unsafe.Pointer",
        }
    }

    /// Looks a kind up by the name `reflect.Kind.String` prints.
    pub fn by_name(name: &str) -> Option<Kind> {
        Kind::ALL.iter().skip(1).copied().find(|k| k.name() == name)
    }

    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Kind::Int
                | Kind::Int8
                | Kind::Int16
                | Kind::Int32
                | Kind::Int64
                | Kind::Uint
                | Kind::Uint8
                | Kind::Uint16
                | Kind::Uint32
                | Kind::Uint64
                | Kind::Uintptr
        )
    }

    pub fn is_float(self) -> bool {
        matches!(self, Kind::Float32 | Kind::Float64)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The named struct types of the `time` package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Struct {
    /// `struct {}`, the element type of the optimizer's constant sets.
    Empty,
    /// `time.Time`.
    Time,
    /// `time.Location`, only ever seen behind a pointer.
    Location,
}

/// The named non-struct types of the `time` package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Named {
    /// `time.Duration`, an `int64`.
    Duration,
    /// `time.Month`, an `int`.
    Month,
    /// `time.Weekday`, an `int`.
    Weekday,
}

/// A function signature.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FuncType {
    pub ins: Vec<Type>,
    pub outs: Vec<Type>,
    pub variadic: bool,
}

/// A Go type, compared by identity like `reflect.Type`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Bool,
    Int,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Uintptr,
    Float32,
    Float64,
    String,
    /// `interface {}`.
    Any,
    /// The predeclared `error` interface.
    Error,
    Slice(Rc<Type>),
    Array(usize, Rc<Type>),
    Map(Rc<Type>, Rc<Type>),
    Ptr(Rc<Type>),
    Func(Rc<FuncType>),
    Struct(Struct),
    Named(Named),
}

impl Type {
    pub fn slice(elem: Type) -> Type {
        Type::Slice(Rc::new(elem))
    }

    pub fn array(len: usize, elem: Type) -> Type {
        Type::Array(len, Rc::new(elem))
    }

    pub fn map(key: Type, elem: Type) -> Type {
        Type::Map(Rc::new(key), Rc::new(elem))
    }

    pub fn ptr(elem: Type) -> Type {
        Type::Ptr(Rc::new(elem))
    }

    pub fn func(ins: Vec<Type>, outs: Vec<Type>, variadic: bool) -> Type {
        Type::Func(Rc::new(FuncType { ins, outs, variadic }))
    }

    pub fn time() -> Type {
        Type::Struct(Struct::Time)
    }

    pub fn duration() -> Type {
        Type::Named(Named::Duration)
    }

    pub fn location_ptr() -> Type {
        Type::ptr(Type::Struct(Struct::Location))
    }

    /// `[]interface {}`.
    pub fn any_slice() -> Type {
        Type::slice(Type::Any)
    }

    /// `map[string]interface {}`.
    pub fn string_map() -> Type {
        Type::map(Type::String, Type::Any)
    }

    /// `map[interface {}]interface {}`.
    pub fn any_map() -> Type {
        Type::map(Type::Any, Type::Any)
    }

    /// `[]uint8`.
    pub fn bytes() -> Type {
        Type::slice(Type::Uint8)
    }

    pub fn kind(&self) -> Kind {
        match self {
            Type::Bool => Kind::Bool,
            Type::Int => Kind::Int,
            Type::Int8 => Kind::Int8,
            Type::Int16 => Kind::Int16,
            Type::Int32 => Kind::Int32,
            Type::Int64 => Kind::Int64,
            Type::Uint => Kind::Uint,
            Type::Uint8 => Kind::Uint8,
            Type::Uint16 => Kind::Uint16,
            Type::Uint32 => Kind::Uint32,
            Type::Uint64 => Kind::Uint64,
            Type::Uintptr => Kind::Uintptr,
            Type::Float32 => Kind::Float32,
            Type::Float64 => Kind::Float64,
            Type::String => Kind::String,
            Type::Any | Type::Error => Kind::Interface,
            Type::Slice(_) => Kind::Slice,
            Type::Array(..) => Kind::Array,
            Type::Map(..) => Kind::Map,
            Type::Ptr(_) => Kind::Ptr,
            Type::Func(_) => Kind::Func,
            Type::Struct(_) => Kind::Struct,
            Type::Named(Named::Duration) => Kind::Int64,
            Type::Named(Named::Month | Named::Weekday) => Kind::Int,
        }
    }

    /// The element type of a slice, array, map or pointer (`reflect.Type.Elem`).
    pub fn elem(&self) -> Option<&Type> {
        match self {
            Type::Slice(e) | Type::Array(_, e) | Type::Map(_, e) | Type::Ptr(e) => Some(e),
            _ => None,
        }
    }

    /// The key type of a map.
    pub fn key(&self) -> Option<&Type> {
        match self {
            Type::Map(k, _) => Some(k),
            _ => None,
        }
    }

    pub fn as_func(&self) -> Option<&FuncType> {
        match self {
            Type::Func(f) => Some(f),
            _ => None,
        }
    }

    /// The package path of a named type; empty for predeclared and unnamed types.
    pub fn pkg_path(&self) -> &'static str {
        match self {
            Type::Struct(Struct::Time | Struct::Location) | Type::Named(_) => "time",
            _ => "",
        }
    }

    /// The name of a named type (`reflect.Type.Name`).
    pub fn name(&self) -> &'static str {
        match self {
            Type::Struct(Struct::Time) => "Time",
            Type::Struct(Struct::Location) => "Location",
            Type::Named(Named::Duration) => "Duration",
            Type::Named(Named::Month) => "Month",
            Type::Named(Named::Weekday) => "Weekday",
            Type::Struct(Struct::Empty) => "",
            Type::Slice(_) | Type::Array(..) | Type::Map(..) | Type::Ptr(_) | Type::Func(_) => "",
            Type::Any => "",
            other => kind_type_name(other.kind()),
        }
    }

    /// Whether the type is one of Go's builtin integer types (not a named one such as `time.Month`).
    pub fn is_builtin_integer(&self) -> bool {
        matches!(
            self,
            Type::Int
                | Type::Int8
                | Type::Int16
                | Type::Int32
                | Type::Int64
                | Type::Uint
                | Type::Uint8
                | Type::Uint16
                | Type::Uint32
                | Type::Uint64
                | Type::Uintptr
        )
    }

    pub fn is_builtin_float(&self) -> bool {
        matches!(self, Type::Float32 | Type::Float64)
    }

    /// Go's `reflect.Type.AssignableTo` for the types an expression can meet.
    pub fn assignable_to(&self, to: &Type) -> bool {
        if self == to {
            return true;
        }
        match to {
            Type::Any => true,
            Type::Error => false,
            _ => false,
        }
    }

    /// The exported methods of the type, sorted by name like `reflect.Type.Method`.
    pub fn methods(&self) -> &'static [Method] {
        match self {
            Type::Struct(Struct::Time) => TIME_METHODS,
            Type::Named(Named::Duration) => DURATION_METHODS,
            Type::Named(Named::Month) => MONTH_METHODS,
            Type::Named(Named::Weekday) => WEEKDAY_METHODS,
            Type::Ptr(e) if **e == Type::Struct(Struct::Location) => LOCATION_METHODS,
            Type::Error => ERROR_METHODS,
            _ => &[],
        }
    }

    pub fn num_method(&self) -> usize {
        self.methods().len()
    }

    /// Looks a method up by name; returns its index in the method table.
    pub fn method_by_name(&self, name: &str) -> Option<(usize, &'static Method)> {
        self.methods().iter().enumerate().find(|(_, m)| m.name == name)
    }

    /// The type of a method as `reflect.Type.Method(i).Type`: a function whose first input is the receiver.
    pub fn method_type(&self, method: &Method) -> Type {
        let mut ins = vec![self.clone()];
        ins.extend(method.ins.iter().map(|s| s.build()));
        Type::func(ins, method.outs.iter().map(|s| s.build()).collect(), false)
    }

    /// The type of a method value bound to a receiver: the method type without the receiver.
    pub fn bound_method_type(method: &Method) -> Type {
        Type::func(
            method.ins.iter().map(|s| s.build()).collect(),
            method.outs.iter().map(|s| s.build()).collect(),
            false,
        )
    }

    /// Whether values of the type can be map keys (Go's comparable types).
    pub fn is_hashable(&self) -> bool {
        match self {
            Type::Slice(_) | Type::Map(..) | Type::Func(_) => false,
            Type::Array(_, e) => e.is_hashable(),
            _ => true,
        }
    }
}

fn kind_type_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Interface => "error",
        other => other.name(),
    }
}

impl fmt::Display for Type {
    /// Go's `reflect.Type.String`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Any => f.write_str("interface {}"),
            Type::Error => f.write_str("error"),
            Type::Slice(e) => write!(f, "[]{e}"),
            Type::Array(n, e) => write!(f, "[{n}]{e}"),
            Type::Map(k, v) => write!(f, "map[{k}]{v}"),
            Type::Ptr(e) => write!(f, "*{e}"),
            Type::Func(func) => {
                f.write_str("func(")?;
                for (i, t) in func.ins.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    if func.variadic && i == func.ins.len() - 1 {
                        match t {
                            Type::Slice(e) => write!(f, "...{e}")?,
                            other => write!(f, "...{other}")?,
                        }
                    } else {
                        write!(f, "{t}")?;
                    }
                }
                f.write_str(")")?;
                match func.outs.len() {
                    0 => Ok(()),
                    1 => write!(f, " {}", func.outs[0]),
                    _ => {
                        f.write_str(" (")?;
                        for (i, t) in func.outs.iter().enumerate() {
                            if i > 0 {
                                f.write_str(", ")?;
                            }
                            write!(f, "{t}")?;
                        }
                        f.write_str(")")
                    }
                }
            }
            Type::Struct(Struct::Empty) => f.write_str("struct {}"),
            Type::Struct(Struct::Time) => f.write_str("time.Time"),
            Type::Struct(Struct::Location) => f.write_str("time.Location"),
            Type::Named(Named::Duration) => f.write_str("time.Duration"),
            Type::Named(Named::Month) => f.write_str("time.Month"),
            Type::Named(Named::Weekday) => f.write_str("time.Weekday"),
            other => f.write_str(other.kind().name()),
        }
    }
}

/// A type used in the static method tables.
#[derive(Debug)]
pub enum Sig {
    Int,
    Int64,
    Float64,
    Bool,
    String,
    Bytes,
    Error,
    Time,
    Duration,
    Month,
    Weekday,
    LocationPtr,
}

impl Sig {
    fn build(&self) -> Type {
        match self {
            Sig::Int => Type::Int,
            Sig::Int64 => Type::Int64,
            Sig::Float64 => Type::Float64,
            Sig::Bool => Type::Bool,
            Sig::String => Type::String,
            Sig::Bytes => Type::bytes(),
            Sig::Error => Type::Error,
            Sig::Time => Type::time(),
            Sig::Duration => Type::duration(),
            Sig::Month => Type::Named(Named::Month),
            Sig::Weekday => Type::Named(Named::Weekday),
            Sig::LocationPtr => Type::location_ptr(),
        }
    }
}

/// An exported method of one of the `time` types.
#[derive(Debug)]
pub struct Method {
    pub name: &'static str,
    pub ins: &'static [Sig],
    pub outs: &'static [Sig],
}

const fn m(name: &'static str, ins: &'static [Sig], outs: &'static [Sig]) -> Method {
    Method { name, ins, outs }
}

/// The value methods of `time.Time` in Go 1.25, in `reflect` order.
pub static TIME_METHODS: &[Method] = &[
    m("Add", &[Sig::Duration], &[Sig::Time]),
    m("AddDate", &[Sig::Int, Sig::Int, Sig::Int], &[Sig::Time]),
    m("After", &[Sig::Time], &[Sig::Bool]),
    m("AppendBinary", &[Sig::Bytes], &[Sig::Bytes, Sig::Error]),
    m("AppendFormat", &[Sig::Bytes, Sig::String], &[Sig::Bytes]),
    m("AppendText", &[Sig::Bytes], &[Sig::Bytes, Sig::Error]),
    m("Before", &[Sig::Time], &[Sig::Bool]),
    m("Clock", &[], &[Sig::Int, Sig::Int, Sig::Int]),
    m("Compare", &[Sig::Time], &[Sig::Int]),
    m("Date", &[], &[Sig::Int, Sig::Month, Sig::Int]),
    m("Day", &[], &[Sig::Int]),
    m("Equal", &[Sig::Time], &[Sig::Bool]),
    m("Format", &[Sig::String], &[Sig::String]),
    m("GoString", &[], &[Sig::String]),
    m("GobEncode", &[], &[Sig::Bytes, Sig::Error]),
    m("Hour", &[], &[Sig::Int]),
    m("ISOWeek", &[], &[Sig::Int, Sig::Int]),
    m("In", &[Sig::LocationPtr], &[Sig::Time]),
    m("IsDST", &[], &[Sig::Bool]),
    m("IsZero", &[], &[Sig::Bool]),
    m("Local", &[], &[Sig::Time]),
    m("Location", &[], &[Sig::LocationPtr]),
    m("MarshalBinary", &[], &[Sig::Bytes, Sig::Error]),
    m("MarshalJSON", &[], &[Sig::Bytes, Sig::Error]),
    m("MarshalText", &[], &[Sig::Bytes, Sig::Error]),
    m("Minute", &[], &[Sig::Int]),
    m("Month", &[], &[Sig::Month]),
    m("Nanosecond", &[], &[Sig::Int]),
    m("Round", &[Sig::Duration], &[Sig::Time]),
    m("Second", &[], &[Sig::Int]),
    m("String", &[], &[Sig::String]),
    m("Sub", &[Sig::Time], &[Sig::Duration]),
    m("Truncate", &[Sig::Duration], &[Sig::Time]),
    m("UTC", &[], &[Sig::Time]),
    m("Unix", &[], &[Sig::Int64]),
    m("UnixMicro", &[], &[Sig::Int64]),
    m("UnixMilli", &[], &[Sig::Int64]),
    m("UnixNano", &[], &[Sig::Int64]),
    m("Weekday", &[], &[Sig::Weekday]),
    m("Year", &[], &[Sig::Int]),
    m("YearDay", &[], &[Sig::Int]),
    m("Zone", &[], &[Sig::String, Sig::Int]),
    m("ZoneBounds", &[], &[Sig::Time, Sig::Time]),
];

/// The methods of `time.Duration`.
pub static DURATION_METHODS: &[Method] = &[
    m("Abs", &[], &[Sig::Duration]),
    m("Hours", &[], &[Sig::Float64]),
    m("Microseconds", &[], &[Sig::Int64]),
    m("Milliseconds", &[], &[Sig::Int64]),
    m("Minutes", &[], &[Sig::Float64]),
    m("Nanoseconds", &[], &[Sig::Int64]),
    m("Round", &[Sig::Duration], &[Sig::Duration]),
    m("Seconds", &[], &[Sig::Float64]),
    m("String", &[], &[Sig::String]),
    m("Truncate", &[Sig::Duration], &[Sig::Duration]),
];

pub static MONTH_METHODS: &[Method] = &[m("String", &[], &[Sig::String])];
pub static WEEKDAY_METHODS: &[Method] = &[m("String", &[], &[Sig::String])];
pub static LOCATION_METHODS: &[Method] = &[m("String", &[], &[Sig::String])];
pub static ERROR_METHODS: &[Method] = &[m("Error", &[], &[Sig::String])];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spells_types_like_reflect() {
        assert_eq!(Type::any_slice().to_string(), "[]interface {}");
        assert_eq!(Type::map(Type::Any, Type::any_slice()).to_string(), "map[interface {}][]interface {}");
        assert_eq!(Type::slice(Type::array(2, Type::Any)).to_string(), "[][2]interface {}");
        assert_eq!(Type::location_ptr().to_string(), "*time.Location");
        let f = Type::func(vec![Type::String, Type::slice(Type::Any)], vec![Type::String], true);
        assert_eq!(f.to_string(), "func(string, ...interface {}) string");
        let g = Type::func(vec![Type::String], vec![Type::location_ptr(), Type::Error], false);
        assert_eq!(g.to_string(), "func(string) (*time.Location, error)");
    }

    #[test]
    fn method_types_carry_the_receiver() {
        let (index, add) = Type::time().method_by_name("Add").expect("Add");
        assert_eq!(index, 0);
        assert_eq!(Type::time().method_type(add).to_string(), "func(time.Time, time.Duration) time.Time");
        assert_eq!(Type::bound_method_type(add).to_string(), "func(time.Duration) time.Time");
        assert_eq!(Type::time().num_method(), 43);
    }

    #[test]
    fn named_types_are_not_assignable_to_their_underlying_type() {
        assert!(!Type::duration().assignable_to(&Type::Int64));
        assert!(Type::duration().assignable_to(&Type::Any));
        assert_eq!(Type::duration().kind(), Kind::Int64);
        assert!(!Type::duration().is_builtin_integer());
    }
}
