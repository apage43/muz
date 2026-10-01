use super::diagnostic::{Diagnostic, Location, Origin, SourceFile, syntax};
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
/// Unit suffixes accepted by number literals, listed for typo hints.
const UNITS: &[&str] = &[
    "b", "beat", "beats", "bar", "bars", "ms", "s", "sec", "dB", "db", "Hz", "hz", "kHz", "bpm",
    "%",
];

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
            x => {
                return Err(Diagnostic::new(format!("unknown unit '{x}'"))
                    .helps(super::diagnostic::suggest_vocabulary(
                        "units",
                        x,
                        UNITS.iter().copied(),
                    ))
                    .help("or drop the suffix to use a plain scalar number")
                    .err());
            }
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

/// Record values keep the span that produced them: user records from their
/// literal, builtin results from the call that built them. Later stages
/// (lowering, graph preparation) read it back to name a failing value.
#[derive(Clone, Debug)]
pub struct Record {
    fields: BTreeMap<String, Value>,
    origin: Option<Origin>,
}
impl Record {
    pub fn new(fields: BTreeMap<String, Value>) -> Self {
        Self {
            fields,
            origin: None,
        }
    }
    pub fn with_origin(mut self, origin: Option<Origin>) -> Self {
        self.origin = origin;
        self
    }
    pub fn origin(&self) -> Option<&Origin> {
        self.origin.as_ref()
    }
    pub fn set_origin(&mut self, origin: Origin) {
        self.origin = Some(origin);
    }
}
impl std::ops::Deref for Record {
    type Target = BTreeMap<String, Value>;
    fn deref(&self) -> &Self::Target {
        &self.fields
    }
}
impl std::ops::DerefMut for Record {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.fields
    }
}
impl From<BTreeMap<String, Value>> for Record {
    fn from(fields: BTreeMap<String, Value>) -> Self {
        Self::new(fields)
    }
}

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
    Record(Rc<Record>),
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
    pub fn scalar(&self) -> Result<f64> {
        self.quantity(Unit::Scalar, 1.0)
    }
    /// A scalar uses the field's documented default unit; explicit units must agree.
    pub fn quantity(&self, unit: Unit, scale: f64) -> Result<f64> {
        match self {
            Self::Num(q) if q.unit == Unit::Scalar || q.unit == unit => {
                let value = q.number() * if q.unit == Unit::Scalar { 1. } else { scale };
                if !value.is_finite() {
                    bail!("number must be finite");
                }
                Ok(value)
            }
            _ => bail!("expected {unit:?} quantity (or a plain number)"),
        }
    }
    pub fn field_number(&self, field: &str) -> Result<f64> {
        let (unit, scale) = if field.ends_with("_ms") {
            (Unit::Seconds, 1000.)
        } else if field.ends_with("_hz") {
            (Unit::Hz, 1.)
        } else if field.ends_with("_db") || field == "gain" {
            (Unit::Db, 1.)
        } else if matches!(field, "tempo" | "bpm") {
            (Unit::Bpm, 1.)
        } else if matches!(field, "tail" | "offset" | "seconds") {
            (Unit::Seconds, 1.)
        } else {
            (Unit::Scalar, 1.)
        };
        self.quantity(unit, scale)
            .with_context(|| format!("field '{field}'"))
    }
    pub fn integer_in(&self, min: i64, max: i64) -> Result<i64> {
        use num_traits::ToPrimitive;
        let value = match self {
            Self::Num(Quantity {
                value: Number::Exact(v),
                unit: Unit::Scalar,
            }) if *v.denom() == 1 => Some(*v.numer()),
            Self::Num(Quantity {
                value: Number::Inexact(v),
                unit: Unit::Scalar,
            }) if v.is_finite() && v.fract() == 0. => v.to_i64(),
            _ => None,
        };
        value
            .filter(|v| (min..=max).contains(v))
            .ok_or_else(|| anyhow::anyhow!("expected unitless integer in {min}..={max}"))
    }
    pub fn scalar_exact(&self) -> Result<Beat> {
        match self {
            Self::Num(q) if q.unit == Unit::Scalar => q.value.exact(),
            _ => bail!("expected scalar"),
        }
    }
    /// Language equality is independent of inspection serialization and origins.
    pub fn semantic_eq(&self, other: &Self) -> Result<bool> {
        Ok(match (self, other) {
            (Self::Invalid(message), _) | (_, Self::Invalid(message)) => bail!("{message}"),
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Num(a), Self::Num(c)) => {
                let normalize = |q: &Quantity| -> Result<Quantity> {
                    if q.unit == Unit::Bar {
                        Ok(Quantity {
                            value: q.value.arithmetic("*", b(4).into(), false)?,
                            unit: Unit::Beat,
                        })
                    } else {
                        Ok(q.clone())
                    }
                };
                normalize(a)? == normalize(c)?
            }
            (Self::Array(a), Self::Array(b)) => {
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (a, b) in a.iter().zip(b.iter()) {
                    if !a.semantic_eq(b)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Self::Record(a), Self::Record(b)) => {
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (key, a) in a.iter() {
                    let Some(b) = b.get(key) else {
                        return Ok(false);
                    };
                    if !a.semantic_eq(b)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Self::Pattern(a), Self::Pattern(b)) => a == b,
            (Self::Function(a), Self::Function(b)) => Rc::ptr_eq(a, b),
            (Self::Builtin(a, ar), Self::Builtin(b, br)) => {
                if a.strip_prefix("std.").unwrap_or(a) != b.strip_prefix("std.").unwrap_or(b) {
                    return Ok(false);
                }
                match (ar, br) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.semantic_eq(b)?,
                    _ => false,
                }
            }
            _ => false,
        })
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
    pub fn record(&self) -> Result<&Record> {
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
    provenance: crate::provenance::Arena,
    call_origin: Option<Origin>,
    pub context: crate::host::HostContext,
    pub expansion_limits: crate::limits::ExpansionLimits,
    loader: Rc<dyn super::SourceLoader>,
    pub dependencies: Vec<PathBuf>,
    pub path: PathBuf,
    cache: BTreeMap<PathBuf, Value>,
    sources: BTreeMap<PathBuf, Arc<SourceFile>>,
    active: BTreeSet<PathBuf>,
    pub steps: usize,
    pub depth: usize,
}
impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}
/// Bundled standard-library modules, keyed by their import path. The table also
/// backs the "known modules" hint for misspelled imports.
pub const STANDARD_MODULES: &[(&str, &str)] = &[
    ("std/prelude", include_str!("../../std/prelude.muz")),
    ("std/patterns", include_str!("../../std/patterns.muz")),
    ("std/arrange", include_str!("../../std/arrange.muz")),
    ("std/performance", include_str!("../../std/performance.muz")),
    ("std/catalogs", include_str!("../../std/catalogs.muz")),
    ("std/music", include_str!("../../std/music.muz")),
    ("std/tonal", include_str!("../../std/tonal.muz")),
    ("std/piano", include_str!("../../std/piano.muz")),
    ("std/grooves", include_str!("../../std/grooves.muz")),
    ("std/sampler", include_str!("../../std/sampler.muz")),
    ("std/synthesis", include_str!("../../std/synthesis.muz")),
    ("std/instrument", include_str!("../../std/instrument.muz")),
    ("std/signal", include_str!("../../std/signal.muz")),
    ("std/mix", include_str!("../../std/mix.muz")),
];

impl Evaluator {
    pub fn new() -> Self {
        Self::with_loader(Rc::new(super::FileSourceLoader))
    }
    pub fn with_loader(loader: Rc<dyn super::SourceLoader>) -> Self {
        Self::with_context(
            loader,
            crate::host::current().unwrap_or_else(crate::host::HostContext::cli),
        )
    }
    pub fn with_context(
        loader: Rc<dyn super::SourceLoader>,
        context: crate::host::HostContext,
    ) -> Self {
        Self {
            provenance: Default::default(),
            call_origin: None,
            expansion_limits: context.expansion,
            context,
            loader,
            dependencies: vec![],
            path: PathBuf::from("<source>"),
            cache: BTreeMap::new(),
            sources: BTreeMap::new(),
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
        let path = self.loader.resolve(path).map_err(|error| {
            Diagnostic::new(format!("cannot read {}: {error}", path.display())).err()
        })?;
        if let Some(v) = self.cache.get(&path) {
            return Ok(v.clone());
        }
        if !self.active.insert(path.clone()) {
            return Err(
                Diagnostic::new(format!("circular import of {}", path.display()))
                    .help("break the cycle by moving the shared definitions into a third module")
                    .err(),
            );
        }
        self.dependencies.push(path.clone());
        let source = self.loader.read(&path).map_err(|error| {
            Diagnostic::new(format!("cannot read {}: {error}", path.display())).err()
        })?;
        let previous = std::mem::replace(&mut self.path, path.clone());
        let result = self.source(&source);
        self.path = previous;
        self.active.remove(&path);
        let value = result.map_err(|error| Diagnostic::named(error, &path))?;
        self.cache.insert(path, value.clone());
        Ok(value)
    }
    /// Resolve `contrib/<pack>/<module>` against the contrib library root.
    /// Contrib modules are ordinary files, so their imports are watched and
    /// reloaded like any other source.
    fn contrib_module(&mut self, module: &str) -> Result<Value> {
        use std::path::Component;
        if module.is_empty()
            || Path::new(module)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            bail!(
                "contrib import must name a module inside the contrib library: 'contrib/{module}'"
            );
        }
        let root = self.loader.contrib_root()?;
        let file = root.join(format!("{module}.muz"));
        if self.loader.resolve(&file).is_err() {
            let known = self.loader.contrib_modules();
            return Err(Diagnostic::new(format!(
                "unknown contrib module 'contrib/{module}': {} does not exist",
                file.display()
            ))
            .helps(super::diagnostic::suggest_vocabulary(
                "available modules",
                &format!("contrib/{module}"),
                known.iter().map(String::as_str),
            ))
            .err());
        }
        self.module(&file)
    }
    pub fn source(&mut self, source: &str) -> Result<Value> {
        let context = self.context.clone();
        context.run(|| self.source_inner(source))
    }
    fn source_inner(&mut self, source: &str) -> Result<Value> {
        let program = super::parse(source).map_err(|error| syntax(&self.path, source, error))?;
        self.sources.insert(
            self.path.clone(),
            Arc::new(SourceFile::new(self.path.clone(), Arc::from(source))),
        );
        let mut env = Env::new();
        let result = self.program(&program, &mut env)?;
        env.insert("__result".into(), result);
        Ok(Value::Record(Rc::new(Record::new(env))))
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
                Stmt::Import(path, alias, at, end) => {
                    let value = self.import(path, *at, *end)?;
                    env.insert(alias.clone(), value);
                }
                Stmt::Expr(e) => {
                    result = self.eval(e, env)?;
                }
            }
        }
        Ok(result)
    }
    fn import(&mut self, path: &str, at: usize, end: usize) -> Result<Value> {
        let loaded = (|| -> Result<Value> {
            if path == "std" {
                return Ok(Value::Builtin(String::new(), None));
            }
            if let Some(text) = STANDARD_MODULES
                .iter()
                .find(|(name, _)| *name == path)
                .map(|(_, text)| *text)
            {
                return self.standard_module(path.strip_prefix("std/").unwrap(), text);
            }
            if path.starts_with("std/") {
                let known: Vec<&str> = STANDARD_MODULES.iter().map(|(name, _)| *name).collect();
                return Err(Diagnostic::new(format!("unknown standard module '{path}'"))
                    .helps(super::diagnostic::suggest_vocabulary(
                        "known modules",
                        path,
                        known.iter().copied(),
                    ))
                    .err());
            }
            if let Some(module) = path.strip_prefix("contrib/") {
                return self.contrib_module(module);
            }
            let file = self.path.parent().unwrap_or(Path::new(".")).join(path);
            self.module(&file)
        })();
        loaded.map_err(|error| match self.sources.get(&self.path) {
            Some(source) => Diagnostic::attach(
                error,
                Location::span(&self.path, &source.text, at, end),
                false,
            ),
            None => error,
        })
    }
    pub fn eval(&mut self, n: &Node, env: &Env) -> Result<Value> {
        let result = if self.context.is_cancelled() {
            Err(Diagnostic::new("evaluation cancelled")
                .help("this run was interrupted; run it again to see the full result")
                .err())
        } else {
            self.steps += 1;
            if self.steps > self.context.evaluation_steps {
                Err(Diagnostic::new(format!(
                    "evaluation budget exceeded ({} operations)",
                    self.context.evaluation_steps
                ))
                .help("simplify the expression, or split the work into smaller definitions")
                .err())
            } else {
                self.eval_inner(n, env)
            }
        };
        result.map_err(|error| match self.sources.get(&self.path) {
            Some(source) => Diagnostic::attach(
                error,
                Location::new(self.path.clone(), &source.text, n),
                matches!(n.kind, Expr::Call(..)),
            ),
            None => error,
        })
    }
    /// Best-effort span of `n` in the module currently being evaluated.
    fn origin(&self, n: &Node) -> Option<Origin> {
        self.sources
            .get(&self.path)
            .map(|file| Origin::new(file.clone(), n.at, n.end))
    }
    /// Names a call site could have meant: locals in scope, the standard-library
    /// exports and the builtin function names.
    fn known_names(&mut self, env: &Env) -> Vec<String> {
        let mut names: Vec<String> = env.keys().cloned().collect();
        if let Ok(library) = self.standard_module("prelude", include_str!("../../std/prelude.muz"))
            && let Some(exports) = library
                .get("__result")
                .and_then(|value| value.record().ok())
        {
            names.extend(exports.keys().cloned());
        }
        names.extend(
            super::builtins::names()
                .iter()
                .map(|name| (*name).to_owned()),
        );
        names.sort();
        names.dedup();
        names
    }
    /// Resolve one binding exported by the source prelude. Prelude data and
    /// functions are both ordinary values; call dispatch must not be the only
    /// path that can reach them.
    fn prelude_export(&mut self, name: &str) -> Result<Option<Value>> {
        let library = self.standard_module("prelude", include_str!("../../std/prelude.muz"))?;
        let exports = library
            .record()?
            .get("__result")
            .ok_or_else(|| anyhow::anyhow!("standard prelude has no exports"))?
            .record()?;
        Ok(exports.get(name).cloned())
    }
    /// Remember where a value produced by this expression came from.
    fn origin_of(&self, mut value: Value, n: &Node) -> Value {
        if let Some(origin) = self.origin(n)
            && let Value::Record(record) = &mut value
            && let Some(record) = Rc::get_mut(record)
        {
            record.set_origin(origin);
        }
        value
    }
    fn eval_inner(&mut self, n: &Node, env: &Env) -> Result<Value> {
        Ok(match &n.kind {
            Expr::Number(s) => Value::Num(Quantity::parse(s)?),
            Expr::String(s) => Value::Str(s.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Ident(s) => {
                if let Some(value) = env.get(s) {
                    value.clone()
                } else if s == "null" {
                    Value::Null
                } else if super::builtins::names().contains(&s.as_str()) {
                    Value::Builtin(s.clone(), None)
                } else if let Some(value) = self.prelude_export(s)? {
                    value
                } else {
                    Value::Builtin(s.clone(), None)
                }
            }
            Expr::Array(v) => {
                Value::Array(v.iter().map(|n| self.eval(n, env)).collect::<Result<_>>()?)
            }
            Expr::Record(r) => {
                let mut out = BTreeMap::new();
                for (k, n) in r {
                    if out.insert(k.clone(), self.eval(n, env)?).is_some() {
                        return Err(Diagnostic::new(format!("duplicate record field '{k}'"))
                            .help("keep one of the repeated field definitions")
                            .err());
                    }
                }
                Value::Record(Rc::new(Record::new(out).with_origin(self.origin(n))))
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
                    Value::Record(r) => match r.get(key).cloned() {
                        Some(value) => value,
                        None => {
                            let fields: Vec<&str> = r.keys().map(String::as_str).collect();
                            return Err(Diagnostic::new(format!("record has no field '{key}'"))
                                .helps(super::diagnostic::suggest_vocabulary(
                                    "fields",
                                    key,
                                    fields.iter().copied(),
                                ))
                                .origin(r.origin())
                                .err());
                        }
                    },
                    Value::Builtin(prefix, None) => Value::Builtin(
                        if prefix.is_empty() {
                            key.clone()
                        } else {
                            format!("{prefix}.{key}")
                        },
                        None,
                    ),
                    Value::Pattern(p) if key == "span" => Value::beat(p.span),
                    Value::Pattern(p) if key == "has_clock_timing" => {
                        Value::Bool(p.notes.iter().any(|note| note.clock.is_some()))
                    }
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
                        let idx = i.integer_in(i64::MIN, i64::MAX)?;
                        let idx = if idx < 0 { a.len() as i64 + idx } else { idx };
                        a.get(idx as usize).cloned().ok_or_else(|| {
                            Diagnostic::new(format!("index {idx} out of range"))
                                .help(format!(
                                    "list has {} items; use 0..{} or a negative index from the end",
                                    a.len(),
                                    a.len().saturating_sub(1)
                                ))
                                .err()
                        })?
                    }
                    Value::Record(r) => match r.get(i.text()?).cloned() {
                        Some(value) => value,
                        None => {
                            let fields: Vec<&str> = r.keys().map(String::as_str).collect();
                            return Err(Diagnostic::new(format!(
                                "record has no key '{}'",
                                i.text()?
                            ))
                            .helps(super::diagnostic::suggest_vocabulary(
                                "fields",
                                i.text()?,
                                fields.iter().copied(),
                            ))
                            .origin(r.origin())
                            .err());
                        }
                    },
                    _ => bail!("indexing needs list or record"),
                }
            }
            Expr::Call(f, args) => {
                let f = self.eval(f, env)?;
                let mut vs = vec![];
                for (k, v) in args {
                    vs.push((k.clone(), self.eval(v, env)?));
                }
                let origin = self.origin(n);
                let previous = std::mem::replace(&mut self.call_origin, origin);
                let called = self.call(f, vs);
                self.call_origin = previous;
                match called {
                    Ok(value) => self.origin_of(value, n),
                    Err(error) => {
                        // The identifier was unknown: offer the names this call
                        // site could use before the span is attached.
                        let Some(unknown) =
                            error.downcast_ref::<super::builtins::UnknownFunction>()
                        else {
                            return Err(error);
                        };
                        let missing = unknown
                            .0
                            .strip_prefix("std.")
                            .unwrap_or(&unknown.0)
                            .to_owned();
                        let known = self.known_names(env);
                        let names: Vec<&str> = known.iter().map(String::as_str).collect();
                        let message = format!("{error:#}");
                        return Err(Diagnostic::new(message)
                            .helps(super::diagnostic::suggest_vocabulary(
                                "known functions",
                                &missing,
                                names,
                            ))
                            .err());
                    }
                }
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
                let origin = self.call_origin.clone();
                match super::builtins::call(self, &name, args.clone()) {
                    Err(error)
                        if error
                            .downcast_ref::<super::builtins::UnknownFunction>()
                            .is_some_and(|missing| {
                                missing.0 == name.strip_prefix("std.").unwrap_or(&name)
                            }) =>
                    {
                        let bare = name.strip_prefix("std.").unwrap_or(&name);
                        match self.prelude_export(bare)? {
                            Some(function) => self.call(function, args),
                            None => Err(error),
                        }
                    }
                    Ok(mut value) => {
                        if self.context.provenance
                            && let (Value::Pattern(p), Some(origin)) = (&mut value, origin)
                        {
                            let inputs = args.into_iter().map(|(_, v)| v).collect::<Vec<_>>();
                            self.provenance.trace(
                                Arc::make_mut(p),
                                &inputs,
                                &origin,
                                name.strip_prefix("std.").unwrap_or(&name),
                                self.expansion_limits.bytes,
                            )?;
                        }
                        Ok(value)
                    }
                    Err(error) => Err(error),
                }
            }
            Value::Function(f) => {
                // Defaults and body both belong to the definition's module.
                let old = std::mem::replace(&mut self.path, f.path.clone());
                let value = (|| {
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
                                return Err(argument_error(
                                    &f,
                                    format!("duplicate argument {name}"),
                                ));
                            }
                            v.clone()
                        } else if let Some((i, (_, v))) = positional.get(pos) {
                            pos += 1;
                            used.insert(*i);
                            v.clone()
                        } else if let Some(n) = default {
                            self.eval(n, &env)?
                        } else {
                            return Err(argument_error(&f, format!("missing argument '{name}'")));
                        };
                        env.insert(name.clone(), v);
                    }
                    if used.len() != args.len() {
                        return Err(argument_error(
                            &f,
                            "unexpected or repeated function argument".into(),
                        ));
                    }
                    self.eval(&f.body, &env)
                })();
                self.path = old;
                value
            }
            _ => Err(Diagnostic::new(format!("{} is not callable", f.kind()))
                .help("call a function, or remove the call parentheses")
                .err()),
        }
    }
}
/// Argument failures name the parameters the definition accepts.
fn argument_error(f: &Function, message: String) -> anyhow::Error {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|(name, default)| match default {
            Some(_) => format!("{name} (optional)"),
            None => name.clone(),
        })
        .collect();
    let mut diagnostic = Diagnostic::new(message);
    if !params.is_empty() {
        diagnostic = diagnostic.help(format!("parameters: {}", params.join(", ")));
    }
    diagnostic.err()
}
/// Explicit override, checkout library, then the installed user data library.
pub(crate) fn contrib_root() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("MUZ_CONTRIB_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.is_dir() {
            bail!("MUZ_CONTRIB_DIR {} is not a directory", dir.display());
        }
        return Ok(dir);
    }
    let exe = std::env::current_exe().context("locating the muz executable")?;
    let installed = installed_contrib_root(
        std::env::var_os("XDG_DATA_HOME").as_deref().map(Path::new),
        std::env::var_os("HOME").as_deref().map(Path::new),
    );
    discover_contrib_root(&exe, installed.as_deref())
}

fn installed_contrib_root(data_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    // XDG base directories must be absolute; empty/relative values are ignored.
    data_home
        .filter(|path| path.is_absolute())
        .map(Path::to_path_buf)
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|path| path.join(".local/share"))
        })
        .map(|base| base.join("muz/contrib"))
}

fn discover_contrib_root(exe: &Path, installed: Option<&Path>) -> Result<PathBuf> {
    let checkout = exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|checkout| checkout.join("contrib"));
    if let Some(dir) = checkout.as_ref().filter(|dir| dir.is_dir()) {
        return Ok(dir.clone());
    }
    if let Some(dir) = installed.filter(|dir| dir.is_dir()) {
        return Ok(dir.to_path_buf());
    }
    bail!(
        "contrib library not found for {}; checked checkout {} and installed {}; \
         run install.sh or set MUZ_CONTRIB_DIR to a contrib directory",
        exe.display(),
        checkout
            .as_deref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "(unavailable)".into()),
        installed
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "(unavailable: no absolute XDG_DATA_HOME or HOME)".into()),
    )
}

#[cfg(test)]
mod contrib_discovery_tests {
    use super::*;

    #[test]
    fn installed_library_is_found_outside_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("data/muz/contrib");
        std::fs::create_dir_all(&installed).unwrap();
        let exe = dir.path().join("cargo/bin/muz");
        assert_eq!(
            discover_contrib_root(&exe, Some(&installed)).unwrap(),
            installed
        );
        // A checkout build continues to use its own library even after installation.
        let checkout = dir.path().join("checkout/contrib");
        std::fs::create_dir_all(&checkout).unwrap();
        let exe = dir.path().join("checkout/target/debug/muz");
        assert_eq!(
            discover_contrib_root(&exe, Some(&installed)).unwrap(),
            checkout
        );
    }

    #[test]
    fn installed_data_location_obeys_xdg_and_home_defaults() {
        let home = Path::new("/home/composer");
        assert_eq!(
            installed_contrib_root(Some(Path::new("/custom/data")), Some(home)),
            Some(PathBuf::from("/custom/data/muz/contrib")),
        );
        for data_home in [None, Some(Path::new("")), Some(Path::new("relative"))] {
            assert_eq!(
                installed_contrib_root(data_home, Some(home)),
                Some(home.join(".local/share/muz/contrib")),
            );
        }
        assert_eq!(installed_contrib_root(None, None), None);
    }

    #[test]
    fn missing_library_reports_both_locations_and_remedies() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("cargo/bin/muz");
        let installed = dir.path().join("data/muz/contrib");
        let error = discover_contrib_root(&exe, Some(&installed))
            .unwrap_err()
            .to_string();
        for expected in [
            dir.path().join("contrib").display().to_string(),
            installed.display().to_string(),
            "install.sh".into(),
            "MUZ_CONTRIB_DIR".into(),
        ] {
            assert!(error.contains(&expected), "{error}");
        }
    }
}

fn binary(op: &str, mut x: Value, mut y: Value) -> Result<Value> {
    if let (Value::Num(a), Value::Num(b)) = (&mut x, &mut y)
        && matches!(
            (a.unit, b.unit),
            (Unit::Beat, Unit::Bar) | (Unit::Bar, Unit::Beat)
        )
    {
        for q in [a, b] {
            if q.unit == Unit::Bar {
                q.value = q.value.arithmetic("*", music::b(4).into(), false)?;
                q.unit = Unit::Beat;
            }
        }
    }
    if op == "==" || op == "!=" {
        let eq = x.semantic_eq(&y)?;
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
