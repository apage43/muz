//! A bounded expression language. Evaluation builds musical and production values off-thread.
mod builtins;
mod cache;
mod diagnostic;
mod eval;
mod loader;
pub use cache::ModuleCache;
pub use eval::STANDARD_MODULES;
pub(crate) use eval::contrib_root;
pub use loader::{FileSourceLoader, SourceLoader};
mod parser;
use anyhow::Result;
pub use builtins::names as function_names;
pub use builtins::note_value;
pub use diagnostic::{
    Diagnostic, Location, Origin, SourceFile, closest, suggest_vocabulary, syntax,
};
pub use eval::{
    DEFAULT_EVALUATION_STEPS, Evaluator, MAX_EVALUATION_STEPS, Number, Quantity, Record, Unit,
    Value, evaluation_budget, module_label,
};
pub use parser::{Program, SyntaxError, parse};
use std::path::Path;
pub fn load(path: &Path) -> Result<(Value, Vec<std::path::PathBuf>)> {
    load_with_loader(path, std::rc::Rc::new(FileSourceLoader))
}
pub fn load_with_loader(
    path: &Path,
    loader: std::rc::Rc<dyn SourceLoader>,
) -> Result<(Value, Vec<std::path::PathBuf>)> {
    load_evaluated(path, Evaluator::with_loader(loader))
}

/// Evaluate a module and, when `expression` is given, that expression in the
/// module's top-level scope. `text` supplies the module source instead of reading
/// `path` (used for standard input); `path` still anchors relative `use` paths.
pub fn evaluate(path: &Path, text: Option<&str>, expression: Option<&str>) -> Result<Value> {
    let mut e = Evaluator::new();
    if expression.is_some() {
        e.enter_expression(path);
    }
    let module = match text {
        Some(text) => e.module_text(path, text)?,
        None => e.module(path)?,
    };
    match expression {
        Some(expression) => e.expression_in(&module, path, expression),
        None => Ok(module_result(module)),
    }
}

/// Evaluate `path` as `load_with_loader` does, reusing the modules `cache`
/// kept from earlier evaluations whose inputs are unchanged and keeping the
/// modules this evaluation loads. The result is the one a fresh evaluation
/// returns.
pub fn load_with_cache(
    path: &Path,
    loader: std::rc::Rc<dyn SourceLoader>,
    cache: &std::rc::Rc<ModuleCache>,
) -> Result<(Value, Vec<std::path::PathBuf>)> {
    let context = crate::host::current().unwrap_or_else(crate::host::HostContext::cli);
    let e = Evaluator::with_cache(loader, context, cache.clone());
    e.adopt_cache(path);
    load_evaluated(path, e)
}
fn module_result(value: Value) -> Value {
    let result = match &value {
        Value::Record(r) => r
            .get("main")
            .cloned()
            .or_else(|| r.get("__result").cloned()),
        _ => None,
    };
    result.unwrap_or(value)
}

/// What a finished evaluation spent: its operations against its budget, and
/// each module's own share (its imports count for themselves), most first.
#[derive(Clone, Debug, serde::Serialize)]
pub struct EvaluationReport {
    pub steps: usize,
    pub budget: usize,
    pub modules: Vec<(std::path::PathBuf, usize)>,
}
impl EvaluationReport {
    pub fn of(e: &Evaluator) -> Self {
        Self {
            steps: e.steps,
            budget: e.context.evaluation_steps,
            modules: e.module_spending(),
        }
    }
}
thread_local! {
    static LAST_EVALUATION: std::cell::RefCell<Option<EvaluationReport>> =
        const { std::cell::RefCell::new(None) };
}
/// The report of the last module this thread loaded as a root (with `load`,
/// `load_with_loader` or `compile`), for `muz check` to show what it cost.
pub fn last_evaluation() -> Option<EvaluationReport> {
    LAST_EVALUATION.with(|r| r.borrow().clone())
}

fn load_evaluated(path: &Path, mut e: Evaluator) -> Result<(Value, Vec<std::path::PathBuf>)> {
    let value = e.module(path);
    LAST_EVALUATION.with(|r| *r.borrow_mut() = Some(EvaluationReport::of(&e)));
    Ok((module_result(value?), e.dependencies))
}

/// Evaluated root identity, deliberately independent of compilation/readiness.
#[derive(Debug)]
pub enum RootClassification {
    Song { dependencies: Vec<std::path::PathBuf> },
    NonSong { description: String, dependencies: Vec<std::path::PathBuf> },
    Unresolved { error: anyhow::Error },
}

/// Evaluate with the supplied authority and budgets, without lowering or preparation.
/// Cancellation and budget exhaustion abort the operation (outer `Err`); ordinary
/// source/dependency failures retain their original diagnostic in `Unresolved`.
pub fn classify_root_with_context(
    path: &Path,
    loader: std::rc::Rc<dyn SourceLoader>,
    context: &crate::host::HostContext,
) -> Result<RootClassification> {
    context.run(|| {
        crate::host::check_cancelled()?;
        let loaded = load_evaluated(path, Evaluator::with_context(loader, context.clone()));
        match loaded {
            Ok((value, dependencies)) => {
                crate::host::check_cancelled()?;
                if matches!(&value, Value::Record(record)
                    if matches!(record.get("type"), Some(Value::Str(tag)) if tag == "song"))
                {
                    Ok(RootClassification::Song { dependencies })
                } else {
                    let description = match &value {
                        Value::Record(record) => match record.get("type") {
                            Some(Value::Str(tag)) => format!("record with type {tag:?}"),
                            Some(tag) => format!("record with {} type field", tag.kind()),
                            None => "record without a type field".to_owned(),
                        },
                        _ => value.kind().to_owned(),
                    };
                    Ok(RootClassification::NonSong { description, dependencies })
                }
            }
            Err(error) if context.is_cancelled() || error.chain().any(|cause|
                cause.downcast_ref::<Diagnostic>().is_some_and(Diagnostic::is_limit)) => Err(error),
            Err(error) => Ok(RootClassification::Unresolved { error }),
        }
    })
}
mod controls;
pub mod format;
