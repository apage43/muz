//! Lower language values into score/performance and a prepared audio graph description.
use crate::{
    lang::{self, Unit, Value},
    midi::*,
    model::{self, *},
    music::{self, Pattern, real},
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
pub const PPQ: u32 = 960_000;
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct Section {
    pub name: String,
    pub start: f64,
    pub end: f64,
    pub meter: [u8; 2],
}
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct CurvePoint {
    pub seconds: f64,
    pub value: f32,
}
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct Automation {
    pub target: String,
    pub points: Vec<CurvePoint>,
    pub shape: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct Extras {
    #[serde(default)]
    pub source: Option<PathBuf>,
    pub title: String,
    pub dependencies: Vec<PathBuf>,
    pub sections: Vec<Section>,
    pub automation: Vec<Automation>,
    pub tail: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub severity: String,
    pub code: String,
    pub track: String,
    pub beat: f64,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ScoreTrack {
    pub id: String,
    pub policy: String,
    pub pattern: Pattern,
}
#[derive(Clone, Debug, Serialize)]
pub struct Compiled {
    pub session: Session,
    pub score: Vec<ScoreTrack>,
    pub diagnostics: Vec<Diagnostic>,
}
fn num(r: &BTreeMap<String, Value>, k: &str, d: f64) -> Result<f64> {
    r.get(k).map(|v| v.number()).unwrap_or(Ok(d))
}
fn text(r: &BTreeMap<String, Value>, k: &str, d: &str) -> Result<String> {
    r.get(k)
        .map(|v| v.text().map(str::to_owned))
        .unwrap_or(Ok(d.into()))
}
fn list<'a>(r: &'a BTreeMap<String, Value>, k: &str) -> Result<&'a [Value]> {
    r.get(k).map(|v| v.array()).unwrap_or(Ok(&[]))
}
fn req<'a>(r: &'a BTreeMap<String, Value>, k: &str) -> Result<&'a Value> {
    r.get(k).ok_or_else(|| anyhow::anyhow!("missing '{k}'"))
}
pub fn compile(path: &Path) -> Result<Compiled> {
    let c = inspect(path)?;
    if c.diagnostics.iter().any(|d| d.severity == "error") {
        bail!(
            "playing policy failed: {}",
            c.diagnostics
                .iter()
                .filter(|d| d.severity == "error")
                .map(|d| format!("{} beat {}: {}", d.track, d.beat, d.message))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    Ok(c)
}
pub fn inspect(path: &Path) -> Result<Compiled> {
    let (v, deps) = lang::load(path)?;
    lower(v, path, deps)
}
pub fn lower(value: Value, path: &Path, dependencies: Vec<PathBuf>) -> Result<Compiled> {
    let r = value
        .record()
        .context("root must return song({...}) or bind let main = song({...})")?;
    if text(r, "type", "")? != "song" {
        bail!("root expression must be song({{...}})");
    }
    fields(
        r,
        &[
            "type",
            "title",
            "tempo",
            "meter",
            "tail",
            "sections",
            "tempos",
            "tracks",
            "master",
            "buses",
            "automation",
            "throws",
        ],
        "song",
    )?;
    let bpm = num(r, "tempo", 120.0)?;
    if !(20.0..=400.0).contains(&bpm) {
        bail!("tempo must be 20..400 BPM");
    }
    let meter = if let Some(v) = r.get("meter") {
        let vs = v.array()?;
        if vs.len() != 2 {
            bail!("meter needs numerator and denominator");
        }
        [vs[0].number()? as u8, vs[1].number()? as u8]
    } else {
        [4, 4]
    };
    if meter[0] == 0 || !meter[1].is_power_of_two() {
        bail!("invalid meter");
    }
    let mut extras = Extras {
        source: Some(path.canonicalize().unwrap_or_else(|_| path.to_owned())),
        title: text(
            r,
            "title",
            path.file_stem()
                .unwrap_or_default()
                .to_str()
                .unwrap_or("song"),
        )?,
        dependencies,
        tail: num(r, "tail", 4.0)?,
        ..Default::default()
    };
    let mut end = 0.0;
    for s in list(r, "sections")? {
        let sr = s.record()?;
        let name = text(sr, "name", "")?;
        if extras.sections.iter().any(|s| s.name == name) {
            bail!("duplicate section '{name}'");
        }
        let duration = req(sr, "duration")?;
        let duration = if let Value::Num(q) = duration {
            real(q.beats(meter[0] as f64 * 4.0 / meter[1] as f64)?)
        } else {
            bail!("section duration must be musical time")
        };
        if duration <= 0.0 {
            bail!("section '{name}' needs positive duration");
        }
        extras.sections.push(Section {
            name,
            start: end,
            end: end + duration,
            meter,
        });
        end += duration;
    }
    let mut tempos = vec![MidiTempo {
        tick: 0,
        micros_per_quarter: (60_000_000.0 / bpm).round() as u32,
        source_order: 0,
    }];
    for (i, v) in list(r, "tempos")?.iter().enumerate() {
        let row = v.array()?;
        if row.len() != 2 {
            bail!("tempo points are [beat, bpm]");
        }
        let beat = real(row[0].beats()?);
        let bpm = row[1].number()?;
        if beat < 0.0 || !(20.0..=400.0).contains(&bpm) {
            bail!("invalid tempo point");
        }
        tempos.push(MidiTempo {
            tick: tick(beat),
            micros_per_quarter: (60_000_000.0 / bpm).round() as u32,
            source_order: i as u32 + 1,
        });
    }
    tempos.sort_by_key(|t| t.tick);
    tempos.dedup_by(|later, earlier| {
        if later.tick == earlier.tick {
            *earlier = *later;
            true
        } else {
            false
        }
    });
    let mut score = vec![];
    let mut tracks = vec![];
    let mut diagnostics = vec![];
    let mut ids = BTreeSet::new();
    for tv in list(r, "tracks")? {
        let tr = tv.record()?;
        fields(
            tr,
            &[
                "id",
                "pattern",
                "instrument",
                "chain",
                "pan",
                "gain",
                "sends",
                "output",
                "policy",
                "reach",
                "movement",
                "fingering",
                "strict",
            ],
            "track",
        )?;
        let id = text(tr, "id", "")?;
        valid_id(&id)?;
        if !ids.insert(id.clone()) {
            bail!("duplicate track '{id}'");
        }
        let mut p = req(tr, "pattern")?.pattern()?.clone();
        for n in &mut p.notes {
            if let Some(at) = n.data.get("clock_start").and_then(|v| v.as_f64()) {
                let duration = n.data["clock_duration"].as_f64().unwrap();
                let span = n.data["clock_span"].as_f64().unwrap();
                let onset = beat_at_seconds(at, &tempos);
                n.at = music::rational(onset)?;
                n.dur = music::rational(beat_at_seconds(at + duration, &tempos) - onset)?;
                p.span = p
                    .span
                    .max(music::rational(beat_at_seconds(at + span, &tempos))?);
            }
        }

        p.validate()?;
        end = end.max(real(p.span));
        let iv = req(tr, "instrument")?;
        let ir = iv.record()?;
        let kind = text(ir, "type", "synth")?;
        let policy = text(tr, "policy", if kind == "piano" { "piano" } else { "free" })?;
        if policy == "piano" {
            if p.raw
                .iter()
                .any(|r| r.bytes.first().is_some_and(|b| matches!(*b >> 4, 8 | 9)))
            {
                diagnostics.push(Diagnostic{severity:if tr.get("strict").is_some_and(Value::truth){"error"}else{"warning"}.into(),code:"piano.raw".into(),track:id.clone(),beat:0.,message:"raw note messages bypass the piano allocation; use musical notes for checked piano writing".into()});
            }
            if !tr.get("fingering").is_some_and(|v| !v.truth()) {
                let times = p
                    .notes
                    .iter()
                    .map(|n| {
                        (
                            seconds_at(real(n.at), &tempos) + n.offset_ms / 1000.,
                            seconds_at(real(n.at) + real(n.dur) * n.gate, &tempos)
                                + (n.offset_ms + n.release_offset_ms) / 1000.,
                        )
                    })
                    .collect::<Vec<_>>();
                if let Err((beat, message)) =
                    crate::performance::fingers(&mut p, num(tr, "reach", 12.)?, &times)
                {
                    diagnostics.push(Diagnostic {
                        severity: if tr.get("strict").is_some_and(Value::truth) {
                            "error"
                        } else {
                            "warning"
                        }
                        .into(),
                        code: "piano.fingering".into(),
                        track: id.clone(),
                        beat,
                        message,
                    });
                }
            }
            crate::performance::hands(&mut p, num(tr, "reach", 12.)?);
            check_piano(&id, &p, &tempos, tr, &mut diagnostics)?;
        }
        score.push(ScoreTrack {
            id: id.clone(),
            policy,
            pattern: p.clone(),
        });
        if kind == "kit" {
            let voices: BTreeSet<String> = p.notes.iter().map(|n| n.voice.clone()).collect();
            for voice in voices {
                let mut part = p.clone();
                part.notes.retain(|n| n.voice == voice);
                part.controls.clear();
                let choke_groups = ir
                    .get("chokes")
                    .map(|v| v.array().map(|v| v.to_vec()))
                    .transpose()?
                    .unwrap_or_else(|| {
                        vec![Value::Array(
                            ["hat", "open_hat", "pedal_hat"]
                                .into_iter()
                                .map(|s| Value::Str(s.into()))
                                .collect(),
                        )]
                    });
                for group in &choke_groups {
                    let names = group
                        .array()?
                        .iter()
                        .map(Value::text)
                        .collect::<Result<Vec<_>>>()?;
                    if names.contains(&voice.as_str()) {
                        for hit in &p.notes {
                            if names.contains(&hit.voice.as_str()) {
                                part.controls.push(music::Control {
                                    at: hit.at,
                                    offset_ms: hit.offset_ms,
                                    cc: 120,
                                    value: 0,
                                });
                            }
                        }
                    }
                }
                let subid = format!("{id}.{voice}");
                let preset = match voice.as_str() {
                    "kick" => "kick",
                    "snare" | "rim" | "rimshot" => "snare",
                    "open_hat" | "crash" | "ride" => "crash",
                    "tom" | "tom_low" | "tom_high" => "tom",
                    _ => "hat",
                };
                let inst = Value::Record(BTreeMap::from([
                    ("type".into(), Value::Str("synth".into())),
                    ("name".into(), Value::Str(preset.into())),
                ]));
                let mut selected = ir.get(&voice).unwrap_or(&inst).clone();
                let mut voice_options = tr.clone();
                if let Value::Record(options) = &selected {
                    if let Some(instrument) = options.get("instrument") {
                        fields(
                            options,
                            &["instrument", "gain", "pan", "chain", "sends", "output"],
                            "kit voice",
                        )?;
                        for (key, value) in options {
                            if key != "instrument" {
                                voice_options.insert(key.clone(), value.clone());
                            }
                        }
                        voice_options.insert(
                            "gain".into(),
                            Value::num(num(tr, "gain", 0.)? + num(options, "gain", 0.)?),
                        );
                        let mut fx = list(options, "chain")?.to_vec();
                        fx.extend_from_slice(list(tr, "chain")?);
                        voice_options.insert("chain".into(), Value::Array(fx));
                        selected = instrument.clone();
                    }
                }
                if let Value::Record(r) = &mut selected {
                    if text(r, "type", "")? == "sample" {
                        r.entry("root".into()).or_insert_with(|| {
                            Value::num(part.notes.first().map_or(60., |n| n.pitch))
                        });
                        r.entry("one_shot".into()).or_insert(Value::Bool(true));
                    }
                }
                tracks.push(make_track(
                    &subid,
                    &voice_options,
                    &part,
                    &selected,
                    &tempos,
                    path,
                )?);
            }
        } else {
            tracks.push(make_track(&id, tr, &p, iv, &tempos, path)?);
        }
    }
    if tracks.is_empty() {
        bail!("song needs at least one track");
    }
    if tracks.len() > model::MAX_TRACKS {
        bail!(
            "{} tracks after kit expansion exceeds maximum {}",
            tracks.len(),
            model::MAX_TRACKS
        );
    }
    for t in &mut tracks {
        if let TrackSource::Midi(src) = &mut t.source {
            src.imported.summary.end_tick = tick(end);
            src.summary.end_tick = tick(end);
        }
    }
    let master = Bus {
        id: Id::new("master"),
        name: "Master".into(),
        inserts: chain(list(r, "master")?, "master", path)?,
        output: None,
        sends: vec![],
    };
    let mut buses = vec![];
    for bv in list(r, "buses")? {
        let br = bv.record()?;
        let id = text(br, "id", "")?;
        valid_id(&id)?;
        if id == "master" || !ids.insert(id.clone()) {
            bail!("duplicate bus/track '{id}'");
        }
        buses.push(Bus {
            id: Id::new(&id),
            name: id.clone(),
            inserts: chain(list(br, "chain")?, &id, path)?,
            output: Some(route(
                &id,
                &text(br, "output", "master")?,
                num(br, "gain", 0.0)?,
                "out",
            )),
            sends: sends(br, &id)?,
        });
    }
    for av in list(r, "automation")? {
        let ar = av.record()?;
        let cr = req(ar, "curve")?.record()?;
        if !matches!(
            text(cr, "shape", "linear")?.as_str(),
            "linear" | "smooth" | "step"
        ) {
            bail!("curve shape must be linear, smooth or step");
        }
        let mut points = vec![];
        for pv in list(cr, "points")? {
            let pair = pv.array()?;
            if pair.len() != 2 {
                bail!("automation points are [position, value]");
            }
            let seconds = match &pair[0] {
                Value::Num(q) if q.unit == Unit::Seconds => q.number(),
                v => seconds_at(real(v.beats()?), &tempos),
            };
            points.push(CurvePoint {
                seconds,
                value: pair[1].number()? as f32,
            });
        }
        if points
            .iter()
            .any(|p| !p.seconds.is_finite() || p.seconds < 0.0 || !p.value.is_finite())
            || points.is_empty()
            || points.windows(2).any(|p| p[0].seconds >= p[1].seconds)
        {
            bail!("automation points must be nonempty and strictly increasing");
        }
        extras.automation.push(Automation {
            target: text(ar, "target", "")?,
            points,
            shape: text(cr, "shape", "linear")?,
        });
    }
    for tv in list(r, "throws")? {
        let tr = tv.record()?;
        let track = text(tr, "track", "")?;
        let tag = text(tr, "tag", "echo")?;
        let target = text(tr, "target", "")?;
        let level = num(tr, "level", -12.0)? as f32;
        let tail = num(tr, "tail_ms", 120.0)? / 1000.0;
        let s = score
            .iter()
            .find(|s| s.id == track)
            .ok_or_else(|| anyhow::anyhow!("throw references unknown track '{track}'"))?;
        let mut intervals: Vec<_> = s
            .pattern
            .notes
            .iter()
            .filter(|n| n.tags.contains(&tag))
            .map(|n| {
                (
                    seconds_at(real(n.at), &tempos) + n.offset_ms / 1000.0,
                    seconds_at(
                        real(n.at + n.dur * music::rational(n.gate).unwrap()),
                        &tempos,
                    ) + (n.offset_ms + n.release_offset_ms) / 1000.0
                        + tail,
                )
            })
            .collect();
        if intervals.is_empty() {
            bail!("throw tag '{tag}' matched no notes in '{track}'");
        }
        intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = vec![];
        for (s, e) in intervals {
            if let Some(last) = merged.last_mut().filter(|v| v.1 >= s) {
                last.1 = last.1.max(e);
            } else {
                merged.push((s.max(0.0), e));
            }
        }
        let mut points = vec![CurvePoint {
            seconds: 0.0,
            value: -120.0,
        }];
        for (s, e) in merged {
            if s == 0.0 {
                points[0].value = level;
            } else {
                points.push(CurvePoint {
                    seconds: s,
                    value: level,
                });
            }
            points.push(CurvePoint {
                seconds: e,
                value: -120.0,
            });
        }
        extras.automation.push(Automation {
            target,
            points,
            shape: "step".into(),
        });
    }
    let mut session = Session {
        transport: Transport::OneShot {
            meter,
            meter_source: MeterSource::Declared,
        },
        master,
        buses,
        tracks,
        extras,
    };
    for d in session
        .tracks
        .iter_mut()
        .flat_map(|t| std::iter::once(&mut t.instrument).chain(&mut t.inserts))
        .chain(session.buses.iter_mut().flat_map(|b| &mut b.inserts))
        .chain(&mut session.master.inserts)
    {
        crate::assets::stamp(d)?;
        session.extras.dependencies.extend(crate::assets::paths(d));
    }
    session.extras.dependencies.sort();
    session.extras.dependencies.dedup();
    validate_graph(&session)?;
    Ok(Compiled {
        session,
        score,
        diagnostics,
    })
}
pub fn tick(beat: f64) -> u64 {
    (beat.max(0.0) * PPQ as f64).round() as u64
}
pub fn seconds_at(beat: f64, tempos: &[MidiTempo]) -> f64 {
    let mut seconds = 0.0;
    let mut at = 0.0;
    let mut bpm = 120.0;
    for t in tempos {
        let pos = t.tick as f64 / PPQ as f64;
        if pos > beat {
            break;
        }
        seconds += (pos - at) * 60.0 / bpm;
        at = pos;
        bpm = 60_000_000.0 / t.micros_per_quarter as f64;
    }
    seconds + (beat - at) * 60.0 / bpm
}
fn make_track(
    id: &str,
    tr: &BTreeMap<String, Value>,
    p: &Pattern,
    iv: &Value,
    tempos: &[MidiTempo],
    path: &Path,
) -> Result<Track> {
    let mut imported = ImportedMidi {
        tempos: tempos.to_vec(),
        ..Default::default()
    };
    for (i, n) in p.notes.iter().enumerate() {
        let onset_seconds = seconds_at(real(n.at), tempos) + n.offset_ms / 1000.0;
        let release_seconds = seconds_at(real(n.at) + real(n.dur) * n.gate, tempos)
            + (n.offset_ms + n.release_offset_ms) / 1000.0;
        if release_seconds <= onset_seconds.max(0.0) {
            bail!("note {} has a nonpositive performed duration", n.key);
        }
        if n.data.get("channel").is_some_and(|v| {
            v.as_f64()
                .is_none_or(|n| n.fract() != 0. || !(0.0..=15.).contains(&n))
        }) {
            bail!("note {} has an invalid MIDI channel; use 0..15", n.key);
        }
        let onset = tick(beat_at_seconds(onset_seconds, tempos));
        let release = tick(beat_at_seconds(release_seconds, tempos));
        let expression = crate::expression::Program::parse(n.data.get("expression"))?;
        if expression.len > 0
            && !(text(iv.record()?, "type", "")? == "voice_patch"
                || text(iv.record()?, "name", "")?.ends_with(".clap")
                || text(iv.record()?, "path", "")?.ends_with(".clap"))
        {
            bail!(
                "track {id}: per-note expression requires voice_patch or CLAP; split selected notes into a separate track for channel-wide effects"
            );
        }
        imported.notes.push(MidiNote {
            performance: Some(crate::expression::Performance {
                pitch: n.pitch,
                velocity: n.velocity,
                expression,
            }),
            id: n.key.clone(),
            tags: n.tags.iter().cloned().collect(),
            annotations: n.data.clone(),
            start_tick: onset,
            duration_ticks: release.saturating_sub(onset).max(1),
            channel: n.data.get("channel").and_then(|v| v.as_f64()).unwrap_or(0.) as u8,
            key: n.pitch.round() as u8,
            attack_velocity: (n.velocity * 127.0).round().clamp(1.0, 127.0) as u8,
            release_velocity: (n.release * 127.0).round() as u8,
            source_order: (i * 2) as u32,
            end_source_order: (i * 2 + 1) as u32,
        });
    }
    imported
        .notes
        .sort_by_key(|n| (n.start_tick, n.source_order));
    for (i, c) in p.controls.iter().enumerate() {
        imported.controllers.push(MidiController {
            tick: tick(beat_at_seconds(
                seconds_at(real(c.at), tempos) + c.offset_ms / 1000.0,
                tempos,
            )),
            channel: 0,
            controller: c.cc,
            value: c.value,
            source_order: i as u32,
        });
    }
    let mut raw_order = p.raw.iter().enumerate().collect::<Vec<_>>();
    raw_order.sort_by(|(_, a), (_, b)| {
        (seconds_at(real(a.at), tempos) + a.offset_ms / 1000.)
            .total_cmp(&(seconds_at(real(b.at), tempos) + b.offset_ms / 1000.))
    });
    let mut held = BTreeMap::<(u8, u8), (u64, u8, usize)>::new();
    for (i, raw) in raw_order {
        let at = tick(beat_at_seconds(
            seconds_at(real(raw.at), tempos) + raw.offset_ms / 1000.,
            tempos,
        ));
        let bytes = &raw.bytes;
        if bytes.is_empty()
            || !(0x80..=0xef).contains(&bytes[0])
            || bytes.len()
                != if matches!(bytes[0] >> 4, 12 | 13) {
                    2
                } else {
                    3
                }
            || bytes[1..].iter().any(|b| *b > 127)
        {
            bail!(
                "track {id}: device adapter cannot consume this raw message; structural SMF still preserves it"
            );
        }
        if matches!(bytes[0] >> 4, 8 | 9) {
            let channel = bytes[0] & 15;
            let key = bytes[1];
            if bytes[0] >> 4 == 9 && bytes[2] > 0 {
                if held.insert((channel, key), (at, bytes[2], i)).is_some() {
                    bail!(
                        "track {id}: ambiguous overlapping raw note-ons on channel {channel}, key {key}; release first or use separate channels"
                    )
                }
            } else {
                let (start, velocity, order) = held.remove(&(channel, key)).ok_or_else(|| {
                    anyhow::anyhow!("track {id}: raw note-off has no matching note-on")
                })?;
                if at <= start {
                    bail!("raw note needs positive duration")
                }
                imported.notes.push(MidiNote {
                    performance: None,
                    id: format!("raw{order}"),
                    tags: vec!["raw".into()],
                    annotations: Default::default(),
                    start_tick: start,
                    duration_ticks: at - start,
                    channel,
                    key,
                    attack_velocity: velocity,
                    release_velocity: if bytes[0] >> 4 == 8 { bytes[2] } else { 0 },
                    source_order: (p.notes.len() * 2 + order) as u32,
                    end_source_order: (p.notes.len() * 2 + i) as u32,
                });
            }
            continue;
        }
        if bytes[0] >> 4 == 11 {
            imported.controllers.push(MidiController {
                tick: at,
                channel: bytes[0] & 15,
                controller: bytes[1],
                value: bytes[2],
                source_order: i as u32,
            });
        } else {
            if !matches!(text(iv.record()?, "type", "")?.as_str(), "piano" | "plugin") {
                bail!(
                    "track {id}: native device does not consume raw channel messages; use a capable plugin or notes_only()"
                );
            }
            let mut packed = [0; 3];
            packed[..bytes.len()].copy_from_slice(bytes);
            imported.messages.push(crate::midi::ChannelMessage {
                tick: at,
                bytes: packed,
                len: bytes.len() as u8,
                source_order: i as u32,
            });
        }
    }
    if !held.is_empty() {
        bail!("track {id}: raw note-on has no release")
    }
    imported
        .notes
        .sort_by_key(|n| (n.start_tick, n.source_order));
    let mut owners = BTreeMap::new();
    for c in &imported.controllers {
        if owners
            .insert((c.tick, c.channel, c.controller), c.value)
            .is_some_and(|v| v != c.value)
        {
            bail!(
                "track {id}: conflicting CC {} values on channel {} at beat {}",
                c.controller,
                c.channel,
                c.tick as f64 / PPQ as f64
            )
        }
    }
    imported.messages.sort_by_key(|m| (m.tick, m.source_order));
    imported
        .controllers
        .sort_by_key(|c| (c.tick, c.source_order));
    imported.summary = MidiSummary {
        ppq: PPQ,
        end_tick: tick(real(p.span)),
        notes: imported.notes.len() as u32,
        controllers: imported.controllers.len() as u32,
        tempos: tempos.len() as u32,
        events: (imported.notes.len() * 2 + imported.controllers.len() + tempos.len()) as u32,
        ..Default::default()
    };
    let mut inserts = chain(list(tr, "chain")?, id, path)?;
    if let Some(pan) = tr.get("pan") {
        inserts.push(Device {
            asset_versions: Vec::new(),
            patch: None,
            generation: 0,
            rack: None,
            sample: None,
            sidechain: None,
            id: Id::new(format!("{id}.pan")),
            kind: DeviceKind::Stereo,
            params: BTreeMap::from([("pan".into(), pan.number()? as f32)]),
            vst3: None,
        });
    }
    Ok(Track {
        id: Id::new(id),
        name: id.into(),
        source: TrackSource::Midi(MidiTrackSource {
            all_channels: true,
            id: Id::new(format!("{id}.events")),
            asset: format!("@source/{id}"),
            channel: 0,
            summary: imported.summary.clone(),
            imported,
        }),
        instrument: device(iv, &format!("{id}.instrument"), path)?,
        inserts,
        output: route(
            id,
            &text(tr, "output", "master")?,
            num(tr, "gain", 0.0)?,
            "out",
        ),
        sends: sends(tr, id)?,
    })
}
fn route(id: &str, to: &str, gain: f64, key: &str) -> Route {
    Route {
        pre: true,
        id: Id::new(format!("{id}.{key}")),
        to: Id::new(to),
        gain_db: gain as f32,
    }
}
fn sends(r: &BTreeMap<String, Value>, id: &str) -> Result<Vec<Route>> {
    let mut out = vec![];
    if let Some(v) = r.get("sends") {
        for (k, v) in v.record()? {
            let (gain, pre) = if let Value::Record(r) = v {
                fields(r, &["gain", "pre"], "send")?;
                (
                    num(r, "gain", -12.)?,
                    r.get("pre").is_some_and(|v| matches!(v, Value::Bool(true))),
                )
            } else {
                (v.number()?, false)
            };
            let mut send = route(id, k, gain, &format!("send.{k}"));
            send.pre = pre;
            out.push(send);
        }
    }
    Ok(out)
}
fn chain(vs: &[Value], id: &str, path: &Path) -> Result<Vec<Device>> {
    vs.iter()
        .enumerate()
        .map(|(i, v)| {
            let r = v.record()?;
            let key = text(r, "id", &format!("fx{i}"))?;
            device(v, &format!("{id}.{key}"), path)
        })
        .collect()
}
pub fn device(v: &Value, id: &str, path: &Path) -> Result<Device> {
    let r = v.record()?;
    let ty = text(r, "type", "synth")?;
    let name = if ty == "sample" {
        String::new()
    } else {
        text(r, "name", "")?
    };
    let mut params = BTreeMap::new();
    let kind = if ty == "voice_patch" {
        DeviceKind::VoicePatch
    } else if ty == "rack" {
        DeviceKind::Rack
    } else if ty == "sample" {
        DeviceKind::Sampler
    } else if ty == "fx" {
        match name.as_str() {
            "eq" => DeviceKind::Eq,
            "chorus" => DeviceKind::Chorus,
            "gate" => DeviceKind::Gate,
            "gain" => DeviceKind::Gain,
            "lowpass" => DeviceKind::Lowpass,
            "highpass" => DeviceKind::Highpass,
            "compressor" => DeviceKind::Compressor,
            "limiter" => DeviceKind::Limiter,
            "delay" => DeviceKind::Delay,
            "reverb" => DeviceKind::Reverb,
            "stereo" => DeviceKind::Stereo,
            "drive" => DeviceKind::Drive,
            _ => bail!("unknown effect '{name}'"),
        }
    } else if ty == "piano" || ty == "plugin" {
        if text(r, "path", &name)?.ends_with(".clap") {
            DeviceKind::Clap
        } else {
            DeviceKind::Vst3
        }
    } else {
        params = preset(&name)?;
        DeviceKind::StudioSynth
    };
    for (k, v) in r {
        if ![
            "type",
            "name",
            "id",
            "path",
            "class",
            "version",
            "state",
            "_module_dir",
            "sidechain",
            "root",
            "keys",
            "velocity",
            "offset",
            "loop",
            "one_shot",
            "branches",
            "expose",
            "modulate",
            "nodes",
            "output",
        ]
        .contains(&k.as_str())
        {
            let mut n = v.number()? as f32;
            if let Value::Num(q) = v {
                if q.unit == Unit::Seconds && k.ends_with("_ms") {
                    n *= 1000.0;
                }
            }
            params.insert(k.clone(), n);
        }
    }
    let resource_root = r
        .get("_module_dir")
        .map(|v| v.text().map(PathBuf::from))
        .transpose()?
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_owned());
    let vst3 = if matches!(kind, DeviceKind::Vst3 | DeviceKind::Clap) {
        let bundle = if ty == "piano" {
            text(
                r,
                "path",
                &format!(
                    "{}/Documents/Pianoteq 9/x86-64bit/Pianoteq 9.vst3",
                    std::env::var("HOME")?
                ),
            )?
        } else {
            text(r, "path", &name)?
        };
        let bundle = PathBuf::from(bundle);
        let bundle = if bundle.is_absolute() {
            bundle
        } else {
            resource_root.join(bundle)
        };
        Some(Vst3Config {
            state: r
                .get("state")
                .map(|v| {
                    v.text()
                        .map(|s| resource_root.join(s).display().to_string())
                })
                .transpose()?,
            bundle_env: bundle.display().to_string(),
            class_id: text(
                r,
                "class",
                if ty == "piano" {
                    "565354507439717069616E6F74657120"
                } else {
                    ""
                },
            )?,
            expected_version: text(r, "version", "")?,
        })
    } else {
        None
    };
    Ok(Device {
        asset_versions: Vec::new(),
        patch: if ty == "voice_patch" {
            Some(v.json())
        } else {
            None
        },
        generation: 0,
        rack: if ty == "rack" {
            let branches = list(r, "branches")?
                .iter()
                .enumerate()
                .map(|(i, v)| chain(v.array()?, &format!("{id}.{i}"), path))
                .collect::<Result<Vec<_>>>()?;
            if branches.is_empty()
                || branches.len() > 8
                || branches.iter().map(Vec::len).sum::<usize>() > 32
            {
                bail!("rack needs 1..8 branches and at most 32 devices");
            }
            if branches.iter().flatten().any(|d| {
                d.kind == DeviceKind::Rack
                    || (d.kind.is_instrument()
                        && !matches!(d.kind, DeviceKind::Vst3 | DeviceKind::Clap))
                    || d.sidechain.is_some()
            }) {
                bail!(
                    "rack branches contain effects; attach sidechain to the rack; nested racks are not supported"
                );
            }
            Some(Rack {
                branches,
                expose: r
                    .get("expose")
                    .map(|v| serde_json::from_value(v.json()))
                    .transpose()?
                    .unwrap_or_default(),
                modulate: r
                    .get("modulate")
                    .map(|v| serde_json::from_value(v.json()))
                    .transpose()?
                    .unwrap_or_default(),
            })
        } else {
            None
        },
        sample: if kind == DeviceKind::Sampler {
            Some(sample_zones(r, path)?)
        } else {
            None
        },
        sidechain: r
            .get("sidechain")
            .map(|v| v.text().map(str::to_owned))
            .transpose()?,
        id: Id::new(id),
        kind,
        params,
        vst3,
    })
}
pub fn preset(name: &str) -> Result<BTreeMap<String, f32>> {
    let pairs: &[(&str, f32)] = match name {
        "init" => &[],
        "pulse-bass" => &[
            ("mode", 1.0),
            ("cutoff_hz", 800.0),
            ("filter_env", 2.5),
            ("attack_ms", 2.0),
            ("decay_ms", 120.0),
            ("sustain", 0.45),
            ("release_ms", 70.0),
            ("sub", 0.45),
            ("drive_db", 3.0),
            ("gain_db", -13.0),
        ],
        "glass-lead" => &[
            ("mode", 0.0),
            ("unison", 3.0),
            ("detune_cents", 8.0),
            ("attack_ms", 8.0),
            ("decay_ms", 170.0),
            ("sustain", 0.6),
            ("release_ms", 180.0),
            ("cutoff_hz", 3800.0),
            ("filter_env", 1.0),
            ("vibrato_cents", 9.0),
            ("gain_db", -18.0),
        ],
        "pad" | "choir" => &[
            ("mode", 0.0),
            ("unison", 5.0),
            ("detune_cents", 14.0),
            ("attack_ms", 650.0),
            ("release_ms", 1800.0),
            ("sustain", 0.7),
            ("cutoff_hz", 1800.0),
            ("filter_env", 0.4),
            ("gain_db", -23.0),
        ],
        "bell" => &[
            ("mode", 2.0),
            ("fm_ratio", 2.0),
            ("fm_index", 2.5),
            ("attack_ms", 2.0),
            ("decay_ms", 450.0),
            ("sustain", 0.1),
            ("release_ms", 850.0),
            ("gain_db", -18.0),
            ("cutoff_hz", 9000.0),
        ],
        "kick" => &[("mode", 3.0), ("decay_ms", 150.0), ("gain_db", -7.0)],
        "snare" => &[
            ("mode", 4.0),
            ("decay_ms", 125.0),
            ("drive_db", 4.0),
            ("gain_db", -13.0),
        ],
        "hat" => &[("mode", 5.0), ("decay_ms", 32.0), ("gain_db", -25.0)],
        "crash" => &[("mode", 5.0), ("decay_ms", 850.0), ("gain_db", -23.0)],
        "tom" => &[
            ("mode", 6.0),
            ("decay_ms", 190.0),
            ("fm_ratio", 1.5),
            ("fm_index", 1.0),
            ("gain_db", -13.0),
        ],
        "noise" => &[
            ("mode", 7.0),
            ("attack_ms", 700.0),
            ("release_ms", 300.0),
            ("gain_db", -27.0),
            ("cutoff_hz", 3000.0),
        ],
        "fifths" => &[
            ("mode", 1.0),
            ("unison", 3.0),
            ("detune_cents", 5.0),
            ("attack_ms", 3.0),
            ("release_ms", 90.0),
            ("cutoff_hz", 2300.0),
            ("filter_env", 0.5),
            ("drive_db", 10.0),
            ("gain_db", -20.0),
        ],
        _ => bail!("unknown synth preset '{name}'"),
    };
    Ok(pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect())
}
fn valid_id(s: &str) -> Result<()> {
    if s.is_empty()
        || s.len() > 96
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    {
        bail!("invalid name '{s}': use letters, digits, _, . or -");
    }
    Ok(())
}
fn validate_graph(s: &Session) -> Result<()> {
    let buses: BTreeSet<_> = std::iter::once(&s.master.id)
        .chain(s.buses.iter().map(|b| &b.id))
        .collect();
    let mut done = BTreeSet::from([s.master.id.clone()]);
    loop {
        let before = done.len();
        for b in &s.buses {
            if b.output
                .iter()
                .chain(&b.sends)
                .all(|r| done.contains(&r.to))
            {
                done.insert(b.id.clone());
            }
        }
        if done.len() == buses.len() {
            break;
        }
        if before == done.len() {
            bail!("bus routing cycle or missing target");
        }
    }
    for route in s
        .tracks
        .iter()
        .flat_map(|t| std::iter::once(&t.output).chain(&t.sends))
        .chain(s.buses.iter().flat_map(|b| b.output.iter().chain(&b.sends)))
    {
        if !buses.contains(&route.to) {
            bail!("route {} targets missing bus {}", route.id, route.to);
        }
        if !route.gain_db.is_finite() || !(-120.0..=24.0).contains(&route.gain_db) {
            bail!("route gain must be -120..24 dB");
        }
    }
    let mut device_ids = BTreeSet::new();
    for d in s
        .tracks
        .iter()
        .flat_map(|t| std::iter::once(&t.instrument).chain(&t.inserts))
        .chain(s.buses.iter().flat_map(|b| &b.inserts))
        .chain(&s.master.inserts)
    {
        if !device_ids.insert(&d.id) {
            bail!("duplicate device {}", d.id);
        }
        for (k, v) in &d.params {
            if matches!(
                d.kind,
                DeviceKind::Vst3 | DeviceKind::Clap | DeviceKind::Rack | DeviceKind::VoicePatch
            ) {
                continue;
            }
            let spec = crate::source::parameter_specs(d.kind)
                .iter()
                .find(|s| s.name == k)
                .ok_or_else(|| anyhow::anyhow!("{}: unknown parameter '{k}'", d.id))?;
            if !v.is_finite() || *v < spec.min || *v > spec.max {
                bail!("{}.{} must be {}..{}", d.id, k, spec.min, spec.max);
            }
        }
    }
    Ok(())
}
fn check_piano(
    id: &str,
    p: &Pattern,
    tempos: &[MidiTempo],
    opts: &BTreeMap<String, Value>,
    out: &mut Vec<Diagnostic>,
) -> Result<()> {
    let reach = num(opts, "reach", 12.0)?;
    let movement = num(opts, "movement", 48.0)?;
    let mut notes = p.notes.clone();
    notes.sort_by(|a, b| {
        (seconds_at(real(a.at), tempos) + a.offset_ms / 1000.0)
            .total_cmp(&(seconds_at(real(b.at), tempos) + b.offset_ms / 1000.0))
    });
    let mut last: [Option<(f64, f64)>; 2] = [None, None];
    let mut keys = BTreeSet::new();
    for n in &notes {
        let time = seconds_at(real(n.at), tempos) + n.offset_ms / 1000.0;
        let active: Vec<_> = notes
            .iter()
            .filter(|m| {
                m.hand == n.hand
                    && (seconds_at(real(m.at), tempos) + m.offset_ms / 1000.0) <= time + 1e-8
                    && (seconds_at(real(m.at) + real(m.dur) * m.gate, tempos)
                        + (m.offset_ms + m.release_offset_ms) / 1000.0)
                        > time + 1e-8
            })
            .collect();
        let pitches: BTreeSet<i32> = active.iter().map(|n| n.pitch.round() as i32).collect();
        let mut messages = vec![];
        if !(21.0..=108.0).contains(&n.pitch) {
            messages.push(("piano.range", "note outside acoustic piano range".into()));
        }
        if pitches.len() > 5 {
            messages.push((
                "piano.capacity",
                format!(
                    "{} distinct keys held by {} hand",
                    pitches.len(),
                    n.hand.as_deref().unwrap()
                ),
            ));
        }
        if let (Some(lo), Some(hi)) = (pitches.first(), pitches.last()) {
            if (hi - lo) as f64 > reach {
                messages.push((
                    "piano.reach",
                    format!("held span {} exceeds configured reach {reach}", hi - lo),
                ));
            }
        }
        if notes
            .iter()
            .filter(|m| {
                m.pitch.round() == n.pitch.round()
                    && seconds_at(real(m.at), tempos) + m.offset_ms / 1000. <= time + 1e-8
                    && seconds_at(real(m.at) + real(m.dur) * m.gate, tempos)
                        + (m.offset_ms + m.release_offset_ms) / 1000.
                        > time + 1e-8
            })
            .count()
            > 1
        {
            messages.push((
                "piano.retrigger",
                "same key has overlapping depressions".into(),
            ));
        }
        let h = usize::from(n.hand.as_deref() == Some("right"));
        if let Some((at, pitch)) = last[h] {
            if time > at + 1e-6 && (n.pitch - pitch).abs() > reach + movement * (time - at) {
                messages.push((
                    "piano.movement",
                    "hand movement exceeds configured rate".into(),
                ));
            }
        }
        last[h] = Some((time, n.pitch));
        for (code, message) in messages {
            if keys.insert((code, n.at)) {
                out.push(Diagnostic {
                    severity: if opts.get("strict").is_some_and(Value::truth) {
                        "error"
                    } else {
                        "warning"
                    }
                    .into(),
                    code: code.into(),
                    track: id.into(),
                    beat: real(n.at),
                    message,
                });
            }
        }
    }
    Ok(())
}

fn fields(r: &BTreeMap<String, Value>, allowed: &[&str], context: &str) -> Result<()> {
    for k in r.keys() {
        if !allowed.contains(&k.as_str()) {
            bail!(
                "unknown {context} field '{k}'; available: {}",
                allowed.join(", ")
            );
        }
    }
    Ok(())
}

fn sample_zones(r: &BTreeMap<String, Value>, path: &Path) -> Result<Vec<model::SampleZone>> {
    let resource_root = r
        .get("_module_dir")
        .map(|v| v.text().map(PathBuf::from))
        .transpose()?
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_owned());
    let sources = match req(r, "name")? {
        Value::Array(xs) => xs.clone(),
        v => vec![v.clone()],
    };
    if sources.is_empty() || sources.len() > 128 {
        bail!("sample needs 1..128 zones");
    }
    sources
        .iter()
        .map(|v| {
            let mut options = r.clone();
            let file = if let Value::Record(zone) = v {
                options.extend(zone.clone());
                text(zone, "path", "")?
            } else {
                v.text()?.to_owned()
            };
            let pair = |key: &str, default: [f64; 2]| -> Result<[f64; 2]> {
                if let Some(v) = options.get(key) {
                    let vs = v.array()?;
                    if vs.len() != 2 {
                        bail!("{key} needs two values");
                    }
                    Ok([vs[0].number()?, vs[1].number()?])
                } else {
                    Ok(default)
                }
            };
            let keys = pair("keys", [0., 127.])?;
            let vel = pair("velocity", [0., 1.])?;
            let root = num(&options, "root", 60.)?;
            let offset = num(&options, "offset", 0.)?;
            if keys[0] < 0.
                || keys[1] > 127.
                || keys[1] < keys[0]
                || vel[0] < 0.
                || vel[1] > 1.
                || vel[1] < vel[0]
                || root < 0.
                || root > 127.
                || offset < 0.
            {
                bail!("invalid sample zone range");
            }
            Ok(model::SampleZone {
                path: resource_root.join(file).display().to_string(),
                root: root as u8,
                keys: [keys[0] as u8, keys[1] as u8],
                velocity: [vel[0] as f32, vel[1] as f32],
                offset_seconds: offset,
                loop_seconds: options
                    .get("loop")
                    .map(|_| pair("loop", [0., 0.]))
                    .transpose()?,
                one_shot: options.get("one_shot").is_some_and(Value::truth),
            })
        })
        .collect()
}

pub fn beat_at_seconds(seconds: f64, tempos: &[MidiTempo]) -> f64 {
    let target = seconds.max(0.0);
    let mut at = 0.0;
    let mut elapsed = 0.0;
    let mut bpm = 120.0;
    for t in tempos {
        let beat = t.tick as f64 / PPQ as f64;
        let next = elapsed + (beat - at) * 60.0 / bpm;
        if next > target {
            break;
        }
        at = beat;
        elapsed = next;
        bpm = 60_000_000. / t.micros_per_quarter as f64;
    }
    at + (target - elapsed) * bpm / 60.0
}
