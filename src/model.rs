use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

use crate::midi::{ImportedMidi, MidiSummary};

pub const TICKS_PER_BEAT: u32 = 960;
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
    pub extras: crate::description::Extras,
    pub transport: Transport,
    pub master: Bus,
    pub buses: Vec<Bus>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Transport {
    OneShot {
        meter: [u8; 2],
        meter_source: MeterSource,
    },
}

impl Transport {
    pub fn meter(&self) -> [u8; 2] {
        match self {
            Self::OneShot { meter, .. } => *meter,
        }
    }
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
    /// Seconds of silent input, without events or held notes, after which each
    /// device of the chain stops processing and outputs silence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_after: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrackSource {
    Midi(MidiTrackSource),
}

impl TrackSource {
    pub fn id(&self) -> &Id {
        match self {
            Self::Midi(midi) => &midi.id,
        }
    }

    /// Structural source identity deliberately excludes imported content. This lets
    /// a reload retain compatible runtime/plugin state when the same source changes.
    pub fn same_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Midi(left), Self::Midi(right)) => {
                left.id == right.id
                    && left.asset == right.asset
                    && left.channel == right.channel
                    && left.all_channels == right.all_channels
            }
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
pub struct Device {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sfz: Option<SfzConfig>,
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

/// Native SFZ preparation options. Asset embedding is opt-in to preserve licensing.
pub const DEFAULT_SFZ_SAMPLE_FRAMES: usize = 64 * 1024 * 1024;
pub const MAX_SFZ_SAMPLE_FRAMES: usize = 1024 * 1024 * 1024;
fn default_sfz_sample_frames() -> usize {
    DEFAULT_SFZ_SAMPLE_FRAMES
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SfzConfig {
    #[serde(default = "default_sfz_sample_frames")]
    pub max_sample_frames: usize,
    #[serde(default)]
    pub source_overlays: Vec<crate::sfz::SourceOverlay>,
    #[serde(default)]
    pub program: crate::sfz::Program,
    pub path: String,
    #[serde(default)]
    pub defines: BTreeMap<String, String>,
    pub max_voices: usize,
    pub seed: u64,
    #[serde(default)]
    pub embed_assets: bool,
}

impl SfzConfig {
    pub fn controller_default(&self, cc: u16) -> f32 {
        self.program.controls.get(&cc).copied().unwrap_or(match cc {
            7 => 100.,
            11 => 127.,
            _ => 0.,
        })
    }
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
        if self.kind == DeviceKind::VoicePatch
            && let Some(patch) = &self.patch
            && let Ok(checked) = crate::patch_description::ValidatedPatch::from_json(patch)
        {
            values.extend(checked.controls().clone());
        }
        if let Some(config) = &self.sfz {
            for cc in 0..128u16 {
                values.insert(format!("cc{cc}"), config.controller_default(cc));
            }
        }
        values.extend(self.params.iter().map(|(k, v)| (k.clone(), *v)));
        values
    }

    fn same_patch_structure(&self, other: &Self) -> bool {
        match (&self.patch, &other.patch) {
            (None, None) => true,
            (Some(a), Some(b)) => match (
                crate::patch_description::ValidatedPatch::from_json(a),
                crate::patch_description::ValidatedPatch::from_json(b),
            ) {
                (Ok(a), Ok(b)) => a.same_structure(&b),
                _ => false,
            },
            _ => false,
        }
    }
    pub fn same_structural_identity(&self, other: &Self) -> bool {
        self.asset_versions == other.asset_versions
            && self.same_patch_structure(other)
            && self.generation == other.generation
            && self.rack == other.rack
            && self.sample == other.sample
            && self.sfz == other.sfz
            && self.kind == other.kind
            && self.vst3 == other.vst3
            && self.sidechain == other.sidechain
            && crate::description::parameter_specs(self.kind)
                .iter()
                .filter(|s| s.effect != crate::description::ParameterEffect::Control)
                .all(|s| self.params.get(s.name) == other.params.get(s.name))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vst3Config {
    #[serde(default)]
    pub state: Option<String>,
    pub bundle_env: String,
    pub class_id: String,
    pub expected_version: String,
    /// CLAP note controls: per-note expression names bound to plugin parameters.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub note_controls: BTreeMap<String, NoteParameter>,
    /// Process this plugin on the rendering thread in parallel offline renders.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub render_thread: bool,
}

/// A per-note curve destination on a plugin parameter, addressed by note ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteParameter {
    /// Parameter identity as lanes use it: inspected key (`p12`), numeric ID, or name.
    pub param: String,
    pub mode: NoteParameterMode,
}

/// `Value` sets the note's parameter value in plain units; `Mod` adds a plain-unit
/// offset on top of the parameter's value without moving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteParameterMode {
    Value,
    Mod,
}
impl NoteParameterMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::Mod => "mod",
        }
    }
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
    #[serde(rename = "builtin.sfz")]
    Sfz,
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
                | Self::Sfz
                | Self::StudioSynth
                | Self::Vst3
                | Self::Clap
        )
    }

    pub fn port_signature(self) -> PortSignature {
        match self {
            Self::VoicePatch
            | Self::Sampler
            | Self::Sfz
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

/// Largest accepted graph budget. The budget value itself, not the graph's use
/// of it, sizes the per-stream device telemetry tables and bounds reader zone
/// counts, so it stays a fixed ceiling rather than an open-ended number.
pub const MAX_GRAPH_BUDGET: usize = 65536;

/// Process-wide preparation budget; also sizes live telemetry before playback.
/// One unit is a device, bus (including master), route, or prepared sample zone.
pub fn graph_budget() -> Result<usize, String> {
    if let Some(context) = crate::host::current() {
        return if (1..=MAX_GRAPH_BUDGET).contains(&context.graph_units) {
            Ok(context.graph_units)
        } else {
            Err(format!(
                "graph budget must be an integer in 1..={MAX_GRAPH_BUDGET}"
            ))
        };
    }
    static BUDGET: std::sync::OnceLock<Result<usize, String>> = std::sync::OnceLock::new();
    BUDGET
        .get_or_init(|| match std::env::var("MUZ_GRAPH_BUDGET") {
            Err(std::env::VarError::NotPresent) => Ok(4096),
            value => value
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|n| (1..=MAX_GRAPH_BUDGET).contains(n))
                .ok_or_else(|| {
                    format!("MUZ_GRAPH_BUDGET must be an integer in 1..={MAX_GRAPH_BUDGET}")
                }),
        })
        .clone()
}

/// What one distinct SFZ program costs the graph budget, however many
/// instruments use it.
#[derive(Clone, Debug, Serialize)]
pub struct SfzProgramCost {
    pub source: String,
    pub instruments: usize,
    pub regions: usize,
    pub samples: usize,
    pub modulation_terms: usize,
    /// Charged once, with the first instrument that uses the program.
    pub units: usize,
    /// Charged for each further instrument, before its own voice slots.
    pub units_per_additional_instrument: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct GraphResources {
    pub tracks: usize,
    pub devices: usize,
    pub buses: usize,
    pub routes: usize,
    pub sample_zones: usize,
    /// Distinct programs; instruments with an identical program share one.
    pub sfz_programs: usize,
    pub sfz_instruments: usize,
    /// Regions, samples and modulation terms count each distinct program once.
    pub sfz_regions: usize,
    pub sfz_samples: usize,
    pub sfz_modulation_terms: usize,
    pub sfz_voice_slots: usize,
    pub units: usize,
    pub contributors: Vec<(String, usize)>,
    /// The five most expensive distinct programs.
    pub sfz_program_costs: Vec<SfzProgramCost>,
}
/// SFZ accounting follows what preparation allocates. A program's regions,
/// recordings and modulation tables are prepared once and shared by every
/// instrument that uses an identical program: regions and samples cost one unit
/// each and modulation terms one unit per 32, charged to the first instrument.
/// Each instrument owns its voices (one unit per 32 slots); each instrument
/// after the first also pays one unit per 32 regions for its own per-region
/// counters and its own scan of the region table at note-on.
#[derive(Default)]
struct SfzAccounting<'a> {
    programs: Vec<(&'a crate::sfz::Program, SfzProgramCost)>,
    voice_slots: usize,
}
impl<'a> SfzAccounting<'a> {
    /// Units this device adds, including SFZ instruments inside its rack.
    fn charge(&mut self, device: &'a Device) -> usize {
        let mut units = 0;
        if let Some(config) = &device.sfz {
            self.voice_slots += config.max_voices;
            units += config.max_voices.div_ceil(32);
            let program = &config.program;
            if let Some((_, cost)) = self
                .programs
                .iter_mut()
                .find(|(known, _)| std::ptr::eq(*known, program) || *known == program)
            {
                cost.instruments += 1;
                units += cost.units_per_additional_instrument;
            } else {
                let regions = program.regions.len();
                let samples = program
                    .regions
                    .iter()
                    .filter_map(|r| r.sample.as_ref())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                // Conservatively charge each authored controller route/numbered modulation
                // declaration, including inherited copies required by distinct regions.
                let modulation_terms = program
                    .regions
                    .iter()
                    .map(|r| {
                        r.opcodes
                            .keys()
                            .filter(|name| {
                                name.contains("cc")
                                    || name.starts_with("eg")
                                    || name.starts_with("lfo")
                                    || name.starts_with("var")
                            })
                            .count()
                    })
                    .sum::<usize>();
                let cost = SfzProgramCost {
                    source: program.source.display().to_string(),
                    instruments: 1,
                    regions,
                    samples,
                    modulation_terms,
                    units: regions + samples + modulation_terms.div_ceil(32),
                    units_per_additional_instrument: regions.div_ceil(32),
                };
                units += cost.units;
                self.programs.push((program, cost));
            }
        }
        if let Some(rack) = &device.rack {
            for child in rack.branches.iter().flatten() {
                units += self.charge(child);
            }
        }
        units
    }
}
impl Session {
    pub fn graph_resources(&self) -> GraphResources {
        let mut contributors = Vec::new();
        let mut devices = 0;
        let mut routes = 0;
        let mut sample_zones = 0;
        let mut sfz = SfzAccounting::default();
        let mut total_sfz_units = 0;
        for bus in std::iter::once(&self.master).chain(&self.buses) {
            devices += bus.inserts.len();
            let sfz_units: usize = bus.inserts.iter().map(|d| sfz.charge(d)).sum();
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
            total_sfz_units += sfz_units;
            let n = bus.sends.len() + usize::from(bus.output.is_some());
            routes += n;
            contributors.push((
                format!("bus {}", bus.id),
                1 + bus.inserts.len() + n + zones + sfz_units,
            ));
        }
        for track in &self.tracks {
            devices += 1 + track.inserts.len();
            let sfz_units: usize = std::iter::once(&track.instrument)
                .chain(&track.inserts)
                .map(|d| sfz.charge(d))
                .sum();
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
            total_sfz_units += sfz_units;
            routes += 1 + track.sends.len();
            contributors.push((
                format!("track {}", track.id),
                2 + track.inserts.len() + track.sends.len() + zones + sfz_units,
            ));
        }
        contributors.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        contributors.truncate(5);
        let buses = self.buses.len() + 1;
        let mut sfz_program_costs: Vec<_> = sfz.programs.into_iter().map(|(_, cost)| cost).collect();
        let sfz_programs = sfz_program_costs.len();
        let count = |f: fn(&SfzProgramCost) -> usize| sfz_program_costs.iter().map(f).sum::<usize>();
        let (sfz_instruments, sfz_regions, sfz_samples, sfz_modulation_terms) = (
            count(|p| p.instruments),
            count(|p| p.regions),
            count(|p| p.samples),
            count(|p| p.modulation_terms),
        );
        sfz_program_costs.sort_by(|a, b| b.units.cmp(&a.units).then_with(|| a.source.cmp(&b.source)));
        sfz_program_costs.truncate(5);
        GraphResources {
            tracks: self.tracks.len(),
            devices,
            buses,
            routes,
            sample_zones,
            sfz_programs,
            sfz_instruments,
            sfz_regions,
            sfz_samples,
            sfz_modulation_terms,
            sfz_voice_slots: sfz.voice_slots,
            units: devices + buses + routes + sample_zones + total_sfz_units,
            contributors,
            sfz_program_costs,
        }
    }
    pub fn validate_graph_budget(&self) -> Result<(), String> {
        let allowed = graph_budget()?;
        let r = self.graph_resources();
        if r.units <= allowed {
            return Ok(());
        }
        let mut message = format!(
            "expanded graph requires {} resource units; allowed {} (MUZ_GRAPH_BUDGET, maximum {MAX_GRAPH_BUDGET}): {} tracks, {} devices, {} buses, {} routes, {} sample zones, {} SFZ regions, {} SFZ samples, {} SFZ modulation terms, {} SFZ voice slots; main contributors: {}",
            r.units,
            allowed,
            r.tracks,
            r.devices,
            r.buses,
            r.routes,
            r.sample_zones,
            r.sfz_regions,
            r.sfz_samples,
            r.sfz_modulation_terms,
            r.sfz_voice_slots,
            r.contributors
                .iter()
                .map(|(name, n)| format!("{name}: {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(p) = r.sfz_program_costs.first() {
            message.push_str(&format!(
                "; {} SFZ instruments use {} distinct programs (instruments with an identical program share one prepared copy, counted once); the largest, {}, used by {}, costs {} units once ({} regions, {} samples, {} modulation terms) plus {} units for each instrument after the first",
                r.sfz_instruments,
                r.sfz_programs,
                p.source,
                p.instruments,
                p.units,
                p.regions,
                p.samples,
                p.modulation_terms,
                p.units_per_additional_instrument,
            ));
        }
        Err(message)
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
