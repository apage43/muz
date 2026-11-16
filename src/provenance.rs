//! Bounded, read-only score explanations. Origins never define note identity.
use crate::{
    compile::Compiled,
    diagnostic::{Location, Origin},
    music::{Note, RawEvent},
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
/// Lineage and occurrence numbers only tell notes and contexts apart inside
/// tracing. They are unique in the process, so values traced by one evaluation
/// can be reused by another (see `lang::ModuleCache`) without colliding.
static NEXT_NUMBER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
fn next_number() -> usize {
    NEXT_NUMBER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
/// One interned entry, as a journal records it.
#[derive(Clone)]
pub(crate) enum Interned {
    Context(ContextKey, Arc<Occurrence>),
    Edit(EditKey, Arc<BTreeMap<&'static str, Origin>>),
}
#[derive(Default)]
pub struct Arena {
    contexts: BTreeMap<ContextKey, Arc<Occurrence>>,
    edits: BTreeMap<EditKey, Arc<BTreeMap<&'static str, Origin>>>,
    bytes: usize,
    /// Entries interned since journaling started, when it has.
    journal: Option<Vec<Interned>>,
}
impl Arena {
    fn charge(&mut self, size: usize, budget: usize) -> anyhow::Result<()> {
        if self.bytes.saturating_add(size) > budget {
            return Err(
                crate::diagnostic::Diagnostic::new("provenance arena byte limit exceeded")
                    .limit()
                    .err(),
            );
        }
        self.bytes += size;
        Ok(())
    }
    /// Bytes charged so far.
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
    /// Record every entry interned from now on.
    pub(crate) fn start_journal(&mut self) {
        self.journal.get_or_insert_with(Vec::new);
    }
    pub(crate) fn journal_len(&self) -> usize {
        self.journal.as_ref().map_or(0, Vec::len)
    }
    /// Remove and return the entries interned since the journal had `from`.
    pub(crate) fn take_journal(&mut self, from: usize) -> Vec<Interned> {
        self.journal
            .as_mut()
            .map_or_else(Vec::new, |journal| journal.drain(from..).collect())
    }
    /// Intern entries another evaluation recorded and charge their bytes, as
    /// if they had been traced here.
    pub(crate) fn restore(
        &mut self,
        entries: &[Interned],
        bytes: usize,
        budget: usize,
    ) -> anyhow::Result<()> {
        self.charge(bytes, budget)?;
        for entry in entries {
            match entry {
                Interned::Context(key, value) => {
                    self.contexts
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
                Interned::Edit(key, value) => {
                    self.edits
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
            }
        }
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
        if let Some(journal) = &mut self.journal {
            journal.push(Interned::Edit(key.clone(), value.clone()));
        }
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
        if depth > 128 {
            return Err(crate::diagnostic::Diagnostic::new(
                "provenance occurrence depth exceeds 128",
            )
            .limit()
            .err());
        }
        // Conservative accounting includes both interning keys and retained
        // nodes, B-tree/Arc overhead and variable-length strings.
        let size = 1024usize
            .saturating_add(key.len() * 2)
            .saturating_add(operation.len() * 2)
            .saturating_add(origin.path().as_os_str().len() * 2);
        self.charge(size, budget)?;
        let value = Arc::new(Occurrence {
            id: next_number(),
            depth,
            origin: origin.clone(),
            operation: operation.into(),
            key: key.into(),
            parent,
        });
        if let Some(journal) = &mut self.journal {
            journal.push(Interned::Context(identity.clone(), value.clone()));
        }
        self.contexts.insert(identity, value.clone());
        Ok(value)
    }
    /// Called only for primitive pattern operations, never for returning wrappers.
    /// Notes and raw events are traced alike: each keeps the call that made it,
    /// the placements it passed through and the calls that changed its fields.
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
            notes: &mut BTreeMap<(usize, usize), Vec<&'a Note>>,
            raw: &mut BTreeMap<(usize, usize), Vec<&'a RawEvent>>,
        ) {
            match v {
                crate::lang::Value::Pattern(p) => {
                    for n in &p.notes {
                        notes.entry(n.provenance.identity()).or_default().push(n);
                    }
                    for r in &p.raw {
                        raw.entry(r.provenance.identity()).or_default().push(r);
                    }
                }
                crate::lang::Value::Array(a) => {
                    for v in a.iter() {
                        collect(v, notes, raw);
                    }
                }
                _ => {}
            }
        }
        let mut notes = BTreeMap::new();
        let mut raw = BTreeMap::new();
        for value in inputs {
            collect(value, &mut notes, &mut raw);
        }
        for note in &mut pattern.notes {
            self.trace_one(note, &notes, origin, operation, budget)?;
        }
        for event in &mut pattern.raw {
            self.trace_one(event, &raw, origin, operation, budget)?;
        }
        Ok(())
    }
    fn trace_one<T: Traced>(
        &mut self,
        item: &mut T,
        before: &BTreeMap<(usize, usize), Vec<&T>>,
        origin: &Origin,
        operation: &str,
        budget: usize,
    ) -> anyhow::Result<()> {
        crate::host::check_cancelled()?;
        if item.provenance().definition.is_none() {
            let asset = item
                .provenance()
                .asset
                .as_ref()
                .map_or(0, |a| 512 + a.path.len() * 2);
            self.charge(256 + asset, budget)?;
            let occurrence = self.occurrence(origin, operation, "", None, budget)?;
            let provenance = item.provenance_mut();
            provenance.lineage = next_number();
            provenance.definition = Some(origin.clone());
            provenance.latest_call = Some(origin.clone());
            provenance.occurrence = Some(occurrence);
            return Ok(());
        }
        let Some(old) = before
            .get(&item.provenance().identity())
            .and_then(|candidates| {
                candidates
                    .iter()
                    .find(|old| old.same(item))
                    .copied()
                    .or_else(|| (candidates.len() == 1).then(|| candidates[0]))
            })
        else {
            return Ok(());
        };
        let fields = item.changed_fields(old);
        if !fields.is_empty() {
            let edits = self.edits(&item.provenance().edits, fields, origin, budget)?;
            let provenance = item.provenance_mut();
            provenance.latest_call = Some(origin.clone());
            provenance.edits = edits;
        }
        if old.key() != item.key()
            || matches!(
                operation,
                "at" | "place" | "repeat" | "seq" | "stack" | "fit"
            )
        {
            let occurrence = self.occurrence(
                origin,
                operation,
                item.key(),
                old.provenance().occurrence.clone(),
                budget,
            )?;
            item.provenance_mut().occurrence = Some(occurrence);
        }
        Ok(())
    }
}
/// What tracing needs from a traced score value: a note or a raw event.
trait Traced {
    fn provenance(&self) -> &NoteProvenance;
    fn provenance_mut(&mut self) -> &mut NoteProvenance;
    /// The authored key path; raw events have none.
    fn key(&self) -> &str;
    /// Whether `old` is the input this value came from, among inputs with its lineage.
    fn same(&self, old: &Self) -> bool;
    fn changed_fields(&self, old: &Self) -> Vec<&'static str>;
}
impl Traced for Note {
    fn provenance(&self) -> &NoteProvenance {
        &self.provenance
    }
    fn provenance_mut(&mut self) -> &mut NoteProvenance {
        &mut self.provenance
    }
    fn key(&self) -> &str {
        &self.key
    }
    fn same(&self, old: &Self) -> bool {
        old.key == self.key
    }
    fn changed_fields(&self, old: &Self) -> Vec<&'static str> {
        changed_fields(old, self)
    }
}
impl Traced for RawEvent {
    fn provenance(&self) -> &NoteProvenance {
        &self.provenance
    }
    fn provenance_mut(&mut self) -> &mut NoteProvenance {
        &mut self.provenance
    }
    fn key(&self) -> &str {
        ""
    }
    fn same(&self, old: &Self) -> bool {
        old.bytes == self.bytes
    }
    fn changed_fields(&self, old: &Self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if old.at != self.at {
            fields.push("at");
        }
        if old.offset_ms != self.offset_ms {
            fields.push("offset_ms");
        }
        if old.bytes != self.bytes {
            fields.push("bytes");
        }
        fields
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
        // Score notes, then the notes raw note-ons make, keyed as the
        // compiled MIDI keys them: `raw<N>` for the Nth raw event.
        let raw_notes = t
            .pattern
            .raw
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_note_on());
        let total = t.pattern.notes.len() + raw_notes.clone().count();
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        for (provenance, key) in t
            .pattern
            .notes
            .iter()
            .map(|n| (&n.provenance, Ok(&n.key)))
            .chain(raw_notes.map(|(i, r)| (&r.provenance, Err(i))))
            .skip(offset)
        {
            crate::host::check_cancelled()?;
            if rows.len() >= limit {
                break;
            }
            let row = match key {
                Ok(key) => provenance.explain(track, key.clone(), false),
                Err(index) => provenance.explain(track, format!("raw{index}"), true),
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
            total,
            next: (end < total).then_some(end),
        })
    }
}
impl NoteProvenance {
    /// Tells a traced value apart from others while tracing.
    fn identity(&self) -> (usize, usize) {
        (self.lineage, self.occurrence.as_ref().map_or(0, |c| c.id))
    }
    fn explain(&self, track: &str, key: String, raw: bool) -> NoteExplanation {
        let mut occurrences = Vec::new();
        let mut current = self.occurrence.as_deref();
        while let Some(c) = current {
            occurrences.push(OccurrenceExplanation {
                operation: c.operation.clone(),
                key: c.key.clone(),
                location: c.origin.location(),
            });
            current = c.parent.as_deref();
        }
        NoteExplanation {
            track: track.into(),
            key,
            definition: self.definition.as_ref().map(Origin::location),
            latest_pattern_call: self.latest_call.as_ref().map(Origin::location),
            latest_edit: self.latest_call.as_ref().map(Origin::location),
            occurrences,
            field_edits: self.edits.iter().map(|(k, v)| (*k, v.location())).collect(),
            asset: self.asset.as_deref().cloned(),
            location_quality: if self.asset.is_some() {
                "import call fallback; asset track and event order"
            } else if self.definition.is_none() {
                "unavailable; compile with provenance enabled"
            } else if raw {
                "note_on call, occurrence and relevant primitive edit spans of the raw note-on"
            } else {
                "definition, occurrence and relevant primitive edit spans"
            },
            identity_quality: if raw {
                "raw event order in the track; changes when earlier raw events are added or removed"
            } else {
                "authored key path; positional components may change after insertion"
            },
        }
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
                let fields = changed_fields(left[0], right[0])
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
