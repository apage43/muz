use super::diagnostic::Diagnostic;
use super::eval::{Evaluator, Number, Record, Unit, Value};
use crate::music::{self, Beat, Control, Note, Pattern, b, checked_time, rational, real};
use anyhow::{Result, bail};
use num_traits::{CheckedAdd, CheckedDiv, CheckedMul, CheckedSub};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
#[derive(Debug)]
pub struct UnknownFunction(pub String);
impl std::fmt::Display for UnknownFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown function '{}'; see `muz docs language`", self.0)
    }
}
impl std::error::Error for UnknownFunction {}
/// Every name dispatched by [`call`], listed for typo suggestions. Kept honest
/// by `tests/diagnostics.rs`, which calls each name and requires the dispatch to
/// recognize it.
pub fn names() -> &'static [&'static str] {
    &[
        "keys",
        "group_by",
        "overlay",
        "keyed_noise",
        "map_notes",
        "flat_map_notes",
        "filter_notes",
        "map_controls",
        "flat_map_controls",
        "map_raw",
        "flat_map_raw",
        "control",
        "midi",
        "midi_tempos",
        "clip",
        "notes_only",
        "phrase",
        "note",
        "rest",
        "seq",
        "stack",
        "repeat",
        "at",
        "place",
        "slice",
        "stretch",
        "fit",
        "express",
        "allocate_hands",
        "transpose",
        "gate",
        "velocity",
        "gain",
        "hand",
        "voice",
        "reverse",
        "invert",
        "tag",
        "annotate",
        "select",
        "reject",
        "refine",
        "chord",
        "pitch",
        "chords",
        "voicelead_solve",
        "reharmonize_solve",
        "reharmonizations_solve",
        "diatonic_transpose",
        "split",
        "drum_grid",
        "cc",
        "seconds_at",
        "sort_by",
        "range",
        "map",
        "filter",
        "fold",
        "len",
        "contains",
        "str",
        "format",
        "merge",
        "min",
        "max",
        "pow",
        "sin",
        "cos",
        "abs",
        "floor",
        "round",
        "song",
        "section",
        "piano",
        "fx",
        "plugin",
        "sample",
        "voice_patch",
        "rack",
        "bus",
        "curve",
        "curve_value",
        "unit",
        "automation",
        "channel",
        "note_on",
        "note_off",
        "program",
        "bank",
        "bend",
        "pressure",
        "poly_pressure",
        "sysex",
        "meta",
        "opaque",
        "raw_midi",
        "assert",
    ]
}
struct Args {
    values: Vec<(Option<String>, Value)>,
    position: usize,
}
impl Args {
    fn new(v: Vec<(Option<String>, Value)>) -> Self {
        Self {
            values: v,
            position: 0,
        }
    }
    fn take(&mut self, name: &str) -> Option<Value> {
        if let Some(i) = self
            .values
            .iter()
            .position(|(k, _)| k.as_deref() == Some(name))
        {
            return Some(self.values.remove(i).1);
        }
        if self.position < self.values.len() && self.values[self.position].0.is_none() {
            Some(self.values.remove(self.position).1)
        } else {
            None
        }
    }
    fn req(&mut self, n: &str) -> Result<Value> {
        self.take(n)
            .ok_or_else(|| anyhow::anyhow!("missing argument '{n}'"))
    }
    fn num(&mut self, n: &str, d: f64) -> Result<f64> {
        self.take(n).map(|v| v.scalar()).unwrap_or(Ok(d))
    }
    fn txt(&mut self, n: &str, d: &str) -> Result<String> {
        self.take(n)
            .map(|v| v.text().map(str::to_owned))
            .unwrap_or(Ok(d.into()))
    }
    fn beat(&mut self, n: &str, d: Beat) -> Result<Beat> {
        self.take(n).map(|v| v.beats()).unwrap_or(Ok(d))
    }
    fn done(self) -> Result<()> {
        if !self.values.is_empty() {
            bail!(
                "unexpected arguments: {}",
                self.values
                    .iter()
                    .map(|(k, _)| k.clone().unwrap_or("(positional)".into()))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(())
    }
}
fn pat(p: Pattern) -> Value {
    Value::Pattern(Arc::new(p))
}
fn preflight_patterns(
    values: &[Value],
    prefix: usize,
    limits: crate::limits::ExpansionLimits,
) -> Result<()> {
    let mut cost = crate::limits::ExpansionCost::default();
    for value in values {
        cost = cost
            .plus(crate::limits::ExpansionCost::of(value.pattern()?)?.repeated(1, prefix)?)?
            .check(limits)?;
    }
    Ok(())
}
fn record(values: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    rec(values
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect::<BTreeMap<_, _>>())
}
/// Build a record value. The evaluator stamps builtin results with the call
/// span, so construction here stays origin-free.
fn rec(fields: impl Into<Record>) -> Value {
    Value::Record(Arc::new(fields.into()))
}
pub fn note_value(n: &Note) -> Value {
    record([
        ("pitch", Value::num(n.pitch)),
        ("at", Value::beat(n.at)),
        ("duration", Value::beat(n.dur)),
        ("velocity", Value::num(n.velocity)),
        ("gate", Value::num(n.gate)),
        ("release", Value::num(n.release)),
        (
            "hand",
            n.hand
                .as_ref()
                .map_or(Value::Null, |h| Value::Str(h.clone())),
        ),
        ("offset", seconds_value(n.offset_ms / 1000.0)),
        (
            "release_offset",
            seconds_value(n.release_offset_ms / 1000.0),
        ),
        ("voice", Value::Str(n.voice.clone())),
        ("key", Value::Str(n.key.clone())),
        (
            "tags",
            Value::Array(n.tags.iter().cloned().map(Value::Str).collect()),
        ),
        (
            "data",
            rec(n
                .data
                .iter()
                .map(|(k, v)| (k.clone(), json_value(v)))
                .collect::<BTreeMap<_, _>>()),
        ),
    ])
}
fn seconds_value(seconds: f64) -> Value {
    // Renderer time and performed offsets are already floating-point values.
    // Re-rationalizing them invents large exact denominators that can overflow
    // when composers combine time and offsets in dimensional arithmetic.
    match Number::finite(seconds) {
        Ok(value) => Value::Num(super::eval::Quantity {
            value,
            unit: Unit::Seconds,
        }),
        Err(e) => Value::Invalid(e.to_string()),
    }
}
fn json_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => Value::num(n.as_f64().unwrap()),
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Array(a) => Value::Array(a.iter().map(json_value).collect()),
        serde_json::Value::Object(o) => rec(o
            .iter()
            .map(|(k, v)| (k.clone(), json_value(v)))
            .collect::<BTreeMap<_, _>>()),
    }
}
fn amount_ms(v: Value) -> Result<f64> {
    match v {
        Value::Num(n) if matches!(n.unit, Unit::Seconds | Unit::Scalar) => {
            Ok(n.number() * if n.unit == Unit::Seconds { 1000.0 } else { 1.0 })
        }
        _ => bail!("expected milliseconds"),
    }
}
fn clock_seconds(v: Value) -> Result<f64> {
    match v {
        Value::Num(n) if matches!(n.unit, Unit::Seconds | Unit::Scalar) => Ok(n.number()),
        _ => bail!("clock positions require seconds (for example 1.5s)"),
    }
}
fn selector(n: &Note, i: usize, len: usize, v: &Value) -> Result<bool> {
    match v {
        Value::Str(s) => Ok(match s.as_str() {
            "all" => true,
            "first" => i == 0,
            "last" => i + 1 == len,
            _ => {
                if let Some(s) = s.strip_prefix("tag:") {
                    n.tags.contains(s)
                } else if let Some(s) = s.strip_prefix("voice:") {
                    n.voice == s
                } else {
                    n.tags.contains(s) || n.voice == *s
                }
            }
        }),
        Value::Array(a) => {
            for v in a.iter() {
                if selector(n, i, len, v)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Value::Record(r) => {
            let tag = r.get("tag").map(|v| v.text()).transpose()?;
            let voice = r.get("voice").map(|v| v.text()).transpose()?;
            let pitch = r.get("pitch").map(|v| v.scalar()).transpose()?;
            Ok(tag.is_none_or(|t| n.tags.contains(t))
                && voice.is_none_or(|v| n.voice == v)
                && pitch.is_none_or(|p| p == n.pitch))
        }
        _ => bail!("selector needs a tag, list (union), field record (intersection), or predicate"),
    }
}
fn select(e: &mut Evaluator, n: &Note, i: usize, len: usize, v: &Value) -> Result<bool> {
    if matches!(v, Value::Function(_)) {
        Ok(e.call(v.clone(), vec![(None, note_value(n))])?.truth())
    } else {
        selector(n, i, len, v)
    }
}
pub fn control_value(c: &Control) -> Value {
    record([
        ("at", Value::beat(c.at)),
        ("offset", seconds_value(c.offset_ms / 1000.0)),
        ("controller", Value::integer(c.cc as i64)),
        ("value", Value::integer(c.value as i64)),
    ])
}
pub fn raw_value(r: &music::RawEvent) -> Value {
    record([
        ("at", Value::beat(r.at)),
        ("offset", seconds_value(r.offset_ms / 1000.0)),
        (
            "bytes",
            Value::Array(r.bytes.iter().map(|v| Value::integer(*v as i64)).collect()),
        ),
    ])
}
/// Note fields `refine` and note patches accept, listed for typo hints.
const NOTE_REFINEMENTS: &[&str] = &[
    "at",
    "duration",
    "pitch",
    "velocity",
    "release",
    "gate",
    "offset",
    "release_offset",
    "offset_ms",
    "release_offset_ms",
    "hand",
    "voice",
    "key",
    "tags",
    "data",
];
fn byte(v: &Value, max: u8) -> Result<u8> {
    let x = v.scalar()?;
    if !x.is_finite() || x.fract() != 0.0 || !(0.0..=max as f64).contains(&x) {
        bail!("event byte must be an integer 0..{max}");
    }
    Ok(x as u8)
}
fn record_patch(k: &str, v: &Value) -> Value {
    rec(BTreeMap::from([(k.to_owned(), v.clone())]))
}
fn patch_note(n: &mut Note, patch: &Value) -> Result<()> {
    if n.clock.is_some() && patch.record()?.contains_key("duration") {
        bail!("clock clip duration must be changed through clip trim options");
    }
    for (k, v) in patch.record()?.iter() {
        match k.as_str() {
            "at" => n.at = v.beats()?,
            "duration" => n.dur = v.beats()?,
            "pitch" => n.pitch = v.scalar()?,
            "velocity" => n.velocity = v.scalar()?,
            "release" => n.release = v.scalar()?,
            "gate" => n.gate = v.scalar()?,
            "offset" => n.offset_ms = amount_ms(v.clone())?,
            "release_offset" => n.release_offset_ms = amount_ms(v.clone())?,
            "offset_ms" => n.offset_ms = v.field_number("offset_ms")?,
            "release_offset_ms" => n.release_offset_ms = v.field_number("release_offset_ms")?,
            "hand" => {
                n.hand = if matches!(v, Value::Null) {
                    None
                } else {
                    Some(v.text()?.into())
                }
            }
            "voice" => n.voice = v.text()?.into(),
            "key" => n.key = v.text()?.into(),
            "tags" => {
                n.tags = v
                    .array()?
                    .iter()
                    .map(|v| Ok(v.text()?.to_owned()))
                    .collect::<Result<_>>()?
            }
            "data" => {
                n.data = v
                    .record()?
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), annotation_value(k, v)?)))
                    .collect::<Result<_>>()?
            }
            _ => {
                return Err(Diagnostic::new(format!("unknown note refinement '{k}'"))
                    .helps(super::diagnostic::suggest_vocabulary(
                        "refinements",
                        k,
                        NOTE_REFINEMENTS.iter().copied(),
                    ))
                    .err());
            }
        }
    }
    Ok(())
}
// Grouping and random keys preserve data types and dimensions. JSON output for
// inspection intentionally omits units, so it cannot serve as a key encoding.
fn data_key(value: &Value) -> Result<serde_json::Value> {
    Ok(match value {
        Value::Null => serde_json::json!(["null"]),
        Value::Bool(v) => serde_json::json!(["bool", v]),
        Value::Str(v) => serde_json::json!(["text", v]),
        Value::Num(q) => {
            let normalized = if q.unit == Unit::Bar {
                q.beats(4.0)?
            } else {
                q.value.exact()?
            };
            let unit = if q.unit == Unit::Bar {
                Unit::Beat
            } else {
                q.unit
            };
            serde_json::json!([
                "number",
                format!("{unit:?}"),
                normalized.numer(),
                normalized.denom()
            ])
        }
        Value::Array(values) => serde_json::json!([
            "list",
            values.iter().map(data_key).collect::<Result<Vec<_>>>()?
        ]),
        Value::Record(fields) => {
            let fields = fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), data_key(v)?)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            serde_json::json!(["record", fields])
        }
        _ => bail!("group and random keys must be ordinary data values"),
    })
}
pub fn call(e: &mut Evaluator, name: &str, args: Vec<(Option<String>, Value)>) -> Result<Value> {
    let mut a = Args::new(args);
    let name = name.strip_prefix("std.").unwrap_or(name);
    let result = match name {
        "keys" => Value::Array(
            a.req("record")?
                .record()?
                .keys()
                .cloned()
                .map(Value::Str)
                .collect(),
        ),
        "group_by" => {
            let values = a.req("list")?;
            let f = a.req("function")?;
            if values.array()?.len() > 200_000 {
                bail!("group_by exceeds 200000 items");
            }
            let mut indices = BTreeMap::new();
            let mut groups: Vec<Vec<Value>> = Vec::new();
            for value in values.array()? {
                let key = data_key(&e.call(f.clone(), vec![(None, value.clone())])?)?.to_string();
                let next = groups.len();
                let index = *indices.entry(key).or_insert(next);
                if index == next {
                    groups.push(Vec::new());
                }
                groups[index].push(value.clone());
            }
            Value::Array(groups.into_iter().map(|v| Value::Array(v.into())).collect())
        }
        "overlay" => {
            let patterns = a.req("patterns")?;
            preflight_patterns(patterns.array()?, 0, e.expansion_limits)?;
            let mut p = Pattern::default();
            let mut keys = BTreeSet::new();
            for v in patterns.array()? {
                for n in &v.pattern()?.notes {
                    if !keys.insert(n.key.clone()) {
                        bail!(
                            "duplicate note identity '{}' in overlay; namespace occurrences with at(..., key=...)",
                            n.key
                        );
                    }
                }
                p.overlay(v.pattern()?.clone());
            }
            p.validate_local()?;
            pat(p)
        }
        "keyed_noise" => {
            let key = a.req("key")?;
            let seed = a.num("seed", 0.0)?;
            let stream = a.take("stream").unwrap_or(Value::integer(0));
            if seed.fract() != 0.0 || !(0.0..=9_007_199_254_740_991.0).contains(&seed) {
                bail!("random seed must be a nonnegative integer no larger than 2^53-1");
            }
            Value::num(noise(hash(
                &format!("{}|{}", data_key(&key)?, data_key(&stream)?),
                seed as u64,
            )))
        }
        "map_notes" | "flat_map_notes" | "filter_notes" => {
            let original = a.req("pattern")?;
            let original = original.pattern()?;
            original.validate_local()?;
            let f = a.req("function")?;
            let sel = a.take("selector").unwrap_or(Value::Str("all".into()));
            let mut p = Pattern {
                notes: Vec::new(),
                ..original.clone()
            };
            let mut cost = crate::limits::ExpansionCost::of(&p)?;
            for (i, n) in original.notes.iter().enumerate() {
                if !select(e, n, i, original.notes.len(), &sel)? {
                    cost = cost
                        .plus(crate::limits::ExpansionCost::note(n)?)?
                        .check(e.expansion_limits)?;
                    p.notes.push(n.clone());
                    continue;
                }
                let result = e.call(f.clone(), vec![(None, note_value(n))])?;
                if name == "filter_notes" {
                    if result.truth() {
                        cost = cost
                            .plus(crate::limits::ExpansionCost::note(n)?)?
                            .check(e.expansion_limits)?;
                        p.notes.push(n.clone());
                    }
                    continue;
                }
                let patches = if matches!(result, Value::Null) {
                    vec![]
                } else if name == "flat_map_notes" {
                    result.array()?.to_vec()
                } else {
                    vec![result]
                };
                if p.notes.len() + patches.len() > 200_000 {
                    bail!("pattern exceeds 200000 notes");
                }
                let expanded = patches.len() > 1;
                for (j, patch) in patches.iter().enumerate() {
                    let mut hit = n.clone();
                    if expanded && !patch.record()?.contains_key("key") {
                        hit.key = format!("{}/expand{j}", n.key);
                    }
                    patch_note(&mut hit, patch)?;
                    cost = cost
                        .plus(crate::limits::ExpansionCost::note(&hit)?)?
                        .check(e.expansion_limits)?;
                    p.notes.push(hit);
                }
            }
            p.validate_local()?;
            let mut keys = BTreeSet::new();
            for n in &p.notes {
                if !keys.insert(&n.key) {
                    bail!(
                        "note transformation produced duplicate identity '{}'",
                        n.key
                    );
                }
            }
            pat(p)
        }
        "map_controls" | "flat_map_controls" | "map_raw" | "flat_map_raw" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            let f = a.req("function")?;
            p.validate_local()?;
            let expand = name.starts_with("flat_");
            if name.ends_with("controls") {
                let original = std::mem::take(&mut p.controls);
                let mut cost = crate::limits::ExpansionCost::of(&p)?;
                for c in original {
                    let result = e.call(f.clone(), vec![(None, control_value(&c))])?;
                    let patches = if matches!(result, Value::Null) {
                        vec![]
                    } else if expand {
                        result.array()?.to_vec()
                    } else {
                        vec![result]
                    };
                    if p.controls.len() + patches.len() > 200_000 {
                        bail!("pattern exceeds 200000 controls");
                    }
                    for patch in patches {
                        let mut out = c.clone();
                        for (k, v) in patch.record()?.iter() {
                            match k.as_str() {
                                "at" => out.at = v.beats()?,
                                "offset" => out.offset_ms = amount_ms(v.clone())?,
                                "controller" => out.cc = byte(v, 127)?,
                                "value" => out.value = byte(v, 127)?,
                                _ => {
                                    return Err(Diagnostic::new(format!(
                                        "unknown control field '{k}'"
                                    ))
                                    .helps(super::diagnostic::suggest_vocabulary(
                                        "control fields",
                                        k,
                                        ["at", "offset", "controller", "value"],
                                    ))
                                    .err());
                                }
                            }
                        }
                        cost = cost
                            .plus(crate::limits::ExpansionCost {
                                controls: 1,
                                bytes: std::mem::size_of_val(&out),
                                ..Default::default()
                            })?
                            .check(e.expansion_limits)?;
                        p.controls.push(out);
                    }
                }
            } else {
                let original = std::mem::take(&mut p.raw);
                let mut cost = crate::limits::ExpansionCost::of(&p)?;
                for r in original {
                    let result = e.call(f.clone(), vec![(None, raw_value(&r))])?;
                    let patches = if matches!(result, Value::Null) {
                        vec![]
                    } else if expand {
                        result.array()?.to_vec()
                    } else {
                        vec![result]
                    };
                    if p.raw.len() + patches.len() > 200_000 {
                        bail!("pattern exceeds 200000 raw events");
                    }
                    for patch in patches {
                        let mut out = r.clone();
                        for (k, v) in patch.record()?.iter() {
                            match k.as_str() {
                                "at" => out.at = v.beats()?,
                                "offset" => out.offset_ms = amount_ms(v.clone())?,
                                "bytes" => {
                                    out.bytes = v
                                        .array()?
                                        .iter()
                                        .map(|v| byte(v, 255))
                                        .collect::<Result<_>>()?
                                }
                                _ => {
                                    return Err(Diagnostic::new(format!(
                                        "unknown raw event field '{k}'"
                                    ))
                                    .helps(super::diagnostic::suggest_vocabulary(
                                        "raw event fields",
                                        k,
                                        ["at", "offset", "bytes"],
                                    ))
                                    .err());
                                }
                            }
                        }
                        cost = cost
                            .plus(crate::limits::ExpansionCost {
                                raw: 1,
                                bytes: std::mem::size_of_val(&out).saturating_add(out.bytes.len()),
                                ..Default::default()
                            })?
                            .check(e.expansion_limits)?;
                        p.raw.push(out);
                    }
                }
            }
            p.validate_local()?;
            pat(p)
        }
        "control" => {
            let controller = byte(&a.req("controller")?, 127)?;
            let value = byte(&a.req("value")?, 127)?;
            let at = a.beat("at", b(0))?;
            let offset_ms = a.take("offset").map(amount_ms).transpose()?.unwrap_or(0.0);
            let p = Pattern {
                span: at,
                controls: vec![Control {
                    at,
                    offset_ms,
                    cc: controller,
                    value,
                }],
                ..Default::default()
            };
            p.validate_local()?;
            pat(p)
        }
        "midi" | "midi_tempos" => {
            let file = a.req("path")?.text()?.to_owned();
            let path = crate::assets::resolve(
                &e.path
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join(file),
            )?;
            if !e.dependencies.contains(&path) {
                e.dependencies.push(path.clone());
            }
            let doc = crate::smf::read(&path)?;
            let track = a
                .take("track")
                .map(|v| v.integer_in(0, 65535).map(|n| n as usize))
                .transpose()?;
            if name == "midi" {
                pat(crate::smf::pattern(&doc, track)?)
            } else {
                if doc.division & 0x8000 != 0 {
                    bail!("SMPTE files have no musical beat map");
                }
                if doc.format == 2 && track.is_none() {
                    bail!("select an independent format-2 sequence");
                }
                let mut points = vec![];
                for (i, tr) in doc
                    .tracks
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| track.is_none_or(|t| t == *i))
                {
                    let mut tick = 0u64;
                    for ev in tr {
                        tick += ev.delta as u64;
                        if let crate::smf::Kind::Meta { tag: 0x51, data } = &ev.kind {
                            if data.len() != 3 {
                                bail!("invalid tempo in track {i}");
                            }
                            let n =
                                ((data[0] as u32) << 16) | ((data[1] as u32) << 8) | data[2] as u32;
                            if n == 0 {
                                bail!("zero MIDI tempo");
                            }
                            points.push((
                                tick,
                                Value::Array(
                                    vec![
                                        Value::beat(b(tick as i64) / b(doc.division as i64)),
                                        Value::num(60_000_000. / n as f64),
                                    ]
                                    .into(),
                                ),
                            ));
                        }
                    }
                }
                points.sort_by_key(|p| p.0);
                Value::Array(points.into_iter().map(|p| p.1).collect())
            }
        }
        "clip" => {
            let id = a.req("id")?;
            let file = a.req("path")?.text()?.to_owned();
            let options = a.take("options").unwrap_or(rec(BTreeMap::new()));
            let mut opts = options.record()?.clone();
            let path = crate::assets::resolve(
                &e.path
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join(file),
            )?;
            if !e.dependencies.contains(&path) {
                e.dependencies.push(path.clone());
            }
            let info = crate::audio_file::info(&path)?;
            let length = info.frames as f64 / info.rate as f64;
            let offset = opts
                .remove("offset")
                .map(clock_seconds)
                .transpose()?
                .unwrap_or(0.0);
            let at = opts
                .remove("at")
                .map(clock_seconds)
                .transpose()?
                .unwrap_or(0.0);
            let duration = opts
                .remove("duration")
                .map(clock_seconds)
                .transpose()?
                .unwrap_or(length - offset);
            let fade_in = opts
                .remove("fade_in")
                .map(amount_ms)
                .transpose()?
                .unwrap_or(5.);
            let fade_out = opts
                .remove("fade_out")
                .map(amount_ms)
                .transpose()?
                .unwrap_or(10.);
            if offset < 0.
                || at < 0.
                || duration <= 0.
                || duration + offset > length + 1e-6
                || fade_out < 0.
                || fade_in < 0.
                || fade_out / 1000. >= duration
            {
                bail!("invalid clip trim, position or fades");
            }
            let mut note = Note::new(b(0), b(1), 60., "clip".into());
            note.gate = 1.;
            note.velocity = 1.;
            note.clock = Some(music::ClockPlacement::new(
                at,
                duration - fade_out / 1000.,
                duration,
            )?);
            opts.insert("id".into(), id);
            opts.insert(
                "pattern".into(),
                pat(Pattern {
                    span: b(1),
                    notes: vec![note],
                    ..Default::default()
                }),
            );
            opts.insert(
                "instrument".into(),
                record([
                    ("type", Value::Str("sample".into())),
                    ("name", Value::Str(path.display().to_string())),
                    ("offset", Value::num(offset)),
                    ("attack_ms", Value::num(fade_in)),
                    ("release_ms", Value::num(fade_out)),
                ]),
            );
            rec(opts)
        }
        "notes_only" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            p.raw.clear();
            p.controls.clear();
            pat(p)
        }
        "phrase" => pat(music::phrase(a.req("notes")?.text()?)?),
        "note" => {
            let pitch = a.req("pitch")?;
            let pitch = if let Value::Str(s) = pitch {
                music::pitch(&s)?
            } else {
                pitch.scalar()?
            };
            let dur = a.beat("duration", b(1))?;
            let at = a.beat("at", b(0))?;
            let mut n = Note::new(at, dur, pitch, "note".into());
            n.velocity = a.num("velocity", 0.72)?;
            let mut p = Pattern {
                span: checked_time(at.checked_add(&dur))?,
                ..Default::default()
            };
            p.notes.push(n);
            pat(p)
        }
        "rest" => pat(Pattern {
            span: a.req("duration")?.beats()?,
            ..Default::default()
        }),
        "seq" | "stack" => {
            let ps = a.req("patterns")?;
            preflight_patterns(ps.array()?, 32, e.expansion_limits)?;
            let mut p = Pattern::default();
            for (i, v) in ps.array()?.iter().enumerate() {
                let offset = if name == "seq" { p.span } else { b(0) };
                p.overlay(v.pattern()?.shifted(offset, &format!("{name}{i}"))?);
            }
            pat(p)
        }
        "repeat" => {
            let p = a.req("pattern")?;
            let p = p.pattern()?;
            let count = a.num("count", 2.0)?;
            if count.fract() != 0.0 || !(0.0..=10000.0).contains(&count) {
                bail!("repeat count must be an integer in 0..10000");
            }
            if p.notes.len().saturating_mul(count as usize) > 200000 {
                bail!("repeat exceeds note budget");
            }
            crate::limits::ExpansionCost::of(p)?
                .repeated(count as usize, 16)?
                .check(e.expansion_limits)?;
            let mut out = Pattern::default();
            for i in 0..count as usize {
                let offset = checked_time(p.span.checked_mul(&b(i as i64)))?;
                out.overlay(p.shifted(offset, &format!("repeat{i}"))?);
            }
            pat(out)
        }
        "at" | "place" => {
            let p = a.req("pattern")?;
            let at = a.req("at")?.beats()?;
            let key = a.txt("key", &format!("at{}", real(at)))?;
            crate::limits::ExpansionCost::of(p.pattern()?)?
                .repeated(1, key.len().saturating_add(1))?
                .check(e.expansion_limits)?;
            pat(p.pattern()?.shifted(at, &key)?)
        }
        "slice" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            p.require_score_time(name)?;
            let start = a.req("from")?.beats()?;
            let end = a.req("to")?.beats()?;
            if end <= start {
                bail!("slice end must be after start");
            }
            let mut notes = Vec::new();
            for mut n in p.notes {
                let note_end = checked_time(n.at.checked_add(&n.dur))?;
                if note_end <= start || n.at >= end {
                    continue;
                }
                let off = n.at.max(start);
                n.dur = checked_time(note_end.min(end).checked_sub(&off))?;
                n.at = checked_time(off.checked_sub(&start))?;
                notes.push(n);
            }
            p.notes = notes;
            p.controls.retain(|c| c.at >= start && c.at < end);
            for c in &mut p.controls {
                c.at = checked_time(c.at.checked_sub(&start))?;
            }
            p.raw.retain(|r| r.at >= start && r.at < end);
            for r in &mut p.raw {
                r.at = checked_time(r.at.checked_sub(&start))?;
            }
            p.span = checked_time(end.checked_sub(&start))?;
            pat(p)
        }
        "stretch" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            p.require_score_time(name)?;
            let factor = a.req("factor")?.scalar_exact()?;
            if factor <= b(0) {
                bail!("stretch factor must be positive");
            }
            p.span = checked_time(p.span.checked_mul(&factor))?;
            for n in &mut p.notes {
                n.at = checked_time(n.at.checked_mul(&factor))?;
                n.dur = checked_time(n.dur.checked_mul(&factor))?;
            }
            for c in &mut p.controls {
                c.at = checked_time(c.at.checked_mul(&factor))?;
            }
            for r in &mut p.raw {
                r.at = checked_time(r.at.checked_mul(&factor))?;
            }
            pat(p)
        }
        "fit" => {
            let p = a.req("pattern")?;
            let p = p.pattern()?;
            p.require_score_time(name)?;
            let span = a.req("duration")?.beats()?;
            if p.span <= b(0) || span < b(0) {
                bail!("fit requires a nonempty pattern and nonnegative extent");
            }
            let ratio = checked_time(span.checked_div(&p.span))?;
            let repeats = (ratio.numer() / ratio.denom()
                + i64::from(ratio.numer() % ratio.denom() != 0)) as usize;
            if repeats > 10000 || repeats.saturating_mul(p.notes.len()) > 200000 {
                bail!("fit exceeds event budget");
            }
            crate::limits::ExpansionCost::of(p)?
                .repeated(repeats, 16)?
                .check(e.expansion_limits)?;
            let mut out = Pattern::default();
            for i in 0..repeats {
                let offset = checked_time(p.span.checked_mul(&b(i as i64)))?;
                out.overlay(p.shifted(offset, &format!("fit{i}"))?);
            }
            out.notes.retain(|n| n.at < span);
            for n in &mut out.notes {
                n.dur = n.dur.min(checked_time(span.checked_sub(&n.at))?);
            }
            out.controls.retain(|c| c.at <= span);
            out.raw.retain(|r| r.at < span);
            out.span = span;
            pat(out)
        }
        "express" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            let value = a.req("values")?;
            scalar_data(&value)?;
            let values = value.json();
            crate::expression::Program::parse(Some(&values))?;
            let selector = a.take("selector").unwrap_or(Value::Str("all".into()));
            let len = p.notes.len();
            for (i, n) in p.notes.iter_mut().enumerate() {
                if select(e, n, i, len, &selector)? {
                    let mut merged = n
                        .data
                        .get("expression")
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_default();
                    for (k, v) in values.as_object().unwrap() {
                        merged.insert(k.clone(), v.clone());
                    }
                    let merged = serde_json::Value::Object(merged);
                    crate::expression::Program::parse(Some(&merged))?;
                    n.data.insert("expression".into(), merged);
                }
            }
            pat(p)
        }
        "allocate_hands" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            let reach = a.num("reach", 12.)?;
            if !(1.0..=24.).contains(&reach) {
                bail!("hand reach must be 1..24 semitones");
            }
            let preferences = crate::performance::Preferences::parse(&a.req("preferences")?)?;
            crate::performance::hands(&mut p, reach, &preferences);
            pat(p)
        }
        "transpose" | "gate" | "velocity" | "gain" | "hand" | "voice" | "reverse" | "invert" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            match name {
                "transpose" => {
                    let v = a.req("semitones")?.scalar()?;
                    for n in &mut p.notes {
                        n.pitch += v;
                    }
                }
                "gate" => {
                    let v = a.req("value")?.scalar()?;
                    if !v.is_finite() || v <= 0.0 {
                        bail!("{name} requires a positive finite value");
                    }
                    for n in &mut p.notes {
                        n.gate = v;
                    }
                    p.validate()?;
                }
                "velocity" => {
                    let v = a.req("value")?.scalar()?;
                    for n in &mut p.notes {
                        n.velocity = v;
                    }
                }
                "gain" => {
                    let v = a.req("factor")?.scalar()?;
                    for n in &mut p.notes {
                        n.velocity = (n.velocity * v).clamp(0.001, 1.0);
                    }
                }
                "hand" | "voice" => {
                    let v = a.req("name")?.text()?.to_owned();
                    for n in &mut p.notes {
                        if name == "hand" {
                            if v != "left" && v != "right" {
                                return Err(Diagnostic::new("hand must be left or right")
                                    .helps(super::diagnostic::suggest_vocabulary(
                                        "hands",
                                        &v,
                                        ["left", "right"],
                                    ))
                                    .err());
                            }
                            n.hand = Some(v.clone());
                        } else {
                            n.voice = v.clone();
                        }
                    }
                }
                "reverse" => {
                    p.require_score_time(name)?;
                    for n in &mut p.notes {
                        n.at = checked_time(
                            p.span
                                .checked_sub(&n.at)
                                .and_then(|at| at.checked_sub(&n.dur)),
                        )?;
                    }
                    p.notes.sort_by_key(|n| n.at);
                }
                "invert" => {
                    let center = a.num("around", p.notes.first().map_or(60.0, |n| n.pitch))?;
                    for n in &mut p.notes {
                        n.pitch = 2.0 * center - n.pitch;
                    }
                }
                _ => unreachable!(),
            };
            p.validate()?;
            pat(p)
        }
        "tag" | "annotate" | "select" | "reject" | "refine" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            let selector = a.take("selector").unwrap_or(Value::Str("all".into()));
            let name_value = if name == "tag" {
                Some(a.req("name")?)
            } else {
                None
            };
            let payload = if name == "annotate" || name == "refine" {
                Some(a.req("values")?)
            } else {
                None
            };
            let len = p.notes.len();
            let mut selected = BTreeSet::new();
            for (i, n) in p.notes.iter_mut().enumerate() {
                if select(e, n, i, len, &selector)? {
                    selected.insert(i);
                    if let Some(t) = &name_value {
                        n.tags.insert(t.text()?.into());
                    }
                    if let Some(v) = &payload {
                        let resolved = if matches!(v, Value::Function(_)) {
                            e.call(v.clone(), vec![(None, note_value(n))])?
                        } else {
                            v.clone()
                        };
                        for (k, v) in resolved.record()?.iter() {
                            if name == "refine" {
                                patch_note(n, &record_patch(k, v))?;
                            } else {
                                n.data.insert(k.clone(), annotation_value(k, v)?);
                            }
                        }
                    }
                }
            }
            if name == "select" || name == "reject" {
                p.notes = p
                    .notes
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| selected.contains(i) == (name == "select"))
                    .map(|(_, n)| n)
                    .collect();
            } else if selected.is_empty() {
                bail!("selector matched no notes");
            }
            p.validate()?;
            pat(p)
        }
        "chord" => {
            let symbol = a.req("symbol")?;
            let octave = a
                .take("octave")
                .unwrap_or(Value::integer(3))
                .integer_in(-1, 9)? as i32;
            Value::Array(
                music::chord(symbol.text()?, octave)?
                    .into_iter()
                    .map(Value::num)
                    .collect(),
            )
        }
        "pitch" => {
            let value = a.req("name")?;
            let octave = a.take("octave");
            Value::num(if let Value::Str(text) = value {
                if let Some(octave) = octave {
                    music::chord(&text, octave.integer_in(-1, 9)? as i32)?[0]
                } else {
                    music::pitch(&text)?
                }
            } else {
                value.scalar()?
            })
        }
        "chords" => {
            let symbols = a.req("symbols")?;
            let each = a.beat("each", b(4))?;
            let octave = a
                .take("octave")
                .unwrap_or(Value::integer(3))
                .integer_in(-1, 9)? as i32;
            let mut p = Pattern::default();
            for (i, s) in symbols.text()?.split_whitespace().enumerate() {
                for (j, pitch) in music::chord(s, octave)?.iter().enumerate() {
                    let mut n = Note::new(p.span, each, *pitch, format!("chord{i}.{j}"));
                    n.tags.insert("harmony".into());
                    n.data.insert("chord".into(), serde_json::json!(s));
                    p.notes.push(n);
                }
                p.span = checked_time(p.span.checked_add(&each))?;
            }
            pat(p)
        }
        "voicelead_solve" => {
            let p = a.req("harmony")?;
            let low = a.req("low")?.scalar()?;
            let high = a.req("high")?.scalar()?;
            let center = a.req("center")?.scalar()?;
            let scoring = tonal_scoring(a.req("scoring")?)?;
            pat(crate::tonal::voicelead(
                p.pattern()?,
                low,
                high,
                center,
                &scoring,
            )?)
        }
        "reharmonize_solve" | "reharmonizations_solve" => {
            let h = a.req("harmony")?;
            let melody = a.req("melody")?;
            let candidates = a.req("candidates")?;
            let symbols = candidates
                .array()?
                .iter()
                .map(|v| v.text().map(str::to_owned))
                .collect::<Result<Vec<_>>>()?;
            let octave = a.req("octave")?.integer_in(-1, 9)? as i32;
            let scoring = tonal_scoring(a.req("scoring")?)?;
            if name == "reharmonizations_solve" {
                let count = a.req("count")?.integer_in(0, 10000)? as usize;
                Value::Array(
                    crate::tonal::alternatives(
                        h.pattern()?,
                        melody.pattern()?,
                        &symbols,
                        octave,
                        count,
                        &scoring,
                    )?
                    .into_iter()
                    .map(|(p, score)| record([("harmony", pat(p)), ("score", Value::num(score))]))
                    .collect(),
                )
            } else {
                pat(crate::tonal::reharmonize(
                    h.pattern()?,
                    melody.pattern()?,
                    &symbols,
                    octave,
                    &scoring,
                )?)
            }
        }
        "diatonic_transpose" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            let scale = a
                .req("scale")?
                .array()?
                .iter()
                .map(Value::number)
                .collect::<Result<Vec<_>>>()?;
            let steps = a.req("steps")?.integer_in(1, 200000)?;
            if scale.is_empty() {
                bail!("scale is empty")
            };
            for n in &mut p.notes {
                let index = (-128..128)
                    .min_by(|&a, &b| {
                        (crate::tonal::degree(&scale, a).unwrap() - n.pitch)
                            .abs()
                            .total_cmp(&(crate::tonal::degree(&scale, b).unwrap() - n.pitch).abs())
                    })
                    .unwrap();
                let original = crate::tonal::degree(&scale, index)?;
                n.pitch = crate::tonal::degree(&scale, index + steps)? + (n.pitch - original);
            }
            p.validate()?;
            pat(p)
        }
        "split" => {
            let p = a.req("pattern")?.pattern()?.clone();
            let sel = a.req("selector")?;
            let mut chosen = p.clone();
            let mut remaining = p.clone();
            chosen.notes.clear();
            remaining.notes.clear();
            chosen.controls.clear();
            chosen.raw.clear();
            for (i, n) in p.notes.iter().enumerate() {
                if select(e, n, i, p.notes.len(), &sel)? {
                    chosen.notes.push(n.clone());
                } else {
                    remaining.notes.push(n.clone());
                }
            }
            record([("selected", pat(chosen)), ("remaining", pat(remaining))])
        }
        "drum_grid" => {
            let lanes = a.req("lanes")?;
            let voices = a.req("voices")?;
            let articulations = a.req("articulations")?;
            let gate = a.num("gate", 0.5)?;
            if !(0.0..=1.0).contains(&gate) {
                bail!("grid gate must be 0..1");
            }
            let span = a.beat("span", b(4))?;
            let mut p = Pattern {
                span,
                ..Default::default()
            };
            for (voice, grid) in lanes.record()?.iter() {
                let key = voices
                    .record()?
                    .get(voice)
                    .ok_or_else(|| {
                        let known: Vec<&str> = voices
                            .record()
                            .map(|r| r.keys().map(String::as_str).collect())
                            .unwrap_or_default();
                        Diagnostic::new(format!("grid has no pitch mapping for voice '{voice}'"))
                            .helps(super::diagnostic::suggest_vocabulary(
                                "voices", voice, known,
                            ))
                            .err()
                    })?
                    .scalar()?;
                let chars: Vec<_> = grid
                    .text()?
                    .chars()
                    .filter(|c| !c.is_whitespace() && *c != '|')
                    .collect();
                if chars.len() > 200000 {
                    bail!("grid lane exceeds step budget");
                }
                if chars.is_empty() {
                    continue;
                }
                let step = checked_time(span.checked_div(&b(chars.len() as i64)))?;
                for (i, c) in chars.iter().enumerate() {
                    let symbol = c.to_string();
                    let articulation = articulations.record()?.get(&symbol).ok_or_else(|| {
                        let known: Vec<&str> = articulations
                            .record()
                            .map(|r| r.keys().map(String::as_str).collect())
                            .unwrap_or_default();
                        Diagnostic::new(format!("invalid drum grid symbol '{c}'"))
                            .helps(super::diagnostic::suggest_vocabulary(
                                "symbols", &symbol, known,
                            ))
                            .help("the bundled kit vocabularies map '.' to a rest step")
                            .err()
                    })?;
                    if matches!(articulation, Value::Null) {
                        continue;
                    }
                    let velocity = articulation.scalar()?;
                    if !(0.0..=1.0).contains(&velocity) {
                        bail!("grid velocity must be 0..1");
                    }
                    if p.notes.len() >= 200000 {
                        bail!("grid exceeds note budget");
                    }
                    let mut n = Note::new(
                        checked_time(step.checked_mul(&b(i as i64)))?,
                        checked_time(step.checked_mul(&rational(gate)?))?,
                        key,
                        format!("{voice}{i}"),
                    );
                    n.voice = voice.to_owned();
                    n.tags.insert(voice.clone());
                    n.velocity = velocity;
                    p.notes.push(n);
                }
            }
            p.validate_local()?;
            pat(p)
        }
        "cc" => {
            let cc = a.req("controller")?.scalar()?;
            let value = a.req("value")?.scalar()?;
            let at = a.beat("at", b(0))?;
            let channel = a.num("channel", 0.)?;
            if channel.fract() != 0.
                || !(0.0..=15.).contains(&channel)
                || cc.fract() != 0.
                || value.fract() != 0.
            {
                bail!("CC/channel values must be integers, channel 0..15")
            }
            if !(0.0..=127.0).contains(&cc) || !(0.0..=127.0).contains(&value) {
                bail!("MIDI controller and value must be 0..127");
            }
            pat(Pattern {
                span: at,
                raw: vec![music::RawEvent {
                    offset_ms: 0.,
                    at,
                    bytes: vec![0xb0 | channel as u8, cc as u8, value as u8],
                }],
                ..Default::default()
            })
        }
        "seconds_at" => {
            let position = a.req("position")?;
            let timing = a.take("timing").unwrap_or(rec(BTreeMap::new()));
            let tempos = crate::compile::tempo_map(timing.record()?)?;
            if matches!(&position, Value::Num(q) if q.unit == Unit::Seconds) {
                position
            } else {
                seconds_value(crate::compile::seconds_at(real(position.beats()?), &tempos))
            }
        }
        "sort_by" => {
            let vs = a.req("list")?;
            let f = a.req("function")?;
            if vs.array()?.len() > 200000 {
                bail!("sort_by exceeds 200000 items");
            }
            let mut keyed = Vec::new();
            let mut unit = None;
            let mut strings = None;
            for v in vs.array()? {
                let mut key = e.call(f.clone(), vec![(None, v.clone())])?;
                let is_string = matches!(key, Value::Str(_));
                if strings.is_some_and(|s| s != is_string) {
                    bail!("sort_by keys must have one type");
                }
                strings = Some(is_string);
                match &mut key {
                    Value::Num(q) => {
                        if q.unit == Unit::Bar {
                            q.value = q.value.arithmetic("*", b(4).into(), false)?;
                            q.unit = Unit::Beat;
                        }
                        if unit.is_some_and(|u| u != q.unit) {
                            bail!("sort_by keys must have compatible units");
                        }
                        unit = Some(q.unit);
                    }
                    Value::Str(_) => {}
                    _ => bail!("sort_by keys must be numbers or strings"),
                }
                keyed.push((key, v.clone()));
            }
            keyed.sort_by(|(a, _), (b, _)| match (a, b) {
                (Value::Num(a), Value::Num(b)) => a.value.compare(b.value),
                (Value::Str(a), Value::Str(b)) => a.cmp(b),
                _ => unreachable!(),
            });
            Value::Array(keyed.into_iter().map(|(_, v)| v).collect())
        }
        "range" => {
            let Value::Num(mut end) = a.req("end")? else {
                bail!("range requires numeric bounds");
            };
            let Value::Num(mut start) =
                a.take("start").unwrap_or(Value::Num(super::eval::Quantity {
                    value: b(0).into(),
                    unit: end.unit,
                }))
            else {
                bail!("range requires numeric bounds");
            };
            let Value::Num(mut step) =
                a.take("step").unwrap_or(Value::Num(super::eval::Quantity {
                    value: b(1).into(),
                    unit: end.unit,
                }))
            else {
                bail!("range requires a numeric step");
            };
            for quantity in [&mut end, &mut start, &mut step] {
                if quantity.unit == Unit::Bar {
                    quantity.value = quantity.value.arithmetic("*", b(4).into(), false)?;
                    quantity.unit = Unit::Beat;
                }
            }
            if start.unit != end.unit || step.unit != end.unit {
                bail!("range requires compatible units");
            }
            if step.number() <= 0. || end.value.compare(start.value).is_lt() {
                bail!("range must be bounded, increasing, with positive step");
            }
            let distance = end
                .value
                .arithmetic("-", start.value, end.unit == Unit::Scalar)?;
            let count = distance.arithmetic("/", step.value, true)?;
            if count.number() > 200000. {
                bail!("range must be bounded to 200000 items");
            }
            let count = match count {
                Number::Exact(n) => {
                    (n.numer() / n.denom() + i64::from(n.numer() % n.denom() != 0)) as usize
                }
                Number::Inexact(n) => n.ceil() as usize,
            };
            if count > 200000 {
                bail!("range must be bounded to 200000 items");
            }
            Value::Array(
                (0..count)
                    .map(|i| {
                        let offset = step.value.arithmetic(
                            "*",
                            b(i as i64).into(),
                            end.unit == Unit::Scalar,
                        )?;
                        Ok(Value::Num(super::eval::Quantity {
                            value: start
                                .value
                                .arithmetic("+", offset, end.unit == Unit::Scalar)?,
                            unit: end.unit,
                        }))
                    })
                    .collect::<Result<_>>()?,
            )
        }
        "map" | "filter" => {
            let vs = a.req("list")?;
            let f = a.req("function")?;
            let mut out = vec![];
            for v in vs.array()? {
                let result = e.call(f.clone(), vec![(None, v.clone())])?;
                if name == "map" {
                    out.push(result)
                } else if result.truth() {
                    out.push(v.clone());
                }
            }
            Value::Array(out.into())
        }
        "fold" => {
            let vs = a.req("list")?;
            let mut out = a.req("initial")?;
            let f = a.req("function")?;
            for v in vs.array()? {
                out = e.call(f.clone(), vec![(None, out), (None, v.clone())])?;
            }
            out
        }
        "len" => {
            let v = a.req("value")?;
            Value::integer(match v {
                Value::Array(a) => a.len(),
                Value::Record(r) => r.len(),
                Value::Pattern(p) => p.notes.len(),
                Value::Str(s) => s.len(),
                _ => bail!("len needs collection"),
            } as i64)
        }
        "contains" => {
            let v = a.req("list")?;
            let needle = a.req("value")?;
            let mut found = false;
            for value in v.array()? {
                if value.semantic_eq(&needle)? {
                    found = true;
                    break;
                }
            }
            Value::Bool(found)
        }
        "str" => {
            let v = a.req("value")?;
            Value::Str(match &v {
                Value::Str(s) => s.clone(),
                Value::Num(q) => q.number().to_string(),
                _ => v.json().to_string(),
            })
        }
        "format" => {
            let template = a.req("template")?.text()?.to_owned();
            let values = a.req("values")?;
            let parts = template.split("{}").collect::<Vec<_>>();
            let values = values.array()?;
            if parts.len() != values.len() + 1 {
                bail!("format placeholder/value counts differ");
            }
            let mut out = parts[0].to_owned();
            for (v, suffix) in values.iter().zip(&parts[1..]) {
                out.push_str(&match v {
                    Value::Str(s) => s.clone(),
                    Value::Num(q) => q.number().to_string(),
                    _ => v.json().to_string(),
                });
                out.push_str(suffix);
            }
            Value::Str(out)
        }
        "merge" => {
            let mut r = if a.values.is_empty() {
                Record::new(BTreeMap::new())
            } else {
                a.req("base")?.record()?.clone()
            };
            if let Some(overrides) = a.take("overrides") {
                r.extend(
                    overrides
                        .record()?
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone())),
                );
            }
            while a.values.first().is_some_and(|(name, _)| name.is_none()) {
                let (_, overrides) = a.values.remove(0);
                r.extend(
                    overrides
                        .record()?
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone())),
                );
            }
            rec(r)
        }
        "min" | "max" => {
            let (Value::Num(mut x), Value::Num(mut y)) = (a.req("a")?, a.req("b")?) else {
                bail!("{name} requires numbers");
            };
            if matches!(
                (x.unit, y.unit),
                (Unit::Beat, Unit::Bar) | (Unit::Bar, Unit::Beat)
            ) {
                for q in [&mut x, &mut y] {
                    if q.unit == Unit::Bar {
                        q.value = q.value.arithmetic("*", b(4).into(), false)?;
                        q.unit = Unit::Beat;
                    }
                }
            }
            if x.unit != y.unit {
                bail!("incompatible units {:?} and {:?}", x.unit, y.unit);
            }
            let order = x.value.compare(y.value);
            Value::Num(
                if (name == "min" && order.is_gt()) || (name == "max" && order.is_lt()) {
                    y
                } else {
                    x
                },
            )
        }
        "pow" => {
            let (Value::Num(x), Value::Num(y)) = (a.req("a")?, a.req("b")?) else {
                bail!("pow requires numbers");
            };
            if x.unit != Unit::Scalar || y.unit != Unit::Scalar {
                bail!("pow requires scalar arguments");
            }
            Value::num(x.number().powf(y.number()))
        }
        "sin" | "cos" => {
            let Value::Num(x) = a.req("x")? else {
                bail!("{name} requires a number");
            };
            if x.unit != Unit::Scalar {
                bail!("{name} requires a scalar argument");
            }
            Value::num(if name == "sin" {
                x.number().sin()
            } else {
                x.number().cos()
            })
        }
        "abs" | "floor" | "round" => {
            let Value::Num(mut x) = a.req("x")? else {
                bail!("{name} requires a number");
            };
            x.value = match (name, x.value) {
                ("abs", value) if x.number() < 0. => {
                    Number::Exact(b(0)).arithmetic("-", value, x.unit == Unit::Scalar)?
                }
                ("abs", value) => value,
                ("floor", Number::Exact(value)) => {
                    Number::Exact(b(value.numer().div_euclid(*value.denom())))
                }
                ("round", Number::Exact(value)) => Number::Exact(value.round()),
                ("floor", Number::Inexact(value)) => Number::finite(value.floor())?,
                (_, Number::Inexact(value)) => Number::finite(value.round())?,
                _ => unreachable!(),
            };
            Value::Num(x)
        }
        "song" => {
            let mut r = a.req("settings")?.record()?.clone();
            r.insert("type".into(), Value::Str("song".into()));
            rec(r)
        }
        "section" => {
            let name = a.req("name")?;
            let duration = a.req("duration")?;
            let options = a.take("options").unwrap_or(rec(BTreeMap::new()));
            let mut r = options.record()?.clone();
            r.insert("name".into(), name);
            r.insert("duration".into(), duration);
            rec(r)
        }
        "piano" | "fx" | "plugin" | "sample" | "voice_patch" => {
            let name_value = if name == "piano" {
                a.take("name").unwrap_or(Value::Str("default".into()))
            } else {
                a.req("name")?
            };
            let options = a.take("params").unwrap_or(rec(BTreeMap::new()));
            let mut r = options.record()?.clone();
            if matches!(name, "sample" | "plugin" | "piano" | "voice_patch") {
                r.insert(
                    "_module_dir".into(),
                    Value::Str(
                        e.path
                            .parent()
                            .unwrap_or(std::path::Path::new("."))
                            .display()
                            .to_string(),
                    ),
                );
            }
            r.insert("type".into(), Value::Str(name.into()));
            r.insert("name".into(), name_value);
            rec(r)
        }
        "rack" => {
            let branches = a.req("branches")?;
            let options = a.take("options").unwrap_or(rec(BTreeMap::new()));
            let mut r = options.record()?.clone();
            r.insert("type".into(), Value::Str("rack".into()));
            r.insert("branches".into(), branches);
            rec(r)
        }
        "bus" => {
            let id = a.req("name")?;
            let chain = a.req("chain")?;
            let options = a.take("options").unwrap_or(rec(BTreeMap::new()));
            let mut r = options.record()?.clone();
            r.insert("id".into(), id);
            r.insert("chain".into(), chain);
            rec(r)
        }
        "curve" => {
            let points = a.req("points")?;
            let shape = a.take("shape").unwrap_or(Value::Str("linear".into()));
            record([("points", points), ("shape", shape)])
        }
        "curve_value" => {
            let curve = a.req("curve")?;
            let positions = a.req("positions")?;
            super::controls::value(&curve, &positions)?
        }
        "unit" => {
            let value = a.req("value")?;
            let Value::Num(mut quantity) = value else {
                bail!("unit needs a number");
            };
            quantity.value = b(1).into();
            Value::Num(quantity)
        }
        "automation" => {
            let target = a.req("target")?;
            let curve = a.req("curve")?;
            record([("target", target), ("curve", curve)])
        }
        "channel" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            let ch = a.req("number")?.scalar()?;
            if ch.fract() != 0. || !(0.0..=15.).contains(&ch) {
                bail!("channel is 0..15")
            };
            let ch = ch as u8;
            for n in &mut p.notes {
                n.data.insert("channel".into(), serde_json::json!(ch));
            }
            for c in p.controls.drain(..) {
                p.raw.push(music::RawEvent {
                    offset_ms: c.offset_ms,
                    at: c.at,
                    bytes: vec![0xb0 | ch, c.cc, c.value],
                });
            }
            for r in &mut p.raw {
                if r.bytes.first().is_some_and(|s| *s >= 0x80 && *s < 0xf0) {
                    r.bytes[0] = (r.bytes[0] & 0xf0) | ch;
                }
            }
            pat(p)
        }
        "note_on" | "note_off" | "program" | "bank" | "bend" | "pressure" | "poly_pressure"
        | "sysex" | "meta" | "opaque" => {
            let mut bytes = match name {
                "note_on" | "note_off" => {
                    let key = a.req("pitch")?;
                    let key = if let Value::Str(s) = key {
                        music::pitch(&s)?
                    } else {
                        key.scalar()?
                    };
                    let velocity = a.num("velocity", if name == "note_on" { 0.7 } else { 0.3 })?;
                    if key.fract() != 0.
                        || !(0.0..=127.).contains(&key)
                        || !(0.0..=1.).contains(&velocity)
                    {
                        bail!("MIDI note needs integer pitch 0..127 and velocity 0..1")
                    };
                    vec![
                        if name == "note_on" { 0x90 } else { 0x80 },
                        key as u8,
                        (velocity * 127.).round() as u8,
                    ]
                }
                "program" | "bank" => {
                    let value = a.req("number")?.scalar()?;
                    let max = if name == "bank" { 16383. } else { 127. };
                    if value.fract() != 0. || !(0.0..=max).contains(&value) {
                        bail!("number must be integer 0..{max}")
                    };
                    if name == "bank" {
                        vec![
                            0xb0,
                            0,
                            ((value as u16) >> 7) as u8,
                            0xb0,
                            32,
                            (value as u16 & 127) as u8,
                        ]
                    } else {
                        vec![0xc0, value as u8]
                    }
                }
                "bend" => {
                    let value = a.req("value")?.scalar()?;
                    if !(-1.0..=1.).contains(&value) {
                        bail!("bend is -1..1 of the destination's configured bend range")
                    };
                    let v =
                        (8192. + value * if value >= 0. { 8191. } else { 8192. }).round() as u16;
                    vec![0xe0, (v & 127) as u8, (v >> 7) as u8]
                }
                "pressure" | "poly_pressure" => {
                    let key = if name == "poly_pressure" {
                        Some(a.req("pitch")?.scalar()?)
                    } else {
                        None
                    };
                    let value = a.req("value")?.scalar()?;
                    if !(0.0..=1.).contains(&value)
                        || key.is_some_and(|k| k.fract() != 0. || !(0.0..=127.).contains(&k))
                    {
                        bail!("pressure needs value 0..1 and key 0..127")
                    };
                    if let Some(k) = key {
                        vec![0xa0, k as u8, (value * 127.).round() as u8]
                    } else {
                        vec![0xd0, (value * 127.).round() as u8]
                    }
                }
                _ => {
                    let tag = if name == "meta" {
                        Some(a.req("type")?.scalar()?)
                    } else {
                        None
                    };
                    let data = a.req("data")?;
                    let data = data
                        .array()?
                        .iter()
                        .map(|v| {
                            let n = v.scalar()?;
                            if n.fract() != 0. || !(0.0..=255.).contains(&n) {
                                bail!("payload bytes are 0..255")
                            };
                            Ok(n as u8)
                        })
                        .collect::<Result<Vec<_>>>()?;
                    if name == "sysex" {
                        if data.iter().any(|v| *v > 127) {
                            bail!("SysEx payload is 7-bit; omit F0/F7")
                        };
                        [vec![0xf0], data, vec![0xf7]].concat()
                    } else if let Some(tag) = tag {
                        if tag.fract() != 0. || !(0.0..=127.).contains(&tag) {
                            bail!("meta type is 0..127")
                        };
                        [vec![0xff, tag as u8], data].concat()
                    } else {
                        data
                    }
                }
            };
            let at = a.beat("at", b(0))?;
            let channel = a.num("channel", 0.)?;
            if channel.fract() != 0. || !(0.0..=15.).contains(&channel) {
                bail!("channel is 0..15")
            };
            if bytes.first().is_some_and(|b| *b >= 0x80 && *b < 0xf0) {
                bytes[0] |= channel as u8;
            }
            let raw = if name == "bank" {
                bytes[3] |= channel as u8;
                vec![
                    music::RawEvent {
                        offset_ms: 0.,
                        at,
                        bytes: bytes[..3].to_vec(),
                    },
                    music::RawEvent {
                        offset_ms: 0.,
                        at,
                        bytes: bytes[3..].to_vec(),
                    },
                ]
            } else {
                vec![music::RawEvent {
                    offset_ms: 0.,
                    at,
                    bytes,
                }]
            };
            pat(Pattern {
                span: at,
                raw,
                ..Default::default()
            })
        }
        "raw_midi" => {
            let bytes = a
                .req("bytes")?
                .array()?
                .iter()
                .map(|v| {
                    let n = v.scalar()?;
                    if !(0.0..=255.0).contains(&n) || n.fract() != 0.0 {
                        bail!("MIDI bytes must be integers 0..255");
                    }
                    Ok(n as u8)
                })
                .collect::<Result<Vec<_>>>()?;
            let at = a.beat("at", b(0))?;
            pat(Pattern {
                span: at,
                raw: vec![music::RawEvent {
                    offset_ms: 0.,
                    at,
                    bytes,
                }],
                ..Default::default()
            })
        }
        "assert" => {
            let condition = a.req("condition")?.truth();
            let message = a.txt("message", "assertion failed")?;
            if !condition {
                bail!("{message}");
            }
            Value::Null
        }
        _ => return Err(UnknownFunction(name.into()).into()),
    };
    a.done()?;
    if let Value::Pattern(p) = &result {
        crate::limits::ExpansionCost::of(p)?.check(e.expansion_limits)?;
    }
    if let Value::Invalid(message) = &result {
        bail!("{message}");
    }
    Ok(result)
}
fn scalar_data(v: &Value) -> Result<()> {
    match v {
        Value::Num(_) => {
            v.scalar()?;
        }
        Value::Array(a) => {
            for v in a.iter() {
                scalar_data(v)?;
            }
        }
        Value::Record(r) => {
            for v in r.values() {
                scalar_data(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn annotation_value(key: &str, value: &Value) -> Result<serde_json::Value> {
    match key {
        "channel" => {
            value.integer_in(0, 15)?;
        }
        "sample_zone" => {
            value.integer_in(0, i64::MAX)?;
        }
        "expression" => scalar_data(value)?,
        _ => {}
    }
    Ok(value.json())
}
pub fn hash(s: &str, seed: u64) -> u64 {
    s.bytes().fold(14695981039346656037u64 ^ seed, |h, b| {
        (h ^ b as u64).wrapping_mul(1099511628211)
    })
}
pub fn noise(mut x: u64) -> f64 {
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    (x.wrapping_mul(2685821657736338717) >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

fn tonal_scoring(value: Value) -> Result<crate::tonal::Scoring> {
    crate::tonal::Scoring::new(
        value
            .record()?
            .iter()
            .map(|(k, v)| Ok((k.clone(), v.scalar()?)))
            .collect::<Result<_>>()?,
    )
}
