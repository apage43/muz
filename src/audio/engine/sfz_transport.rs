//! Transport preparation for sessions whose SFZ instruments are track
//! instruments.
//!
//! An SFZ instrument's state at a position depends on everything it has
//! played: round robins, random layers, keyswitches, controllers, held and
//! releasing voices. Every other device seeks as it does without SFZ. So a
//! seek replays only the SFZ instruments and their schedules from zero, then
//! the callback seeks the running engine and swaps the replayed instruments
//! in. Nothing is prepared or decoded again: the rig keeps one pristine copy
//! of each instrument for the accepted description, made once, and duplicates
//! it. Replay checkpoints taken on the way let later seeks start from the
//! nearest earlier one.
use super::*;
use std::collections::BTreeMap;

/// Snapshots are spread over the timeline, at least a second apart, and stop
/// at this many: each holds a copy of every SFZ instrument's voices.
const MAX_SNAPSHOTS: usize = 32;

/// The state of one SFZ instrument that replay advances.
struct InstrumentState {
    processor: Box<dyn DeviceProcessor>,
    automation: Vec<(String, Lane, f32)>,
    idle: Idle,
}

impl InstrumentState {
    fn duplicate(&self) -> Result<Self, EngineError> {
        Ok(Self {
            processor: self.processor.duplicate().ok_or(EngineError::InvalidGraph(
                "an SFZ instrument cannot duplicate its state",
            ))?,
            automation: self.automation.clone(),
            idle: self.idle.duplicate(),
        })
    }
    fn of(device: &DeviceRuntime) -> Result<Self, EngineError> {
        Ok(Self {
            processor: device.processor.duplicate().ok_or(EngineError::InvalidGraph(
                "an SFZ instrument cannot duplicate its state",
            ))?,
            automation: device.automation.clone(),
            idle: device.idle.duplicate(),
        })
    }
    fn swap_into(&mut self, device: &mut DeviceRuntime) {
        std::mem::swap(&mut self.processor, &mut device.processor);
        std::mem::swap(&mut self.automation, &mut device.automation);
        std::mem::swap(&mut self.idle, &mut device.idle);
    }
}

struct RigTrack {
    index: usize,
    id: model::Id,
    /// Replay's working schedule; every preparation restores its position.
    schedule: TrackSchedule,
    /// A device shell that replay processes through: the processor,
    /// automation and idle state are swapped in and out of it.
    device: DeviceRuntime,
}

struct Snapshot {
    transport: RuntimeTransport,
    generation: u64,
    tracks: Vec<(SchedulePlayback, InstrumentState)>,
}

struct Working {
    transport: RuntimeTransport,
    generation: u64,
    states: Vec<InstrumentState>,
}

/// Per accepted description: pristine SFZ instruments and replay snapshots.
pub(crate) struct SfzTransportRig {
    config: AudioConfig,
    tracks: Vec<RigTrack>,
    /// Frame zero, before anything has played.
    pristine: Snapshot,
    /// Keyed by project frame, each a multiple of `config.max_frames` so that
    /// replay from a snapshot renders the same blocks as replay from zero.
    snapshots: BTreeMap<u64, Snapshot>,
    interval: u64,
    /// The last loop's state at its start frame, before its checkpoint:
    /// restarts and seeks inside that loop begin here.
    loop_start: Option<(u64, Snapshot)>,
    /// Frames rendered by the last preparation.
    replayed: u64,
    left: Vec<f32>,
    right: Vec<f32>,
}

/// SFZ instruments replayed to a position, for the running engine to adopt.
pub(crate) struct PreparedSfzTransport {
    frame: u64,
    range: Option<(u64, u64)>,
    tracks: Vec<PreparedTrack>,
    checkpoint: Option<SfzLoopCheckpoint>,
}

struct PreparedTrack {
    index: usize,
    id: model::Id,
    playback: SchedulePlayback,
    state: InstrumentState,
}

fn contains_sfz(device: &model::Device) -> bool {
    device.sfz.is_some()
        || device
            .rack
            .as_ref()
            .is_some_and(|rack| rack.branches.iter().flatten().any(contains_sfz))
}

impl SfzTransportRig {
    /// Whether every SFZ instrument of the session is a track's instrument,
    /// not a rack branch or an insert. Other sessions replay the whole engine.
    pub(crate) fn supports(session: &model::Session) -> bool {
        session.tracks.iter().all(|track| {
            !track.inserts.iter().any(contains_sfz)
                && (track.instrument.rack.is_none() || !contains_sfz(&track.instrument))
        }) && std::iter::once(&session.master)
            .chain(&session.buses)
            .all(|bus| !bus.inserts.iter().any(contains_sfz))
    }

    /// Prepare each SFZ instrument once, under the caller's host context.
    pub(crate) fn new(session: &model::Session, config: AudioConfig) -> Result<Self, EngineError> {
        validate_config(config)?;
        let mut transport = RuntimeTransport::from_session(f64::from(config.sample_rate), session)
            .map_err(EngineError::InvalidGraph)?;
        // As a replaying engine does: start running, and let SFZ schedules
        // keep that discontinuity rather than flush.
        transport.set_running(true);
        let generation = transport.discontinuity();
        let mut tracks = Vec::new();
        let mut pristine = Vec::new();
        for (index, track) in session.tracks.iter().enumerate() {
            if track.instrument.kind != model::DeviceKind::Sfz {
                continue;
            }
            crate::host::check_cancelled().map_err(|e| EngineError::Preflight(e.to_string()))?;
            let mut schedule = TrackSchedule::prepare(track, transport.timeline(), config)?;
            schedule.preserve_discontinuity(generation);
            let mut device = DeviceRuntime::new(&track.instrument, config, &session.extras)?;
            device.idle.set(track.idle_after, config);
            pristine.push((schedule.playback(), InstrumentState::of(&device)?));
            tracks.push(RigTrack { index, id: track.id.clone(), schedule, device });
        }
        let end = transport.timeline().end_project_frame().max(0.0) as u64;
        let block = config.max_frames as u64;
        let interval = (config.sample_rate as u64).max(end / MAX_SNAPSHOTS as u64);
        let interval = interval.div_ceil(block).max(1) * block;
        Ok(Self {
            config,
            tracks,
            pristine: Snapshot { transport, generation, tracks: pristine },
            snapshots: BTreeMap::new(),
            interval,
            loop_start: None,
            replayed: 0,
            left: vec![0.0; config.max_frames],
            right: vec![0.0; config.max_frames],
        })
    }

    /// Replay the SFZ instruments to `tick`. With a loop, as an engine
    /// preparing that loop does: replay to its start, checkpoint, enable it,
    /// then continue to `tick`, which must lie inside it.
    pub(crate) fn prepare(
        &mut self,
        tick: u64,
        range: Option<(u64, u64)>,
        max_replay_frames: u64,
    ) -> Result<PreparedSfzTransport, EngineError> {
        let timeline = self.pristine.transport.timeline();
        let frame_of = |tick: u64| -> Result<u64, EngineError> {
            let frame = timeline.tick_to_project_frame(tick as f64);
            if !frame.is_finite() || frame < 0.0 || frame >= u64::MAX as f64 {
                return Err(EngineError::InvalidConfig("seek frame is out of range"));
            }
            let target = frame.round() as u64;
            if target > max_replay_frames {
                return Err(EngineError::InvalidConfig("seek exceeds replay preparation budget"));
            }
            if frame > timeline.end_project_frame() {
                return Err(EngineError::InvalidConfig(
                    "prepared seek exceeds the performed timeline",
                ));
            }
            Ok(target)
        };
        let target = frame_of(tick)?;
        self.replayed = 0;
        let Some((start, end)) = range else {
            let working = self.advance_to(target)?;
            return self.finish(working, None, None);
        };
        if end <= start {
            return Err(EngineError::InvalidConfig("loop end must follow start"));
        }
        if end > timeline.end_tick() {
            return Err(EngineError::InvalidConfig("loop exceeds the performed timeline"));
        }
        let start_frame = frame_of(start)?;
        let mut working = match &self.loop_start {
            Some((frame, snapshot)) if *frame == start_frame => resume(&mut self.tracks, snapshot)?,
            _ => {
                let working = self.advance_to(start_frame)?;
                self.loop_start = Some((start_frame, snapshot(&self.tracks, &working)?));
                working
            }
        };
        let mut schedules = Vec::with_capacity(self.tracks.len());
        for (track, state) in self.tracks.iter_mut().zip(&mut working.states) {
            state.processor.prepare_loop_checkpoint()?;
            schedules.push(track.schedule.playback());
        }
        working.transport.set_loop(Some((start, end)));
        working.generation = working.transport.discontinuity();
        for (track, playback) in self.tracks.iter_mut().zip(&schedules) {
            track.schedule.restore_playback(playback, working.generation);
        }
        let mut current = working.transport.snapshot().project_frame.round() as u64;
        while current < target {
            crate::host::check_cancelled().map_err(|e| EngineError::Preflight(e.to_string()))?;
            let frames = (target - current).min(self.config.max_frames as u64) as usize;
            self.render(&mut working, frames)?;
            current += frames as u64;
            self.replayed += frames as u64;
        }
        let checkpoint = SfzLoopCheckpoint {
            enabled: true,
            range: (start, end),
            schedules: self
                .tracks
                .iter()
                .zip(schedules)
                .map(|(track, playback)| (track.index, playback))
                .collect(),
        };
        self.finish(working, Some((start, end)), Some(checkpoint))
    }

    /// Record the next snapshot after the last one. Returns false once the
    /// snapshots cover the timeline. Cancellable like a preparation.
    pub(crate) fn warm(&mut self) -> Result<bool, EngineError> {
        let next = self.snapshots.last_key_value().map_or(0, |(frame, _)| *frame) + self.interval;
        let end = self.pristine.transport.timeline().end_project_frame();
        if self.snapshots.len() >= MAX_SNAPSHOTS || next as f64 > end {
            return Ok(false);
        }
        self.advance_to(next)?;
        Ok(true)
    }

    /// Replay checkpoints recorded so far.
    #[cfg(test)]
    pub(crate) fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }

    /// Frames the last preparation rendered.
    #[cfg(test)]
    pub(crate) fn replayed_frames(&self) -> u64 {
        self.replayed
    }

    fn finish(
        &mut self,
        working: Working,
        range: Option<(u64, u64)>,
        checkpoint: Option<SfzLoopCheckpoint>,
    ) -> Result<PreparedSfzTransport, EngineError> {
        let frame = working.transport.snapshot().project_frame.round() as u64;
        let tracks = self
            .tracks
            .iter()
            .zip(working.states)
            .map(|(track, state)| PreparedTrack {
                index: track.index,
                id: track.id.clone(),
                playback: track.schedule.playback(),
                state,
            })
            .collect();
        Ok(PreparedSfzTransport { frame, range, tracks, checkpoint })
    }

    /// Replay from the latest snapshot at or before `target`, recording
    /// snapshots on the way.
    fn advance_to(&mut self, target: u64) -> Result<Working, EngineError> {
        let (mut frame, from) = self
            .snapshots
            .range(..=target)
            .next_back()
            .map_or((0, &self.pristine), |(frame, snapshot)| (*frame, snapshot));
        let mut working = resume(&mut self.tracks, from)?;
        let block = self.config.max_frames as u64;
        while frame < target {
            crate::host::check_cancelled().map_err(|e| EngineError::Preflight(e.to_string()))?;
            let frames = (target - frame).min(block);
            self.render(&mut working, frames as usize)?;
            frame += frames;
            self.replayed += frames;
            if frames == block
                && frame.is_multiple_of(self.interval)
                && self.snapshots.len() < MAX_SNAPSHOTS
                && !self.snapshots.contains_key(&frame)
            {
                self.snapshots.insert(frame, snapshot(&self.tracks, &working)?);
            }
        }
        Ok(working)
    }

    /// One hardware block of the SFZ instruments alone, as the engine renders
    /// them: the same transport blocks, schedules and device processing.
    fn render(&mut self, working: &mut Working, frames: usize) -> Result<(), EngineError> {
        let mut offset = 0;
        while offset < frames {
            let block = working.transport.block(frames - offset);
            debug_assert!(block.frames != 0);
            let reset = block.generation() != working.generation;
            working.generation = block.generation();
            let context = ProcessContext {
                frames: block.frames,
                block_start_sample: block.start_sample(),
                transport: block.snapshot,
            };
            let timeline = working.transport.timeline();
            for (track, state) in self.tracks.iter_mut().zip(&mut working.states) {
                state.swap_into(&mut track.device);
                let result = (|| {
                    if reset {
                        track.device.processor.reset();
                        track.device.idle.reset();
                    }
                    if !block.snapshot.running {
                        return Ok(());
                    }
                    let TrackSchedule::Arrangement { source, scheduler } = &mut track.schedule;
                    scheduler.schedule(source, timeline, block)?;
                    let left = &mut self.left[..block.frames];
                    let right = &mut self.right[..block.frames];
                    left.fill(0.0);
                    right.fill(0.0);
                    track.device.process(context, scheduler.events_mut(), left, right)?;
                    Ok::<_, EngineError>(())
                })();
                state.swap_into(&mut track.device);
                result?;
            }
            offset += block.frames;
            working.transport.advance(block.frames);
        }
        Ok(())
    }
}

/// Working state from a snapshot, with the schedules at its position.
fn resume(tracks: &mut [RigTrack], from: &Snapshot) -> Result<Working, EngineError> {
    let mut working = Working {
        transport: from.transport.clone(),
        generation: from.generation,
        states: Vec::with_capacity(from.tracks.len()),
    };
    for (track, (playback, state)) in tracks.iter_mut().zip(&from.tracks) {
        track.schedule.restore_playback(playback, from.generation);
        working.states.push(state.duplicate()?);
    }
    Ok(working)
}

fn snapshot(tracks: &[RigTrack], working: &Working) -> Result<Snapshot, EngineError> {
    Ok(Snapshot {
        transport: working.transport.clone(),
        generation: working.generation,
        tracks: tracks
            .iter()
            .zip(&working.states)
            .map(|(track, state)| Ok((track.schedule.playback(), state.duplicate()?)))
            .collect::<Result<_, EngineError>>()?,
    })
}

impl AudioEngine {
    /// Seek to a prepared position and adopt its replayed SFZ instruments,
    /// schedules and loop checkpoint. Every check precedes the first change.
    /// Allocation-free: the replaced instruments and checkpoint move into
    /// `prepared` for retirement off the callback.
    pub(crate) fn apply_sfz_transport(
        &mut self,
        prepared: &mut PreparedSfzTransport,
    ) -> Result<(), EngineError> {
        for track in &prepared.tracks {
            let Some(current) = self.tracks.get(track.index) else {
                return Err(EngineError::InvalidGraph("prepared SFZ track no longer exists"));
            };
            if current.id != track.id
                || current.instrument.processor.kind() != model::DeviceKind::Sfz
            {
                return Err(EngineError::InvalidGraph("prepared SFZ track no longer matches"));
            }
        }
        self.transport.set_loop(prepared.range);
        for track in &mut self.tracks {
            track.reset_delivered();
        }
        self.transport.seek_project_frame(prepared.frame);
        self.reset_audio();
        let generation = self.transport.discontinuity();
        for track in &mut prepared.tracks {
            let current = &mut self.tracks[track.index];
            track.state.swap_into(&mut current.instrument);
            current.schedule.restore_playback(&track.playback, generation);
        }
        std::mem::swap(&mut self.sfz_loop, &mut prepared.checkpoint);
        self.runtime_loop_policy = None;
        self.transport_generation = generation;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEAT: u64 = crate::compile::PPQ as u64;
    const CONFIG: AudioConfig = AudioConfig { sample_rate: 48_000.0, max_frames: 256, offline: true };

    /// Round robins, random layers, keyswitches, a controller crossfade and
    /// release triggers, with notes held across the seek points.
    fn session(dir: &std::path::Path) -> model::Session {
        for (name, frequency) in [("a.wav", 0.031), ("b.wav", 0.047), ("c.wav", 0.019), ("d.wav", 0.073)] {
            let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
            let mut wave = hound::WavWriter::create(dir.join(name), spec).unwrap();
            for frame in 0..96_000 {
                wave.write_sample(0.2 * (frame as f32 * frequency).sin()).unwrap();
            }
            wave.finalize().unwrap();
        }
        std::fs::write(dir.join("inst.sfz"), "<control> set_cc1=0\n\
            <global> ampeg_attack=0.003 ampeg_release=0.4 sw_lokey=24 sw_hikey=25 sw_default=24 lokey=48 hikey=72\n\
            <group> sw_last=24 seq_length=2\n\
            <region> seq_position=1 sample=a.wav\n\
            <region> seq_position=2 sample=b.wav fil_type=lpf_2p cutoff=900\n\
            <group> sw_last=25\n\
            <region> lorand=0 hirand=0.5 locc1=0 hicc1=63 sample=c.wav\n\
            <region> lorand=0.5 hirand=1 locc1=0 hicc1=63 sample=d.wav\n\
            <region> locc1=64 hicc1=127 sample=a.wav pitch_keycenter=62\n\
            <group> trigger=release\n\
            <region> sample=d.wav volume=-6 ampeg_release=0.05\n").unwrap();
        let path = dir.join("song.muz");
        std::fs::write(&path, r#"song({tempo: 120, tracks: [track("lead", stack([
            phrase("C4:q D4:h C4:e E4:e C#1:s G4:q C4:h D4:q C1:s E4:q C4:e C4:e D4:w"),
            cc(1, 100, at = 5b), cc(1, 0, at = 8b)
        ]), sfz("inst.sfz"))]})"#).unwrap();
        crate::compile::compile(&path).unwrap().session
    }

    fn render(engine: &mut AudioEngine, blocks: usize) -> Vec<f32> {
        let mut out = vec![0.0; blocks * 512];
        for chunk in out.chunks_mut(512) {
            engine.render_interleaved(chunk, 2).unwrap();
        }
        out
    }

    /// The engine the old path prepared: the whole graph replayed from zero.
    fn full_replay(session: &model::Session, tick: u64, range: Option<(u64, u64)>) -> AudioEngine {
        let checked = crate::description::ValidatedSession::new(session).unwrap();
        let Some((start, end)) = range else {
            return AudioEngine::prepare_seek(&checked, CONFIG, tick, u64::MAX).unwrap();
        };
        let mut engine = AudioEngine::prepare_loop(&checked, CONFIG, start, end, u64::MAX).unwrap();
        let target = engine.transport.timeline().tick_to_project_frame(tick as f64).round() as u64;
        let mut current = engine.status().transport.project_frame.round() as u64;
        let mut scratch = [0.0; 512];
        while current < target {
            let frames = (target - current).min(256) as usize;
            engine.render_interleaved(&mut scratch[..frames * 2], 2).unwrap();
            current += frames as u64;
        }
        engine
    }

    /// A running engine that has played elsewhere, then adopts a prepared seek.
    fn adopted(session: &model::Session, rig: &mut SfzTransportRig, tick: u64, range: Option<(u64, u64)>) -> AudioEngine {
        let mut engine = AudioEngine::new(session, CONFIG).unwrap();
        engine.set_running(true);
        render(&mut engine, 300);
        let mut prepared = rig.prepare(tick, range, u64::MAX).unwrap();
        engine.apply_sfz_transport(&mut prepared).unwrap();
        engine
    }

    #[test]
    fn replaying_only_sfz_instruments_matches_a_full_engine_replay() {
        let dir = tempfile::tempdir().unwrap();
        let session = session(dir.path());
        let mut rig = SfzTransportRig::new(&session, CONFIG).unwrap();
        // From zero, then again from the snapshots the first passes recorded.
        for pass in 0..2 {
            for (tick, range) in [
                (BEAT * 9 / 4, None),
                (BEAT * 21 / 4, None),
                (BEAT * 9, None),
                (BEAT * 13 / 2, Some((BEAT * 4, BEAT * 8))),
                (BEAT * 4, Some((BEAT * 4, BEAT * 8))),
            ] {
                let mut expected = full_replay(&session, tick, range);
                let mut actual = adopted(&session, &mut rig, tick, range);
                assert_eq!(actual.status().transport.project_frame, expected.status().transport.project_frame);
                assert_eq!(actual.loop_range(), expected.loop_range());
                // Long enough to wrap the loop and restore its checkpoint.
                let (expected, actual) = (render(&mut expected, 450), render(&mut actual, 450));
                assert!(expected.iter().any(|sample| *sample != 0.0), "tick {tick} is silent");
                assert!(expected == actual, "pass {pass}, tick {tick}, loop {range:?}");
            }
            if pass == 0 {
                while rig.warm().unwrap() {}
                assert!(rig.snapshot_count() >= 4);
            }
        }
    }

    #[test]
    fn seeks_replay_from_the_nearest_snapshot_and_restarts_reuse_the_loop_start() {
        let dir = tempfile::tempdir().unwrap();
        let session = session(dir.path());
        let mut rig = SfzTransportRig::new(&session, CONFIG).unwrap();
        let tick = BEAT * 11;
        rig.prepare(tick, None, u64::MAX).unwrap();
        let from_zero = rig.replayed_frames();
        assert!(from_zero >= 5 * 48_000);
        rig.prepare(tick, None, u64::MAX).unwrap();
        assert!(rig.replayed_frames() <= rig.interval, "{} frames", rig.replayed_frames());
        let range = Some((BEAT * 4, BEAT * 8));
        rig.prepare(BEAT * 4, range, u64::MAX).unwrap();
        rig.prepare(BEAT * 4, range, u64::MAX).unwrap();
        assert_eq!(rig.replayed_frames(), 0);
        assert!(rig.prepare(BEAT * 1000, None, u64::MAX).is_err());
        assert!(rig.prepare(tick, None, 1000).is_err(), "the replay budget is still enforced");
    }

    #[test]
    fn a_prepared_seek_rejects_a_different_graph_before_changing_it() {
        let dir = tempfile::tempdir().unwrap();
        let session = session(dir.path());
        let mut rig = SfzTransportRig::new(&session, CONFIG).unwrap();
        let mut other = session.clone();
        other.tracks[0].id = model::Id::new("renamed");
        let mut engine = AudioEngine::new(&other, CONFIG).unwrap();
        let before = engine.status().transport;
        let mut prepared = rig.prepare(BEAT * 3, None, u64::MAX).unwrap();
        assert!(engine.apply_sfz_transport(&mut prepared).is_err());
        assert_eq!(engine.status().transport, before);
    }
}
