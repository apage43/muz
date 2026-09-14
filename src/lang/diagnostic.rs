use super::parser::Node;
pub use crate::diagnostic::*;
use std::path::{Path, PathBuf};
impl Location {
    pub fn new(path: PathBuf, source: &str, node: &Node) -> Self {
        Self::span(&path, source, node.at, node.end)
    }
}
pub fn syntax(path: &Path, source: &str, error: anyhow::Error) -> anyhow::Error {
    match error.downcast::<super::parser::SyntaxError>() {
        Ok(failure) => {
            let location = Location::span(path, source, failure.at, failure.end);
            let helps: Vec<String> = failure.helps().map(str::to_owned).collect();
            Diagnostic::at(location, failure.message).helps(helps).err()
        }
        Err(error) => Diagnostic::named(error, path),
    }
}
