//! A bounded expression language. Evaluation builds musical and production values off-thread.
mod builtins;
mod diagnostic;
mod eval;
mod loader;
pub use eval::STANDARD_MODULES;
pub(crate) use eval::contrib_root;
pub use loader::{FileSourceLoader, SourceLoader};
mod parser;
use anyhow::Result;
pub use builtins::names as function_names;
pub use diagnostic::{
    Diagnostic, Location, Origin, SourceFile, closest, suggest_vocabulary, syntax,
};
pub use eval::{Evaluator, Number, Quantity, Record, Unit, Value};
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

fn load_evaluated(path: &Path, mut e: Evaluator) -> Result<(Value, Vec<std::path::PathBuf>)> {
    let value = e.module(path)?;
    let result = match &value {
        Value::Record(r) => r
            .get("main")
            .cloned()
            .or_else(|| r.get("__result").cloned()),
        _ => None,
    };
    Ok((result.unwrap_or(value), e.dependencies))
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
