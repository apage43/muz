use anyhow::{Context, Result, bail};
#[derive(Clone, Debug)]
pub struct Node {
    pub at: usize,
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
    Import(String, String),
    Expr(Node),
}
pub type Program = Vec<Stmt>;
#[derive(Clone, Debug)]
pub(super) struct Token {
    pub(super) text: String,
    pub(super) at: usize,
    pub(super) end: usize,
    pub(super) string: bool,
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
                bail!("unclosed comment");
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
                bail!("unclosed string at byte {start}");
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
            bail!("unexpected character '{c}' at byte {i}");
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
    };
    p.program(false).context("parsing .muz source")
}
struct Parser {
    tokens: Vec<Token>,
    i: usize,
}
impl Parser {
    fn peek(&self) -> &str {
        &self.tokens[self.i].text
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
        if self.peek() == t {
            self.next();
            true
        } else {
            false
        }
    }
    fn need(&mut self, t: &str) -> Result<()> {
        if !self.eat(t) {
            bail!(
                "expected '{t}', got '{}' at byte {}",
                self.peek(),
                self.at()
            );
        }
        Ok(())
    }
    fn ident(&mut self) -> Result<String> {
        let t = self.next();
        if !t
            .text
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            bail!("expected name at byte {}", t.at);
        }
        Ok(t.text)
    }
    fn program(&mut self, block: bool) -> Result<Program> {
        let mut out = vec![];
        while self.peek() != "<eof>" && (!block || self.peek() != "}") {
            self.eat("export");
            if self.eat("use") {
                let path = self.next().text;
                self.need("as")?;
                let alias = self.ident()?;
                out.push(Stmt::Import(path, alias));
            } else if self.eat("let") {
                let n = self.ident()?;
                self.need("=")?;
                out.push(Stmt::Let(n, self.expr(0)?));
            } else if self.peek() == "fn"
                && self.tokens.get(self.i + 1).is_some_and(|x| x.text != "(")
            {
                self.next();
                let n = self.ident()?;
                let params = self.params()?;
                let body = if self.eat("=") {
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
        self.need("{")?;
        let body = self.program(true)?;
        self.need("}")?;
        Ok(Node {
            at,
            kind: Expr::Block(body),
        })
    }
    fn expr(&mut self, min: u8) -> Result<Node> {
        let at = self.at();
        let mut lhs = if self.eat("-") {
            Node {
                at,
                kind: Expr::Unary("-".into(), Box::new(self.expr(9)?)),
            }
        } else if self.eat("!") {
            Node {
                at,
                kind: Expr::Unary("!".into(), Box::new(self.expr(9)?)),
            }
        } else if self.eat("if") {
            let cond = self.expr(0)?;
            let yes = self.block()?;
            self.need("else")?;
            let no = self.block()?;
            Node {
                at,
                kind: Expr::If(Box::new(cond), Box::new(yes), Box::new(no)),
            }
        } else if self.eat("fn") {
            let params = self.params()?;
            self.need("=>")?;
            Node {
                at,
                kind: Expr::Lambda(params, Box::new(self.expr(0)?)),
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
                at,
                kind: Expr::Array(vs),
            }
        } else if self.peek() == "{"
            && (self
                .tokens
                .get(self.i + 1)
                .is_some_and(|t| matches!(t.text.as_str(), "let" | "fn"))
                || self.tokens.get(self.i + 2).is_some_and(|t| t.text == "("))
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
                at,
                kind: Expr::Record(vs),
            }
        } else if self.eat("(") {
            let n = self.expr(0)?;
            self.need(")")?;
            n
        } else {
            let t = self.next();
            Node {
                at,
                kind: if t.string {
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
                } else {
                    bail!("expected expression, got '{}' at byte {at}", t.text);
                },
            }
        };
        loop {
            if self.peek() == "(" {
                self.next();
                let mut args = vec![];
                while !self.eat(")") {
                    let name = if self
                        .tokens
                        .get(self.i + 1)
                        .is_some_and(|x| x.text == ":" || x.text == "=")
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
                    at,
                    kind: Expr::Call(Box::new(lhs), args),
                };
                continue;
            }
            if self.eat(".") {
                let name = self.ident()?;
                lhs = Node {
                    at,
                    kind: Expr::Get(Box::new(lhs), name),
                };
                continue;
            }
            if self.eat("[") {
                let index = self.expr(0)?;
                self.need("]")?;
                lhs = Node {
                    at,
                    kind: Expr::Index(Box::new(lhs), Box::new(index)),
                };
                continue;
            }
            let op = self.peek().to_owned();
            let prec = match op.as_str() {
                "||" => 1,
                "&&" => 2,
                "==" | "!=" => 3,
                "<" | ">" | "<=" | ">=" => 4,
                "+" | "-" => 5,
                "*" | "/" | "%" => 6,
                _ => 0,
            };
            if prec == 0 || prec < min {
                break;
            }
            self.next();
            let rhs = self.expr(prec + 1)?;
            lhs = Node {
                at,
                kind: Expr::Binary(op, Box::new(lhs), Box::new(rhs)),
            };
        }
        Ok(lhs)
    }
}
