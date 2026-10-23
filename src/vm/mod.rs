//! The virtual machine: runs a compiled program against an environment.

pub mod call;
pub mod func_types;
pub mod runtime;

use std::rc::Rc;

use crate::builtin::functions;
use crate::compiler::{Constant, Op, Program};
use crate::conf::DEFAULT_MEMORY_BUDGET;
use crate::file::{Error, Location};
use crate::gostd::{regexp, sort, strings};
use crate::types::Type;
use crate::value::{MapValue, Value};
use runtime::{as_bool, as_int, as_str, assertion, Panic, RtResult};

/// The accumulator of a predicate loop.
enum Acc {
    Value(Value),
    GroupBy(MapValue),
    SortBy { desc: bool, array: Vec<Value>, values: Vec<Value> },
}

/// The state of one predicate loop over a collection.
struct Scope {
    collection: Value,
    index: i64,
    len: i64,
    count: i64,
    acc: Acc,
}

impl Scope {
    /// The current element; strings yield their bytes.
    fn item(&self) -> RtResult<Value> {
        let i = self.index as usize;
        match &self.collection {
            Value::String(s) => Ok(Value::Uint8(s[i])),
            Value::Slice(seq) | Value::Array(seq) => Ok(seq.items[i].clone()),
            other => Err(runtime::reflect_kind_error("Index", other)),
        }
    }
}

struct Vm<'p> {
    program: &'p Program,
    stack: Vec<Value>,
    scopes: Vec<Scope>,
    variables: Vec<Value>,
    memory: u64,
    ip: usize,
    pending_acc: Option<Acc>,
}

/// Runs a program; `env` is the environment map, or nil when none was given.
pub fn run(program: &Program, env: &Value) -> Result<Value, Error> {
    let mut vm = Vm {
        program,
        stack: Vec::with_capacity(2),
        scopes: Vec::new(),
        variables: vec![Value::Nil; program.variables],
        memory: 0,
        ip: 0,
        pending_acc: None,
    };
    match vm.execute(env) {
        Ok(v) => Ok(v),
        Err(Panic(message)) => {
            let location = program.locations.get(vm.ip.wrapping_sub(1)).copied().unwrap_or(Location::default());
            Err(Error::new(location, message).bind(&program.source))
        }
    }
}

/// The message Go's sort reports when a `Less` comparison fails.
struct SortData<'a> {
    desc: bool,
    array: &'a mut Vec<Value>,
    values: Option<&'a mut Vec<Value>>,
}

impl sort::Data for SortData<'_> {
    type Error = Panic;

    fn len(&self) -> usize {
        self.array.len()
    }

    fn less(&mut self, i: usize, j: usize) -> Result<bool, Panic> {
        let keys: &Vec<Value> = match &self.values {
            Some(v) => v,
            None => self.array,
        };
        if self.desc {
            runtime::less(&keys[j], &keys[i])
        } else {
            runtime::less(&keys[i], &keys[j])
        }
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.array.swap(i, j);
        if let Some(v) = &mut self.values {
            v.swap(i, j);
        }
    }
}

/// Sorts values like the original's `sort` builtin and `sortBy` (Go's `sort.Sort`).
pub fn sort_values(array: &mut Vec<Value>, values: Option<&mut Vec<Value>>, desc: bool) -> RtResult<()> {
    sort::sort(&mut SortData { desc, array, values })
}

impl Vm<'_> {
    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    fn pop(&mut self) -> RtResult<Value> {
        self.stack.pop().ok_or_else(|| "stack underflow".into())
    }

    fn current(&self) -> RtResult<&Value> {
        self.stack.last().ok_or_else(|| "stack underflow".into())
    }

    fn scope(&mut self) -> RtResult<&mut Scope> {
        self.scopes.last_mut().ok_or_else(|| "runtime error: invalid memory address or nil pointer dereference".into())
    }

    fn mem_grow(&mut self, size: u64) -> RtResult<()> {
        self.memory = self.memory.saturating_add(size);
        if self.memory >= DEFAULT_MEMORY_BUDGET {
            return Err("memory budget exceeded".into());
        }
        Ok(())
    }

    fn pop_args(&mut self, n: usize) -> RtResult<Vec<Value>> {
        if self.stack.len() < n {
            return Err("stack underflow".into());
        }
        Ok(self.stack.split_off(self.stack.len() - n))
    }

    fn constant_name(&self, arg: i64) -> Value {
        match &self.program.constants[arg as usize] {
            Constant::Value(v) => v.clone(),
            _ => Value::Nil,
        }
    }

    fn execute(&mut self, env: &Value) -> RtResult<Value> {
        let program = self.program;
        while self.ip < program.bytecode.len() {
            let op = program.bytecode[self.ip];
            let arg = program.arguments[self.ip];
            self.ip += 1;
            match op {
                Op::Push => match &program.constants[arg as usize] {
                    Constant::Value(v) => self.push(v.clone()),
                    Constant::Error(msg) | Constant::Safe(msg) => self.push(Value::string(msg)),
                    Constant::Regexp(_) | Constant::Method(..) => self.push(Value::Nil),
                },
                Op::Int => self.push(Value::Int(arg)),
                Op::Pop => {
                    self.pop()?;
                }
                Op::Store => {
                    let v = self.pop()?;
                    self.variables[arg as usize] = v;
                }
                Op::LoadVar => self.push(self.variables[arg as usize].clone()),
                Op::LoadConst => {
                    let name = self.constant_name(arg);
                    let v = runtime::fetch(env, &name)?;
                    self.push(v);
                }
                Op::LoadFast => {
                    let name = self.constant_name(arg);
                    let v = match env {
                        Value::Map(m) => m.get(&name).ok().flatten().cloned().unwrap_or(Value::Nil),
                        other => return Err(assertion(other, "map[string]interface {}")),
                    };
                    self.push(v);
                }
                Op::LoadFunc => self.push(Value::string(program.functions[arg as usize])),
                Op::LoadEnv => self.push(env.clone()),
                Op::Fetch => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.push(runtime::fetch(&a, &b)?);
                }
                Op::Method => {
                    let a = self.pop()?;
                    let name = match &program.constants[arg as usize] {
                        Constant::Method(name, _) => *name,
                        _ => "",
                    };
                    self.push(runtime::fetch_method(&a, name)?);
                }
                Op::True => self.push(Value::Bool(true)),
                Op::False => self.push(Value::Bool(false)),
                Op::Nil => self.push(Value::Nil),
                Op::Negate => {
                    let v = self.pop()?;
                    self.push(runtime::negate(&v)?);
                }
                Op::Not => {
                    let v = self.pop()?;
                    self.push(Value::Bool(!as_bool(&v)?));
                }
                Op::Equal => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.push(Value::Bool(runtime::equal(&a, &b)));
                }
                Op::EqualInt => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let x = as_int(&a)?;
                    let y = as_int(&b)?;
                    self.push(Value::Bool(x == y));
                }
                Op::EqualString => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let eq = as_str(&a)? == as_str(&b)?;
                    self.push(Value::Bool(eq));
                }
                Op::Jump => self.jump(arg)?,
                Op::JumpIfTrue => {
                    if as_bool(self.current()?)? {
                        self.jump(arg)?;
                    }
                }
                Op::JumpIfFalse => {
                    if !as_bool(self.current()?)? {
                        self.jump(arg)?;
                    }
                }
                Op::JumpIfNil => {
                    if runtime::is_nil(self.current()?) {
                        self.jump(arg)?;
                    }
                }
                Op::JumpIfNotNil => {
                    if !runtime::is_nil(self.current()?) {
                        self.jump(arg)?;
                    }
                }
                Op::JumpIfEnd => {
                    let scope = self.scope()?;
                    if scope.index >= scope.len {
                        self.jump(arg)?;
                    }
                }
                Op::JumpBackward => self.ip = self.ip.wrapping_sub(arg as usize),
                Op::In => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.push(Value::Bool(runtime::contains_in(&a, &b)?));
                }
                Op::Less | Op::More | Op::LessOrEqual | Op::MoreOrEqual => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let r = match op {
                        Op::Less => runtime::less(&a, &b)?,
                        Op::More => runtime::more(&a, &b)?,
                        Op::LessOrEqual => runtime::less_or_equal(&a, &b)?,
                        _ => runtime::more_or_equal(&a, &b)?,
                    };
                    self.push(Value::Bool(r));
                }
                Op::Add | Op::Subtract | Op::Multiply | Op::Divide | Op::Modulo | Op::Exponent => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let r = match op {
                        Op::Add => runtime::add(&a, &b)?,
                        Op::Subtract => runtime::subtract(&a, &b)?,
                        Op::Multiply => runtime::multiply(&a, &b)?,
                        Op::Divide => runtime::divide(&a, &b)?,
                        Op::Modulo => runtime::modulo(&a, &b)?,
                        _ => runtime::exponent(&a, &b)?,
                    };
                    self.push(r);
                }
                Op::Range => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let min = runtime::to_int(&a)?;
                    let max = runtime::to_int(&b)?;
                    let size = max.wrapping_sub(min).wrapping_add(1).max(0);
                    self.mem_grow(size as u64)?;
                    self.push(runtime::make_range(min, max));
                }
                Op::Matches => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if runtime::is_nil(&a) || runtime::is_nil(&b) {
                        self.push(Value::Bool(false));
                        continue;
                    }
                    let pattern = as_str(&b)?;
                    let subject = match &a {
                        Value::String(s) => s.to_vec(),
                        other => bytes_of(other)?,
                    };
                    let re = regexp::compile(pattern).map_err(Panic::from)?;
                    self.push(Value::Bool(re.is_match(&subject)));
                }
                Op::MatchesConst => {
                    let a = self.pop()?;
                    if runtime::is_nil(&a) {
                        self.push(Value::Bool(false));
                        continue;
                    }
                    let Constant::Regexp(re) = &program.constants[arg as usize] else { continue };
                    let matched = match &a {
                        Value::String(s) => re.is_match(s),
                        other => re.is_match(&bytes_of(other)?),
                    };
                    self.push(Value::Bool(matched));
                }
                Op::Contains | Op::StartsWith | Op::EndsWith => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if runtime::is_nil(&a) || runtime::is_nil(&b) {
                        self.push(Value::Bool(false));
                        continue;
                    }
                    let (s, t) = (as_str(&a)?, as_str(&b)?);
                    let r = match op {
                        Op::Contains => strings::contains(s, t),
                        Op::StartsWith => strings::has_prefix(s, t),
                        _ => strings::has_suffix(s, t),
                    };
                    self.push(Value::Bool(r));
                }
                Op::Slice => {
                    let from = self.pop()?;
                    let to = self.pop()?;
                    let node = self.pop()?;
                    self.push(runtime::slice(&node, &from, &to)?);
                }
                Op::Call => {
                    let f = self.pop()?;
                    let args = self.pop_args(arg as usize)?;
                    let out = call::call_reflect(&f, args)?;
                    self.push(out);
                }
                Op::CallTyped => {
                    let f = self.pop()?;
                    let out = call::call_typed(&f, arg as usize, &mut self.stack)?;
                    self.push(out);
                }
                Op::Call0 | Op::Call1 | Op::Call2 | Op::Call3 => {
                    let n = match op {
                        Op::Call0 => 0,
                        Op::Call1 => 1,
                        Op::Call2 => 2,
                        _ => 3,
                    };
                    let args = self.pop_args(n)?;
                    let out = functions::call(program.functions[arg as usize], &args)?;
                    self.push(out);
                }
                Op::CallN => {
                    let f = self.pop()?;
                    let args = self.pop_args(arg as usize)?;
                    let name = as_str(&f)?.to_vec();
                    let out = functions::call(&String::from_utf8_lossy(&name), &args)?;
                    self.push(out);
                }
                Op::CallSafe => {
                    let f = self.pop()?;
                    let args = self.pop_args(arg as usize)?;
                    let name = as_str(&f)?.to_vec();
                    let (out, mem) = functions::call_safe(&String::from_utf8_lossy(&name), &args)?;
                    self.mem_grow(mem)?;
                    self.push(out);
                }
                Op::CallBuiltin1 => {
                    let v = self.pop()?;
                    self.push(functions::call_fast(crate::builtin::NAMES[arg as usize], &v)?);
                }
                Op::Array => {
                    let size = as_int(&self.pop()?)?;
                    self.mem_grow(size as u64)?;
                    let items = self.pop_args(size as usize)?;
                    self.push(Value::any_slice(items));
                }
                Op::Map => {
                    let size = as_int(&self.pop()?)?;
                    self.mem_grow(size as u64)?;
                    let items = self.pop_args(2 * size as usize)?;
                    let mut m = MapValue::new(Type::string_map());
                    for pair in items.chunks(2).rev() {
                        let key = as_str(&pair[0])?;
                        let _ = m.insert(Value::string(key), pair[1].clone());
                    }
                    self.push(Value::Map(Rc::new(m)));
                }
                Op::Len => {
                    let n = runtime::len(self.current()?)?;
                    self.push(Value::Int(n));
                }
                Op::Cast => {
                    let v = self.pop()?;
                    let r = match arg {
                        0 => Value::Int(runtime::to_int(&v)?),
                        1 => Value::Int64(runtime::to_int64(&v)?),
                        2 => Value::Float64(runtime::to_float64(&v)?),
                        _ => Value::Bool(runtime::to_bool(&v)?),
                    };
                    self.push(r);
                }
                Op::Deref => {
                    let v = self.pop()?;
                    self.push(runtime::deref(v));
                }
                Op::IncrementIndex => self.scope()?.index += 1,
                Op::DecrementIndex => self.scope()?.index -= 1,
                Op::IncrementCount => self.scope()?.count += 1,
                Op::GetIndex => {
                    let i = self.scope()?.index;
                    self.push(Value::Int(i));
                }
                Op::GetCount => {
                    let c = self.scope()?.count;
                    self.push(Value::Int(c));
                }
                Op::GetLen => {
                    let l = self.scope()?.len;
                    self.push(Value::Int(l));
                }
                Op::GetAcc => {
                    let v = match &self.scope()?.acc {
                        Acc::Value(v) => v.clone(),
                        Acc::GroupBy(m) => Value::Map(Rc::new(m.clone())),
                        Acc::SortBy { .. } => Value::Nil,
                    };
                    self.push(v);
                }
                Op::SetAcc => {
                    let v = self.pop()?;
                    let acc = self.pending_acc.take().unwrap_or(Acc::Value(v));
                    self.scope()?.acc = acc;
                }
                Op::SetIndex => {
                    let i = as_int(&self.pop()?)?;
                    self.scope()?.index = i;
                }
                Op::Pointer => {
                    let item = self.scope()?.item()?;
                    self.push(item);
                }
                Op::Throw => {
                    let v = self.pop()?;
                    return Err(Panic(as_str(&v)?.to_vec()));
                }
                Op::Create => {
                    if arg == 1 {
                        self.pending_acc = Some(Acc::GroupBy(MapValue::new(Type::map(Type::Any, Type::any_slice()))));
                    } else {
                        let order = self.pop()?;
                        let desc = match &order {
                            Value::String(s) if &s[..] == b"asc" => false,
                            Value::String(s) if &s[..] == b"desc" => true,
                            Value::String(_) => return Err("unknown order, use asc or desc".into()),
                            _ => return Err("sortBy order argument must be a string".into()),
                        };
                        self.pending_acc = Some(Acc::SortBy { desc, array: Vec::new(), values: Vec::new() });
                    }
                    self.push(Value::Nil);
                }
                Op::GroupBy => {
                    let key = self.pop()?;
                    let scope = self.scope()?;
                    let item = scope.item()?;
                    if let Acc::GroupBy(m) = &mut scope.acc {
                        group_by_append(m, key, item)?;
                    }
                }
                Op::SortBy => {
                    let value = self.pop()?;
                    let scope = self.scope()?;
                    let item = scope.item()?;
                    if let Acc::SortBy { array, values, .. } = &mut scope.acc {
                        array.push(item);
                        values.push(value);
                    }
                }
                Op::Sort => {
                    let scope = self.scope()?;
                    let len = scope.len;
                    let sorted = match &mut scope.acc {
                        Acc::SortBy { desc, array, values } => {
                            sort_values(array, Some(values), *desc)?;
                            std::mem::take(array)
                        }
                        _ => Vec::new(),
                    };
                    self.mem_grow(len as u64)?;
                    self.push(Value::any_slice(sorted));
                }
                Op::Begin => {
                    let a = self.pop()?;
                    let len = match &a {
                        Value::Slice(_) | Value::Array(_) | Value::String(_) | Value::Map(_) => {
                            a.len().unwrap_or_default() as i64
                        }
                        other => return Err(runtime::reflect_kind_error("Len", other)),
                    };
                    self.scopes.push(Scope { collection: a, index: 0, len, count: 0, acc: Acc::Value(Value::Nil) });
                }
                Op::And | Op::Or => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    let x = as_bool(&a)?;
                    let r = if op == Op::And { x && as_bool(&b)? } else { x || as_bool(&b)? };
                    self.push(Value::Bool(r));
                }
                Op::End => {
                    self.scopes.pop();
                }
            }
        }
        Ok(self.stack.pop().unwrap_or(Value::Nil))
    }

    fn jump(&mut self, arg: i64) -> RtResult<()> {
        if arg < 0 {
            return Err("negative jump offset is invalid".into());
        }
        self.ip += arg as usize;
        Ok(())
    }
}

/// The bytes of a `[]uint8` operand of `matches`.
fn bytes_of(v: &Value) -> RtResult<Vec<u8>> {
    match v {
        Value::Slice(seq) if seq.ty == Type::bytes() => {
            Ok(seq.items.iter().map(|b| if let Value::Uint8(x) = b { *x } else { 0 }).collect())
        }
        other => Err(assertion(other, "[]uint8")),
    }
}

/// Appends an element to its group (`groups[key] = append(groups[key], item)`).
fn group_by_append(m: &mut MapValue, key: Value, item: Value) -> RtResult<()> {
    let unhashable = |crate::value::Unhashable(t)| Panic::from(format!("hash of unhashable type: {t}"));
    match m.get_mut(&key).map_err(unhashable)? {
        Some(Value::Slice(seq)) => {
            let seq = Rc::make_mut(seq);
            seq.items.push(item);
            seq.nil = false;
        }
        _ => {
            m.insert(key, Value::any_slice(vec![item])).map_err(unhashable)?;
        }
    }
    Ok(())
}
