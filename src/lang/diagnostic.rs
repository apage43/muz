//! Evaluation diagnostics retain the innermost span and only distinct caller lines.
use super::parser::Node;
use std::{fmt, path::PathBuf};

#[derive(Debug)]
pub(super) struct Location {
    path: PathBuf,
    line: usize,
    column: usize,
    text: String,
    indent: usize,
    width: usize,
}

impl Location {
    pub fn new(path: PathBuf, source: &str, node: &Node) -> Self {
        let at = node.at.min(source.len());
        let start = source[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = source[at..].find('\n').map_or(source.len(), |i| at + i);
        let prefix = &source[start..at];
        let span_end = node.end.min(end).max(at);
        Self {
            path,
            line: source[..start].bytes().filter(|b| *b == b'\n').count() + 1,
            column: prefix.chars().count() + 1,
            text: source[start..end]
                .trim_end_matches('\r')
                .replace('\t', "    "),
            indent: prefix.replace('\t', "    ").chars().count(),
            width: source[at..span_end]
                .replace('\t', "    ")
                .chars()
                .count()
                .max(1),
        }
    }

    fn same_line(&self, other: &Self) -> bool {
        self.path == other.path && self.line == other.line
    }
}

#[derive(Debug)]
pub(super) struct EvaluationDiagnostic {
    message: String,
    primary: Location,
    callers: Vec<Location>,
}

impl EvaluationDiagnostic {
    pub fn attach(error: anyhow::Error, location: Location, call: bool) -> anyhow::Error {
        match error.downcast::<Self>() {
            Ok(mut diagnostic) => {
                if call
                    && !diagnostic.primary.same_line(&location)
                    && !diagnostic.callers.iter().any(|c| c.same_line(&location))
                {
                    diagnostic.callers.push(location);
                }
                diagnostic.into()
            }
            Err(error) => Self {
                message: format!("{error:#}"),
                primary: location,
                callers: Vec::new(),
            }
            .into(),
        }
    }
}

impl std::error::Error for EvaluationDiagnostic {}

impl fmt::Display for EvaluationDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = &self.primary;
        writeln!(
            f,
            "{}:{}:{}: {}",
            p.path.display(),
            p.line,
            p.column,
            self.message
        )?;
        let gutter = p.line.to_string().len();
        writeln!(f, "{:gutter$} |", "")?;
        writeln!(f, "{} | {}", p.line, p.text)?;
        write!(
            f,
            "{:gutter$} | {}{}",
            "",
            " ".repeat(p.indent),
            "^".repeat(p.width)
        )?;
        if !self.callers.is_empty() {
            write!(f, "\ncallers (inner first):")?;
            let mut omitted = 0;
            for (i, c) in self.callers.iter().enumerate() {
                // Keep both ends of long traces and every module transition.
                if i >= 4 && i + 4 < self.callers.len() && c.path == self.callers[i - 1].path {
                    omitted += 1;
                    continue;
                }
                if omitted > 0 {
                    write!(f, "\n  ... {omitted} caller lines omitted")?;
                    omitted = 0;
                }
                let path = p
                    .path
                    .parent()
                    .and_then(|root| c.path.strip_prefix(root).ok())
                    .unwrap_or(&c.path);
                write!(f, "\n  {}:{}:{}", path.display(), c.line, c.column)?;
            }
        }
        Ok(())
    }
}
