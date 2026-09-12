use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

use crate::midi::{ImportedMidi, MidiSummary};

pub const SCHEMA_VERSION: u32 = 1;
pub const TICKS_PER_BEAT: u32 = 960;
pub const MAX_NOTES: usize = 1_024;
pub const MAX_TRANSACTION_OPS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(String);

impl Id {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub extras: crate::compile::Extras,
    pub transport: Transport,
    pub master: Bus,
    pub buses: Vec<Bus>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Transport {
    Loop {
        bpm: f64,
        meter: [u8; 2],
        loop_ticks: u64,
    },
    OneShot {
        meter: [u8; 2],
        meter_source: MeterSource,
    },
}

impl Transport {
    pub fn mode(&self) -> TransportMode {
        match self {
            Self::Loop { .. } => TransportMode::Loop,
            Self::OneShot { .. } => TransportMode::OneShot,
        }
    }

    pub fn meter(&self) -> [u8; 2] {
        match self {
            Self::Loop { meter, .. } | Self::OneShot { meter, .. } => *meter,
        }
    }

    /// Constant loop tempo, or the MIDI default used until a one-shot tempo map is compiled.
    pub fn initial_bpm(&self) -> f64 {
        match self {
            Self::Loop { bpm, .. } => *bpm,
            Self::OneShot { .. } => 120.0,
        }
    }

    pub fn loop_ticks(&self) -> u64 {
        match self {
            Self::Loop { loop_ticks, .. } => *loop_ticks,
            Self::OneShot { .. } => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    Loop,
    OneShot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterSource {
    Declared,
    Inferred,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: Id,
    pub name: String,
    pub source: TrackSource,
    pub instrument: Device,
    pub inserts: Vec<Device>,
    pub output: Route,
    pub sends: Vec<Route>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrackSource {
    Pattern(Pattern),
    Midi(MidiTrackSource),
}

impl TrackSource {
    pub fn id(&self) -> &Id {
        match self {
            Self::Pattern(pattern) => &pattern.id,
            Self::Midi(midi) => &midi.id,
        }
    }

    pub fn pattern(&self) -> Option<&Pattern> {
        match self {
            Self::Pattern(pattern) => Some(pattern),
            Self::Midi(_) => None,
        }
    }

    pub fn pattern_mut(&mut self) -> Option<&mut Pattern> {
        match self {
            Self::Pattern(pattern) => Some(pattern),
            Self::Midi(_) => None,
        }
    }

    /// Structural source identity deliberately excludes imported content. This lets
    /// a reload retain compatible runtime/plugin state when the same source changes.
    pub fn same_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Pattern(left), Self::Pattern(right)) => left.id == right.id,
            (Self::Midi(left), Self::Midi(right)) => {
                left.id == right.id
                    && left.asset == right.asset
                    && left.channel == right.channel
                    && left.all_channels == right.all_channels
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MidiTrackSource {
    #[serde(default)]
    pub all_channels: bool,
    pub id: Id,
    pub asset: String,
    pub channel: u8,
    pub summary: MidiSummary,
    /// Event arrays participate in equality/reconciliation but are omitted from
    /// serialization so control status remains bounded.
    #[serde(skip, default)]
    pub imported: ImportedMidi,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bus {
    pub id: Id,
    pub name: String,
    pub inserts: Vec<Device>,
    pub output: Option<Route>,
    pub sends: Vec<Route>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub id: Id,
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: Id,
    pub start_ticks: u64,
    pub duration_ticks: u64,
    pub key: u8,
    pub velocity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asset_versions: Vec<(u64, u128)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<serde_json::Value>,
    #[serde(default)]
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<Rack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<Vec<SampleZone>>,
    #[serde(default)]
    pub sidechain: Option<String>,
    pub id: Id,
    pub kind: DeviceKind,
    pub params: BTreeMap<String, f32>,
    #[serde(rename = "plugin", skip_serializing_if = "Option::is_none", default)]
    pub vst3: Option<Vst3Config>,
}

/// Per-patch decoded stereo-frame storage. Defaults preserve the legacy 64 MiB cap.
pub const DEFAULT_PATCH_SAMPLE_FRAMES: usize = 8 * 1024 * 1024;
pub const MAX_PATCH_SAMPLE_FRAMES: usize = 128 * 1024 * 1024;
pub fn patch_sample_budget(patch: &serde_json::Value) -> Result<usize, String> {
    match patch.get("sample_budget_frames") {
        None => Ok(DEFAULT_PATCH_SAMPLE_FRAMES),
        Some(value) => value
            .as_f64()
            .filter(|n| {
                n.is_finite()
                    && n.fract() == 0.0
                    && *n >= 1.0
                    && *n <= MAX_PATCH_SAMPLE_FRAMES as f64
            })
            .map(|n| n as usize)
            .ok_or_else(|| {
                format!("sample_budget_frames must be an integer in 1..={MAX_PATCH_SAMPLE_FRAMES}")
            }),
    }
}

impl Device {
    pub(crate) fn sample_maps(&self) -> Result<Vec<Vec<SampleZone>>, String> {
        let mut maps = self.sample.iter().cloned().collect::<Vec<_>>();
        if let Some(nodes) = self.patch.as_ref().and_then(|p| p["nodes"].as_array()) {
            for n in nodes {
                if n["op"] == "reader" {
                    maps.push(
                        serde_json::from_value(n["zones"].clone())
                            .map_err(|e| format!("reader zones: {e}"))?,
                    );
                }
            }
        }
        Ok(maps)
    }

    /// Effective controls, including defaults in older serialized voice patches.
    /// Called during preparation/reconciliation, never in the audio callback.
    pub fn control_values(&self) -> BTreeMap<String, f32> {
        let mut values = BTreeMap::new();
        if self.kind == DeviceKind::VoicePatch {
            if let Some(patch) = &self.patch {
                if let Some(nodes) = patch["nodes"].as_array() {
                    for node in nodes {
                        if node["op"] == "param" {
                            if let (Some(name), Some(value)) =
                                (node["id"].as_str(), node["value"].as_f64())
                            {
                                values.insert(name.to_owned(), value as f32);
                            }
                        }
                    }
                }
                values.insert("gain_db".into(), 20.0 * 0.2_f32.log10());
                values.insert("glide_ms".into(), 0.);
                values.insert("velocity_track".into(), 1.);
                for (name, value) in &mut values {
                    if let Some(override_value) = patch[name].as_f64() {
                        *value = override_value as f32;
                    }
                }
            }
        }
        values.extend(self.params.iter().map(|(k, v)| (k.clone(), *v)));
        values
    }

    fn patch_structure(&self) -> Option<serde_json::Value> {
        let mut patch = self.patch.clone()?;
        if self.kind == DeviceKind::VoicePatch {
            if let Some(record) = patch.as_object_mut() {
                for name in self.control_values().keys() {
                    record.remove(name);
                }
                for name in ["name", "id", "type"] {
                    record.remove(name);
                }
                if let Some(nodes) = record.get_mut("nodes").and_then(|n| n.as_array_mut()) {
                    for node in nodes {
                        if node["op"] == "param" {
                            if let Some(row) = node.as_object_mut() {
                                row.remove("value");
                            }
                        }
                    }
                }
            }
        }
        Some(patch)
    }
    pub fn same_structural_identity(&self, other: &Self) -> bool {
        self.asset_versions == other.asset_versions
            && self.patch_structure() == other.patch_structure()
            && self.generation == other.generation
            && self.rack == other.rack
            && self.sample == other.sample
            && self.kind == other.kind
            && self.vst3 == other.vst3
            && self.sidechain == other.sidechain
            && self.params.get("lookahead_ms") == other.params.get("lookahead_ms")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vst3Config {
    #[serde(default)]
    pub state: Option<String>,
    pub bundle_env: String,
    pub class_id: String,
    pub expected_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DeviceKind {
    #[serde(rename = "builtin.voice_patch")]
    VoicePatch,
    #[serde(rename = "clap")]
    Clap,
    #[serde(rename = "builtin.rack")]
    Rack,
    #[serde(rename = "builtin.sampler")]
    Sampler,
    #[serde(rename = "builtin.eq")]
    Eq,
    #[serde(rename = "builtin.bitcrusher")]
    Bitcrusher,
    #[serde(rename = "builtin.chorus")]
    Chorus,
    #[serde(rename = "builtin.gate")]
    Gate,
    #[serde(rename = "builtin.studio_synth")]
    StudioSynth,
    #[serde(rename = "builtin.reverb")]
    Reverb,
    #[serde(rename = "builtin.stereo")]
    Stereo,
    #[serde(rename = "builtin.poly_synth")]
    PolySynth,
    #[serde(rename = "builtin.lowpass")]
    Lowpass,
    #[serde(rename = "builtin.highpass")]
    Highpass,
    #[serde(rename = "builtin.drive")]
    Drive,
    #[serde(rename = "builtin.gain")]
    Gain,
    #[serde(rename = "builtin.delay")]
    Delay,
    #[serde(rename = "builtin.compressor")]
    Compressor,
    #[serde(rename = "builtin.limiter")]
    Limiter,
    #[serde(rename = "vst3")]
    Vst3,
}

impl DeviceKind {
    pub fn is_instrument(self) -> bool {
        matches!(
            self,
            Self::VoicePatch
                | Self::Sampler
                | Self::PolySynth
                | Self::StudioSynth
                | Self::Vst3
                | Self::Clap
        )
    }

    pub fn port_signature(self) -> PortSignature {
        match self {
            Self::VoicePatch
            | Self::Sampler
            | Self::PolySynth
            | Self::StudioSynth
            | Self::Vst3
            | Self::Clap => PortSignature {
                audio_inputs: 0,
                audio_outputs: 2,
                note_input: true,
            },
            Self::Rack
            | Self::Eq
            | Self::Bitcrusher
            | Self::Chorus
            | Self::Gate
            | Self::Reverb
            | Self::Stereo
            | Self::Lowpass
            | Self::Highpass
            | Self::Drive
            | Self::Gain
            | Self::Delay
            | Self::Compressor
            | Self::Limiter => PortSignature {
                audio_inputs: 2,
                audio_outputs: 2,
                note_input: false,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortSignature {
    pub audio_inputs: u8,
    pub audio_outputs: u8,
    pub note_input: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Route {
    #[serde(default)]
    pub pre: bool,
    pub id: Id,
    pub to: Id,
    pub gain_db: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rack {
    pub branches: Vec<Vec<Device>>,
    pub expose: BTreeMap<String, String>,
    pub modulate: Vec<RackModulation>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RackModulation {
    pub target: String,
    #[serde(default)]
    pub base: f32,
    #[serde(default)]
    pub depth: f32,
    #[serde(default)]
    pub rate_hz: f32,
    #[serde(default)]
    pub follower: f32,
    #[serde(default = "follower_attack")]
    pub attack_ms: f32,
    #[serde(default = "follower_release")]
    pub release_ms: f32,
    pub min: f32,
    pub max: f32,
}
fn follower_attack() -> f32 {
    5.
}
fn follower_release() -> f32 {
    100.
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SampleZone {
    pub path: String,
    /// Static recording calibration, captured independently by each voice.
    #[serde(default)]
    pub gain_db: f32,
    /// MIDI pitch of the recording; fractional values preserve its fine tuning.
    pub root: f64,
    pub keys: [u8; 2],
    pub velocity: [f32; 2],
    pub offset_seconds: f64,
    pub loop_seconds: Option<[f64; 2]>,
    pub one_shot: bool,
}

impl SampleZone {
    /// Key ranges are closed; velocity layers are half-open except at full scale.
    pub fn matches(&self, key: u8, velocity: f32) -> bool {
        key >= self.keys[0]
            && key <= self.keys[1]
            && velocity >= self.velocity[0]
            && (velocity < self.velocity[1] || self.velocity[1] == 1.0 && velocity <= 1.0)
    }
}

impl Session {
    /// Check the performed events, using the same precision and zone predicate as playback.
    pub fn validate_sample_coverage(&self) -> Result<(), String> {
        let mut gaps = Vec::new();
        for track in &self.tracks {
            for zones in track.instrument.sample_maps()? {
                let mut count = 0;
                let mut example = String::new();
                let mut check = |key, pitch, velocity, id: &str| {
                    if !zones.iter().any(|zone| zone.matches(key, velocity)) {
                        count += 1;
                        if count == 1 {
                            example = format!(
                                "pitch {pitch} (MIDI key {key}), velocity {velocity}, source key '{id}'"
                            );
                        }
                    }
                };
                match &track.source {
                    TrackSource::Pattern(pattern) => {
                        for note in &pattern.notes {
                            check(note.key, note.key as f32, note.velocity, note.id.as_str());
                        }
                    }
                    TrackSource::Midi(source) => {
                        for note in &source.imported.notes {
                            if !source.all_channels && note.channel != source.channel {
                                continue;
                            }
                            if let Some(value) = note.annotations.get("sample_zone") {
                                let velocity = note
                                    .performance
                                    .map_or(note.attack_velocity as f32 / 127.0, |p| {
                                        p.velocity as f32
                                    });
                                let valid = valid_sample_zone(value, &zones, note.key, velocity);
                                if !valid {
                                    return Err(format!(
                                        "track '{}', note '{}': sample_zone must be a zero-based index of a zone matching the performed key and velocity",
                                        track.id, note.id
                                    ));
                                }
                            }
                            check(
                                note.key,
                                note.performance.map_or(note.key as f32, |p| p.pitch as f32),
                                note.performance
                                    .map_or(note.attack_velocity as f32 / 127.0, |p| {
                                        p.velocity as f32
                                    }),
                                &note.id,
                            );
                        }
                    }
                }
                if count > 0 {
                    gaps.push(format!("track '{}': {count} performed sampler notes have no matching zone; example {example}", track.id));
                }
            }
        }
        if gaps.is_empty() {
            Ok(())
        } else {
            Err(gaps.join("\n"))
        }
    }
}

/// Process-wide preparation budget; also sizes live telemetry before playback.
/// One unit is a device, bus (including master), route, or prepared sample zone.
pub fn graph_budget() -> Result<usize, String> {
    static BUDGET: std::sync::OnceLock<Result<usize, String>> = std::sync::OnceLock::new();
    BUDGET
        .get_or_init(|| match std::env::var("MUZ_GRAPH_BUDGET") {
            Err(std::env::VarError::NotPresent) => Ok(4096),
            value => value
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|n| (1..=65536).contains(n))
                .ok_or_else(|| "MUZ_GRAPH_BUDGET must be an integer in 1..=65536".into()),
        })
        .clone()
}

#[derive(Clone, Debug, Serialize)]
pub struct GraphResources {
    pub tracks: usize,
    pub devices: usize,
    pub buses: usize,
    pub routes: usize,
    pub sample_zones: usize,
    pub units: usize,
    pub contributors: Vec<(String, usize)>,
}
impl Session {
    pub fn graph_resources(&self) -> GraphResources {
        let mut contributors = Vec::new();
        let mut devices = 0;
        let mut routes = 0;
        let mut sample_zones = 0;
        for bus in std::iter::once(&self.master).chain(&self.buses) {
            devices += bus.inserts.len();
            let zones: usize = bus
                .inserts
                .iter()
                .map(|d| {
                    d.sample_maps()
                        .unwrap_or_default()
                        .iter()
                        .map(Vec::len)
                        .sum::<usize>()
                })
                .sum();
            sample_zones += zones;
            let n = bus.sends.len() + usize::from(bus.output.is_some());
            routes += n;
            contributors.push((format!("bus {}", bus.id), 1 + bus.inserts.len() + n + zones));
        }
        for track in &self.tracks {
            devices += 1 + track.inserts.len();
            let zones: usize = std::iter::once(&track.instrument)
                .chain(&track.inserts)
                .map(|d| {
                    d.sample_maps()
                        .unwrap_or_default()
                        .iter()
                        .map(Vec::len)
                        .sum::<usize>()
                })
                .sum();
            sample_zones += zones;
            routes += 1 + track.sends.len();
            contributors.push((
                format!("track {}", track.id),
                2 + track.inserts.len() + track.sends.len() + zones,
            ));
        }
        contributors.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        contributors.truncate(5);
        let buses = self.buses.len() + 1;
        GraphResources {
            tracks: self.tracks.len(),
            devices,
            buses,
            routes,
            sample_zones,
            units: devices + buses + routes + sample_zones,
            contributors,
        }
    }
    pub fn validate_graph_budget(&self) -> Result<(), String> {
        let allowed = graph_budget()?;
        let r = self.graph_resources();
        if r.units <= allowed {
            return Ok(());
        }
        Err(format!(
            "expanded graph requires {} resource units; allowed {} (MUZ_GRAPH_BUDGET): {} tracks, {} devices, {} buses, {} routes, {} sample zones; main contributors: {}",
            r.units,
            allowed,
            r.tracks,
            r.devices,
            r.buses,
            r.routes,
            r.sample_zones,
            r.contributors
                .iter()
                .map(|(name, n)| format!("{name}: {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Validate an authored absolute zone choice without loading sample audio.
pub(crate) fn valid_sample_zone(
    value: &serde_json::Value,
    zones: &[SampleZone],
    key: u8,
    velocity: f32,
) -> bool {
    value.as_f64().is_some_and(|index| {
        index.is_finite()
            && index.fract() == 0.
            && index >= 0.
            && index < zones.len() as f64
            && zones[index as usize].matches(key, velocity)
    })
}
