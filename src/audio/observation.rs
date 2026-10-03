//! Prepared native listening/capture state. Only DTO conversion runs on the coordinator.
use std::{
    cell::UnsafeCell,
    sync::{Arc, atomic::{AtomicU8, AtomicU64, Ordering}},
};

use crate::{model::Session, native::{DrainCount, ObservationFrame, ObservationSelection, ObservationTarget, TrackMeter}};
use super::{AudioConfig, EngineError};

const SLOT_COUNT: usize = 4;
const MAX_METERS: usize = 1_000;
const MAX_DETAIL: usize = 2_048;
const FREE: u8 = 0;
const WRITING: u8 = 1;
const READY: u8 = 2;
const READING: u8 = 3;

/// Index-only payload: the submitting output operation supplies its revision guard.
#[derive(Debug)]
pub(crate) struct PreparedAudibility {
    pub(super) enabled: Vec<bool>,
}

impl PreparedAudibility {
    pub(crate) fn new(session: &Session, ids: &[String]) -> Result<Self, EngineError> {
        let tracks: Vec<_> = session.tracks.iter().map(|t| t.id.as_str().to_owned()).collect();
        Self::from_track_ids(&tracks, ids)
    }

    pub(crate) fn from_track_ids(tracks: &[String], ids: &[String]) -> Result<Self, EngineError> {
        let mut enabled = vec![false; tracks.len()];
        for id in ids {
            let index = tracks.iter().position(|track| track == id)
                .ok_or(EngineError::InvalidGraph("unknown audition track"))?;
            if enabled[index] {
                return Err(EngineError::InvalidGraph("duplicate audition track"));
            }
            enabled[index] = true;
        }
        Ok(Self { enabled })
    }
}

#[derive(Default)]
struct Counters {
    sequence: AtomicU64,
    dropped: AtomicU64,
}

impl Counters {
    fn next_sequence(&self) -> Option<u64> {
        self.sequence.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
            .ok().map(|previous| previous + 1)
    }

    fn drop_frame(&self) {
        let _ = self.dropped.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_add(1))
        });
    }
}

struct CapturedFrame {
    revision: u64,
    generation: u64,
    epoch: u64,
    sequence: u64,
    sample_position: u64,
    running: bool,
    meters: bool,
    detail: bool,
    master: [f32; 2],
    tracks: Box<[[f32; 2]]>,
    samples: [f32; MAX_DETAIL],
    sample_count: usize,
}

struct Slot {
    state: AtomicU8,
    sequence: AtomicU64,
    frame: UnsafeCell<CapturedFrame>,
}

// Access to frame is exclusive: FREE/READY -> WRITING on the single producer,
// READY -> READING on the single consumer. Release publication and Acquire claims
// synchronize the contents. Neither side touches a slot owned by the other.
unsafe impl Sync for Slot {}

struct Pool {
    slots: [Slot; SLOT_COUNT],
    epoch: AtomicU64,
    counters: Arc<Counters>,
}

impl Pool {
    fn flush(&self) {
        for slot in &self.slots {
            if slot.state.compare_exchange(READY, FREE, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                self.counters.drop_frame();
            }
        }
    }

    fn claim_write(&self) -> Option<&Slot> {
        for slot in &self.slots {
            if slot.state.compare_exchange(FREE, WRITING, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                return Some(slot);
            }
        }
        // A stalled consumer cannot stall the callback. Reuse the oldest unread
        // slot, never a slot whose conversion is in progress.
        let oldest = self.slots.iter().filter(|s| s.state.load(Ordering::Acquire) == READY)
            .min_by_key(|s| s.sequence.load(Ordering::Relaxed));
        if let Some(slot) = oldest {
            if slot.state.compare_exchange(READY, WRITING, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                self.counters.drop_frame();
                return Some(slot);
            }
        }
        self.counters.drop_frame();
        None
    }
}

#[derive(Clone, Copy)]
enum TargetIndex {
    Master,
    Track(usize),
}

/// Owned callback producer. Returning its ownership is mandatory on replacement.
pub(crate) struct PreparedObservation {
    pool: Arc<Pool>,
    tracks: Vec<usize>,
    master_meter: bool,
    target: Option<TargetIndex>,
    meter_period: f64,
    detail_period: f64,
    meter_until: f64,
    detail_until: f64,
    running: Option<bool>,
    revision: u64,
    generation: u64,
}

/// Single coordinator consumer. IDs are never resolved or cloned by the callback.
pub(crate) struct ObservationConsumer {
    pool: Arc<Pool>,
    tracks: Vec<String>,
    master_meter: bool,
    target: Option<ObservationTarget>,
    sample_rate: f32,
}

impl std::fmt::Debug for PreparedObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedObservation").field("meter_count", &self.tracks.len()).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ObservationConsumer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObservationConsumer").field("tracks", &self.tracks).field("target", &self.target).finish_non_exhaustive()
    }
}

impl PreparedObservation {
    pub(crate) fn new(session: &Session, config: AudioConfig, selection: &ObservationSelection)
        -> Result<(Self, ObservationConsumer), EngineError>
    {
        let tracks: Vec<_> = session.tracks.iter().map(|t| t.id.as_str().to_owned()).collect();
        Self::from_track_ids(&tracks, config, selection)
    }

    pub(crate) fn new_continuing(session: &Session, config: AudioConfig, selection: &ObservationSelection, previous: &ObservationConsumer)
        -> Result<(Self, ObservationConsumer), EngineError>
    {
        let tracks: Vec<_> = session.tracks.iter().map(|t| t.id.as_str().to_owned()).collect();
        Self::from_track_ids_continuing(&tracks, config, selection, previous)
    }

    pub(crate) fn from_track_ids(tracks: &[String], config: AudioConfig, selection: &ObservationSelection)
        -> Result<(Self, ObservationConsumer), EngineError>
    {
        Self::prepare(tracks, config, selection, Arc::new(Counters::default()))
    }

    pub(crate) fn from_track_ids_continuing(tracks: &[String], config: AudioConfig, selection: &ObservationSelection, previous: &ObservationConsumer)
        -> Result<(Self, ObservationConsumer), EngineError>
    {
        Self::prepare(tracks, config, selection, Arc::clone(&previous.pool.counters))
    }

    fn prepare(track_ids: &[String], config: AudioConfig, selection: &ObservationSelection, counters: Arc<Counters>)
        -> Result<(Self, ObservationConsumer), EngineError>
    {
        if !config.sample_rate.is_finite() || config.sample_rate <= 0.0 {
            return Err(EngineError::InvalidConfig("observation sample rate must be positive and finite"));
        }
        if selection.meter_tracks.len() > MAX_METERS {
            return Err(EngineError::InvalidConfig("observation exceeds 1000 track meters"));
        }
        let meters = selection.master_meter || !selection.meter_tracks.is_empty();
        if meters && selection.meter_hz > 20 {
            return Err(EngineError::InvalidConfig("meter frequency exceeds 20 Hz"));
        }
        if selection.analysis.is_some() && selection.analysis_hz > 10 {
            return Err(EngineError::InvalidConfig("analysis frequency exceeds 10 Hz"));
        }
        let mut tracks = Vec::with_capacity(selection.meter_tracks.len());
        for id in &selection.meter_tracks {
            let index = track_ids.iter().position(|track| track == id)
                .ok_or(EngineError::InvalidGraph("unknown observation track"))?;
            if tracks.contains(&index) {
                return Err(EngineError::InvalidGraph("duplicate observation track"));
            }
            tracks.push(index);
        }
        let target = match &selection.analysis {
            None => None,
            Some(ObservationTarget::Master) => Some(TargetIndex::Master),
            Some(ObservationTarget::Track(id)) => Some(TargetIndex::Track(
                track_ids.iter().position(|track| track == id)
                    .ok_or(EngineError::InvalidGraph("unknown observation target"))?
            )),
        };
        let meter_enabled = meters && selection.meter_hz != 0;
        let detail_enabled = target.is_some() && selection.analysis_hz != 0;
        let pool = Arc::new(Pool {
            epoch: AtomicU64::new(0),
            counters,
            slots: std::array::from_fn(|_| Slot {
                state: AtomicU8::new(FREE),
                sequence: AtomicU64::new(0),
                frame: UnsafeCell::new(CapturedFrame {
                    revision: 0, generation: 0, epoch: 0, sequence: 0,
                    sample_position: 0, running: false, meters: false, detail: false,
                    master: [0.0; 2], tracks: vec![[0.0; 2]; tracks.len()].into_boxed_slice(),
                    samples: [0.0; MAX_DETAIL], sample_count: 0,
                }),
            }),
        });
        let consumer = ObservationConsumer {
            pool: Arc::clone(&pool), tracks: selection.meter_tracks.clone(),
            master_meter: selection.master_meter,
            target: if detail_enabled { selection.analysis.clone() } else { None },
            sample_rate: config.sample_rate,
        };
        Ok((Self {
            pool, tracks, master_meter: selection.master_meter,
            target: if detail_enabled { target } else { None },
            meter_period: if meter_enabled { f64::from(config.sample_rate) / f64::from(selection.meter_hz) } else { 0.0 },
            detail_period: if detail_enabled { f64::from(config.sample_rate) / f64::from(selection.analysis_hz) } else { 0.0 },
            meter_until: 0.0, detail_until: 0.0, running: None, revision: 0, generation: 0,
        }, consumer))
    }

    pub(super) fn set_identity(&mut self, revision: u64, generation: u64) {
        if self.revision != revision || self.generation != generation {
            self.revision = revision;
            self.generation = generation;
            self.pool.epoch.fetch_add(1, Ordering::AcqRel);
            self.pool.flush();
        }
    }

    pub(super) fn retire(&self) {
        self.pool.epoch.fetch_add(1, Ordering::AcqRel);
        self.pool.flush();
    }

    /// Capture a completed hardware block. Track buffers are post-insert/pan,
    /// master is post-master with the audible transaction fade applied on read.
    pub(super) fn capture<'a>(
        &mut self, frames: usize, sample_position: u64, running: bool,
        track_audio: impl Fn(usize) -> (&'a [f32], &'a [f32]),
        master: (&[f32], &[f32]), master_gain: impl Fn(usize) -> f32,
    ) {
        if frames == 0 || (self.meter_period == 0.0 && self.detail_period == 0.0) {
            return;
        }
        let paused = self.running != Some(false) && !running;
        self.running = Some(running);
        let meters = clock_due(&mut self.meter_until, self.meter_period, frames)
            || (paused && self.meter_period != 0.0);
        let detail = clock_due(&mut self.detail_until, self.detail_period, frames);
        if !meters && !detail && !paused {
            return;
        }
        let Some(sequence) = self.pool.counters.next_sequence() else {
            self.pool.counters.drop_frame();
            return;
        };
        let Some(slot) = self.pool.claim_write() else { return; };
        // SAFETY: claim_write exclusively owns this WRITING slot.
        let frame = unsafe { &mut *slot.frame.get() };
        frame.revision = self.revision;
        frame.generation = self.generation;
        frame.epoch = self.pool.epoch.load(Ordering::Acquire);
        frame.sequence = sequence;
        frame.sample_position = sample_position;
        frame.running = running;
        frame.meters = meters;
        frame.detail = detail && running;
        frame.sample_count = 0;
        frame.master = [0.0; 2];
        if meters {
            if self.master_meter && running {
                frame.master = meter(master.0, master.1, frames, &master_gain);
            }
            for (index, values) in self.tracks.iter().zip(frame.tracks.iter_mut()) {
                *values = if running {
                    let (left, right) = track_audio(*index);
                    meter(left, right, frames, |_| 1.0)
                } else { [0.0; 2] };
            }
        }
        if frame.detail {
            let (left, right) = match self.target {
                Some(TargetIndex::Master) => master,
                Some(TargetIndex::Track(index)) => track_audio(index),
                None => unreachable!("detail clock requires prepared target"),
            };
            frame.sample_count = frames.min(MAX_DETAIL);
            for (index, sample) in frame.samples[..frame.sample_count].iter_mut().enumerate() {
                let gain = if matches!(self.target, Some(TargetIndex::Master)) { master_gain(index) } else { 1.0 };
                *sample = (left[index] * 0.5 + right[index] * 0.5) * gain;
            }
        }
        slot.sequence.store(sequence, Ordering::Relaxed);
        slot.state.store(READY, Ordering::Release);
    }
}

fn clock_due(until: &mut f64, period: f64, frames: usize) -> bool {
    if period == 0.0 { return false; }
    let due = *until <= 0.0;
    if due { *until += period; }
    *until -= frames as f64;
    // Huge hardware blocks still publish at most one frame per block.
    if *until < -period { *until %= period; }
    due
}

fn meter(left: &[f32], right: &[f32], frames: usize, gain: impl Fn(usize) -> f32) -> [f32; 2] {
    let mut peak = 0.0_f32;
    let mut squares = 0.0_f64;
    for index in 0..frames {
        let gain = gain(index);
        for value in [left[index] * gain, right[index] * gain] {
            let value = if value.is_finite() { value } else { 0.0 };
            peak = peak.max(value.abs());
            squares += f64::from(value) * f64::from(value);
        }
    }
    [peak, (squares / (frames * 2) as f64).sqrt() as f32]
}

impl ObservationConsumer {
    pub(crate) fn dropped(&self) -> u64 {
        self.pool.counters.dropped.load(Ordering::Relaxed)
    }

    pub(crate) fn flush(&mut self) {
        self.pool.flush();
    }

    pub(crate) fn drain(&mut self, output: &mut [Option<ObservationFrame>]) -> DrainCount {
        let mut written = 0;
        for destination in output.iter_mut().filter(|slot| slot.is_none()) {
            let newest = self.pool.slots.iter().filter(|s| s.state.load(Ordering::Acquire) == READY)
                .max_by_key(|s| s.sequence.load(Ordering::Relaxed));
            let Some(slot) = newest else { break; };
            if slot.state.compare_exchange(READY, READING, Ordering::Acquire, Ordering::Relaxed).is_err() {
                continue;
            }
            // SAFETY: this consumer exclusively owns the READING slot until the
            // release below. DTO allocation/cloning is coordinator-only.
            let frame = unsafe { &*slot.frame.get() };
            let sequence = frame.sequence;
            let epoch = frame.epoch;
            let converted = ObservationFrame {
                revision: frame.revision, transport_generation: frame.generation,
                sequence, sample_position: frame.sample_position, sample_rate: self.sample_rate,
                running: frame.running,
                master: (frame.meters && self.master_meter).then_some(frame.master),
                tracks: if frame.meters {
                    self.tracks.iter().zip(frame.tracks.iter()).map(|(id, values)| TrackMeter {
                        id: id.clone(), peak: values[0], rms: values[1],
                    }).collect()
                } else { Vec::new() },
                target: if frame.detail { self.target.clone() } else { None },
                mono_samples: frame.samples[..frame.sample_count].to_vec(),
                dropped: 0,
            };
            slot.state.store(FREE, Ordering::Release);
            for older in &self.pool.slots {
                if older.state.compare_exchange(READY, READING, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                    // Claim first: the producer may have reused the slot since
                    // the initial scan. Never discard a newly published frame.
                    if older.sequence.load(Ordering::Relaxed) < sequence {
                        self.pool.counters.drop_frame();
                        older.state.store(FREE, Ordering::Release);
                    } else {
                        older.state.store(READY, Ordering::Release);
                    }
                }
            }
            if epoch != self.pool.epoch.load(Ordering::Acquire) {
                self.pool.counters.drop_frame();
                continue;
            }
            *destination = Some(ObservationFrame { dropped: self.dropped(), ..converted });
            written += 1;
        }
        DrainCount {
            written,
            remaining: self.pool.slots.iter().any(|slot| slot.state.load(Ordering::Acquire) == READY),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::audio::AudioEngine;
    use std::{alloc::{GlobalAlloc, Layout, System}, cell::Cell, path::Path};

    thread_local! {
        static WATCH: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
        static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }

    struct CallbackAllocator;
    unsafe impl GlobalAlloc for CallbackAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if WATCH.try_with(Cell::get).unwrap_or(false) {
                ALLOCATIONS.with(|n| n.set(n.get() + 1));
            }
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            if WATCH.try_with(Cell::get).unwrap_or(false) {
                DEALLOCATIONS.with(|n| n.set(n.get() + 1));
            }
            unsafe { System.dealloc(pointer, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: CallbackAllocator = CallbackAllocator;

    pub(crate) fn allocation_activity<R>(f: impl FnOnce() -> R) -> (R, usize, usize) {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) { WATCH.with(|watch| watch.set(false)); }
        }
        ALLOCATIONS.with(|n| n.set(0));
        DEALLOCATIONS.with(|n| n.set(0));
        WATCH.with(|watch| watch.set(true));
        let reset = Reset;
        let value = f();
        drop(reset);
        (value, ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get))
    }

    fn config() -> AudioConfig {
        AudioConfig { sample_rate: 48_000.0, max_frames: crate::audio::MAX_AUDIO_FRAMES, offline: true }
    }

    fn selection() -> ObservationSelection {
        ObservationSelection {
            meter_tracks: vec!["lead".into()], master_meter: true,
            analysis: Some(ObservationTarget::Track("lead".into())),
            meter_hz: 20, analysis_hz: 10,
        }
    }

    fn session() -> Session {
        let source = r#"song({tracks:[
            track("lead",note(60,16b).gate(1),synth("pad"),{
                sends:{pre:{gain:0,pre:true},post:{gain:0,pre:false}}
            })
        ],buses:[bus("pre",[]),bus("post",[])],tail:0})"#;
        let value = crate::lang::Evaluator::new().source(source).unwrap();
        crate::compile::lower(
            value.get("__result").unwrap().clone(), Path::new("observation-test.muz"), vec![],
        ).unwrap().session
    }

    fn drain_one(consumer: &mut ObservationConsumer) -> ObservationFrame {
        let mut output = [None];
        assert_eq!(consumer.drain(&mut output).written, 1);
        output[0].take().unwrap()
    }

    #[test]
    fn validates_exact_maps_and_hard_limits_without_capping_project_tracks() {
        let ids: Vec<_> = (0..1_001).map(|i| format!("track-{i}")).collect();
        assert!(PreparedAudibility::from_track_ids(&ids, &["track-1000".into()]).is_ok());
        assert!(PreparedAudibility::from_track_ids(&ids, &["track-0".into(), "track-0".into()]).is_err());
        assert!(PreparedAudibility::from_track_ids(&ids, &["missing".into()]).is_err());
        let mut selected = selection();
        selected.meter_tracks = vec!["track-1000".into()];
        selected.analysis = Some(ObservationTarget::Track("track-1000".into()));
        assert!(PreparedObservation::from_track_ids(&ids, config(), &selected).is_ok());
        selected.meter_tracks = ids.clone();
        assert!(PreparedObservation::from_track_ids(&ids, config(), &selected).is_err());
        selected.meter_tracks = vec!["track-0".into(), "track-0".into()];
        assert!(PreparedObservation::from_track_ids(&ids, config(), &selected).is_err());
        selected.meter_tracks.clear();
        selected.analysis = Some(ObservationTarget::Track("missing".into()));
        assert!(PreparedObservation::from_track_ids(&ids, config(), &selected).is_err());
        selected.analysis = Some(ObservationTarget::Master);
        selected.meter_hz = 21;
        assert!(PreparedObservation::from_track_ids(&ids, config(), &selected).is_err());
        selected.meter_hz = 20;
        selected.analysis_hz = 11;
        assert!(PreparedObservation::from_track_ids(&ids, config(), &selected).is_err());
    }

    #[test]
    fn independent_clocks_preserve_contiguous_mono_and_labels() {
        let (mut producer, mut consumer) = PreparedObservation::from_track_ids(
            &["lead".into()], config(), &selection(),
        ).unwrap();
        producer.set_identity(7, 9);
        let left: Vec<_> = (0..480).map(|i| i as f32 / 480.0).collect();
        let right: Vec<_> = left.iter().map(|v| -v * 0.5).collect();
        let mut meters = 0;
        let mut details = 0;
        for block in 0..100 {
            producer.capture(480, block * 480, true, |_| (&left, &right), (&left, &right), |_| 0.5);
            let mut output = [None];
            if consumer.drain(&mut output).written == 0 { continue; }
            let frame = output[0].take().unwrap();
            assert_eq!((frame.revision, frame.transport_generation), (7, 9));
            assert_eq!(frame.sample_position, block * 480);
            assert_eq!(frame.sample_rate, 48_000.0);
            if frame.master.is_some() {
                meters += 1;
                assert_eq!(frame.tracks[0].id, "lead");
                assert!(frame.tracks[0].peak > frame.master.unwrap()[0]);
            }
            if !frame.mono_samples.is_empty() {
                details += 1;
                assert_eq!(frame.target, Some(ObservationTarget::Track("lead".into())));
                assert_eq!(frame.mono_samples.len(), 480);
                for (index, sample) in frame.mono_samples.iter().enumerate() {
                    assert_eq!(*sample, (left[index] + right[index]) * 0.5);
                }
            } else {
                assert_eq!(frame.target, None);
            }
        }
        assert_eq!((meters, details), (20, 10));
        assert_eq!(consumer.dropped(), 0);
    }

    #[test]
    fn stalled_four_slot_pool_is_latest_wins_and_continuation_keeps_counters() {
        let ids = ["lead".into()];
        let (mut producer, mut consumer) = PreparedObservation::from_track_ids(
            &ids, config(), &selection(),
        ).unwrap();
        let samples = [0.25; 4_800];
        for block in 0..12 {
            producer.capture(4_800, block * 4_800, true, |_| (&samples, &samples), (&samples, &samples), |_| 1.0);
        }
        assert_eq!(producer.pool.slots.iter().filter(|s| s.state.load(Ordering::Relaxed) == READY).count(), 4);
        assert_eq!(consumer.dropped(), 8);
        let count = consumer.drain(&mut []);
        assert_eq!(count.written, 0);
        assert!(count.remaining);
        let last = drain_one(&mut consumer);
        assert_eq!(last.sequence, 12);
        assert_eq!(last.dropped, 11);
        assert_eq!(last.mono_samples, vec![0.25; MAX_DETAIL]);
        let (mut replacement, mut next) = PreparedObservation::from_track_ids_continuing(
            &ids, config(), &selection(), &consumer,
        ).unwrap();
        replacement.capture(4_800, 57_600, true, |_| (&samples, &samples), (&samples, &samples), |_| 1.0);
        let last = drain_one(&mut next);
        assert_eq!((last.sequence, last.dropped), (13, 11));
        let mut occupied = [Some(last)];
        replacement.capture(4_800, 62_400, true, |_| (&samples, &samples), (&samples, &samples), |_| 1.0);
        assert_eq!(next.drain(&mut occupied).written, 0);
        assert_eq!(occupied[0].as_ref().unwrap().sequence, 13);
        next.flush();
        assert_eq!(next.dropped(), 12);
        assert!(!next.drain(&mut []).remaining);
    }

    #[test]
    fn pause_clears_meters_and_identity_flushes_previous_generation() {
        let (mut producer, mut consumer) = PreparedObservation::from_track_ids(
            &["lead".into()], config(), &selection(),
        ).unwrap();
        let samples = [0.25; 480];
        producer.capture(480, 0, true, |_| (&samples, &samples), (&samples, &samples), |_| 1.0);
        producer.set_identity(2, 3);
        assert!(!consumer.drain(&mut []).remaining);
        assert_eq!(consumer.dropped(), 1);
        // Pause must clear immediately, not wait for the next meter clock.
        producer.capture(480, 480, false, |_| (&samples, &samples), (&samples, &samples), |_| 1.0);
        let frame = drain_one(&mut consumer);
        assert_eq!((frame.revision, frame.transport_generation), (2, 3));
        assert!(!frame.running);
        assert_eq!(frame.master, Some([0.0; 2]));
        assert_eq!((frame.tracks[0].peak, frame.tracks[0].rms), (0.0, 0.0));
        assert!(frame.mono_samples.is_empty());
        assert_eq!(frame.target, None);
    }

    #[test]
    fn all_disabled_capture_stops_and_meter_statistics_are_finite() {
        let mut selected = selection();
        selected.meter_hz = 0;
        selected.analysis_hz = 0;
        let (mut producer, mut consumer) = PreparedObservation::from_track_ids(
            &["lead".into()], config(), &selected,
        ).unwrap();
        let samples = [f32::NAN, f32::INFINITY, -1.0, 0.5];
        producer.capture(4, 0, true, |_| (&samples, &samples), (&samples, &samples), |_| 1.0);
        assert!(!consumer.drain(&mut []).remaining);
        let values = meter(&samples, &samples, 4, |_| 1.0);
        assert_eq!(values[0], 1.0);
        assert!(values[1].is_finite());
    }

    #[test]
    fn observation_never_changes_audio_latency_or_offline_tap() {
        let session = session();
        let mut observed = AudioEngine::new(&session, config()).unwrap();
        let mut baseline = AudioEngine::new(&session, config()).unwrap();
        let (producer, mut consumer) = PreparedObservation::new(&session, config(), &selection()).unwrap();
        assert!(observed.replace_observation(Some(producer)).is_none());
        observed.set_running(true);
        baseline.set_running(true);
        let mut a = [0.0; 2_048];
        let mut b = [0.0; 2_048];
        for tap in [None, Some("lead"), Some("pre"), None] {
            observed.set_tap(tap).unwrap();
            baseline.set_tap(tap).unwrap();
            assert_eq!(observed.latency_samples(), baseline.latency_samples());
            for _ in 0..8 {
                observed.render_interleaved(&mut a, 2).unwrap();
                baseline.render_interleaved(&mut b, 2).unwrap();
                assert_eq!(a, b);
                assert_eq!(observed.status().transport, baseline.status().transport);
                let mut output = [None];
                consumer.drain(&mut output);
            }
        }
    }

    #[test]
    fn prepared_mask_keeps_pre_mask_capture_and_all_send_routes_ramp() {
        let session = session();
        let mut engine = AudioEngine::new(&session, config()).unwrap();
        let (producer, mut consumer) = PreparedObservation::new(&session, config(), &selection()).unwrap();
        engine.replace_observation(Some(producer));
        engine.set_running(true);
        let mut pcm = [0.0; 2_048];
        for _ in 0..4 { engine.render_interleaved(&mut pcm, 2).unwrap(); }
        let mask = PreparedAudibility::new(&session, &[]).unwrap();
        engine.apply_prepared_audibility(&mask, true).unwrap();
        engine.render_interleaved(&mut pcm, 2).unwrap();
        assert!(pcm[..100].iter().any(|v| v.abs() > 0.0001));
        assert!(pcm[500..].iter().all(|v| *v == 0.0));
        for _ in 0..10 { engine.render_interleaved(&mut pcm, 2).unwrap(); }
        let frame = drain_one(&mut consumer);
        assert_eq!(frame.master, Some([0.0; 2]));
        assert!(frame.tracks[0].peak > 0.0001);
        assert_eq!(engine.revision(), 0);
        assert!(engine.status().transport.running);
    }

    #[test]
    fn capture_replacement_identity_and_pool_pressure_do_not_allocate_or_deallocate() {
        let session = session();
        let mut engine = AudioEngine::new(&session, config()).unwrap();
        let (producer, consumer) = PreparedObservation::new(&session, config(), &selection()).unwrap();
        engine.replace_observation(Some(producer));
        let (replacement, _next) = PreparedObservation::new_continuing(
            &session, config(), &selection(), &consumer,
        ).unwrap();
        let mask = PreparedAudibility::new(&session, &[]).unwrap();
        let invalid = PreparedAudibility::from_track_ids(&[], &[]).unwrap();
        let mut pcm = [0.0; 512];
        engine.set_running(true);
        engine.render_interleaved(&mut pcm, 2).unwrap();
        let ((retired, final_capture), allocations, deallocations) = allocation_activity(|| {
            engine.apply_prepared_audibility(&mask, true).unwrap();
            assert!(engine.apply_prepared_audibility(&invalid, true).is_err());
            for _ in 0..1_000 { engine.render_interleaved(&mut pcm, 2).unwrap(); }
            engine.set_observation_identity(1, 2);
            let retired = engine.replace_observation(Some(replacement));
            for _ in 0..1_000 { engine.render_interleaved(&mut pcm, 2).unwrap(); }
            let final_capture = engine.replace_observation(None);
            (retired, final_capture)
        });
        assert_eq!((allocations, deallocations), (0, 0));
        assert!(consumer.dropped() > 4);
        assert!(retired.is_some() && final_capture.is_some());
        drop((retired, final_capture));
    }

    #[test]
    fn reordered_mapping_uses_new_indices_and_removed_detail_has_no_old_label() {
        let first = ["lead".into(), "other".into()];
        let (_, consumer) = PreparedObservation::from_track_ids(&first, config(), &selection()).unwrap();
        let mut selected = selection();
        selected.analysis = None;
        let reordered = ["other".into(), "lead".into()];
        let (mut producer, mut consumer) = PreparedObservation::from_track_ids_continuing(
            &reordered, config(), &selected, &consumer,
        ).unwrap();
        producer.set_identity(1, 0);
        let quiet = [0.125; 480];
        let loud = [0.75; 480];
        producer.capture(480, 0, true, |index| {
            if index == 1 { (&loud, &loud) } else { (&quiet, &quiet) }
        }, (&quiet, &quiet), |_| 1.0);
        let frame = drain_one(&mut consumer);
        assert_eq!(frame.tracks[0].id, "lead");
        assert_eq!(frame.tracks[0].peak, 0.75);
        assert_eq!(frame.target, None);
        assert!(frame.mono_samples.is_empty());
    }
}

#[cfg(test)]
mod exhaustion_tests {
    use super::*;

    #[test]
    fn observation_counters_never_wrap_or_repeat_sequence() {
        let counters = Counters::default();
        counters.sequence.store(u64::MAX - 1, Ordering::Relaxed);
        assert_eq!(counters.next_sequence(), Some(u64::MAX));
        assert_eq!(counters.next_sequence(), None);
        assert_eq!(counters.sequence.load(Ordering::Relaxed), u64::MAX);
        counters.dropped.store(u64::MAX - 1, Ordering::Relaxed);
        counters.drop_frame();
        counters.drop_frame();
        assert_eq!(counters.dropped.load(Ordering::Relaxed), u64::MAX);
    }
}
