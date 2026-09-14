//! Lower language values into score/performance and a prepared audio graph description.
use crate::{
    lang::{self, Origin, Record, Unit, Value},
    midi::*,
    model::{self, *},
    music::{self, Pattern, real},
};
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
pub const PPQ: u32 = 960_000;
pub use crate::description::{
    Automation, CurvePoint, Extras, Section, TrackGroup, TrackGroupMember,
};
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
    pub locations: BTreeMap<String, lang::Location>,
}
/// Built-in effect names accepted by `fx(...)`, listed for typo hints.
const EFFECTS: &[&str] = &[
    "eq",
    "bitcrusher",
    "chorus",
    "gate",
    "gain",
    "lowpass",
    "highpass",
    "compressor",
    "limiter",
    "delay",
    "reverb",
    "stereo",
    "drive",
];
/// Where each declared id came from, so validation that runs on the compiled
/// model can still name a source line.
#[derive(Default)]
struct Origins(BTreeMap<String, Origin>);
impl Origins {
    fn record(&mut self, kind: &str, id: &str, r: &Record) {
        if let Some(origin) = r.origin() {
            self.0.insert(format!("{kind}.{id}"), origin.clone());
        }
    }
    fn get(&self, kind: &str, id: &str) -> Option<&Origin> {
        self.0.get(&format!("{kind}.{id}"))
    }
}
/// Report a failing field: which field, which record, and what it wanted.
fn field(r: &Record, key: &str, error: anyhow::Error) -> anyhow::Error {
    lang::Diagnostic::new(format!("'{key}' {error:#}"))
        .origin(r.origin())
        .err()
}
fn num(r: &Record, k: &str, d: f64) -> Result<f64> {
    match r.get(k) {
        Some(v) => v.field_number(k).map_err(|error| field(r, k, error)),
        None => Ok(d),
    }
}
fn text(r: &Record, k: &str, d: &str) -> Result<String> {
    match r.get(k) {
        Some(v) => v
            .text()
            .map(str::to_owned)
            .map_err(|error| field(r, k, error)),
        None => Ok(d.into()),
    }
}
fn list<'a>(r: &'a Record, k: &str) -> Result<&'a [Value]> {
    match r.get(k) {
        Some(v) => v.array().map_err(|error| field(r, k, error)),
        None => Ok(&[]),
    }
}
fn req<'a>(r: &'a Record, k: &str) -> Result<&'a Value> {
    r.get(k).ok_or_else(|| {
        let known: Vec<&str> = r.keys().map(String::as_str).collect();
        lang::Diagnostic::new(format!("missing '{k}'"))
            .helps(lang::suggest_vocabulary("fields", k, known))
            .origin(r.origin())
            .err()
    })
}
pub fn compile(path: &Path) -> Result<Compiled> {
    validate_compiled(inspect(path)?, path)
}
pub fn compile_with_loader(
    path: &Path,
    loader: std::rc::Rc<dyn lang::SourceLoader>,
) -> Result<Compiled> {
    let (value, dependencies) = lang::load_with_loader(path, loader)?;
    validate_compiled(lower(value, path, dependencies)?, path)
}
fn validate_compiled(c: Compiled, path: &Path) -> Result<Compiled> {
    if c.diagnostics.iter().any(|d| d.severity == "error") {
        return Err(lang::Diagnostic::new(format!(
            "playing policy failed: {}",
            c.diagnostics
                .iter()
                .filter(|d| d.severity == "error")
                .map(|d| format!("{} beat {}: {}", d.track, d.beat, d.message))
                .collect::<Vec<_>>()
                .join("; ")
        ))
        .help("relax the track's `strict`/`policy` settings or fix the listed notes; `muz check --json` lists every diagnostic")
        .path(path)
        .err());
    }
    Ok(c)
}
pub fn inspect(path: &Path) -> Result<Compiled> {
    let (v, deps) = lang::load(path)?;
    lower(v, path, deps)
}
pub fn lower(value: Value, path: &Path, dependencies: Vec<PathBuf>) -> Result<Compiled> {
    lower_inner(value, path, dependencies).map_err(|error| lang::Diagnostic::named(error, path))
}
fn lower_inner(value: Value, path: &Path, dependencies: Vec<PathBuf>) -> Result<Compiled> {
    let r = value.record().map_err(|error| {
        lang::Diagnostic::new(format!(
            "root must return song({{...}}) or bind let main = song({{...}}); {error:#}"
        ))
        .help("end the file with the value of `song({...})`")
        .err()
    })?;
    if text(r, "type", "")? != "song" {
        return Err(lang::Diagnostic::new("root expression must be song({...})")
            .help("return the record built by `song({...})`")
            .origin(r.origin())
            .err());
    }
    let mut origins = Origins::default();
    // Authored insert IDs survive track expansion only here. Lower their lanes
    // to ordinary device lanes; the audio graph needs no alias machinery.
    let mut insert_aliases: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
        ],
        "song",
    )?;
    let meter = if let Some(v) = r.get("meter") {
        let vs = v.array().map_err(|error| field(r, "meter", error))?;
        if vs.len() != 2 {
            return Err(
                lang::Diagnostic::new("meter needs numerator and denominator")
                    .help("write the meter as [beats, unit], for example [4, 4] or [7, 8]")
                    .origin(r.origin())
                    .err(),
            );
        }
        [
            vs[0]
                .integer_in(1, 255)
                .map_err(|error| field(r, "meter", error))? as u8,
            vs[1]
                .integer_in(1, 128)
                .map_err(|error| field(r, "meter", error))? as u8,
        ]
    } else {
        [4, 4]
    };
    if meter[0] == 0 || !meter[1].is_power_of_two() {
        return Err(lang::Diagnostic::new("invalid meter")
            .help("the denominator must be a power of two, for example [4, 4] or [7, 8]")
            .origin(r.origin())
            .err());
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
            return Err(lang::Diagnostic::new(format!("duplicate section '{name}'"))
                .help(
                    "sections are ordered by their position in the list; give each a distinct name",
                )
                .origin(sr.origin())
                .err());
        }
        let duration = req(sr, "duration")?;
        let duration = if let Value::Num(q) = duration {
            real(q.beats(4.0)?)
        } else {
            return Err(
                lang::Diagnostic::new("section duration must be musical time")
                    .help("use a musical duration such as 16b or 4bar")
                    .origin(sr.origin())
                    .err(),
            );
        };
        if duration <= 0.0 {
            return Err(
                lang::Diagnostic::new(format!("section '{name}' needs positive duration"))
                    .origin(sr.origin())
                    .err(),
            );
        }
        extras.sections.push(Section {
            name,
            start: end,
            end: end + duration,
            meter,
        });
        end += duration;
    }
    let tempos = tempo_map(r)?;
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
                "lifetime",
                "voice_mode",
                "policy",
                "reach",
                "movement",
                "fingering",
                "playing",
                "strict",
            ],
            "track",
        )?;
        let id = text(tr, "id", "")?;
        valid_id(&id, tr)?;
        if !ids.insert(id.clone()) {
            return Err(lang::Diagnostic::new(format!("duplicate track '{id}'"))
                .help("track, bus and kit voice ids share one namespace; rename one of them")
                .origin(tr.origin())
                .err());
        }
        origins.record("track", &id, tr);
        origins.record("route", &format!("{id}.out"), tr);
        let mut p = req(tr, "pattern")?.pattern()?.clone();
        for n in &mut p.notes {
            if let Some(clock) = n.clock.take() {
                clock.validate().map_err(|error| {
                    lang::Diagnostic::new(format!("note {}: {error}", n.key))
                        .origin(tr.origin())
                        .err()
                })?;
                let (at, duration, span) = clock.seconds();
                // Clock clips keep a seconds-local offset relative to their musical placement.
                let at = seconds_at(real(n.at), &tempos) + at;
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
            let preferences = crate::performance::Preferences::parse(req(tr, "playing")?)?;
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
                if let Err((beat, message)) = crate::performance::fingers(
                    &mut p,
                    num(tr, "reach", 12.)?,
                    &times,
                    &preferences,
                ) {
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
            crate::performance::hands(&mut p, num(tr, "reach", 12.)?, &preferences);
            check_piano(&id, &p, &tempos, tr, &mut diagnostics)?;
        }
        score.push(ScoreTrack {
            id: id.clone(),
            policy,
            pattern: p.clone(),
        });
        if kind == "kit" {
            let mut group = TrackGroup {
                id: id.clone(),
                kind: kind.clone(),
                members: Vec::new(),
            };
            let logical_inserts = list(tr, "chain")?
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    Ok(format!(
                        "{id}.{}",
                        text(v.record()?, "id", &format!("fx{i}"))?
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            for logical in &logical_inserts {
                insert_aliases.entry(logical.clone()).or_default();
            }
            let voices: BTreeSet<String> = p.notes.iter().map(|n| n.voice.clone()).collect();
            for voice in voices {
                let mut part = p.clone();
                part.notes.retain(|n| n.voice == voice);
                part.controls.clear();
                let choke_groups = ir
                    .get("chokes")
                    .map(|v| {
                        v.array()
                            .map(|v| v.to_vec())
                            .map_err(|error| field(ir, "chokes", error))
                    })
                    .transpose()?
                    .unwrap_or_default();
                for group in &choke_groups {
                    let names = group
                        .array()
                        .map_err(|error| field(ir, "chokes", error))?
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
                let mut selected = ir
                    .get(&voice)
                    .ok_or_else(|| {
                        let mapped: Vec<&str> = ir
                            .keys()
                            .map(String::as_str)
                            .filter(|key| !["type", "name", "id", "chokes"].contains(key))
                            .collect();
                        lang::Diagnostic::new(format!(
                            "kit has no instrument mapping for voice '{voice}'"
                        ))
                        .helps(lang::suggest_vocabulary("mapped voices", &voice, mapped))
                        .origin(ir.origin())
                        .err()
                    })?
                    .clone();
                let mut voice_options = tr.clone();
                if let Value::Record(options) = &selected {
                    if let Some(instrument) = options.get("instrument") {
                        fields(
                            options,
                            &["instrument", "gain", "pan", "chain", "sends", "output"],
                            "kit voice",
                        )?;
                        for (key, value) in options.iter() {
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
                        voice_options.insert("chain".into(), Value::Array(fx.into()));
                        selected = instrument.clone();
                    }
                }
                if let Value::Record(r) = &mut selected {
                    let r = std::sync::Arc::make_mut(r);
                    if text(r, "type", "")? == "sample" {
                        r.entry("root".into()).or_insert_with(|| {
                            Value::num(part.notes.first().map_or(60., |n| n.pitch))
                        });
                        r.entry("one_shot".into()).or_insert(Value::Bool(true));
                    }
                }
                let track = make_track(
                    &subid,
                    &voice_options,
                    &part,
                    &selected,
                    &tempos,
                    path,
                    &mut origins,
                )?;
                let voice_insert_count =
                    list(&voice_options, "chain")?.len() - logical_inserts.len();
                for (logical, physical) in logical_inserts
                    .iter()
                    .zip(track.inserts.iter().skip(voice_insert_count))
                {
                    insert_aliases
                        .get_mut(logical)
                        .unwrap()
                        .push(physical.id.as_str().to_owned());
                }
                group.members.push(TrackGroupMember {
                    track: subid.clone(),
                    label: voice,
                });
                tracks.push(track);
                // The expanded voice is the id the session graph validates.
                origins.record("track", &subid, tr);
                origins.record("route", &format!("{subid}.out"), tr);
            }
            extras.track_groups.push(group);
        } else {
            tracks.push(make_track(&id, tr, &p, iv, &tempos, path, &mut origins)?);
        }
    }
    if tracks.is_empty() {
        return Err(lang::Diagnostic::new("song needs at least one track")
            .help("add at least one `track(...)` entry to `tracks`")
            .origin(r.origin())
            .err());
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
        inserts: chain(list(r, "master")?, "master", path, &mut origins)?,
        output: None,
        sends: vec![],
    };
    let mut buses = vec![];
    for bv in list(r, "buses")? {
        let br = bv.record()?;
        let id = text(br, "id", "")?;
        valid_id(&id, br)?;
        if id == "master" || !ids.insert(id.clone()) {
            return Err(lang::Diagnostic::new(format!("duplicate bus/track '{id}'"))
                .help("track, bus and kit voice ids share one namespace; rename one of them")
                .origin(br.origin())
                .err());
        }
        origins.record("bus", &id, br);
        origins.record("route", &format!("{id}.out"), br);
        buses.push(Bus {
            id: Id::new(&id),
            name: id.clone(),
            inserts: chain(list(br, "chain")?, &id, path, &mut origins)?,
            output: Some(route(
                &id,
                &text(br, "output", "master")?,
                num(br, "gain", 0.0)?,
                "out",
            )),
            sends: sends(br, &id, &mut origins)?,
        });
    }
    for av in list(r, "automation")? {
        let ar = av.record()?;
        let target = text(ar, "target", "")?;
        let parameter = if target.ends_with(".out") || target.contains(".send.") {
            "gain_db"
        } else {
            target.rsplit('.').next().unwrap_or("")
        };
        let cr = req(ar, "curve")?.record()?;
        let shape = text(cr, "shape", "linear")?;
        if !matches!(shape.as_str(), "linear" | "smooth" | "step") {
            return Err(
                lang::Diagnostic::new(format!("unknown curve shape '{shape}'"))
                    .helps(lang::suggest_vocabulary(
                        "shapes",
                        &shape,
                        ["linear", "smooth", "step"],
                    ))
                    .origin(cr.origin())
                    .err(),
            );
        }
        let mut points = vec![];
        for pv in list(cr, "points")? {
            let pair = pv.array()?;
            if pair.len() != 2 {
                return Err(
                    lang::Diagnostic::new("automation points are [position, value]")
                        .help("each point is a two-item list, for example [0b, 500]")
                        .origin(cr.origin())
                        .err(),
                );
            }
            let seconds = match &pair[0] {
                Value::Num(q) if q.unit == Unit::Seconds => q.number(),
                v => seconds_at(real(v.beats()?), &tempos),
            };
            points.push(CurvePoint {
                seconds,
                value: pair[1].field_number(parameter)? as f32,
            });
        }
        if points
            .iter()
            .any(|p| !p.seconds.is_finite() || p.seconds < 0.0 || !p.value.is_finite())
            || points.is_empty()
            || points.windows(2).any(|p| p[0].seconds >= p[1].seconds)
        {
            return Err(lang::Diagnostic::new(
                "automation points must be nonempty and strictly increasing",
            )
            .help("keep at least one point and order positions from earliest to latest")
            .origin(cr.origin())
            .err());
        }
        let lane = Automation {
            target: text(ar, "target", "")?,
            points,
            shape: text(cr, "shape", "linear")?,
            origin: ar.origin().cloned(),
        };
        // An explicit physical device may sit beneath an alias prefix (for
        // example logical `drums.kick` versus physical `drums.kick.level`).
        // Resolve the longest declared device ID, never just the first prefix.
        let physical_device = tracks
            .iter()
            .flat_map(|t| std::iter::once(&t.instrument).chain(&t.inserts))
            .chain(
                std::iter::once(&master)
                    .chain(&buses)
                    .flat_map(|b| &b.inserts),
            )
            .map(|d| d.id.as_str())
            .filter(|id| {
                lane.target
                    .strip_prefix(id)
                    .is_some_and(|s| s.starts_with('.'))
            })
            .max_by_key(|id| id.len());
        let physical_route = tracks
            .iter()
            .flat_map(|t| std::iter::once(&t.output).chain(&t.sends))
            .chain(
                std::iter::once(&master)
                    .chain(&buses)
                    .flat_map(|b| b.output.iter().chain(&b.sends)),
            )
            .any(|route| route.id.as_str() == lane.target);
        if let Some((logical, physical, parameter)) = insert_aliases
            .iter()
            .filter(|_| !physical_route)
            .filter(|(logical, _)| physical_device.is_none_or(|id| id.len() <= logical.len()))
            .filter_map(|(logical, physical)| {
                lane.target
                    .strip_prefix(logical)
                    .and_then(|suffix| suffix.strip_prefix('.'))
                    .map(|parameter| (logical, physical, parameter))
            })
            .max_by_key(|(logical, _, _)| logical.len())
        {
            if physical.is_empty() {
                return Err(lang::Diagnostic::new(format!(
                    "automation target '{}' has no expanded kit voices",
                    lane.target
                ))
                .help("add hits to the kit track or remove its automation lane")
                .origin(lane.origin.as_ref())
                .err());
            }
            if physical_device == Some(logical.as_str()) {
                return Err(lang::Diagnostic::new(format!(
                    "automation target '{}' names both a logical kit insert and a physical device",
                    lane.target
                ))
                .help("rename the conflicting insert or device")
                .origin(lane.origin.as_ref())
                .err());
            }
            for device in physical {
                extras.automation.push(Automation {
                    target: format!("{device}.{parameter}"),
                    ..lane.clone()
                });
            }
        } else {
            extras.automation.push(lane);
        }
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
    if let Err(message) = session.validate_graph_budget() {
        let resources = session.graph_resources();
        let mut diagnostic = lang::Diagnostic::new(message).path(path).help(
            "reduce device counts or sample zones, or raise MUZ_GRAPH_BUDGET when the host can afford it",
        );
        if let Some((name, _)) = resources.contributors.first()
            && let Some(origin) = origin_of_contributor(name, &origins)
        {
            diagnostic = diagnostic.origin(Some(origin));
        }
        return Err(diagnostic.err());
    }
    for d in session
        .tracks
        .iter_mut()
        .flat_map(|t| std::iter::once(&mut t.instrument).chain(&mut t.inserts))
        .chain(session.buses.iter_mut().flat_map(|b| &mut b.inserts))
        .chain(&mut session.master.inserts)
    {
        crate::assets::stamp(d).map_err(|error| {
            lang::Diagnostic::locate(error, origins.get("device", d.id.as_str()))
        })?;
        session.extras.dependencies.extend(crate::assets::paths(d));
    }
    session.extras.dependencies.sort();
    session.extras.dependencies.dedup();
    validate_graph(&session, &origins)?;
    Ok(Compiled {
        session,
        score,
        diagnostics,
        locations: origins
            .0
            .into_iter()
            .map(|(id, origin)| (id, origin.location()))
            .collect(),
    })
}
pub fn tick(beat: f64) -> u64 {
    (beat.max(0.0) * PPQ as f64).round() as u64
}
/// Shared tempo interpretation for source-level time arithmetic and rendering.
pub(crate) fn tempo_map(r: &Record) -> Result<Vec<MidiTempo>> {
    let bpm = num(r, "tempo", 120.0)?;
    if !(20.0..=400.0).contains(&bpm) {
        return Err(lang::Diagnostic::new("tempo must be 20..400 BPM")
            .help("use a plain number in beats per minute, for example tempo: 128")
            .origin(r.origin())
            .err());
    }
    let mut tempos = vec![MidiTempo {
        tick: 0,
        micros_per_quarter: (60_000_000.0 / bpm).round() as u32,
        source_order: 0,
    }];
    for (i, v) in list(r, "tempos")?.iter().enumerate() {
        let row = v.array()?;
        if row.len() != 2 {
            return Err(lang::Diagnostic::new("tempo points are [beat, bpm]")
                .help("each point is a two-item list, for example [32b, 140]")
                .origin(r.origin())
                .err());
        }
        let beat = real(row[0].beats()?);
        let bpm = row[1].quantity(Unit::Bpm, 1.)?;
        if beat < 0.0 || !(20.0..=400.0).contains(&bpm) {
            return Err(lang::Diagnostic::new("invalid tempo point")
                .help("beats are nonnegative and tempo stays within 20..400 BPM")
                .origin(r.origin())
                .err());
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
    Ok(tempos)
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
    tr: &Record,
    p: &Pattern,
    iv: &Value,
    tempos: &[MidiTempo],
    path: &Path,
    origins: &mut Origins,
) -> Result<Track> {
    let instrument = device(iv, &format!("{id}.instrument"), path, origins)?;
    let mut imported = ImportedMidi {
        tempos: tempos.to_vec(),
        ..Default::default()
    };
    for (i, n) in p.notes.iter().enumerate() {
        let onset_seconds = seconds_at(real(n.at), tempos) + n.offset_ms / 1000.0;
        let release_seconds = seconds_at(real(n.at) + real(n.dur) * n.gate, tempos)
            + (n.offset_ms + n.release_offset_ms) / 1000.0;
        if release_seconds <= onset_seconds.max(0.0) {
            return Err(lang::Diagnostic::new(format!(
                "note {} has a nonpositive performed duration",
                n.key
            ))
            .help("give the note a positive duration; releases must follow their attack")
            .origin(tr.origin())
            .err());
        }
        if n.data.get("channel").is_some_and(|v| {
            v.as_f64()
                .is_none_or(|n| n.fract() != 0. || !(0.0..=15.).contains(&n))
        }) {
            return Err(lang::Diagnostic::new(format!(
                "note {} has an invalid MIDI channel; use 0..15",
                n.key
            ))
            .origin(tr.origin())
            .err());
        }
        let onset = tick(beat_at_seconds(onset_seconds, tempos));
        let release = tick(beat_at_seconds(release_seconds, tempos));
        if let Some(value) = n.data.get("sample_zone") {
            let maps = instrument.sample_maps().map_err(anyhow::Error::msg)?;
            if maps.is_empty() {
                return Err(lang::Diagnostic::new(format!(
                    "track {id}: sample_zone requires a sampler or graph reader"
                ))
                .origin(iv.record().ok().and_then(Record::origin))
                .err());
            }
            for zones in &maps {
                if !model::valid_sample_zone(value, zones, n.pitch.round() as u8, n.velocity as f32)
                {
                    return Err(lang::Diagnostic::new(format!(
                        "track {id}, note {}: sample_zone must be a zero-based index of a zone matching the performed key and velocity",
                        n.key
                    ))
                    .help("zones are ordered as written in `sample(...)`; indices start at 0")
                    .origin(iv.record().ok().and_then(Record::origin))
                    .err());
                }
            }
        }
        let expression = crate::expression::Program::parse(n.data.get("expression"))?;
        if expression.points[..expression.len as usize]
            .iter()
            .any(|p| match instrument.kind {
                DeviceKind::StudioSynth | DeviceKind::Sampler => !matches!(p.kind, 0 | 1 | 2 | 4),
                DeviceKind::VoicePatch | DeviceKind::Clap => false,
                _ => true,
            })
        {
            return Err(lang::Diagnostic::new(format!(
                "track {id}: {:?} does not support the requested note expression; preset synths and samplers support per-note volume, expression, pan and tuning; other expression requires voice_patch or CLAP",
                instrument.kind
            ))
            .origin(tr.origin())
            .err());
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
            return Err(lang::Diagnostic::new(format!(
                "track {id}: device adapter cannot consume this raw message; structural SMF still preserves it"
            ))
            .help("send channel messages through note/cc helpers, or export structural MIDI instead")
            .origin(tr.origin())
            .err());
        }
        if matches!(bytes[0] >> 4, 8 | 9) {
            let channel = bytes[0] & 15;
            let key = bytes[1];
            if bytes[0] >> 4 == 9 && bytes[2] > 0 {
                if held.insert((channel, key), (at, bytes[2], i)).is_some() {
                    return Err(lang::Diagnostic::new(format!(
                        "track {id}: ambiguous overlapping raw note-ons on channel {channel}, key {key}; release first or use separate channels"
                    ))
                    .origin(tr.origin())
                    .err());
                }
            } else {
                let (start, velocity, order) = held.remove(&(channel, key)).ok_or_else(|| {
                    lang::Diagnostic::new(format!(
                        "track {id}: raw note-off has no matching note-on"
                    ))
                    .origin(tr.origin())
                    .err()
                })?;
                if at <= start {
                    return Err(lang::Diagnostic::new("raw note needs positive duration")
                        .origin(tr.origin())
                        .err());
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
                return Err(lang::Diagnostic::new(format!(
                    "track {id}: native device does not consume raw channel messages; use a capable plugin or notes_only()"
                ))
                .origin(tr.origin())
                .err());
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
        return Err(
            lang::Diagnostic::new(format!("track {id}: raw note-on has no release"))
                .help("pair every raw note-on with a note-off, or use note(...) values")
                .origin(tr.origin())
                .err(),
        );
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
            return Err(lang::Diagnostic::new(format!(
                "track {id}: conflicting CC {} values on channel {} at beat {}",
                c.controller,
                c.channel,
                c.tick as f64 / PPQ as f64
            ))
            .origin(tr.origin())
            .err());
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
    let mut inserts = chain(list(tr, "chain")?, id, path, origins)?;
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
            params: BTreeMap::from([(
                "pan".into(),
                pan.scalar().map_err(|error| field(tr, "pan", error))? as f32,
            )]),
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
        instrument,
        inserts,
        output: route(
            id,
            &text(tr, "output", "master")?,
            num(tr, "gain", 0.0)?,
            "out",
        ),
        sends: sends(tr, id, origins)?,
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
fn sends(r: &Record, id: &str, origins: &mut Origins) -> Result<Vec<Route>> {
    let mut out = vec![];
    if let Some(v) = r.get("sends") {
        for (k, v) in v.record()?.iter() {
            let (gain, pre) = if let Value::Record(send) = v {
                fields(send, &["gain", "pre"], "send")?;
                (
                    num(send, "gain", -12.)?,
                    send.get("pre")
                        .is_some_and(|v| matches!(v, Value::Bool(true))),
                )
            } else {
                (
                    v.quantity(Unit::Db, 1.)
                        .map_err(|error| field(r, &format!("send.{k}"), error))?,
                    false,
                )
            };
            let mut send = route(id, k, gain, &format!("send.{k}"));
            send.pre = pre;
            origins.record(
                "route",
                send.id.as_str(),
                match v {
                    Value::Record(send) => send,
                    _ => r,
                },
            );
            out.push(send);
        }
    }
    Ok(out)
}
fn chain(vs: &[Value], id: &str, path: &Path, origins: &mut Origins) -> Result<Vec<Device>> {
    vs.iter()
        .enumerate()
        .map(|(i, v)| {
            let r = v.record()?;
            let key = text(r, "id", &format!("fx{i}"))?;
            device(v, &format!("{id}.{key}"), path, origins)
        })
        .collect()
}
fn device(v: &Value, id: &str, path: &Path, origins: &mut Origins) -> Result<Device> {
    let r = v.record()?;
    origins.record("device", id, r);
    let ty = text(r, "type", "synth")?;
    let name = if ty == "sample" {
        String::new()
    } else {
        text(r, "name", "")?
    };
    #[cfg(not(feature = "desktop"))]
    if matches!(ty.as_str(), "piano" | "plugin") {
        return Err(
            lang::Diagnostic::new("native plugin hosting is unavailable in this build")
                .origin(r.origin())
                .err(),
        );
    }
    #[cfg(not(feature = "desktop"))]
    let plugin_alias: Option<serde_json::Value> = None;
    #[cfg(feature = "desktop")]
    let plugin_alias = if matches!(ty.as_str(), "piano" | "plugin") && !r.contains_key("path") {
        crate::plugins::configured_alias(&name)?
    } else {
        None
    };
    let plugin_path = plugin_alias
        .as_ref()
        .and_then(|v| v.get("path"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&name);
    #[cfg(feature = "desktop")]
    if matches!(ty.as_str(), "piano" | "plugin")
        && !r.contains_key("path")
        && plugin_alias.is_none()
        && !name.is_empty()
        && name != "default"
        && !name.contains('/')
        && Path::new(&name).extension().is_none()
    {
        return Err(lang::Diagnostic::new(format!(
            "plugin alias '{name}' is not configured; add it to {} or give an explicit path",
            crate::plugins::config_path().display()
        ))
        .origin(r.origin())
        .err());
    }
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
            "bitcrusher" => DeviceKind::Bitcrusher,
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
            _ => {
                return Err(lang::Diagnostic::new(format!("unknown effect '{name}'"))
                    .helps(lang::suggest_vocabulary(
                        "effects",
                        &name,
                        EFFECTS.iter().copied(),
                    ))
                    .origin(r.origin())
                    .err());
            }
        }
    } else if ty == "piano" || ty == "plugin" {
        if text(r, "path", plugin_path)?.ends_with(".clap") {
            DeviceKind::Clap
        } else {
            DeviceKind::Vst3
        }
    } else {
        // Synth parameters are supplied by source catalogs or explicit records.
        DeviceKind::StudioSynth
    };
    for (k, v) in r.iter() {
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
            "lifetime",
            "voice_mode",
            "sample_budget_frames",
        ]
        .contains(&k.as_str())
        {
            let n = if matches!(kind, DeviceKind::Vst3 | DeviceKind::Clap) {
                v.scalar()?
            } else {
                v.field_number(k)?
            } as f32;
            params.insert(k.clone(), n);
        }
    }
    let resource_root = r
        .get("_module_dir")
        .map(|v| v.text().map(PathBuf::from))
        .transpose()?
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_owned());
    let vst3 = if matches!(kind, DeviceKind::Vst3 | DeviceKind::Clap) {
        let bundle = text(r, "path", plugin_path)?;
        if bundle.is_empty() || bundle == "default" {
            return Err(lang::Diagnostic::new(
                "plugin requires an explicit path; keep machine-specific instrument settings in a user module",
            )
            .help("pass an explicit path or name a configured alias from plugins.json")
            .origin(r.origin())
            .err());
        }
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
                plugin_alias
                    .as_ref()
                    .and_then(|v| v.get("class"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(""),
            )?,
            expected_version: text(r, "version", "")?,
        })
    } else {
        None
    };
    Ok(Device {
        asset_versions: Vec::new(),
        patch: if ty == "voice_patch" {
            Some(crate::patch_source::lower(r)?)
        } else {
            None
        },
        generation: 0,
        rack: if ty == "rack" {
            let branches = list(r, "branches")?
                .iter()
                .enumerate()
                .map(|(i, v)| chain(v.array()?, &format!("{id}.{i}"), path, origins))
                .collect::<Result<Vec<_>>>()?;
            if branches.is_empty()
                || branches.len() > 8
                || branches.iter().map(Vec::len).sum::<usize>() > 32
            {
                return Err(lang::Diagnostic::new(
                    "rack needs 1..8 branches and at most 32 devices",
                )
                .origin(r.origin())
                .err());
            }
            if branches.iter().flatten().any(|d| {
                d.kind == DeviceKind::Rack
                    || (d.kind.is_instrument()
                        && !matches!(d.kind, DeviceKind::Vst3 | DeviceKind::Clap))
                    || d.sidechain.is_some()
            }) {
                return Err(lang::Diagnostic::new(
                    "rack branches contain effects; attach sidechain to the rack; nested racks are not supported",
                )
                .origin(r.origin())
                .err());
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
fn valid_id(s: &str, record: &Record) -> Result<()> {
    if s.is_empty()
        || s.len() > 96
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    {
        return Err(lang::Diagnostic::new(format!("invalid name '{s}'"))
            .help("ids use letters, digits, _ . or - (at most 96 characters)")
            .origin(record.origin())
            .err());
    }
    Ok(())
}
/// Name the record behind a graph-budget contributor such as `track lead`.
fn origin_of_contributor<'a>(name: &str, origins: &'a Origins) -> Option<&'a Origin> {
    let (kind, id) = name.split_once(' ')?;
    origins.get(kind, id)
}
fn validate_graph(s: &Session, origins: &Origins) -> Result<()> {
    let buses: BTreeSet<_> = std::iter::once(&s.master.id)
        .chain(s.buses.iter().map(|b| &b.id))
        .collect();
    let bus_names: Vec<&str> = buses.iter().map(|id| id.as_str()).collect();
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
            let pending: Vec<&str> = s
                .buses
                .iter()
                .filter(|b| !done.contains(&b.id))
                .map(|b| b.id.as_str())
                .collect();
            let mut diagnostic = lang::Diagnostic::new(format!(
                "bus routing cycle or missing target: {} never resolves",
                pending.join(", ")
            ))
            .help("a bus cannot route to itself or to a bus that routes back to it");
            if let Some(origin) = pending.first().and_then(|id| origins.get("bus", id)) {
                diagnostic = diagnostic.origin(Some(origin));
            }
            return Err(diagnostic.err());
        }
    }
    for route in s
        .tracks
        .iter()
        .flat_map(|t| std::iter::once(&t.output).chain(&t.sends))
        .chain(s.buses.iter().flat_map(|b| b.output.iter().chain(&b.sends)))
    {
        if !buses.contains(&route.to) {
            return Err(lang::Diagnostic::new(format!(
                "route {} targets missing bus {}",
                route.id, route.to
            ))
            .helps(lang::suggest_vocabulary(
                "buses",
                route.to.as_str(),
                bus_names.iter().copied(),
            ))
            .origin(origins.get("route", route.id.as_str()))
            .err());
        }
        if !route.gain_db.is_finite() || !(-120.0..=24.0).contains(&route.gain_db) {
            return Err(lang::Diagnostic::new(format!(
                "route {} gain must be -120..24 dB",
                route.id
            ))
            .origin(origins.get("route", route.id.as_str()))
            .err());
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
            return Err(lang::Diagnostic::new(format!("duplicate device {}", d.id))
                .help("device ids repeat per track and bus; rename one of them")
                .origin(origins.get("device", d.id.as_str()))
                .err());
        }
        for (k, v) in &d.params {
            if matches!(
                d.kind,
                DeviceKind::Vst3 | DeviceKind::Clap | DeviceKind::Rack | DeviceKind::VoicePatch
            ) {
                continue;
            }
            let specs = crate::source::parameter_specs(d.kind);
            let spec = specs.iter().find(|s| s.name == k).ok_or_else(|| {
                lang::Diagnostic::new(format!("{}: unknown parameter '{k}'", d.id))
                    .helps(lang::suggest_vocabulary(
                        "parameters",
                        k,
                        specs.iter().map(|s| s.name),
                    ))
                    .origin(origins.get("device", d.id.as_str()))
                    .err()
            })?;
            if !v.is_finite() || *v < spec.min || *v > spec.max {
                return Err(lang::Diagnostic::new(format!(
                    "{}.{} must be {}..{}",
                    d.id, k, spec.min, spec.max
                ))
                .origin(origins.get("device", d.id.as_str()))
                .err());
            }
        }
    }
    Ok(())
}
fn check_piano(
    id: &str,
    p: &Pattern,
    tempos: &[MidiTempo],
    opts: &Record,
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

fn fields(r: &Record, allowed: &[&str], context: &str) -> Result<()> {
    for k in r.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(
                lang::Diagnostic::new(format!("unknown {context} field '{k}'"))
                    .helps(lang::suggest_vocabulary(
                        &format!("{context} fields"),
                        k,
                        allowed.iter().copied(),
                    ))
                    .origin(r.origin())
                    .err(),
            );
        }
    }
    Ok(())
}

pub(crate) fn sample_zones(r: &Record, path: &Path) -> Result<Vec<model::SampleZone>> {
    let resource_root = r
        .get("_module_dir")
        .map(|v| v.text().map(PathBuf::from))
        .transpose()?
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_owned());
    let sources = match req(r, "name")? {
        Value::Array(xs) => xs.to_vec(),
        v => vec![v.clone()],
    };
    if sources.is_empty() {
        return Err(lang::Diagnostic::new("sample needs at least one zone")
            .help("pass `name: \"…/sample.wav\"` or a list of zone records")
            .origin(r.origin())
            .err());
    }
    sources
        .iter()
        .map(|v| {
            // Instrument gain remains shared; only a zone record supplies calibration.
            let gain_db = if let Value::Record(zone) = v {
                num(zone, "gain_db", 0.)?
            } else {
                0.
            };
            if !gain_db.is_finite() || !(-120.0..=120.0).contains(&gain_db) {
                let zone = match v {
                    Value::Record(zone) => zone,
                    _ => r,
                };
                return Err(lang::Diagnostic::new(
                    "sample zone gain_db must be finite and within -120..120",
                )
                .origin(zone.origin())
                .err());
            }
            let mut options = r.clone();
            let zone_origin = match v {
                Value::Record(zone) => zone.origin().cloned(),
                _ => None,
            };
            let file = if let Value::Record(zone) = v {
                options.extend(zone.iter().map(|(k, v)| (k.clone(), v.clone())));
                text(zone, "path", "")?
            } else {
                v.text()?.to_owned()
            };
            let pair = |key: &str, default: [f64; 2]| -> Result<[f64; 2]> {
                if let Some(v) = options.get(key) {
                    let vs = v.array()?;
                    if vs.len() != 2 {
                        return Err(lang::Diagnostic::new(format!("{key} needs two values"))
                            .help("write the range as [low, high]")
                            .origin(zone_origin.as_ref())
                            .err());
                    }
                    if key == "keys" { Ok([vs[0].integer_in(0,127)? as f64,vs[1].integer_in(0,127)? as f64]) } else { Ok([vs[0].scalar()?, vs[1].scalar()?]) }
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
                return Err(lang::Diagnostic::new("invalid sample zone range")
                    .help("keys are 0..127 in ascending order, velocity 0..1, root 0..127 and offset nonnegative")
                    .origin(zone_origin.as_ref())
                    .err());
            }
            Ok(model::SampleZone {
                path: resource_root.join(file).display().to_string(),
                root,
                gain_db: gain_db as f32,
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
