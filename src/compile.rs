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
impl Diagnostic {
    /// The position as a reader counts it in the song's meter: one-based bar
    /// and beat (in the meter's beat unit, so eighths in 7/8), then the score
    /// beat that `pattern.notes` and `--json` use, as in `bar 52 beat 3.5 (205.5b)`.
    pub fn position(&self, meter: [u8; 2]) -> String {
        let quarters_per_bar = f64::from(meter[0]) * 4.0 / f64::from(meter[1]);
        let bar = (self.beat / quarters_per_bar + 1e-9).floor();
        let within = (self.beat - bar * quarters_per_bar) * f64::from(meter[1]) / 4.0 + 1.0;
        let tidy = |x: f64| (x * 1e6).round() / 1e6 + 0.0;
        format!(
            "bar {} beat {} ({}b)",
            bar + 1.0,
            tidy(within),
            tidy(self.beat)
        )
    }
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
pub fn compile_with_context(
    path: &Path,
    loader: std::rc::Rc<dyn lang::SourceLoader>,
    context: &crate::host::HostContext,
) -> Result<Compiled> {
    context.run(|| compile_with_loader(path, loader))
}
pub fn compile_with_loader(
    path: &Path,
    loader: std::rc::Rc<dyn lang::SourceLoader>,
) -> Result<Compiled> {
    let (value, dependencies) = lang::load_with_loader(path, loader)?;
    validate_compiled(lower(value, path, dependencies)?, path)
}
/// Fail compilation when any diagnostic is an error. Errors from source checks
/// (`check.*`) and from playing policies are described separately, each
/// followed by its diagnostics one per line, as `muz check` lists warnings.
pub fn validate_compiled(c: Compiled, path: &Path) -> Result<Compiled> {
    let errors: Vec<&Diagnostic> = c
        .diagnostics
        .iter()
        .filter(|d| d.severity == "error")
        .collect();
    if errors.is_empty() {
        return Ok(c);
    }
    let meter = c.session.transport.meter();
    let lines = |keep: &dyn Fn(&Diagnostic) -> bool| -> String {
        errors
            .iter()
            .filter(|d| keep(d))
            .map(|d| {
                format!(
                    "\n  {}: {} {}: {}",
                    d.code,
                    d.track,
                    d.position(meter),
                    d.message
                )
            })
            .collect()
    };
    let is_check = |d: &Diagnostic| d.code.starts_with("check.");
    let mut sections = vec![];
    let mut policy_tracks: Vec<&str> = vec![];
    let mut checks: Vec<(&str, &str)> = vec![];
    for d in &errors {
        if is_check(d) {
            if !checks.contains(&(d.code.as_str(), d.track.as_str())) {
                checks.push((&d.code, &d.track));
            }
        } else if !policy_tracks.contains(&d.track.as_str()) {
            policy_tracks.push(&d.track);
        }
    }
    if !policy_tracks.is_empty() {
        sections.push(format!(
            "playing policy failed on {}:{}",
            policy_tracks.join(", "),
            lines(&|d| !is_check(d))
        ));
    }
    for (code, track) in &checks {
        sections.push(format!(
            "check {code} failed on {track}:{}",
            lines(&|d| d.code == *code && d.track == *track)
        ));
    }
    let mut error = lang::Diagnostic::new(sections.join("\n"));
    if !policy_tracks.is_empty() {
        error = error.help("relax the track's `strict`/`policy` settings or fix the listed notes");
    }
    if !checks.is_empty() {
        error = error.help("a source check in the track's or song's `checks` reported these with `severity: \"error\"`; fix the listed notes, or report the rule as a warning, for example with `checks.severity(f, \"warning\")` from std/checks");
    }
    Err(error
        .help("`muz check --json` lists every diagnostic")
        .path(path)
        .err())
}
pub fn inspect(path: &Path) -> Result<Compiled> {
    let (v, deps) = lang::load(path)?;
    lower(v, path, deps)
}
pub fn lower(value: Value, path: &Path, dependencies: Vec<PathBuf>) -> Result<Compiled> {
    lower_inner(value, path, dependencies).map_err(|error| lang::Diagnostic::named(error, path))
}
fn lower_inner(value: Value, path: &Path, dependencies: Vec<PathBuf>) -> Result<Compiled> {
    crate::host::check_cancelled()?;
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
    // Explicitly written fader and send levels by route id, so a lane that
    // replaces one can say so.
    let mut static_levels: BTreeMap<String, StaticLevel> = BTreeMap::new();
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
            "checks",
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
    // Source checks of every track share one evaluator, and so one budget.
    let mut checker = None;
    for tv in list(r, "tracks")? {
        crate::host::check_cancelled()?;
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
                "checks",
                "idle_after",
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
                n.at = music::time(onset)?;
                n.dur = music::time(beat_at_seconds(at + duration, &tempos) - onset)?;
                p.span = p
                    .span
                    .max(music::time(beat_at_seconds(at + span, &tempos))?);
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
        run_checks(&id, tr, &p, &mut checker, &mut diagnostics)?;
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
                if let Value::Record(options) = &selected
                    && let Some(instrument) = options.get("instrument")
                {
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
                    if tr.get("gain").is_some() || options.get("gain").is_some() {
                        voice_options.insert(
                            "gain".into(),
                            Value::num(num(tr, "gain", 0.)? + num(options, "gain", 0.)?),
                        );
                    }
                    let mut fx = list(options, "chain")?.to_vec();
                    fx.extend_from_slice(list(tr, "chain")?);
                    voice_options.insert("chain".into(), Value::Array(fx.into()));
                    selected = instrument.clone();
                }
                if let Value::Record(r) = &mut selected {
                    let r = std::rc::Rc::make_mut(r);
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
                static_route_levels(&subid, &voice_options, &mut static_levels)?;
                tracks.push(track);
                // The expanded voice is the id the session graph validates.
                origins.record("track", &subid, tr);
                origins.record("route", &format!("{subid}.out"), tr);
            }
            extras.track_groups.push(group);
        } else {
            tracks.push(make_track(&id, tr, &p, iv, &tempos, path, &mut origins)?);
            static_route_levels(&id, tr, &mut static_levels)?;
        }
    }
    if tracks.is_empty() {
        return Err(lang::Diagnostic::new("song needs at least one track")
            .help("add at least one `track(...)` entry to `tracks`")
            .origin(r.origin())
            .err());
    }
    run_song_checks(r, &score, &mut checker, &mut diagnostics)?;
    for t in &mut tracks {
        let TrackSource::Midi(src) = &mut t.source;
        src.imported.summary.end_tick = checked_tick(end)?;
        src.summary.end_tick = checked_tick(end)?;
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
        static_route_levels(&id, br, &mut static_levels)?;
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
            if let Some(level) = static_levels.get(&lane.target) {
                diagnostics.push(level.replaced_by(&lane.target));
            }
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
/// Checked lowering boundary; negative performed pickups intentionally clamp at zero.
pub fn checked_tick(beat: f64) -> Result<u64> {
    let ticks = (beat.max(0.) * PPQ as f64).round();
    anyhow::ensure!(
        beat.is_finite() && ticks < u64::MAX as f64,
        "performed tick time overflow"
    );
    Ok(ticks as u64)
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
            tick: checked_tick(beat)?,
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
    let expression_schema = crate::expression::Schema::for_device(&instrument)?;
    let mut imported = ImportedMidi {
        tempos: tempos.to_vec(),
        ..Default::default()
    };
    for (i, n) in p.notes.iter().enumerate() {
        crate::host::check_cancelled()?;
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
        let onset = checked_tick(beat_at_seconds(onset_seconds, tempos))?;
        let release = checked_tick(beat_at_seconds(release_seconds, tempos))?;
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
        // Time positions become phases of this note's performed gate; beats
        // count through the tempo map from the performed attack, and a clock
        // offset after beats is added once that beat is located.
        let attack = onset_seconds.max(0.0);
        let attack_beat = beat_at_seconds(attack, tempos);
        let gate = release_seconds - attack;
        let after_beats =
            |beats: f64| seconds_at(attack_beat + beats, tempos) - seconds_at(attack_beat, tempos);
        let resolve = |position| match position {
            crate::expression::Position::Phase(phase) => phase,
            crate::expression::Position::Seconds(seconds) => seconds / gate,
            crate::expression::Position::Beats(beats) => after_beats(beats) / gate,
            crate::expression::Position::BeatsSeconds { beats, seconds } => {
                (after_beats(beats) + seconds) / gate
            }
        };
        let expression = crate::expression::Program::parse_for_note(
            n.data.get("expression"),
            &expression_schema,
            &resolve,
        )
        .map_err(|e| {
            lang::Diagnostic::new(format!(
                "track {id}, note {}, patch {}: {e:#}",
                n.key,
                instrument
                    .patch
                    .as_ref()
                    .and_then(|p| p.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(instrument.id.as_str())
            ))
            .origin(tr.origin())
            .err()
        })?;
        if expression.points[..expression.len as usize]
            .iter()
            .any(|p| match instrument.kind {
                DeviceKind::StudioSynth | DeviceKind::Sampler | DeviceKind::Sfz => {
                    !matches!(p.kind, 0 | 1 | 2 | 4)
                }
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
        crate::host::check_cancelled()?;
        imported.controllers.push(MidiController {
            tick: checked_tick(beat_at_seconds(
                seconds_at(real(c.at), tempos) + c.offset_ms / 1000.0,
                tempos,
            ))?,
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
        let at = checked_tick(beat_at_seconds(
            seconds_at(real(raw.at), tempos) + raw.offset_ms / 1000.,
            tempos,
        ))?;
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
        end_tick: checked_tick(real(p.span))?,
        notes: imported.notes.len() as u32,
        controllers: imported.controllers.len() as u32,
        tempos: tempos.len() as u32,
        events: (imported.notes.len() * 2 + imported.controllers.len() + tempos.len()) as u32,
        ..Default::default()
    };
    let mut inserts = chain(list(tr, "chain")?, id, path, origins)?;
    if let Some(pan) = tr.get("pan") {
        inserts.push(Device {
            sfz: None,
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
        idle_after: tr
            .get("idle_after")
            .map(|v| {
                v.quantity(Unit::Seconds, 1.)
                    .and_then(|seconds| {
                        if !(0.0..=3600.0).contains(&seconds) {
                            anyhow::bail!("must be 0 to 3600 seconds");
                        }
                        Ok(seconds)
                    })
                    .map_err(|error| field(tr, "idle_after", error))
            })
            .transpose()?,
    })
}
/// An output `gain` written in source, which a lane on its `.out` replaces.
struct StaticLevel {
    owner: String,
    gain_db: f64,
}
impl StaticLevel {
    /// A route lane sets the whole fader from the song's start to its end (it
    /// holds its first value before its first point and its last value after
    /// its last), so an explicit `gain` under it never sounds.
    fn replaced_by(&self, target: &str) -> Diagnostic {
        Diagnostic {
            severity: "warning".into(),
            code: "automation.replaces_gain".into(),
            track: self.owner.clone(),
            beat: 0.,
            message: format!(
                "`gain: {gain}` on {owner} has no effect: the automation lane on {target} \
                 replaces it for the whole song; put the whole fader level in the lane \
                 (add {gain} dB to each point) or drop `gain`",
                gain = self.gain_db,
                owner = self.owner,
            ),
        }
    }
}
/// Record the output `gain` a track, kit voice or bus writes explicitly. A
/// default is not recorded, so leaving `gain` out never warns. Send levels are
/// not recorded: a send must be declared with a level to exist, and a parked
/// level such as −120 dB under a throw lane is the ordinary idiom.
fn static_route_levels(
    id: &str,
    r: &Record,
    levels: &mut BTreeMap<String, StaticLevel>,
) -> Result<()> {
    if r.get("gain").is_some() {
        levels.insert(
            format!("{id}.out"),
            StaticLevel {
                owner: id.into(),
                gain_db: num(r, "gain", 0.)?,
            },
        );
    }
    Ok(())
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
            let d = device(v, &format!("{id}.{key}"), path, origins)?;
            if d.vst3.as_ref().is_some_and(|c| !c.note_controls.is_empty()) {
                return Err(lang::Diagnostic::new(format!(
                    "{id}.{key}: note_controls belong on a track's plugin instrument; effects receive no notes"
                ))
                .origin(r.origin())
                .err());
            }
            Ok(d)
        })
        .collect()
}
/// Lower one standalone device expression for embedded CLAP source state.
pub fn lower_standalone_device(value: &Value, id: &str) -> Result<Device> {
    device(
        value,
        id,
        Path::new("/muz-state/main.muz"),
        &mut Origins::default(),
    )
}

/// `note_controls: {name: {param, mode}}` on a CLAP instrument binds per-note
/// expression names to plugin parameters; the adapter checks plugin support.
fn plugin_note_controls(
    r: &Record,
    kind: DeviceKind,
) -> Result<BTreeMap<String, model::NoteParameter>> {
    let Some(value) = r.get("note_controls") else {
        return Ok(BTreeMap::new());
    };
    let fail = |message: String| {
        Err(lang::Diagnostic::new(message)
            .help("write note_controls: {name: {param: \"p12\", mode: \"mod\"}}; mode is \"value\" or \"mod\"")
            .origin(r.origin())
            .err())
    };
    if kind != DeviceKind::Clap {
        return fail(
            "per-note parameter control needs a CLAP instrument with a CLAP note port; VST3 parameters are not addressed per note".into(),
        );
    }
    let mut out = BTreeMap::new();
    for (name, spec) in value.record()?.iter() {
        let spec = spec.record()?;
        if let Some(extra) = spec
            .keys()
            .find(|k| !matches!(k.as_str(), "param" | "mode"))
        {
            return fail(format!("note control '{name}' has unknown field '{extra}'"));
        }
        let param = match spec.get("param") {
            Some(Value::Str(s)) => s.to_string(),
            Some(v) => format!("{}", v.scalar()?),
            None => return fail(format!("note control '{name}' requires a param")),
        };
        let mode = match spec.get("mode").map(Value::text).transpose()? {
            Some("value") => model::NoteParameterMode::Value,
            Some("mod") => model::NoteParameterMode::Mod,
            _ => {
                return fail(format!(
                    "note control '{name}' requires mode \"value\" or \"mod\""
                ));
            }
        };
        out.insert(name.clone(), model::NoteParameter { param, mode });
    }
    crate::expression::Schema::plugin(out.keys().map(String::as_str))?;
    Ok(out)
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
    } else if ty == "sfz" {
        DeviceKind::Sfz
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
            "note_controls",
            "defines",
            "source_overlays",
            "max_voices",
            "seed",
            "embed_assets",
            "program",
            "render_thread",
        ]
        .contains(&k.as_str())
        {
            let n = if matches!(kind, DeviceKind::Vst3 | DeviceKind::Clap) {
                v.scalar()?
            } else if let Some(spec) = crate::description::parameter_specs(kind)
                .iter()
                .find(|s| s.name == k)
            {
                use crate::description::ParameterUnit::*;
                match spec.unit {
                    // Switches such as the limiter's `true_peak` accept booleans.
                    Scalar => match v {
                        Value::Bool(on) => f64::from(u8::from(*on)),
                        _ => v.scalar()?,
                    },
                    Milliseconds => v.quantity(Unit::Seconds, 1000.)?,
                    Seconds => v.quantity(Unit::Seconds, 1.)?,
                    Hertz => v.quantity(Unit::Hz, 1.)?,
                    Decibels => v.quantity(Unit::Db, 1.)?,
                    Beats => music::real(v.beats()?),
                }
            } else {
                v.field_number(k)?
            } as f32;
            params.insert(k.clone(), n);
        }
    }
    if kind == DeviceKind::Sampler
        && let Some(mode) = r.get("voice_mode")
    {
        // Named as on a voice patch; sessions keep the numeric code.
        let names = crate::description::parameter_value_names(kind, "voice_mode");
        let code = match mode {
            Value::Str(name) => names.iter().find(|(_, n)| n == name).map(|(code, _)| *code),
            _ => Some(mode.scalar()? as i32).filter(|c| names.iter().any(|(code, _)| code == c)),
        }
        .ok_or_else(|| {
            lang::Diagnostic::new("voice_mode must be poly, legato or retrigger")
                .origin(r.origin())
                .err()
        })?;
        params.insert("voice_mode".into(), code as f32);
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
            note_controls: plugin_note_controls(r, kind)?,
            render_thread: r.get("render_thread").is_some_and(Value::truth),
        })
    } else {
        None
    };
    Ok(Device {
        sfz: if kind == DeviceKind::Sfz {
            let voices = r
                .get("max_voices")
                .map(|v| v.integer_in(1, 4096))
                .transpose()?
                .unwrap_or(256) as usize;
            let sfz_path = resource_root.join(text(r, "path", "")?);
            let defines: BTreeMap<String, String> = r
                .get("defines")
                .map(|v| serde_json::from_value(v.json()))
                .transpose()?
                .unwrap_or_default();
            let source_overlays: Vec<crate::sfz::SourceOverlay> = r
                .get("source_overlays")
                .map(|v| serde_json::from_value(v.json()))
                .transpose()?
                .unwrap_or_default();
            let program = if let Some(p) = r.get("program") {
                serde_json::from_value(p.json())?
            } else {
                crate::sfz::load(
                    &sfz_path,
                    &crate::sfz::Options {
                        defines: defines.clone(),
                        source_overlays: source_overlays.clone(),
                        ..Default::default()
                    },
                )?
            };
            Some(model::SfzConfig {
                max_sample_frames: r
                    .get("sample_budget_frames")
                    .map(|v| v.integer_in(1, model::MAX_SFZ_SAMPLE_FRAMES as i64))
                    .transpose()?
                    .unwrap_or(model::DEFAULT_SFZ_SAMPLE_FRAMES as i64)
                    as usize,
                source_overlays,
                program,
                path: resource_root
                    .join(text(r, "path", "")?)
                    .display()
                    .to_string(),
                defines: r
                    .get("defines")
                    .map(|v| serde_json::from_value(v.json()))
                    .transpose()?
                    .unwrap_or_default(),
                max_voices: voices,
                seed: r
                    .get("seed")
                    .map(|v| v.integer_in(0, i64::MAX))
                    .transpose()?
                    .unwrap_or(0) as u64,
                embed_assets: r.get("embed_assets").is_some_and(|v| v.truth()),
            })
        } else {
            None
        },
        asset_versions: Vec::new(),
        patch: if ty == "voice_patch" {
            Some(crate::patch_source::lower(r).map_err(|e| {
                lang::Diagnostic::new(format!("device {id}, patch {name}: {e:#}"))
                    .origin(r.origin())
                    .err()
            })?)
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
            // SFZ controllers and voice-patch controls are validated from the
            // device below, rather than a fixed native parameter table.
            if matches!(
                d.kind,
                DeviceKind::Vst3
                    | DeviceKind::Clap
                    | DeviceKind::Rack
                    | DeviceKind::VoicePatch
                    | DeviceKind::Sfz
            ) {
                continue;
            }
            let specs = crate::description::parameter_specs(d.kind);
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
        crate::description::validate_device(d).map_err(|e| {
            lang::Diagnostic::new(format!("{}: {e:#}", d.id))
                .origin(origins.get("device", d.id.as_str()))
                .err()
        })?;
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
        if let (Some(lo), Some(hi)) = (pitches.first(), pitches.last())
            && (hi - lo) as f64 > reach
        {
            messages.push((
                "piano.reach",
                format!("held span {} exceeds configured reach {reach}", hi - lo),
            ));
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
        if let Some((at, pitch)) = last[h]
            && time > at + 1e-6
            && (n.pitch - pitch).abs() > reach + movement * (time - at)
        {
            messages.push((
                "piano.movement",
                "hand movement exceeds configured rate".into(),
            ));
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

/// Call each source function in the track's `checks` with the logical track's
/// performed note records (all kit voices together, after clock resolution and
/// piano allocation), each carrying `track`. Values only flow out as
/// diagnostics, so a check cannot change the music.
fn run_checks(
    id: &str,
    tr: &Record,
    p: &Pattern,
    evaluator: &mut Option<lang::Evaluator>,
    out: &mut Vec<Diagnostic>,
) -> Result<()> {
    let checks = list(tr, "checks")?;
    if checks.is_empty() {
        return Ok(());
    }
    let notes = Value::Array(p.notes.iter().map(|n| check_note(n, id)).collect());
    let evaluator = evaluator.get_or_insert_with(lang::Evaluator::new);
    for (i, check) in checks.iter().enumerate() {
        let name = check_name(check, &format!("checks[{i}]")).ok_or_else(|| {
            lang::Diagnostic::new(format!(
                "track '{id}' checks[{i}] must be a function, got {}",
                check.kind()
            ))
            .help("pass functions such as `checks: [monophonic()]` from std/checks, or `fn(notes) => [...]`")
            .origin(tr.origin())
            .err()
        })?;
        let context = format!("check {name} on track '{id}'");
        run_check(check, &context, &notes, tr.origin(), id, evaluator, out)?;
    }
    Ok(())
}

/// A note record as checks see it: `pattern.notes` plus its logical `track`.
fn check_note(n: &music::Note, track: &str) -> Value {
    let Value::Record(r) = lang::note_value(n) else {
        unreachable!("note_value returns a record")
    };
    let mut fields: BTreeMap<String, Value> =
        r.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    fields.insert("track".into(), Value::Str(track.into()));
    Value::Record(std::rc::Rc::new(Record::new(fields)))
}

fn check_name(check: &Value, slot: &str) -> Option<String> {
    match check {
        Value::Function(f) => Some(match &f.name {
            Some(name) => format!("'{name}' ({slot})"),
            None => slot.into(),
        }),
        Value::Builtin(name, _) => Some(format!("'{name}' ({slot})")),
        _ => None,
    }
}

/// Song `checks: [{tracks: [...], check}]` see the named tracks' performed
/// notes merged in onset order (stable, so simultaneous notes keep track
/// order), each carrying its `track`, and report under the joined track names.
fn run_song_checks(
    r: &Record,
    score: &[ScoreTrack],
    evaluator: &mut Option<lang::Evaluator>,
    out: &mut Vec<Diagnostic>,
) -> Result<()> {
    for (i, entry) in list(r, "checks")?.iter().enumerate() {
        let er = entry.record().map_err(|error| field(r, "checks", error))?;
        fields(er, &["tracks", "check"], "song check")?;
        let mut names: Vec<&str> = vec![];
        for v in list(er, "tracks")? {
            let name = v.text().map_err(|error| field(er, "tracks", error))?;
            if !score.iter().any(|t| t.id == name) {
                return Err(lang::Diagnostic::new(format!(
                    "song checks[{i}] names unknown track '{name}'"
                ))
                .helps(lang::suggest_vocabulary(
                    "tracks",
                    name,
                    score.iter().map(|t| t.id.as_str()),
                ))
                .origin(er.origin())
                .err());
            }
            if !names.contains(&name) {
                names.push(name);
            }
        }
        if names.is_empty() {
            return Err(lang::Diagnostic::new(format!(
                "song checks[{i}] needs at least one track in 'tracks'"
            ))
            .help("write `{tracks: [\"violin\", \"violin_spiccato\"], check: monophonic()}`")
            .origin(er.origin())
            .err());
        }
        let check = req(er, "check")?;
        let name = check_name(check, &format!("checks[{i}]")).ok_or_else(|| {
            lang::Diagnostic::new(format!(
                "song checks[{i}] 'check' must be a function, got {}",
                check.kind()
            ))
            .help("pass functions such as `check: monophonic()` from std/checks, or `fn(notes) => [...]`")
            .origin(er.origin())
            .err()
        })?;
        let mut notes = score
            .iter()
            .filter(|t| names.contains(&t.id.as_str()))
            .flat_map(|t| t.pattern.notes.iter().map(|n| (n.at, check_note(n, &t.id))))
            .collect::<Vec<_>>();
        notes.sort_by(|a, b| a.0.cmp(&b.0));
        let notes = Value::Array(notes.into_iter().map(|(_, v)| v).collect());
        let evaluator = evaluator.get_or_insert_with(lang::Evaluator::new);
        let quoted = names.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>();
        let context = format!("song check {name} on tracks {}", quoted.join(", "));
        run_check(
            check,
            &context,
            &notes,
            er.origin(),
            &names.join("+"),
            evaluator,
            out,
        )?;
    }
    Ok(())
}

fn run_check(
    check: &Value,
    context: &str,
    notes: &Value,
    origin: Option<&Origin>,
    track: &str,
    evaluator: &mut lang::Evaluator,
    out: &mut Vec<Diagnostic>,
) -> Result<()> {
    let returned = evaluator
        .call(check.clone(), vec![(None, notes.clone())])
        .map_err(|error| {
            lang::Diagnostic::locate(lang::Diagnostic::within(error, context), origin)
        })?;
    let malformed = |problem: String| {
        lang::Diagnostic::new(format!("{context} returned {problem}"))
            .help("a check returns a list of {code, message, at?, severity?} records; `at` is a musical time and `severity` is \"warning\" or \"error\"")
            .origin(origin)
            .err()
    };
    let Value::Array(items) = &returned else {
        return Err(malformed(format!("{}, not a list", returned.kind())));
    };
    for (j, item) in items.iter().enumerate() {
        let Value::Record(d) = item else {
            return Err(malformed(format!("{} at [{j}], not a record", item.kind())));
        };
        if let Some(key) = d
            .keys()
            .find(|k| !matches!(k.as_str(), "code" | "message" | "at" | "severity"))
        {
            return Err(malformed(format!("unknown field '{key}' at [{j}]")));
        }
        let code = match d.get("code") {
            Some(Value::Str(code))
                if !code.is_empty()
                    && code
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) =>
            {
                code
            }
            _ => {
                return Err(malformed(format!(
                    "no valid 'code' at [{j}] (a text of letters, digits, '.', '_' or '-')"
                )));
            }
        };
        let Some(Value::Str(message)) = d.get("message") else {
            return Err(malformed(format!("no text 'message' at [{j}]")));
        };
        let beat = match d.get("at") {
            None | Some(Value::Null) => 0.0,
            Some(Value::Num(q)) if matches!(q.unit, Unit::Beat | Unit::Bar) => real(
                q.beats(4.0)
                    .map_err(|error| malformed(format!("invalid 'at' at [{j}]: {error:#}")))?,
            ),
            Some(other) => {
                return Err(malformed(format!(
                    "'at' {} at [{j}], not a musical time such as 3b or 2bar",
                    other.kind()
                )));
            }
        };
        let severity = match d.get("severity") {
            None | Some(Value::Null) => "warning",
            Some(Value::Str(s)) if s == "warning" || s == "error" => s,
            Some(_) => {
                return Err(malformed(format!(
                    "an invalid 'severity' at [{j}]; use \"warning\" or \"error\""
                )));
            }
        };
        out.push(Diagnostic {
            severity: severity.into(),
            code: format!("check.{code}"),
            track: track.into(),
            beat,
            message: message.clone(),
        });
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
                    if key == "keys" { Ok([vs[0].integer_in(0,127)? as f64,vs[1].integer_in(0,127)? as f64]) } else if key=="loop" {Ok([vs[0].quantity(Unit::Seconds,1.)?,vs[1].quantity(Unit::Seconds,1.)?])} else { Ok([vs[0].scalar()?, vs[1].scalar()?]) }
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
