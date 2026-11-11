use std::{cmp::Ordering, error::Error, fmt};

use arrayvec::ArrayVec;

use crate::{
    midi::ImportedMidi,
    model::{self, TICKS_PER_BEAT, Transport},
};

use super::{DeviceEvent, DeviceEventKind, MAX_ACTIVE_NOTES, MAX_EVENTS_PER_BLOCK};

pub type ScheduledEvents = ArrayVec<DeviceEvent, MAX_EVENTS_PER_BLOCK>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportSnapshot {
    pub sample_rate: f64,
    pub running: bool,
    /// Monotonic position in the physical audio stream.
    pub sample_position: u64,
    pub beat_position: f64,
    pub current_tick: f64,
    pub project_frame: f64,
    pub bpm: f64,
    /// The meter in force at `beat_position`.
    pub meter: [u8; 2],
    /// Start of the bar containing `beat_position`, in quarter beats.
    pub bar_start: f64,
    /// Zero-based index of that bar in the song's meter map.
    pub bar_index: u32,
    pub ended: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportBlock {
    pub snapshot: TransportSnapshot,
    pub frames: usize,
    project_start_frame: f64,
    project_end_frame: f64,
    end_tick_position: f64,
    discontinuity: u64,
}

impl TransportBlock {
    pub(crate) fn generation(self) -> u64 {
        self.discontinuity
    }
    pub fn start_sample(self) -> u64 {
        self.snapshot.sample_position
    }

    pub fn end_sample(self) -> u64 {
        self.start_sample().saturating_add(self.frames as u64)
    }

    pub fn start_project_frame(self) -> f64 {
        self.project_start_frame
    }

    pub fn end_project_frame(self) -> f64 {
        self.project_end_frame
    }

    pub fn start_tick(self) -> f64 {
        self.snapshot.beat_position * f64::from(TICKS_PER_BEAT)
    }

    pub fn end_tick(self) -> f64 {
        self.end_tick_position
    }

    pub fn start_beat(self) -> f64 {
        self.snapshot.beat_position
    }

    pub fn end_beat(self) -> f64 {
        self.end_tick_position / f64::from(TICKS_PER_BEAT)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TempoSegment {
    tick: u64,
    project_frame: f64,
    bpm: f64,
}

/// Coordinator-compiled tempo map. Its vectors are immutable on the audio thread.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoTimeline {
    sample_rate: f64,
    ppq: u32,
    segments: Vec<TempoSegment>,
    end_tick: u64,
}

impl TempoTimeline {
    pub fn compile(
        sample_rate: f64,
        transport: &Transport,
        tracks: &[model::Track],
    ) -> Result<Self, &'static str> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("tempo timeline requires a positive finite sample rate");
        }
        match transport {
            Transport::OneShot { .. } => {
                let mut ppq = None;
                let mut end_tick = 0;
                let mut tempos = Vec::new();
                for (track_order, track) in tracks.iter().enumerate() {
                    let model::TrackSource::Midi(source) = &track.source;
                    let source_ppq = source.imported.summary.ppq;
                    if source_ppq == 0 {
                        return Err("one-shot MIDI source has zero PPQ");
                    }
                    if ppq
                        .replace(source_ppq)
                        .is_some_and(|value| value != source_ppq)
                    {
                        return Err("one-shot MIDI sources disagree on PPQ");
                    }
                    end_tick = end_tick.max(source.imported.summary.end_tick);
                    tempos.extend(source.imported.tempos.iter().map(|tempo| {
                        (
                            tempo.tick,
                            tempo.source_order,
                            track_order,
                            60_000_000.0 / f64::from(tempo.micros_per_quarter),
                        )
                    }));
                }
                let ppq = ppq.unwrap_or(TICKS_PER_BEAT);
                tempos.sort_unstable_by_key(|&(tick, source_order, track_order, _)| {
                    (tick, source_order, track_order)
                });

                let mut segments = vec![TempoSegment {
                    tick: 0,
                    project_frame: 0.0,
                    bpm: 120.0,
                }];
                let mut current_tick = 0;
                let mut current_frame = 0.0;
                let mut current_bpm = 120.0;
                for (tick, _, _, bpm) in tempos {
                    if tick > current_tick {
                        current_frame += (tick - current_tick) as f64 * sample_rate * 60.0
                            / (current_bpm * f64::from(ppq));
                        current_tick = tick;
                        segments.push(TempoSegment {
                            tick,
                            project_frame: current_frame,
                            bpm,
                        });
                    } else {
                        segments
                            .last_mut()
                            .expect("tempo timeline always has an initial segment")
                            .bpm = bpm;
                    }
                    current_bpm = bpm;
                }
                Ok(Self {
                    sample_rate,
                    ppq,
                    segments,
                    end_tick,
                })
            }
        }
    }

    pub fn ppq(&self) -> u32 {
        self.ppq
    }

    pub fn end_tick(&self) -> u64 {
        self.end_tick
    }

    pub fn end_project_frame(&self) -> f64 {
        self.tick_to_project_frame(self.end_tick as f64)
    }

    pub fn tick_to_project_frame(&self, tick: f64) -> f64 {
        let tick = tick.max(0.0);
        let index = self
            .segments
            .partition_point(|segment| segment.tick as f64 <= tick)
            .saturating_sub(1);
        let segment = self.segments[index];
        segment.project_frame
            + (tick - segment.tick as f64) * self.sample_rate * 60.0
                / (segment.bpm * f64::from(self.ppq))
    }

    pub fn project_frame_to_tick(&self, frame: f64) -> f64 {
        let frame = frame.max(0.0);
        let index = self
            .segments
            .partition_point(|segment| segment.project_frame <= frame)
            .saturating_sub(1);
        let segment = self.segments[index];
        segment.tick as f64
            + (frame - segment.project_frame) * segment.bpm * f64::from(self.ppq)
                / (self.sample_rate * 60.0)
    }

    pub fn tempo_at_tick(&self, tick: f64) -> f64 {
        let index = self
            .segments
            .partition_point(|segment| segment.tick as f64 <= tick)
            .saturating_sub(1);
        self.segments[index].bpm
    }

    pub fn tempo_at_project_frame(&self, frame: f64) -> f64 {
        let index = self
            .segments
            .partition_point(|segment| segment.project_frame <= frame)
            .saturating_sub(1);
        self.segments[index].bpm
    }

    fn frames_to_next_tempo(&self, frame: f64, maximum: usize) -> usize {
        let Some(next) = self
            .segments
            .iter()
            .find(|segment| segment.project_frame > frame)
        else {
            return maximum;
        };
        let distance = (next.project_frame - frame).ceil().max(1.0) as usize;
        distance.min(maximum)
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeTransport {
    sample_rate: f64,
    running: bool,
    sample_position: u64,
    project_frame: f64,
    meter: [u8; 2],
    meter_changes: Vec<model::MeterChange>,
    timeline: TempoTimeline,
    discontinuity: u64,
    audition_loop: Option<(u64, u64)>,
}

impl RuntimeTransport {
    pub(crate) fn discontinuity(&self) -> u64 {
        self.discontinuity
    }

    pub(crate) fn audition_loop(&self) -> Option<(u64, u64)> {
        self.audition_loop
    }

    /// Constructor for a session without MIDI tracks (constant default tempo).
    pub fn new(sample_rate: f64, source: &Transport) -> Self {
        let timeline = TempoTimeline::compile(sample_rate, source, &[])
            .expect("validated transport must compile");
        Self::with_timeline(source, timeline)
    }

    pub fn from_session(sample_rate: f64, session: &model::Session) -> Result<Self, &'static str> {
        let timeline = TempoTimeline::compile(sample_rate, &session.transport, &session.tracks)?;
        Ok(Self::with_timeline(&session.transport, timeline))
    }

    fn with_timeline(source: &Transport, timeline: TempoTimeline) -> Self {
        Self {
            sample_rate: timeline.sample_rate,
            running: false,
            sample_position: 0,
            project_frame: 0.0,
            meter: source.meter(),
            meter_changes: source.meter_changes().to_vec(),
            timeline,
            discontinuity: 0,
            audition_loop: None,
        }
    }

    pub fn timeline(&self) -> &TempoTimeline {
        &self.timeline
    }

    pub fn snapshot(&self) -> TransportSnapshot {
        let tick_position = self.timeline.project_frame_to_tick(self.project_frame);
        let beat_position = tick_position / f64::from(self.timeline.ppq());
        let bar = model::bar_at(self.meter, &self.meter_changes, beat_position);
        TransportSnapshot {
            sample_rate: self.sample_rate,
            running: self.running,
            sample_position: self.sample_position,
            beat_position,
            current_tick: tick_position,
            project_frame: self.project_frame,
            bpm: self.timeline.tempo_at_project_frame(self.project_frame),
            meter: bar.meter,
            bar_start: bar.start,
            bar_index: u32::try_from(bar.number - 1).unwrap_or(u32::MAX),
            ended: self.project_frame >= self.timeline.end_project_frame(),
        }
    }

    pub fn current_tick(&self) -> f64 {
        self.timeline.project_frame_to_tick(self.project_frame)
    }

    pub fn end_tick(&self) -> u64 {
        self.timeline.end_tick()
    }

    pub fn end_project_frame(&self) -> f64 {
        self.timeline.end_project_frame()
    }

    /// Returns a non-empty slice no longer than `frames`, ending at the next tempo/end boundary.
    pub fn block(&self, frames: usize) -> TransportBlock {
        let mut slice_frames = frames;
        if self.running && frames != 0 {
            slice_frames = self
                .timeline
                .frames_to_next_tempo(self.project_frame, slice_frames);
            let end = self.timeline.end_project_frame();
            if self.project_frame < end {
                let distance = (end - self.project_frame).ceil().max(1.0) as usize;
                slice_frames = slice_frames.min(distance);
            }
        }
        if self.running
            && let Some((_, end)) = self.audition_loop
        {
            let end = self.timeline.tick_to_project_frame(end as f64);
            slice_frames = slice_frames.min((end - self.project_frame).ceil().max(1.0) as usize);
        }
        let project_end_frame = if self.running {
            self.project_frame + slice_frames as f64
        } else {
            self.project_frame
        };
        TransportBlock {
            snapshot: self.snapshot(),
            project_start_frame: self.project_frame,
            frames: slice_frames,
            project_end_frame,
            end_tick_position: self.timeline.project_frame_to_tick(project_end_frame),
            discontinuity: self.discontinuity,
        }
    }

    pub fn set_running(&mut self, running: bool) {
        if self.running == running {
            return;
        }
        if running && self.project_frame >= self.timeline.end_project_frame() {
            self.project_frame = 0.0;
        }
        self.discontinuity = self.discontinuity.wrapping_add(1);
        self.running = running;
    }

    pub fn restart(&mut self) {
        self.project_frame = 0.0;
        self.running = true;
        self.discontinuity = self.discontinuity.wrapping_add(1);
    }

    pub fn seek_ticks(&mut self, tick: u64) {
        let tick = tick.min(self.timeline.end_tick());
        self.project_frame = self.timeline.tick_to_project_frame(tick as f64);
        self.discontinuity = self.discontinuity.wrapping_add(1);
    }

    /// Seek to an exact project frame, so an offline render can start where a
    /// render from zero has a block boundary.
    pub fn seek_project_frame(&mut self, frame: u64) {
        self.project_frame = (frame as f64).min(self.timeline.end_project_frame());
        self.discontinuity = self.discontinuity.wrapping_add(1);
    }

    pub fn update_source(&mut self, source: &Transport) {
        let tick = self.current_tick();
        self.meter = source.meter();
        self.meter_changes = source.meter_changes().to_vec();
        self.project_frame = self
            .timeline
            .tick_to_project_frame(tick)
            .min(self.timeline.end_project_frame());
    }

    pub(crate) fn adopt_position_from(&mut self, previous: &Self) {
        let tick = previous.snapshot().beat_position * f64::from(self.timeline.ppq());
        // Source time/extent edits preserve sounding obligations. Only an explicit
        // transport discontinuity flushes retained voices.
        self.audition_loop = previous.audition_loop;
        self.sample_position = previous.sample_position;
        self.project_frame = self
            .timeline
            .tick_to_project_frame(tick)
            .min(self.timeline.end_project_frame());
        self.running = previous.running;
        self.discontinuity = previous.discontinuity;
    }

    pub fn set_loop(&mut self, range: Option<(u64, u64)>) {
        self.audition_loop = range.filter(|(a, b)| b > a);
        if let Some((a, b)) = self.audition_loop
            && (self.current_tick() < a as f64 || self.current_tick() >= b as f64)
        {
            self.seek_ticks(a);
        }
    }
    pub fn advance(&mut self, frames: usize) {
        self.sample_position = self.sample_position.saturating_add(frames as u64);
        if !self.running {
            return;
        }
        self.project_frame += frames as f64;
        if let Some((start, end)) = self.audition_loop
            && self.project_frame >= self.timeline.tick_to_project_frame(end as f64)
        {
            self.project_frame = self.timeline.tick_to_project_frame(start as f64);
            self.discontinuity = self.discontinuity.wrapping_add(1);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleError {
    EventCapacityExceeded,
    PendingNoteOffCapacityExceeded,
    RuntimeNoteIdExhausted,
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventCapacityExceeded => f.write_str("scheduled event capacity exceeded"),
            Self::PendingNoteOffCapacityExceeded => {
                f.write_str("pending note-off capacity exceeded")
            }
            Self::RuntimeNoteIdExhausted => f.write_str("runtime note ID space exhausted"),
        }
    }
}

impl Error for ScheduleError {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct DeliveredEvents {
    pub note_ons: u64,
    pub note_offs: u64,
    pub controllers: u64,
}

impl DeliveredEvents {
    fn record(&mut self, kind: DeviceEventKind) {
        match kind {
            DeviceEventKind::Midi { .. } => self.controllers = self.controllers.saturating_add(1),
            DeviceEventKind::NoteOn { .. } => self.note_ons = self.note_ons.saturating_add(1),
            DeviceEventKind::NoteOff { .. } | DeviceEventKind::NoteChoke { .. } => {
                self.note_offs = self.note_offs.saturating_add(1)
            }
            DeviceEventKind::Controller { .. } => {
                self.controllers = self.controllers.saturating_add(1);
            }
            DeviceEventKind::Flush | DeviceEventKind::NoteExpression { .. } => {}
        }
    }

    pub fn merge(&mut self, other: Self) {
        self.note_ons = self.note_ons.saturating_add(other.note_ons);
        self.note_offs = self.note_offs.saturating_add(other.note_offs);
        self.controllers = self.controllers.saturating_add(other.controllers);
    }
}

#[derive(Clone, Copy, Debug)]
struct PreparedNote {
    sample_zone: Option<usize>,
    articulation: Option<u16>,
    identity: u64,
    pitch: f32,
    expression: crate::expression::Program,
    frame: u64,
    off: u64,
    channel: u8,
    key: u8,
    velocity: f32,
    release: f32,
}
#[derive(Clone, Copy, Debug)]
struct PreparedControl {
    frame: u64,
    channel: u8,
    controller: u8,
    value: u8,
}
#[derive(Clone, Copy, Debug)]
struct ActiveNote {
    expression: crate::expression::Program,
    origin: i128,
    next_expression: u64,
    off: u64,
    id: u64,
    channel: u8,
    key: u8,
    velocity: f32,
}
fn expression_cost(program: &crate::expression::Program, frames: usize) -> usize {
    let kinds = program.kinds().count();
    kinds
        * if program.changing() {
            frames.div_ceil(128) + 1
        } else {
            1
        }
}
#[derive(Clone, Debug)]
pub struct ArrangementScheduler {
    restoration: Vec<Vec<(u64, DeviceEventKind)>>,
    interval_max: Vec<u64>,
    max_block_cost: usize,
    max_frames: usize,
    notes: Vec<PreparedNote>,
    controls: Vec<PreparedControl>,
    cursor: usize,
    control_cursor: usize,
    active: Box<ArrayVec<ActiveNote, MAX_ACTIVE_NOTES>>,
    next_id: u64,
    last_end: Option<u64>,
    discontinuity: u64,
    delivered: DeliveredEvents,
    reload: bool,
    messages: Vec<(u64, crate::midi::ChannelMessage)>,
    message_cursor: usize,
}
impl ArrangementScheduler {
    pub(crate) fn preserve_discontinuity(&mut self, discontinuity: u64) {
        self.discontinuity = discontinuity;
    }
    pub(crate) fn restore_playback_from(&mut self, source: &Self, discontinuity: u64) {
        self.cursor = source.cursor;
        self.control_cursor = source.control_cursor;
        self.active.clear();
        self.active.extend(source.active.iter().copied());
        self.next_id = source.next_id;
        self.last_end = source.last_end;
        self.discontinuity = discontinuity;
        self.delivered = source.delivered;
        self.reload = source.reload;
        self.message_cursor = source.message_cursor;
    }

    pub fn compile(
        midi: &ImportedMidi,
        timeline: &TempoTimeline,
        max_frames: usize,
        sfz: Option<&crate::model::SfzConfig>,
    ) -> Result<Self, String> {
        if max_frames == 0 || max_frames > super::MAX_AUDIO_FRAMES {
            return Err("invalid schedule block capacity".into());
        }
        crate::snapshot::validate_midi(midi).map_err(|e| e.to_string())?;
        for n in &midi.notes {
            crate::host::check_cancelled().map_err(|e| e.to_string())?;
            for tick in [
                n.start_tick,
                n.start_tick
                    .checked_add(n.duration_ticks)
                    .ok_or("note tick overflow")?,
            ] {
                let frame = timeline.tick_to_project_frame(tick as f64);
                if !frame.is_finite()
                    || frame < 0.
                    || frame >= (u64::MAX - super::MAX_AUDIO_FRAMES as u64) as f64
                {
                    return Err("performed frame overflow".into());
                }
            }
        }
        let quantize =
            |tick: u64| timeline.tick_to_project_frame(tick as f64).round().max(0.0) as u64;
        // Articulations are resolved here, off the audio thread, so the device
        // receives one explicit code with the note-on.
        let mut articulations = Vec::with_capacity(midi.notes.len());
        for n in &midi.notes {
            let choice = n.annotations.get("articulation");
            articulations.push(match sfz {
                Some(config) => config
                    .articulation_code(choice)
                    .map_err(|e| format!("note '{}': {e}", n.id))?,
                None if choice.is_some() => {
                    return Err(format!(
                        "note '{}': articulation requires an sfz() instrument",
                        n.id
                    ));
                }
                None => None,
            });
        }
        // A note's identity counts only notes at its own onset, channel and key,
        // so adding or removing any other note leaves it unchanged.
        let mut identical = std::collections::HashMap::<(u64, u8, u8), u32>::new();
        let identities: Vec<_> = midi
            .notes
            .iter()
            .map(|n| {
                let ordinal = identical
                    .entry((n.start_tick, n.channel, n.key))
                    .or_default();
                let identity =
                    super::device::score_note_identity(n.start_tick, n.channel, n.key, *ordinal);
                *ordinal += 1;
                identity
            })
            .collect();
        let mut notes: Vec<_> = midi
            .notes
            .iter()
            .zip(articulations)
            .zip(identities)
            .map(|((n, articulation), identity)| PreparedNote {
                articulation,
                identity,
                sample_zone: n
                    .annotations
                    .get("sample_zone")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as usize),
                pitch: n.performance.map_or(n.key as f32, |p| p.pitch as f32),
                expression: n.performance.map(|p| p.expression).unwrap_or_default(),
                frame: quantize(n.start_tick),
                off: quantize(n.start_tick + n.duration_ticks).max(quantize(n.start_tick) + 1),
                channel: n.channel,
                key: n.key,
                velocity: n
                    .performance
                    .map_or(n.attack_velocity as f32 / 127.0, |p| p.velocity as f32),
                release: n.release_velocity as f32 / 127.0,
            })
            .collect();
        notes.sort_by_key(|n| n.frame);
        let mut controls: Vec<_> = midi
            .controllers
            .iter()
            .map(|c| PreparedControl {
                frame: quantize(c.tick),
                channel: c.channel,
                controller: c.controller,
                value: c.value,
            })
            .collect();
        controls.sort_by_key(|c| c.frame);
        let mut restoration =
            std::collections::BTreeMap::<usize, Vec<(u64, DeviceEventKind)>>::new();
        for c in &controls {
            restoration
                .entry(c.channel as usize * 128 + c.controller as usize)
                .or_default()
                .push((
                    c.frame,
                    DeviceEventKind::Controller {
                        channel: c.channel,
                        controller: c.controller,
                        value: c.value,
                    },
                ));
        }
        for m in &midi.messages {
            let status = m.bytes[0] >> 4;
            if matches!(status, 12..=14) {
                restoration
                    .entry(2048 + (m.bytes[0] & 15) as usize * 3 + (status - 12) as usize)
                    .or_default()
                    .push((
                        quantize(m.tick),
                        DeviceEventKind::Midi {
                            bytes: m.bytes,
                            len: m.len,
                        },
                    ));
            }
        }
        let restoration: Vec<_> = restoration.into_values().collect();
        let mut changes =
            Vec::with_capacity(notes.len() * 2 + controls.len() * 2 + midi.messages.len() * 2);
        let mut interval = |start: u64, end: u64, cost: usize| {
            changes.push((start.saturating_sub(max_frames as u64 - 1), cost as isize));
            changes.push((end.saturating_add(1), -(cost as isize)));
        };
        for n in &notes {
            interval(
                n.frame,
                n.off,
                2 + expression_cost(&n.expression, max_frames),
            );
        }
        for c in &controls {
            interval(c.frame, c.frame, 1);
        }
        for m in &midi.messages {
            let frame = quantize(m.tick);
            interval(frame, frame, 1);
        }
        changes.sort_unstable_by_key(|v| v.0);
        let mut cost = 0isize;
        let mut peak = 0usize;
        let mut i = 0;
        while i < changes.len() {
            let frame = changes[i].0;
            while i < changes.len() && changes[i].0 == frame {
                cost += changes[i].1;
                i += 1;
            }
            peak = peak.max(cost as usize);
        }
        let max_block_cost = peak + restoration.len() + 1;
        if max_block_cost > MAX_EVENTS_PER_BLOCK {
            return Err(format!(
                "schedule requires up to {max_block_cost} callback events including seek restoration; capacity is {MAX_EVENTS_PER_BLOCK}"
            ));
        }
        let mut interval_max = vec![0; notes.len().saturating_mul(4).max(2)];
        fn index(notes: &[PreparedNote], tree: &mut [u64], node: usize, a: usize, b: usize) -> u64 {
            if a == b {
                return 0;
            }
            let mid = (a + b) / 2;
            let max = notes[mid]
                .off
                .max(index(notes, tree, node * 2, a, mid))
                .max(index(notes, tree, node * 2 + 1, mid + 1, b));
            tree[node] = max;
            max
        }
        index(&notes, &mut interval_max, 1, 0, notes.len());
        Ok(Self {
            restoration,
            interval_max,
            max_block_cost,
            max_frames,
            messages: midi
                .messages
                .iter()
                .map(|m| {
                    (
                        timeline.tick_to_project_frame(m.tick as f64).round() as u64,
                        *m,
                    )
                })
                .collect(),
            message_cursor: 0,
            notes,
            controls,
            cursor: 0,
            control_cursor: 0,
            active: Box::new(ArrayVec::new()),
            next_id: 1,
            last_end: None,
            discontinuity: 0,
            delivered: DeliveredEvents::default(),
            reload: false,
        })
    }
    pub(crate) fn can_adopt(&self, previous: &Self) -> bool {
        let retained: usize = previous
            .active
            .iter()
            .map(|n| 1 + expression_cost(&n.expression, self.max_frames))
            .sum();
        self.max_block_cost + retained <= MAX_EVENTS_PER_BLOCK
    }
    pub fn delivered(&self) -> DeliveredEvents {
        self.delivered
    }
    fn sounding_at(
        &self,
        node: usize,
        a: usize,
        b: usize,
        start: u64,
        out: &mut ArrayVec<usize, MAX_ACTIVE_NOTES>,
    ) -> Result<(), ScheduleError> {
        if a == b || self.interval_max[node] <= start {
            return Ok(());
        }
        let mid = (a + b) / 2;
        self.sounding_at(node * 2, a, mid, start, out)?;
        if self.notes[mid].frame < start {
            if self.notes[mid].off > start {
                out.try_push(mid)
                    .map_err(|_| ScheduleError::PendingNoteOffCapacityExceeded)?;
            }
            self.sounding_at(node * 2 + 1, mid + 1, b, start, out)?;
        }
        Ok(())
    }
    pub fn reset_delivered(&mut self) {
        self.delivered = DeliveredEvents::default();
    }
    /// Carry already sounding voices and their original release obligations into a replacement.
    pub fn adopt(&mut self, previous: &mut Self) {
        std::mem::swap(&mut self.active, &mut previous.active);
        self.next_id = previous.next_id;
        self.delivered = previous.delivered;
        self.discontinuity = previous.discontinuity;
        self.last_end = previous.last_end;
        self.reload = true;
    }
    pub fn schedule(
        &mut self,
        _midi: &ImportedMidi,
        _timeline: &TempoTimeline,
        block: TransportBlock,
    ) -> Result<ScheduledEvents, ScheduleError> {
        let start = block.start_project_frame().round().max(0.0) as u64;
        // Continue release obligations and processing into the effect tail after musical end.
        let end = start + block.frames as u64;
        if self.reload
            && let Some(previous) = self.last_end
        {
            let delta = start as i128 - previous as i128;
            for n in self.active.iter_mut() {
                n.off = (n.off as i128 + delta).max(0) as u64;
                n.origin += delta;
                if n.next_expression != u64::MAX {
                    n.next_expression = (n.next_expression as i128 + delta).max(0) as u64;
                }
            }
        }
        let discontinuous = block.discontinuity != self.discontinuity;
        let seek = discontinuous
            || self.last_end.is_none()
            || self.last_end.is_some_and(|last| last != start);
        let mut events = ScheduledEvents::new();
        if discontinuous {
            self.active.clear();
            push_event(
                &mut events,
                DeviceEvent {
                    offset: 0,
                    kind: DeviceEventKind::Flush,
                },
            )?;
        }
        if seek || self.reload {
            self.cursor = self.notes.partition_point(|n| n.frame < start);
            self.message_cursor = self.messages.partition_point(|(frame, _)| *frame < start);
            self.control_cursor = self.controls.partition_point(|c| c.frame < start);
            if block.snapshot.running {
                for history in &self.restoration {
                    let at = history.partition_point(|(frame, _)| *frame < start);
                    if at > 0 {
                        push_event(
                            &mut events,
                            DeviceEvent {
                                offset: 0,
                                kind: history[at - 1].1,
                            },
                        )?;
                    }
                }
                if seek && !self.reload {
                    let mut sounding = ArrayVec::<usize, MAX_ACTIVE_NOTES>::new();
                    self.sounding_at(1, 0, self.notes.len(), start, &mut sounding)?;
                    for i in sounding {
                        let n = self.notes[i];
                        self.start_note(n, 0, start.saturating_sub(n.frame), &mut events)?;
                    }
                }
            }
            self.reload = false;
        }
        if block.snapshot.running {
            while self.cursor < self.notes.len() && self.notes[self.cursor].frame < end {
                let n = self.notes[self.cursor];
                if n.frame >= start {
                    self.start_note(n, (n.frame - start) as u32, 0, &mut events)?;
                }
                self.cursor += 1;
            }
            while self.control_cursor < self.controls.len()
                && self.controls[self.control_cursor].frame < end
            {
                let c = self.controls[self.control_cursor];
                push_event(
                    &mut events,
                    DeviceEvent {
                        offset: c.frame.saturating_sub(start) as u32,
                        kind: DeviceEventKind::Controller {
                            channel: c.channel,
                            controller: c.controller,
                            value: c.value,
                        },
                    },
                )?;
                self.control_cursor += 1;
            }
            while self.message_cursor < self.messages.len()
                && self.messages[self.message_cursor].0 < end
            {
                let (frame, m) = self.messages[self.message_cursor];
                push_event(
                    &mut events,
                    DeviceEvent {
                        offset: frame.saturating_sub(start) as u32,
                        kind: DeviceEventKind::Midi {
                            bytes: m.bytes,
                            len: m.len,
                        },
                    },
                )?;
                self.message_cursor += 1;
            }
            let mut i = 0;
            while i < self.active.len() {
                let n = &mut self.active[i];
                while n.expression.len > 0 && n.next_expression < end.min(n.off) {
                    let phase = (n.next_expression as i128 - n.origin).max(0) as f32
                        / (n.off as i128 - n.origin).max(1) as f32;
                    for kind in n.expression.kinds() {
                        if let Some(value) = n.expression.value(kind, phase) {
                            push_event(
                                &mut events,
                                DeviceEvent {
                                    offset: n.next_expression.saturating_sub(start) as u32,
                                    kind: DeviceEventKind::NoteExpression {
                                        note_id: n.id,
                                        channel: n.channel,
                                        key: n.key,
                                        expression: kind as u16,
                                        value: value as f64,
                                    },
                                },
                            )?;
                        }
                    }
                    n.next_expression = if n.expression.changing() {
                        n.next_expression + 128
                    } else {
                        u64::MAX
                    };
                }
                let n = self.active[i];
                if n.off < end {
                    push_event(
                        &mut events,
                        DeviceEvent {
                            offset: n.off.saturating_sub(start) as u32,
                            kind: DeviceEventKind::NoteOff {
                                note_id: n.id,
                                channel: n.channel,
                                key: n.key,
                                velocity: n.velocity,
                            },
                        },
                    )?;
                    self.active.swap_remove(i);
                } else {
                    i += 1;
                }
            }
        }
        events.sort_unstable_by(compare_events);
        for ev in &events {
            self.delivered.record(ev.kind);
        }
        self.discontinuity = block.discontinuity;
        self.last_end = Some(if block.snapshot.running { end } else { start });
        Ok(events)
    }
    fn start_note(
        &mut self,
        n: PreparedNote,
        offset: u32,
        elapsed_frames: u64,
        events: &mut ScheduledEvents,
    ) -> Result<(), ScheduleError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(ScheduleError::RuntimeNoteIdExhausted)?;
        self.active
            .try_push(ActiveNote {
                expression: n.expression,
                origin: n.frame as i128,
                next_expression: n.frame + elapsed_frames,
                off: n.off,
                id,
                channel: n.channel,
                key: n.key,
                velocity: n.release,
            })
            .map_err(|_| ScheduleError::PendingNoteOffCapacityExceeded)?;
        push_event(
            events,
            DeviceEvent {
                offset,
                kind: DeviceEventKind::NoteOn {
                    sample_zone: n.sample_zone,
                    articulation: n.articulation,
                    identity: Some(n.identity),
                    pitch: n.pitch,
                    elapsed_frames,
                    note_id: id,
                    channel: n.channel,
                    key: n.key,
                    velocity: n.velocity,
                },
            },
        )
    }
}

fn push_event(events: &mut ScheduledEvents, event: DeviceEvent) -> Result<(), ScheduleError> {
    events
        .try_push(event)
        .map_err(|_| ScheduleError::EventCapacityExceeded)
}

fn compare_events(left: &DeviceEvent, right: &DeviceEvent) -> Ordering {
    left.offset
        .cmp(&right.offset)
        .then_with(|| event_kind_order(left.kind).cmp(&event_kind_order(right.kind)))
        .then_with(|| event_note_id(left.kind).cmp(&event_note_id(right.kind)))
}

fn event_kind_order(kind: DeviceEventKind) -> u8 {
    match kind {
        DeviceEventKind::Flush => 0,
        DeviceEventKind::NoteOff { .. } | DeviceEventKind::NoteChoke { .. } => 1,
        DeviceEventKind::Controller { .. } | DeviceEventKind::Midi { .. } => 2,
        DeviceEventKind::NoteOn { .. } => 3,
        DeviceEventKind::NoteExpression { .. } => 4,
    }
}

fn event_note_id(kind: DeviceEventKind) -> u64 {
    match kind {
        DeviceEventKind::NoteOn { note_id, .. }
        | DeviceEventKind::NoteOff { note_id, .. }
        | DeviceEventKind::NoteChoke { note_id, .. }
        | DeviceEventKind::NoteExpression { note_id, .. } => note_id,
        DeviceEventKind::Midi { bytes, .. } => match bytes[0] >> 4 {
            12 => 1,
            13 => 2,
            14 => 3,
            _ => 4,
        },
        DeviceEventKind::Controller { controller, .. } => u64::from(controller),
        DeviceEventKind::Flush => 0,
    }
}

#[cfg(test)]
mod event_order_tests {
    use super::*;

    /// A note-off and a note-on at the same frame are delivered off first, whatever
    /// their note ids or arrival order; mono voice continuation depends on this.
    #[test]
    fn equal_frame_note_off_precedes_note_on() {
        let on = |note_id| DeviceEvent {
            offset: 100,
            kind: DeviceEventKind::NoteOn {
                sample_zone: None,
                articulation: None,
                identity: None,
                pitch: 60.0,
                elapsed_frames: 0,
                note_id,
                channel: 0,
                key: 60,
                velocity: 1.0,
            },
        };
        let off = |note_id| DeviceEvent {
            offset: 100,
            kind: DeviceEventKind::NoteOff {
                note_id,
                channel: 0,
                key: 60,
                velocity: 0.0,
            },
        };
        for mut events in [vec![on(1), off(2)], vec![off(2), on(1)]] {
            events.sort_unstable_by(compare_events);
            assert!(matches!(events[0].kind, DeviceEventKind::NoteOff { .. }));
            assert!(matches!(events[1].kind, DeviceEventKind::NoteOn { .. }));
        }
        let mut later = vec![on(1), DeviceEvent { offset: 99, ..on(2) }];
        later.sort_unstable_by(compare_events);
        assert_eq!(later[0].offset, 99);
    }
}
