//! A bounded expression language. Evaluation builds musical and production values off-thread.
mod builtins;
mod diagnostic;
mod eval;
mod loader;
pub use loader::{FileSourceLoader, SourceLoader};
pub use eval::STANDARD_MODULES;
mod parser;
use anyhow::Result;
pub use diagnostic::{Diagnostic, Location, Origin, SourceFile, closest, suggest_vocabulary, syntax};
pub use eval::{Evaluator, Number, Quantity, Record, Unit, Value};
pub use builtins::names as function_names;
pub use parser::{Program, SyntaxError, parse};
use std::path::Path;
pub fn load(path: &Path) -> Result<(Value, Vec<std::path::PathBuf>)> {
    load_with_loader(path, std::rc::Rc::new(FileSourceLoader))
}
pub fn load_with_loader(path: &Path, loader: std::rc::Rc<dyn SourceLoader>) -> Result<(Value, Vec<std::path::PathBuf>)> {
    let mut e = Evaluator::with_loader(loader);
    let value = e.module(path)?;
    let result = match value {
        Value::Record(ref r) => r
            .get("main")
            .cloned()
            .or_else(|| r.get("__result").cloned())
            .unwrap_or(value.clone()),
        _ => value,
    };
    Ok((result, e.dependencies))
}
mod controls;
pub mod format;
