use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedMidi {
    pub summary: MidiSummary,
    pub notes: Vec<MidiNote>,
    pub controllers: Vec<MidiController>,
    pub tempos: Vec<MidiTempo>,
    #[serde(default)]
    pub messages: Vec<ChannelMessage>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiSummary {
    pub bytes: u64,
    pub ppq: u32,
    pub end_tick: u64,
    pub events: u32,
    pub notes: u32,
    pub controllers: u32,
    pub tempos: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiNote {
    #[serde(default)]
    pub performance: Option<crate::expression::Performance>,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub annotations: std::collections::BTreeMap<String, serde_json::Value>,
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub channel: u8,
    pub key: u8,
    pub attack_velocity: u8,
    pub release_velocity: u8,
    pub source_order: u32,
    pub end_source_order: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiController {
    pub tick: u64,
    pub channel: u8,
    pub controller: u8,
    pub value: u8,
    pub source_order: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiTempo {
    pub tick: u64,
    pub micros_per_quarter: u32,
    pub source_order: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChannelMessage {
    pub tick: u64,
    pub bytes: [u8; 3],
    pub len: u8,
    pub source_order: u32,
}
