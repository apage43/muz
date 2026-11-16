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
        .map_err(validation_error)
}

/// `parse_project` under the active host context, reusing the unchanged
/// modules `modules` kept from earlier compilations.
pub fn parse_project_with_cache(
    path: impl AsRef<Path>,
    modules: &std::rc::Rc<crate::lang::ModuleCache>,
) -> Result<Session, SourceError> {
    let context = crate::host::current().unwrap_or_else(crate::host::HostContext::cli);
    crate::compile::compile_with_cache(
        path.as_ref(),
        std::rc::Rc::new(crate::lang::FileSourceLoader),
        &context,
        modules,
    )
    .map(|c| c.session)
    .map_err(validation_error)
}

impl From<anyhow::Error> for SourceError {
    fn from(e: anyhow::Error) -> Self {
        validation_error(e)
    }
}

fn validation_error(e: anyhow::Error) -> SourceError {
    SourceError::Validation(Box::new(match e.downcast::<Diagnostic>() {
        Ok(diagnostic) => diagnostic,
        Err(e) => Diagnostic::new(format!("{e:#}")),
    }))
}
