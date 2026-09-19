//! Bounded, read-only score explanations. Origins never define note identity.
use crate::{
    compile::Compiled,
    diagnostic::{Location, Origin},
    music::Note,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
#[derive(Clone, Debug, Serialize)]
pub struct AssetOrigin {
    pub path: String,
    pub version: (u64, u128),
    pub track: usize,
    pub order: usize,
}
#[derive(Debug)]
pub struct Occurrence {
    id: usize,
    depth: usize,
    pub origin: Origin,
    pub operation: String,
    pub key: String,
    pub parent: Option<Arc<Occurrence>>,
}
type ContextKey = ((std::path::PathBuf, u32, u32), String, String, usize);
type EditKey = Vec<(&'static str, (std::path::PathBuf, u32, u32))>;
#[derive(Default)]
pub struct Arena {
    contexts: BTreeMap<ContextKey, Arc<Occurrence>>,
    edits: BTreeMap<EditKey, Arc<BTreeMap<&'static str, Origin>>>,
    bytes: usize,
    next_note: usize,
}
impl Arena {
    fn charge(&mut self, size: usize, budget: usize) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.bytes.saturating_add(size) <= budget,
            "provenance arena byte limit exceeded"
        );
        self.bytes += size;
        Ok(())
    }
    fn edits(
        &mut self,
        previous: &BTreeMap<&'static str, Origin>,
        fields: Vec<&'static str>,
        origin: &Origin,
        budget: usize,
    ) -> anyhow::Result<Arc<BTreeMap<&'static str, Origin>>> {
        // There are at most fourteen note fields. Intern complete edit maps so
        // a broad transform shares one map across identically attributed notes.
        let mut edits = previous.clone();
        for field in fields {
            edits.insert(field, origin.clone());
        }
        let key: EditKey = edits.iter().map(|(k, v)| (*k, v.identity())).collect();
        if let Some(value) = self.edits.get(&key) {
            return Ok(value.clone());
        }
        let size = 1024
            + key
                .iter()
                .map(|(_, (p, _, _))| 512 + p.as_os_str().len() * 2)
                .sum::<usize>();
        self.charge(size, budget)?;
        let value = Arc::new(edits);
        self.edits.insert(key, value.clone());
        Ok(value)
    }
    fn occurrence(
        &mut self,
        origin: &Origin,
        operation: &str,
        key: &str,
        parent: Option<Arc<Occurrence>>,
        budget: usize,
    ) -> anyhow::Result<Arc<Occurrence>> {
        let identity = (
            origin.identity(),
            operation.to_owned(),
            key.to_owned(),
            parent.as_ref().map_or(0, |p| p.id),
        );
        if let Some(value) = self.contexts.get(&identity) {
            return Ok(value.clone());
        }
        let depth = parent.as_ref().map_or(1, |p| p.depth + 1);
        anyhow::ensure!(depth <= 128, "provenance occurrence depth exceeds 128");
        // Conservative accounting includes both interning keys and retained
        // nodes, B-tree/Arc overhead and variable-length strings.
        let size = 1024usize
            .saturating_add(key.len() * 2)
            .saturating_add(operation.len() * 2)
            .saturating_add(origin.path().as_os_str().len() * 2);
        self.charge(size, budget)?;
        let value = Arc::new(Occurrence {
            id: self.contexts.len() + 1,
            depth,
            origin: origin.clone(),
            operation: operation.into(),
            key: key.into(),
            parent,
        });
        self.contexts.insert(identity, value.clone());
        Ok(value)
    }
    /// Called only for primitive pattern operations, never for returning wrappers.
    pub fn trace(
        &mut self,
        pattern: &mut crate::music::Pattern,
        inputs: &[crate::lang::Value],
        origin: &Origin,
        operation: &str,
        budget: usize,
    ) -> anyhow::Result<()> {
        fn collect<'a>(
            v: &'a crate::lang::Value,
            out: &mut BTreeMap<(usize, usize), Vec<&'a Note>>,
        ) {
            match v {
                crate::lang::Value::Pattern(p) => {
                    for n in &p.notes {
                        out.entry((
                            n.provenance.lineage,
                            n.provenance.occurrence.as_ref().map_or(0, |c| c.id),
                        ))
                        .or_default()
                        .push(n);
                    }
                }
                crate::lang::Value::Array(a) => {
                    for v in a.iter() {
                        collect(v, out);
                    }
                }
                _ => {}
            }
        }
        let mut before = BTreeMap::new();
        for value in inputs {
            collect(value, &mut before);
        }
        for note in &mut pattern.notes {
            crate::host::check_cancelled()?;
            if note.provenance.definition.is_none() {
                self.charge(
                    256 + note
                        .provenance
                        .asset
                        .as_ref()
                        .map_or(0, |a| 512 + a.path.len() * 2),
                    budget,
                )?;
                self.next_note += 1;
                note.provenance.lineage = self.next_note;
                note.provenance.definition = Some(origin.clone());
                note.provenance.latest_call = Some(origin.clone());
                note.provenance.occurrence =
                    Some(self.occurrence(origin, operation, "", None, budget)?);
                continue;
            }
            let candidates = before.get(&(
                note.provenance.lineage,
                note.provenance.occurrence.as_ref().map_or(0, |c| c.id),
            ));
            let old = candidates.and_then(|ns| {
                ns.iter()
                    .find(|n| n.key == note.key)
                    .copied()
                    .or_else(|| (ns.len() == 1).then(|| ns[0]))
            });
            if let Some(old) = old {
                let fields = changed_fields(old, note);
                if !fields.is_empty() {
                    note.provenance.latest_call = Some(origin.clone());
                    note.provenance.edits =
                        self.edits(&note.provenance.edits, fields, origin, budget)?;
                }
                if old.key != note.key
                    || matches!(
                        operation,
                        "at" | "place" | "repeat" | "seq" | "stack" | "fit"
                    )
                {
                    note.provenance.occurrence = Some(self.occurrence(
                        origin,
                        operation,
                        &note.key,
                        old.provenance.occurrence.clone(),
                        budget,
                    )?);
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default)]
pub struct NoteProvenance {
    pub definition: Option<Origin>,
    pub latest_call: Option<Origin>,
    pub occurrence: Option<Arc<Occurrence>>,
    pub edits: Arc<BTreeMap<&'static str, Origin>>,
    pub asset: Option<Arc<AssetOrigin>>,
    lineage: usize,
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
    pub latest_edit: Option<Location>,
    pub occurrences: Vec<OccurrenceExplanation>,
    pub field_edits: BTreeMap<&'static str, Location>,
    pub asset: Option<AssetOrigin>,
    pub location_quality: &'static str,
    pub identity_quality: &'static str,
}
#[derive(Debug, Serialize)]
pub struct OccurrenceExplanation {
    pub operation: String,
    pub key: String,
    pub location: Location,
}
pub fn changed_fields(a: &Note, b: &Note) -> Vec<&'static str> {
    let mut fields = Vec::new();
    macro_rules! field { ($($f:ident),*) => { $(if a.$f != b.$f { fields.push(stringify!($f)); })* }; }
    field!(
        at,
        dur,
        pitch,
        velocity,
        release,
        gate,
        key,
        tags,
        data,
        voice,
        hand,
        offset_ms,
        release_offset_ms,
        clock
    );
    fields
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
        let size = crate::inspect::bounded_json(row)?.len();
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
        anyhow::ensure!(limit > 0, "page limit must be positive");
        anyhow::ensure!(limit <= 1000, "page limit exceeds 1000 rows");
        let t = self
            .score
            .iter()
            .find(|t| t.id == track)
            .ok_or_else(|| anyhow::anyhow!("unknown score track {track}"))?;
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        for n in t.pattern.notes.iter().skip(offset) {
            crate::host::check_cancelled()?;
            if rows.len() >= limit {
                break;
            }
            let mut occurrences = Vec::new();
            let mut current = n.provenance.occurrence.as_deref();
            while let Some(c) = current {
                occurrences.push(OccurrenceExplanation {
                    operation: c.operation.clone(),
                    key: c.key.clone(),
                    location: c.origin.location(),
                });
                current = c.parent.as_deref();
            }
            let row = NoteExplanation {
                track: track.into(),
                key: n.key.clone(),
                definition: n.provenance.definition.as_ref().map(Origin::location),
                latest_pattern_call: n.provenance.latest_call.as_ref().map(Origin::location),
                latest_edit: n.provenance.latest_call.as_ref().map(Origin::location),
                occurrences,
                field_edits: n
                    .provenance
                    .edits
                    .iter()
                    .map(|(k, v)| (*k, v.location()))
                    .collect(),
                asset: n.provenance.asset.as_deref().cloned(),
                location_quality: if n.provenance.asset.is_some() {
                    "import call fallback; asset track and event order"
                } else if n.provenance.definition.is_some() {
                    "definition, occurrence and relevant primitive edit spans"
                } else {
                    "unavailable; compile with provenance enabled"
                },
                identity_quality: "authored key path; positional components may change after insertion",
            };
            let size = crate::inspect::bounded_json(&row)?.len();
            anyhow::ensure!(size <= 1024 * 1024, "one inspection row exceeds 1 MiB");
            if bytes.saturating_add(size) > 1024 * 1024 {
                break;
            }
            bytes += size;
            rows.push(row);
        }
        let end = offset.saturating_add(rows.len());
        Ok(Page {
            rows,
            total: t.pattern.notes.len(),
            next: (end < t.pattern.notes.len()).then_some(end),
        })
    }
}
#[derive(Debug, Serialize)]
pub struct NoteChange {
    pub track: String,
    pub key: String,
    pub change: &'static str,
    pub fields: Vec<&'static str>,
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
    anyhow::ensure!(limit > 0, "page limit must be positive");
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
                fields: match (left, right) {
                    (Some(a), Some(b)) if a.len() == 1 && b.len() == 1 => {
                        changed_fields(a[0], b[0])
                    }
                    _ => vec![],
                },
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

/// A category is requested independently, keeping the whole response bounded.
#[allow(unused_assignments)] // Macro state is consumed by multi-row branches.
pub fn diff_page(
    old: &Compiled,
    new: &Compiled,
    category: &str,
    offset: usize,
    limit: usize,
) -> anyhow::Result<Page<serde_json::Value>> {
    anyhow::ensure!(limit > 0, "page limit must be positive");
    anyhow::ensure!(limit <= 1000, "page limit exceeds 1000 rows");
    let mut total = 0;
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    let mut full = false;
    // Construct and serialize only rows selected for this page. Once the byte
    // budget omits a row, continuation must resume at that row rather than
    // skipping forward to a later smaller one.
    macro_rules! push {
        ($row:expr $(,)?) => {{
            crate::host::check_cancelled()?;
            let selected = total >= offset && rows.len() < limit && !full;
            total += 1;
            if selected {
                let row = $row;
                let size = crate::inspect::bounded_json(&row)?.len();
                anyhow::ensure!(size <= 1024 * 1024, "one inspection row exceeds 1 MiB");
                if bytes.saturating_add(size) <= 1024 * 1024 {
                    bytes += size;
                    rows.push(row);
                } else {
                    full = true;
                }
            }
        }};
    }
    match category {
        "notes" | "metadata" | "occurrences" => {
            fn index(c: &Compiled) -> anyhow::Result<BTreeMap<(&str, &str), Vec<&Note>>> {
                let mut map = BTreeMap::<(&str, &str), Vec<&Note>>::new();
                for t in &c.score {
                    for n in &t.pattern.notes {
                        crate::host::check_cancelled()?;
                        map.entry((&t.id, &n.key)).or_default().push(n);
                    }
                }
                Ok(map)
            }
            let a = index(old)?;
            let b = index(new)?;
            for key in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
                let left = a.get(key);
                let right = b.get(key);
                let (Some(left), Some(right)) = (left, right) else {
                    if category == "occurrences" {
                        push!(
                            serde_json::json!({"track":key.0,"key":key.1,"change":if left.is_none(){"added"}else{"removed"}}),
                        );
                    }
                    continue;
                };
                if left.len() != 1 || right.len() != 1 {
                    if category == "occurrences" {
                        push!(
                            serde_json::json!({"track":key.0,"key":key.1,"change":"ambiguous duplicate key"}),
                        );
                    }
                    continue;
                }
                let fields = changed_fields(&left[0], &right[0])
                    .into_iter()
                    .filter(|f| match category {
                        "metadata" => matches!(*f, "tags" | "data" | "hand"),
                        "notes" => !matches!(*f, "tags" | "data" | "hand" | "key"),
                        _ => *f == "key",
                    })
                    .collect::<Vec<_>>();
                if !fields.is_empty() {
                    push!(
                        serde_json::json!({"track":key.0,"key":key.1,"change":"changed","fields":fields}),
                    );
                }
            }
            if category == "metadata" {
                let a = &old.session.extras;
                let b = &new.session.extras;
                macro_rules! extra {($field:ident)=>{if a.$field!=b.$field{push!(serde_json::json!({"entity":"session","field":stringify!($field),"change":"changed"}));}}}
                extra!(source);
                extra!(title);
                extra!(dependencies);
                extra!(sections);
                extra!(automation);
                extra!(tail);
                extra!(track_groups);
            }
        }
        "controllers" | "messages" | "tempos" | "performed" => {
            let a: BTreeMap<_, _> = old.session.tracks.iter().map(|t| (&t.id, t)).collect();
            let b: BTreeMap<_, _> = new.session.tracks.iter().map(|t| (&t.id, t)).collect();
            for id in a.keys().chain(b.keys()).copied().collect::<BTreeSet<_>>() {
                fn midi<'a>(
                    t: Option<&&'a crate::model::Track>,
                ) -> Option<&'a crate::midi::ImportedMidi> {
                    match t.map(|t| &t.source) {
                        Some(crate::model::TrackSource::Midi(m)) => Some(&m.imported),
                        _ => None,
                    }
                }
                let left = midi(a.get(id));
                let right = midi(b.get(id));
                macro_rules! stream { ($field:ident) => {{
                    let l=left.as_ref().map(|m|m.$field.as_slice()).unwrap_or(&[]);
                    let r=right.as_ref().map(|m|m.$field.as_slice()).unwrap_or(&[]);
                    for i in 0..l.len().max(r.len()) { crate::host::check_cancelled()?; if l.get(i)!=r.get(i) { push!(serde_json::json!({"track":id,"order":i,"change":match (l.get(i),r.get(i)){(None,_)=>"added",(_,None)=>"removed",_=>"changed"},"before":l.get(i),"after":r.get(i),"identity_quality":"ordered stream; no inferred event identity"})); } }
                }}; }
                match category {
                    "controllers" => stream!(controllers),
                    "messages" => stream!(messages),
                    "tempos" => stream!(tempos),
                    _ => stream!(notes),
                }
            }
        }
        "consequences" => {
            let plan = crate::plan_reconciliation(0, &old.session, &new.session)?;
            use crate::reconcile::ReconcileOperation as O;
            for op in &plan.operations {
                push!(match op {
                    O::SetParameters { device_id, deltas } => {
                        serde_json::json!({"kind":"controls","id":device_id,"fields":deltas.keys().collect::<Vec<_>>()})
                    }
                    O::ReplaceTrackSource { track_id, .. } => {
                        serde_json::json!({"kind":"schedule","id":track_id})
                    }
                    O::ReplacePattern { pattern } => {
                        serde_json::json!({"kind":"schedule","id":pattern.id})
                    }
                    O::UpdateTransport { .. } => {
                        serde_json::json!({"kind":"schedule","id":"transport"})
                    }
                    O::UpdateRoute { route } => serde_json::json!({"kind":"route","id":route.id}),
                    O::Add { entity } | O::Remove { entity } | O::Replace { entity } => {
                        serde_json::json!({"kind":"entity","id":entity.entity.id(),"entity_type":entity.entity.entity_type(),"action":match op {O::Add{..}=>"added",O::Remove{..}=>"removed",_=>"replaced"}})
                    }
                    O::Rename { entity, .. }
                    | O::Move { entity, .. }
                    | O::Reparent { entity, .. } => {
                        serde_json::json!({"kind":"entity","id":entity.id,"action":match op {O::Rename{..}=>"renamed",O::Move{..}=>"moved",_=>"reparented"}})
                    }
                    O::UpdateExtras => {
                        let a = &old.session.extras;
                        let b = &new.session.extras;
                        let mut fields = Vec::new();
                        if a.source != b.source {
                            fields.push("source")
                        }
                        if a.title != b.title {
                            fields.push("title")
                        }
                        if a.dependencies != b.dependencies {
                            fields.push("dependencies")
                        }
                        if a.sections != b.sections {
                            fields.push("sections")
                        }
                        if a.automation != b.automation {
                            fields.push("automation")
                        }
                        if a.tail != b.tail {
                            fields.push("tail")
                        }
                        if a.track_groups != b.track_groups {
                            fields.push("track_groups")
                        }
                        serde_json::json!({"kind":"extras","fields":fields,"automation":a.automation!=b.automation,"tail":a.tail!=b.tail})
                    }
                });
            }
            crate::audio::transaction::try_visit_processor_consequences(
                &old.session,
                &new.session,
                |id, effect, reason| -> anyhow::Result<()> {
                    push!(
                        serde_json::json!({"device_id":id.as_str(),"effect":effect,"reason":reason})
                    );
                    Ok(())
                },
            )?;
        }
        _ => anyhow::bail!("unknown diff category {category}"),
    }
    let end = offset.saturating_add(rows.len());
    Ok(Page {
        rows,
        total,
        next: (end < total).then_some(end),
    })
}
