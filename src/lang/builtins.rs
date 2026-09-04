use super::eval::{Evaluator, Unit, Value};
use crate::music::{self, Beat, Control, Note, Pattern, b, rational, real};
use anyhow::{Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
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
        self.take(n).map(|v| v.number()).unwrap_or(Ok(d))
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
fn record(values: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Record(values.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub fn note_value(n: &Note) -> Value {
    record([
        ("pitch", Value::num(n.pitch)),
        ("at", Value::beat(n.at)),
        ("duration", Value::beat(n.dur)),
        ("velocity", Value::num(n.velocity)),
        ("voice", Value::Str(n.voice.clone())),
        ("key", Value::Str(n.key.clone())),
        (
            "tags",
            Value::Array(n.tags.iter().cloned().map(Value::Str).collect()),
        ),
        (
            "data",
            Value::Record(
                n.data
                    .iter()
                    .map(|(k, v)| (k.clone(), json_value(v)))
                    .collect(),
            ),
        ),
    ])
}
fn json_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => Value::num(n.as_f64().unwrap()),
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Array(a) => Value::Array(a.iter().map(json_value).collect()),
        serde_json::Value::Object(o) => {
            Value::Record(o.iter().map(|(k, v)| (k.clone(), json_value(v))).collect())
        }
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
            _ => n.tags.contains(s) || n.voice == *s,
        }),
        Value::Array(a) => {
            for v in a {
                if selector(n, i, len, v)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Value::Record(r) => {
            let tag = r.get("tag").map(|v| v.text()).transpose()?;
            let voice = r.get("voice").map(|v| v.text()).transpose()?;
            let pitch = r.get("pitch").map(|v| v.number()).transpose()?;
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
pub fn call(e: &mut Evaluator, name: &str, args: Vec<(Option<String>, Value)>) -> Result<Value> {
    let mut a = Args::new(args);
    let name = name.strip_prefix("std.").unwrap_or(name);
    let result = match name {
        "midi" | "midi_tempos" => {
            let file = a.req("path")?.text()?.to_owned();
            let path = e
                .path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(file)
                .canonicalize()?;
            if !e.dependencies.contains(&path) {
                e.dependencies.push(path.clone());
            }
            let doc = crate::smf::read(&path)?;
            let track = a
                .take("track")
                .map(|v| v.number().map(|n| n as usize))
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
                                Value::Array(vec![
                                    Value::beat(b(tick as i64) / b(doc.division as i64)),
                                    Value::num(60_000_000. / n as f64),
                                ]),
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
            let options = a.take("options").unwrap_or(Value::Record(BTreeMap::new()));
            let mut opts = options.record()?.clone();
            let path = e
                .path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(file)
                .canonicalize()?;
            if !e.dependencies.contains(&path) {
                e.dependencies.push(path.clone());
            }
            let reader = hound::WavReader::open(&path)?;
            let length = reader.duration() as f64 / reader.spec().sample_rate as f64;
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
            note.data
                .insert("clock_start".into(), serde_json::json!(at));
            note.data.insert(
                "clock_duration".into(),
                serde_json::json!(duration - fade_out / 1000.),
            );
            note.data
                .insert("clock_span".into(), serde_json::json!(duration));
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
            Value::Record(opts)
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
                pitch.number()?
            };
            let dur = a.beat("duration", b(1))?;
            let at = a.beat("at", b(0))?;
            let mut n = Note::new(at, dur, pitch, "note".into());
            n.velocity = a.num("velocity", 0.72)?;
            let mut p = Pattern {
                span: at + dur,
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
            let mut p = Pattern::default();
            for (i, v) in ps.array()?.iter().enumerate() {
                let offset = if name == "seq" { p.span } else { b(0) };
                p.overlay(v.pattern()?.shifted(offset, &format!("{name}{i}")));
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
            let mut out = Pattern::default();
            for i in 0..count as usize {
                out.overlay(p.shifted(p.span * b(i as i64), &format!("repeat{i}")));
            }
            pat(out)
        }
        "at" | "place" => {
            let p = a.req("pattern")?;
            let at = a.req("at")?.beats()?;
            let key = a.txt("key", &format!("at{}", real(at)))?;
            pat(p.pattern()?.shifted(at, &key))
        }
        "slice" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            let start = a.req("from")?.beats()?;
            let end = a.req("to")?.beats()?;
            if end <= start {
                bail!("slice end must be after start");
            }
            p.notes.retain(|n| n.at + n.dur > start && n.at < end);
            for n in &mut p.notes {
                let off = n.at.max(start);
                n.dur = (n.at + n.dur).min(end) - off;
                n.at = off - start;
            }
            p.controls.retain(|c| c.at >= start && c.at < end);
            for c in &mut p.controls {
                c.at -= start;
            }
            p.raw.retain(|r| r.at >= start && r.at < end);
            for r in &mut p.raw {
                r.at -= start;
            }
            p.span = end - start;
            pat(p)
        }
        "stretch" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            let factor = a.req("factor")?.number()?;
            if factor <= 0.0 {
                bail!("stretch factor must be positive");
            }
            let factor = rational(factor)?;
            p.span *= factor;
            for n in &mut p.notes {
                n.at *= factor;
                n.dur *= factor;
            }
            for c in &mut p.controls {
                c.at *= factor;
            }
            for r in &mut p.raw {
                r.at *= factor;
            }
            pat(p)
        }
        "fit" => {
            let p = a.req("pattern")?;
            let p = p.pattern()?;
            let span = a.req("duration")?.beats()?;
            if p.span <= b(0) || span < b(0) {
                bail!("fit requires a nonempty pattern and nonnegative extent");
            }
            let repeats = (real(span) / real(p.span)).ceil() as usize;
            if repeats > 10000 || repeats.saturating_mul(p.notes.len()) > 200000 {
                bail!("fit exceeds event budget");
            }
            let mut out = Pattern::default();
            for i in 0..repeats {
                out.overlay(p.shifted(p.span * b(i as i64), &format!("fit{i}")));
            }
            out.notes.retain(|n| n.at < span);
            for n in &mut out.notes {
                n.dur = n.dur.min(span - n.at);
            }
            out.controls.retain(|c| c.at <= span);
            out.raw.retain(|r| r.at < span);
            out.span = span;
            pat(out)
        }
        "hands" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            let reach = a.num("reach", 12.)?;
            if !(1.0..=24.).contains(&reach) {
                bail!("hand reach must be 1..24 semitones");
            }
            crate::performance::hands(&mut p, reach);
            pat(p)
        }
        "transpose" | "gate" | "velocity" | "gain" | "hand" | "voice" | "reverse" | "invert"
        | "dynamics" | "humanize" | "swing" | "rubato" => {
            let mut p = a.req("pattern")?.pattern()?.clone();
            match name {
                "transpose" => {
                    let v = a.req("semitones")?.number()?;
                    for n in &mut p.notes {
                        n.pitch += v;
                    }
                }
                "gate" => {
                    let v = a.req("factor")?.number()?;
                    for n in &mut p.notes {
                        n.gate = v;
                    }
                }
                "velocity" => {
                    let v = a.req("value")?.number()?;
                    for n in &mut p.notes {
                        n.velocity = v;
                    }
                }
                "gain" => {
                    let v = a.req("factor")?.number()?;
                    for n in &mut p.notes {
                        n.velocity = (n.velocity * v).clamp(0.001, 1.0);
                    }
                }
                "hand" | "voice" => {
                    let v = a.req("name")?.text()?.to_owned();
                    for n in &mut p.notes {
                        if name == "hand" {
                            if v != "left" && v != "right" {
                                bail!("hand must be left or right");
                            }
                            n.hand = Some(v.clone());
                        } else {
                            n.voice = v.clone();
                        }
                    }
                }
                "reverse" => {
                    for n in &mut p.notes {
                        n.at = p.span - n.at - n.dur;
                    }
                    p.notes.sort_by_key(|n| n.at);
                }
                "invert" => {
                    let center = a.num("around", p.notes.first().map_or(60.0, |n| n.pitch))?;
                    for n in &mut p.notes {
                        n.pitch = 2.0 * center - n.pitch;
                    }
                }
                "dynamics" => {
                    let start = a.num("from", 0.45)?;
                    let end = a.num("to", 0.85)?;
                    for n in &mut p.notes {
                        n.velocity = (start + (end - start) * real(n.at) / real(p.span).max(0.001))
                            .clamp(0.001, 1.0);
                    }
                }
                "humanize" => {
                    let ms = a.take("timing").map(amount_ms).transpose()?.unwrap_or(4.0);
                    let amount = a.num("velocity", 0.025)?;
                    let seed = a.num("seed", 0.0)? as u64;
                    for n in &mut p.notes {
                        let h = hash(&n.key, seed);
                        let group = hash(&format!("{}:{}", n.voice, n.at), seed);
                        if !n.tags.contains("fixed") {
                            n.offset_ms += (0.85 * noise(group) + 0.15 * noise(h)) * ms;
                        }
                        n.velocity = (n.velocity
                            + (0.65 * noise(group.wrapping_add(13))
                                + 0.35 * noise(h.wrapping_add(13)))
                                * amount)
                            .clamp(0.01, 1.0);
                    }
                }
                "swing" => {
                    let ratio = a.num("ratio", 0.57)?;
                    let grid = a.beat("grid", music::duration("e")?)?;
                    if !(0.5..=0.8).contains(&ratio) {
                        bail!("swing ratio must be 0.5..0.8");
                    }
                    for n in &mut p.notes {
                        let x = real(n.at) / real(grid);
                        if (x - x.round()).abs() < 1e-6 && (x.round() as i64) % 2 == 1 {
                            n.at += rational((ratio * 2.0 - 1.0) * real(grid))?;
                        }
                    }
                }
                "rubato" => {
                    let ms = a.take("amount").map(amount_ms).transpose()?.unwrap_or(25.0);
                    let span = real(p.span);
                    if span <= 0.0 || ms.abs() * std::f64::consts::TAU / span >= 150.0 {
                        bail!(
                            "rubato must preserve forward time at all supported tempos; reduce the amount or lengthen the phrase"
                        );
                    }
                    let displacement = |beat: f64| {
                        -(beat.clamp(0.0, span) / span * std::f64::consts::TAU).sin() * ms
                    };
                    for n in &mut p.notes {
                        let a = displacement(real(n.at));
                        let b =
                            displacement(real(n.at + n.dur) * n.gate + real(n.at) * (1.0 - n.gate));
                        n.offset_ms += a;
                        n.release_offset_ms += b - a;
                    }
                    for c in &mut p.controls {
                        c.offset_ms += displacement(real(c.at));
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
                        for (k, v) in v.record()? {
                            if name == "refine" {
                                match k.as_str() {
                                    "pitch" => n.pitch = v.number()?,
                                    "velocity" => n.velocity = v.number()?,
                                    "gate" => n.gate = v.number()?,
                                    "hand" => n.hand = Some(v.text()?.into()),
                                    "voice" => n.voice = v.text()?.into(),
                                    "offset_ms" => n.offset_ms = v.number()?,
                                    _ => bail!("unknown note refinement '{k}'"),
                                }
                            } else {
                                n.data.insert(k.clone(), v.json());
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
            pat(p)
        }
        "chord" => {
            let symbol = a.req("symbol")?;
            let octave = a.num("octave", 3.0)? as i32;
            Value::Array(
                music::chord(symbol.text()?, octave)?
                    .into_iter()
                    .map(Value::num)
                    .collect(),
            )
        }
        "pitch" => Value::num(music::pitch(a.req("name")?.text()?)?),
        "chords" => {
            let symbols = a.req("symbols")?;
            let each = a.beat("each", b(4))?;
            let octave = a.num("octave", 3.0)? as i32;
            let mut p = Pattern::default();
            for (i, s) in symbols.text()?.split_whitespace().enumerate() {
                for (j, pitch) in music::chord(s, octave)?.iter().enumerate() {
                    let mut n =
                        Note::new(each * b(i as i64), each, *pitch, format!("chord{i}.{j}"));
                    n.tags.insert("harmony".into());
                    n.data.insert("chord".into(), serde_json::json!(s));
                    p.notes.push(n);
                }
                p.span += each;
            }
            pat(p)
        }
        "voicelead" => {
            let mut p = a.req("harmony")?.pattern()?.clone();
            let low = a.num("low", 48.0)?;
            let high = a.num("high", 84.0)?;
            let center = a.num("center", 64.0)?;
            if high - low < 12.0 {
                bail!("voice-leading register needs at least an octave");
            }
            p.notes
                .sort_by(|a, b| a.at.cmp(&b.at).then(a.pitch.total_cmp(&b.pitch)));
            let mut previous: Vec<f64> = vec![];
            let mut start = 0;
            while start < p.notes.len() {
                let at = p.notes[start].at;
                let end = start + p.notes[start..].iter().take_while(|n| n.at == at).count();
                let mut voiced = vec![];
                for (j, n) in p.notes[start..end].iter_mut().enumerate() {
                    let pc = n.pitch.rem_euclid(12.0);
                    let target = previous
                        .get(j)
                        .copied()
                        .unwrap_or(center + j as f64 * 3.0 - 3.0);
                    let pitch = (0..11)
                        .map(|o| pc + o as f64 * 12.0)
                        .filter(|x| {
                            *x >= low && *x <= high && voiced.last().is_none_or(|last| *x > *last)
                        })
                        .min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs()))
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "cannot voice chord at beat {} in requested register",
                                real(at)
                            )
                        })?;
                    n.pitch = pitch;
                    voiced.push(pitch);
                }
                previous = voiced;
                start = end;
            }
            pat(p)
        }
        "arpeggiate" => {
            let p = a.req("harmony")?;
            let p = p.pattern()?;
            let order = a.take("order").unwrap_or(Value::Array(vec![
                Value::integer(0),
                Value::integer(2),
                Value::integer(1),
                Value::integer(2),
            ]));
            let step = a.beat("step", music::duration("e")?)?;
            if step <= b(0) || order.array()?.is_empty() {
                bail!("arpeggiator needs positive step and nonempty order");
            }
            let mut starts: Vec<Beat> = p.notes.iter().map(|n| n.at).collect();
            starts.sort();
            starts.dedup();
            let mut out = Pattern {
                span: p.span,
                ..Default::default()
            };
            for (i, at) in starts.iter().enumerate() {
                let mut chord: Vec<_> = p.notes.iter().filter(|n| n.at == *at).cloned().collect();
                chord.sort_by(|a, b| a.pitch.total_cmp(&b.pitch));
                let end = starts.get(i + 1).copied().unwrap_or(p.span);
                let mut t = *at;
                let mut j = 0;
                while t < end {
                    if out.notes.len() > 200000 {
                        bail!("arpeggio exceeds event budget");
                    }
                    let k = order.array()?[j % order.array()?.len()].number()? as i64;
                    let index = k.rem_euclid(chord.len() as i64) as usize;
                    let mut n = chord[index].clone();
                    n.pitch += 12.0 * k.div_euclid(chord.len() as i64) as f64;
                    n.at = t;
                    n.dur = step.min(end - t);
                    n.key = format!("arp{i}.{j}");
                    out.notes.push(n);
                    t += step;
                    j += 1;
                }
            }
            pat(out)
        }
        "drums" => {
            let lanes = a.req("lanes")?;
            let span = a.beat("span", b(4))?;
            let mut p = Pattern {
                span,
                ..Default::default()
            };
            for (voice, grid) in lanes.record()? {
                let key = match voice.as_str() {
                    "kick" => 36,
                    "snare" => 38,
                    "rim" => 37,
                    "hat" | "closed_hat" => 42,
                    "open_hat" => 46,
                    "pedal_hat" => 44,
                    "crash" => 49,
                    "ride" => 51,
                    "tom_high" => 50,
                    "tom" => 47,
                    "tom_low" => 43,
                    _ => bail!("unknown kit voice '{voice}'"),
                };
                let chars: Vec<_> = grid
                    .text()?
                    .chars()
                    .filter(|c| !c.is_whitespace() && *c != '|')
                    .collect();
                if chars.is_empty() {
                    continue;
                }
                let step = span / b(chars.len() as i64);
                for (i, c) in chars.iter().enumerate() {
                    let velocity = match c {
                        'X' => 0.95,
                        'x' => 0.72,
                        'g' => 0.34,
                        'o' => 0.58,
                        '1'..='9' => c.to_digit(10).unwrap() as f64 / 10.0,
                        '.' | '-' | '_' => continue,
                        _ => bail!("invalid drum grid symbol '{c}'"),
                    };
                    let mut n = Note::new(
                        step * b(i as i64),
                        step * music::decimal("0.5")?,
                        key as f64,
                        format!("{voice}{i}"),
                    );
                    n.voice = voice.clone();
                    n.tags.insert(voice.clone());
                    n.velocity = velocity;
                    p.notes.push(n);
                }
            }
            pat(p)
        }
        "euclidean" => {
            let hits = a.req("hits")?.number()? as usize;
            let steps = a.req("steps")?.number()? as usize;
            let pitch = a.num("pitch", 42.0)?;
            let span = a.beat("span", b(4))?;
            let rotation = a.num("rotation", 0.0)? as usize;
            if steps == 0 || steps > 4096 || hits > steps {
                bail!("euclidean requires 0 <= hits <= steps <= 4096");
            }
            let mut p = Pattern {
                span,
                ..Default::default()
            };
            for i in 0..steps {
                if (i * hits) % steps < hits {
                    let at = (i + rotation) % steps;
                    p.notes.push(Note::new(
                        span * b(at as i64) / b(steps as i64),
                        span / b((steps * 2) as i64),
                        pitch,
                        format!("euclid{i}"),
                    ));
                }
            }
            pat(p)
        }
        "cc" => {
            let cc = a.req("controller")?.number()?;
            let value = a.req("value")?.number()?;
            let at = a.beat("at", b(0))?;
            if !(0.0..=127.0).contains(&cc) || !(0.0..=127.0).contains(&value) {
                bail!("MIDI controller and value must be 0..127");
            }
            pat(Pattern {
                span: at,
                controls: vec![Control {
                    offset_ms: 0.0,
                    at,
                    cc: cc as u8,
                    value: value as u8,
                }],
                ..Default::default()
            })
        }
        "pedal" => {
            let p = a.req("harmony")?;
            let depth = a.num("depth", 0.65)?;
            let controller = a.num("controller", 64.)?;
            let catch_ms = a.take("catch").map(amount_ms).transpose()?.unwrap_or(30.);
            let aware = a.take("aware").map(|v| v.truth()).unwrap_or(true);
            if ![64., 66., 67.].contains(&controller) || !(0.0..=500.).contains(&catch_ms) {
                bail!("pedal controller is 64/66/67; catch must be 0..500ms");
            }
            if !(0.0..=1.0).contains(&depth) {
                bail!("pedal depth must be 0..1");
            }
            let p = p.pattern()?;
            let mut ats: Vec<_> = p.notes.iter().map(|n| n.at).collect();
            ats.sort();
            ats.dedup();
            let mut out = Pattern {
                span: p.span,
                ..Default::default()
            };
            for at in ats {
                let chord = p.notes.iter().filter(|n| n.at == at).collect::<Vec<_>>();
                let mean = chord.iter().map(|n| n.pitch).sum::<f64>() / chord.len().max(1) as f64;
                let depth = if aware {
                    depth
                        * (1.
                            - (60. - mean).max(0.) * 0.007
                            - (chord.len().saturating_sub(3) as f64) * 0.04)
                            .clamp(0.4, 1.)
                } else {
                    depth
                };
                out.controls.push(Control {
                    offset_ms: 0.0,
                    at,
                    cc: controller as u8,
                    value: 0,
                });
                out.controls.push(Control {
                    offset_ms: catch_ms,
                    at,
                    cc: controller as u8,
                    value: (depth * 127.0).round() as u8,
                });
            }
            out.controls.push(Control {
                offset_ms: 0.0,
                at: p.span,
                cc: controller as u8,
                value: 0,
            });
            pat(out)
        }
        "range" => {
            let end = a.req("end")?.number()?;
            let start = a.num("start", 0.0)?;
            let step = a.num("step", 1.0)?;
            if step <= 0.0 || end < start || (end - start) / step > 200000.0 {
                bail!("range must be bounded, increasing, with positive step");
            }
            Value::Array(
                (0..((end - start) / step).ceil() as usize)
                    .map(|i| Value::num(start + i as f64 * step))
                    .collect(),
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
            Value::Array(out)
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
            Value::Bool(v.array()?.iter().any(|v| v.json() == needle.json()))
        }
        "merge" => {
            let mut r = a.req("base")?.record()?.clone();
            r.extend(a.req("overrides")?.record()?.clone());
            Value::Record(r)
        }
        "min" | "max" | "pow" => {
            let x = a.req("a")?.number()?;
            let y = a.req("b")?.number()?;
            Value::num(match name {
                "min" => x.min(y),
                "max" => x.max(y),
                _ => x.powf(y),
            })
        }
        "sin" | "cos" | "abs" | "floor" | "round" => {
            let x = a.req("x")?.number()?;
            Value::num(match name {
                "sin" => x.sin(),
                "cos" => x.cos(),
                "abs" => x.abs(),
                "floor" => x.floor(),
                _ => x.round(),
            })
        }
        "song" => {
            let mut r = a.req("settings")?.record()?.clone();
            r.insert("type".into(), Value::Str("song".into()));
            Value::Record(r)
        }
        "section" => {
            let name = a.req("name")?;
            let duration = a.req("duration")?;
            let options = a.take("options").unwrap_or(Value::Record(BTreeMap::new()));
            let mut r = options.record()?.clone();
            r.insert("name".into(), name);
            r.insert("duration".into(), duration);
            Value::Record(r)
        }
        "track" => {
            let name = a.req("name")?;
            let pattern = a.req("pattern")?;
            let instrument = a.req("instrument")?;
            let options = a.take("options").unwrap_or(Value::Record(BTreeMap::new()));
            let mut r = options.record()?.clone();
            r.insert("id".into(), name);
            r.insert("pattern".into(), pattern);
            r.insert("instrument".into(), instrument);
            Value::Record(r)
        }
        "synth" | "piano" | "kit" | "fx" | "plugin" | "sample" | "voice_patch" => {
            let name_value = if name == "piano" || name == "kit" {
                a.take("name").unwrap_or(Value::Str("default".into()))
            } else {
                a.req("name")?
            };
            let options = a.take("params").unwrap_or(Value::Record(BTreeMap::new()));
            let mut r = options.record()?.clone();
            if matches!(name, "sample" | "plugin" | "piano") {
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
            Value::Record(r)
        }
        "rack" => {
            let branches = a.req("branches")?;
            let options = a.take("options").unwrap_or(Value::Record(BTreeMap::new()));
            let mut r = options.record()?.clone();
            r.insert("type".into(), Value::Str("rack".into()));
            r.insert("branches".into(), branches);
            Value::Record(r)
        }
        "bus" => {
            let id = a.req("name")?;
            let chain = a.req("chain")?;
            let options = a.take("options").unwrap_or(Value::Record(BTreeMap::new()));
            let mut r = options.record()?.clone();
            r.insert("id".into(), id);
            r.insert("chain".into(), chain);
            Value::Record(r)
        }
        "curve" => {
            let points = a.req("points")?;
            let shape = a.take("shape").unwrap_or(Value::Str("linear".into()));
            record([("points", points), ("shape", shape)])
        }
        "lfo" => {
            let period = a.req("period")?;
            let duration = a.req("duration")?;
            let clock = matches!(&period,Value::Num(q) if q.unit==Unit::Seconds);
            let pos = |v: &Value| -> Result<f64> {
                if clock {
                    clock_seconds(v.clone())
                } else {
                    Ok(real(v.beats()?))
                }
            };
            let p = pos(&period)?;
            let duration = pos(&duration)?;
            let low = a.num("low", 0.)?;
            let high = a.num("high", 1.)?;
            let phase = a.num("phase", 0.)?;
            if p <= 0. || duration <= 0. || duration / p > 1500. {
                bail!("LFO needs positive period/duration and at most 1500 cycles");
            }
            let count = (duration / p * 64.).ceil().max(1.) as usize;
            let points = (0..=count)
                .map(|i| {
                    let t = duration * i as f64 / count as f64;
                    let at = Value::Num(super::eval::Quantity {
                        value: rational(t).unwrap(),
                        unit: if clock { Unit::Seconds } else { Unit::Beat },
                    });
                    Value::Array(vec![
                        at,
                        Value::num(
                            low + (high - low)
                                * (0.5 - 0.5 * (std::f64::consts::TAU * (t / p + phase)).cos()),
                        ),
                    ])
                })
                .collect();
            record([
                ("points", Value::Array(points)),
                ("shape", Value::Str("linear".into())),
            ])
        }
        "curve_at" => {
            let mut curve = a.req("curve")?.record()?.clone();
            let offset = a.req("offset")?;
            let points = curve
                .get("points")
                .ok_or_else(|| anyhow::anyhow!("curve needs points"))?
                .array()?;
            let clock = matches!(&offset,Value::Num(q) if q.unit==Unit::Seconds);
            let mut out = Vec::new();
            for p in points {
                let p = p.array()?;
                if p.len() != 2 {
                    bail!("curve points need position and value");
                }
                let at = if clock {
                    Value::Num(super::eval::Quantity {
                        value: rational(
                            clock_seconds(p[0].clone())? + clock_seconds(offset.clone())?,
                        )?,
                        unit: Unit::Seconds,
                    })
                } else {
                    Value::beat(p[0].beats()? + offset.beats()?)
                };
                out.push(Value::Array(vec![at, p[1].clone()]));
            }
            curve.insert("points".into(), Value::Array(out));
            Value::Record(curve)
        }
        "curve_map" | "curve_add" | "curve_mul" => {
            let first = a.req("curve")?;
            let second = if name == "curve_map" {
                a.req("function")?
            } else {
                a.req("other")?
            };
            let resolution = a.num("resolution", 1. / 64.)?;
            super::controls::combine(e, &first, &second, name, resolution)?
        }
        "automation" => {
            let target = a.req("target")?;
            let curve = a.req("curve")?;
            record([("target", target), ("curve", curve)])
        }
        "throws" => {
            let track = a.req("track")?;
            let tag = a.txt("tag", "echo")?;
            let target = a.req("target")?;
            let level = a.num("level", -12.0)?;
            let tail = a.take("tail").map(amount_ms).transpose()?.unwrap_or(120.0);
            record([
                ("track", track),
                ("tag", Value::Str(tag)),
                ("target", target),
                ("level", Value::num(level)),
                ("tail_ms", Value::num(tail)),
            ])
        }
        "raw_midi" => {
            let bytes = a
                .req("bytes")?
                .array()?
                .iter()
                .map(|v| {
                    let n = v.number()?;
                    if !(0.0..=255.0).contains(&n) || n.fract() != 0.0 {
                        bail!("MIDI bytes must be integers 0..255");
                    }
                    Ok(n as u8)
                })
                .collect::<Result<Vec<_>>>()?;
            let at = a.beat("at", b(0))?;
            pat(Pattern {
                span: at,
                raw: vec![music::RawEvent { at, bytes }],
                ..Default::default()
            })
        }
        "assert" => {
            if !a.req("condition")?.truth() {
                bail!("{}", a.txt("message", "assertion failed")?);
            }
            Value::Null
        }
        _ => bail!("unknown function '{name}'; see `muz help language`"),
    };
    a.done()?;
    if let Value::Invalid(message) = &result {
        bail!("{message}");
    }
    Ok(result)
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
