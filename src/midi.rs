use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use midly::{Format, MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Bound imported asset memory on the coordinator thread.
pub const MAX_MIDI_BYTES: usize = 1_048_576;
pub const MAX_MIDI_EVENTS: usize = 16_384;
pub const MAX_MIDI_NOTES: usize = 8_192;
pub const MAX_MIDI_CONTROLLERS: usize = 4_096;
pub const MAX_MIDI_TEMPOS: usize = 4_096;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedMidi {
    pub summary: MidiSummary,
    pub notes: Vec<MidiNote>,
    pub controllers: Vec<MidiController>,
    pub tempos: Vec<MidiTempo>,
    #[serde(default)]
    pub messages: Vec<ChannelMessage>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiSummary {
    pub bytes: u64,
    pub ppq: u32,
    pub end_tick: u64,
    pub events: u32,
    pub notes: u32,
    pub controllers: u32,
    pub tempos: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiNote {
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
pub struct MidiController {
    pub tick: u64,
    pub channel: u8,
    pub controller: u8,
    pub value: u8,
    pub source_order: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiTempo {
    pub tick: u64,
    pub micros_per_quarter: u32,
    pub source_order: u32,
}

#[derive(Debug, Error)]
pub enum MidiImportError {
    #[error("MIDI asset `{path}` could not be inspected: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("MIDI asset `{path}` could not be read: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("MIDI asset has {actual} bytes; maximum is {maximum}")]
    ByteCapacityExceeded { actual: u64, maximum: usize },
    #[error("invalid Standard MIDI File: {0}")]
    Decode(String),
    #[error("MIDI must use SMF format 0")]
    Format,
    #[error("MIDI format 0 must contain exactly one track; found {0}")]
    TrackCount(usize),
    #[error("MIDI must use nonzero metrical PPQ timing")]
    Timing,
    #[error("MIDI tick overflow at source event {0}")]
    TickOverflow(u32),
    #[error("MIDI event count exceeds capacity {0}")]
    EventCapacityExceeded(usize),
    #[error("MIDI note count exceeds capacity {0}")]
    NoteCapacityExceeded(usize),
    #[error("MIDI controller count exceeds capacity {0}")]
    ControllerCapacityExceeded(usize),
    #[error("MIDI tempo count exceeds capacity {0}")]
    TempoCapacityExceeded(usize),
    #[error("unsupported MIDI {kind} at source event {order}")]
    UnsupportedEvent { order: u32, kind: &'static str },
    #[error("MIDI tempo at source event {0} is zero")]
    ZeroTempo(u32),
    #[error(
        "MIDI note-off at source event {order} has no matching note-on for channel {channel}, key {key}"
    )]
    UnmatchedNoteOff { order: u32, channel: u8, key: u8 },
    #[error("MIDI note at source event {order} has zero duration")]
    ZeroLengthNote { order: u32 },
    #[error("MIDI end-of-track must be the final event")]
    EventAfterEndOfTrack,
    #[error("MIDI track is missing its final end-of-track event")]
    MissingEndOfTrack,
    #[error("MIDI track ends with {count} unterminated notes")]
    UnterminatedNotes { count: usize },
}

#[derive(Clone, Copy)]
struct PendingNote {
    start_tick: u64,
    attack_velocity: u8,
    source_order: u32,
}

pub fn import_midi_file(path: impl AsRef<Path>) -> Result<ImportedMidi, MidiImportError> {
    let path = path.as_ref();
    let length = fs::metadata(path)
        .map_err(|source| MidiImportError::Inspect {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if length > MAX_MIDI_BYTES as u64 {
        return Err(MidiImportError::ByteCapacityExceeded {
            actual: length,
            maximum: MAX_MIDI_BYTES,
        });
    }
    let bytes = fs::read(path).map_err(|source| MidiImportError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    import_midi(&bytes)
}

pub fn import_midi(bytes: &[u8]) -> Result<ImportedMidi, MidiImportError> {
    if bytes.len() > MAX_MIDI_BYTES {
        return Err(MidiImportError::ByteCapacityExceeded {
            actual: bytes.len() as u64,
            maximum: MAX_MIDI_BYTES,
        });
    }

    let smf = Smf::parse(bytes).map_err(|error| MidiImportError::Decode(error.to_string()))?;
    if smf.header.format != Format::SingleTrack {
        return Err(MidiImportError::Format);
    }
    if smf.tracks.len() != 1 {
        return Err(MidiImportError::TrackCount(smf.tracks.len()));
    }
    let ppq = match smf.header.timing {
        Timing::Metrical(value) if value.as_int() != 0 => value.as_int(),
        Timing::Metrical(_) | Timing::Timecode(_, _) => return Err(MidiImportError::Timing),
    };

    let mut tick = 0_u64;
    let mut end_tick = None;
    let mut event_count = 0_usize;
    let mut note_on_count = 0_usize;
    let mut pending = BTreeMap::<(u8, u8), VecDeque<PendingNote>>::new();
    let mut notes = Vec::new();
    let mut controllers = Vec::new();
    let mut tempos = Vec::new();

    for (index, event) in smf.tracks[0].iter().enumerate() {
        let source_order = u32::try_from(index)
            .map_err(|_| MidiImportError::EventCapacityExceeded(MAX_MIDI_EVENTS))?;
        if end_tick.is_some() {
            return Err(MidiImportError::EventAfterEndOfTrack);
        }
        event_count += 1;
        if event_count > MAX_MIDI_EVENTS {
            return Err(MidiImportError::EventCapacityExceeded(MAX_MIDI_EVENTS));
        }
        tick = tick
            .checked_add(u64::from(event.delta.as_int()))
            .ok_or(MidiImportError::TickOverflow(source_order))?;

        match event.kind {
            TrackEventKind::Midi { channel, message } => {
                let channel = channel.as_int();
                match message {
                    MidiMessage::NoteOn { key, vel } if vel.as_int() != 0 => {
                        note_on_count += 1;
                        if note_on_count > MAX_MIDI_NOTES {
                            return Err(MidiImportError::NoteCapacityExceeded(MAX_MIDI_NOTES));
                        }
                        pending
                            .entry((channel, key.as_int()))
                            .or_default()
                            .push_back(PendingNote {
                                start_tick: tick,
                                attack_velocity: vel.as_int(),
                                source_order,
                            });
                    }
                    MidiMessage::NoteOff { key, vel } => finish_note(
                        &mut pending,
                        &mut notes,
                        tick,
                        channel,
                        key.as_int(),
                        vel.as_int(),
                        source_order,
                    )?,
                    MidiMessage::NoteOn { key, .. } => finish_note(
                        &mut pending,
                        &mut notes,
                        tick,
                        channel,
                        key.as_int(),
                        0,
                        source_order,
                    )?,
                    MidiMessage::Controller { controller, value } => {
                        if controllers.len() == MAX_MIDI_CONTROLLERS {
                            return Err(MidiImportError::ControllerCapacityExceeded(
                                MAX_MIDI_CONTROLLERS,
                            ));
                        }
                        controllers.push(MidiController {
                            tick,
                            channel,
                            controller: controller.as_int(),
                            value: value.as_int(),
                            source_order,
                        });
                    }
                    MidiMessage::Aftertouch { .. } => {
                        return unsupported(source_order, "polyphonic aftertouch");
                    }
                    MidiMessage::ProgramChange { .. } => {
                        return unsupported(source_order, "program change");
                    }
                    MidiMessage::ChannelAftertouch { .. } => {
                        return unsupported(source_order, "channel aftertouch");
                    }
                    MidiMessage::PitchBend { .. } => {
                        return unsupported(source_order, "pitch bend");
                    }
                }
            }
            TrackEventKind::Meta(MetaMessage::Tempo(value)) => {
                if tempos.len() == MAX_MIDI_TEMPOS {
                    return Err(MidiImportError::TempoCapacityExceeded(MAX_MIDI_TEMPOS));
                }
                let micros_per_quarter = value.as_int();
                if micros_per_quarter == 0 {
                    return Err(MidiImportError::ZeroTempo(source_order));
                }
                tempos.push(MidiTempo {
                    tick,
                    micros_per_quarter,
                    source_order,
                });
            }
            TrackEventKind::Meta(MetaMessage::EndOfTrack) => end_tick = Some(tick),
            TrackEventKind::Meta(_) => return unsupported(source_order, "meta event"),
            TrackEventKind::SysEx(_) => return unsupported(source_order, "system-exclusive event"),
            TrackEventKind::Escape(_) => return unsupported(source_order, "escape event"),
        }
    }

    let end_tick = end_tick.ok_or(MidiImportError::MissingEndOfTrack)?;
    let unterminated = pending.values().map(VecDeque::len).sum::<usize>();
    if unterminated != 0 {
        return Err(MidiImportError::UnterminatedNotes {
            count: unterminated,
        });
    }
    notes.sort_by_key(|note| note.source_order);

    let summary = MidiSummary {
        bytes: bytes.len() as u64,
        ppq: u32::from(ppq),
        end_tick,
        events: event_count as u32,
        notes: notes.len() as u32,
        controllers: controllers.len() as u32,
        tempos: tempos.len() as u32,
    };
    Ok(ImportedMidi {
        summary,
        notes,
        controllers,
        tempos,
        messages: Vec::new(),
    })
}

fn finish_note(
    pending: &mut BTreeMap<(u8, u8), VecDeque<PendingNote>>,
    notes: &mut Vec<MidiNote>,
    tick: u64,
    channel: u8,
    key: u8,
    release_velocity: u8,
    off_order: u32,
) -> Result<(), MidiImportError> {
    let Some(queue) = pending.get_mut(&(channel, key)) else {
        return Err(MidiImportError::UnmatchedNoteOff {
            order: off_order,
            channel,
            key,
        });
    };
    let Some(on) = queue.pop_front() else {
        return Err(MidiImportError::UnmatchedNoteOff {
            order: off_order,
            channel,
            key,
        });
    };
    if queue.is_empty() {
        pending.remove(&(channel, key));
    }
    let duration_ticks = tick
        .checked_sub(on.start_tick)
        .expect("absolute MIDI ticks are monotonic");
    if duration_ticks == 0 {
        return Err(MidiImportError::ZeroLengthNote {
            order: on.source_order,
        });
    }
    if notes.len() == MAX_MIDI_NOTES {
        return Err(MidiImportError::NoteCapacityExceeded(MAX_MIDI_NOTES));
    }
    notes.push(MidiNote {
        id: format!("midi.{}", on.source_order),
        tags: Vec::new(),
        annotations: Default::default(),
        start_tick: on.start_tick,
        duration_ticks,
        channel,
        key,
        attack_velocity: on.attack_velocity,
        release_velocity,
        source_order: on.source_order,
        end_source_order: off_order,
    });
    Ok(())
}

fn unsupported<T>(order: u32, kind: &'static str) -> Result<T, MidiImportError> {
    Err(MidiImportError::UnsupportedEvent { order, kind })
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelMessage {
    pub tick: u64,
    pub bytes: [u8; 3],
    pub len: u8,
    pub source_order: u32,
}
