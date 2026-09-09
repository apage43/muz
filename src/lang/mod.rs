//! A bounded expression language. Evaluation builds musical and production values off-thread.
mod builtins;
mod diagnostic;
mod eval;
mod parser;
use anyhow::Result;
pub use eval::{Evaluator, Number, Quantity, Unit, Value};
pub use parser::{Program, parse};
use std::path::Path;
pub fn load(path: &Path) -> Result<(Value, Vec<std::path::PathBuf>)> {
    let mut e = Evaluator::new();
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
