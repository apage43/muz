//! Compatibility entry points kept at their historical import paths.

use std::path::Path;

use thiserror::Error;

use crate::{lang::Diagnostic, model::Session};

pub use crate::description::{ParameterSpec, parameter_specs};

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("invalid session: {0}")]
    Validation(Box<Diagnostic>),
}

/// Compiles a `.muz` project into a session, resolving every asset relative to
/// the project file.
pub fn parse_project(path: impl AsRef<Path>) -> Result<Session, SourceError> {
    crate::compile::compile(path.as_ref())
        .map(|c| c.session)
        .map_err(|e| {
            SourceError::Validation(Box::new(match e.downcast::<Diagnostic>() {
                Ok(diagnostic) => diagnostic,
                Err(e) => Diagnostic::new(format!("{e:#}")),
            }))
        })
}
