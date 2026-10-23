//! Natures: what the checker knows about the value of a node.
//!
//! A nature is a Go type plus what the type alone does not say: the element nature of arrays built by the
//! language, the fields of the environment, whether the value is nil, and the builtin a name refers to.

use std::collections::HashMap;
use std::rc::Rc;

use crate::types::{Kind, Type};
use crate::value::Value;

/// Extra information carried by struct, function and environment natures.
#[derive(Clone, Debug, Default)]
pub struct TypeData {
    /// The natures of the environment's variables.
    pub fields: Option<Rc<HashMap<Vec<u8>, Nature>>>,
    /// The builtin an identifier refers to.
    pub func: Option<usize>,
    pub method_index: usize,
}

/// What is known about a value.
#[derive(Clone, Debug, Default)]
pub struct Nature {
    /// The type; `None` when unknown.
    pub ty: Option<Type>,
    pub kind: Kind,
    pub type_data: Option<Rc<TypeData>>,
    /// The element nature of an array the language built.
    pub elem_ref: Option<Box<Nature>>,
    /// The value is the nil literal.
    pub nil: bool,
    /// Unknown fields are errors (the environment map).
    pub strict: bool,
    /// The nature of a method; its type has the receiver as first input.
    pub method: bool,
    pub is_integer: bool,
    pub is_float: bool,
}

/// The categories `MaybeCompatible` and `IsAnyOf` test for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Bool,
    String,
    Integer,
    Number,
    Map,
    Array,
    Time,
    Duration,
}

impl Nature {
    /// The nature of a value of type `t`.
    pub fn from_type(t: &Type) -> Nature {
        let kind = t.kind();
        let type_data = matches!(kind, Kind::Struct | Kind::Func).then(|| Rc::new(TypeData::default()));
        Nature {
            ty: Some(t.clone()),
            kind,
            type_data,
            is_integer: t.is_builtin_integer(),
            is_float: t.is_builtin_float(),
            ..Nature::default()
        }
    }

    /// The nature of a value; nil has the nil nature.
    pub fn of_value(value: &Value) -> Nature {
        match value.type_of() {
            None => Nature::nil(),
            Some(t) => Nature::from_type(&t),
        }
    }

    pub fn nil() -> Nature {
        Nature { nil: true, ..Nature::default() }
    }

    /// The nature of the environment map: strict, with one nature per variable.
    pub fn env(vars: &[(Vec<u8>, Value)]) -> Nature {
        let mut n = Nature::from_type(&Type::string_map());
        let fields = vars.iter().map(|(k, v)| (k.clone(), Nature::of_value(v))).collect();
        n.type_data = Some(Rc::new(TypeData { fields: Some(Rc::new(fields)), ..TypeData::default() }));
        n.strict = true;
        n
    }

    /// An array whose elements have the nature of `t`.
    pub fn array_of_type(t: &Type) -> Nature {
        Nature::from_type(t).make_array_of()
    }

    /// `[]interface {}` with this nature as its element nature.
    pub fn make_array_of(&self) -> Nature {
        let mut nt = Nature::from_type(&Type::any_slice());
        nt.elem_ref = Some(Box::new(self.clone()));
        nt
    }

    pub fn is_any(&self) -> bool {
        matches!(self.ty, Some(Type::Any))
    }

    pub fn is_unknown(&self) -> bool {
        self.ty.is_none() && !self.nil || self.is_any()
    }

    /// The type name, or `unknown`.
    pub fn name(&self) -> String {
        self.ty.as_ref().map_or_else(|| "unknown".to_string(), |t| t.to_string())
    }

    pub fn deref(&self) -> Nature {
        let mut t = match &self.ty {
            Some(t) if self.kind == Kind::Ptr => t.clone(),
            _ => return self.clone(),
        };
        while let Type::Ptr(e) = t {
            t = (*e).clone();
        }
        Nature::from_type(&t)
    }

    pub fn key(&self) -> Nature {
        match (&self.ty, self.kind) {
            (Some(Type::Map(k, _)), Kind::Map) => Nature::from_type(k),
            _ => Nature::default(),
        }
    }

    pub fn elem(&self) -> Nature {
        match self.kind {
            Kind::Ptr | Kind::Map => self.ty.as_ref().and_then(Type::elem).map(Nature::from_type).unwrap_or_default(),
            Kind::Slice | Kind::Array => match &self.elem_ref {
                Some(r) => (**r).clone(),
                None => self.ty.as_ref().and_then(Type::elem).map(Nature::from_type).unwrap_or_default(),
            },
            _ => Nature::default(),
        }
    }

    pub fn assignable_to(&self, to: &Nature) -> bool {
        if self.nil
            && matches!(to.kind, Kind::Ptr | Kind::Interface | Kind::Chan | Kind::Func | Kind::Map | Kind::Slice)
        {
            return true;
        }
        match (&self.ty, &to.ty) {
            (Some(a), Some(b)) if self.kind == to.kind || to.kind == Kind::Interface => a.assignable_to(b),
            _ => false,
        }
    }

    pub fn num_methods(&self) -> usize {
        self.ty.as_ref().map_or(0, Type::num_method)
    }

    /// The nature of the named method, as `reflect.Type.Method` gives it (receiver first).
    pub fn method_by_name(&self, name: &[u8]) -> Option<Nature> {
        let t = self.ty.as_ref()?;
        let name = std::str::from_utf8(name).ok()?;
        let (index, m) = t.method_by_name(name)?;
        let mut nt = Nature::from_type(&t.method_type(m));
        if t.kind() != Kind::Interface {
            nt.method = true;
            nt.type_data = Some(Rc::new(TypeData { method_index: index, ..TypeData::default() }));
        }
        Some(nt)
    }

    fn func(&self) -> Option<&crate::types::FuncType> {
        self.ty.as_ref().and_then(Type::as_func)
    }

    pub fn num_in(&self) -> usize {
        self.func().map_or(0, |f| f.ins.len())
    }

    pub fn num_out(&self) -> usize {
        self.func().map_or(0, |f| f.outs.len())
    }

    pub fn is_variadic(&self) -> bool {
        self.func().is_some_and(|f| f.variadic)
    }

    pub fn input(&self, i: usize) -> Nature {
        self.func().and_then(|f| f.ins.get(i)).map(Nature::from_type).unwrap_or_default()
    }

    /// The element nature of the variadic input `i`.
    pub fn input_elem(&self, i: usize) -> Nature {
        self.input(i).elem()
    }

    pub fn output(&self, i: usize) -> Nature {
        self.func().and_then(|f| f.outs.get(i)).map(Nature::from_type).unwrap_or_default()
    }

    pub fn is_first_arg_unknown(&self) -> bool {
        self.ty.is_some() && self.input(0).is_unknown()
    }

    /// The nature of a variable of the environment, or of a method of the environment's type.
    pub fn get(&self, name: &[u8]) -> Option<Nature> {
        if self.kind == Kind::Map {
            if let Some(td) = &self.type_data {
                return td.fields.as_ref().and_then(|f| f.get(name).cloned());
            }
        }
        self.method_by_name(name)
    }

    /// Whether the environment map holds plain `interface {}` values (the fast path of the compiler).
    pub fn is_fast_map(&self) -> bool {
        matches!(&self.ty, Some(Type::Map(k, v)) if **k == Type::String && **v == Type::Any)
    }

    pub fn is_number(&self) -> bool {
        self.is_integer || self.is_float
    }

    pub fn promote_numeric(&self, rhs: &Nature) -> Nature {
        if self.is_unknown() || rhs.is_unknown() {
            return Nature::default();
        }
        if self.is_float || rhs.is_float {
            Nature::from_type(&Type::Float64)
        } else {
            Nature::from_type(&Type::Int)
        }
    }

    pub fn is_time(&self) -> bool {
        self.ty == Some(Type::time())
    }

    pub fn is_duration(&self) -> bool {
        self.ty == Some(Type::duration())
    }

    pub fn is_bool(&self) -> bool {
        self.kind == Kind::Bool
    }

    pub fn is_string(&self) -> bool {
        self.kind == Kind::String
    }

    pub fn is_byte_slice(&self) -> bool {
        self.ty == Some(Type::bytes())
    }

    pub fn is_array(&self) -> bool {
        matches!(self.kind, Kind::Slice | Kind::Array)
    }

    pub fn is_map(&self) -> bool {
        self.kind == Kind::Map
    }

    pub fn is_struct(&self) -> bool {
        self.kind == Kind::Struct
    }

    pub fn is_func(&self) -> bool {
        self.kind == Kind::Func
    }

    pub fn is_pointer(&self) -> bool {
        self.kind == Kind::Ptr
    }

    pub fn is_any_of(&self, checks: &[Check]) -> bool {
        checks.iter().any(|c| match c {
            Check::Bool => self.is_bool(),
            Check::String => self.is_string(),
            Check::Integer => self.is_integer,
            Check::Number => self.is_number(),
            Check::Map => self.is_map(),
            Check::Array => self.is_array(),
            Check::Time => self.is_time(),
            Check::Duration => self.is_duration(),
        })
    }

    pub fn comparable_to(&self, rhs: &Nature) -> bool {
        self.is_unknown()
            || rhs.is_unknown()
            || self.nil
            || rhs.nil
            || self.is_number() && rhs.is_number()
            || self.is_duration() && rhs.is_duration()
            || self.is_time() && rhs.is_time()
            || self.is_array() && rhs.is_array()
            || self.assignable_to(rhs)
    }

    pub fn maybe_compatible(&self, rhs: &Nature, checks: &[Check]) -> bool {
        let (lu, ru) = (self.is_unknown(), rhs.is_unknown());
        lu && (ru || rhs.is_any_of(checks)) || ru && self.is_any_of(checks)
    }

    pub fn builtin_func(&self) -> Option<usize> {
        self.type_data.as_ref().and_then(|td| td.func)
    }

    pub fn method_index(&self) -> usize {
        self.type_data.as_ref().map_or(0, |td| td.method_index)
    }

    /// The type, `interface {}` when unknown (the original's `Node.Type`).
    pub fn ty_or_any(&self) -> Type {
        self.ty.clone().unwrap_or(Type::Any)
    }
}
