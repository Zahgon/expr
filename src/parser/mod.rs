//! The parser: a precedence-climbing parser over the lexer's tokens.
//!
//! The parser keeps going after the first error the way the original does (later errors never replace the first
//! one, except lexing errors), so that every input reports the same error at the same position.

pub mod lexer;
pub mod operator;

use crate::ast::{Node, NodeKind};
use crate::builtin;
use crate::conf::{Config, DEFAULT_MAX_NODES};
use crate::file::{Error, Location, Source};
use crate::gostd::strconv;
use lexer::{is_valid_identifier, Kind, Lexer, Stop, Token};
use operator::Associativity;

/// A parsed expression with its source.
#[derive(Clone, Debug)]
pub struct Tree {
    pub node: Node,
    pub source: Source,
}

const EXPR: u8 = 1;
const PREDICATE: u8 = 2;
const OPTIONAL: u8 = 1 << 7;

/// The argument shapes of the builtins that take predicates.
fn predicate_args(name: &[u8]) -> Option<&'static [u8]> {
    Some(match name {
        b"all" | b"none" | b"any" | b"one" | b"filter" | b"map" | b"find" | b"findIndex" | b"findLast"
        | b"findLastIndex" | b"groupBy" => &[EXPR, PREDICATE],
        b"count" | b"sum" => &[EXPR, PREDICATE | OPTIONAL],
        b"sortBy" | b"reduce" => &[EXPR, PREDICATE, EXPR | OPTIONAL],
        _ => return None,
    })
}

struct Parser<'a> {
    lexer: Lexer,
    current: Token,
    stashed: Token,
    has_stash: bool,
    err: Option<Error>,
    config: Option<&'a Config>,
    depth: usize,
    node_count: u64,
}

/// Parses an expression with the given configuration (`None`: the defaults of a bare parse).
pub fn parse(input: &[u8], config: Option<&Config>) -> Result<Tree, Error> {
    let source = Source::new(input);
    let mut lexer = Lexer::new(source.clone());
    lexer.disable_if_operator = config.is_some_and(|c| c.disable_if_operator);
    let mut p = Parser {
        lexer,
        current: Token::default(),
        stashed: Token::default(),
        has_stash: false,
        err: None,
        config,
        depth: 0,
        node_count: 0,
    };
    p.next();
    let node = p.parse_sequence_expression();
    if !p.current.is(Kind::Eof, &[]) {
        let msg = format!("unexpected token {}", p.current);
        p.error(msg);
    }
    match (p.err, node) {
        (Some(err), _) => Err(err.bind(&source)),
        (None, Some(node)) => Ok(Tree { node, source }),
        (None, None) => Err(Error::plain("unexpected end of expression")),
    }
}

fn boxed(node: Node) -> Box<Node> {
    Box::new(node)
}

impl Parser<'_> {
    fn check_node_limit(&mut self) {
        self.node_count += 1;
        let limit = match self.config {
            None => DEFAULT_MAX_NODES,
            Some(c) => c.max_nodes,
        };
        if limit > 0 && self.node_count > limit {
            self.error("compilation failed: expression exceeds maximum allowed nodes");
        }
    }

    /// Creates a node, counting it against the node limit; `None` once an error occurred.
    fn create(&mut self, kind: Option<NodeKind>, loc: Location) -> Option<Node> {
        self.check_node_limit();
        if self.err.is_some() {
            return None;
        }
        kind.map(|k| Node::new(k, loc))
    }

    fn error(&mut self, message: impl Into<Vec<u8>>) {
        let token = self.current.clone();
        self.error_at(&token, message);
    }

    fn error_at(&mut self, token: &Token, message: impl Into<Vec<u8>>) {
        if self.err.is_none() {
            self.err = Some(Error::new(token.location, message));
        }
    }

    fn next(&mut self) {
        if self.has_stash {
            self.current = self.stashed.clone();
            self.has_stash = false;
            return;
        }
        match self.lexer.next_token() {
            Ok(token) => self.current = token,
            Err(Stop::Eof) => self.error("unexpected end of expression"),
            Err(Stop::Error(e)) => self.err = Some(e),
        }
    }

    fn expect(&mut self, kind: Kind, values: &[&str]) {
        if self.current.is(kind, values) {
            self.next();
            return;
        }
        let msg = format!("unexpected token {}", self.current);
        self.error(msg);
    }

    fn parse_sequence_expression(&mut self) -> Option<Node> {
        let mut nodes = vec![self.parse_expression(0)];
        while self.current.is(Kind::Operator, &[";"]) && self.err.is_none() {
            self.next();
            if self.current.is(Kind::Eof, &[]) {
                break;
            }
            nodes.push(self.parse_expression(0));
        }
        if nodes.len() == 1 {
            return nodes.pop().flatten();
        }
        let loc = nodes[0].as_ref().map(|n| n.loc).unwrap_or_default();
        let nodes: Option<Vec<Node>> = nodes.into_iter().collect();
        self.create(nodes.map(NodeKind::Sequence), loc)
    }

    fn parse_expression(&mut self, precedence: i32) -> Option<Node> {
        if self.err.is_some() {
            return None;
        }
        if precedence == 0 && self.current.is(Kind::Operator, &["let"]) {
            return self.parse_variable_declaration();
        }
        if precedence == 0
            && !self.config.is_some_and(|c| c.disable_if_operator)
            && self.current.is(Kind::Operator, &["if"])
        {
            return self.parse_conditional_if();
        }

        let mut node_left = self.parse_primary();
        let mut prev_operator: Vec<u8> = Vec::new();
        let mut op_token = self.current.clone();
        while op_token.is(Kind::Operator, &[]) && self.err.is_none() {
            let negate = op_token.is(Kind::Operator, &["not"]);
            let mut not_token = Token::default();
            if negate {
                let backup = self.current.clone();
                self.next();
                if operator::allowed_negate_suffix(&self.current.value) {
                    match operator::lookup_binary(&self.current.value) {
                        Some(op) if op.precedence >= precedence => {
                            not_token = self.current.clone();
                            op_token = self.current.clone();
                        }
                        _ => {
                            self.has_stash = true;
                            self.stashed = self.current.clone();
                            self.current = backup;
                            break;
                        }
                    }
                } else {
                    let msg = format!("unexpected token {}", self.current);
                    self.error(msg);
                    break;
                }
            }

            let Some(op) = operator::lookup_binary(&op_token.value).filter(|op| op.precedence >= precedence) else {
                break;
            };
            self.next();

            if op_token.value == b"|" {
                let ident = self.current.clone();
                self.expect(Kind::Identifier, &[]);
                node_left = self.parse_call(&ident, vec![node_left], true);
            } else if prev_operator == b"??" && op_token.value != b"??" {
                let msg = format!(
                    "Operator ({}) and coalesce expressions (??) cannot be mixed. Wrap either by parentheses.",
                    op_token.value_str()
                );
                self.error_at(&op_token, msg);
                break;
            } else if operator::is_comparison(&op_token.value) {
                node_left = self.parse_comparison(node_left, op_token.clone(), op.precedence);
            } else {
                let node_right = if op.associativity == Associativity::Left {
                    self.parse_expression(op.precedence + 1)
                } else {
                    self.parse_expression(op.precedence)
                };
                let kind = match (node_left, node_right) {
                    (Some(l), Some(r)) => {
                        Some(NodeKind::Binary { op: op_token.value_str(), left: boxed(l), right: boxed(r) })
                    }
                    _ => None,
                };
                node_left = Some(self.create(kind, op_token.location)?);
                if negate {
                    let kind = node_left.map(|n| NodeKind::Unary { op: "not".to_string(), node: boxed(n) });
                    node_left = Some(self.create(kind, not_token.location)?);
                }
            }
            prev_operator = op_token.value.clone();
            op_token = self.current.clone();
        }

        if precedence == 0 {
            node_left = self.parse_conditional(node_left);
        }
        node_left
    }

    fn parse_variable_declaration(&mut self) -> Option<Node> {
        self.expect(Kind::Operator, &["let"]);
        let name = self.current.clone();
        self.expect(Kind::Identifier, &[]);
        self.expect(Kind::Operator, &["="]);
        let value = self.parse_expression(0);
        self.expect(Kind::Operator, &[";"]);
        let expr = self.parse_sequence_expression();
        let kind = match (value, expr) {
            (Some(v), Some(e)) => {
                Some(NodeKind::VariableDeclarator { name: name.value_str(), value: boxed(v), expr: boxed(e) })
            }
            _ => None,
        };
        self.create(kind, name.location)
    }

    fn parse_conditional_if(&mut self) -> Option<Node> {
        self.next();
        if self.err.is_some() {
            return None;
        }
        let cond = self.parse_expression(0);
        self.expect(Kind::Bracket, &["{"]);
        let exp1 = self.parse_sequence_expression();
        self.expect(Kind::Bracket, &["}"]);
        self.expect(Kind::Operator, &["else"]);
        let exp2 = if self.current.is(Kind::Operator, &["if"]) {
            self.parse_conditional_if()
        } else {
            self.expect(Kind::Bracket, &["{"]);
            let e = self.parse_sequence_expression();
            self.expect(Kind::Bracket, &["}"]);
            e
        };
        // The original builds this node without counting it or giving it a location.
        Some(Node::new(
            NodeKind::Conditional { ternary: false, cond: boxed(cond?), exp1: boxed(exp1?), exp2: boxed(exp2?) },
            Location::default(),
        ))
    }

    fn parse_conditional(&mut self, mut node: Option<Node>) -> Option<Node> {
        while self.current.is(Kind::Operator, &["?"]) && self.err.is_none() {
            self.next();
            let (exp1, exp2) = if !self.current.is(Kind::Operator, &[":"]) {
                let e1 = self.parse_expression(0);
                self.expect(Kind::Operator, &[":"]);
                (e1, self.parse_expression(0))
            } else {
                self.next();
                (node.clone(), self.parse_expression(0))
            };
            let kind = match (node, exp1, exp2) {
                (Some(c), Some(e1), Some(e2)) => {
                    Some(NodeKind::Conditional { ternary: true, cond: boxed(c), exp1: boxed(e1), exp2: boxed(e2) })
                }
                _ => None,
            };
            let loc = self.current.location;
            node = Some(self.create(kind, loc)?);
        }
        node
    }

    fn parse_primary(&mut self) -> Option<Node> {
        let token = self.current.clone();

        if token.is(Kind::Operator, &["not", "!", "-", "+"]) {
            let op = operator::unary(&token.value_str());
            self.next();
            let expr = self.parse_expression(op.precedence);
            let kind = expr.map(|e| NodeKind::Unary { op: token.value_str(), node: boxed(e) });
            let node = self.create(kind, token.location)?;
            return self.parse_postfix_expression(Some(node));
        }

        if token.is(Kind::Bracket, &["("]) {
            self.next();
            let expr = self.parse_sequence_expression();
            self.expect(Kind::Bracket, &[")"]);
            return self.parse_postfix_expression(expr);
        }

        if self.depth > 0 && (token.is(Kind::Operator, &["#"]) || token.is(Kind::Operator, &["."])) {
            let mut name = String::new();
            if token.is(Kind::Operator, &["#"]) {
                self.next();
                if self.current.is(Kind::Identifier, &[]) {
                    name = self.current.value_str();
                    self.next();
                }
            }
            let node = self.create(Some(NodeKind::Pointer(name)), token.location)?;
            return self.parse_postfix_expression(Some(node));
        }

        if token.is(Kind::Operator, &["::"]) {
            self.next();
            let ident = self.current.clone();
            self.expect(Kind::Identifier, &[]);
            let call = self.parse_call(&ident, Vec::new(), false);
            return self.parse_postfix_expression(call);
        }

        self.parse_secondary()
    }

    fn parse_secondary(&mut self) -> Option<Node> {
        let token = self.current.clone();
        let node = match token.kind {
            Kind::Identifier => {
                self.next();
                match token.value.as_slice() {
                    b"true" => return self.create(Some(NodeKind::Bool(true)), token.location),
                    b"false" => return self.create(Some(NodeKind::Bool(false)), token.location),
                    b"nil" => return self.create(Some(NodeKind::Nil), token.location),
                    _ => {
                        if self.current.is(Kind::Bracket, &["("]) {
                            self.parse_call(&token, Vec::new(), true)
                        } else {
                            Some(self.create(Some(NodeKind::Identifier(token.value_str())), token.location)?)
                        }
                    }
                }
            }
            Kind::Number => {
                self.next();
                let mut node = self.parse_number(&token);
                if let Some(n) = node.as_mut() {
                    n.loc = token.location;
                }
                return node;
            }
            Kind::String => {
                self.next();
                Some(self.create(Some(NodeKind::String(token.value.clone())), token.location)?)
            }
            Kind::Bytes => {
                self.next();
                Some(self.create(Some(NodeKind::Bytes(token.value.clone())), token.location)?)
            }
            _ => {
                if token.is(Kind::Bracket, &["["]) {
                    self.parse_array_expression(&token)
                } else if token.is(Kind::Bracket, &["{"]) {
                    self.parse_map_expression(&token)
                } else {
                    let msg = format!("unexpected token {token}");
                    self.error(msg);
                    None
                }
            }
        };
        self.parse_postfix_expression(node)
    }

    fn parse_number(&mut self, token: &Token) -> Option<Node> {
        let value: Vec<u8> = token.value.iter().copied().filter(|&b| b != b'_').collect();
        let lower = value.to_ascii_lowercase();
        if lower.starts_with(b"0x") {
            let n = self.parse_int_literal(&value, 0, "hex");
            self.integer_node(n)
        } else if lower.iter().any(|&b| b == b'.' || b == b'e') {
            match strconv::parse_float(&value, 64) {
                Ok(f) => self.create(Some(NodeKind::Float(f)), self.current.location),
                Err(e) => {
                    self.error(format!("invalid float literal: {e}"));
                    None
                }
            }
        } else if lower.starts_with(b"0b") {
            let n = self.parse_int_literal(&value, 0, "binary");
            self.integer_node(n)
        } else if lower.starts_with(b"0o") {
            let n = self.parse_int_literal(&value, 0, "octal");
            self.integer_node(n)
        } else {
            let n = self.parse_int_literal(&value, 10, "integer");
            self.integer_node(n)
        }
    }

    fn parse_int_literal(&mut self, value: &[u8], base: u32, what: &str) -> i64 {
        match strconv::parse_int(value, base, 64) {
            Ok(n) => n,
            Err(e) => {
                self.error(format!("invalid {what} literal: {e}"));
                0
            }
        }
    }

    fn integer_node(&mut self, n: i64) -> Option<Node> {
        let loc = self.current.location;
        self.create(Some(NodeKind::Integer(n)), loc)
    }

    fn parse_call(&mut self, token: &Token, mut arguments: Vec<Option<Node>>, check_overrides: bool) -> Option<Node> {
        let overridden = check_overrides && self.config.is_some_and(|c| c.is_overridden(&token.value));
        let name = token.value_str();

        if let Some(shape) = predicate_args(&token.value).filter(|_| !overridden) {
            self.expect(Kind::Bracket, &["("]);
            let args = shape.get(arguments.len()..).unwrap_or(&[]);
            for (i, &arg) in args.iter().enumerate() {
                if arg & OPTIONAL == OPTIONAL {
                    if self.current.is(Kind::Bracket, &[")"]) {
                        break;
                    }
                } else if self.current.is(Kind::Bracket, &[")"]) {
                    self.error(format!("expected at least {} arguments", args.len()));
                }
                if i > 0 {
                    self.expect(Kind::Operator, &[","]);
                }
                let node = if arg & EXPR == EXPR { self.parse_expression(0) } else { self.parse_predicate() };
                arguments.push(node);
            }
            if self.current.is(Kind::Operator, &[","]) {
                self.next();
            }
            self.expect(Kind::Bracket, &[")"]);
            let args: Option<Vec<Node>> = arguments.into_iter().collect();
            let kind = args.map(|args| NodeKind::Builtin { name, args, throws: false, map: None, threshold: None });
            return self.create(kind, token.location);
        }

        let disabled = self.config.is_some_and(|c| c.is_disabled(&name));
        if builtin::index(&name).is_some() && !disabled && !overridden {
            let args = self.parse_arguments(arguments);
            let kind = args.map(|args| NodeKind::Builtin { name, args, throws: false, map: None, threshold: None });
            return self.create(kind, token.location);
        }

        let callee = self.create(Some(NodeKind::Identifier(name)), token.location)?;
        let args = self.parse_arguments(arguments);
        let kind = args.map(|args| NodeKind::Call { callee: boxed(callee), args });
        self.create(kind, token.location)
    }

    fn parse_arguments(&mut self, mut arguments: Vec<Option<Node>>) -> Option<Vec<Node>> {
        let offset = arguments.len();
        self.expect(Kind::Bracket, &["("]);
        while !self.current.is(Kind::Bracket, &[")"]) && self.err.is_none() {
            if arguments.len() > offset {
                self.expect(Kind::Operator, &[","]);
            }
            if self.current.is(Kind::Bracket, &[")"]) {
                break;
            }
            arguments.push(self.parse_expression(0));
        }
        self.expect(Kind::Bracket, &[")"]);
        arguments.into_iter().collect()
    }

    fn parse_predicate(&mut self) -> Option<Node> {
        let start = self.current.clone();
        let with_brackets = self.current.is(Kind::Bracket, &["{"]);
        if with_brackets {
            self.next();
        }
        self.depth += 1;
        let node = if with_brackets {
            self.parse_sequence_expression()
        } else {
            let node = self.parse_expression(0);
            if self.current.is(Kind::Operator, &[";"]) {
                self.error("wrap predicate with brackets { and }");
            }
            node
        };
        self.depth -= 1;
        if with_brackets {
            self.expect(Kind::Bracket, &["}"]);
        }
        let kind = node.map(|n| NodeKind::Predicate(boxed(n)));
        self.create(kind, start.location)
    }

    fn parse_array_expression(&mut self, token: &Token) -> Option<Node> {
        let mut nodes = Vec::new();
        self.expect(Kind::Bracket, &["["]);
        while !self.current.is(Kind::Bracket, &["]"]) && self.err.is_none() {
            if !nodes.is_empty() {
                self.expect(Kind::Operator, &[","]);
                if self.current.is(Kind::Bracket, &["]"]) {
                    break;
                }
            }
            nodes.push(self.parse_expression(0));
        }
        self.expect(Kind::Bracket, &["]"]);
        let nodes: Option<Vec<Node>> = nodes.into_iter().collect();
        self.create(nodes.map(NodeKind::Array), token.location)
    }

    fn parse_map_expression(&mut self, token: &Token) -> Option<Node> {
        self.expect(Kind::Bracket, &["{"]);
        let mut pairs = Vec::new();
        while !self.current.is(Kind::Bracket, &["}"]) && self.err.is_none() {
            if !pairs.is_empty() {
                self.expect(Kind::Operator, &[","]);
                if self.current.is(Kind::Bracket, &["}"]) {
                    break;
                }
                if self.current.is(Kind::Operator, &[","]) {
                    let msg = format!("unexpected token {}", self.current);
                    self.error(msg);
                }
            }
            let key = if self.current.is(Kind::Number, &[])
                || self.current.is(Kind::String, &[])
                || self.current.is(Kind::Identifier, &[])
            {
                let loc = self.current.location;
                let key = self.create(Some(NodeKind::String(self.current.value.clone())), loc)?;
                self.next();
                Some(key)
            } else if self.current.is(Kind::Bracket, &["("]) {
                self.parse_expression(0)
            } else {
                let msg = format!(
                    "a map key must be a quoted string, a number, a identifier, or an expression enclosed in \
                     parentheses (unexpected token {})",
                    self.current
                );
                self.error(msg);
                None
            };
            self.expect(Kind::Operator, &[":"]);
            let value = self.parse_expression(0);
            let kind = match (key, value) {
                (Some(k), Some(v)) => Some(NodeKind::Pair { key: boxed(k), value: boxed(v) }),
                _ => None,
            };
            pairs.push(self.create(kind, token.location)?);
        }
        self.expect(Kind::Bracket, &["}"]);
        self.create(Some(NodeKind::Map(pairs)), token.location)
    }

    fn parse_postfix_expression(&mut self, mut node: Option<Node>) -> Option<Node> {
        let mut postfix = self.current.clone();
        while (postfix.is(Kind::Operator, &[]) || postfix.is(Kind::Bracket, &[])) && self.err.is_none() {
            let optional = postfix.value == b"?.";
            if postfix.value == b"." || postfix.value == b"?." {
                self.next();
                let property_token = self.current.clone();
                if optional && property_token.is(Kind::Bracket, &["["]) {
                    postfix = property_token;
                    node = self.parse_index(node, &postfix, true);
                    node.as_ref()?;
                    postfix = self.current.clone();
                    continue;
                }
                self.next();
                if property_token.kind != Kind::Identifier
                    && (property_token.kind != Kind::Operator || !is_valid_identifier(&property_token.value))
                {
                    self.error("expected name");
                }
                let property =
                    self.create(Some(NodeKind::String(property_token.value.clone())), property_token.location)?;
                let (base, is_chain) = match node {
                    Some(Node { kind: NodeKind::Chain(inner), .. }) => (Some(*inner), true),
                    other => (other, false),
                };
                let kind = base.map(|b| NodeKind::Member {
                    node: boxed(b),
                    property: boxed(property),
                    optional,
                    method: false,
                });
                let mut member = self.create(kind, property_token.location)?;
                let mut result = if self.current.is(Kind::Bracket, &["("]) {
                    if let NodeKind::Member { method, .. } = &mut member.kind {
                        *method = true;
                    }
                    let args = self.parse_arguments(Vec::new());
                    let kind = args.map(|args| NodeKind::Call { callee: boxed(member), args });
                    self.create(kind, property_token.location)?
                } else {
                    member
                };
                if is_chain || optional {
                    result = self.create(Some(NodeKind::Chain(boxed(result))), property_token.location)?;
                }
                node = Some(result);
            } else if postfix.value == b"[" {
                node = self.parse_index(node, &postfix, false);
                node.as_ref()?;
            } else {
                break;
            }
            postfix = self.current.clone();
        }
        node
    }

    /// Parses `[index]`, `[from:to]` and their optional forms after `postfix`, the `[` token.
    fn parse_index(&mut self, node: Option<Node>, postfix: &Token, optional: bool) -> Option<Node> {
        self.next();
        if self.current.is(Kind::Operator, &[":"]) {
            self.next();
            let to = if !self.current.is(Kind::Bracket, &["]"]) { self.parse_expression(0) } else { None };
            let kind = node.map(|n| NodeKind::Slice { node: boxed(n), from: None, to: to.map(boxed) });
            let slice = self.create(kind, postfix.location)?;
            self.expect(Kind::Bracket, &["]"]);
            return Some(slice);
        }
        let from = self.parse_expression(0);
        if self.current.is(Kind::Operator, &[":"]) {
            self.next();
            let to = if !self.current.is(Kind::Bracket, &["]"]) { self.parse_expression(0) } else { None };
            let kind = match (node, from) {
                (Some(n), Some(f)) => Some(NodeKind::Slice { node: boxed(n), from: Some(boxed(f)), to: to.map(boxed) }),
                _ => None,
            };
            let slice = self.create(kind, postfix.location)?;
            self.expect(Kind::Bracket, &["]"]);
            return Some(slice);
        }
        let kind = match (node, from) {
            (Some(n), Some(f)) => {
                Some(NodeKind::Member { node: boxed(n), property: boxed(f), optional, method: false })
            }
            _ => None,
        };
        let mut member = self.create(kind, postfix.location)?;
        if optional {
            member = self.create(Some(NodeKind::Chain(boxed(member))), postfix.location)?;
        }
        self.expect(Kind::Bracket, &["]"]);
        Some(member)
    }

    fn parse_comparison(&mut self, left: Option<Node>, mut token: Token, precedence: i32) -> Option<Node> {
        let mut left = left;
        let mut root: Option<Node> = None;
        loop {
            let comparator = self.parse_expression(precedence + 1);
            let kind = match (left, comparator.clone()) {
                (Some(l), Some(r)) => Some(NodeKind::Binary { op: token.value_str(), left: boxed(l), right: boxed(r) }),
                _ => None,
            };
            let cmp = self.create(kind, token.location)?;
            root = Some(match root {
                None => cmp,
                Some(r) => self.create(
                    Some(NodeKind::Binary { op: "&&".to_string(), left: boxed(r), right: boxed(cmp) }),
                    token.location,
                )?,
            });
            left = comparator;
            token = self.current.clone();
            if !(token.is(Kind::Operator, &[]) && operator::is_comparison(&token.value) && self.err.is_none()) {
                break;
            }
            self.next();
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(input: &str) -> String {
        match parse(input.as_bytes(), None) {
            Ok(tree) => tree.node.to_source(),
            Err(e) => format!("error: {e}"),
        }
    }

    #[test]
    fn prints_parsed_trees() {
        assert_eq!(src("a + b * c"), "a + b * c");
        assert_eq!(src("(a + b) * c"), "(a + b) * c");
        assert_eq!(src("1 < x < 3"), "1 < x && x < 3");
        assert_eq!(src("a?.b.c"), "a?.b.c");
        assert_eq!(src("x | filter(# > 1)"), "filter(x, # > 1)");
        assert_eq!(src("not (a in b)"), "not (a in b)");
        assert_eq!(src("a not in b"), "not (a in b)");
        assert_eq!(src("let x = 1; x + 2"), "let x = 1; x + 2");
        assert_eq!(src("{a: 1, 'b c': 2}"), "{a: 1, \"b c\": 2}");
        assert_eq!(src("if a { 1 } else { 2 }"), "if a { 1 } else { 2 }");
        assert_eq!(src("a ? b : c ? d : e"), "a ? b : (c ? d : e)");
        assert_eq!(src("arr[1:]"), "arr[1:]");
        assert_eq!(src("2 ** 3 ** 2"), "2 ** 3 ** 2");
    }

    #[test]
    fn reports_parse_errors() {
        assert_eq!(src("1 +"), "error: unexpected token EOF (1:3)\n | 1 +\n | ..^");
        assert_eq!(src("a ?? b + c"), "error: Operator (+) and coalesce expressions (??) cannot be mixed. Wrap either by parentheses. (1:8)\n | a ?? b + c\n | .......^");
        assert_eq!(src("foo."), "error: unexpected end of expression (1:4)\n | foo.\n | ...^");
        assert_eq!(src("all()"), "error: expected at least 2 arguments (1:5)\n | all()\n | ....^");
        assert_eq!(
            src("99999999999999999999 + 1"),
            "error: invalid integer literal: strconv.ParseInt: parsing \"99999999999999999999\": value out of range (1:22)\n | 99999999999999999999 + 1\n | .....................^"
        );
    }
}
