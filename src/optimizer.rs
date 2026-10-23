//! The optimizer: constant folding and rewrites of common predicate patterns.
//!
//! The rewrites are observable: folding reports some errors at compile time, `in` over a literal list becomes a
//! set lookup, and `filter(...)[0]` stops at the first match. They run only when optimization is on.

use crate::ast::{Node, NodeKind};
use crate::file::Error;
use crate::parser::operator;
use crate::types::Type;
use crate::value::{MapValue, Value};

/// Optimizes the tree in place.
pub fn optimize(node: &mut Node) -> Result<(), Error> {
    node.walk(&mut in_array);
    for _ in 0..=1000 {
        let mut applied = false;
        let mut err = None;
        node.walk(&mut |n| fold(n, &mut applied, &mut err));
        if let Some(err) = err {
            return Err(err);
        }
        if !applied {
            break;
        }
    }
    node.walk(&mut in_range);
    node.walk(&mut filter_map);
    node.walk(&mut filter_len);
    node.walk(&mut filter_last);
    node.walk(&mut filter_first);
    node.walk(&mut predicate_combination);
    node.walk(&mut sum_range);
    node.walk(&mut sum_array);
    node.walk(&mut sum_map);
    node.walk(&mut count_any);
    node.walk(&mut count_threshold);
    Ok(())
}

/// Replaces a node, giving the new node the plain type of its kind and the old node's location.
fn patch_with_type(node: &mut Node, kind: NodeKind) {
    let ty = match &kind {
        NodeKind::Bool(_) => Type::Bool,
        NodeKind::Integer(_) => Type::Int,
        NodeKind::Float(_) => Type::Float64,
        NodeKind::String(_) => Type::String,
        NodeKind::Constant(v) => v.type_of().unwrap_or(Type::Any),
        _ => Type::Any,
    };
    node.kind = kind;
    node.set_type(ty);
}

/// Replaces a node by `new`, which takes the old node's type and location.
fn patch_copy_type(node: &mut Node, mut new: Node) {
    new.set_type(node.ty());
    new.loc = node.loc;
    *node = new;
}

fn bare(kind: NodeKind) -> Node {
    Node::new(kind, Default::default())
}

fn builtin(name: &str, args: Vec<Node>, throws: bool, map: Option<Box<Node>>) -> Node {
    bare(NodeKind::Builtin { name: name.to_string(), args, throws, map, threshold: None })
}

fn in_array(node: &mut Node) {
    let NodeKind::Binary { op, left, right } = &node.kind else { return };
    if op != "in" {
        return;
    }
    let NodeKind::Array(items) = &right.kind else { return };
    if items.is_empty() {
        return;
    }
    let left_is_int = left.nature.ty.as_ref().is_some_and(|t| t.kind() == crate::types::Kind::Int);
    let set = if left_is_int && items.iter().all(|a| matches!(a.kind, NodeKind::Integer(_))) {
        let mut set = MapValue::new(Type::map(Type::Int, Type::Struct(crate::types::Struct::Empty)));
        for a in items {
            if let NodeKind::Integer(i) = a.kind {
                let _ = set.insert(Value::Int(i), Value::EmptyStruct);
            }
        }
        set
    } else if items.iter().all(|a| matches!(a.kind, NodeKind::String(_))) {
        let mut set = MapValue::new(Type::map(Type::String, Type::Struct(crate::types::Struct::Empty)));
        for a in items {
            if let NodeKind::String(s) = &a.kind {
                let _ = set.insert(Value::string(s), Value::EmptyStruct);
            }
        }
        set
    } else {
        return;
    };
    let mut constant = bare(NodeKind::Constant(Value::Map(std::rc::Rc::new(set))));
    constant.set_type(constant_type(&constant));
    let left = left.clone();
    patch_copy_type(node, bare(NodeKind::Binary { op: "in".to_string(), left, right: Box::new(constant) }));
}

fn constant_type(node: &Node) -> Type {
    match &node.kind {
        NodeKind::Constant(v) => v.type_of().unwrap_or(Type::Any),
        _ => Type::Any,
    }
}

fn fold(node: &mut Node, applied: &mut bool, err: &mut Option<Error>) {
    let mut patch = |node: &mut Node, kind: NodeKind| {
        *applied = true;
        patch_with_type(node, kind);
    };
    match &node.kind {
        NodeKind::Unary { op, node: inner } => {
            let new = match (op.as_str(), &inner.kind) {
                ("-", NodeKind::Integer(i)) => Some(NodeKind::Integer(i.wrapping_neg())),
                ("-", NodeKind::Float(f)) => Some(NodeKind::Float(-f)),
                ("+", NodeKind::Integer(i)) => Some(NodeKind::Integer(*i)),
                ("+", NodeKind::Float(f)) => Some(NodeKind::Float(*f)),
                ("!" | "not", NodeKind::Bool(b)) => Some(NodeKind::Bool(!b)),
                _ => None,
            };
            if let Some(kind) = new {
                patch(node, kind);
            }
        }
        NodeKind::Binary { op, left, right } => {
            let (l, r) = (&left.kind, &right.kind);
            let new = match (op.as_str(), l, r) {
                ("+", NodeKind::Integer(a), NodeKind::Integer(b)) => Some(NodeKind::Integer(a.wrapping_add(*b))),
                ("-", NodeKind::Integer(a), NodeKind::Integer(b)) => Some(NodeKind::Integer(a.wrapping_sub(*b))),
                ("*", NodeKind::Integer(a), NodeKind::Integer(b)) => Some(NodeKind::Integer(a.wrapping_mul(*b))),
                ("+", NodeKind::String(a), NodeKind::String(b)) => {
                    let mut s = a.clone();
                    s.extend_from_slice(b);
                    Some(NodeKind::String(s))
                }
                ("+" | "-" | "*" | "/" | "**" | "^", _, _) if float_operands(l, r).is_some() => {
                    float_operands(l, r).map(|(a, b)| NodeKind::Float(arith(op, a, b)))
                }
                ("%", NodeKind::Integer(a), NodeKind::Integer(b)) => {
                    if *b == 0 {
                        *err = Some(Error::new(node.loc, "integer divide by zero"));
                        return;
                    }
                    Some(NodeKind::Integer(a.wrapping_rem(*b)))
                }
                ("and" | "&&", _, _) => match (as_bool(l), as_bool(r)) {
                    (Some(true), _) => {
                        *applied = true;
                        let right = (**right).clone();
                        patch_copy_type(node, right);
                        return;
                    }
                    (_, Some(true)) => {
                        *applied = true;
                        let left = (**left).clone();
                        patch_copy_type(node, left);
                        return;
                    }
                    (Some(false), _) | (_, Some(false)) => Some(NodeKind::Bool(false)),
                    _ => None,
                },
                ("or" | "||", _, _) => match (as_bool(l), as_bool(r)) {
                    (Some(false), _) => {
                        *applied = true;
                        let right = (**right).clone();
                        patch_copy_type(node, right);
                        return;
                    }
                    (_, Some(false)) => {
                        *applied = true;
                        let left = (**left).clone();
                        patch_copy_type(node, left);
                        return;
                    }
                    (Some(true), _) | (_, Some(true)) => Some(NodeKind::Bool(true)),
                    _ => None,
                },
                ("==", NodeKind::Integer(a), NodeKind::Integer(b)) => Some(NodeKind::Bool(a == b)),
                ("==", NodeKind::String(a), NodeKind::String(b)) => Some(NodeKind::Bool(a == b)),
                ("==", NodeKind::Bool(a), NodeKind::Bool(b)) => Some(NodeKind::Bool(a == b)),
                _ => None,
            };
            if let Some(kind) = new {
                patch(node, kind);
            }
        }
        NodeKind::Array(items) => {
            if items.is_empty() {
                return;
            }
            let mut values = Vec::with_capacity(items.len());
            for a in items {
                values.push(match &a.kind {
                    NodeKind::Integer(i) => Value::Int(*i),
                    NodeKind::Float(f) => Value::Float64(*f),
                    NodeKind::String(s) => Value::string(s),
                    NodeKind::Bool(b) => Value::Bool(*b),
                    _ => return,
                });
            }
            patch(node, NodeKind::Constant(Value::any_slice(values)));
        }
        NodeKind::Builtin { name, args, .. } if name == "filter" && args.len() == 2 => {
            if let NodeKind::Builtin { name: inner, args: inner_args, .. } = &args[0].kind {
                if inner == "filter" {
                    let (Some(p1), Some(p2)) = (predicate_body(&inner_args[1]), predicate_body(&args[1])) else {
                        return;
                    };
                    let combined = bare(NodeKind::Predicate(Box::new(bare(NodeKind::Binary {
                        op: "&&".to_string(),
                        left: Box::new(p1.clone()),
                        right: Box::new(p2.clone()),
                    }))));
                    *applied = true;
                    let new = builtin("filter", vec![inner_args[0].clone(), combined], false, None);
                    patch_copy_type(node, new);
                }
            }
        }
        _ => {}
    }
}

fn predicate_body(node: &Node) -> Option<&Node> {
    match &node.kind {
        NodeKind::Predicate(body) => Some(body),
        _ => None,
    }
}

fn as_bool(kind: &NodeKind) -> Option<bool> {
    match kind {
        NodeKind::Bool(b) => Some(*b),
        _ => None,
    }
}

/// The operands of a float fold: at least one float literal, the other an integer or float literal.
fn float_operands(l: &NodeKind, r: &NodeKind) -> Option<(f64, f64)> {
    match (l, r) {
        (NodeKind::Integer(a), NodeKind::Integer(b)) => Some((*a as f64, *b as f64)),
        (NodeKind::Integer(a), NodeKind::Float(b)) => Some((*a as f64, *b)),
        (NodeKind::Float(a), NodeKind::Integer(b)) => Some((*a, *b as f64)),
        (NodeKind::Float(a), NodeKind::Float(b)) => Some((*a, *b)),
        _ => None,
    }
}

fn arith(op: &str, a: f64, b: f64) -> f64 {
    match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" => a / b,
        _ => crate::gostd::math::pow(a, b),
    }
}

fn in_range(node: &mut Node) {
    let NodeKind::Binary { op, left, right } = &node.kind else { return };
    if op != "in" || left.nature.ty.as_ref().is_none_or(|t| t.kind() != crate::types::Kind::Int) {
        return;
    }
    let NodeKind::Binary { op: range_op, left: from, right: to } = &right.kind else { return };
    if range_op != ".." || from.as_integer().is_none() || to.as_integer().is_none() {
        return;
    }
    let ge = bare(NodeKind::Binary { op: ">=".to_string(), left: left.clone(), right: from.clone() });
    let le = bare(NodeKind::Binary { op: "<=".to_string(), left: left.clone(), right: to.clone() });
    patch_copy_type(node, bare(NodeKind::Binary { op: "and".to_string(), left: Box::new(ge), right: Box::new(le) }));
}

fn filter_map(node: &mut Node) {
    let NodeKind::Builtin { name, args, .. } = &node.kind else { return };
    if name != "map" || args.len() != 2 || args[1].find(&|n| n.is_pointer("index")) {
        return;
    }
    let Some(body) = predicate_body(&args[1]) else { return };
    let NodeKind::Builtin { name: inner, args: filter_args, map: None, .. } = &args[0].kind else { return };
    if inner != "filter" {
        return;
    }
    let new = builtin("filter", filter_args.clone(), false, Some(Box::new(body.clone())));
    patch_copy_type(node, new);
}

fn filter_len(node: &mut Node) {
    let NodeKind::Builtin { name, args, .. } = &node.kind else { return };
    if name != "len" || args.len() != 1 {
        return;
    }
    let NodeKind::Builtin { name: inner, args: filter_args, .. } = &args[0].kind else { return };
    if inner != "filter" || filter_args.len() != 2 {
        return;
    }
    let new = builtin("count", filter_args.clone(), false, None);
    patch_copy_type(node, new);
}

/// `filter(...)[i]` and `first/last(filter(...))` become `find`/`findLast`.
fn filter_find(node: &mut Node, index: i64, wrapper: &str, find: &str) {
    let replacement = match &node.kind {
        NodeKind::Member { node: base, property, optional: false, .. } if property.as_integer() == Some(index) => {
            match &base.kind {
                NodeKind::Builtin { name, args, map, .. } if name == "filter" && args.len() == 2 => {
                    Some(builtin(find, args.clone(), true, map.clone()))
                }
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(new) = replacement {
        patch_copy_type(node, new);
    }
    let replacement = match &node.kind {
        NodeKind::Builtin { name, args, .. } if name == wrapper && args.len() == 1 => match &args[0].kind {
            NodeKind::Builtin { name: inner, args: filter_args, map, .. }
                if inner == "filter" && filter_args.len() == 2 =>
            {
                Some(builtin(find, filter_args.clone(), false, map.clone()))
            }
            _ => None,
        },
        _ => None,
    };
    if let Some(new) = replacement {
        patch_copy_type(node, new);
    }
}

fn filter_last(node: &mut Node) {
    filter_find(node, -1, "last", "findLast");
}

fn filter_first(node: &mut Node) {
    filter_find(node, 0, "first", "find");
}

fn combined_operator(func: &str, op: &str) -> Option<&'static str> {
    match (func, op) {
        ("all", "and") => Some("and"),
        ("all", "&&") => Some("&&"),
        ("any", "or") => Some("or"),
        ("any", "||") => Some("||"),
        ("none", "and") => Some("or"),
        ("none", "&&") => Some("||"),
        _ => None,
    }
}

fn predicate_combination(node: &mut Node) {
    let NodeKind::Binary { op, left, right } = &node.kind else { return };
    if !operator::is_boolean(op) {
        return;
    }
    let NodeKind::Builtin { name: lname, args: largs, .. } = &left.kind else { return };
    let Some(combined) = combined_operator(lname, op) else { return };
    let NodeKind::Builtin { name: rname, args: rargs, .. } = &right.kind else { return };
    if rname != lname || largs[0].ty() != rargs[0].ty() || largs[0].to_source() != rargs[0].to_source() {
        return;
    }
    let (Some(lbody), Some(rbody)) = (predicate_body(&largs[1]), predicate_body(&rargs[1])) else { return };
    let mut body = bare(NodeKind::Binary {
        op: combined.to_string(),
        left: Box::new(lbody.clone()),
        right: Box::new(rbody.clone()),
    });
    predicate_combination(&mut body);
    let predicate = bare(NodeKind::Predicate(Box::new(body)));
    let new = builtin(lname, vec![largs[0].clone(), predicate], false, None);
    patch_copy_type(node, new);
}

fn sum_range(node: &mut Node) {
    let replacement = match &node.kind {
        NodeKind::Builtin { name, args, .. } if name == "sum" && (args.len() == 1 || args.len() == 2) => {
            range_bounds(&args[0]).and_then(|(m, n)| {
                if n < m {
                    return None;
                }
                let count = n.wrapping_sub(m).wrapping_add(1);
                let sum = count.wrapping_mul(m.wrapping_add(n)) / 2;
                if args.len() == 1 {
                    Some(sum)
                } else {
                    apply_sum_predicate(sum, count, &args[1])
                }
            })
        }
        NodeKind::Builtin { name, args, .. } if name == "reduce" && (args.len() == 2 || args.len() == 3) => {
            range_bounds(&args[0]).filter(|_| is_pointer_plus_acc(&args[1])).and_then(|(m, n)| {
                if n < m {
                    return None;
                }
                let sum = n.wrapping_sub(m).wrapping_add(1).wrapping_mul(m.wrapping_add(n)) / 2;
                if args.len() == 3 {
                    args[2].as_integer().map(|init| init.wrapping_add(sum))
                } else {
                    Some(sum)
                }
            })
        }
        _ => None,
    };
    if let Some(value) = replacement {
        patch_with_type(node, NodeKind::Integer(value));
    }
}

fn range_bounds(node: &Node) -> Option<(i64, i64)> {
    match &node.kind {
        NodeKind::Binary { op, left, right } if op == ".." => Some((left.as_integer()?, right.as_integer()?)),
        _ => None,
    }
}

fn is_pointer_plus_acc(node: &Node) -> bool {
    let Some(body) = predicate_body(node) else { return false };
    match &body.kind {
        NodeKind::Binary { op, left, right } if op == "+" => {
            left.is_pointer("") && right.is_pointer("acc") || left.is_pointer("acc") && right.is_pointer("")
        }
        _ => false,
    }
}

fn apply_sum_predicate(sum: i64, count: i64, predicate: &Node) -> Option<i64> {
    let body = predicate_body(predicate)?;
    if body.is_pointer("") {
        return Some(sum);
    }
    let NodeKind::Binary { op, left, right } = &body.kind else { return None };
    let (constant, pointer_on_left) = if left.is_pointer("") {
        (right.as_integer()?, true)
    } else if right.is_pointer("") {
        (left.as_integer()?, false)
    } else {
        return None;
    };
    match op.as_str() {
        "*" => Some(constant.wrapping_mul(sum)),
        "+" => Some(sum.wrapping_add(count.wrapping_mul(constant))),
        "-" if pointer_on_left => Some(sum.wrapping_sub(count.wrapping_mul(constant))),
        "-" => Some(count.wrapping_mul(constant).wrapping_sub(sum)),
        _ => None,
    }
}

fn sum_array(node: &mut Node) {
    let NodeKind::Builtin { name, args, .. } = &node.kind else { return };
    if name != "sum" || args.len() != 1 {
        return;
    }
    let NodeKind::Array(items) = &args[0].kind else { return };
    if items.len() < 2 {
        return;
    }
    let new = sum_array_fold(items);
    patch_copy_type(node, new);
}

fn sum_array_fold(items: &[Node]) -> Node {
    let right = if items.len() > 2 { sum_array_fold(&items[1..]) } else { items[1].clone() };
    bare(NodeKind::Binary { op: "+".to_string(), left: Box::new(items[0].clone()), right: Box::new(right) })
}

fn sum_map(node: &mut Node) {
    let NodeKind::Builtin { name, args, .. } = &node.kind else { return };
    if name != "sum" || args.len() != 1 {
        return;
    }
    let NodeKind::Builtin { name: inner, args: map_args, .. } = &args[0].kind else { return };
    if inner != "map" || map_args.len() != 2 {
        return;
    }
    let new = builtin("sum", vec![map_args[0].clone(), map_args[1].clone()], false, None);
    patch_copy_type(node, new);
}

fn count_any(node: &mut Node) {
    let NodeKind::Binary { op, left, right } = &node.kind else { return };
    let NodeKind::Builtin { name, args, .. } = &left.kind else { return };
    if name != "count" || args.len() != 2 {
        return;
    }
    let Some(value) = right.as_integer() else { return };
    if op == ">" && value == 0 || op == ">=" && value == 1 {
        let new = builtin("any", args.clone(), false, None);
        patch_copy_type(node, new);
    }
}

fn count_threshold(node: &mut Node) {
    let NodeKind::Binary { op, left, right } = &mut node.kind else { return };
    let Some(value) = right.as_integer() else { return };
    if value < 0 {
        return;
    }
    let NodeKind::Builtin { name, args, threshold, .. } = &mut left.kind else { return };
    if name != "count" || args.len() != 2 {
        return;
    }
    let t = match op.as_str() {
        ">" | "<=" => value.wrapping_add(1),
        ">=" | "<" => value,
        _ => return,
    };
    if t <= 1 {
        return;
    }
    *threshold = Some(t);
}
