//! The compiler: turns a checked tree into bytecode for the VM.
//!
//! Every instruction remembers the location of the node it was emitted for; a runtime error is reported at the
//! location of the instruction that failed, so the emission order follows the original exactly.

use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{Node, NodeKind};
use crate::builtin::{self, Call};
use crate::conf::Config;
use crate::file::{Error, Location, Source};
use crate::gostd::regexp::{self, Regexp};
use crate::parser::Tree;
use crate::types::{Kind, Type};
use crate::value::Value;
use crate::vm::func_types;

/// The VM instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Push,
    Int,
    Pop,
    Store,
    LoadVar,
    LoadConst,
    LoadFast,
    LoadFunc,
    LoadEnv,
    Fetch,
    Method,
    True,
    False,
    Nil,
    Negate,
    Not,
    Equal,
    EqualInt,
    EqualString,
    Jump,
    JumpIfTrue,
    JumpIfFalse,
    JumpIfNil,
    JumpIfNotNil,
    JumpIfEnd,
    JumpBackward,
    In,
    Less,
    More,
    LessOrEqual,
    MoreOrEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Exponent,
    Range,
    Matches,
    MatchesConst,
    Contains,
    StartsWith,
    EndsWith,
    Slice,
    Call,
    Call0,
    Call1,
    Call2,
    Call3,
    CallN,
    CallSafe,
    CallTyped,
    CallBuiltin1,
    Array,
    Map,
    Len,
    Cast,
    Deref,
    IncrementIndex,
    DecrementIndex,
    IncrementCount,
    GetIndex,
    GetCount,
    GetLen,
    GetAcc,
    SetAcc,
    SetIndex,
    Pointer,
    Throw,
    Create,
    GroupBy,
    SortBy,
    Sort,
    Begin,
    And,
    Or,
    End,
}

/// A constant of a program.
#[derive(Clone, Debug)]
pub enum Constant {
    Value(Value),
    Regexp(Rc<Regexp>),
    /// A method by name and index in its type's method set.
    Method(&'static str, usize),
    /// An error raised by `OpThrow`.
    Error(&'static str),
    /// A builtin called through `OpCallSafe`.
    Safe(&'static str),
}

/// A compiled expression.
#[derive(Debug)]
pub struct Program {
    pub bytecode: Vec<Op>,
    pub arguments: Vec<i64>,
    pub constants: Vec<Constant>,
    pub locations: Vec<Location>,
    pub variables: usize,
    /// The builtins called through `OpCall0`..`OpCallN`.
    pub functions: Vec<&'static str>,
    pub source: Source,
    /// The optimized tree the program was compiled from.
    pub node: Node,
}

const PLACEHOLDER: i64 = 12345;

struct Compiler<'a> {
    config: &'a Config,
    locations: Vec<Location>,
    bytecode: Vec<Op>,
    arguments: Vec<i64>,
    constants: Vec<Constant>,
    variables: usize,
    scopes: Vec<(&'a str, usize)>,
    functions: Vec<&'static str>,
    function_index: HashMap<&'static str, usize>,
    nodes: Vec<&'a Node>,
    chains: Vec<Vec<usize>>,
}

/// Compiles a checked (and optionally optimized) tree.
pub fn compile(tree: Tree, config: &Config) -> Result<Program, Error> {
    let mut c = Compiler {
        config,
        locations: Vec::new(),
        bytecode: Vec::new(),
        arguments: Vec::new(),
        constants: Vec::new(),
        variables: 0,
        scopes: Vec::new(),
        functions: Vec::new(),
        function_index: HashMap::new(),
        nodes: Vec::new(),
        chains: Vec::new(),
    };
    c.compile(&tree.node)?;
    match config.expect {
        Kind::Int => c.emit(Op::Cast, 0),
        Kind::Int64 => c.emit(Op::Cast, 1),
        Kind::Float64 => c.emit(Op::Cast, 2),
        Kind::Bool => c.emit(Op::Cast, 3),
        _ => 0,
    };
    if config.optimize {
        c.thread_jumps();
    }
    Ok(Program {
        bytecode: c.bytecode,
        arguments: c.arguments,
        constants: c.constants,
        locations: c.locations,
        variables: c.variables,
        functions: c.functions,
        source: tree.source,
        node: tree.node,
    })
}

fn kind_of(node: &Node) -> Kind {
    node.ty().kind()
}

impl<'a> Compiler<'a> {
    fn emit_location(&mut self, loc: Location, op: Op, arg: i64) -> usize {
        self.bytecode.push(op);
        self.arguments.push(arg);
        self.locations.push(loc);
        self.bytecode.len()
    }

    fn emit(&mut self, op: Op, arg: i64) -> usize {
        let loc = self.nodes.last().map(|n| n.loc).unwrap_or_default();
        self.emit_location(loc, op, arg)
    }

    fn emit0(&mut self, op: Op) -> usize {
        self.emit(op, 0)
    }

    /// Adds a constant; equal scalar constants share one slot, compared with Go's `==` on `interface {}`
    /// values (so `-0.0` reuses an earlier `0.0`).
    fn add_constant(&mut self, constant: Constant) -> i64 {
        if let Constant::Value(v) = &constant {
            if let Some(i) = self.constants.iter().position(|c| matches!(c, Constant::Value(w) if same_constant(v, w)))
            {
                return i as i64;
            }
        }
        self.constants.push(constant);
        (self.constants.len() - 1) as i64
    }

    fn emit_push(&mut self, value: Value) -> usize {
        let index = self.add_constant(Constant::Value(value));
        self.emit(Op::Push, index)
    }

    fn add_function(&mut self, name: &'static str) -> i64 {
        if let Some(&p) = self.function_index.get(name) {
            return p as i64;
        }
        let p = self.functions.len();
        self.functions.push(name);
        self.function_index.insert(name, p);
        p as i64
    }

    fn emit_function(&mut self, name: &'static str, args: usize) {
        let f = self.add_function(name);
        match args {
            0 => self.emit(Op::Call0, f),
            1 => self.emit(Op::Call1, f),
            2 => self.emit(Op::Call2, f),
            3 => self.emit(Op::Call3, f),
            n => {
                self.emit(Op::LoadFunc, f);
                self.emit(Op::CallN, n as i64)
            }
        };
    }

    fn patch_jump(&mut self, placeholder: usize) {
        let offset = self.bytecode.len() - placeholder;
        self.arguments[placeholder - 1] = offset as i64;
    }

    fn backward_jump(&self, to: usize) -> i64 {
        (self.bytecode.len() + 1 - to) as i64
    }

    fn node_parent(&self) -> Option<&'a Node> {
        if self.nodes.len() > 1 {
            Some(self.nodes[self.nodes.len() - 2])
        } else {
            None
        }
    }

    fn compile(&mut self, node: &'a Node) -> Result<(), Error> {
        self.nodes.push(node);
        let result = self.compile_node(node);
        self.nodes.pop();
        result
    }

    fn compile_node(&mut self, node: &'a Node) -> Result<(), Error> {
        match &node.kind {
            NodeKind::Nil => {
                self.emit0(Op::Nil);
            }
            NodeKind::Identifier(name) => self.identifier(name),
            NodeKind::Integer(i) => self.integer(node, *i),
            NodeKind::Float(f) => {
                if kind_of(node) == Kind::Float32 {
                    self.emit_push(Value::Float32(*f as f32));
                } else {
                    self.emit_push(Value::Float64(*f));
                }
            }
            NodeKind::Bool(b) => {
                self.emit0(if *b { Op::True } else { Op::False });
            }
            NodeKind::String(s) => {
                self.emit_push(Value::string(s));
            }
            NodeKind::Bytes(b) => {
                self.emit_push(Value::slice(Type::Uint8, b.iter().map(|&x| Value::Uint8(x)).collect()));
            }
            NodeKind::Constant(v) => {
                if v.is_nil() {
                    self.emit0(Op::Nil);
                } else {
                    self.emit_push(v.clone());
                }
            }
            NodeKind::Unary { op, node: inner } => {
                self.compile(inner)?;
                self.deref_if_needed(inner);
                match op.as_str() {
                    "!" | "not" => {
                        self.emit0(Op::Not);
                    }
                    "-" => {
                        self.emit0(Op::Negate);
                    }
                    _ => {}
                }
            }
            NodeKind::Binary { op, left, right } => self.binary(op, left, right)?,
            NodeKind::Chain(inner) => {
                self.chains.push(Vec::new());
                self.compile(inner)?;
                for ph in self.chains.last().cloned().unwrap_or_default() {
                    self.patch_jump(ph);
                }
                let parent_is_coalesce =
                    matches!(self.node_parent().map(|p| &p.kind), Some(NodeKind::Binary { op, .. }) if op == "??");
                if !parent_is_coalesce {
                    let j = self.emit(Op::JumpIfNotNil, PLACEHOLDER);
                    self.emit0(Op::Pop);
                    self.emit0(Op::Nil);
                    self.patch_jump(j);
                }
                self.chains.pop();
            }
            NodeKind::Member { node: base, property, optional, .. } => {
                if let Some((name, index)) = method_index(node) {
                    self.compile(base)?;
                    let c = self.add_constant(Constant::Method(name, index));
                    self.emit(Op::Method, c);
                    return Ok(());
                }
                self.compile(base)?;
                if *optional && !self.chains.is_empty() {
                    let ph = self.emit(Op::JumpIfNil, PLACEHOLDER);
                    if let Some(chain) = self.chains.last_mut() {
                        chain.push(ph);
                    }
                }
                self.compile(property)?;
                let mut deref = true;
                if let Some(Type::Map(key, _)) = &base.nature.ty {
                    if property.nature.ty.as_ref().is_some_and(|p| p.assignable_to(key)) {
                        deref = false;
                    }
                }
                if deref {
                    self.deref_if_needed(property);
                }
                self.emit0(Op::Fetch);
            }
            NodeKind::Slice { node: base, from, to } => {
                self.compile(base)?;
                match to {
                    Some(to) => {
                        self.compile(to)?;
                        self.deref_if_needed(to);
                    }
                    None => {
                        self.emit0(Op::Len);
                    }
                }
                match from {
                    Some(from) => {
                        self.compile(from)?;
                        self.deref_if_needed(from);
                    }
                    None => {
                        self.emit_push(Value::Int(0));
                    }
                }
                self.emit0(Op::Slice);
            }
            NodeKind::Call { callee, args } => self.call(callee, args)?,
            NodeKind::Builtin { name, args, throws, map, threshold } => {
                self.builtin(name, args, *throws, map.as_deref(), *threshold)?
            }
            NodeKind::Predicate(inner) => self.compile(inner)?,
            NodeKind::Pointer(name) => {
                match name.as_str() {
                    "index" => self.emit0(Op::GetIndex),
                    "acc" => self.emit0(Op::GetAcc),
                    _ => self.emit0(Op::Pointer),
                };
            }
            NodeKind::VariableDeclarator { name, value, expr } => {
                self.compile(value)?;
                let index = self.variables;
                self.variables += 1;
                self.emit(Op::Store, index as i64);
                self.scopes.push((name.as_str(), index));
                self.compile(expr)?;
                self.scopes.pop();
            }
            NodeKind::Sequence(nodes) => {
                for (i, n) in nodes.iter().enumerate() {
                    self.compile(n)?;
                    if i < nodes.len() - 1 {
                        self.emit0(Op::Pop);
                    }
                }
            }
            NodeKind::Conditional { cond, exp1, exp2, .. } => {
                self.compile(cond)?;
                self.deref_if_needed(cond);
                let otherwise = self.emit(Op::JumpIfFalse, PLACEHOLDER);
                self.emit0(Op::Pop);
                self.compile(exp1)?;
                let end = self.emit(Op::Jump, PLACEHOLDER);
                self.patch_jump(otherwise);
                self.emit0(Op::Pop);
                self.compile(exp2)?;
                self.patch_jump(end);
            }
            NodeKind::Array(nodes) => {
                for n in nodes {
                    self.compile(n)?;
                }
                self.emit_push(Value::Int(nodes.len() as i64));
                self.emit0(Op::Array);
            }
            NodeKind::Map(pairs) => {
                for pair in pairs {
                    self.compile(pair)?;
                }
                self.emit_push(Value::Int(pairs.len() as i64));
                self.emit0(Op::Map);
            }
            NodeKind::Pair { key, value } => {
                self.compile(key)?;
                self.compile(value)?;
            }
        }
        Ok(())
    }

    fn identifier(&mut self, name: &str) {
        if let Some(&(_, index)) = self.scopes.iter().rev().find(|(n, _)| *n == name) {
            self.emit(Op::LoadVar, index as i64);
            return;
        }
        if name == "$env" {
            self.emit0(Op::LoadEnv);
            return;
        }
        let c = self.add_constant(Constant::Value(Value::string(name)));
        if self.config.env.is_fast_map() {
            self.emit(Op::LoadFast, c);
        } else {
            self.emit(Op::LoadConst, c);
        }
    }

    fn integer(&mut self, node: &Node, i: i64) {
        let v = match kind_of(node) {
            Kind::Float32 => Value::Float32(i as f32),
            Kind::Float64 => Value::Float64(i as f64),
            Kind::Int8 => Value::Int8(i as i8),
            Kind::Int16 => Value::Int16(i as i16),
            Kind::Int32 => Value::Int32(i as i32),
            Kind::Int64 => Value::Int64(i),
            Kind::Uint => Value::Uint(i as u64),
            Kind::Uint8 => Value::Uint8(i as u8),
            Kind::Uint16 => Value::Uint16(i as u16),
            Kind::Uint32 => Value::Uint32(i as u32),
            Kind::Uint64 => Value::Uint64(i as u64),
            _ => Value::Int(i),
        };
        self.emit_push(v);
    }

    fn operands(&mut self, left: &'a Node, right: &'a Node) -> Result<(), Error> {
        self.compile(left)?;
        self.deref_if_needed(left);
        self.compile(right)?;
        self.deref_if_needed(right);
        Ok(())
    }

    fn binary(&mut self, op: &str, left: &'a Node, right: &'a Node) -> Result<(), Error> {
        let simple_op = match op {
            "<" => Some(Op::Less),
            ">" => Some(Op::More),
            "<=" => Some(Op::LessOrEqual),
            ">=" => Some(Op::MoreOrEqual),
            "+" => Some(Op::Add),
            "-" => Some(Op::Subtract),
            "*" => Some(Op::Multiply),
            "/" => Some(Op::Divide),
            "%" => Some(Op::Modulo),
            "**" | "^" => Some(Op::Exponent),
            "in" => Some(Op::In),
            "contains" => Some(Op::Contains),
            "startsWith" => Some(Op::StartsWith),
            "endsWith" => Some(Op::EndsWith),
            ".." => Some(Op::Range),
            _ => None,
        };
        if let Some(code) = simple_op {
            self.operands(left, right)?;
            self.emit0(code);
            return Ok(());
        }
        match op {
            "==" | "!=" => {
                let (l, r) = (kind_of(left), kind_of(right));
                let simple = left.ty().pkg_path().is_empty() && right.ty().pkg_path().is_empty();
                self.operands(left, right)?;
                if l == r && l == Kind::Int && simple {
                    self.emit0(Op::EqualInt);
                } else if l == r && l == Kind::String && simple {
                    self.emit0(Op::EqualString);
                } else {
                    self.emit0(Op::Equal);
                }
                if op == "!=" {
                    self.emit0(Op::Not);
                }
            }
            "or" | "||" | "and" | "&&" => {
                let is_or = op == "or" || op == "||";
                if !self.config.short_circuit {
                    self.operands(left, right)?;
                    self.emit0(if is_or { Op::Or } else { Op::And });
                    return Ok(());
                }
                self.compile(left)?;
                self.deref_if_needed(left);
                let end = self.emit(if is_or { Op::JumpIfTrue } else { Op::JumpIfFalse }, PLACEHOLDER);
                self.emit0(Op::Pop);
                self.compile(right)?;
                self.deref_if_needed(right);
                self.patch_jump(end);
            }
            "matches" => {
                if let Some(pattern) = right.as_string() {
                    let re = regexp::compile(pattern).map_err(Error::plain)?;
                    self.compile(left)?;
                    self.deref_if_needed(left);
                    let c = self.add_constant(Constant::Regexp(Rc::new(re)));
                    self.emit(Op::MatchesConst, c);
                } else {
                    self.operands(left, right)?;
                    self.emit0(Op::Matches);
                }
            }
            "??" => {
                self.compile(left)?;
                self.deref_if_needed(left);
                let end = self.emit(Op::JumpIfNotNil, PLACEHOLDER);
                self.emit0(Op::Pop);
                self.compile(right)?;
                self.deref_if_needed(right);
                self.patch_jump(end);
            }
            _ => {}
        }
        Ok(())
    }

    fn call(&mut self, callee: &'a Node, args: &'a [Node]) -> Result<(), Error> {
        let fn_type = callee.ty();
        if let Some(func) = fn_type.as_func() {
            let mut offset = 0;
            let mut num_in = func.ins.len();
            if let NodeKind::Member { node: base, property, .. } = &callee.kind {
                if let Some(name) = property.as_string() {
                    let bt = base.ty();
                    let has = std::str::from_utf8(name).ok().and_then(|n| bt.method_by_name(n)).is_some();
                    if has && bt.kind() != Kind::Interface {
                        offset = 1;
                        num_in -= 1;
                    }
                }
            }
            for (i, arg) in args.iter().enumerate() {
                self.compile(arg)?;
                let input = if func.variadic && i + 1 >= num_in {
                    func.ins.last().and_then(Type::elem).cloned().unwrap_or(Type::Any)
                } else {
                    func.ins.get(i + offset).cloned().unwrap_or(Type::Any)
                };
                self.deref_param(&input, arg);
            }
        } else {
            for arg in args {
                self.compile(arg)?;
            }
        }
        self.compile(callee)?;
        let is_method = method_index(callee).is_some();
        match typed_func_index(&fn_type, is_method) {
            Some(index) => self.emit(Op::CallTyped, index as i64),
            None => self.emit(Op::Call, args.len() as i64),
        };
        Ok(())
    }

    fn begin_collection(&mut self, args: &'a [Node]) -> Result<(), Error> {
        self.compile(&args[0])?;
        self.deref_if_needed(&args[0]);
        self.emit0(Op::Begin);
        Ok(())
    }

    fn builtin(
        &mut self,
        name: &str,
        args: &'a [Node],
        throws: bool,
        map: Option<&'a Node>,
        threshold: Option<i64>,
    ) -> Result<(), Error> {
        match name {
            "all" | "none" | "any" => {
                self.begin_collection(args)?;
                let mut loop_break = 0;
                self.emit_loop(|c| {
                    c.compile(&args[1])?;
                    if name == "none" {
                        c.emit0(Op::Not);
                    }
                    loop_break = c.emit(if name == "any" { Op::JumpIfTrue } else { Op::JumpIfFalse }, PLACEHOLDER);
                    c.emit0(Op::Pop);
                    Ok(())
                })?;
                self.emit0(if name == "any" { Op::False } else { Op::True });
                self.patch_jump(loop_break);
                self.emit0(Op::End);
            }
            "one" => {
                self.begin_collection(args)?;
                self.emit_loop(|c| {
                    c.compile(&args[1])?;
                    c.emit_cond(|c| {
                        c.emit0(Op::IncrementCount);
                        Ok(())
                    })
                })?;
                self.emit0(Op::GetCount);
                self.emit_push(Value::Int(1));
                self.emit0(Op::Equal);
                self.emit0(Op::End);
            }
            "filter" => {
                self.begin_collection(args)?;
                self.emit_loop(|c| {
                    c.compile(&args[1])?;
                    c.emit_cond(|c| {
                        c.emit0(Op::IncrementCount);
                        match map {
                            Some(m) => c.compile(m)?,
                            None => {
                                c.emit0(Op::Pointer);
                            }
                        }
                        Ok(())
                    })
                })?;
                self.emit0(Op::GetCount);
                self.emit0(Op::End);
                self.emit0(Op::Array);
            }
            "map" => {
                self.begin_collection(args)?;
                self.emit_loop(|c| c.compile(&args[1]))?;
                self.emit0(Op::GetLen);
                self.emit0(Op::End);
                self.emit0(Op::Array);
            }
            "count" => {
                self.begin_collection(args)?;
                let mut loop_break = 0;
                self.emit_loop(|c| {
                    if args.len() == 2 {
                        c.compile(&args[1])?;
                    } else {
                        c.emit0(Op::Pointer);
                    }
                    c.emit_cond(|c| {
                        c.emit0(Op::IncrementCount);
                        if let Some(t) = threshold {
                            c.emit0(Op::GetCount);
                            c.emit(Op::Int, t);
                            c.emit0(Op::MoreOrEqual);
                            loop_break = c.emit(Op::JumpIfTrue, PLACEHOLDER);
                            c.emit0(Op::Pop);
                        }
                        Ok(())
                    })
                })?;
                self.emit0(Op::GetCount);
                if threshold.is_some() {
                    let end = self.emit(Op::Jump, PLACEHOLDER);
                    self.patch_jump(loop_break);
                    self.emit0(Op::Pop);
                    self.emit0(Op::GetCount);
                    self.patch_jump(end);
                }
                self.emit0(Op::End);
            }
            "sum" => {
                self.begin_collection(args)?;
                self.emit(Op::Int, 0);
                self.emit0(Op::SetAcc);
                self.emit_loop(|c| {
                    if args.len() == 2 {
                        c.compile(&args[1])?;
                    } else {
                        c.emit0(Op::Pointer);
                    }
                    c.emit0(Op::GetAcc);
                    c.emit0(Op::Add);
                    c.emit0(Op::SetAcc);
                    Ok(())
                })?;
                self.emit0(Op::GetAcc);
                self.emit0(Op::End);
            }
            "find" | "findLast" | "findIndex" | "findLastIndex" => {
                self.begin_collection(args)?;
                let index = name.ends_with("Index");
                let mut loop_break = 0;
                let body = |c: &mut Self| -> Result<(), Error> {
                    c.compile(&args[1])?;
                    let noop = c.emit(Op::JumpIfFalse, PLACEHOLDER);
                    c.emit0(Op::Pop);
                    if index {
                        c.emit0(Op::GetIndex);
                    } else {
                        match map {
                            Some(m) => c.compile(m)?,
                            None => {
                                c.emit0(Op::Pointer);
                            }
                        }
                    }
                    loop_break = c.emit(Op::Jump, PLACEHOLDER);
                    c.patch_jump(noop);
                    c.emit0(Op::Pop);
                    Ok(())
                };
                if name.starts_with("findLast") {
                    self.emit_loop_backwards(body)?;
                } else {
                    self.emit_loop(body)?;
                }
                if throws && !index {
                    let c = self.add_constant(Constant::Error("reflect: slice index out of range"));
                    self.emit(Op::Push, c);
                    self.emit0(Op::Throw);
                } else {
                    self.emit0(Op::Nil);
                }
                self.patch_jump(loop_break);
                self.emit0(Op::End);
            }
            "groupBy" => {
                self.begin_collection(args)?;
                self.emit(Op::Create, 1);
                self.emit0(Op::SetAcc);
                self.emit_loop(|c| {
                    c.compile(&args[1])?;
                    c.emit0(Op::GroupBy);
                    Ok(())
                })?;
                self.emit0(Op::GetAcc);
                self.emit0(Op::End);
            }
            "sortBy" => {
                self.begin_collection(args)?;
                if args.len() == 3 {
                    self.compile(&args[2])?;
                } else {
                    self.emit_push(Value::string("asc"));
                }
                self.emit(Op::Create, 2);
                self.emit0(Op::SetAcc);
                self.emit_loop(|c| {
                    c.compile(&args[1])?;
                    c.emit0(Op::SortBy);
                    Ok(())
                })?;
                self.emit0(Op::Sort);
                self.emit0(Op::End);
            }
            "reduce" => {
                self.begin_collection(args)?;
                if args.len() == 3 {
                    self.compile(&args[2])?;
                    self.deref_if_needed(&args[2]);
                    self.emit0(Op::SetAcc);
                } else {
                    let empty = self.emit(Op::JumpIfEnd, PLACEHOLDER);
                    self.emit0(Op::Pointer);
                    self.emit0(Op::IncrementIndex);
                    self.emit0(Op::SetAcc);
                    let past = self.emit(Op::Jump, PLACEHOLDER);
                    self.patch_jump(empty);
                    let c = self.add_constant(Constant::Error("reduce of empty array with no initial value"));
                    self.emit(Op::Push, c);
                    self.emit0(Op::Throw);
                    self.patch_jump(past);
                }
                self.emit_loop(|c| {
                    c.compile(&args[1])?;
                    c.emit0(Op::SetAcc);
                    Ok(())
                })?;
                self.emit0(Op::GetAcc);
                self.emit0(Op::End);
            }
            _ => {
                let Some(id) = builtin::index(name) else { return Ok(()) };
                let static_name = builtin::NAMES[id];
                for (i, arg) in args.iter().enumerate() {
                    self.compile(arg)?;
                    let arg_type = arg.ty();
                    if arg_type.kind() == Kind::Ptr || arg.nature.is_unknown() {
                        let deref = builtin::deref_arg(static_name, &arg_type).unwrap_or(true);
                        let _ = i;
                        if deref {
                            self.emit0(Op::Deref);
                        }
                    }
                }
                match builtin::call_kind(static_name) {
                    Call::Fast => {
                        self.emit(Op::CallBuiltin1, id as i64);
                    }
                    Call::Safe => {
                        let c = self.add_constant(Constant::Safe(static_name));
                        self.emit(Op::Push, c);
                        self.emit(Op::CallSafe, args.len() as i64);
                    }
                    Call::Func | Call::Predicate => self.emit_function(static_name, args.len()),
                }
            }
        }
        Ok(())
    }

    fn emit_cond(&mut self, body: impl FnOnce(&mut Self) -> Result<(), Error>) -> Result<(), Error> {
        let noop = self.emit(Op::JumpIfFalse, PLACEHOLDER);
        self.emit0(Op::Pop);
        body(self)?;
        let jmp = self.emit(Op::Jump, PLACEHOLDER);
        self.patch_jump(noop);
        self.emit0(Op::Pop);
        self.patch_jump(jmp);
        Ok(())
    }

    fn emit_loop(&mut self, body: impl FnOnce(&mut Self) -> Result<(), Error>) -> Result<(), Error> {
        let begin = self.bytecode.len();
        let end = self.emit(Op::JumpIfEnd, PLACEHOLDER);
        body(self)?;
        self.emit0(Op::IncrementIndex);
        let back = self.backward_jump(begin);
        self.emit(Op::JumpBackward, back);
        self.patch_jump(end);
        Ok(())
    }

    fn emit_loop_backwards(&mut self, body: impl FnOnce(&mut Self) -> Result<(), Error>) -> Result<(), Error> {
        self.emit0(Op::GetLen);
        self.emit(Op::Int, 1);
        self.emit0(Op::Subtract);
        self.emit0(Op::SetIndex);
        let begin = self.bytecode.len();
        self.emit0(Op::GetIndex);
        self.emit(Op::Int, 0);
        self.emit0(Op::MoreOrEqual);
        let end = self.emit(Op::JumpIfFalse, PLACEHOLDER);
        body(self)?;
        self.emit0(Op::DecrementIndex);
        let back = self.backward_jump(begin);
        self.emit(Op::JumpBackward, back);
        self.patch_jump(end);
        Ok(())
    }

    fn deref_if_needed(&mut self, node: &Node) {
        if node.nature.nil {
            return;
        }
        if matches!(kind_of(node), Kind::Ptr | Kind::Interface) {
            self.emit0(Op::Deref);
        }
    }

    fn deref_param(&mut self, input: &Type, param: &Node) {
        if param.nature.nil {
            return;
        }
        let t = param.ty();
        if t.assignable_to(input) {
            return;
        }
        if input.kind() != Kind::Ptr && t.kind() == Kind::Ptr {
            self.emit0(Op::Deref);
        }
    }

    /// Lets a conditional jump that lands on the same kind of jump go straight to that jump's target.
    fn thread_jumps(&mut self) {
        for i in 0..self.bytecode.len() {
            let op = self.bytecode[i];
            if matches!(op, Op::JumpIfTrue | Op::JumpIfFalse | Op::JumpIfNil | Op::JumpIfNotNil) {
                let mut target = i as i64 + self.arguments[i] + 1;
                while (target as usize) < self.bytecode.len() && self.bytecode[target as usize] == op {
                    target += self.arguments[target as usize] + 1;
                }
                self.arguments[i] = target - i as i64 - 1;
            }
        }
    }
}

/// Whether two scalar constants are the same map key in the original's constant index.
fn same_constant(a: &Value, b: &Value) -> bool {
    if matches!(a.kind(), Kind::Slice | Kind::Map | Kind::Struct | Kind::Func | Kind::Array)
        || a.type_of() != b.type_of()
    {
        return false;
    }
    match (a, b) {
        (Value::Float64(x), Value::Float64(y)) => x == y,
        (Value::Float32(x), Value::Float32(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        _ => a.as_i64().is_some() && a.as_i64() == b.as_i64(),
    }
}

/// The method a member node selects on a value of known (non-interface) type.
fn method_index(node: &Node) -> Option<(&'static str, usize)> {
    let NodeKind::Member { node: base, property, .. } = &node.kind else { return None };
    let name = std::str::from_utf8(property.as_string()?).ok()?;
    let base_type = base.nature.ty.clone().unwrap_or(Type::Any);
    if base_type.kind() == Kind::Interface {
        return None;
    }
    base_type.method_by_name(name).map(|(index, m)| (m.name, index))
}

/// The typed-call index of a function type, the receiver left out for methods.
fn typed_func_index(fn_type: &Type, method: bool) -> Option<usize> {
    let func = fn_type.as_func()?;
    if func.variadic {
        return None;
    }
    let ins = if method { func.ins.get(1..).unwrap_or(&[]) } else { &func.ins[..] };
    let signature = Type::func(ins.to_vec(), func.outs.clone(), false).to_string();
    func_types::index(&signature)
}
