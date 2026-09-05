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
/// Exact source values and finite, inexact control calculations.
#[derive(Clone, Copy, Debug)]
pub enum Number {
    Exact(Beat),
    Inexact(f64),
}
impl Number {
    pub fn number(self) -> f64 {
        match self {
            Self::Exact(x) => real(x),
            Self::Inexact(x) => x,
        }
    }
    pub fn exact(self) -> Result<Beat> {
        match self {
            Self::Exact(x) => Ok(x),
            Self::Inexact(x) => music::rational(x),
        }
    }
    pub fn finite(x: f64) -> Result<Self> {
        if !x.is_finite() {
            bail!("numeric result is not finite");
        }
        Ok(Self::Inexact(x))
    }
    pub fn compare(self, other: Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Exact(a), Self::Exact(b)) => a.cmp(&b),
            _ => self
                .number()
                .partial_cmp(&other.number())
                .expect("finite numbers"),
        }
    }
    pub fn arithmetic(self, op: &str, rhs: Self, scalar: bool) -> Result<Self> {
        use num_traits::{CheckedAdd, CheckedDiv, CheckedMul, CheckedSub};
        if matches!(op, "/" | "%") && rhs.number() == 0. {
            bail!("division by zero");
        }
        if let (Self::Exact(a), Self::Exact(b)) = (self, rhs) {
            let exact = match op {
                "+" => a.checked_add(&b),
                "-" => a.checked_sub(&b),
                "*" => a.checked_mul(&b),
                "/" => a.checked_div(&b),
                // Compute remainder without Ratio's unchecked cross products.
                "%" => a
                    .checked_div(&b)
                    .and_then(|q| b.checked_mul(&music::b(q.to_integer())))
                    .and_then(|product| a.checked_sub(&product)),
                _ => bail!("unknown operator {op}"),
            };
            if let Some(x) = exact {
                return Ok(Self::Exact(x));
            }
            if !scalar {
                bail!("exact number overflow");
            }
        }
        let (a, b) = (self.number(), rhs.number());
        Self::finite(match op {
            "+" => a + b,
            "-" => a - b,
            "*" => a * b,
            "/" => a / b,
            "%" => a % b,
            _ => bail!("unknown operator {op}"),
        })
    }
}
impl From<Beat> for Number {
    fn from(x: Beat) -> Self {
        Self::Exact(x)
    }
}
impl PartialEq for Number {
    fn eq(&self, rhs: &Self) -> bool {
        self.compare(*rhs).is_eq()
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Quantity {
    pub value: Number,
    pub unit: Unit,
}
impl Quantity {
    pub fn parse(s: &str) -> Result<Self> {
        let i = s
            .find(|c: char| c.is_ascii_alphabetic() || c == '%')
            .unwrap_or(s.len());
        let mut value = Number::Exact(music::decimal(&s[..i])?);
        let unit = match &s[i..] {
            "" => Unit::Scalar,
            "b" | "beat" | "beats" => Unit::Beat,
            "bar" | "bars" => Unit::Bar,
            "ms" => {
                value = value.arithmetic("/", b(1000).into(), false)?;
                Unit::Seconds
            }
            "s" | "sec" => Unit::Seconds,
            "dB" | "db" => Unit::Db,
            "Hz" | "hz" => Unit::Hz,
            "kHz" => {
                value = value.arithmetic("*", b(1000).into(), false)?;
                Unit::Hz
            }
            "bpm" => Unit::Bpm,
            "%" => {
                value = value.arithmetic("/", b(100).into(), true)?;
                Unit::Scalar
            }
            x => bail!("unknown unit '{x}'"),
        };
        Ok(Self { value, unit })
    }
    pub fn number(&self) -> f64 {
        self.value.number()
    }
    pub fn beats(&self, meter: f64) -> Result<Beat> {
        match self.unit {
            Unit::Scalar | Unit::Beat => self.value.exact(),
            Unit::Bar => self
                .value
                .arithmetic("*", music::rational(meter)?.into(), false)?
                .exact(),
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
    // Source containers are immutable: captures and indexing share storage rather
    // than recursively copying each input collection on every callback.
    Array(Arc<[Value]>),
    Record(Arc<BTreeMap<String, Value>>),
    Pattern(Arc<Pattern>),
    Function(Rc<Function>),
    Builtin(String, Option<Box<Value>>),
}
impl Value {
    pub fn num(x: f64) -> Self {
        match Number::finite(x) {
            Ok(value) => Self::Num(Quantity {
                value,
                unit: Unit::Scalar,
            }),
            Err(e) => Self::Invalid(e.to_string()),
        }
    }
    pub fn integer(x: i64) -> Self {
        Self::Num(Quantity {
            value: b(x).into(),
            unit: Unit::Scalar,
        })
    }
    pub fn beat(x: Beat) -> Self {
        Self::Num(Quantity {
            value: x.into(),
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
            Self::Num(q) => q.number() != 0.,
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
            Self::Num(q) => match q.value {
                Number::Exact(value) if *value.denom() == 1 => serde_json::json!(*value.numer()),
                Number::Inexact(value)
                    if value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0 =>
                {
                    serde_json::json!(value as i64)
                }
                _ => serde_json::json!(q.number()),
            },
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
    pub fn standard_module(&mut self, name: &str, source: &str) -> Result<Value> {
        let key = PathBuf::from(format!("<std/{name}>"));
        if let Some(value) = self.cache.get(&key) {
            return Ok(value.clone());
        }
        if !self.active.insert(key.clone()) {
            bail!("circular standard module initialization: {name}");
        }
        let previous = std::mem::replace(&mut self.path, key.clone());
        let value = self.source(source);
        self.path = previous;
        self.active.remove(&key);
        let value = value?;
        self.cache.insert(key, value.clone());
        Ok(value)
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
        Ok(Value::Record(env.into()))
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
                            "std/prelude" => include_str!("../../std/prelude.muz"),
                            "std/patterns" => include_str!("../../std/patterns.muz"),
                            "std/arrange" => include_str!("../../std/arrange.muz"),
                            "std/performance" => include_str!("../../std/performance.muz"),
                            "std/catalogs" => include_str!("../../std/catalogs.muz"),
                            "std/music" => include_str!("../../std/music.muz"),
                            "std/tonal" => include_str!("../../std/tonal.muz"),
                            "std/piano" => include_str!("../../std/piano.muz"),
                            "std/grooves" => include_str!("../../std/grooves.muz"),
                            "std/mix" => include_str!("../../std/mix.muz"),
                            _ => bail!("unknown standard module '{path}'"),
                        };
                        self.standard_module(path.strip_prefix("std/").unwrap(), text)?
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
                Value::Record(out.into())
            }
            Expr::Unary(op, x) => {
                let x = self.eval(x, env)?;
                if op == "!" {
                    Value::Bool(!x.truth())
                } else if let Value::Num(mut q) = x {
                    q.value =
                        Number::Exact(b(0)).arithmetic("-", q.value, q.unit == Unit::Scalar)?;
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
                    Value::Pattern(p) if key == "controls" => Value::Array(
                        p.controls
                            .iter()
                            .map(super::builtins::control_value)
                            .collect(),
                    ),
                    Value::Pattern(p) if key == "raw" => {
                        Value::Array(p.raw.iter().map(super::builtins::raw_value).collect())
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
                match super::builtins::call(self, &name, args.clone()) {
                    Err(error)
                        if error
                            .downcast_ref::<super::builtins::UnknownFunction>()
                            .is_some_and(|missing| {
                                missing.0 == name.strip_prefix("std.").unwrap_or(&name)
                            }) =>
                    {
                        let library =
                            self.standard_module("prelude", include_str!("../../std/prelude.muz"))?;
                        let exports = library
                            .record()?
                            .get("__result")
                            .ok_or_else(|| anyhow::anyhow!("standard prelude has no exports"))?
                            .record()?;
                        let bare = name.strip_prefix("std.").unwrap_or(&name);
                        match exports.get(bare) {
                            Some(function) => self.call(function.clone(), args),
                            None => Err(error),
                        }
                    }
                    result => result,
                }
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
                    q.value = q.value.arithmetic("*", music::b(4).into(), false)?;
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
                return Ok(Value::Array(a.iter().chain(b.iter()).cloned().collect()));
            }
            _ => {}
        }
    }
    let (Value::Num(a), Value::Num(c)) = (x, y) else {
        bail!("operator {op} requires numbers");
    };
    let unit = match op {
        "+" | "-" | "%" | "<" | ">" | "<=" | ">=" => {
            if a.unit != c.unit {
                bail!("incompatible units {:?} and {:?}", a.unit, c.unit);
            }
            a.unit
        }
        "*" => {
            if a.unit == Unit::Scalar {
                c.unit
            } else if c.unit == Unit::Scalar {
                a.unit
            } else {
                bail!("multiplication of two dimensional quantities is not supported");
            }
        }
        "/" => {
            if a.unit == c.unit {
                Unit::Scalar
            } else if c.unit == Unit::Scalar {
                a.unit
            } else {
                bail!("incompatible division units");
            }
        }
        _ => bail!("unknown operator {op}"),
    };
    let order = a.value.compare(c.value);
    let boolv = match op {
        "<" => Some(order.is_lt()),
        ">" => Some(order.is_gt()),
        "<=" => Some(!order.is_gt()),
        ">=" => Some(!order.is_lt()),
        _ => None,
    };
    if let Some(v) = boolv {
        return Ok(Value::Bool(v));
    }
    let value = a.value.arithmetic(op, c.value, unit == Unit::Scalar)?;
    Ok(Value::Num(Quantity { value, unit }))
}
