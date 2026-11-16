use super::cache::{self, Event, ModuleCache, ModuleRecord, Recording};
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
    /// Exact score time: inexact values are rounded to the score-time grid
    /// (`music::time`), exact values are returned unchanged.
    pub fn time(self) -> Result<Beat> {
        match self {
            Self::Exact(x) => Ok(x),
            Self::Inexact(x) => music::time(x),
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
                bail!("exact number overflow: {a} {op} {b} does not fit a 64-bit rational");
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
        let mut i = s
            .find(|c: char| c.is_ascii_alphabetic() || c == '%')
            .unwrap_or(s.len());
        let exponent = super::parser::exponent_len(&s.as_bytes()[i..]);
        let mut value = if exponent == 0 {
            Number::Exact(music::decimal(&s[..i])?)
        } else {
            // Scientific notation is inexact: the nearest f64, at any magnitude.
            let mantissa = s[..i].strip_prefix('-').unwrap_or(&s[..i]);
            let (whole, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
            anyhow::ensure!(
                !(whole.is_empty() && frac.is_empty())
                    && (whole.bytes().chain(frac.bytes())).all(|c| c.is_ascii_digit()),
                "invalid number '{s}'"
            );
            i += exponent;
            let x: f64 = s[..i].parse()?;
            anyhow::ensure!(x.is_finite(), "number '{}' is too large", &s[..i]);
            Number::Inexact(x)
        };
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
    /// The quantity as a source expression that evaluates to the same value.
    pub fn source(&self) -> String {
        let suffix = match self.unit {
            Unit::Scalar => "",
            Unit::Beat => "b",
            Unit::Bar => "bar",
            Unit::Seconds => "s",
            Unit::Db => "dB",
            Unit::Hz => "Hz",
            Unit::Bpm => "bpm",
        };
        match self.value {
            Number::Inexact(x) => format!("{x}{suffix}"),
            Number::Exact(r) => {
                let (n, d) = (*r.numer(), *r.denom());
                let (mut rest, mut terminating) = (d, true);
                for f in [2, 5] {
                    while rest % f == 0 {
                        rest /= f;
                    }
                }
                if rest != 1 {
                    terminating = false;
                }
                if d == 1 {
                    format!("{n}{suffix}")
                } else if terminating {
                    format!("{}{suffix}", real(r))
                } else {
                    // No finite decimal: write the exact ratio instead.
                    format!("{n}{suffix}/{d}")
                }
            }
        }
    }
    pub fn beats(&self, meter: f64) -> Result<Beat> {
        match self.unit {
            Unit::Scalar | Unit::Beat => self.value.time(),
            Unit::Bar => self
                .value
                .arithmetic("*", music::time(meter)?.into(), false)?
                .time(),
            _ => bail!("expected musical duration, got {:?}", self.unit),
        }
    }
}
/// Lexical bindings: a scope's own names over the shared, immutable scope it
/// was opened in. A call or closure opens a scope over its captured one, so
/// neither copies the whole module environment.
#[derive(Clone, Debug, Default)]
pub struct Env {
    parent: Option<Rc<Env>>,
    local: BTreeMap<String, Value>,
}
impl Env {
    pub fn new() -> Self {
        Self::default()
    }
    fn within(parent: Rc<Env>) -> Self {
        Self {
            parent: Some(parent),
            local: BTreeMap::new(),
        }
    }
    pub fn get(&self, name: &str) -> Option<&Value> {
        let mut scope = self;
        loop {
            if let Some(value) = scope.local.get(name) {
                return Some(value);
            }
            scope = scope.parent.as_deref()?;
        }
    }
    pub fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
    pub fn insert(&mut self, name: String, value: Value) {
        self.local.insert(name, value);
    }
    fn keys(&self) -> Vec<String> {
        let mut names: Vec<String> = self.local.keys().cloned().collect();
        if let Some(parent) = &self.parent {
            names.extend(parent.keys());
        }
        names
    }
}
impl FromIterator<(String, Value)> for Env {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(items: I) -> Self {
        Self {
            parent: None,
            local: items.into_iter().collect(),
        }
    }
}

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
    /// The `fn` name, or `None` for a lambda. Used only for diagnostics.
    pub name: Option<String>,
    pub params: Vec<(String, Option<Node>)>,
    pub body: Node,
    pub env: Rc<Env>,
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
            Self::Num(q) if q.unit == Unit::Scalar => q.value.time(),
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
    /// JSON for `muz eval`: like [`Self::json`], except that a quantity with a
    /// unit prints as a string in source spelling (`"1b"`, `"-6dB"`), so a
    /// printed value can be pasted back into source and keeps its unit.
    /// Scalars stay bare numbers. A pattern prints as the record
    /// [`super::builtins::pattern_dump`] builds, so its times carry units too.
    pub fn display_json(&self) -> serde_json::Value {
        match self {
            Self::Num(q) if q.unit != Unit::Scalar => serde_json::json!(q.source()),
            Self::Array(v) => {
                serde_json::Value::Array(v.iter().map(Self::display_json).collect())
            }
            Self::Record(r) => serde_json::Value::Object(
                r.iter()
                    .map(|(k, v)| (k.clone(), v.display_json()))
                    .collect(),
            ),
            Self::Pattern(p) => super::builtins::pattern_dump(p).display_json(),
            _ => self.json(),
        }
    }
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Record(r) => r.get(key),
            _ => None,
        }
    }
}
/// A module's short name in reports: its file name, or `<std/name>`.
pub fn module_label(p: &Path) -> String {
    let text = p.display().to_string();
    if text.starts_with('<') {
        return text;
    }
    p.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or(text)
}
/// The evaluation budget when `MUZ_EVAL_BUDGET` is not set.
pub const DEFAULT_EVALUATION_STEPS: usize = 5_000_000;
/// The largest evaluation budget `MUZ_EVAL_BUDGET` accepts.
pub const MAX_EVALUATION_STEPS: usize = 1_000_000_000;
/// The process's evaluation budget: `MUZ_EVAL_BUDGET`, else the default.
pub fn evaluation_budget() -> Result<usize, String> {
    static BUDGET: std::sync::OnceLock<Result<usize, String>> = std::sync::OnceLock::new();
    BUDGET
        .get_or_init(|| match std::env::var("MUZ_EVAL_BUDGET") {
            Err(std::env::VarError::NotPresent) => Ok(DEFAULT_EVALUATION_STEPS),
            value => value
                .ok()
                .and_then(|v| v.replace('_', "").parse::<usize>().ok())
                .filter(|n| (1..=MAX_EVALUATION_STEPS).contains(n))
                .ok_or_else(|| {
                    format!("MUZ_EVAL_BUDGET must be an integer in 1..={MAX_EVALUATION_STEPS}")
                }),
        })
        .clone()
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
    /// Operations spent evaluating each module's own top level (imports it
    /// loads are counted for them), for a budget error to name the spenders.
    module_steps: BTreeMap<PathBuf, usize>,
    /// Modules being evaluated: path, steps at entry, steps of nested loads.
    module_stack: Vec<(PathBuf, usize, usize)>,
    /// The first module this evaluator loads: the evaluation's entry point.
    entry: Option<PathBuf>,
    pub depth: usize,
    /// Names of `fn` definitions whose bodies are currently executing, so an
    /// attempted self-reference can be diagnosed as unsupported recursion.
    running: Vec<String>,
    /// Results of `memo` functions, keyed by the memo's id and its arguments;
    /// each entry keeps its arguments alive so pointer keys stay unique.
    pub(crate) memo: std::collections::HashMap<(usize, String), (Vec<Value>, Value)>,
    /// Modules kept from earlier evaluations, when the host keeps them.
    persist: Option<Rc<ModuleCache>>,
    /// What each module being evaluated has read, while `persist` is set.
    recordings: Vec<Recording>,
    /// The recording of the module `counted` finished last.
    finished: Option<Finished>,
    /// Whether each kept module checked so far can be reused in this evaluation.
    validity: std::collections::HashMap<PathBuf, bool>,
    /// Memo entries added, in order, while `persist` is set.
    memo_journal: Vec<(usize, String)>,
    /// The deepest call reached in the module being evaluated.
    max_depth: usize,
}
/// A module's finished recording.
struct Finished {
    events: Vec<Event>,
    steps: usize,
    inclusive_steps: usize,
    bytes: usize,
    inclusive_bytes: usize,
    depth: usize,
    interned: Vec<crate::provenance::Interned>,
    memo: Vec<(usize, String)>,
    uncacheable: bool,
}
impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}
/// The source of the standard module `name` (`prelude` or a `std/` import).
fn standard_text(name: &str) -> &'static str {
    if name == "prelude" {
        return include_str!("../../std/prelude.muz");
    }
    STANDARD_MODULES
        .iter()
        .find(|(path, _)| path.strip_prefix("std/") == Some(name))
        .map_or("", |(_, text)| text)
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
    ("std/physical", include_str!("../../std/physical.muz")),
    ("std/woodwind_calibration", include_str!("../../std/woodwind_calibration.muz")),
    ("std/woodwinds", include_str!("../../std/woodwinds.muz")),
    ("std/mix", include_str!("../../std/mix.muz")),
    ("std/checks", include_str!("../../std/checks.muz")),
    ("std/gestures", include_str!("../../std/gestures.muz")),
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
            module_steps: BTreeMap::new(),
            module_stack: Vec::new(),
            entry: None,
            depth: 0,
            running: Vec::new(),
            memo: Default::default(),
            persist: None,
            recordings: Vec::new(),
            finished: None,
            validity: Default::default(),
            memo_journal: Vec::new(),
            max_depth: 0,
        }
    }
    /// Reuse and keep evaluated modules in `cache` (see [`ModuleCache`]).
    pub fn with_cache(
        loader: Rc<dyn super::SourceLoader>,
        context: crate::host::HostContext,
        cache: Rc<ModuleCache>,
    ) -> Self {
        let mut e = Self::with_context(loader, context);
        e.provenance.start_journal();
        e.persist = Some(cache);
        e
    }
    /// Clear `cache` unless it was kept for the same root and settings.
    pub(super) fn adopt_cache(&self, root: &Path) {
        let Some(persist) = &self.persist else {
            return;
        };
        let limits = self.expansion_limits;
        let config = cache::Config {
            root: self.loader.resolve(root).unwrap_or_else(|_| root.to_path_buf()),
            provenance: self.context.provenance,
            expansion: (limits.notes, limits.controls, limits.raw, limits.bytes),
            evaluation_steps: self.context.evaluation_steps,
            contrib: self.loader.contrib_root().ok(),
        };
        persist.reused.borrow_mut().clear();
        let mut state = persist.state.borrow_mut();
        if state.config.as_ref() != Some(&config) {
            *state = cache::State {
                config: Some(config),
                records: Default::default(),
            };
        }
    }
    /// Keep a memo result; a kept module restores the entries its evaluation added.
    pub(crate) fn memo_insert(&mut self, key: (usize, String), entry: (Vec<Value>, Value)) {
        if self.persist.is_some() {
            self.memo_journal.push(key.clone());
        }
        self.memo.insert(key, entry);
    }
    /// The module being evaluated read something besides its inputs, so its
    /// value cannot be kept.
    pub(crate) fn mark_uncacheable(&mut self) {
        for recording in &mut self.recordings {
            recording.uncacheable = true;
        }
    }
    /// A builtin read the file `resolved` (resolved from `requested`).
    pub(crate) fn note_asset(&mut self, requested: &Path, resolved: &Path) {
        if self.persist.is_none() || self.recordings.is_empty() {
            return;
        }
        match crate::assets::resolver().version(resolved) {
            Ok(version) => {
                let recording = self.recordings.last_mut().expect("checked above");
                recording.events.push(Event::Asset {
                    requested: requested.to_path_buf(),
                    resolved: resolved.to_path_buf(),
                    version,
                });
            }
            Err(_) => self.mark_uncacheable(),
        }
    }
    /// The module being evaluated loaded (or found loaded) the module `key`.
    fn note_import(&mut self, requested: Option<&Path>, key: &Path) {
        let Some(persist) = &self.persist else {
            return;
        };
        let Some(recording) = self.recordings.last_mut() else {
            return;
        };
        if recording.seen.contains(key) {
            return;
        }
        let generation = persist.state.borrow().records.get(key).map(|r| r.generation);
        let Some(generation) = generation else {
            recording.uncacheable = true;
            return;
        };
        recording.seen.insert(key.to_path_buf());
        recording.events.push(match requested {
            Some(requested) => Event::Import {
                requested: requested.to_path_buf(),
                resolved: key.to_path_buf(),
                generation,
            },
            None => Event::Standard {
                name: key
                    .to_string_lossy()
                    .trim_start_matches("<std/")
                    .trim_end_matches('>')
                    .to_owned(),
                generation,
            },
        });
    }
    /// Keep the module `key` just evaluated from `text`.
    fn keep(&mut self, key: &Path, text: Arc<str>, value: &Value) {
        let (Some(persist), Some(finished)) = (self.persist.clone(), self.finished.take()) else {
            return;
        };
        if finished.uncacheable {
            persist.state.borrow_mut().records.remove(key);
            return;
        }
        let memo = finished
            .memo
            .into_iter()
            .filter_map(|k| self.memo.get(&k).map(|entry| (k, entry.clone())))
            .collect();
        let record = ModuleRecord {
            generation: cache::next_generation(),
            text,
            source: self.sources.get(key).cloned(),
            value: value.clone(),
            events: finished.events,
            steps: finished.steps,
            inclusive_steps: finished.inclusive_steps,
            bytes: finished.bytes,
            inclusive_bytes: finished.inclusive_bytes,
            depth: finished.depth,
            interned: finished.interned,
            memo,
        };
        persist
            .state
            .borrow_mut()
            .records
            .insert(key.to_path_buf(), Rc::new(record));
        self.validity.insert(key.to_path_buf(), true);
    }
    /// The kept record of `key`, when this evaluation can reuse it.
    fn reusable(&mut self, key: &Path) -> Option<Rc<ModuleRecord>> {
        let persist = self.persist.clone()?;
        if !self.valid(&persist, key) {
            return None;
        }
        let record = persist.state.borrow().records.get(key).cloned()?;
        // A reuse past a budget falls back to evaluation, which fails as a
        // fresh one does.
        let fits = self.steps.saturating_add(record.inclusive_steps) <= self.context.evaluation_steps
            && self.provenance.bytes().saturating_add(record.inclusive_bytes)
                <= self.expansion_limits.bytes
            && self.depth + record.depth <= 128;
        fits.then_some(record)
    }
    fn valid(&mut self, persist: &ModuleCache, key: &Path) -> bool {
        if let Some(valid) = self.validity.get(key) {
            return *valid;
        }
        let record = persist.state.borrow().records.get(key).cloned();
        let valid = record.is_some_and(|record| self.unchanged(persist, key, &record));
        self.validity.insert(key.to_path_buf(), valid);
        valid
    }
    fn unchanged(&mut self, persist: &ModuleCache, key: &Path, record: &ModuleRecord) -> bool {
        if !cache::is_standard(key)
            && !self
                .loader
                .read(key)
                .is_ok_and(|text| *text == *record.text)
        {
            return false;
        }
        let generation =
            |key: &Path| persist.state.borrow().records.get(key).map(|r| r.generation);
        for event in &record.events {
            let unchanged = match event {
                Event::Import {
                    requested,
                    resolved,
                    generation: g,
                } => {
                    self.loader.resolve(requested).ok().as_ref() == Some(resolved)
                        && self.valid(persist, resolved)
                        && generation(resolved) == Some(*g)
                }
                Event::Standard { name, generation: g } => {
                    let key = cache::standard_key(name);
                    self.valid(persist, &key) && generation(&key) == Some(*g)
                }
                Event::Asset {
                    requested,
                    resolved,
                    version,
                } => {
                    crate::assets::resolve(requested).ok().as_ref() == Some(resolved)
                        && crate::assets::resolver().version(resolved).ok() == Some(*version)
                }
            };
            if !unchanged {
                return false;
            }
        }
        true
    }
    /// Load `key` from its kept record: load what it loaded, in order, and
    /// restore its own operations, provenance and memo entries.
    fn replay(&mut self, key: &Path, record: &ModuleRecord) -> Result<Value> {
        self.active.insert(key.to_path_buf());
        if !cache::is_standard(key) {
            self.dependencies.push(key.to_path_buf());
        }
        if let Some(source) = &record.source {
            self.sources.insert(key.to_path_buf(), source.clone());
        }
        let previous = std::mem::replace(&mut self.path, key.to_path_buf());
        let result = self.counted(key, |e| -> Result<Value> {
            for event in &record.events {
                crate::host::check_cancelled()?;
                match event {
                    Event::Import { requested, .. } => {
                        e.module(requested)?;
                    }
                    Event::Standard { name, .. } => {
                        e.standard_module(name, standard_text(name))?;
                    }
                    Event::Asset { resolved, .. } => {
                        if !e.dependencies.contains(resolved) {
                            e.dependencies.push(resolved.clone());
                        }
                    }
                }
            }
            e.steps += record.steps;
            e.provenance
                .restore(&record.interned, record.bytes, e.expansion_limits.bytes)?;
            for (k, entry) in &record.memo {
                e.memo.entry(k.clone()).or_insert_with(|| entry.clone());
            }
            e.max_depth = e.max_depth.max(e.depth + record.depth);
            Ok(record.value.clone())
        });
        self.finished = None;
        self.path = previous;
        self.active.remove(key);
        let value = result.map_err(|error| Diagnostic::named(error, key))?;
        self.cache.insert(key.to_path_buf(), value.clone());
        if let Some(persist) = &self.persist {
            persist.reused.borrow_mut().push(key.to_path_buf());
        }
        Ok(value)
    }
    pub fn standard_module(&mut self, name: &str, source: &str) -> Result<Value> {
        let key = cache::standard_key(name);
        if let Some(value) = self.cache.get(&key) {
            let value = value.clone();
            self.note_import(None, &key);
            return Ok(value);
        }
        if let Some(record) = self.reusable(&key) {
            let value = self.replay(&key, &record)?;
            self.note_import(None, &key);
            return Ok(value);
        }
        if !self.active.insert(key.clone()) {
            bail!("circular standard module initialization: {name}");
        }
        let previous = std::mem::replace(&mut self.path, key.clone());
        let value = self.counted(&key, |e| e.source(source));
        self.path = previous;
        self.active.remove(&key);
        let value = value?;
        self.cache.insert(key.clone(), value.clone());
        self.keep(&key, Arc::from(source), &value);
        self.note_import(None, &key);
        Ok(value)
    }
    /// Evaluate one module's text as `path`, counting its own operations.
    fn counted<T>(&mut self, path: &Path, run: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if !path.starts_with("<std") {
            self.entry.get_or_insert_with(|| path.to_path_buf());
        }
        self.module_stack.push((path.to_path_buf(), self.steps, 0));
        if self.persist.is_some() {
            self.recordings.push(Recording {
                events: Vec::new(),
                seen: Default::default(),
                interned_from: self.provenance.journal_len(),
                memo_from: self.memo_journal.len(),
                bytes_from: self.provenance.bytes(),
                nested_bytes: 0,
                entry_depth: self.depth,
                outer_max_depth: self.max_depth,
                uncacheable: false,
            });
            self.max_depth = self.depth;
        }
        let result = run(self);
        let (path, start, nested) = self.module_stack.pop().expect("pushed above");
        let inclusive = self.steps - start;
        *self.module_steps.entry(path).or_default() += inclusive - nested;
        if let Some(parent) = self.module_stack.last_mut() {
            parent.2 += inclusive;
        }
        if let Some(recording) = self.recordings.pop() {
            let inclusive_bytes = self.provenance.bytes() - recording.bytes_from;
            if let Some(parent) = self.recordings.last_mut() {
                parent.nested_bytes += inclusive_bytes;
            }
            let depth = self.max_depth - recording.entry_depth;
            self.max_depth = self.max_depth.max(recording.outer_max_depth);
            self.finished = Some(Finished {
                events: recording.events,
                steps: inclusive - nested,
                inclusive_steps: inclusive,
                bytes: inclusive_bytes - recording.nested_bytes,
                inclusive_bytes,
                depth,
                interned: self.provenance.take_journal(recording.interned_from),
                memo: self.memo_journal.drain(recording.memo_from..).collect(),
                uncacheable: recording.uncacheable,
            });
        }
        result
    }
    /// Make an `--expr` expression the evaluation's entry point, so the module
    /// it inspects evaluates as it would when imported.
    pub fn enter_expression(&mut self, module_path: &Path) {
        self.entry = Some(expression_path(module_path));
    }
    /// Whether the module being evaluated is the evaluation's entry point.
    pub fn is_entry(&self) -> bool {
        self.module_stack.last().map(|m| &m.0) == self.entry.as_ref()
    }
    /// Operations each module has spent on its own top level so far (imports
    /// count for themselves), most first; a module still open counts what it
    /// has spent until now.
    pub fn module_spending(&self) -> Vec<(PathBuf, usize)> {
        // An open module's own share is its inclusive count minus finished
        // nested loads and the nested load still open inside it.
        let mut spent = self.module_steps.clone();
        let mut child_inclusive = 0;
        for (path, start, nested) in self.module_stack.iter().rev() {
            let inclusive = self.steps - start;
            *spent.entry(path.clone()).or_default() += inclusive - nested - child_inclusive;
            child_inclusive = inclusive;
        }
        let mut ranked: Vec<_> = spent.into_iter().filter(|(_, n)| *n > 0).collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        ranked
    }
    /// The budget error, naming the modules that spent the most operations.
    pub fn budget_error(&self) -> anyhow::Error {
        let ranked = self.module_spending();
        let name = module_label;
        let listed: Vec<String> = ranked
            .iter()
            .take(8)
            .map(|(p, n)| format!("{} {}", name(p), n))
            .collect();
        let mut diagnostic = Diagnostic::new(format!(
            "evaluation budget exceeded ({} operations)",
            self.context.evaluation_steps
        ))
        .help(super::builtins::BUDGET_HELP)
        .help(format!(
            "raise the budget with MUZ_EVAL_BUDGET (default {DEFAULT_EVALUATION_STEPS}, maximum {MAX_EVALUATION_STEPS}) when the host can afford it, for example `MUZ_EVAL_BUDGET=20000000 muz check song.muz`"
        ));
        if !listed.is_empty() {
            diagnostic = diagnostic.help(format!(
                "operations spent per module (all imports share one budget): {}; \
                 run a module's self-checks only when it is evaluated directly with `if is_entry() {{ ... }}`",
                listed.join(", ")
            ));
        }
        diagnostic.limit().err()
    }
    pub fn module(&mut self, requested: &Path) -> Result<Value> {
        let path = self.loader.resolve(requested).map_err(|error| {
            Diagnostic::new(format!("cannot read {}: {error}", requested.display())).err()
        })?;
        if let Some(v) = self.cache.get(&path) {
            let v = v.clone();
            self.note_import(Some(requested), &path);
            return Ok(v);
        }
        if !self.active.contains(&path)
            && let Some(record) = self.reusable(&path)
        {
            let value = self.replay(&path, &record)?;
            self.note_import(Some(requested), &path);
            return Ok(value);
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
        let result = self.counted(&path, |e| e.source(&source));
        self.path = previous;
        self.active.remove(&path);
        let value = result.map_err(|error| Diagnostic::named(error, &path))?;
        self.cache.insert(path.clone(), value.clone());
        self.keep(&path, Arc::from(source), &value);
        self.note_import(Some(requested), &path);
        Ok(value)
    }
    /// Evaluate only what one top-level binding of a module needs: the last
    /// `let`, `fn` or `use` that binds `name` and, transitively, the earlier
    /// top-level statements whose names it mentions. Other bindings and bare
    /// expressions are not evaluated, so their cost and errors don't count.
    /// The partial module is not cached. `None` when no top-level statement
    /// binds `name` (it may still be a field of the module's result).
    pub fn module_binding(&mut self, requested: &Path, name: &str) -> Result<Option<Value>> {
        let path = self.loader.resolve(requested).map_err(|error| {
            Diagnostic::new(format!("cannot read {}: {error}", requested.display())).err()
        })?;
        if let Some(module) = self.cache.get(&path) {
            return Ok(module.get(name).cloned());
        }
        let source = self.loader.read(&path).map_err(|error| {
            Diagnostic::new(format!("cannot read {}: {error}", path.display())).err()
        })?;
        let program = super::parse(&source).map_err(|error| syntax(&path, &source, error))?;
        let Some(selected) = binding_statements(&program, name) else {
            return Ok(None);
        };
        self.active.insert(path.clone());
        self.dependencies.push(path.clone());
        let previous = std::mem::replace(&mut self.path, path.clone());
        let context = self.context.clone();
        let result = self.counted(&path, |e| {
            context.run(|| {
                e.sources.insert(
                    e.path.clone(),
                    Arc::new(SourceFile::new(e.path.clone(), Arc::from(source.as_str()))),
                );
                let mut env = Env::new();
                e.program(&selected, &mut env)?;
                Ok(env.get(name).cloned())
            })
        });
        self.path = previous;
        self.active.remove(&path);
        result.map_err(|error| Diagnostic::named(error, &path))
    }
    /// Evaluate module text that has no file of its own (standard input). Relative
    /// `use` paths resolve against `path`'s directory.
    pub fn module_text(&mut self, path: &Path, text: &str) -> Result<Value> {
        let previous = std::mem::replace(&mut self.path, path.to_path_buf());
        let result = self.counted(path, |e| e.source(text));
        self.path = previous;
        result.map_err(|error| Diagnostic::named(error, path))
    }
    /// Evaluate `text` in the top-level scope of an evaluated `module`. Relative
    /// `use` paths resolve against `module_path`'s directory; diagnostics name the
    /// pseudo-file `<expr>` beside it, so spans point into the expression text.
    pub fn expression_in(
        &mut self,
        module: &Value,
        module_path: &Path,
        text: &str,
    ) -> Result<Value> {
        let path = expression_path(module_path);
        let mut env = match module {
            Value::Record(r) => r.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => Env::new(),
        };
        env.local.remove("__result");
        let context = self.context.clone();
        let previous = std::mem::replace(&mut self.path, path.clone());
        let result = context.run(|| -> Result<Value> {
            let program = super::parse(text).map_err(|error| syntax(&self.path, text, error))?;
            self.sources.insert(
                path.clone(),
                Arc::new(SourceFile::new(path.clone(), Arc::from(text))),
            );
            self.program(&program, &mut env)
        });
        self.path = previous;
        result
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
        Ok(Value::Record(Rc::new(Record::new(env.local))))
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
                            name: Some(n.clone()),
                            params: params.clone(),
                            body: body.clone(),
                            env: Rc::new(env.clone()),
                            path: self.path.clone(),
                        })),
                    );
                }
                Stmt::Import(path, alias, at, end) => {
                    // A module record is never callable, so an alias this
                    // program also calls as a function is a mistake; most often
                    // it hides a builtin such as keys().
                    if calls_name(p, alias) {
                        let builtin = super::builtins::names().contains(&alias.as_str())
                            || self.prelude_export(alias)?.is_some();
                        let mut diagnostic = Diagnostic::new(format!(
                            "import alias '{alias}' is also called as a function in this module"
                        ));
                        diagnostic = if builtin {
                            diagnostic.help(format!(
                                "the alias hides the builtin {alias}() for the whole module; choose another alias, such as `use {path:?} as {alias}_part`"
                            ))
                        } else {
                            diagnostic.help("a module is a record, not a function; choose another alias or call a function it exports, such as `{alias}.name(...)`".replace("{alias}", alias))
                        };
                        let error = diagnostic.err();
                        return Err(match self.sources.get(&self.path) {
                            Some(source) => Diagnostic::attach(
                                error,
                                Location::span(&self.path, &source.text, *at, *end),
                                false,
                            ),
                            None => error,
                        });
                    }
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
                // The standard library is small enough to list whole, however near
                // the misspelling is to one of its modules.
                let mut helps =
                    super::diagnostic::suggest_vocabulary("modules", path, known.iter().copied());
                helps.truncate(1);
                helps.retain(|help| help.starts_with("did you mean"));
                helps.push(format!("known modules: {}", known.join(", ")));
                return Err(Diagnostic::new(format!("unknown standard module '{path}'"))
                    .helps(helps)
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
                Err(self.budget_error())
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
        let mut names: Vec<String> = env.keys();
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
    /// Whether `x.name` may bind `x` as the first argument: only builtin and
    /// prelude functions act as methods.
    fn is_method(&mut self, name: &str) -> Result<bool> {
        Ok(super::builtins::names().contains(&name)
            || matches!(
                self.prelude_export(name)?,
                Some(Value::Function(_) | Value::Builtin(..))
            ))
    }
    fn method_names(&mut self) -> Vec<String> {
        let mut names: Vec<String> = super::builtins::names()
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        if let Ok(library) = self.standard_module("prelude", include_str!("../../std/prelude.muz"))
            && let Some(exports) = library
                .get("__result")
                .and_then(|value| value.record().ok())
        {
            names.extend(
                exports
                    .iter()
                    .filter(|(_, v)| matches!(v, Value::Function(_) | Value::Builtin(..)))
                    .map(|(k, _)| k.clone()),
            );
        }
        names.sort();
        names.dedup();
        names
    }
    fn unknown_method(&mut self, value: &Value, key: &str) -> anyhow::Error {
        let mut known = self.method_names();
        if let Value::Pattern(_) = value {
            known
                .extend(["span", "has_clock_timing", "notes", "controls", "raw"].map(String::from));
        } else if let Value::Array(_) = value {
            known.push("length".into());
        }
        Diagnostic::new(format!("{} has no field or method '{key}'", value.kind()))
            .helps(super::diagnostic::suggest_vocabulary(
                "known methods",
                key,
                known.iter().map(String::as_str),
            ))
            .err()
    }
    /// An identifier bound nowhere: not local, builtin, nor prelude.
    fn unknown_name(&mut self, name: &str, env: &Env, called: bool) -> anyhow::Error {
        if self.running.iter().any(|running| running == name) {
            return Diagnostic::new(format!(
                "function '{name}' cannot refer to itself: muz functions are not recursive"
            ))
            .help(
                "a function sees only names bound before its definition; \
                 iterate with map, fold or range instead",
            )
            .err();
        }
        if let Some(line) = self.sources.get(&self.path).and_then(|source| {
            source.text.lines().position(|line| {
                let line = line.strip_prefix("export ").unwrap_or(line);
                ["fn ", "let "].iter().any(|keyword| {
                    line.strip_prefix(keyword).is_some_and(|rest| {
                        rest.trim_start().strip_prefix(name).is_some_and(|after| {
                            !after.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                        })
                    })
                })
            })
        }) {
            let what = if called { "function" } else { "name" };
            return Diagnostic::new(format!(
                "{what} '{name}' is used before its definition at line {}",
                line + 1
            ))
            .help(
                "a name is visible only after its `fn` or `let`; \
                 move the definition above its first use",
            )
            .err();
        }
        let known = self.known_names(env);
        let (message, kind) = if called {
            (
                super::builtins::UnknownFunction(name.to_owned()).to_string(),
                "known functions",
            )
        } else {
            (format!("unknown name '{name}'"), "known names")
        };
        Diagnostic::new(message)
            .helps(super::diagnostic::suggest_vocabulary(
                kind,
                name,
                known.iter().map(String::as_str),
            ))
            .err()
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
                    return Err(self.unknown_name(s, env, false));
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
                    // `use "std" as s` binds the builtin namespace.
                    Value::Builtin(prefix, None) if prefix.is_empty() => {
                        if super::builtins::names().contains(&key.as_str()) {
                            Value::Builtin(key.clone(), None)
                        } else if let Some(value) = self.prelude_export(key)? {
                            value
                        } else {
                            let known = self.known_names(&Env::new());
                            return Err(Diagnostic::new(format!(
                                "standard library has no function '{key}'"
                            ))
                            .helps(super::diagnostic::suggest_vocabulary(
                                "known functions",
                                key,
                                known.iter().map(String::as_str),
                            ))
                            .err());
                        }
                    }
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
                    _ if self.is_method(key)? => Value::Builtin(key.clone(), Some(Box::new(x))),
                    _ => return Err(self.unknown_method(&x, key)),
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
                if let Expr::Ident(name) = &f.kind
                    && !env.contains_key(name)
                    && name != "null"
                    && !super::builtins::names().contains(&name.as_str())
                    && self.prelude_export(name)?.is_none()
                {
                    return Err(self.unknown_name(name, env, true));
                }
                let callee = f;
                let f = self.eval(f, env)?;
                if let Expr::Ident(name) = &callee.kind
                    && !matches!(f, Value::Function(_) | Value::Builtin(..))
                    && super::builtins::names().contains(&name.as_str())
                {
                    return Err(Diagnostic::new(format!(
                        "'{name}' is a {} here, not the builtin {name}(): a binding named '{name}' hides it",
                        f.kind()
                    ))
                    .help(format!(
                        "rename the `let`, parameter or `use ... as` binding called '{name}' in this scope"
                    ))
                    .err());
                }
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
                name: None,
                params: params.clone(),
                body: *body.clone(),
                env: Rc::new(env.clone()),
                path: self.path.clone(),
            })),
            Expr::If(cond, yes, no) => {
                if self.eval(cond, env)?.truth() {
                    self.eval(yes, env)?
                } else {
                    self.eval(no, env)?
                }
            }
            Expr::Block(p) => self.program(p, &mut Env::within(Rc::new(env.clone())))?,
        })
    }
    pub fn call(&mut self, f: Value, args: Vec<(Option<String>, Value)>) -> Result<Value> {
        self.depth += 1;
        self.max_depth = self.max_depth.max(self.depth);
        if self.depth > 128 {
            return Err(Diagnostic::new("call depth exceeds 128").limit().err());
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
                if let Some(name) = &f.name {
                    self.running.push(name.clone());
                }
                let value = (|| {
                    let mut env = Env::within(f.env.clone());
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
                if f.name.is_some() {
                    self.running.pop();
                }
                self.path = old;
                value
            }
            _ => Err(Diagnostic::new(format!("{} is not callable", f.kind()))
                .help("call a function, or remove the call parentheses")
                .err()),
        }
    }
}
/// Whether `program` calls `name(...)` where `name` is not rebound by a
/// function's or lambda's parameters.
/// The top-level statements one binding needs, in source order: the last
/// statement binding `name`, then for each kept statement the latest earlier
/// binding of every identifier it mentions (ignoring scopes, so a local name
/// may keep an unneeded binding but a needed one is never left out).
fn binding_statements(program: &Program, name: &str) -> Option<Program> {
    fn idents(n: &Node, out: &mut BTreeSet<String>) {
        match &n.kind {
            Expr::Ident(id) => {
                out.insert(id.clone());
            }
            Expr::Call(f, args) => {
                idents(f, out);
                args.iter().for_each(|(_, a)| idents(a, out));
            }
            Expr::Array(items) => items.iter().for_each(|i| idents(i, out)),
            Expr::Record(fields) => fields.iter().for_each(|(_, v)| idents(v, out)),
            Expr::Unary(_, a) | Expr::Get(a, _) => idents(a, out),
            Expr::Binary(_, a, b) | Expr::Index(a, b) => {
                idents(a, out);
                idents(b, out);
            }
            Expr::If(c, a, b) => {
                idents(c, out);
                idents(a, out);
                idents(b, out);
            }
            Expr::Lambda(params, body) => {
                params.iter().flat_map(|(_, d)| d).for_each(|d| idents(d, out));
                idents(body, out);
            }
            Expr::Block(program) => program.iter().for_each(|stmt| stmt_idents(stmt, out)),
            Expr::Number(_) | Expr::String(_) | Expr::Bool(_) => {}
        }
    }
    fn stmt_idents(stmt: &Stmt, out: &mut BTreeSet<String>) {
        match stmt {
            Stmt::Let(_, e) | Stmt::Expr(e) => idents(e, out),
            Stmt::Function(_, params, body) => {
                params.iter().flat_map(|(_, d)| d).for_each(|d| idents(d, out));
                idents(body, out);
            }
            Stmt::Import(..) => {}
        }
    }
    fn binds(stmt: &Stmt) -> Option<&str> {
        match stmt {
            Stmt::Let(n, _) | Stmt::Function(n, ..) | Stmt::Import(_, n, ..) => Some(n),
            Stmt::Expr(_) => None,
        }
    }
    let mut wanted = BTreeSet::from([name.to_owned()]);
    let mut keep = vec![false; program.len()];
    let mut found = false;
    for (i, stmt) in program.iter().enumerate().rev() {
        let Some(bound) = binds(stmt) else { continue };
        if !wanted.remove(bound) {
            continue;
        }
        found |= bound == name;
        keep[i] = true;
        stmt_idents(stmt, &mut wanted);
    }
    found.then(|| {
        program
            .iter()
            .zip(keep)
            .filter_map(|(stmt, keep)| keep.then(|| stmt.clone()))
            .collect()
    })
}
fn calls_name(program: &Program, name: &str) -> bool {
    fn node(n: &Node, name: &str) -> bool {
        match &n.kind {
            Expr::Call(f, args) => {
                matches!(&f.kind, Expr::Ident(called) if called == name)
                    || node(f, name)
                    || args.iter().any(|(_, a)| node(a, name))
            }
            Expr::Array(items) => items.iter().any(|i| node(i, name)),
            Expr::Record(fields) => fields.iter().any(|(_, v)| node(v, name)),
            Expr::Unary(_, a) | Expr::Get(a, _) => node(a, name),
            Expr::Binary(_, a, b) | Expr::Index(a, b) => node(a, name) || node(b, name),
            Expr::If(c, a, b) => node(c, name) || node(a, name) || node(b, name),
            Expr::Lambda(params, body) => {
                !params.iter().any(|(p, _)| p == name)
                    && (params.iter().any(|(_, d)| d.as_ref().is_some_and(|d| node(d, name)))
                        || node(body, name))
            }
            Expr::Block(program) => calls_name(program, name),
            Expr::Number(_) | Expr::String(_) | Expr::Bool(_) | Expr::Ident(_) => false,
        }
    }
    program.iter().any(|stmt| match stmt {
        Stmt::Let(_, e) | Stmt::Expr(e) => node(e, name),
        Stmt::Function(_, params, body) => {
            !params.iter().any(|(p, _)| p == name) && node(body, name)
        }
        Stmt::Import(..) => false,
    })
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

/// The path an `--expr` expression evaluates as, beside its module.
fn expression_path(module_path: &Path) -> PathBuf {
    module_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("<expr>")
}
