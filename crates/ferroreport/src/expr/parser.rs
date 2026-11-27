//! Pratt parser producing an [`Expr`] tree.

use super::lexer::{tokenize, GPart, Token};
use crate::value::{JavaType, Value};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// Groovy `**`.
    Pow,
    /// Groovy `<=>`.
    Cmp,
    /// Groovy `in`.
    In,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Plus,
    Not,
}

/// Parsed expression tree.
#[derive(Debug, Clone)]
pub enum Expr {
    Lit(Value),
    Field(Box<str>),
    Param(Box<str>),
    Var(Box<str>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Elvis(Box<Expr>, Box<Expr>),
    /// `target.name(args)`; `safe` for `?.` (null when the target is null), `property` for
    /// Groovy's `target.name` (a map's entry, or the getter).
    Method {
        target: Box<Expr>,
        name: Box<str>,
        args: Vec<Expr>,
        safe: bool,
        property: bool,
    },
    Static {
        class: Box<str>,
        name: Box<str>,
        args: Vec<Expr>,
    },
    StaticField {
        class: Box<str>,
        name: Box<str>,
    },
    New {
        class: Box<str>,
        args: Vec<Expr>,
    },
    Cast(JavaType, Box<Expr>),
    /// Groovy `[a, b]`.
    List(Vec<Expr>),
    /// Groovy `[key: value]`.
    Map(Vec<(Expr, Expr)>),
    /// Groovy `a..b` (and `a..<b`, without the end).
    Range(Box<Expr>, Box<Expr>, bool),
    /// Groovy `{ x -> body }`, given to a method such as `findAll` or `collect`.
    Closure {
        params: Vec<Box<str>>,
        body: Box<Expr>,
    },
    /// A closure's parameter (`it`, `x`).
    Local(Box<str>),
    /// `target[index]`.
    Index(Box<Expr>, Box<Expr>),
    /// Groovy `value as Type`.
    As(JavaType, Box<Expr>),
    /// A Groovy string with expressions in it: the pieces, joined.
    GString(Vec<Expr>),
}

impl Expr {
    /// Calls `f` on this expression and on every expression inside it, outermost first.
    pub(crate) fn walk_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        f(self);
        match self {
            Expr::Lit(_) | Expr::Field(_) | Expr::Param(_) | Expr::Var(_) | Expr::StaticField { .. } | Expr::Local(_) => {}
            Expr::Unary(_, e) | Expr::Cast(_, e) | Expr::As(_, e) => e.walk_mut(f),
            Expr::Closure { body, .. } => body.walk_mut(f),
            Expr::List(items) | Expr::GString(items) => items.iter_mut().for_each(|e| e.walk_mut(f)),
            Expr::Map(entries) => entries.iter_mut().for_each(|(k, v)| {
                k.walk_mut(f);
                v.walk_mut(f);
            }),
            Expr::Binary(_, a, b)
            | Expr::And(a, b)
            | Expr::Or(a, b)
            | Expr::Elvis(a, b)
            | Expr::Range(a, b, _)
            | Expr::Index(a, b) => {
                a.walk_mut(f);
                b.walk_mut(f);
            }
            Expr::Cond(a, b, c) => {
                a.walk_mut(f);
                b.walk_mut(f);
                c.walk_mut(f);
            }
            Expr::Method { target, args, .. } => {
                target.walk_mut(f);
                args.iter_mut().for_each(|a| a.walk_mut(f));
            }
            Expr::Static { args, .. } | Expr::New { args, .. } => args.iter_mut().for_each(|a| a.walk_mut(f)),
        }
    }

    /// The names of the variables the expression reads.
    pub fn variables(&self, out: &mut Vec<Box<str>>) {
        match self {
            Expr::Var(name) => out.push(name.clone()),
            Expr::Lit(_) | Expr::Field(_) | Expr::Param(_) | Expr::StaticField { .. } | Expr::Local(_) => {}
            Expr::Unary(_, e) | Expr::Cast(_, e) | Expr::As(_, e) => e.variables(out),
            Expr::Closure { body, .. } => body.variables(out),
            Expr::List(items) | Expr::GString(items) => items.iter().for_each(|e| e.variables(out)),
            Expr::Map(entries) => entries.iter().for_each(|(k, v)| {
                k.variables(out);
                v.variables(out);
            }),
            Expr::Binary(_, a, b)
            | Expr::And(a, b)
            | Expr::Or(a, b)
            | Expr::Elvis(a, b)
            | Expr::Range(a, b, _)
            | Expr::Index(a, b) => {
                a.variables(out);
                b.variables(out);
            }
            Expr::Cond(a, b, c) => {
                a.variables(out);
                b.variables(out);
                c.variables(out);
            }
            Expr::Method { target, args, .. } => {
                target.variables(out);
                args.iter().for_each(|a| a.variables(out));
            }
            Expr::Static { args, .. } | Expr::New { args, .. } => args.iter().for_each(|a| a.variables(out)),
        }
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// How deep the expression being built is nested (parentheses, operators, calls).
    depth: usize,
    /// The parameters of the closures being read, innermost last.
    locals: Vec<Vec<String>>,
    groovy: bool,
}

/// Deepest nesting accepted in an expression. Evaluation recurses once per level, so a
/// template cannot exhaust the stack with `((((...))))` or a chain of thousands of `+`.
const MAX_DEPTH: usize = 256;

const TYPE_NAMES: &[&str] = &[
    "String",
    "Integer",
    "Long",
    "Short",
    "Byte",
    "Double",
    "Float",
    "BigDecimal",
    "BigInteger",
    "Number",
    "Boolean",
    "Date",
    "Timestamp",
    "Object",
    "int",
    "long",
    "double",
    "float",
    "boolean",
    "short",
    "byte",
    "Comparable",
    "List",
    "Collection",
    "Map",
    "JRDataSource",
    "JRRewindableDataSource",
    "JsonDataSource",
    "JRBeanCollectionDataSource",
    "JRMapCollectionDataSource",
];

/// Parses expression source text.
pub fn parse_expr(src: &str, groovy: bool) -> Result<Expr, String> {
    parse_with(src, groovy, Vec::new(), 0)
}

fn parse_with(src: &str, groovy: bool, locals: Vec<Vec<String>>, depth: usize) -> Result<Expr, String> {
    let tokens = tokenize(src, groovy)?;
    let mut p = Parser { tokens, pos: 0, depth, locals, groovy };
    let e = p.expr(0)?;
    match p.peek() {
        Token::Eof => Ok(e),
        t => Err(format!("unexpected token {t:?}")),
    }
}

fn simple_name(qualified: &str) -> &str {
    qualified.rsplit('.').next().unwrap_or(qualified)
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }
    fn peek_at(&self, n: usize) -> &Token {
        self.tokens.get(self.pos + n).unwrap_or(&Token::Eof)
    }
    fn next(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }
    fn expect(&mut self, t: Token) -> Result<(), String> {
        let got = self.next();
        if got == t {
            Ok(())
        } else {
            Err(format!("expected {t:?}, found {got:?}"))
        }
    }

    /// Binding power of infix operators: (left, right).
    fn infix_bp(t: &Token) -> Option<(u8, u8)> {
        Some(match t {
            Token::Op("?") | Token::Op("?:") => (2, 1),
            Token::Op("||") => (3, 4),
            Token::Op("&&") => (5, 6),
            Token::Op("==") | Token::Op("!=") => (7, 8),
            Token::Op("<") | Token::Op("<=") | Token::Op(">") | Token::Op(">=") | Token::Op("<=>") => (9, 10),
            Token::Ident(w) if w == "in" => (9, 10),
            Token::Op("..") | Token::Op("..<") => (10, 11),
            Token::Op("+") | Token::Op("-") => (11, 12),
            Token::Op("*") | Token::Op("/") | Token::Op("%") => (13, 14),
            Token::Op("**") => (16, 15),
            _ => return None,
        })
    }

    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(format!("expression nested more than {MAX_DEPTH} levels deep"));
        }
        Ok(())
    }

    fn expr(&mut self, min_bp: u8) -> Result<Expr, String> {
        let start = self.depth;
        let r = self.expr_inner(min_bp);
        self.depth = start;
        r
    }

    fn expr_inner(&mut self, min_bp: u8) -> Result<Expr, String> {
        self.enter()?;
        let mut lhs = self.unary()?;
        loop {
            let op = self.peek().clone();
            let Some((lbp, rbp)) = Self::infix_bp(&op) else { break };
            if lbp < min_bp {
                break;
            }
            // Each operator in a chain nests the expression one level deeper.
            self.enter()?;
            self.next();
            lhs = match op {
                Token::Op("?") => {
                    let then = self.expr(0)?;
                    self.expect(Token::Op(":"))?;
                    let otherwise = self.expr(rbp)?;
                    Expr::Cond(Box::new(lhs), Box::new(then), Box::new(otherwise))
                }
                Token::Op("?:") => Expr::Elvis(Box::new(lhs), Box::new(self.expr(rbp)?)),
                Token::Op("&&") => Expr::And(Box::new(lhs), Box::new(self.expr(rbp)?)),
                Token::Op("||") => Expr::Or(Box::new(lhs), Box::new(self.expr(rbp)?)),
                Token::Op("..") => Expr::Range(Box::new(lhs), Box::new(self.expr(rbp)?), false),
                Token::Op("..<") => Expr::Range(Box::new(lhs), Box::new(self.expr(rbp)?), true),
                Token::Ident(_) => Expr::Binary(BinOp::In, Box::new(lhs), Box::new(self.expr(rbp)?)),
                Token::Op(o) => {
                    let bin = match o {
                        "+" => BinOp::Add,
                        "-" => BinOp::Sub,
                        "*" => BinOp::Mul,
                        "/" => BinOp::Div,
                        "%" => BinOp::Rem,
                        "==" => BinOp::Eq,
                        "!=" => BinOp::Ne,
                        "<" => BinOp::Lt,
                        "<=" => BinOp::Le,
                        ">" => BinOp::Gt,
                        ">=" => BinOp::Ge,
                        "**" => BinOp::Pow,
                        "<=>" => BinOp::Cmp,
                        _ => return Err(format!("unknown operator {o}")),
                    };
                    Expr::Binary(bin, Box::new(lhs), Box::new(self.expr(rbp)?))
                }
                _ => unreachable!(),
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        let start = self.depth;
        let r = self.unary_inner();
        self.depth = start;
        r
    }

    fn unary_inner(&mut self) -> Result<Expr, String> {
        self.enter()?;
        match self.peek().clone() {
            Token::Op("-") => {
                self.next();
                Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)))
            }
            Token::Op("+") => {
                self.next();
                Ok(Expr::Unary(UnOp::Plus, Box::new(self.unary()?)))
            }
            Token::Op("!") => {
                self.next();
                Ok(Expr::Unary(UnOp::Not, Box::new(self.unary()?)))
            }
            Token::LParen if self.is_cast() => {
                self.next();
                let name = self.qualified_name()?;
                self.expect(Token::RParen)?;
                let inner = self.unary()?;
                Ok(Expr::Cast(JavaType::from_class_name(&name), Box::new(inner)))
            }
            _ => self.postfix(),
        }
    }

    /// `(Type) operand` — a parenthesised known type name followed by something that can
    /// start an operand.
    fn is_cast(&self) -> bool {
        let mut n = 1;
        let mut last;
        loop {
            match self.peek_at(n) {
                Token::Ident(s) => last = s.clone(),
                _ => return false,
            }
            n += 1;
            match self.peek_at(n) {
                Token::Dot => n += 1,
                Token::RParen => break,
                _ => return false,
            }
        }
        // `(AnyClass) $P{x}`: a name in parentheses before a reference can only be a cast.
        if matches!(self.peek_at(n + 1), Token::Field(_) | Token::Param(_) | Token::Var(_)) {
            return true;
        }
        if !TYPE_NAMES.contains(&last.as_str()) {
            return false;
        }
        // A primitive type can be cast to before a sign: `(short) -3`.
        let primitive = last.chars().next().is_some_and(char::is_lowercase);
        if primitive && matches!(self.peek_at(n + 1), Token::Op("-") | Token::Op("+")) {
            return true;
        }
        matches!(
            self.peek_at(n + 1),
            Token::Ident(_)
                | Token::Field(_)
                | Token::Param(_)
                | Token::Var(_)
                | Token::Str(_)
                | Token::Num(_)
                | Token::LParen
                | Token::Op("!")
        )
    }

    fn qualified_name(&mut self) -> Result<String, String> {
        let mut name = match self.next() {
            Token::Ident(s) => s,
            t => return Err(format!("expected identifier, found {t:?}")),
        };
        while self.peek() == &Token::Dot && matches!(self.peek_at(1), Token::Ident(_)) {
            self.next();
            if let Token::Ident(s) = self.next() {
                name.push('.');
                name.push_str(&s);
            }
        }
        Ok(name)
    }

    fn args(&mut self) -> Result<Vec<Expr>, String> {
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        if self.peek() == &Token::RParen {
            self.next();
            return Ok(args);
        }
        loop {
            args.push(self.expr(0)?);
            match self.next() {
                Token::Comma => continue,
                Token::RParen => break,
                t => return Err(format!("expected `,` or `)`, found {t:?}")),
            }
        }
        Ok(args)
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        loop {
            match self.peek().clone() {
                Token::Dot | Token::Op("?.") => {
                    let safe = self.next() == Token::Op("?.");
                    let name = match self.next() {
                        Token::Ident(s) => s,
                        t => return Err(format!("expected member name, found {t:?}")),
                    };
                    let mut property = true;
                    let mut args = Vec::new();
                    if self.peek() == &Token::LParen {
                        args = self.args()?;
                        property = false;
                    }
                    // Groovy: a closure after the call is its last argument, `list.findAll { ... }`.
                    if self.peek() == &Token::LBrace {
                        args.push(self.closure()?);
                        property = false;
                    }
                    e = Expr::Method { target: Box::new(e), name: name.into(), args, safe, property };
                }
                Token::LBracket => {
                    self.next();
                    let index = self.expr(0)?;
                    self.expect(Token::RBracket)?;
                    e = Expr::Index(Box::new(e), Box::new(index));
                }
                Token::Ident(w) if w == "as" && matches!(self.peek_at(1), Token::Ident(_)) => {
                    self.next();
                    let class = self.qualified_name()?;
                    e = Expr::As(JavaType::from_class_name(&class), Box::new(e));
                }
                _ => break,
            }
        }
        Ok(e)
    }

    /// `{ a, b -> body }`, or `{ body }` with the parameter `it`.
    #[inline(never)]
    fn closure(&mut self) -> Result<Expr, String> {
        self.enter()?;
        self.expect(Token::LBrace)?;
        // Parameters are names (a type may come before each) up to `->`.
        let mut n = 0;
        let mut arrow = false;
        while let tok @ (Token::Ident(_) | Token::Comma | Token::Op("->")) = self.peek_at(n) {
            if tok == &Token::Op("->") {
                arrow = true;
                break;
            }
            n += 1;
        }
        let mut params = Vec::new();
        if arrow {
            let mut last = None;
            for _ in 0..n {
                match self.next() {
                    Token::Ident(s) => last = Some(s),
                    _ => params.extend(last.take()),
                }
            }
            params.extend(last.take());
            self.next();
        } else {
            params.push("it".to_string());
        }
        self.locals.push(params.clone());
        let body = self.expr(0);
        self.locals.pop();
        let body = body?;
        self.expect(Token::RBrace)?;
        Ok(Expr::Closure { params: params.into_iter().map(Into::into).collect(), body: Box::new(body) })
    }

    /// `[a, b]`, `[key: value]`, `[]` and `[:]`.
    #[inline(never)]
    fn list_or_map(&mut self) -> Result<Expr, String> {
        if self.peek() == &Token::RBracket {
            self.next();
            return Ok(Expr::List(Vec::new()));
        }
        if self.peek() == &Token::Op(":") && self.peek_at(1) == &Token::RBracket {
            self.next();
            self.next();
            return Ok(Expr::Map(Vec::new()));
        }
        let is_map = matches!(self.peek(), Token::Ident(_) | Token::Str(_) | Token::Num(_)) && self.peek_at(1) == &Token::Op(":");
        if is_map {
            let mut entries = Vec::new();
            loop {
                // A bare name as a key is its text.
                let key = match self.next() {
                    Token::Ident(s) | Token::Str(s) => Expr::Lit(Value::str(s)),
                    Token::Num(n) => Expr::Lit(n.into_value()),
                    t => return Err(format!("expected a map key, found {t:?}")),
                };
                self.expect(Token::Op(":"))?;
                entries.push((key, self.expr(0)?));
                match self.next() {
                    Token::Comma if self.peek() == &Token::RBracket => {
                        self.next();
                        break;
                    }
                    Token::Comma => continue,
                    Token::RBracket => break,
                    t => return Err(format!("expected `,` or `]`, found {t:?}")),
                }
            }
            return Ok(Expr::Map(entries));
        }
        let mut items = Vec::new();
        loop {
            items.push(self.expr(0)?);
            match self.next() {
                Token::Comma if self.peek() == &Token::RBracket => {
                    self.next();
                    break;
                }
                Token::Comma => continue,
                Token::RBracket => break,
                t => return Err(format!("expected `,` or `]`, found {t:?}")),
            }
        }
        Ok(Expr::List(items))
    }

    /// A Groovy string: its text and the expressions in it.
    #[inline(never)]
    fn gstring(&mut self, parts: Vec<GPart>) -> Result<Expr, String> {
        let mut pieces = Vec::new();
        for part in parts {
            match part {
                GPart::Text(t) if t.is_empty() => {}
                GPart::Text(t) => pieces.push(Expr::Lit(Value::str(t))),
                GPart::Code(code) => pieces.push(parse_with(&code, self.groovy, self.locals.clone(), self.depth + 1)?),
            }
        }
        Ok(Expr::GString(pieces))
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.iter().any(|frame| frame.iter().any(|p| p == name))
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Token::Num(n) => Ok(Expr::Lit(n.into_value())),
            Token::Str(s) => Ok(Expr::Lit(Value::str(s))),
            Token::Field(n) => Ok(Expr::Field(n.into())),
            Token::Param(n) => Ok(Expr::Param(n.into())),
            Token::Var(n) => Ok(Expr::Var(n.into())),
            // `$R{key}` is `str("key")`.
            Token::Resource(key) => {
                Ok(Expr::Static { class: "".into(), name: "str".into(), args: vec![Expr::Lit(Value::str(key))] })
            }
            Token::LParen => {
                let e = self.expr(0)?;
                self.expect(Token::RParen)?;
                Ok(e)
            }
            Token::LBracket => self.list_or_map(),
            Token::LBrace => {
                self.pos -= 1;
                self.closure()
            }
            Token::GString(parts) => self.gstring(parts),
            Token::Ident(id) if self.is_local(&id) => Ok(Expr::Local(id.into())),
            Token::Ident(id) => match id.as_str() {
                "true" => Ok(Expr::Lit(Value::Bool(true))),
                "false" => Ok(Expr::Lit(Value::Bool(false))),
                "null" => Ok(Expr::Lit(Value::Null)),
                "new" => {
                    let class = self.qualified_name()?;
                    let args = self.args()?;
                    Ok(Expr::New { class: simple_name(&class).into(), args })
                }
                _ => {
                    // Qualified static access: [pkg.]Class.member[(args)]
                    let mut parts = vec![id];
                    while self.peek() == &Token::Dot && matches!(self.peek_at(1), Token::Ident(_)) {
                        // Stop before a lowercase segment that is followed by `(` only when we
                        // already have a class name (handled below by splitting).
                        self.next();
                        if let Token::Ident(s) = self.next() {
                            parts.push(s);
                        }
                        if self.peek() == &Token::LParen {
                            break;
                        }
                    }
                    // The functions every expression has: msg(pattern, args...) and str(key).
                    // And JasperReports' function library: IF(...), SUM(...), DATEFORMAT(...).
                    let library = parts.len() == 1 && super::functions::is_function(&parts[0]);
                    if (library || parts.len() == 1 && matches!(parts[0].as_str(), "msg" | "str"))
                        && self.peek() == &Token::LParen
                    {
                        let args = self.args()?;
                        return Ok(Expr::Static { class: "".into(), name: parts.remove(0).into(), args });
                    }
                    if parts.len() < 2 {
                        return Err(format!("unknown identifier `{}`", parts[0]));
                    }
                    let member = parts.pop().unwrap_or_default();
                    let class = parts.last().cloned().unwrap_or_default();
                    if self.peek() == &Token::LParen {
                        let args = self.args()?;
                        Ok(Expr::Static { class: class.into(), name: member.into(), args })
                    } else {
                        Ok(Expr::StaticField { class: class.into(), name: member.into() })
                    }
                }
            },
            t => Err(format!("unexpected token {t:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_expressions_nested_too_deep() {
        let deep = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(parse_expr(&deep, false).unwrap_err().contains("nested"));
        let chain = vec!["\"a\""; 100_000].join(" + ");
        assert!(parse_expr(&chain, false).is_err());
        assert!(parse_expr(&"-".repeat(100_000), false).is_err());
        let fine = vec!["1"; 100].join(" + ");
        assert!(parse_expr(&fine, false).is_ok());
    }

    #[test]
    fn parses_typical_expressions() {
        for src in [
            "\"Page \" + $V{PAGE_NUMBER} + \" of \"",
            "$F{amount} != null ? $F{amount} : 0",
            "new java.text.SimpleDateFormat(\"dd/MM/yyyy\").format($F{date})",
            "$F{price}.multiply(new java.math.BigDecimal(\"1.18\")).setScale(2, java.math.RoundingMode.HALF_UP)",
            "java.lang.Math.max($F{a}, 10)",
            "((Integer) $V{REPORT_COUNT}).intValue() % 2 == 0",
            "!$F{active}.booleanValue() && $F{name}.trim().length() > 0",
            "$F{x} ?: \"n/a\"",
            "Boolean.TRUE",
        ] {
            parse_expr(src, false).unwrap_or_else(|e| panic!("{src}: {e}"));
        }
    }

    #[test]
    fn precedence() {
        let e = parse_expr("1 + 2 * 3 == 7 && true", false).unwrap();
        assert!(matches!(e, Expr::And(..)));
    }
}
