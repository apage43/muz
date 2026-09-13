use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

// Formatting needs concrete boundaries that the evaluation AST intentionally
// discards (including parentheses). Collect them only for formatter callers.
#[derive(Default)]
pub(super) struct Syntax {
    pub expressions: BTreeMap<(usize, usize), Shape>,
    pub blocks: BTreeSet<usize>,
    pub definitions: BTreeSet<usize>,
    pub statements: BTreeSet<usize>,
    pub calls: BTreeSet<usize>,
}
#[derive(Clone, Copy)]
pub(super) enum Shape {
    Binary { operator: usize },
    Lambda { arrow: usize },
    If { yes: usize, no: usize },
}
pub(super) fn binary_precedence(op: &str) -> u8 {
    match op {
        "||" => 1,
        "&&" => 2,
        "==" | "!=" => 3,
        "<" | ">" | "<=" | ">=" => 4,
        "+" | "-" => 5,
        "*" | "/" | "%" => 6,
        _ => 0,
    }
}
#[derive(Clone, Debug)]
pub struct Node {
    pub at: usize,
    pub end: usize,
    pub kind: Expr,
}
#[derive(Clone, Debug)]
pub enum Expr {
    Number(String),
    String(String),
    Bool(bool),
    Ident(String),
    Array(Vec<Node>),
    Record(Vec<(String, Node)>),
    Unary(String, Box<Node>),
    Binary(String, Box<Node>, Box<Node>),
    Get(Box<Node>, String),
    Index(Box<Node>, Box<Node>),
    Call(Box<Node>, Vec<(Option<String>, Node)>),
    Lambda(Vec<(String, Option<Node>)>, Box<Node>),
    If(Box<Node>, Box<Node>, Box<Node>),
    Block(Program),
}
#[derive(Clone, Debug)]
pub enum Stmt {
    Let(String, Node),
    Function(String, Vec<(String, Option<Node>)>, Node),
    /// `use "path" as alias`: the module path, the local alias and the byte span
    /// of the whole statement, so import failures can name their source line.
    Import(String, String, usize, usize),
    Expr(Node),
}
pub type Program = Vec<Stmt>;
/// A parse failure with the byte span it was found at. Callers that know the
/// file name render it through [`super::diagnostic::syntax`].
#[derive(Debug)]
pub struct SyntaxError {
    pub at: usize,
    pub end: usize,
    pub message: String,
    help: Option<String>,
}
impl SyntaxError {
    fn new(at: usize, end: usize, message: impl Into<String>) -> Self {
        Self {
            at,
            end,
            message: message.into(),
            help: None,
        }
    }
    fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
    pub(super) fn helps(&self) -> impl Iterator<Item = &str> {
        self.help.iter().map(String::as_str)
    }
}
impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.message, self.at)
    }
}
impl std::error::Error for SyntaxError {}

#[derive(Clone, Debug)]
pub(super) struct Token {
    pub(super) text: String,
    pub(super) at: usize,
    pub(super) end: usize,
    pub(super) string: bool,
}
impl Token {
    fn is(&self, syntax: &str) -> bool {
        !self.string && self.text == syntax
    }
}
pub(super) fn lex(s: &str) -> Result<Vec<Token>> {
    let mut out = vec![];
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if s[i..].starts_with("//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if s[i..].starts_with("/*") {
            i += 2;
            let Some(n) = s[i..].find("*/") else {
                return Err(SyntaxError::new(start, start + 2, "unclosed comment")
                    .help("close the comment with `*/`")
                    .into());
            };
            i += n + 2;
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let mut text = String::new();
            let mut closed = false;
            while i < bytes.len() {
                let ch = s[i..].chars().next().unwrap();
                i += ch.len_utf8();
                if ch == quote {
                    closed = true;
                    break;
                }
                if ch == '\\' {
                    if i >= bytes.len() {
                        break;
                    }
                    let x = bytes[i] as char;
                    i += 1;
                    text.push(match x {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        _ => x,
                    });
                } else {
                    text.push(ch);
                }
            }
            if !closed {
                return Err(SyntaxError::new(start, i, "unclosed string")
                    .help(format!("close the string with a matching {quote}"))
                    .into());
            }
            out.push(Token {
                text,
                at: start,
                end: i,
                string: true,
            });
            continue;
        }
        if c.is_ascii_digit() {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1].is_ascii_digit() {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            while i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'%') {
                i += 1;
            }
            out.push(Token {
                text: s[start..i].into(),
                at: start,
                end: i,
                string: false,
            });
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(Token {
                text: s[start..i].into(),
                at: start,
                end: i,
                string: false,
            });
            continue;
        }
        let op = ["=>", "==", "!=", "<=", ">=", "&&", "||", "|>"]
            .iter()
            .find(|x| s[i..].starts_with(**x));
        if let Some(op) = op {
            i += op.len();
            out.push(Token {
                text: (*op).into(),
                at: start,
                end: i,
                string: false,
            });
        } else if "{}[](),;:.+-*/%=!<>".contains(c) {
            i += 1;
            out.push(Token {
                text: c.to_string(),
                at: start,
                end: i,
                string: false,
            });
        } else {
            let ch = s[i..].chars().next().unwrap_or(c);
            let mut error =
                SyntaxError::new(i, i + ch.len_utf8(), format!("unexpected character '{ch}'"));
            if !ch.is_ascii() {
                error = error.help("write non-ASCII text inside a string");
            }
            return Err(error.into());
        }
    }
    out.push(Token {
        text: "<eof>".into(),
        at: s.len(),
        end: s.len(),
        string: false,
    });
    Ok(out)
}
pub fn parse(s: &str) -> Result<Program> {
    let mut p = Parser {
        tokens: lex(s)?,
        i: 0,
        syntax: None,
    };
    p.program(false)
}
pub(super) fn parse_for_format(s: &str) -> Result<Syntax> {
    let mut p = Parser {
        tokens: lex(s)?,
        i: 0,
        syntax: Some(Syntax::default()),
    };
    p.program(false)?;
    Ok(p.syntax.unwrap())
}
struct Parser {
    tokens: Vec<Token>,
    i: usize,
    syntax: Option<Syntax>,
}
impl Parser {
    fn shape(&mut self, start: usize, shape: Shape) {
        if let Some(syntax) = &mut self.syntax {
            syntax
                .expressions
                .insert((start, self.tokens[self.i - 1].end), shape);
        }
    }
    fn peek(&self) -> &Token {
        &self.tokens[self.i]
    }
    fn at(&self) -> usize {
        self.tokens[self.i].at
    }
    fn next(&mut self) -> Token {
        let t = self.tokens[self.i].clone();
        if self.i + 1 < self.tokens.len() {
            self.i += 1;
        }
        t
    }
    fn eat(&mut self, t: &str) -> bool {
        if self.peek().is(t) {
            self.next();
            true
        } else {
            false
        }
    }
    fn need(&mut self, t: &str) -> Result<()> {
        if !self.eat(t) {
            let token = self.peek();
            let (at, end) = (token.at, token.end.max(token.at + 1));
            if token.is("<eof>") {
                return Err(SyntaxError::new(
                    at,
                    end,
                    format!("expected '{t}' before the end of the file"),
                )
                .help("close the expression or statement this token belongs to")
                .into());
            }
            return Err(
                SyntaxError::new(at, end, format!("expected '{t}', got '{}'", token.text)).into(),
            );
        }
        Ok(())
    }
    fn ident(&mut self) -> Result<String> {
        let t = self.next();
        if t.string
            || !t
                .text
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return Err(SyntaxError::new(t.at, t.end, "expected a name")
                .help("names start with a letter or _ and continue with letters, digits or _")
                .into());
        }
        Ok(t.text)
    }
    fn program(&mut self, block: bool) -> Result<Program> {
        let mut out = vec![];
        while !self.peek().is("<eof>") && (!block || !self.peek().is("}")) {
            let start = self.at();
            if let Some(syntax) = &mut self.syntax {
                syntax.statements.insert(start);
            }
            self.eat("export");
            if self.eat("use") {
                let path = self.next().text;
                self.need("as")?;
                let alias = self.ident()?;
                out.push(Stmt::Import(
                    path,
                    alias,
                    start,
                    self.tokens[self.i - 1].end,
                ));
            } else if self.eat("let") {
                let n = self.ident()?;
                let at = self.at();
                if let Some(syntax) = &mut self.syntax {
                    syntax.definitions.insert(at);
                }
                self.need("=")?;
                out.push(Stmt::Let(n, self.expr(0)?));
            } else if self.peek().is("fn")
                && self.tokens.get(self.i + 1).is_some_and(|x| !x.is("("))
            {
                self.next();
                let n = self.ident()?;
                let params = self.params()?;
                let at = self.at();
                let body = if self.eat("=") {
                    if let Some(syntax) = &mut self.syntax {
                        syntax.definitions.insert(at);
                    }
                    self.expr(0)?
                } else {
                    self.block()?
                };
                out.push(Stmt::Function(n, params, body));
            } else {
                out.push(Stmt::Expr(self.expr(0)?));
            }
            self.eat(";");
        }
        Ok(out)
    }
    fn params(&mut self) -> Result<Vec<(String, Option<Node>)>> {
        self.need("(")?;
        let mut out = vec![];
        while !self.eat(")") {
            let n = self.ident()?;
            let default = if self.eat("=") {
                Some(self.expr(0)?)
            } else {
                None
            };
            out.push((n, default));
            if !self.eat(",") {
                self.need(")")?;
                break;
            }
        }
        Ok(out)
    }
    fn block(&mut self) -> Result<Node> {
        let at = self.at();
        if let Some(syntax) = &mut self.syntax {
            syntax.blocks.insert(at);
        }
        self.need("{")?;
        let body = self.program(true)?;
        self.need("}")?;
        Ok(Node {
            end: self.tokens[self.i - 1].end,
            at,
            kind: Expr::Block(body),
        })
    }
    fn expr(&mut self, min: u8) -> Result<Node> {
        let at = self.at();
        let mut lhs = if self.eat("-") {
            Node {
                end: at,
                at,
                kind: Expr::Unary("-".into(), Box::new(self.expr(9)?)),
            }
        } else if self.eat("!") {
            Node {
                end: at,
                at,
                kind: Expr::Unary("!".into(), Box::new(self.expr(9)?)),
            }
        } else if self.eat("if") {
            let cond = self.expr(0)?;
            let yes = self.block()?;
            self.need("else")?;
            let no = self.block()?;
            self.shape(
                at,
                Shape::If {
                    yes: yes.at,
                    no: no.at,
                },
            );
            Node {
                end: at,
                at,
                kind: Expr::If(Box::new(cond), Box::new(yes), Box::new(no)),
            }
        } else if self.eat("fn") {
            let params = self.params()?;
            let arrow = self.at();
            self.need("=>")?;
            let body = self.expr(0)?;
            self.shape(at, Shape::Lambda { arrow });
            Node {
                end: at,
                at,
                kind: Expr::Lambda(params, Box::new(body)),
            }
        } else if self.eat("[") {
            let mut vs = vec![];
            while !self.eat("]") {
                vs.push(self.expr(0)?);
                if !self.eat(",") {
                    self.need("]")?;
                    break;
                }
            }
            Node {
                end: at,
                at,
                kind: Expr::Array(vs),
            }
        } else if self.peek().is("{")
            && (self
                .tokens
                .get(self.i + 1)
                .is_some_and(|t| t.is("let") || t.is("fn"))
                || self.tokens.get(self.i + 2).is_some_and(|t| t.is("(")))
        {
            self.block()?
        } else if self.eat("{") {
            let mut vs = vec![];
            while !self.eat("}") {
                let k = self.next().text;
                if !self.eat(":") {
                    self.need("=")?;
                }
                vs.push((k, self.expr(0)?));
                if !self.eat(",") && !self.eat(";") {
                    self.need("}")?;
                    break;
                }
            }
            Node {
                end: at,
                at,
                kind: Expr::Record(vs),
            }
        } else if self.eat("(") {
            let n = self.expr(0)?;
            self.need(")")?;
            n
        } else {
            let t = self.next();
            let kind = if t.string {
                Expr::String(t.text)
            } else if t
                .text
                .as_bytes()
                .first()
                .is_some_and(|c| c.is_ascii_digit())
            {
                Expr::Number(t.text)
            } else if t.text == "true" || t.text == "false" {
                Expr::Bool(t.text == "true")
            } else if t
                .text
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                Expr::Ident(t.text)
            } else if t.is("<eof>") {
                return Err(SyntaxError::new(
                    at,
                    t.end.max(at + 1),
                    "expected an expression before the end of the file",
                )
                .help("finish the statement or remove the trailing operator")
                .into());
            } else {
                return Err(SyntaxError::new(
                    at,
                    t.end.max(at + 1),
                    format!("expected an expression, got '{}'", t.text),
                )
                .into());
            };
            Node { end: at, at, kind }
        };
        loop {
            // Close this span before it becomes the receiver/left operand of
            // another expression, so nested failures retain their own extent.
            lhs.end = self.tokens[self.i - 1].end;
            if self.peek().is("(") {
                let open = self.at();
                if let Some(syntax) = &mut self.syntax {
                    syntax.calls.insert(open);
                }
                self.next();
                let mut args = vec![];
                while !self.eat(")") {
                    let name = if self
                        .tokens
                        .get(self.i + 1)
                        .is_some_and(|x| x.is(":") || x.is("="))
                    {
                        let n = self.next().text;
                        self.next();
                        Some(n)
                    } else {
                        None
                    };
                    args.push((name, self.expr(0)?));
                    if !self.eat(",") {
                        self.need(")")?;
                        break;
                    }
                }
                lhs = Node {
                    end: at,
                    at,
                    kind: Expr::Call(Box::new(lhs), args),
                };
                continue;
            }
            if self.eat(".") {
                let name = self.ident()?;
                lhs = Node {
                    end: at,
                    at,
                    kind: Expr::Get(Box::new(lhs), name),
                };
                continue;
            }
            if self.eat("[") {
                let index = self.expr(0)?;
                self.need("]")?;
                lhs = Node {
                    end: at,
                    at,
                    kind: Expr::Index(Box::new(lhs), Box::new(index)),
                };
                continue;
            }
            let op = if self.peek().string {
                String::new()
            } else {
                self.peek().text.clone()
            };
            let prec = binary_precedence(&op);
            if prec == 0 || prec < min {
                break;
            }
            let operator = self.next().at;
            let rhs = self.expr(prec + 1)?;
            self.shape(at, Shape::Binary { operator });
            lhs = Node {
                end: at,
                at,
                kind: Expr::Binary(op, Box::new(lhs), Box::new(rhs)),
            };
        }
        Ok(lhs)
    }
}
