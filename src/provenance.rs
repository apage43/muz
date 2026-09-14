//! Bounded, read-only score explanations. Origins never define note identity.
use crate::{
    compile::Compiled,
    diagnostic::{Location, Origin},
    music::Note,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug, Default)]
pub struct NoteProvenance {
    pub definition: Option<Origin>,
    pub latest_call: Option<Origin>,
}
impl PartialEq for NoteProvenance {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
#[derive(Debug, Serialize)]
pub struct NoteExplanation {
    pub track: String,
    pub key: String,
    pub definition: Option<Location>,
    pub latest_pattern_call: Option<Location>,
    pub location_quality: &'static str,
    pub identity_quality: &'static str,
}
#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub rows: Vec<T>,
    pub total: usize,
    pub next: Option<usize>,
}
pub(crate) fn bounded_page<T: Serialize>(
    mut rows: Vec<T>,
    total: usize,
    offset: usize,
) -> anyhow::Result<Page<T>> {
    let mut bytes = 0;
    let mut keep = 0;
    for row in &rows {
        let size = serde_json::to_vec(row)?.len();
        anyhow::ensure!(size <= 1024 * 1024, "one inspection row exceeds 1 MiB");
        if bytes + size > 1024 * 1024 {
            break;
        }
        bytes += size;
        keep += 1;
    }
    rows.truncate(keep);
    let end = offset.saturating_add(rows.len());
    Ok(Page {
        rows,
        total,
        next: (end < total).then_some(end),
    })
}
impl Compiled {
    pub fn explain_notes(
        &self,
        track: &str,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Page<NoteExplanation>> {
        let t = self
            .score
            .iter()
            .find(|t| t.id == track)
            .ok_or_else(|| anyhow::anyhow!("unknown score track {track}"))?;
        let rows=t.pattern.notes.iter().skip(offset).take(limit.min(1000)).map(|n|NoteExplanation{
            track:track.into(),key:n.key.clone(),definition:n.provenance.definition.as_ref().map(Origin::location),latest_pattern_call:n.provenance.latest_call.as_ref().map(Origin::location),
            location_quality:if n.provenance.definition.is_some(){"definition and last pattern-returning call; occurrence span unavailable"}else{"unavailable; compile with provenance enabled"},
            identity_quality:"authored key path; positional components may change after insertion",
        }).collect::<Vec<_>>();
        bounded_page(rows, t.pattern.notes.len(), offset)
    }
}
#[derive(Debug, Serialize)]
pub struct NoteChange {
    pub track: String,
    pub key: String,
    pub change: &'static str,
}
#[derive(Debug, Serialize)]
pub struct SemanticDiff {
    pub notes: Page<NoteChange>,
    pub audio_effect: crate::audio::transaction::PreparationEffect,
}
pub fn diff(
    old: &Compiled,
    new: &Compiled,
    offset: usize,
    limit: usize,
) -> anyhow::Result<SemanticDiff> {
    fn index(c: &Compiled) -> BTreeMap<(&str, &str), Vec<&Note>> {
        let mut map: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for t in &c.score {
            for n in &t.pattern.notes {
                map.entry((t.id.as_str(), n.key.as_str()))
                    .or_default()
                    .push(n);
            }
        }
        map
    }
    let a = index(old);
    let b = index(new);
    let keys: BTreeSet<_> = a.keys().chain(b.keys()).copied().collect();
    let mut total = 0;
    let mut rows = Vec::new();
    for (track, key) in keys {
        let left = a.get(&(track, key));
        let right = b.get(&(track, key));
        let change = match (left, right) {
            (Some(a), Some(b)) if a.len() != 1 || b.len() != 1 => "ambiguous duplicate key",
            (Some(a), Some(b)) if a[0] == b[0] => continue,
            (Some(_), Some(_)) => "changed",
            (Some(a), None) if a.len() > 1 => "ambiguous removed duplicate key",
            (None, Some(b)) if b.len() > 1 => "ambiguous added duplicate key",
            (Some(_), None) => "removed",
            _ => "added",
        };
        if total >= offset && rows.len() < limit.min(1000) {
            rows.push(NoteChange {
                track: track.into(),
                key: key.into(),
                change,
            });
        }
        total += 1;
    }
    let plan = crate::plan_reconciliation(0, &old.session, &new.session)?;
    Ok(SemanticDiff {
        notes: bounded_page(rows, total, offset)?,
        audio_effect: crate::audio::transaction::preparation_effect(
            &old.session,
            &new.session,
            &plan,
        ),
    })
}
