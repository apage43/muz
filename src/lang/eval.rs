use super::parser::{Expr, Node, Program, Stmt};
use crate::music::{self, Beat, Pattern, b, real};
use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Scalar,
    Beat,
    Bar,
    Seconds,
    Db,
    Hz,
    Bpm,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Quantity {
    pub value: Beat,
    pub unit: Unit,
}
impl Quantity {
    pub fn parse(s: &str) -> Result<Self> {
        let i = s
            .find(|c: char| c.is_ascii_alphabetic() || c == '%')
            .unwrap_or(s.len());
        let mut value = music::decimal(&s[..i])?;
        let unit = match &s[i..] {
            "" => Unit::Scalar,
            "b" | "beat" | "beats" => Unit::Beat,
            "bar" | "bars" => Unit::Bar,
            "ms" => {
                value /= b(1000);
                Unit::Seconds
            }
            "s" | "sec" => Unit::Seconds,
            "dB" | "db" => Unit::Db,
            "Hz" | "hz" => Unit::Hz,
            "kHz" => {
                value *= b(1000);
                Unit::Hz
            }
            "bpm" => Unit::Bpm,
            "%" => {
                value /= b(100);
                Unit::Scalar
            }
            x => bail!("unknown unit '{x}'"),
        };
        Ok(Self { value, unit })
    }
    pub fn number(&self) -> f64 {
        real(self.value)
    }
    pub fn beats(&self, meter: f64) -> Result<Beat> {
        match self.unit {
            Unit::Scalar | Unit::Beat => Ok(self.value),
            Unit::Bar => Ok(self.value * music::rational(meter)?),
            _ => bail!("expected musical duration, got {:?}", self.unit),
        }
    }
}
pub type Env = BTreeMap<String, Value>;
#[derive(Clone, Debug)]
pub struct Function {
    pub params: Vec<(String, Option<Node>)>,
    pub body: Node,
    pub env: Env,
    pub path: PathBuf,
}
#[derive(Clone, Debug)]
pub enum Value {
    Invalid(String),
    Null,
    Num(Quantity),
    Bool(bool),
    Str(String),
    Array(Vec<Value>),
    Record(BTreeMap<String, Value>),
    Pattern(Arc<Pattern>),
    Function(Rc<Function>),
    Builtin(String, Option<Box<Value>>),
}
impl Value {
    pub fn num(x: f64) -> Self {
        match music::rational(x) {
            Ok(value) => Self::Num(Quantity {
                value,
                unit: Unit::Scalar,
            }),
            Err(e) => Self::Invalid(e.to_string()),
        }
    }
    pub fn integer(x: i64) -> Self {
        Self::Num(Quantity {
            value: b(x),
            unit: Unit::Scalar,
        })
    }
    pub fn beat(x: Beat) -> Self {
        Self::Num(Quantity {
            value: x,
            unit: Unit::Beat,
        })
    }
    pub fn number(&self) -> Result<f64> {
        match self {
            Self::Num(n) => Ok(n.number()),
            _ => bail!("expected a number, got {}", self.kind()),
        }
    }
    pub fn beats(&self) -> Result<Beat> {
        match self {
            Self::Num(n) => n.beats(4.0),
            _ => bail!("expected a musical duration"),
        }
    }
    pub fn text(&self) -> Result<&str> {
        match self {
            Self::Str(s) => Ok(s),
            _ => bail!("expected text, got {}", self.kind()),
        }
    }
    pub fn array(&self) -> Result<&[Value]> {
        match self {
            Self::Array(s) => Ok(s),
            _ => bail!("expected a list, got {}", self.kind()),
        }
    }
    pub fn record(&self) -> Result<&BTreeMap<String, Value>> {
        match self {
            Self::Record(s) => Ok(s),
            _ => bail!("expected a record, got {}", self.kind()),
        }
    }
    pub fn pattern(&self) -> Result<&Pattern> {
        match self {
            Self::Pattern(p) => Ok(p),
            _ => bail!("expected a pattern, got {}", self.kind()),
        }
    }
    pub fn truth(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Null => false,
            Self::Num(q) => q.value != b(0),
            Self::Array(a) => !a.is_empty(),
            Self::Str(s) => !s.is_empty(),
            _ => true,
        }
    }
    pub fn kind(&self) -> &str {
        match self {
            Self::Invalid(_) => "invalid number",
            Self::Null => "null",
            Self::Num(_) => "number",
            Self::Bool(_) => "bool",
            Self::Str(_) => "text",
            Self::Array(_) => "list",
            Self::Record(_) => "record",
            Self::Pattern(_) => "pattern",
            Self::Function(_) | Self::Builtin(..) => "function",
        }
    }
    pub fn json(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Num(q) => serde_json::json!(q.number()),
            Self::Bool(b) => serde_json::json!(b),
            Self::Str(s) => serde_json::json!(s),
            Self::Array(v) => serde_json::Value::Array(v.iter().map(Self::json).collect()),
            Self::Record(r) => {
                serde_json::Value::Object(r.iter().map(|(k, v)| (k.clone(), v.json())).collect())
            }
            Self::Pattern(p) => serde_json::to_value(p.as_ref()).unwrap(),
            _ => serde_json::json!("<function>"),
        }
    }
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Record(r) => r.get(key),
            _ => None,
        }
    }
}
pub struct Evaluator {
    pub dependencies: Vec<PathBuf>,
    pub path: PathBuf,
    cache: BTreeMap<PathBuf, Value>,
    active: BTreeSet<PathBuf>,
    pub steps: usize,
    pub depth: usize,
}
impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}
impl Evaluator {
    pub fn new() -> Self {
        Self {
            dependencies: vec![],
            path: PathBuf::from("<source>"),
            cache: BTreeMap::new(),
            active: BTreeSet::new(),
            steps: 0,
            depth: 0,
        }
    }
    pub fn module(&mut self, path: &Path) -> Result<Value> {
        let path = path
            .canonicalize()
            .with_context(|| format!("reading {}", path.display()))?;
        if let Some(v) = self.cache.get(&path) {
            return Ok(v.clone());
        }
        if !self.active.insert(path.clone()) {
            bail!("circular import: {}", path.display());
        }
        self.dependencies.push(path.clone());
        let source = std::fs::read_to_string(&path)?;
        let previous = std::mem::replace(&mut self.path, path.clone());
        let result = self.source(&source);
        self.path = previous;
        self.active.remove(&path);
        let value = result.map_err(|error| anyhow::anyhow!("{}: {error:#}", path.display()))?;
        self.cache.insert(path, value.clone());
        Ok(value)
    }
    pub fn source(&mut self, source: &str) -> Result<Value> {
        let program = super::parse(source)?;
        let mut env = Env::new();
        let result = self.program(&program, &mut env)?;
        env.insert("__result".into(), result);
        Ok(Value::Record(env))
    }
    fn program(&mut self, p: &Program, env: &mut Env) -> Result<Value> {
        let mut result = Value::Null;
        for stmt in p {
            match stmt {
                Stmt::Let(n, e) => {
                    let v = self.eval(e, env)?;
                    env.insert(n.clone(), v);
                }
                Stmt::Function(n, params, body) => {
                    env.insert(
                        n.clone(),
                        Value::Function(Rc::new(Function {
                            params: params.clone(),
                            body: body.clone(),
                            env: env.clone(),
                            path: self.path.clone(),
                        })),
                    );
                }
                Stmt::Import(path, alias) => {
                    let value = if path == "std" {
                        Value::Builtin(String::new(), None)
                    } else if path.starts_with("std/") {
                        let text = match path.as_str() {
                            "std/music" => include_str!("../../std/music.muz"),
                            "std/tonal" => include_str!("../../std/tonal.muz"),
                            "std/piano" => include_str!("../../std/piano.muz"),
                            "std/grooves" => include_str!("../../std/grooves.muz"),
                            "std/mix" => include_str!("../../std/mix.muz"),
                            _ => bail!("unknown standard module '{path}'"),
                        };
                        self.source(text)?
                    } else {
                        let path = self.path.parent().unwrap_or(Path::new(".")).join(path);
                        self.module(&path)?
                    };
                    env.insert(alias.clone(), value);
                }
                Stmt::Expr(e) => {
                    result = self.eval(e, env)?;
                }
            }
        }
        Ok(result)
    }
    pub fn eval(&mut self, n: &Node, env: &Env) -> Result<Value> {
        if crate::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("evaluation cancelled")
        }
        self.steps += 1;
        if self.steps > 5_000_000 {
            bail!("evaluation budget exceeded (5 million operations)");
        }
        self.eval_inner(n, env)
            .with_context(|| format!("{} at byte {}", self.path.display(), n.at))
    }
    fn eval_inner(&mut self, n: &Node, env: &Env) -> Result<Value> {
        Ok(match &n.kind {
            Expr::Number(s) => Value::Num(Quantity::parse(s)?),
            Expr::String(s) => Value::Str(s.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Ident(s) => env.get(s).cloned().unwrap_or_else(|| {
                if s == "null" {
                    Value::Null
                } else {
                    Value::Builtin(s.clone(), None)
                }
            }),
            Expr::Array(v) => {
                Value::Array(v.iter().map(|n| self.eval(n, env)).collect::<Result<_>>()?)
            }
            Expr::Record(r) => {
                let mut out = BTreeMap::new();
                for (k, n) in r {
                    if out.insert(k.clone(), self.eval(n, env)?).is_some() {
                        bail!("duplicate record field '{k}'");
                    }
                }
                Value::Record(out)
            }
            Expr::Unary(op, x) => {
                let x = self.eval(x, env)?;
                if op == "!" {
                    Value::Bool(!x.truth())
                } else if let Value::Num(mut q) = x {
                    q.value = -q.value;
                    Value::Num(q)
                } else {
                    bail!("unary '-' needs a number");
                }
            }
            Expr::Binary(op, x, y) => {
                let x = self.eval(x, env)?;
                if op == "&&" && !x.truth() {
                    return Ok(Value::Bool(false));
                }
                if op == "||" && x.truth() {
                    return Ok(Value::Bool(true));
                }
                let y = self.eval(y, env)?;
                binary(op, x, y)?
            }
            Expr::Get(x, key) => {
                let x = self.eval(x, env)?;
                match &x {
                    Value::Record(r) => r
                        .get(key)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("record has no field '{key}'"))?,
                    Value::Builtin(prefix, None) => Value::Builtin(
                        if prefix.is_empty() {
                            key.clone()
                        } else {
                            format!("{prefix}.{key}")
                        },
                        None,
                    ),
                    Value::Pattern(p) if key == "span" => Value::beat(p.span),
                    Value::Pattern(p) if key == "notes" => {
                        Value::Array(p.notes.iter().map(super::builtins::note_value).collect())
                    }
                    Value::Array(a) if key == "length" => Value::integer(a.len() as i64),
                    _ => Value::Builtin(key.clone(), Some(Box::new(x))),
                }
            }
            Expr::Index(x, i) => {
                let x = self.eval(x, env)?;
                let i = self.eval(i, env)?;
                match x {
                    Value::Array(a) => {
                        let idx = i.number()? as i64;
                        let idx = if idx < 0 { a.len() as i64 + idx } else { idx };
                        a.get(idx as usize)
                            .cloned()
                            .ok_or_else(|| anyhow::anyhow!("index {idx} out of range"))?
                    }
                    Value::Record(r) => r
                        .get(i.text()?)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("missing record key"))?,
                    _ => bail!("indexing needs list or record"),
                }
            }
            Expr::Call(f, args) => {
                let f = self.eval(f, env)?;
                let mut vs = vec![];
                for (k, v) in args {
                    vs.push((k.clone(), self.eval(v, env)?));
                }
                self.call(f, vs)?
            }
            Expr::Lambda(params, body) => Value::Function(Rc::new(Function {
                params: params.clone(),
                body: *body.clone(),
                env: env.clone(),
                path: self.path.clone(),
            })),
            Expr::If(cond, yes, no) => {
                if self.eval(cond, env)?.truth() {
                    self.eval(yes, env)?
                } else {
                    self.eval(no, env)?
                }
            }
            Expr::Block(p) => self.program(p, &mut env.clone())?,
        })
    }
    pub fn call(&mut self, f: Value, args: Vec<(Option<String>, Value)>) -> Result<Value> {
        self.depth += 1;
        if self.depth > 128 {
            bail!("call depth exceeds 128");
        }
        let result = self.call_inner(f, args);
        self.depth -= 1;
        result
    }
    fn call_inner(&mut self, f: Value, mut args: Vec<(Option<String>, Value)>) -> Result<Value> {
        match f {
            Value::Builtin(name, bound) => {
                if let Some(v) = bound {
                    args.insert(0, (None, *v));
                }
                super::builtins::call(self, &name, args)
            }
            Value::Function(f) => {
                let mut env = f.env.clone();
                let mut used = BTreeSet::new();
                let positional: Vec<_> = args
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.0.is_none())
                    .collect();
                let mut pos = 0;
                for (name, default) in &f.params {
                    let v = if let Some((i, (_, v))) = args
                        .iter()
                        .enumerate()
                        .find(|(_, a)| a.0.as_deref() == Some(name))
                    {
                        if !used.insert(i) {
                            bail!("duplicate argument {name}");
                        }
                        v.clone()
                    } else if let Some((i, (_, v))) = positional.get(pos) {
                        pos += 1;
                        used.insert(*i);
                        v.clone()
                    } else if let Some(n) = default {
                        self.eval(n, &env)?
                    } else {
                        bail!("missing argument '{name}'");
                    };
                    env.insert(name.clone(), v);
                }
                if used.len() != args.len() {
                    bail!("unexpected or repeated function argument");
                }
                let old = std::mem::replace(&mut self.path, f.path.clone());
                let value = self.eval(&f.body, &env);
                self.path = old;
                value
            }
            _ => bail!("{} is not callable", f.kind()),
        }
    }
}
fn binary(op: &str, mut x: Value, mut y: Value) -> Result<Value> {
    if let (Value::Num(a), Value::Num(b)) = (&mut x, &mut y) {
        if matches!(
            (a.unit, b.unit),
            (Unit::Beat, Unit::Bar) | (Unit::Bar, Unit::Beat)
        ) {
            for q in [a, b] {
                if q.unit == Unit::Bar {
                    q.value *= music::b(4);
                    q.unit = Unit::Beat;
                }
            }
        }
    }
    if op == "==" || op == "!=" {
        let eq = match (&x, &y) {
            (Value::Num(a), Value::Num(b)) => a == b,
            _ => x.json() == y.json(),
        };
        return Ok(Value::Bool(if op == "==" { eq } else { !eq }));
    }
    if op == "&&" || op == "||" {
        return Ok(Value::Bool(if op == "&&" {
            x.truth() && y.truth()
        } else {
            x.truth() || y.truth()
        }));
    }
    if op == "+" {
        match (&x, &y) {
            (Value::Str(a), Value::Str(b)) => return Ok(Value::Str(format!("{a}{b}"))),
            (Value::Array(a), Value::Array(b)) => {
                return Ok(Value::Array(a.iter().chain(b).cloned().collect()));
            }
            _ => {}
        }
    }
    let (Value::Num(a), Value::Num(c)) = (x, y) else {
        bail!("operator {op} requires numbers");
    };
    let mut unit = a.unit;
    if a.unit != c.unit {
        match op {
            "+" | "-" | "<" | ">" | "<=" | ">=" => {
                bail!("incompatible units {:?} and {:?}", a.unit, c.unit)
            }
            "*" if a.unit == Unit::Scalar => unit = c.unit,
            "*" if c.unit != Unit::Scalar => {
                bail!("multiplication of two dimensional quantities is not supported")
            }
            "/" if c.unit != Unit::Scalar => bail!("incompatible division units"),
            _ => {}
        }
    } else if op == "/" {
        unit = Unit::Scalar;
    }
    if matches!(op, "/" | "%") && c.value == b(0) {
        bail!("division by zero");
    }
    let boolv = match op {
        "<" => Some(a.value < c.value),
        ">" => Some(a.value > c.value),
        "<=" => Some(a.value <= c.value),
        ">=" => Some(a.value >= c.value),
        _ => None,
    };
    if let Some(v) = boolv {
        return Ok(Value::Bool(v));
    }
    use num_traits::{CheckedAdd, CheckedDiv, CheckedMul, CheckedSub};
    let value = match op {
        "+" => a.value.checked_add(&c.value),
        "-" => a.value.checked_sub(&c.value),
        "*" => a.value.checked_mul(&c.value),
        "/" => a.value.checked_div(&c.value),
        "%" => Some(a.value % c.value),
        _ => bail!("unknown operator {op}"),
    }
    .ok_or_else(|| anyhow::anyhow!("exact number overflow"))?;
    Ok(Value::Num(Quantity { value, unit }))
}
