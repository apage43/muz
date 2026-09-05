use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

use crate::midi::{ImportedMidi, MidiSummary};

pub const SCHEMA_VERSION: u32 = 1;
pub const TICKS_PER_BEAT: u32 = 960;
pub const MAX_TRACKS: usize = 32;
pub const MAX_BUSES: usize = 15;
pub const MAX_DEVICES: usize = 128;
pub const MAX_ROUTES: usize = 128;
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

impl Device {
    pub fn same_structural_identity(&self, other: &Self) -> bool {
        self.asset_versions == other.asset_versions
            && self.patch == other.patch
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
    pub root: u8,
    pub keys: [u8; 2],
    pub velocity: [f32; 2],
    pub offset_seconds: f64,
    pub loop_seconds: Option<[f64; 2]>,
    pub one_shot: bool,
}
