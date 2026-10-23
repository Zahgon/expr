//! The syntax tree.

use crate::checker::nature::Nature;
use crate::file::Location;
use crate::gostd::strconv;
use crate::parser::lexer::is_valid_identifier;
use crate::parser::operator;
use crate::types::Type;
use crate::value::Value;

/// A node of the tree: its kind, its location and the nature the checker found for it.
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub loc: Location,
    pub nature: Nature,
}

/// The node kinds of the language.
#[derive(Clone, Debug)]
pub enum NodeKind {
    Nil,
    Identifier(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    String(Vec<u8>),
    Bytes(Vec<u8>),
    /// A value computed by the optimizer.
    Constant(Value),
    Unary {
        op: String,
        node: Box<Node>,
    },
    Binary {
        op: String,
        left: Box<Node>,
        right: Box<Node>,
    },
    Chain(Box<Node>),
    Member {
        node: Box<Node>,
        property: Box<Node>,
        optional: bool,
        method: bool,
    },
    Slice {
        node: Box<Node>,
        from: Option<Box<Node>>,
        to: Option<Box<Node>>,
    },
    Call {
        callee: Box<Node>,
        args: Vec<Node>,
    },
    Builtin {
        name: String,
        args: Vec<Node>,
        /// Set by the optimizer: a missing element raises instead of yielding nil.
        throws: bool,
        /// Set by the optimizer: a `map()` folded into a `filter()`.
        map: Option<Box<Node>>,
        /// Set by the optimizer: `count()` may stop once it reaches this count.
        threshold: Option<i64>,
    },
    Predicate(Box<Node>),
    Pointer(String),
    Conditional {
        ternary: bool,
        cond: Box<Node>,
        exp1: Box<Node>,
        exp2: Box<Node>,
    },
    VariableDeclarator {
        name: String,
        value: Box<Node>,
        expr: Box<Node>,
    },
    Sequence(Vec<Node>),
    Array(Vec<Node>),
    Map(Vec<Node>),
    Pair {
        key: Box<Node>,
        value: Box<Node>,
    },
}

impl Node {
    pub fn new(kind: NodeKind, loc: Location) -> Node {
        Node { kind, loc, nature: Nature::default() }
    }

    /// The node's type; unknown natures are `interface {}`.
    pub fn ty(&self) -> Type {
        self.nature.ty.clone().unwrap_or(Type::Any)
    }

    /// Replaces the nature with the plain nature of a type (the original's `SetType`).
    pub fn set_type(&mut self, ty: Type) {
        self.nature = Nature::from_type(&ty);
    }

    pub fn as_string(&self) -> Option<&[u8]> {
        match &self.kind {
            NodeKind::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self.kind {
            NodeKind::Integer(i) => Some(i),
            _ => None,
        }
    }

    pub fn is_pointer(&self, name: &str) -> bool {
        matches!(&self.kind, NodeKind::Pointer(n) if n == name)
    }

    /// Visits the children, then the node itself (the original's `ast.Walk`).
    pub fn walk(&mut self, visit: &mut dyn FnMut(&mut Node)) {
        match &mut self.kind {
            NodeKind::Nil
            | NodeKind::Identifier(_)
            | NodeKind::Integer(_)
            | NodeKind::Float(_)
            | NodeKind::Bool(_)
            | NodeKind::String(_)
            | NodeKind::Bytes(_)
            | NodeKind::Constant(_)
            | NodeKind::Pointer(_) => {}
            NodeKind::Unary { node, .. } | NodeKind::Chain(node) | NodeKind::Predicate(node) => node.walk(visit),
            NodeKind::Binary { left, right, .. } => {
                left.walk(visit);
                right.walk(visit);
            }
            NodeKind::Member { node, property, .. } => {
                node.walk(visit);
                property.walk(visit);
            }
            NodeKind::Slice { node, from, to } => {
                node.walk(visit);
                if let Some(from) = from {
                    from.walk(visit);
                }
                if let Some(to) = to {
                    to.walk(visit);
                }
            }
            NodeKind::Call { callee, args } => {
                callee.walk(visit);
                for arg in args {
                    arg.walk(visit);
                }
            }
            NodeKind::Builtin { args, .. } => {
                for arg in args {
                    arg.walk(visit);
                }
            }
            NodeKind::VariableDeclarator { value, expr, .. } => {
                value.walk(visit);
                expr.walk(visit);
            }
            NodeKind::Sequence(nodes) | NodeKind::Array(nodes) | NodeKind::Map(nodes) => {
                for node in nodes {
                    node.walk(visit);
                }
            }
            NodeKind::Conditional { cond, exp1, exp2, .. } => {
                cond.walk(visit);
                exp1.walk(visit);
                exp2.walk(visit);
            }
            NodeKind::Pair { key, value } => {
                key.walk(visit);
                value.walk(visit);
            }
        }
        visit(self);
    }

    /// Whether any node of the subtree satisfies `pred` (the original's `ast.Find`).
    pub fn find(&self, pred: &dyn Fn(&Node) -> bool) -> bool {
        let mut copy = self.clone();
        let mut found = false;
        copy.walk(&mut |n| found |= pred(n));
        found
    }

    /// The expression as the original prints it.
    pub fn to_source(&self) -> String {
        match &self.kind {
            NodeKind::Nil => "nil".to_string(),
            NodeKind::Identifier(name) => name.clone(),
            NodeKind::Integer(i) => i.to_string(),
            NodeKind::Float(f) => strconv::fmt_v_float(*f, 64),
            NodeKind::Bool(b) => b.to_string(),
            NodeKind::String(s) => strconv::quote(s),
            NodeKind::Bytes(b) => format!("b{}", strconv::quote(b)),
            NodeKind::Constant(v) => match v {
                Value::Nil => "nil".to_string(),
                other => crate::builtin::json::marshal(other)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default(),
            },
            NodeKind::Unary { op, node } => {
                let op_text = if op == "not" { "not ".to_string() } else { op.clone() };
                let wrap = match &node.kind {
                    NodeKind::Binary { op: inner, .. } => {
                        operator::binary(inner).precedence < operator::unary(op).precedence
                    }
                    NodeKind::Conditional { .. } => true,
                    _ => false,
                };
                if wrap {
                    format!("{op_text}({})", node.to_source())
                } else {
                    format!("{op_text}{}", node.to_source())
                }
            }
            NodeKind::Binary { op, left, right } => binary_source(op, left, right),
            NodeKind::Chain(node) | NodeKind::Predicate(node) => node.to_source(),
            NodeKind::Member { node, property, optional, .. } => {
                let mut base = node.to_source();
                if matches!(node.kind, NodeKind::Binary { .. }) {
                    base = format!("({base})");
                }
                let ident = property.as_string().filter(|s| is_valid_identifier(s));
                if *optional {
                    return match ident {
                        Some(s) => format!("{base}?.{}", String::from_utf8_lossy(s)),
                        None => format!("{base}?.[{}]", property.to_source()),
                    };
                }
                match ident {
                    Some(s) if matches!(node.kind, NodeKind::Pointer(_)) => format!(".{}", String::from_utf8_lossy(s)),
                    Some(s) => format!("{base}.{}", String::from_utf8_lossy(s)),
                    None => format!("{base}[{}]", property.to_source()),
                }
            }
            NodeKind::Slice { node, from, to } => {
                let base = node.to_source();
                match (from, to) {
                    (None, None) => format!("{base}[:]"),
                    (None, Some(to)) => format!("{base}[:{}]", to.to_source()),
                    (Some(from), None) => format!("{base}[{}:]", from.to_source()),
                    (Some(from), Some(to)) => format!("{base}[{}:{}]", from.to_source(), to.to_source()),
                }
            }
            NodeKind::Call { callee, args } => format!("{}({})", callee.to_source(), join(args, ", ")),
            NodeKind::Builtin { name, args, .. } => format!("{name}({})", join(args, ", ")),
            NodeKind::Pointer(name) => format!("#{name}"),
            NodeKind::VariableDeclarator { name, value, expr } => {
                format!("let {name} = {}; {}", value.to_source(), expr.to_source())
            }
            NodeKind::Sequence(nodes) => join(nodes, "; "),
            NodeKind::Conditional { ternary, cond, exp1, exp2 } => {
                if !ternary {
                    if let NodeKind::Conditional { ternary: false, .. } = exp2.kind {
                        return format!("if {} {{ {} }} else {}", cond.to_source(), exp1.to_source(), exp2.to_source());
                    }
                    return format!(
                        "if {} {{ {} }} else {{ {} }}",
                        cond.to_source(),
                        exp1.to_source(),
                        exp2.to_source()
                    );
                }
                let part = |n: &Node| {
                    if matches!(n.kind, NodeKind::Conditional { .. }) {
                        format!("({})", n.to_source())
                    } else {
                        n.to_source()
                    }
                };
                format!("{} ? {} : {}", part(cond), part(exp1), part(exp2))
            }
            NodeKind::Array(nodes) => format!("[{}]", join(nodes, ", ")),
            NodeKind::Map(pairs) => format!("{{{}}}", join(pairs, ", ")),
            NodeKind::Pair { key, value } => match key.as_string() {
                Some(s) if is_valid_identifier(s) => format!("{}: {}", String::from_utf8_lossy(s), value.to_source()),
                Some(_) => format!("{}: {}", key.to_source(), value.to_source()),
                None => format!("({}): {}", key.to_source(), value.to_source()),
            },
        }
    }
}

fn join(nodes: &[Node], sep: &str) -> String {
    nodes.iter().map(Node::to_source).collect::<Vec<_>>().join(sep)
}

fn binary_source(op: &str, left: &Node, right: &Node) -> String {
    if op == ".." {
        return format!("{}..{}", left.to_source(), right.to_source());
    }
    let this = operator::binary(op);
    let mut lwrap = false;
    let mut rwrap = false;
    if let NodeKind::Unary { op: l, .. } = &left.kind {
        if operator::unary(l).precedence < this.precedence {
            lwrap = true;
        }
    }
    if let NodeKind::Binary { op: l, .. } = &left.kind {
        let lb = operator::binary(l);
        if lb.precedence < this.precedence
            || lb.precedence == this.precedence && this.associativity == operator::Associativity::Right
            || l == "??"
            || operator::is_boolean(l) && op != l
        {
            lwrap = true;
        }
    }
    if let NodeKind::Binary { op: r, .. } = &right.kind {
        let rb = operator::binary(r);
        if rb.precedence < this.precedence
            || rb.precedence == this.precedence && this.associativity == operator::Associativity::Left
            || operator::is_boolean(r) && op != r
        {
            rwrap = true;
        }
    }
    if matches!(left.kind, NodeKind::Conditional { .. }) {
        lwrap = true;
    }
    if matches!(right.kind, NodeKind::Conditional { .. }) {
        rwrap = true;
    }
    let lhs = if lwrap { format!("({})", left.to_source()) } else { left.to_source() };
    let rhs = if rwrap { format!("({})", right.to_source()) } else { right.to_source() };
    format!("{lhs} {op} {rhs}")
}
