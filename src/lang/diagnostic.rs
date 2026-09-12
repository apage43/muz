//! Source locations and diagnostics, shared by parsing, evaluation and lowering.
//!
//! Diagnostics group the best-effort source location we can still name, the
//! `help:` lines that list mechanical ways to fix the source, and (for nested
//! evaluation) the distinct caller lines of the innermost failure.
use super::parser::Node;
use std::{fmt, path::{Path, PathBuf}, sync::Arc};

/// One parsed source text, shared by every value and diagnostic taken from it.
/// Shared across threads because diagnostics travel inside `anyhow::Error`.
pub struct SourceFile {
    pub path: PathBuf,
    pub text: Arc<str>,
}
impl SourceFile {
    pub fn new(path: PathBuf, text: Arc<str>) -> Self {
        Self { path, text }
    }
}
impl fmt::Debug for SourceFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.path.display())
    }
}

/// The span that produced a value, kept past evaluation so later stages
/// (lowering, graph preparation) can still say where the value came from.
#[derive(Clone)]
pub struct Origin {
    file: Arc<SourceFile>,
    at: u32,
    end: u32,
}
impl Origin {
    pub fn new(file: Arc<SourceFile>, at: usize, end: usize) -> Self {
        Self {
            file,
            at: at.min(u32::MAX as usize) as u32,
            end: end.min(u32::MAX as usize) as u32,
        }
    }
    pub fn location(&self) -> Location {
        Location::span(&self.file.path, &self.file.text, self.at as usize, self.end as usize)
    }
    pub fn path(&self) -> &Path {
        &self.file.path
    }
}
impl PartialEq for Origin {
    fn eq(&self, other: &Self) -> bool {
        self.file.path == other.file.path && self.at == other.at && self.end == other.end
    }
}
impl fmt::Debug for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}..{}", self.file.path.display(), self.at, self.end)
    }
}

#[derive(Debug)]
pub struct Location {
    path: PathBuf,
    line: usize,
    column: usize,
    text: String,
    indent: usize,
    width: usize,
}
impl Location {
    pub fn new(path: PathBuf, source: &str, node: &Node) -> Self {
        Self::span(&path, source, node.at, node.end)
    }
    pub fn span(path: &Path, source: &str, at: usize, end: usize) -> Self {
        let at = at.min(source.len());
        // JSON5 also accepts lone CR and Unicode line separators. Count CRLF
        // once, and use character offsets so Unicode columns remain correct.
        let mut start = 0;
        let mut line = 1;
        let mut chars = source[..at].char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                start = i + c.len_utf8();
                if c == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') {
                    start = chars.next().unwrap().0 + 1;
                }
                line += 1;
            }
        }
        let line_end = source[at..]
            .find(['\n', '\r', '\u{2028}', '\u{2029}'])
            .map_or(source.len(), |i| at + i);
        let prefix = &source[start..at];
        let span_end = end.min(line_end).max(at);
        Self {
            path: path.to_owned(),
            line,
            column: prefix.chars().count() + 1,
            text: source[start..line_end]
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

/// A user-facing failure: what went wrong, the best-effort place it happened,
/// and the mechanical ways the source could be fixed.
#[derive(Debug)]
pub struct Diagnostic {
    message: String,
    primary: Option<Location>,
    origin: Option<Origin>,
    path: Option<PathBuf>,
    callers: Vec<Location>,
    helps: Vec<String>,
}
impl Diagnostic {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            primary: None,
            origin: None,
            path: None,
            callers: vec![],
            helps: vec![],
        }
    }
    pub fn at(location: Location, message: impl Into<String>) -> Self {
        Self {
            primary: Some(location),
            ..Self::new(message)
        }
    }
    /// Name the value's origin as a fallback, for stages that no longer see the
    /// span of the failing expression itself.
    pub fn origin(mut self, origin: Option<&Origin>) -> Self {
        if self.origin.is_none() {
            self.origin = origin.cloned();
        }
        self
    }
    /// Fall back to naming only the file when no span for it survives.
    pub fn path(mut self, path: &Path) -> Self {
        if self.path.is_none() {
            self.path = Some(path.to_owned());
        }
        self
    }
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.helps.push(help.into());
        self
    }
    pub fn helps(mut self, helps: impl IntoIterator<Item = String>) -> Self {
        self.helps.extend(helps);
        self
    }
    pub fn err(self) -> anyhow::Error {
        self.into()
    }
    pub(crate) fn attach(error: anyhow::Error, location: Location, call: bool) -> anyhow::Error {
        match error.downcast::<Self>() {
            Ok(mut diagnostic) => {
                if diagnostic.primary.is_none() {
                    diagnostic.primary = Some(location);
                } else if call
                    && !diagnostic
                        .primary
                        .as_ref()
                        .is_some_and(|p| p.same_line(&location))
                    && !diagnostic.callers.iter().any(|c| c.same_line(&location))
                {
                    diagnostic.callers.push(location);
                }
                diagnostic.into()
            }
            Err(error) => Self {
                message: format!("{error:#}"),
                primary: Some(location),
                ..Self::new(String::new())
            }
            .into(),
        }
    }
    /// Best-effort attribution for a plain error when a value's origin is known.
    pub fn locate(error: anyhow::Error, origin: Option<&Origin>) -> anyhow::Error {
        match error.downcast::<Self>() {
            Ok(diagnostic) => diagnostic.origin(origin).into(),
            Err(error) => Self::new(format!("{error:#}"))
                .origin(origin)
                .into(),
        }
    }
    /// Best-effort file attribution for a stage that no longer sees spans.
    pub fn named(error: anyhow::Error, path: &Path) -> anyhow::Error {
        match error.downcast::<Self>() {
            Ok(mut diagnostic) => {
                if diagnostic.primary.is_none() {
                    diagnostic.path.get_or_insert_with(|| path.to_owned());
                }
                diagnostic.into()
            }
            Err(error) => Self {
                message: format!("{error:#}"),
                path: Some(path.to_owned()),
                ..Self::new(String::new())
            }
            .into(),
        }
    }
}
impl std::error::Error for Diagnostic {}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fallback;
        let place = match (&self.primary, &self.origin) {
            (Some(location), _) => Some(location),
            (None, Some(origin)) => {
                fallback = origin.location();
                Some(&fallback)
            }
            (None, None) => None,
        };
        match place {
            Some(p) => {
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
            }
            None => match &self.path {
                Some(path) => write!(f, "{}: {}", path.display(), self.message)?,
                None => write!(f, "{}", self.message)?,
            },
        }
        for help in &self.helps {
            write!(f, "\n  = help: {help}")?;
        }
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
                let path = place
                    .and_then(|p| p.path.parent())
                    .and_then(|root| c.path.strip_prefix(root).ok())
                    .unwrap_or(&c.path);
                write!(f, "\n  {}:{}:{}", path.display(), c.line, c.column)?;
            }
        }
        Ok(())
    }
}

/// Closest candidate to a misspelled name, within a length-scaled edit distance.
pub fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    ranked(name, candidates).first().map(|(_, name)| *name)
}

/// Help lines for a name that did not resolve: the nearest known name, then the
/// candidate vocabulary that would let the source compile. Long vocabularies
/// list only the nearest names so the hint stays readable.
pub fn suggest_vocabulary<'a>(
    kind: &str,
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let candidates: Vec<&str> = candidates.into_iter().collect();
    let ranked = ranked(name, candidates.iter().copied());
    let mut out = vec![];
    if let Some((_, closest)) = ranked.first() {
        out.push(format!("did you mean '{closest}'?"));
    }
    if candidates.len() <= 16 {
        out.push(format!("{kind}: {}", candidates.join(", ")));
    } else if !ranked.is_empty() {
        let near: Vec<&str> = ranked.iter().take(8).map(|(_, name)| *name).collect();
        out.push(format!("similar {kind}: {}", near.join(", ")));
    }
    out
}

/// Candidates ordered by edit distance, restricted to plausible misspellings.
fn ranked<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Vec<(usize, &'a str)> {
    let typed = name.to_ascii_lowercase();
    let limit = match typed.chars().count() {
        0..=4 => 1,
        5..=8 => 2,
        _ => 3,
    };
    let mut ranked: Vec<(usize, &str)> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let known = candidate.to_ascii_lowercase();
            let distance = edit_distance(&typed, &known);
            (distance <= limit && known != typed).then_some((distance, candidate))
        })
        .collect();
    ranked.sort_by(|(ad, a), (bd, b)| (ad, a.len(), a).cmp(&(bd, b.len(), b)));
    ranked
}

/// Render a structured parse failure against the text it came from.
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

/// Case-insensitive Levenshtein distance over characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ac) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let diagonal = previous;
            previous = row[j + 1];
            row[j + 1] = if ac == bc {
                diagonal
            } else {
                1 + diagonal.min(row[j]).min(row[j + 1])
            };
        }
    }
    row[b.len()]
}
