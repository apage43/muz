//! Index locations in already decoded JSON5. The JSON5 deserializer remains the
//! authority for syntax and values; this walk records fields, never validates
//! them or searches for the text of a failing value. Built only on failure.
use crate::lang::{Origin, SourceFile};
use std::{collections::BTreeMap, path::Path, sync::Arc};

pub(super) fn field(parent: &str, key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
    {
        if parent.is_empty() {
            key.into()
        } else {
            format!("{parent}.{key}")
        }
    } else {
        format!("{parent}[{}]", serde_json::to_string(key).unwrap())
    }
}

pub(super) fn origin(source: &str, file: &Path, target: &str) -> Option<Origin> {
    let mut index = Index {
        source,
        at: 0,
        spans: BTreeMap::new(),
    };
    index.value("")?;
    // A missing field points at its containing record. Aggregate checks point
    // at the root, rather than at whichever field was last visited.
    let span = index
        .spans
        .get(target)
        .or_else(|| {
            index
                .spans
                .iter()
                .filter(|(path, _)| {
                    target
                        .strip_prefix(path.as_str())
                        .is_some_and(|s| s.starts_with('.') || s.starts_with('['))
                })
                .max_by_key(|(path, _)| path.len())
                .map(|(_, span)| span)
        })
        .or_else(|| index.spans.get(""))?;
    Some(Origin::new(
        Arc::new(SourceFile::new(file.into(), source.into())),
        span.0,
        span.1,
    ))
}

struct Index<'a> {
    source: &'a str,
    at: usize,
    spans: BTreeMap<String, (usize, usize)>,
}
impl Index<'_> {
    fn peek(&self) -> Option<char> {
        self.source[self.at..].chars().next()
    }
    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        Some(c)
    }
    fn trivia(&mut self) -> Option<()> {
        loop {
            while self
                .peek()
                .is_some_and(|c| c.is_whitespace() || c == '\u{feff}')
            {
                self.next();
            }
            if self.source[self.at..].starts_with("//") {
                while self
                    .peek()
                    .is_some_and(|c| !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
                {
                    self.next();
                }
            } else if self.source[self.at..].starts_with("/*") {
                self.at += 2;
                self.at += self.source[self.at..].find("*/")? + 2;
            } else {
                return Some(());
            }
        }
    }
    fn string(&mut self) -> Option<()> {
        let quote = self.next()?;
        loop {
            match self.next()? {
                '\\' => {
                    self.next()?;
                }
                c if c == quote => return Some(()),
                _ => {}
            }
        }
    }
    fn value(&mut self, path: &str) -> Option<()> {
        self.trivia()?;
        let start = self.at;
        match self.peek()? {
            '{' => {
                self.next();
                self.trivia()?;
                while self.peek()? != '}' {
                    let key_start = self.at;
                    if matches!(self.peek()?, '\'' | '"') {
                        self.string()?;
                    } else {
                        while self
                            .peek()
                            .is_some_and(|c| c != ':' && c != '/' && !c.is_whitespace())
                        {
                            self.next();
                        }
                    }
                    let key_end = self.at;
                    self.trivia()?;
                    if self.next()? != ':' {
                        return None;
                    }
                    // Delegate quoted/escaped/Unicode identifier decoding too.
                    let key: BTreeMap<String, ()> =
                        json5::from_str(&format!("{{{}:null}}", &self.source[key_start..key_end]))
                            .ok()?;
                    let child = field(path, key.keys().next()?);
                    self.value(&child)?;
                    self.trivia()?;
                    if self.peek()? == ',' {
                        self.next();
                        self.trivia()?;
                    } else {
                        break;
                    }
                }
                if self.next()? != '}' {
                    return None;
                }
            }
            '[' => {
                self.next();
                self.trivia()?;
                let mut i = 0;
                while self.peek()? != ']' {
                    self.value(&format!("{path}[{i}]"))?;
                    i += 1;
                    self.trivia()?;
                    if self.peek()? == ',' {
                        self.next();
                        self.trivia()?;
                    } else {
                        break;
                    }
                }
                if self.next()? != ']' {
                    return None;
                }
            }
            '\'' | '"' => self.string()?,
            _ => {
                while self
                    .peek()
                    .is_some_and(|c| !matches!(c, ',' | '}' | ']' | '/') && !c.is_whitespace())
                {
                    self.next();
                }
                if self.at == start {
                    return None;
                }
            }
        }
        self.spans.insert(path.into(), (start, self.at));
        Some(())
    }
}
