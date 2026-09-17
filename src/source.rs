use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    lang::{Diagnostic, Location, suggest_vocabulary},
    midi::{MidiImportError, import_midi_file},
    model::{
        self, Bus, Device, DeviceKind, Id, MeterSource, MidiTrackSource, Note, Pattern, Route,
        Session, Track, TrackSource, Transport, TransportMode, Vst3Config,
    },
};

mod locations;

const MIN_BPM: f64 = 20.0;
const MAX_BPM: f64 = 400.0;
const MIN_ROUTE_GAIN_DB: f32 = -120.0;
const MAX_ROUTE_GAIN_DB: f32 = 24.0;
const MAX_EVENTS_PER_BLOCK: usize = 256;
const MAX_ENV_NAME_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 128;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("could not read project `{path}`: {source}")]
    ReadProject {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Decode(Diagnostic),
    #[error("invalid session: {0}")]
    Validation(Diagnostic),
    #[error("could not import MIDI asset `{path}`: {source}")]
    Midi {
        path: PathBuf,
        #[source]
        source: MidiImportError,
    },
}

/// Reads and parses a project, resolving every asset relative to the project file.
pub fn parse_project(path: impl AsRef<Path>) -> Result<Session, SourceError> {
    let path = path.as_ref();
    if path.extension().is_some_and(|s| s == "muz") {
        return crate::compile::compile(path)
            .map(|c| c.session)
            .map_err(|e| {
                SourceError::Validation(match e.downcast::<Diagnostic>() {
                    Ok(diagnostic) => diagnostic,
                    Err(e) => Diagnostic::new(format!("{e:#}")),
                })
            });
    }
    let source = fs::read_to_string(path).map_err(|source| SourceError::ReadProject {
        path: path.to_path_buf(),
        source,
    })?;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    parse_session(&source, Some(path), root)
}

/// Parses project text with an explicit asset root. This is the entry point for
/// editors that already own the source text and for deterministic inline tests.
pub fn parse_session_with_root(
    source: &str,
    root: impl AsRef<Path>,
) -> Result<Session, SourceError> {
    parse_session(source, None, root.as_ref())
}

fn parse_session(source: &str, path: Option<&Path>, root: &Path) -> Result<Session, SourceError> {
    let raw: RawSession = json5::from_str(source)
        .map_err(|error| SourceError::Decode(decode_error(source, path, &error)))?;
    let mut validator = Validator::new(root);
    validator.validate(raw).map_err(|error| match error {
        SourceError::Validation(diagnostic) => {
            let file = path.unwrap_or_else(|| Path::new("<project>"));
            let origin = locations::origin(source, file, &validator.position);
            SourceError::Validation(
                diagnostic
                    .help(format!(
                        "field: {}",
                        if validator.position.is_empty() {
                            "$"
                        } else {
                            &validator.position
                        }
                    ))
                    .origin(origin.as_ref())
                    .path(file),
            )
        }
        error => error,
    })
}

/// Attach a JSON5 failure to the line and column the parser reported, so the
/// message names `path:line:column` when the source file is known.
fn decode_error(source: &str, path: Option<&Path>, error: &json5::Error) -> Diagnostic {
    // The JSON5 message ends with its own position; the rendered location names
    // it more precisely, so drop the redundant suffix.
    let mut message = error.to_string();
    match (path, error.position()) {
        (Some(path), Some(position)) => {
            let suffix = format!(
                " at line {} column {}",
                position.line + 1,
                position.column + 1
            );
            if let Some(stripped) = message.strip_suffix(&suffix) {
                message.truncate(stripped.len());
            }
            let at = offset_of(source, position.line, position.column);
            Diagnostic::at(
                Location::span(path, source, at, at),
                format!("invalid JSON5: {message}"),
            )
        }
        _ => Diagnostic::new(format!("invalid JSON5: {message}")),
    }
}

/// Byte offset of a 0-based JSON5 line and column, clamped to the source text.
/// JSON5 counts `\n`, `\r`, `\r\n` and the Unicode separators as line breaks.
fn offset_of(source: &str, line: usize, column: usize) -> usize {
    let mut start = 0;
    for _ in 0..line {
        let Some(next) = line_end(source, start) else {
            return source.len();
        };
        start = next;
    }
    let end = line_end(source, start).unwrap_or(source.len());
    let text = source[start..end]
        .trim_end_matches('\n')
        .trim_end_matches('\r');
    text.char_indices()
        .nth(column)
        .map_or(start + text.len(), |(byte, _)| start + byte)
}

/// Byte offset just past the line break that ends the line starting at `start`.
fn line_end(source: &str, start: usize) -> Option<usize> {
    let mut chars = source[start..].char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\n' => return Some(start + at + 1),
            '\r' => {
                return Some(match chars.next() {
                    Some((next_at, '\n')) if next_at == at + 1 => start + at + 2,
                    _ => start + at + 1,
                });
            }
            '\u{2028}' | '\u{2029}' => return Some(start + at + c.len_utf8()),
            _ => {}
        }
    }
    None
}

/// Resolves an authored project-relative asset reference without permitting an
/// absolute path, parent traversal, or a symlink escape from an existing root.
pub fn resolve_project_asset(root: &Path, reference: &str) -> Result<PathBuf, SourceError> {
    if reference.is_empty() {
        return validation("asset reference must not be empty");
    }
    let relative = Path::new(reference);
    if relative.is_absolute() {
        return validation(format!(
            "asset reference '{reference}' must be project-relative"
        ));
    }

    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return validation(format!(
                    "asset reference '{reference}' must not contain traversal or a root"
                ));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return validation("asset reference must name a file");
    }

    let resolved = root.join(normalized);
    if let (Ok(canonical_root), Ok(canonical_asset)) =
        (fs::canonicalize(root), fs::canonicalize(&resolved))
        && !canonical_asset.starts_with(&canonical_root)
    {
        return validation(format!(
            "asset reference '{reference}' resolves outside the project root"
        ));
    }
    Ok(resolved)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSession {
    schema: u32,
    transport: RawTransport,
    master: RawBus,
    #[serde(default)]
    buses: Vec<RawBus>,
    #[serde(default)]
    tracks: Vec<RawTrack>,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum RawTransport {
    Loop {
        bpm: f64,
        meter: [u8; 2],
        loop_beats: f64,
    },
    OneShot {
        meter: [u8; 2],
        meter_source: MeterSource,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTrack {
    id: String,
    #[serde(default)]
    name: String,
    source: RawTrackSource,
    instrument: RawDevice,
    #[serde(default)]
    inserts: Vec<RawDevice>,
    output: RawRoute,
    #[serde(default)]
    sends: Vec<RawRoute>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RawTrackSource {
    Pattern {
        id: String,
        #[serde(default)]
        notes: Vec<RawNote>,
    },
    Midi {
        #[serde(default)]
        id: Option<String>,
        asset: String,
        channel: u8,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBus {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    inserts: Vec<RawDevice>,
    #[serde(default)]
    output: Option<RawRoute>,
    #[serde(default)]
    sends: Vec<RawRoute>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNote {
    id: String,
    at: f64,
    duration: f64,
    key: u8,
    velocity: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDevice {
    id: String,
    kind: DeviceKind,
    #[serde(default)]
    params: BTreeMap<String, f32>,
    plugin: Option<RawVst3Config>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVst3Config {
    bundle_env: String,
    class_id: String,
    expected_version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRoute {
    id: String,
    to: String,
    gain_db: f32,
}

struct Validator<'a> {
    root: &'a Path,
    ids: BTreeMap<String, String>,
    // Set before each semantic check; on failure this identifies its authored
    // field even after conversion has discarded the raw record.
    position: String,
    note_count: usize,
}

impl<'a> Validator<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            ids: BTreeMap::new(),
            position: String::new(),
            note_count: 0,
        }
    }

    fn validate(&mut self, raw: RawSession) -> Result<Session, SourceError> {
        self.position = "schema".into();
        if raw.schema != model::SCHEMA_VERSION {
            return diagnose(
                Diagnostic::new(format!(
                    "unsupported schema {}; expected {}",
                    raw.schema,
                    model::SCHEMA_VERSION
                ))
                .help(format!(
                    "update the project file to schema {}",
                    model::SCHEMA_VERSION
                )),
            );
        }
        self.position = "master.output".into();
        if raw.master.output.is_some() {
            return validation("master must not have an output");
        }
        self.position = "master.sends".into();
        if !raw.master.sends.is_empty() {
            return validation("master must not have sends");
        }
        if let Some(i) = raw.buses.iter().position(|bus| bus.output.is_none()) {
            self.position = format!("buses[{i}].output");
            return validation("every non-master bus must have an output");
        }

        let transport = validate_transport(raw.transport, &mut self.position)?;
        let master = self.convert_bus(raw.master, "master")?;
        let mut buses = Vec::with_capacity(raw.buses.len());
        for (i, bus) in raw.buses.into_iter().enumerate() {
            buses.push(self.convert_bus(bus, &format!("buses[{i}]"))?);
        }
        let mut tracks = Vec::with_capacity(raw.tracks.len());
        for (i, track) in raw.tracks.into_iter().enumerate() {
            tracks.push(self.convert_track(track, &transport, &format!("tracks[{i}]"))?);
        }

        self.position = "tracks".into();
        let event_count = self
            .note_count
            .checked_mul(2)
            .ok_or_else(|| invalid("note event count overflow"))?;
        if event_count > MAX_EVENTS_PER_BLOCK {
            return validation(format!(
                "session can produce {event_count} note onsets and offs in one block; maximum is {MAX_EVENTS_PER_BLOCK}"
            ));
        }
        validate_routes_and_graph(&master, &buses, &tracks, &self.ids, &mut self.position)?;

        let session = Session {
            extras: Default::default(),
            transport,
            master,
            buses,
            tracks,
        };
        self.position.clear();
        session.validate_graph_budget().map_err(invalid)?;
        Ok(session)
    }

    fn convert_bus(&mut self, raw: RawBus, path: &str) -> Result<Bus, SourceError> {
        let id = self.take_id(raw.id, &format!("{path}.id"))?;
        let mut inserts = Vec::with_capacity(raw.inserts.len());
        for (i, raw_device) in raw.inserts.into_iter().enumerate() {
            let device = self.convert_device(raw_device, &format!("{path}.inserts[{i}]"))?;
            self.position = format!("{path}.inserts[{i}].kind");
            if device.kind.is_instrument() {
                return validation(format!(
                    "insert device '{}' must not be an instrument",
                    device.id
                ));
            }
            inserts.push(device);
        }
        let output = raw
            .output
            .map(|route| self.convert_route(route, &format!("{path}.output")))
            .transpose()?;
        let mut sends = Vec::with_capacity(raw.sends.len());
        for (i, route) in raw.sends.into_iter().enumerate() {
            sends.push(self.convert_route(route, &format!("{path}.sends[{i}]"))?);
        }
        Ok(Bus {
            id,
            name: raw.name,
            inserts,
            output,
            sends,
        })
    }

    fn convert_track(
        &mut self,
        raw: RawTrack,
        transport: &Transport,
        path: &str,
    ) -> Result<Track, SourceError> {
        let id = self.take_id(raw.id, &format!("{path}.id"))?;
        self.position = format!("{path}.source.kind");
        let source = match (transport.mode(), raw.source) {
            (TransportMode::Loop, RawTrackSource::Pattern { id, notes }) => TrackSource::Pattern(
                self.convert_pattern(id, notes, transport.loop_ticks(), &format!("{path}.source"))?,
            ),
            (
                TransportMode::OneShot,
                RawTrackSource::Midi {
                    id: source_id,
                    asset,
                    channel,
                },
            ) => {
                let source_id = source_id.unwrap_or_else(|| format!("{}.source", id.as_str()));
                TrackSource::Midi(self.convert_midi(
                    source_id,
                    asset,
                    channel,
                    &format!("{path}.source"),
                )?)
            }
            (TransportMode::Loop, RawTrackSource::Midi { .. }) => {
                return validation("loop transport requires pattern track sources");
            }
            (TransportMode::OneShot, RawTrackSource::Pattern { .. }) => {
                return validation("one_shot transport requires MIDI track sources");
            }
        };

        let instrument = self.convert_device(raw.instrument, &format!("{path}.instrument"))?;
        self.position = format!("{path}.instrument.kind");
        if !instrument.kind.is_instrument() {
            return diagnose(
                Diagnostic::new(format!(
                    "track instrument '{}' must be an instrument, not an effect",
                    instrument.id
                ))
                .help(concat!(
                    "accepted instrument kinds: builtin.voice_patch, builtin.sampler, ",
                    "builtin.poly_synth, builtin.studio_synth, vst3, clap"
                )),
            );
        }
        let mut inserts = Vec::with_capacity(raw.inserts.len());
        for (i, raw_device) in raw.inserts.into_iter().enumerate() {
            let device = self.convert_device(raw_device, &format!("{path}.inserts[{i}]"))?;
            self.position = format!("{path}.inserts[{i}].kind");
            if device.kind.is_instrument() {
                return validation(format!(
                    "insert device '{}' must not be an instrument",
                    device.id
                ));
            }
            inserts.push(device);
        }
        let output = self.convert_route(raw.output, &format!("{path}.output"))?;
        let mut sends = Vec::with_capacity(raw.sends.len());
        for (i, route) in raw.sends.into_iter().enumerate() {
            sends.push(self.convert_route(route, &format!("{path}.sends[{i}]"))?);
        }
        Ok(Track {
            id,
            name: raw.name,
            source,
            instrument,
            inserts,
            output,
            sends,
        })
    }

    fn convert_pattern(
        &mut self,
        raw_id: String,
        raw_notes: Vec<RawNote>,
        loop_ticks: u64,
        path: &str,
    ) -> Result<Pattern, SourceError> {
        let id = self.take_id(raw_id, &format!("{path}.id"))?;
        let mut notes = Vec::with_capacity(raw_notes.len());
        for (i, raw_note) in raw_notes.into_iter().enumerate() {
            let path = format!("{path}.notes[{i}]");
            self.position = path.clone();
            self.bump_notes()?;
            let note_id = self.take_id(raw_note.id, &format!("{path}.id"))?;
            self.position = format!("{path}.at");
            let start_ticks = beats_to_ticks(raw_note.at, "note at", true)?;
            self.position = format!("{path}.duration");
            let duration_ticks = beats_to_ticks(raw_note.duration, "note duration", false)?;
            let end_ticks = start_ticks
                .checked_add(duration_ticks)
                .ok_or_else(|| invalid(format!("note '{note_id}' range overflows")))?;
            self.position = if start_ticks >= loop_ticks {
                format!("{path}.at")
            } else {
                format!("{path}.duration")
            };
            if start_ticks >= loop_ticks || end_ticks > loop_ticks {
                return validation(format!(
                    "note '{note_id}' must start and end within the transport loop"
                ));
            }
            self.position = format!("{path}.key");
            if raw_note.key > 127 {
                return validation(format!("note '{note_id}' key must be in 0..=127"));
            }
            self.position = format!("{path}.velocity");
            if !raw_note.velocity.is_finite() || !(0.0..=1.0).contains(&raw_note.velocity) {
                return validation(format!(
                    "note '{note_id}' velocity must be finite and in 0..=1"
                ));
            }
            notes.push(Note {
                id: note_id,
                start_ticks,
                duration_ticks,
                key: raw_note.key,
                velocity: raw_note.velocity,
            });
        }
        Ok(Pattern { id, notes })
    }

    fn convert_midi(
        &mut self,
        raw_id: String,
        asset: String,
        channel: u8,
        path: &str,
    ) -> Result<MidiTrackSource, SourceError> {
        let id = self.take_id(raw_id, &format!("{path}.id"))?;
        self.position = format!("{path}.channel");
        if channel > 15 {
            return validation(format!("MIDI source '{id}' channel must be in 0..=15"));
        }
        self.position = format!("{path}.asset");
        if asset.len() > 1_024 {
            return validation(format!("MIDI source '{id}' asset reference is too long"));
        }
        let path = resolve_project_asset(self.root, &asset)?;
        let imported = import_midi_file(&path).map_err(|source| SourceError::Midi {
            path: path.clone(),
            source,
        })?;
        let summary = imported.summary.clone();
        Ok(MidiTrackSource {
            all_channels: false,
            id,
            asset,
            channel,
            summary,
            imported,
        })
    }

    fn convert_device(&mut self, raw: RawDevice, path: &str) -> Result<Device, SourceError> {
        let id = self.take_id(raw.id, &format!("{path}.id"))?;
        self.position = format!("{path}.plugin");
        let vst3 = match (raw.kind, raw.plugin) {
            (DeviceKind::Vst3, Some(plugin)) => Some(validate_vst3(
                &id,
                plugin,
                &format!("{path}.plugin"),
                &mut self.position,
            )?),
            (DeviceKind::Vst3, None) => {
                return validation(format!("VST3 device '{id}' requires plugin configuration"));
            }
            (_, Some(_)) => {
                return validation(format!("built-in device '{id}' must not declare plugin"));
            }
            (_, None) => None,
        };
        let specs = parameter_specs(raw.kind);
        for (name, value) in &raw.params {
            self.position = locations::field(&format!("{path}.params"), name);
            let Some(spec) = specs.iter().find(|spec| spec.name == name) else {
                let mut diagnostic =
                    Diagnostic::new(format!("device '{id}' has unknown parameter '{name}'"));
                if !specs.is_empty() {
                    diagnostic = diagnostic.helps(suggest_vocabulary(
                        "parameters",
                        name,
                        specs.iter().map(|spec| spec.name),
                    ));
                }
                return diagnose(diagnostic);
            };
            if !value.is_finite() || *value < spec.min || *value > spec.max {
                return validation(format!(
                    "device '{id}' parameter '{name}' must be finite and in {}..={}",
                    spec.min, spec.max
                ));
            }
            if raw.kind == DeviceKind::StudioSynth
                && matches!(name.as_str(), "mode" | "unison")
                && value.fract() != 0.0
            {
                return validation(format!(
                    "device '{id}' parameter '{name}' must be an integer"
                ));
            }
        }
        let mut params = specs
            .iter()
            .map(|spec| (spec.name.to_owned(), spec.default))
            .collect::<BTreeMap<_, _>>();
        params.extend(raw.params);
        Ok(Device {
            asset_versions: Vec::new(),
            patch: None,
            generation: 0,
            rack: None,
            sample: None,
            sidechain: None,
            id,
            kind: raw.kind,
            params,
            vst3,
        })
    }

    fn convert_route(&mut self, raw: RawRoute, path: &str) -> Result<Route, SourceError> {
        let id = self.take_id(raw.id, &format!("{path}.id"))?;
        self.position = format!("{path}.to");
        validate_id(&raw.to)?;
        self.position = format!("{path}.gain_db");
        if !raw.gain_db.is_finite()
            || !(MIN_ROUTE_GAIN_DB..=MAX_ROUTE_GAIN_DB).contains(&raw.gain_db)
        {
            return validation(format!(
                "route '{id}' gain_db must be finite and in {MIN_ROUTE_GAIN_DB}..={MAX_ROUTE_GAIN_DB}"
            ));
        }
        Ok(Route {
            pre: true,
            id,
            to: Id::new(raw.to),
            gain_db: raw.gain_db,
        })
    }

    fn take_id(&mut self, value: String, path: &str) -> Result<Id, SourceError> {
        self.position = path.into();
        validate_id(&value)?;
        if let Some(first) = self.ids.get(&value) {
            return diagnose(
                Diagnostic::new(format!("duplicate global id '{value}'")).help(format!(
                    "'{value}' is already used by another bus, track, device, note, pattern, MIDI source, or route"
                )).help(format!("first declared at {first}")),
            );
        }
        self.ids.insert(value.clone(), path.into());
        Ok(Id::new(value))
    }

    fn bump_notes(&mut self) -> Result<(), SourceError> {
        self.note_count += 1;
        if self.note_count > model::MAX_NOTES {
            return validation(format!("note count exceeds capacity {}", model::MAX_NOTES));
        }
        Ok(())
    }
}

pub use crate::description::{ParameterSpec, parameter_specs};

fn validate_vst3(
    id: &Id,
    raw: RawVst3Config,
    path: &str,
    position: &mut String,
) -> Result<Vst3Config, SourceError> {
    *position = format!("{path}.bundle_env");
    let env = raw.bundle_env.as_bytes();
    if env.is_empty()
        || env.len() > MAX_ENV_NAME_BYTES
        || !(env[0].is_ascii_alphabetic() || env[0] == b'_')
        || env[1..]
            .iter()
            .any(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
    {
        return validation(format!(
            "VST3 device '{id}' bundle_env must be an environment variable name"
        ));
    }
    *position = format!("{path}.class_id");
    if raw.class_id.len() != 32 || !raw.class_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return validation(format!(
            "VST3 device '{id}' class_id must contain exactly 32 hexadecimal digits"
        ));
    }
    *position = format!("{path}.expected_version");
    if raw.expected_version.is_empty()
        || raw.expected_version.len() > MAX_VERSION_BYTES
        || raw
            .expected_version
            .bytes()
            .any(|byte| !byte.is_ascii_graphic())
    {
        return validation(format!(
            "VST3 device '{id}' expected_version must be 1..={MAX_VERSION_BYTES} visible ASCII bytes"
        ));
    }
    Ok(Vst3Config {
        state: None,
        bundle_env: raw.bundle_env,
        class_id: raw.class_id,
        expected_version: raw.expected_version,
    })
}

fn validate_transport(raw: RawTransport, position: &mut String) -> Result<Transport, SourceError> {
    match raw {
        RawTransport::Loop {
            bpm,
            meter,
            loop_beats,
        } => {
            *position = "transport.meter".into();
            validate_meter(meter)?;
            *position = "transport.bpm".into();
            if !bpm.is_finite() || !(MIN_BPM..=MAX_BPM).contains(&bpm) {
                return validation(format!(
                    "transport bpm must be finite and in {MIN_BPM}..={MAX_BPM}"
                ));
            }
            *position = "transport.loop_beats".into();
            Ok(Transport::Loop {
                bpm,
                meter,
                loop_ticks: beats_to_ticks(loop_beats, "transport loop_beats", false)?,
            })
        }
        RawTransport::OneShot {
            meter,
            meter_source,
        } => {
            *position = "transport.meter".into();
            validate_meter(meter)?;
            Ok(Transport::OneShot {
                meter,
                meter_source,
            })
        }
    }
}

fn validate_meter(meter: [u8; 2]) -> Result<(), SourceError> {
    let [numerator, denominator] = meter;
    if numerator == 0
        || numerator > 32
        || denominator == 0
        || denominator > 32
        || !denominator.is_power_of_two()
    {
        return validation(
            "transport meter must have numerator 1..=32 and power-of-two denominator 1..=32",
        );
    }
    Ok(())
}

fn beats_to_ticks(value: f64, field: &str, allow_zero: bool) -> Result<u64, SourceError> {
    if !value.is_finite() || value < 0.0 || (!allow_zero && value == 0.0) {
        return validation(format!(
            "{field} must be finite and {}",
            if allow_zero {
                "nonnegative"
            } else {
                "positive"
            }
        ));
    }
    let ticks = value * f64::from(model::TICKS_PER_BEAT);
    if !ticks.is_finite() || ticks >= u64::MAX as f64 {
        return validation(format!("{field} is out of tick range"));
    }
    let rounded = ticks.round();
    if !allow_zero && rounded == 0.0 {
        return validation(format!("{field} is shorter than one tick"));
    }
    Ok(rounded as u64)
}

fn validate_id(value: &str) -> Result<(), SourceError> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 128
        || !bytes[0].is_ascii_alphanumeric()
        || bytes[1..]
            .iter()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return validation(format!(
            "invalid id '{value}'; expected [A-Za-z0-9][A-Za-z0-9._:-]{{0,127}}"
        ));
    }
    Ok(())
}

fn validate_routes_and_graph(
    master: &Bus,
    buses: &[Bus],
    tracks: &[Track],
    ids: &BTreeMap<String, String>,
    position: &mut String,
) -> Result<(), SourceError> {
    let mut bus_indices = BTreeMap::new();
    bus_indices.insert(master.id.as_str(), 0usize);
    for (index, bus) in buses.iter().enumerate() {
        bus_indices.insert(bus.id.as_str(), index + 1);
    }

    let mut adjacency = vec![Vec::new(); buses.len() + 1];
    for (index, bus) in buses.iter().enumerate() {
        for route in bus.output.iter().chain(&bus.sends) {
            let Some(&target) = bus_indices.get(route.to.as_str()) else {
                *position =
                    locations::field(ids[route.id.as_str()].strip_suffix(".id").unwrap(), "to");
                return Err(undeclared_bus(route, &bus_indices));
            };
            adjacency[index + 1].push(target);
        }
    }
    for track in tracks {
        for route in std::iter::once(&track.output).chain(&track.sends) {
            if !bus_indices.contains_key(route.to.as_str()) {
                *position =
                    locations::field(ids[route.id.as_str()].strip_suffix(".id").unwrap(), "to");
                return Err(undeclared_bus(route, &bus_indices));
            }
        }
    }

    *position = "buses".into();
    let mut states = vec![0u8; adjacency.len()];
    for node in 0..adjacency.len() {
        visit_bus(node, &adjacency, &mut states)?;
    }
    Ok(())
}

fn visit_bus(node: usize, adjacency: &[Vec<usize>], states: &mut [u8]) -> Result<(), SourceError> {
    match states[node] {
        1 => return validation("bus output/send graph contains a cycle"),
        2 => return Ok(()),
        _ => {}
    }
    states[node] = 1;
    for &target in &adjacency[node] {
        visit_bus(target, adjacency, states)?;
    }
    states[node] = 2;
    Ok(())
}

fn invalid(message: impl Into<String>) -> SourceError {
    SourceError::Validation(Diagnostic::new(message.into()))
}

fn validation<T>(message: impl Into<String>) -> Result<T, SourceError> {
    Err(invalid(message))
}

/// A validation failure whose message already carries mechanical `help:` lines.
fn diagnose<T>(diagnostic: Diagnostic) -> Result<T, SourceError> {
    Err(SourceError::Validation(diagnostic))
}

/// A route to a bus that was never declared, naming the declared bus ids.
fn undeclared_bus(route: &Route, buses: &BTreeMap<&str, usize>) -> SourceError {
    SourceError::Validation(
        Diagnostic::new(format!(
            "route '{}' targets undeclared bus '{}'",
            route.id, route.to
        ))
        .helps(suggest_vocabulary(
            "buses",
            route.to.as_str(),
            buses.keys().copied(),
        )),
    )
}
