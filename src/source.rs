use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    midi::{MidiImportError, import_midi_file},
    model::{
        self, Bus, Device, DeviceKind, Id, MeterSource, MidiTrackSource, Note, Pattern, Route,
        Session, Track, TrackSource, Transport, TransportMode, Vst3Config,
    },
};

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
    #[error("invalid JSON5: {0}")]
    Decode(String),
    #[error("invalid session: {0}")]
    Validation(String),
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
            .map_err(|e| SourceError::Validation(format!("{e:#}")));
    }
    let source = fs::read_to_string(path).map_err(|source| SourceError::ReadProject {
        path: path.to_path_buf(),
        source,
    })?;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    parse_session_with_root(&source, root)
}

/// Parses project text with an explicit asset root. This is the entry point for
/// editors that already own the source text and for deterministic inline tests.
pub fn parse_session_with_root(
    source: &str,
    root: impl AsRef<Path>,
) -> Result<Session, SourceError> {
    let raw: RawSession =
        json5::from_str(source).map_err(|error| SourceError::Decode(error.to_string()))?;
    Validator::new(root.as_ref()).validate(raw)
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
    ids: BTreeSet<String>,
    device_count: usize,
    route_count: usize,
    note_count: usize,
}

impl<'a> Validator<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            ids: BTreeSet::new(),
            device_count: 0,
            route_count: 0,
            note_count: 0,
        }
    }

    fn validate(mut self, raw: RawSession) -> Result<Session, SourceError> {
        if raw.schema != model::SCHEMA_VERSION {
            return validation(format!(
                "unsupported schema {}; expected {}",
                raw.schema,
                model::SCHEMA_VERSION
            ));
        }
        if raw.buses.len() > model::MAX_BUSES {
            return validation(format!(
                "{} buses exceeds capacity {}",
                raw.buses.len(),
                model::MAX_BUSES
            ));
        }
        if raw.tracks.len() > model::MAX_TRACKS {
            return validation(format!(
                "{} tracks exceeds capacity {}",
                raw.tracks.len(),
                model::MAX_TRACKS
            ));
        }
        if raw.master.output.is_some() {
            return validation("master must not have an output");
        }
        if !raw.master.sends.is_empty() {
            return validation("master must not have sends");
        }
        if raw.buses.iter().any(|bus| bus.output.is_none()) {
            return validation("every non-master bus must have an output");
        }

        let transport = validate_transport(raw.transport)?;
        let master = self.convert_bus(raw.master)?;
        let mut buses = Vec::with_capacity(raw.buses.len());
        for bus in raw.buses {
            buses.push(self.convert_bus(bus)?);
        }
        let mut tracks = Vec::with_capacity(raw.tracks.len());
        for track in raw.tracks {
            tracks.push(self.convert_track(track, &transport)?);
        }

        let event_count = self
            .note_count
            .checked_mul(2)
            .ok_or_else(|| invalid("note event count overflow"))?;
        if event_count > MAX_EVENTS_PER_BLOCK {
            return validation(format!(
                "session can produce {event_count} note onsets and offs in one block; maximum is {MAX_EVENTS_PER_BLOCK}"
            ));
        }
        validate_routes_and_graph(&master, &buses, &tracks)?;

        Ok(Session {
            extras: Default::default(),
            transport,
            master,
            buses,
            tracks,
        })
    }

    fn convert_bus(&mut self, raw: RawBus) -> Result<Bus, SourceError> {
        let id = self.take_id(raw.id)?;
        let mut inserts = Vec::with_capacity(raw.inserts.len());
        for raw_device in raw.inserts {
            let device = self.convert_device(raw_device)?;
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
            .map(|route| self.convert_route(route))
            .transpose()?;
        let mut sends = Vec::with_capacity(raw.sends.len());
        for route in raw.sends {
            sends.push(self.convert_route(route)?);
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
    ) -> Result<Track, SourceError> {
        let id = self.take_id(raw.id)?;
        let source = match (transport.mode(), raw.source) {
            (TransportMode::Loop, RawTrackSource::Pattern { id, notes }) => {
                TrackSource::Pattern(self.convert_pattern(id, notes, transport.loop_ticks())?)
            }
            (
                TransportMode::OneShot,
                RawTrackSource::Midi {
                    id: source_id,
                    asset,
                    channel,
                },
            ) => {
                let source_id = source_id.unwrap_or_else(|| format!("{}.source", id.as_str()));
                TrackSource::Midi(self.convert_midi(source_id, asset, channel)?)
            }
            (TransportMode::Loop, RawTrackSource::Midi { .. }) => {
                return validation("loop transport requires pattern track sources");
            }
            (TransportMode::OneShot, RawTrackSource::Pattern { .. }) => {
                return validation("one_shot transport requires MIDI track sources");
            }
        };

        let instrument = self.convert_device(raw.instrument)?;
        if !instrument.kind.is_instrument() {
            return validation(format!(
                "track instrument '{}' must be builtin.poly_synth, builtin.studio_synth, or vst3",
                instrument.id
            ));
        }
        let mut inserts = Vec::with_capacity(raw.inserts.len());
        for raw_device in raw.inserts {
            let device = self.convert_device(raw_device)?;
            if device.kind.is_instrument() {
                return validation(format!(
                    "insert device '{}' must not be an instrument",
                    device.id
                ));
            }
            inserts.push(device);
        }
        let output = self.convert_route(raw.output)?;
        let mut sends = Vec::with_capacity(raw.sends.len());
        for route in raw.sends {
            sends.push(self.convert_route(route)?);
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
    ) -> Result<Pattern, SourceError> {
        let id = self.take_id(raw_id)?;
        let mut notes = Vec::with_capacity(raw_notes.len());
        for raw_note in raw_notes {
            self.bump_notes()?;
            let note_id = self.take_id(raw_note.id)?;
            let start_ticks = beats_to_ticks(raw_note.at, "note at", true)?;
            let duration_ticks = beats_to_ticks(raw_note.duration, "note duration", false)?;
            let end_ticks = start_ticks
                .checked_add(duration_ticks)
                .ok_or_else(|| invalid(format!("note '{note_id}' range overflows")))?;
            if start_ticks >= loop_ticks || end_ticks > loop_ticks {
                return validation(format!(
                    "note '{note_id}' must start and end within the transport loop"
                ));
            }
            if raw_note.key > 127 {
                return validation(format!("note '{note_id}' key must be in 0..=127"));
            }
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
    ) -> Result<MidiTrackSource, SourceError> {
        let id = self.take_id(raw_id)?;
        if channel > 15 {
            return validation(format!("MIDI source '{id}' channel must be in 0..=15"));
        }
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
            id,
            asset,
            channel,
            summary,
            imported,
        })
    }

    fn convert_device(&mut self, raw: RawDevice) -> Result<Device, SourceError> {
        self.bump_devices()?;
        let id = self.take_id(raw.id)?;
        let vst3 = match (raw.kind, raw.plugin) {
            (DeviceKind::Vst3, Some(plugin)) => Some(validate_vst3(&id, plugin)?),
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
            let Some(spec) = specs.iter().find(|spec| spec.name == name) else {
                return validation(format!("device '{id}' has unknown parameter '{name}'"));
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
            sample: None,
            sidechain: None,
            id,
            kind: raw.kind,
            params,
            vst3,
        })
    }

    fn convert_route(&mut self, raw: RawRoute) -> Result<Route, SourceError> {
        self.bump_routes()?;
        let id = self.take_id(raw.id)?;
        validate_id(&raw.to)?;
        if !raw.gain_db.is_finite()
            || !(MIN_ROUTE_GAIN_DB..=MAX_ROUTE_GAIN_DB).contains(&raw.gain_db)
        {
            return validation(format!(
                "route '{id}' gain_db must be finite and in {MIN_ROUTE_GAIN_DB}..={MAX_ROUTE_GAIN_DB}"
            ));
        }
        Ok(Route {
            id,
            to: Id::new(raw.to),
            gain_db: raw.gain_db,
        })
    }

    fn take_id(&mut self, value: String) -> Result<Id, SourceError> {
        validate_id(&value)?;
        if !self.ids.insert(value.clone()) {
            return validation(format!("duplicate global id '{value}'"));
        }
        Ok(Id::new(value))
    }

    fn bump_devices(&mut self) -> Result<(), SourceError> {
        self.device_count += 1;
        if self.device_count > model::MAX_DEVICES {
            return validation(format!(
                "device count exceeds capacity {}",
                model::MAX_DEVICES
            ));
        }
        Ok(())
    }

    fn bump_routes(&mut self) -> Result<(), SourceError> {
        self.route_count += 1;
        if self.route_count > model::MAX_ROUTES {
            return validation(format!(
                "route count exceeds capacity {}",
                model::MAX_ROUTES
            ));
        }
        Ok(())
    }

    fn bump_notes(&mut self) -> Result<(), SourceError> {
        self.note_count += 1;
        if self.note_count > model::MAX_NOTES {
            return validation(format!("note count exceeds capacity {}", model::MAX_NOTES));
        }
        Ok(())
    }
}

pub(crate) struct ParameterSpec {
    pub(crate) name: &'static str,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) default: f32,
}

const POLY_SYNTH_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "gain_db",
        min: -60.0,
        max: 12.0,
        default: -12.0,
    },
    ParameterSpec {
        name: "attack_ms",
        min: 0.0,
        max: 5_000.0,
        default: 10.0,
    },
    ParameterSpec {
        name: "release_ms",
        min: 1.0,
        max: 10_000.0,
        default: 250.0,
    },
];
const LOWPASS_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "cutoff_hz",
        min: 20.0,
        max: 20_000.0,
        default: 20_000.0,
    },
    ParameterSpec {
        name: "resonance",
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
];
const HIGHPASS_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "cutoff_hz",
        min: 20.0,
        max: 20_000.0,
        default: 20.0,
    },
    ParameterSpec {
        name: "resonance",
        min: 0.0,
        max: 1.0,
        default: 0.707,
    },
];
const DRIVE_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "drive_db",
        min: 0.0,
        max: 36.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 1.0,
    },
];
const GAIN_PARAMS: &[ParameterSpec] = &[ParameterSpec {
    name: "gain_db",
    min: -60.0,
    max: 12.0,
    default: 0.0,
}];
const DELAY_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "time_beats",
        min: 0.03125,
        max: 16.0,
        default: 0.5,
    },
    ParameterSpec {
        name: "feedback",
        min: 0.0,
        max: 0.99,
        default: 0.35,
    },
    ParameterSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.25,
    },
];
const COMPRESSOR_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "threshold_db",
        min: -60.0,
        max: 0.0,
        default: -18.0,
    },
    ParameterSpec {
        name: "ratio",
        min: 1.0,
        max: 20.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "attack_ms",
        min: 0.1,
        max: 200.0,
        default: 30.0,
    },
    ParameterSpec {
        name: "release_ms",
        min: 10.0,
        max: 2_000.0,
        default: 250.0,
    },
    ParameterSpec {
        name: "knee_db",
        min: 0.0,
        max: 24.0,
        default: 6.0,
    },
    ParameterSpec {
        name: "makeup_db",
        min: -24.0,
        max: 24.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 1.0,
    },
];
const LIMITER_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "lookahead_ms",
        min: 0.0,
        max: 20.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "ceiling_db",
        min: -24.0,
        max: 0.0,
        default: -1.0,
    },
    ParameterSpec {
        name: "release_ms",
        min: 1.0,
        max: 2_000.0,
        default: 100.0,
    },
];

pub(crate) fn parameter_specs(kind: DeviceKind) -> &'static [ParameterSpec] {
    match kind {
        DeviceKind::Sampler => SAMPLE_PARAMS,
        DeviceKind::Eq => EQ_PARAMS,
        DeviceKind::Chorus => CHORUS_PARAMS,
        DeviceKind::Gate => GATE_PARAMS,
        DeviceKind::StudioSynth => STUDIO_SYNTH_PARAMS,
        DeviceKind::Reverb => REVERB_PARAMS,
        DeviceKind::Stereo => STEREO_PARAMS,
        DeviceKind::PolySynth => POLY_SYNTH_PARAMS,
        DeviceKind::Lowpass => LOWPASS_PARAMS,
        DeviceKind::Highpass => HIGHPASS_PARAMS,
        DeviceKind::Drive => DRIVE_PARAMS,
        DeviceKind::Gain => GAIN_PARAMS,
        DeviceKind::Delay => DELAY_PARAMS,
        DeviceKind::Compressor => COMPRESSOR_PARAMS,
        DeviceKind::Limiter => LIMITER_PARAMS,
        DeviceKind::Vst3 => &[],
    }
}

fn validate_vst3(id: &Id, raw: RawVst3Config) -> Result<Vst3Config, SourceError> {
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
    if raw.class_id.len() != 32 || !raw.class_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return validation(format!(
            "VST3 device '{id}' class_id must contain exactly 32 hexadecimal digits"
        ));
    }
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

fn validate_transport(raw: RawTransport) -> Result<Transport, SourceError> {
    match raw {
        RawTransport::Loop {
            bpm,
            meter,
            loop_beats,
        } => {
            validate_meter(meter)?;
            if !bpm.is_finite() || !(MIN_BPM..=MAX_BPM).contains(&bpm) {
                return validation(format!(
                    "transport bpm must be finite and in {MIN_BPM}..={MAX_BPM}"
                ));
            }
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
                return validation(format!(
                    "route '{}' targets undeclared bus '{}'",
                    route.id, route.to
                ));
            };
            adjacency[index + 1].push(target);
        }
    }
    for track in tracks {
        for route in std::iter::once(&track.output).chain(&track.sends) {
            if !bus_indices.contains_key(route.to.as_str()) {
                return validation(format!(
                    "route '{}' targets undeclared bus '{}'",
                    route.id, route.to
                ));
            }
        }
    }

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
    SourceError::Validation(message.into())
}

fn validation<T>(message: impl Into<String>) -> Result<T, SourceError> {
    Err(invalid(message))
}

const STUDIO_SYNTH_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "vibrato_cents",
        min: 0.0,
        max: 100.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "vibrato_hz",
        min: 0.1,
        max: 12.0,
        default: 5.5,
    },
    ParameterSpec {
        name: "mode",
        min: 0.0,
        max: 7.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "gain_db",
        min: -60.0,
        max: 12.0,
        default: -14.0,
    },
    ParameterSpec {
        name: "attack_ms",
        min: 0.0,
        max: 10000.0,
        default: 5.0,
    },
    ParameterSpec {
        name: "decay_ms",
        min: 1.0,
        max: 10000.0,
        default: 200.0,
    },
    ParameterSpec {
        name: "sustain",
        min: 0.0,
        max: 1.0,
        default: 0.65,
    },
    ParameterSpec {
        name: "release_ms",
        min: 1.0,
        max: 10000.0,
        default: 200.0,
    },
    ParameterSpec {
        name: "cutoff_hz",
        min: 20.0,
        max: 20000.0,
        default: 4000.0,
    },
    ParameterSpec {
        name: "resonance",
        min: 0.0,
        max: 1.0,
        default: 0.15,
    },
    ParameterSpec {
        name: "filter_env",
        min: -6.0,
        max: 8.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "detune_cents",
        min: 0.0,
        max: 60.0,
        default: 12.0,
    },
    ParameterSpec {
        name: "unison",
        min: 1.0,
        max: 5.0,
        default: 1.0,
    },
    ParameterSpec {
        name: "width",
        min: 0.0,
        max: 1.0,
        default: 0.7,
    },
    ParameterSpec {
        name: "sub",
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "fm_ratio",
        min: 0.1,
        max: 16.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "fm_index",
        min: 0.0,
        max: 16.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "drive_db",
        min: 0.0,
        max: 24.0,
        default: 0.0,
    },
];

const REVERB_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "decay_s",
        min: 0.1,
        max: 15.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "damping_hz",
        min: 500.0,
        max: 20000.0,
        default: 6000.0,
    },
    ParameterSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.25,
    },
    ParameterSpec {
        name: "predelay_ms",
        min: 0.0,
        max: 200.0,
        default: 20.0,
    },
];

const STEREO_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "pan",
        min: -1.0,
        max: 1.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "width",
        min: 0.0,
        max: 2.0,
        default: 1.0,
    },
    ParameterSpec {
        name: "duck",
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "period_beats",
        min: 0.125,
        max: 16.0,
        default: 1.0,
    },
];

const EQ_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "frequency_hz",
        min: 20.0,
        max: 20000.0,
        default: 1000.0,
    },
    ParameterSpec {
        name: "gain_db",
        min: -24.0,
        max: 24.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "q",
        min: 0.1,
        max: 12.0,
        default: 0.707,
    },
    ParameterSpec {
        name: "mode",
        min: 0.0,
        max: 2.0,
        default: 0.0,
    },
];
const CHORUS_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "rate_hz",
        min: 0.01,
        max: 10.0,
        default: 0.4,
    },
    ParameterSpec {
        name: "depth_ms",
        min: 0.0,
        max: 15.0,
        default: 5.0,
    },
    ParameterSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.3,
    },
];
const GATE_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "threshold_db",
        min: -80.0,
        max: 0.0,
        default: -40.0,
    },
    ParameterSpec {
        name: "ratio",
        min: 1.0,
        max: 20.0,
        default: 4.0,
    },
    ParameterSpec {
        name: "attack_ms",
        min: 0.1,
        max: 100.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "release_ms",
        min: 1.0,
        max: 2000.0,
        default: 120.0,
    },
];

const SAMPLE_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "gain_db",
        min: -120.,
        max: 24.,
        default: 0.,
    },
    ParameterSpec {
        name: "attack_ms",
        min: 0.,
        max: 5000.,
        default: 2.,
    },
    ParameterSpec {
        name: "release_ms",
        min: 0.,
        max: 10000.,
        default: 35.,
    },
];
