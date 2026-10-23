//! The type checker: finds the nature of every node and reports type errors.

pub mod nature;

use crate::ast::{Node, NodeKind};
use crate::builtin;
use crate::conf::Config;
use crate::file::{Error, Location};
use crate::gostd::fmt::sprintf_no_args;
use crate::gostd::regexp;
use crate::parser::{self, Tree};
use crate::types::{Kind, Type};
use crate::value::Value;
use nature::{Check, Nature};

struct PredicateScope {
    collection: Nature,
    vars: Vec<(String, Nature)>,
}

/// The checker state for one pass over a tree.
pub struct Checker<'a> {
    config: &'a Config,
    predicate_scopes: Vec<PredicateScope>,
    var_scopes: Vec<(String, Nature)>,
    err: Option<Error>,
}

/// Parses and checks an expression, applying the timezone patch when it is configured.
pub fn parse_check(input: &[u8], config: &Config) -> Result<Tree, Error> {
    let mut tree = parser::parse(input, Some(config))?;
    if let Some(loc) = &config.timezone {
        // The original runs the checker before and after applying its tree patches.
        let _ = check(&mut tree, config);
        patch_timezone(&mut tree.node, loc);
        let _ = check(&mut tree, config);
    }
    check(&mut tree, config)?;
    Ok(tree)
}

/// Prepends the configured location to every `date()` and `now()` call.
fn patch_timezone(node: &mut Node, loc: &crate::gostd::time::Loc) {
    node.walk(&mut |n| {
        if let NodeKind::Builtin { name, args, .. } = &n.kind {
            if name == "date" || name == "now" {
                let constant = Node::new(NodeKind::Constant(Value::Location(loc.clone())), Location::default());
                let mut new_args = vec![constant];
                new_args.extend(args.iter().cloned());
                n.kind =
                    NodeKind::Builtin { name: name.clone(), args: new_args, throws: false, map: None, threshold: None };
                n.nature = Nature::default();
            }
        }
    });
}

/// Checks a parsed tree and returns the nature of the whole expression.
pub fn check(tree: &mut Tree, config: &Config) -> Result<Nature, Error> {
    let mut v = Checker { config, predicate_scopes: Vec::new(), var_scopes: Vec::new(), err: None };
    let nt = v.visit(&mut tree.node);
    if let Some(err) = v.err {
        return Err(err.bind(&tree.source));
    }
    if config.expect != Kind::Invalid {
        if config.expect_any && nt.is_unknown() {
            return Ok(nt);
        }
        let ok = match config.expect {
            Kind::Int | Kind::Int64 | Kind::Float64 => nt.is_number(),
            k => nt.kind == k,
        };
        if !ok {
            return Err(Error::plain(format!("expected {}, but got {}", config.expect, nt.name())));
        }
    }
    Ok(nt)
}

fn bool_nature() -> Nature {
    Nature::from_type(&Type::Bool)
}

fn int_nature() -> Nature {
    Nature::from_type(&Type::Int)
}

fn float_nature() -> Nature {
    Nature::from_type(&Type::Float64)
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl Checker<'_> {
    fn error_at(&mut self, loc: Location, message: impl Into<Vec<u8>>) -> Nature {
        if self.err.is_none() {
            self.err = Some(Error::new(loc, message));
        }
        Nature::default()
    }

    fn visit(&mut self, node: &mut Node) -> Nature {
        let nt = match &node.kind {
            NodeKind::Nil => Nature::nil(),
            NodeKind::Identifier(_) => self.identifier_node(node),
            NodeKind::Integer(_) => int_nature(),
            NodeKind::Float(_) => float_nature(),
            NodeKind::Bool(_) => bool_nature(),
            NodeKind::String(_) => Nature::from_type(&Type::String),
            NodeKind::Bytes(_) => Nature::from_type(&Type::bytes()),
            NodeKind::Constant(v) => Nature::of_value(v),
            NodeKind::Unary { .. } => self.unary_node(node),
            NodeKind::Binary { .. } => self.binary_node(node),
            NodeKind::Chain(_) => match &mut node.kind {
                NodeKind::Chain(inner) => self.visit(inner),
                _ => Nature::default(),
            },
            NodeKind::Member { .. } => self.member_node(node),
            NodeKind::Slice { .. } => self.slice_node(node),
            NodeKind::Call { .. } => self.call_node(node),
            NodeKind::Builtin { .. } => self.builtin_node(node),
            NodeKind::Predicate(_) => self.predicate_node(node),
            NodeKind::Pointer(_) => self.pointer_node(node),
            NodeKind::VariableDeclarator { .. } => self.variable_declarator_node(node),
            NodeKind::Sequence(_) => self.sequence_node(node),
            NodeKind::Conditional { .. } => self.conditional_node(node),
            NodeKind::Array(_) => self.array_node(node),
            NodeKind::Map(_) => self.map_node(node),
            NodeKind::Pair { .. } => self.pair_node(node),
        };
        node.nature = nt.clone();
        nt
    }

    fn identifier_node(&mut self, node: &Node) -> Nature {
        let NodeKind::Identifier(name) = &node.kind else { return Nature::default() };
        if let Some((_, nt)) = self.var_scopes.iter().rev().find(|(n, _)| n == name) {
            return nt.clone();
        }
        if name == "$env" {
            return Nature::default();
        }
        let strict = self.config.strict;
        self.ident(node.loc, name.as_bytes(), strict, true)
    }

    fn ident(&mut self, loc: Location, name: &[u8], strict: bool, builtins: bool) -> Nature {
        if let Some(nt) = self.config.env.get(name) {
            return nt;
        }
        if builtins {
            if let Some(id) =
                std::str::from_utf8(name).ok().filter(|n| self.config.builtins.contains(n)).and_then(builtin::index)
            {
                let mut nt = Nature::from_type(&builtin::func_type(builtin::NAMES[id]));
                let mut td = nt.type_data.as_deref().cloned().unwrap_or_default();
                td.func = Some(id);
                nt.type_data = Some(std::rc::Rc::new(td));
                return nt;
            }
        }
        if self.config.strict && strict {
            return self.error_at(loc, format!("unknown name {}", lossy(name)));
        }
        Nature::default()
    }

    fn unary_node(&mut self, node: &mut Node) -> Nature {
        let loc = node.loc;
        let NodeKind::Unary { op, node: inner } = &mut node.kind else { return Nature::default() };
        let nt = self.visit(inner).deref();
        match op.as_str() {
            "!" | "not" => {
                if nt.is_bool() || nt.is_unknown() {
                    return bool_nature();
                }
            }
            "+" | "-" => {
                if nt.is_number() {
                    return nt;
                }
                if nt.is_unknown() {
                    return Nature::default();
                }
            }
            other => return self.error_at(loc, format!("unknown operator ({other})")),
        }
        let msg = format!("invalid operation: {op} (mismatched type {})", nt.name());
        self.error_at(loc, msg)
    }

    fn binary_node(&mut self, node: &mut Node) -> Nature {
        let loc = node.loc;
        let NodeKind::Binary { op, left, right } = &mut node.kind else { return Nature::default() };
        let l = self.visit(left).deref();
        let r = self.visit(right).deref();
        use Check::*;
        match op.as_str() {
            "==" | "!=" => {
                if l.comparable_to(&r) {
                    return bool_nature();
                }
            }
            "or" | "||" | "and" | "&&" => {
                if l.is_bool() && r.is_bool() || l.maybe_compatible(&r, &[Bool]) {
                    return bool_nature();
                }
            }
            "<" | ">" | ">=" | "<=" => {
                if l.is_number() && r.is_number()
                    || l.is_string() && r.is_string()
                    || l.is_time() && r.is_time()
                    || l.is_duration() && r.is_duration()
                    || l.maybe_compatible(&r, &[Number, String, Time, Duration])
                {
                    return bool_nature();
                }
            }
            "-" => {
                if l.is_number() && r.is_number() {
                    return l.promote_numeric(&r);
                }
                if l.is_time() && r.is_time() {
                    return Nature::from_type(&Type::duration());
                }
                if l.is_time() && r.is_duration() {
                    return Nature::from_type(&Type::time());
                }
                if l.is_duration() && r.is_duration() {
                    return Nature::from_type(&Type::duration());
                }
                if l.maybe_compatible(&r, &[Number, Time, Duration]) {
                    return Nature::default();
                }
            }
            "*" => {
                if l.is_number() && r.is_number() {
                    return l.promote_numeric(&r);
                }
                if l.is_number() && r.is_duration()
                    || l.is_duration() && r.is_number()
                    || l.is_duration() && r.is_duration()
                {
                    return Nature::from_type(&Type::duration());
                }
                if l.maybe_compatible(&r, &[Number, Duration]) {
                    return Nature::default();
                }
            }
            "/" | "**" | "^" => {
                if l.is_number() && r.is_number() || l.maybe_compatible(&r, &[Number]) {
                    return float_nature();
                }
            }
            "%" => {
                if l.is_integer && r.is_integer || l.maybe_compatible(&r, &[Integer]) {
                    return int_nature();
                }
            }
            "+" => {
                if l.is_number() && r.is_number() {
                    return l.promote_numeric(&r);
                }
                if l.is_string() && r.is_string() {
                    return Nature::from_type(&Type::String);
                }
                if l.is_time() && r.is_duration() || l.is_duration() && r.is_time() {
                    return Nature::from_type(&Type::time());
                }
                if l.is_duration() && r.is_duration() {
                    return Nature::from_type(&Type::duration());
                }
                if l.maybe_compatible(&r, &[Number, String, Time, Duration]) {
                    return Nature::default();
                }
            }
            "in" => {
                if (l.is_string() || l.is_unknown()) && r.is_struct() {
                    return bool_nature();
                }
                if r.is_map() {
                    let key = r.key();
                    if !l.is_unknown() && !l.assignable_to(&key) {
                        let msg = format!("cannot use {} as type {} in map key", l.name(), key.name());
                        return self.error_at(loc, msg);
                    }
                    return bool_nature();
                }
                if r.is_array() {
                    let elem = r.elem();
                    if !l.comparable_to(&elem) {
                        let msg = format!("cannot use {} as type {} in array", l.name(), elem.name());
                        return self.error_at(loc, msg);
                    }
                    return bool_nature();
                }
                if l.is_unknown() && r.is_any_of(&[String, Array, Map]) || r.is_unknown() {
                    return bool_nature();
                }
            }
            "matches" => {
                if let Some(pattern) = right.as_string() {
                    if let Err(e) = regexp::compile(pattern) {
                        return self.error_at(loc, sprintf_no_args(e.as_bytes()));
                    }
                }
                if (l.is_string() || l.is_byte_slice()) && r.is_string() || l.maybe_compatible(&r, &[String]) {
                    return bool_nature();
                }
            }
            "contains" | "startsWith" | "endsWith" => {
                if l.is_string() && r.is_string() || l.maybe_compatible(&r, &[String]) {
                    return bool_nature();
                }
            }
            ".." => {
                if l.is_integer && r.is_integer || l.maybe_compatible(&r, &[Integer]) {
                    return Nature::array_of_type(&Type::Int);
                }
            }
            "??" => {
                if l.nil && !r.nil {
                    return r;
                }
                if !l.nil && r.nil {
                    return l;
                }
                if l.nil && r.nil {
                    return Nature::nil();
                }
                if r.assignable_to(&l) {
                    return l;
                }
                return Nature::default();
            }
            other => return self.error_at(loc, format!("unknown operator ({other})")),
        }
        let msg = format!("invalid operation: {op} (mismatched types {} and {})", l.name(), r.name());
        self.error_at(loc, msg)
    }

    fn member_node(&mut self, node: &mut Node) -> Nature {
        let loc = node.loc;
        let NodeKind::Member { node: base_node, property, optional, method } = &mut node.kind else {
            return Nature::default();
        };
        if let NodeKind::Identifier(id) = &base_node.kind {
            if id == "$env" {
                if let Some(name) = property.as_string().map(<[u8]>::to_vec) {
                    let strict = self.config.strict && !*optional;
                    return self.ident(loc, &name, strict, false);
                }
                return Nature::default();
            }
        }

        let base = self.visit(base_node);
        let mut prop = self.visit(property);
        if base.is_unknown() {
            return Nature::default();
        }
        if let Some(name) = property.as_string() {
            if base.nil {
                return self.error_at(loc, format!("type nil has no field {}", lossy(name)));
            }
            if let Some(m) = base.method_by_name(name) {
                return m;
            }
        }
        let base = base.deref();
        match base.kind {
            Kind::Map => {
                let key = base.key();
                if !prop.assignable_to(&key) {
                    let deref = prop.deref();
                    if deref.assignable_to(&key) {
                        prop = deref;
                    }
                }
                if !prop.assignable_to(&key) && !prop.is_unknown() {
                    let msg = format!("cannot use {} to get an element from {}", prop.name(), base.name());
                    return self.error_at(property.loc, msg);
                }
                if let (Some(name), Some(td)) = (property.as_string(), &base.type_data) {
                    match td.fields.as_ref().and_then(|f| f.get(name)) {
                        Some(field) => return field.clone(),
                        None if base.strict => {
                            let msg = format!("unknown field {}", lossy(name));
                            return self.error_at(property.loc, msg);
                        }
                        None => {}
                    }
                }
                return base.elem();
            }
            Kind::Array | Kind::Slice => {
                let p = prop.deref();
                if !p.is_integer && !p.is_unknown() {
                    let msg = format!("array elements can only be selected using an integer (got {})", p.name());
                    return self.error_at(property.loc, msg);
                }
                return base.elem();
            }
            _ => {}
        }
        if let Some(name) = property.as_string() {
            let what = if *method { "method" } else { "field" };
            let msg = format!("type {} has no {what} {}", base.name(), lossy(name));
            return self.error_at(loc, msg);
        }
        let msg = format!("type {}[{}] is undefined", base.name(), prop.name());
        self.error_at(loc, msg)
    }

    fn slice_node(&mut self, node: &mut Node) -> Nature {
        let loc = node.loc;
        let NodeKind::Slice { node: base, from, to } = &mut node.kind else { return Nature::default() };
        let nt = self.visit(base);
        if nt.is_unknown() {
            return Nature::default();
        }
        if !matches!(nt.kind, Kind::String | Kind::Array | Kind::Slice) {
            return self.error_at(loc, format!("cannot slice {}", nt.name()));
        }
        for bound in [from, to].into_iter().flatten() {
            let b = self.visit(bound).deref();
            if !b.is_integer && !b.is_unknown() {
                let msg = format!("non-integer slice index {}", b.name());
                return self.error_at(bound.loc, msg);
            }
        }
        nt
    }

    fn call_node(&mut self, node: &mut Node) -> Nature {
        if !node.nature.is_unknown() && node.nature.ty.is_some() {
            return node.nature.clone();
        }
        let loc = node.loc;
        let env_name = self.config.env.name();
        let NodeKind::Call { callee, args } = &mut node.kind else { return Nature::default() };
        if matches!(&callee.kind, NodeKind::Identifier(id) if id == "$env") {
            return self.error_at(loc, format!("{env_name} is not callable"));
        }
        let nt = self.visit(callee);
        if nt.is_unknown() {
            return Nature::default();
        }
        if let Some(id) = nt.builtin_func() {
            return self.check_function(builtin::NAMES[id], loc, args, Some(callee));
        }
        let fn_name = match &callee.kind {
            NodeKind::Identifier(id) => id.clone(),
            NodeKind::Member { property, .. } => match property.as_string() {
                Some(s) => lossy(s),
                None => "function".to_string(),
            },
            _ => "function".to_string(),
        };
        if nt.nil {
            return self.error_at(loc, format!("{fn_name} is nil; cannot call nil as function"));
        }
        if nt.kind == Kind::Func {
            return match self.check_arguments(&fn_name, &nt, args, loc) {
                Ok(out) => out,
                Err(e) => {
                    if self.err.is_none() {
                        self.err = Some(e);
                    }
                    Nature::default()
                }
            };
        }
        self.error_at(loc, format!("{} is not callable", nt.name()))
    }

    fn collection(&mut self, name: &str, args: &mut [Node]) -> Result<Nature, Nature> {
        let collection = self.visit(&mut args[0]).deref();
        if !collection.is_array() && !collection.is_unknown() {
            let msg = format!("builtin {name} takes only array (got {})", collection.name());
            return Err(self.error_at(args[0].loc, msg));
        }
        Ok(collection)
    }

    fn visit_predicate(&mut self, collection: &Nature, vars: Vec<(String, Nature)>, predicate: &mut Node) -> Nature {
        self.predicate_scopes.push(PredicateScope { collection: collection.clone(), vars });
        let nt = self.visit(predicate);
        self.predicate_scopes.pop();
        nt
    }

    fn is_simple_predicate(predicate: &Nature) -> bool {
        predicate.is_func() && predicate.num_out() == 1 && predicate.num_in() == 1 && predicate.is_first_arg_unknown()
    }

    fn check_predicate_out(&mut self, predicate: &Nature, loc: Location) -> Option<Nature> {
        let out = predicate.output(0);
        if !out.is_bool() && !out.is_unknown() {
            return Some(self.error_at(loc, format!("predicate should return boolean (got {})", out.name())));
        }
        None
    }

    fn builtin_node(&mut self, node: &mut Node) -> Nature {
        let loc = node.loc;
        let NodeKind::Builtin { name, args, .. } = &mut node.kind else { return Nature::default() };
        let name = name.clone();
        let wrong_shape = "predicate should has one input and one output param";
        match name.as_str() {
            "all" | "none" | "any" | "one" | "filter" | "find" | "findLast" | "findIndex" | "findLastIndex" => {
                let collection = match self.collection(&name, args) {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                let predicate = self.visit_predicate(&collection, Vec::new(), &mut args[1]);
                if !Self::is_simple_predicate(&predicate) {
                    return self.error_at(args[1].loc, wrong_shape);
                }
                if let Some(e) = self.check_predicate_out(&predicate, args[1].loc) {
                    return e;
                }
                match name.as_str() {
                    "filter" if collection.is_unknown() => Nature::from_type(&Type::any_slice()),
                    "filter" => collection.elem().make_array_of(),
                    "find" | "findLast" if collection.is_unknown() => Nature::default(),
                    "find" | "findLast" => collection.elem(),
                    "findIndex" | "findLastIndex" => int_nature(),
                    _ => bool_nature(),
                }
            }
            "map" => {
                let collection = match self.collection(&name, args) {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                let vars = vec![("index".to_string(), int_nature())];
                let predicate = self.visit_predicate(&collection, vars, &mut args[1]);
                if !Self::is_simple_predicate(&predicate) {
                    return self.error_at(args[1].loc, wrong_shape);
                }
                predicate.elem_ref.as_deref().cloned().unwrap_or_default().make_array_of()
            }
            "count" => {
                let collection = match self.collection(&name, args) {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                if args.len() == 1 {
                    return int_nature();
                }
                let predicate = self.visit_predicate(&collection, Vec::new(), &mut args[1]);
                if !Self::is_simple_predicate(&predicate) {
                    return self.error_at(args[1].loc, wrong_shape);
                }
                if let Some(e) = self.check_predicate_out(&predicate, args[1].loc) {
                    return e;
                }
                int_nature()
            }
            "sum" => {
                let collection = match self.collection(&name, args) {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                if args.len() == 2 {
                    let predicate = self.visit_predicate(&collection, Vec::new(), &mut args[1]);
                    if Self::is_simple_predicate(&predicate) {
                        return predicate.output(0);
                    }
                    // The original falls through to the generic builtin check here.
                    return self.check_function(&name, loc, args, None);
                }
                if collection.is_unknown() {
                    return Nature::default();
                }
                collection.elem()
            }
            "groupBy" => {
                let collection = match self.collection(&name, args) {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                let predicate = self.visit_predicate(&collection, Vec::new(), &mut args[1]);
                if !Self::is_simple_predicate(&predicate) {
                    return self.error_at(args[1].loc, wrong_shape);
                }
                let elem_array = collection.elem().make_array_of();
                let mut nt = Nature::from_type(&Type::map(Type::Any, Type::any_slice()));
                nt.elem_ref = Some(Box::new(elem_array));
                nt
            }
            "sortBy" => {
                let collection = match self.collection(&name, args) {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                let predicate = self.visit_predicate(&collection, Vec::new(), &mut args[1]);
                if args.len() == 3 {
                    let order = self.visit(&mut args[2]);
                    if !order.is_string() && !order.is_unknown() {
                        let msg = format!("sortBy order argument must be a string (got {})", order.name());
                        return self.error_at(args[2].loc, msg);
                    }
                }
                if !Self::is_simple_predicate(&predicate) {
                    return self.error_at(args[1].loc, wrong_shape);
                }
                collection
            }
            "reduce" => {
                let collection = match self.collection(&name, args) {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                let vars = vec![("index".to_string(), int_nature()), ("acc".to_string(), Nature::default())];
                let predicate = self.visit_predicate(&collection, vars, &mut args[1]);
                if args.len() == 3 {
                    self.visit(&mut args[2]);
                }
                if predicate.is_func() && predicate.num_out() == 1 {
                    return predicate.elem_ref.as_deref().cloned().unwrap_or_default();
                }
                self.error_at(args[1].loc, "predicate should has two input and one output param")
            }
            "get" => self.check_builtin_get(loc, args),
            _ if builtin::index(&name).is_some() => self.check_function(&name, loc, args, None),
            _ => self.error_at(loc, format!("unknown builtin {name}")),
        }
    }

    fn check_builtin_get(&mut self, loc: Location, args: &mut [Node]) -> Nature {
        if args.len() != 2 {
            return self.error_at(loc, format!("invalid number of arguments (expected 2, got {})", args.len()));
        }
        let base = self.visit(&mut args[0]);
        let prop = self.visit(&mut args[1]).deref();
        if matches!(&args[0].kind, NodeKind::Identifier(id) if id == "$env") {
            if let Some(s) = args[1].as_string() {
                if let Some(nt) = self.config.env.get(s) {
                    return nt;
                }
            }
            return Nature::default();
        }
        if base.is_unknown() {
            return Nature::default();
        }
        match base.kind {
            Kind::Slice | Kind::Array => {
                if !prop.is_integer && !prop.is_unknown() {
                    let msg = format!("non-integer slice index {}", prop.name());
                    return self.error_at(args[1].loc, msg);
                }
                base.elem()
            }
            Kind::Map => {
                if !prop.assignable_to(&base.key()) && !prop.is_unknown() {
                    let msg = format!("cannot use {} to get an element from {}", prop.name(), base.name());
                    return self.error_at(args[1].loc, msg);
                }
                base.elem()
            }
            _ => {
                let msg = format!("type {} does not support indexing", base.name());
                self.error_at(args[0].loc, msg)
            }
        }
    }

    /// Checks a call of a builtin: its own validation, or its overloads in order.
    fn check_function(
        &mut self,
        name: &str,
        loc: Location,
        args: &mut [Node],
        callee: Option<&mut Box<Node>>,
    ) -> Nature {
        if builtin::validate(name, &[]).is_some() || matches!(name, "len" | "abs" | "first" | "last") {
            let mut types = Vec::with_capacity(args.len());
            for arg in args.iter_mut() {
                let nt = self.visit(arg);
                types.push(if nt.is_unknown() { Some(Type::Any) } else { nt.ty.clone() });
            }
            return match builtin::validate(name, &types) {
                Some(Ok(t)) => Nature::from_type(&t),
                Some(Err(e)) => self.error_at(loc, e),
                None => Nature::default(),
            };
        }
        let overloads = builtin::types(name);
        if overloads.is_empty() {
            let fn_nature = Nature::from_type(&builtin::func_type(name));
            return match self.check_arguments(name, &fn_nature, args, loc) {
                Ok(nt) => nt,
                Err(e) => {
                    if self.err.is_none() {
                        self.err = Some(e);
                    }
                    Nature::default()
                }
            };
        }
        let mut last_err = None;
        for t in &overloads {
            match self.check_arguments(name, &Nature::from_type(t), args, loc) {
                Ok(out) => {
                    if let Some(callee) = callee {
                        callee.set_type(t.clone());
                    }
                    return out;
                }
                Err(e) => last_err = Some(e),
            }
        }
        match last_err {
            Some(e) => {
                if self.err.is_none() {
                    self.err = Some(e);
                }
                Nature::default()
            }
            None => self.error_at(loc, format!("no matching overload for {name}")),
        }
    }

    fn check_arguments(
        &mut self,
        name: &str,
        func: &Nature,
        args: &mut [Node],
        loc: Location,
    ) -> Result<Nature, Error> {
        if func.is_unknown() {
            return Ok(Nature::default());
        }
        let num_out = func.num_out();
        if num_out == 0 {
            return Err(Error::new(loc, format!("func {name} doesn't return value")));
        }
        if num_out > 2 {
            return Err(Error::new(loc, format!("func {name} returns more then two values")));
        }
        let offset = usize::from(func.method);
        let num_in = func.num_in() - offset;
        let variadic = func.is_variadic();
        let mut err = None;
        if variadic {
            if args.len() + 1 < num_in {
                err = Some(Error::new(loc, format!("not enough arguments to call {name}")));
            }
        } else {
            if args.len() > num_in {
                err = Some(Error::new(loc, format!("too many arguments to call {name}")));
            }
            if args.len() < num_in {
                err = Some(Error::new(loc, format!("not enough arguments to call {name}")));
            }
        }
        if let Some(err) = err {
            for arg in args.iter_mut() {
                self.visit(arg);
            }
            return Err(err);
        }

        for (i, arg) in args.iter_mut().enumerate() {
            let arg_nature = self.visit(arg);
            let input =
                if variadic && i + 1 >= num_in { func.input_elem(num_in - 1 + offset) } else { func.input(i + offset) };
            if input.is_float && arg_nature.is_integer {
                replace_integers_with_floats(arg, &input);
                continue;
            }
            if input.is_integer && arg_nature.is_integer && arg_nature.kind != input.kind {
                retype_integers(arg, &input);
                continue;
            }
            if arg_nature.nil {
                if matches!(input.kind, Kind::Ptr | Kind::Interface) {
                    continue;
                }
                return Err(Error::new(
                    arg.loc,
                    format!("cannot use nil as argument (type {}) to call {name}", input.name()),
                ));
            }
            let mut assignable = arg_nature.assignable_to(&input);
            if !assignable && arg_nature.is_pointer() {
                assignable = arg_nature.deref().assignable_to(&input);
            }
            if !assignable && !arg_nature.is_unknown() {
                return Err(Error::new(
                    arg.loc,
                    format!("cannot use {} as argument (type {}) to call {name} ", arg_nature.name(), input.name()),
                ));
            }
        }
        Ok(func.output(0))
    }

    fn predicate_node(&mut self, node: &mut Node) -> Nature {
        let NodeKind::Predicate(inner) = &mut node.kind else { return Nature::default() };
        let nt = self.visit(inner);
        let outs = if nt.is_unknown() {
            vec![Type::Any]
        } else if !nt.nil {
            nt.ty.iter().cloned().collect()
        } else {
            Vec::new()
        };
        let mut n = Nature::from_type(&Type::func(vec![Type::Any], outs, false));
        n.elem_ref = Some(Box::new(nt));
        n
    }

    fn pointer_node(&mut self, node: &Node) -> Nature {
        let NodeKind::Pointer(name) = &node.kind else { return Nature::default() };
        let Some(scope) = self.predicate_scopes.last() else {
            return self.error_at(node.loc, "cannot use pointer accessor outside predicate");
        };
        if name.is_empty() {
            if scope.collection.is_unknown() {
                return Nature::default();
            }
            if matches!(scope.collection.kind, Kind::Array | Kind::Slice) {
                return scope.collection.elem();
            }
            let msg = format!("cannot use {} as array", scope.collection.name());
            return self.error_at(node.loc, msg);
        }
        if let Some((_, nt)) = scope.vars.iter().find(|(n, _)| n == name) {
            return nt.clone();
        }
        self.error_at(node.loc, format!("unknown pointer #{name}"))
    }

    fn variable_declarator_node(&mut self, node: &mut Node) -> Nature {
        let loc = node.loc;
        let NodeKind::VariableDeclarator { name, value, expr } = &mut node.kind else { return Nature::default() };
        if self.config.env.get(name.as_bytes()).is_some() {
            return self.error_at(loc, format!("cannot redeclare {name}"));
        }
        if self.config.builtins.contains(name.as_str()) {
            return self.error_at(loc, format!("cannot redeclare builtin {name}"));
        }
        if self.var_scopes.iter().any(|(n, _)| n == name) {
            return self.error_at(loc, format!("cannot redeclare variable {name}"));
        }
        let var_nature = self.visit(value);
        self.var_scopes.push((name.clone(), var_nature));
        let nt = self.visit(expr);
        self.var_scopes.pop();
        nt
    }

    fn sequence_node(&mut self, node: &mut Node) -> Nature {
        let loc = node.loc;
        let NodeKind::Sequence(nodes) = &mut node.kind else { return Nature::default() };
        if nodes.is_empty() {
            return self.error_at(loc, "empty sequence expression");
        }
        let mut last = Nature::default();
        for n in nodes.iter_mut() {
            last = self.visit(n);
        }
        last
    }

    fn conditional_node(&mut self, node: &mut Node) -> Nature {
        let NodeKind::Conditional { cond, exp1, exp2, .. } = &mut node.kind else { return Nature::default() };
        let c = self.visit(cond).deref();
        if !c.is_bool() && !c.is_unknown() {
            let msg = format!("non-bool expression (type {}) used as condition", c.name());
            return self.error_at(cond.loc, msg);
        }
        let t1 = self.visit(exp1);
        let t2 = self.visit(exp2);
        if t1.nil && !t2.nil {
            return t2;
        }
        if !t1.nil && t2.nil {
            return t1;
        }
        if t1.nil && t2.nil {
            return Nature::nil();
        }
        if t1.assignable_to(&t2) {
            if t1.is_array() && t2.is_array() {
                let (e1, e2) = (t1.elem(), t2.elem());
                if !e1.assignable_to(&e2) || !e2.assignable_to(&e1) {
                    return Nature::from_type(&Type::any_slice());
                }
            }
            return t1;
        }
        Nature::default()
    }

    fn array_node(&mut self, node: &mut Node) -> Nature {
        let NodeKind::Array(nodes) = &mut node.kind else { return Nature::default() };
        let mut prev = Nature::default();
        let mut same = true;
        for (i, n) in nodes.iter_mut().enumerate() {
            let curr = self.visit(n);
            if i > 0 && curr.kind != prev.kind {
                same = false;
            }
            prev = curr;
        }
        if same {
            return prev.make_array_of();
        }
        Nature::from_type(&Type::any_slice())
    }

    fn map_node(&mut self, node: &mut Node) -> Nature {
        if let NodeKind::Map(pairs) = &mut node.kind {
            for pair in pairs.iter_mut() {
                self.visit(pair);
            }
        }
        Nature::from_type(&Type::string_map())
    }

    fn pair_node(&mut self, node: &mut Node) -> Nature {
        if let NodeKind::Pair { key, value } = &mut node.kind {
            self.visit(key);
            self.visit(value);
        }
        Nature::nil()
    }
}

/// Turns integer literals of an argument into floats for a float parameter, through `+`, `-` and `*`.
fn replace_integers_with_floats(node: &mut Node, input: &Nature) {
    match &mut node.kind {
        NodeKind::Integer(i) => {
            node.kind = NodeKind::Float(*i as f64);
            node.set_type(input.ty_or_any());
        }
        NodeKind::Unary { node: inner, .. } => replace_integers_with_floats(inner, input),
        NodeKind::Binary { op, left, right } if matches!(op.as_str(), "+" | "-" | "*") => {
            replace_integers_with_floats(left, input);
            replace_integers_with_floats(right, input);
        }
        _ => {}
    }
}

/// Gives integer literals of an argument the integer type of the parameter, through `+`, `-` and `*`.
fn retype_integers(node: &mut Node, input: &Nature) {
    match &mut node.kind {
        NodeKind::Integer(_) => node.set_type(input.ty_or_any()),
        NodeKind::Unary { .. } => {
            node.set_type(input.ty_or_any());
            if let NodeKind::Unary { node: inner, .. } = &mut node.kind {
                retype_integers(inner, input);
            }
        }
        NodeKind::Binary { op, left, right } if matches!(op.as_str(), "+" | "-" | "*") => {
            retype_integers(left, input);
            retype_integers(right, input);
        }
        _ => {}
    }
}
