use super::automation::DelayLine;
use crate::compile::Automation;
use thiserror::Error;

use crate::model;

use super::{
    AudioConfig, DeliveredEvents, DeviceDebugState, DeviceError, DeviceProcessor, DeviceSlot,
    MAX_AUDIO_FRAMES, PreparedTransaction, PreparedValueOperation, PreparedValueTransaction,
    ProcessContext, RuntimeTransport, ScheduleError, ScheduledEvents, TransactionApplyError,
    TransportSnapshot, ValueTransactionApplyError,
    transport::{ArrangementScheduler, PatternScheduler, TempoTimeline},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FadeDirection {
    Out,
    In,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EngineFade {
    pub tracks: u32,
    pub buses: u16,
    pub master: bool,
    pub direction: FadeDirection,
}

impl EngineFade {
    fn track(self, index: usize) -> bool {
        self.tracks & (1_u32 << index) != 0
    }

    fn bus(self, index: usize) -> bool {
        self.buses & (1_u16 << index) != 0
    }
}

#[derive(Debug)]
pub(crate) struct DeviceRetention {
    pub current: DeviceSlot,
    pub candidate: DeviceSlot,
    pub expected_id: model::Id,
    pub expected_kind: model::DeviceKind,
    pub parameters: Vec<(String, f32)>,
}

#[derive(Clone, Debug)]
pub(crate) struct TrackRetention {
    pub current: usize,
    pub candidate: usize,
    pub expected_id: model::Id,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum StructuralTransactionApplyError {
    #[error("structural transaction was already applied")]
    AlreadyApplied,
    #[error("prepared structural transaction no longer matches the runtime graph")]
    RuntimeMismatch,
    #[error(transparent)]
    Device(#[from] DeviceError),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineStatus {
    pub transport: TransportSnapshot,
    pub device_count: usize,
    pub current_tick: f64,
    pub end_tick: u64,
    pub end_project_frame: f64,
    pub ended: bool,
    pub last_peak: f32,
    pub last_rms: f32,
    pub delivered_events: DeliveredEvents,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("invalid audio engine configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("invalid static audio graph: {0}")]
    InvalidGraph(&'static str),
    /// A failure rendered as a source diagnostic: location, snippet and help
    /// lines travel in the message because graph validation has no spans.
    #[error("{0}")]
    Source(String),
    #[error("invalid static audio graph: {0}")]
    Preflight(String),
    #[error("audio output must have at least two channels; got {channels}")]
    UnsupportedChannelCount { channels: usize },
    #[error("audio output length {samples} is not divisible by its {channels} channels")]
    OutputNotFrameAligned { samples: usize, channels: usize },
    #[error("audio block of {frames} frames exceeds configured maximum of {max_frames}")]
    FrameCapacityExceeded { frames: usize, max_frames: usize },
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
}

pub struct AudioEngine {
    revision: u64,
    config: AudioConfig,
    transport: RuntimeTransport,
    tracks: Vec<TrackRuntime>,
    buses: Vec<BusRuntime>,
    bus_order: Vec<usize>,
    device_count: usize,
    last_peak: f32,
    last_rms: f32,
    latency: usize,
    track_order: Vec<usize>,
    track_latency: Vec<usize>,
    bus_latency: Vec<usize>,
    tap: Option<(bool, usize)>,
    transport_generation: u64,
}

impl AudioEngine {
    pub fn new(session: &model::Session, config: AudioConfig) -> Result<Self, EngineError> {
        validate_config(config)?;
        validate_bus_shape(session)?;
        validate_graph_capacity(session)?;
        session
            .validate_sample_coverage()
            .map_err(EngineError::Preflight)?;

        let mut buses = Vec::with_capacity(session.buses.len() + 1);
        buses.push(BusRuntime::new(&session.master, session, config)?);
        for bus in &session.buses {
            buses.push(BusRuntime::new(bus, session, config)?);
        }

        let mut tracks = Vec::with_capacity(session.tracks.len());
        for track in &session.tracks {
            tracks.push(TrackRuntime::new(track, session, config)?);
        }

        let bus_order = topological_bus_order(&buses)?;
        let device_count = buses.iter().map(|bus| bus.inserts.len()).sum::<usize>()
            + tracks
                .iter()
                .map(|track| 1 + track.inserts.len())
                .sum::<usize>();

        let mut engine = Self {
            revision: 0,
            config,
            transport: RuntimeTransport::from_session(f64::from(config.sample_rate), session)
                .map_err(EngineError::InvalidGraph)?,
            tracks,
            buses,
            bus_order,
            device_count,
            last_peak: 0.0,
            last_rms: 0.0,
            latency: 0,
            track_order: Vec::new(),
            track_latency: Vec::new(),
            bus_latency: Vec::new(),
            tap: None,
            transport_generation: 0,
        };
        engine.prepare_sidechains(session)?;
        engine.prepare_compensation();
        engine.validate_automation(session)?;
        Ok(engine)
    }

    pub fn set_running(&mut self, running: bool) {
        self.transport.set_running(running);
    }

    pub fn restart(&mut self) {
        for track in &mut self.tracks {
            track.reset_delivered();
        }
        self.transport.restart();
    }

    pub fn seek_ticks(&mut self, tick: u64) {
        for track in &mut self.tracks {
            track.reset_delivered();
        }
        self.transport.seek_ticks(tick);
    }

    pub fn set_loop(&mut self, range: Option<(u64, u64)>) {
        self.transport.set_loop(range);
    }
    pub fn panic(&mut self) {
        self.transport.set_running(false);
        self.reset_audio();
    }
    fn reset_audio(&mut self) {
        for t in &mut self.tracks {
            t.instrument.processor.reset();
            for d in &mut t.inserts {
                d.processor.reset();
                d.input_delay.reset();
                d.detector_delay.reset();
            }
            for r in std::iter::once(&mut t.output).chain(&mut t.sends) {
                r.delay.reset();
            }
        }
        for b in &mut self.buses {
            for d in &mut b.inserts {
                d.processor.reset();
                d.input_delay.reset();
                d.detector_delay.reset();
            }
            for r in &mut b.routes {
                r.delay.reset();
            }
        }
    }
    pub fn update_transport(&mut self, transport: &model::Transport) {
        self.transport.update_source(transport);
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }
    pub fn track_audio(&self, index: usize) -> (&str, &[f32], &[f32], usize) {
        let t = &self.tracks[index];
        (
            t.id.as_str(),
            &t.scratch.left[..],
            &t.scratch.right[..],
            self.track_latency[index],
        )
    }
    pub fn config(&self) -> AudioConfig {
        self.config
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn accept_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    pub(crate) fn swap_structural(
        &mut self,
        candidate: &mut Self,
        device_retentions: &[DeviceRetention],
        track_retentions: &[TrackRetention],
        _transport: &model::Transport,
    ) -> Result<(), StructuralTransactionApplyError> {
        if self.config != candidate.config {
            return Err(StructuralTransactionApplyError::RuntimeMismatch);
        }
        for retention in device_retentions {
            let Some(current) = self.device(retention.current) else {
                return Err(StructuralTransactionApplyError::RuntimeMismatch);
            };
            let Some(staged) = candidate.device(retention.candidate) else {
                return Err(StructuralTransactionApplyError::RuntimeMismatch);
            };
            if current.id != retention.expected_id
                || staged.id != retention.expected_id
                || current.processor.kind() != retention.expected_kind
                || staged.processor.kind() != retention.expected_kind
            {
                return Err(StructuralTransactionApplyError::RuntimeMismatch);
            }
        }
        for retention in track_retentions {
            let Some(current) = self.tracks.get(retention.current) else {
                return Err(StructuralTransactionApplyError::RuntimeMismatch);
            };
            let Some(staged) = candidate.tracks.get(retention.candidate) else {
                return Err(StructuralTransactionApplyError::RuntimeMismatch);
            };
            if current.id != retention.expected_id || staged.id != retention.expected_id {
                return Err(StructuralTransactionApplyError::RuntimeMismatch);
            }
        }

        for retention in device_retentions {
            let processor = &mut self
                .device_mut(retention.current)
                .expect("structural transaction device was preflighted")
                .processor;
            for (name, value) in &retention.parameters {
                processor.set_parameter(name, *value)?;
            }
        }
        for retention in device_retentions {
            std::mem::swap(
                &mut self
                    .device_mut(retention.current)
                    .expect("structural transaction device was preflighted")
                    .processor,
                &mut candidate
                    .device_mut(retention.candidate)
                    .expect("structural transaction candidate device was preflighted")
                    .processor,
            );
        }
        for retention in track_retentions {
            let current = self
                .tracks
                .get_mut(retention.current)
                .expect("structural transaction track was preflighted");
            let staged = candidate
                .tracks
                .get_mut(retention.candidate)
                .expect("structural transaction candidate track was preflighted");
            current.swap_scheduler_with(staged);
        }
        candidate.transport.adopt_position_from(&self.transport);
        candidate.transport_generation = self.transport_generation;
        std::mem::swap(self, candidate);
        Ok(())
    }
    pub fn apply_transaction(
        &mut self,
        transaction: &mut PreparedTransaction,
    ) -> Result<(), TransactionApplyError> {
        transaction.apply(self)
    }

    pub fn apply_value_transaction(
        &mut self,
        transaction: &mut PreparedValueTransaction,
    ) -> Result<(), ValueTransactionApplyError> {
        if self.revision.checked_add(1) != Some(transaction.revision())
            || transaction.config.is_some_and(|c| c != self.config)
        {
            return Err(ValueTransactionApplyError::RuntimeMismatch);
        }
        {
            let operations = transaction.operations_mut()?;
            for operation in operations.iter() {
                match operation {
                    PreparedValueOperation::SetParameter {
                        slot,
                        expected_device_id,
                        ..
                    } => {
                        let Some(device) = self.device(*slot) else {
                            return Err(ValueTransactionApplyError::RuntimeMismatch);
                        };
                        if device.id != *expected_device_id {
                            return Err(ValueTransactionApplyError::RuntimeMismatch);
                        }
                    }
                    PreparedValueOperation::ReplacePattern {
                        track,
                        expected_pattern_id,
                        ..
                    } => {
                        let Some(runtime) = self.tracks.get(*track) else {
                            return Err(ValueTransactionApplyError::RuntimeMismatch);
                        };
                        if runtime.pattern_id() != Some(expected_pattern_id) {
                            return Err(ValueTransactionApplyError::RuntimeMismatch);
                        }
                    }
                    PreparedValueOperation::UpdateTransport { .. } => {}
                }
            }

            for operation in operations.iter_mut() {
                match operation {
                    PreparedValueOperation::SetParameter {
                        slot, name, value, ..
                    } => {
                        self.device_mut(*slot)
                            .expect("value transaction device was preflighted")
                            .processor
                            .set_parameter(name, *value)?;
                    }
                    PreparedValueOperation::ReplacePattern { track, pattern, .. } => {
                        self.tracks
                            .get_mut(*track)
                            .expect("value transaction track was preflighted")
                            .replace_pattern(pattern);
                    }
                    PreparedValueOperation::UpdateTransport { .. } => {}
                }
            }
        }
        if let Some(transport) = transaction.transport.as_mut() {
            transport.adopt_position_from(&self.transport);
            std::mem::swap(&mut self.transport, transport);
        }
        self.revision = transaction.revision();
        transaction.mark_applied();
        Ok(())
    }

    pub fn status(&self) -> EngineStatus {
        let transport = self.transport.snapshot();
        let current_tick = self.transport.current_tick();
        let end_tick = self.transport.end_tick();
        let mut delivered_events = DeliveredEvents::default();
        for track in &self.tracks {
            delivered_events.merge(track.delivered_events());
        }
        EngineStatus {
            transport,
            device_count: self.device_count,
            current_tick,
            end_tick,
            end_project_frame: self.transport.end_project_frame(),
            ended: transport.ended,
            last_peak: self.last_peak,
            last_rms: self.last_rms,
            delivered_events,
        }
    }

    /// This method allocates and must not be called from the audio callback.
    pub fn device_debug_states(&self) -> Vec<(model::Id, DeviceDebugState)> {
        let mut states = Vec::with_capacity(self.device_count);
        for bus in &self.buses {
            for device in &bus.inserts {
                states.push((device.id.clone(), device.processor.debug_state()));
            }
        }
        for track in &self.tracks {
            states.push((
                track.instrument.id.clone(),
                track.instrument.processor.debug_state(),
            ));
            for device in &track.inserts {
                states.push((device.id.clone(), device.processor.debug_state()));
            }
        }
        states
    }
    /// Copies processor identity and process counters in source graph order without allocating.
    ///
    /// The caller must provide room for every runtime device. The order is master inserts,
    /// non-master bus inserts in declaration order, then each track instrument and its inserts.
    pub(crate) fn copy_device_debug_states(&self, states: &mut [DeviceDebugState]) -> usize {
        assert!(states.len() >= self.device_count);
        let mut count = 0;
        for bus in &self.buses {
            for device in &bus.inserts {
                states[count] = device.processor.debug_state();
                count += 1;
            }
        }
        for track in &self.tracks {
            states[count] = track.instrument.processor.debug_state();
            count += 1;
            for device in &track.inserts {
                states[count] = device.processor.debug_state();
                count += 1;
            }
        }
        debug_assert_eq!(count, self.device_count);
        count
    }

    pub fn render_interleaved(
        &mut self,
        output: &mut [f32],
        channels: usize,
    ) -> Result<(), EngineError> {
        self.render_interleaved_with_fade(output, channels, None)
    }

    pub(crate) fn render_interleaved_with_fade(
        &mut self,
        output: &mut [f32],
        channels: usize,
        fade: Option<EngineFade>,
    ) -> Result<(), EngineError> {
        if channels < 2 {
            return Err(EngineError::UnsupportedChannelCount { channels });
        }
        if output.len() % channels != 0 {
            return Err(EngineError::OutputNotFrameAligned {
                samples: output.len(),
                channels,
            });
        }
        let frames = output.len() / channels;
        if frames > self.config.max_frames {
            return Err(EngineError::FrameCapacityExceeded {
                frames,
                max_frames: self.config.max_frames,
            });
        }

        for track in &mut self.tracks {
            track.scratch.clear(frames);
        }
        for bus in &mut self.buses {
            bus.scratch.clear(frames);
        }

        let mut offset = 0;
        while offset < frames {
            let block = self.transport.block(frames - offset);
            debug_assert!(block.frames != 0);
            self.render_slice(block, offset, frames, fade)?;
            offset += block.frames;
            self.transport.advance(block.frames);
        }

        let master = match self.tap {
            Some((true, i)) => &self.tracks[i].scratch,
            Some((false, i)) => &self.buses[i].scratch,
            None => &self.buses[0].scratch,
        };
        let mut peak = 0.0_f32;
        let mut square_sum = 0.0_f64;
        for (frame, samples) in output.chunks_exact_mut(channels).enumerate() {
            let master_gain = fade
                .filter(|mask| mask.master)
                .map_or(1.0, |mask| fade_gain(mask.direction, frame, frames));
            let left = master.left[frame] * master_gain;
            let right = master.right[frame] * master_gain;
            samples[0] = left;
            samples[1] = right;
            samples[2..].fill(0.0);
            let finite_left = if left.is_finite() { left } else { 0.0 };
            let finite_right = if right.is_finite() { right } else { 0.0 };
            peak = peak.max(finite_left.abs()).max(finite_right.abs());
            square_sum += f64::from(finite_left) * f64::from(finite_left)
                + f64::from(finite_right) * f64::from(finite_right);
        }
        self.last_peak = peak;
        self.last_rms = if frames == 0 {
            0.0
        } else {
            (square_sum / (frames * 2) as f64).sqrt() as f32
        };
        Ok(())
    }

    fn render_slice(
        &mut self,
        block: super::TransportBlock,
        offset: usize,
        hardware_frames: usize,
        fade: Option<EngineFade>,
    ) -> Result<(), EngineError> {
        if block.generation() != self.transport_generation {
            self.reset_audio();
            self.transport_generation = block.generation();
        }
        let end = offset + block.frames;
        let context = ProcessContext {
            frames: block.frames,
            block_start_sample: block.start_sample(),
            transport: block.snapshot,
        };
        let timeline = self.transport.timeline();
        for track in &mut self.tracks {
            track.events = track.schedule(timeline, block)?;
        }

        for order_index in 0..self.track_order.len() {
            let track_index = self.track_order[order_index];
            {
                let track = &mut self.tracks[track_index];
                track.instrument.process(
                    context,
                    &track.events,
                    &mut track.scratch.left[offset..end],
                    &mut track.scratch.right[offset..end],
                )?;
            }
            for i in 0..self.tracks[track_index].inserts.len() {
                let sidechain = self.tracks[track_index].inserts[i].sidechain;
                if let Some(source) = sidechain {
                    let (track, detector) = if source < track_index {
                        let (a, b) = self.tracks.split_at_mut(track_index);
                        (&mut b[0], &a[source].scratch)
                    } else {
                        let (a, b) = self.tracks.split_at_mut(source);
                        (&mut a[track_index], &b[0].scratch)
                    };
                    let insert = &mut track.inserts[i];
                    insert.feed_detector(
                        context,
                        &detector.left[offset..end],
                        &detector.right[offset..end],
                    );
                    insert.process(
                        context,
                        &[],
                        &mut track.scratch.left[offset..end],
                        &mut track.scratch.right[offset..end],
                    )?;
                } else {
                    let track = &mut self.tracks[track_index];
                    track.inserts[i].process(
                        context,
                        &[],
                        &mut track.scratch.left[offset..end],
                        &mut track.scratch.right[offset..end],
                    )?;
                }
            }
            let track = &mut self.tracks[track_index];
            let source_fade = fade.filter(|mask| mask.track(track_index));
            mix_into_bus(
                &track.scratch,
                &mut self.buses[track.output.target].scratch,
                offset,
                end,
                hardware_frames,
                &mut track.output,
                context,
                source_fade,
            );
            for route in &mut track.sends {
                mix_into_bus(
                    &track.scratch,
                    &mut self.buses[route.target].scratch,
                    offset,
                    end,
                    hardware_frames,
                    route,
                    context,
                    source_fade,
                );
            }
        }

        for order_index in 0..self.bus_order.len() {
            let source_index = self.bus_order[order_index];
            {
                let source = &mut self.buses[source_index];
                for insert in &mut source.inserts {
                    if let Some(track) = insert.sidechain {
                        let detector = &self.tracks[track].scratch;
                        insert.feed_detector(
                            context,
                            &detector.left[offset..end],
                            &detector.right[offset..end],
                        );
                    }
                    insert.process(
                        context,
                        &[],
                        &mut source.scratch.left[offset..end],
                        &mut source.scratch.right[offset..end],
                    )?;
                }
            }
            for route_index in 0..self.buses[source_index].routes.len() {
                let source_fade = fade.filter(|mask| mask.bus(source_index));
                mix_bus_route(
                    &mut self.buses,
                    source_index,
                    route_index,
                    offset,
                    end,
                    hardware_frames,
                    context,
                    source_fade,
                );
            }
        }
        Ok(())
    }
}

enum TrackSchedule {
    Pattern {
        source: model::Pattern,
        scheduler: PatternScheduler,
    },
    Arrangement {
        source: crate::midi::ImportedMidi,
        scheduler: ArrangementScheduler,
    },
}

struct TrackRuntime {
    id: model::Id,
    schedule: TrackSchedule,
    events: ScheduledEvents,
    instrument: DeviceRuntime,
    inserts: Vec<DeviceRuntime>,
    output: RoutePlan,
    sends: Vec<RoutePlan>,
    scratch: StereoScratch,
}

impl TrackRuntime {
    fn new(
        track: &model::Track,
        session: &model::Session,
        config: AudioConfig,
    ) -> Result<Self, EngineError> {
        let schedule = match &track.source {
            model::TrackSource::Pattern(pattern) => TrackSchedule::Pattern {
                source: pattern.clone(),
                scheduler: PatternScheduler::new(),
            },
            model::TrackSource::Midi(midi) => {
                // A shared multichannel asset feeds independent instruments.
                // Keep its tempo map/end intact, selecting notes AND controllers
                // before entering the callback so one track cannot mute another.
                let mut source = midi.imported.clone();
                if !midi.all_channels {
                    source.notes.retain(|note| note.channel == midi.channel);
                    source
                        .controllers
                        .retain(|event| event.channel == midi.channel);
                    source
                        .messages
                        .retain(|event| event.bytes[0] & 15 == midi.channel);
                }
                TrackSchedule::Arrangement {
                    scheduler: ArrangementScheduler::compile(
                        &source,
                        &TempoTimeline::compile(
                            f64::from(config.sample_rate),
                            &session.transport,
                            &session.tracks,
                        )
                        .map_err(EngineError::InvalidGraph)?,
                    ),
                    source,
                }
            }
        };
        let instrument = DeviceRuntime::new(&track.instrument, config, &session.extras)?;
        if let model::TrackSource::Midi(m) = &track.source {
            if m.imported.notes.iter().any(|n| {
                n.performance.is_some_and(|p| {
                    p.expression.points[..p.expression.len as usize]
                        .iter()
                        .any(|point| !instrument.processor.accepts_note_expression(point.kind))
                })
            }) {
                return Err(EngineError::InvalidGraph(
                    "instrument does not support a requested note-expression kind",
                ));
            }
        }
        let mut inserts = Vec::with_capacity(track.inserts.len());
        for device in &track.inserts {
            inserts.push(DeviceRuntime::new(device, config, &session.extras)?);
        }
        let output = RoutePlan::new(&track.output, session)?;
        let mut sends = Vec::with_capacity(track.sends.len());
        for route in &track.sends {
            sends.push(RoutePlan::new(route, session)?);
        }
        Ok(Self {
            id: track.id.clone(),
            schedule,
            events: ScheduledEvents::new(),
            instrument,
            inserts,
            output,
            sends,
            scratch: StereoScratch::new(),
        })
    }

    fn pattern_id(&self) -> Option<&model::Id> {
        match &self.schedule {
            TrackSchedule::Pattern { source, .. } => Some(&source.id),
            TrackSchedule::Arrangement { .. } => None,
        }
    }

    fn replace_pattern(&mut self, replacement: &mut model::Pattern) {
        let TrackSchedule::Pattern { source, .. } = &mut self.schedule else {
            unreachable!("pattern transaction was preflighted");
        };
        std::mem::swap(source, replacement);
    }

    fn schedule(
        &mut self,
        timeline: &TempoTimeline,
        block: super::TransportBlock,
    ) -> Result<ScheduledEvents, ScheduleError> {
        match &mut self.schedule {
            TrackSchedule::Pattern { source, scheduler } => scheduler.schedule(source, block),
            TrackSchedule::Arrangement { source, scheduler } => {
                scheduler.schedule(source, timeline, block)
            }
        }
    }

    fn delivered_events(&self) -> DeliveredEvents {
        match &self.schedule {
            TrackSchedule::Pattern { .. } => DeliveredEvents::default(),
            TrackSchedule::Arrangement { scheduler, .. } => scheduler.delivered(),
        }
    }

    fn reset_delivered(&mut self) {
        if let TrackSchedule::Arrangement { scheduler, .. } = &mut self.schedule {
            scheduler.reset_delivered();
        }
    }

    fn swap_scheduler_with(&mut self, other: &mut Self) {
        match (&mut self.schedule, &mut other.schedule) {
            (
                TrackSchedule::Pattern {
                    scheduler: current, ..
                },
                TrackSchedule::Pattern {
                    scheduler: staged, ..
                },
            ) => std::mem::swap(current, staged),
            (
                TrackSchedule::Arrangement {
                    scheduler: current, ..
                },
                TrackSchedule::Arrangement {
                    scheduler: staged, ..
                },
            ) => staged.adopt(current),
            _ => unreachable!("track retention requires matching source kinds"),
        }
    }
}

struct BusRuntime {
    id: model::Id,
    inserts: Vec<DeviceRuntime>,
    routes: Vec<RoutePlan>,
    scratch: StereoScratch,
}

impl BusRuntime {
    fn new(
        bus: &model::Bus,
        session: &model::Session,
        config: AudioConfig,
    ) -> Result<Self, EngineError> {
        let mut inserts = Vec::with_capacity(bus.inserts.len());
        for device in &bus.inserts {
            inserts.push(DeviceRuntime::new(device, config, &session.extras)?);
        }
        let mut routes = Vec::with_capacity(bus.sends.len() + usize::from(bus.output.is_some()));
        if let Some(route) = &bus.output {
            routes.push(RoutePlan::new(route, session)?);
        }
        for route in &bus.sends {
            routes.push(RoutePlan::new(route, session)?);
        }
        Ok(Self {
            id: bus.id.clone(),
            inserts,
            routes,
            scratch: StereoScratch::new(),
        })
    }
}

struct DeviceRuntime {
    id: model::Id,
    processor: Box<dyn DeviceProcessor>,
    automation: Vec<(String, Automation, f32)>,
    sidechain: Option<usize>,
    input_delay: DelayLine,
    detector_delay: DelayLine,
    detector: StereoScratch,
}

/// Every automation target the graph could accept: route ids and the native
/// device parameters, so a rejected target can list what was available.
fn automation_targets(session: &model::Session) -> std::collections::BTreeSet<String> {
    use std::collections::BTreeSet;
    fn device(out: &mut BTreeSet<String>, d: &model::Device) {
        let specs = crate::source::parameter_specs(d.kind);
        // Spec-less devices expose whatever the plugin or rack declares; a rack
        // still has a statically known parameter set.
        if specs.is_empty()
            && let Some(rack) = &d.rack
        {
            out.insert(format!("{}.mix", d.id));
            out.insert(format!("{}.gain_db", d.id));
            for name in rack.expose.keys() {
                out.insert(format!("{}.{name}", d.id));
            }
        }
        for spec in specs {
            out.insert(format!("{}.{}", d.id, spec.name));
        }
    }
    let mut out = BTreeSet::new();
    for t in &session.tracks {
        out.insert(t.output.id.as_str().to_owned());
        for r in &t.sends {
            out.insert(r.id.as_str().to_owned());
        }
        device(&mut out, &t.instrument);
        for d in &t.inserts {
            device(&mut out, d);
        }
    }
    for b in std::iter::once(&session.master).chain(session.buses.iter()) {
        for r in b.output.iter().chain(&b.sends) {
            out.insert(r.id.as_str().to_owned());
        }
        for d in &b.inserts {
            device(&mut out, d);
        }
    }
    out
}
/// List the available targets that share the rejected target's first segment,
/// falling back to the whole vocabulary, with an explicit cap.
fn available_targets_hint(target: &str, available: &std::collections::BTreeSet<String>) -> String {
    let head = target.split('.').next().unwrap_or(target);
    let same: Vec<&str> = available
        .iter()
        .map(String::as_str)
        .filter(|t| t.split('.').next() == Some(head))
        .collect();
    let (mut listed, scope) = if same.is_empty() {
        (
            available.iter().map(String::as_str).collect::<Vec<_>>(),
            "available targets".to_owned(),
        )
    } else {
        (same, format!("targets under '{head}'"))
    };
    let total = listed.len();
    listed.truncate(12);
    let mut hint = format!("{scope}: {}", listed.join(", "));
    if total > listed.len() {
        hint.push_str(&format!(", ... ({total} total)"));
    }
    hint
}

impl DeviceRuntime {
    fn new(
        device: &model::Device,
        config: AudioConfig,
        extras: &crate::compile::Extras,
    ) -> Result<Self, DeviceError> {
        Ok(Self {
            sidechain: None,
            input_delay: DelayLine::default(),
            detector_delay: DelayLine::default(),
            detector: StereoScratch::new(),
            id: device.id.clone(),
            processor: super::create_processor(device, config)?,
            automation: extras
                .automation
                .iter()
                .filter_map(|a| {
                    a.target
                        .strip_prefix(&format!("{}.", device.id))
                        .map(|name| (name.to_owned(), a.clone(), f32::NAN))
                })
                .collect(),
        })
    }
}

impl AudioEngine {
    fn device(&self, slot: DeviceSlot) -> Option<&DeviceRuntime> {
        match slot {
            DeviceSlot::BusInsert { bus, insert } => self.buses.get(bus)?.inserts.get(insert),
            DeviceSlot::TrackInstrument { track } => Some(&self.tracks.get(track)?.instrument),
            DeviceSlot::TrackInsert { track, insert } => {
                self.tracks.get(track)?.inserts.get(insert)
            }
        }
    }

    fn device_mut(&mut self, slot: DeviceSlot) -> Option<&mut DeviceRuntime> {
        match slot {
            DeviceSlot::BusInsert { bus, insert } => {
                self.buses.get_mut(bus)?.inserts.get_mut(insert)
            }
            DeviceSlot::TrackInstrument { track } => {
                Some(&mut self.tracks.get_mut(track)?.instrument)
            }
            DeviceSlot::TrackInsert { track, insert } => {
                self.tracks.get_mut(track)?.inserts.get_mut(insert)
            }
        }
    }
}

struct RoutePlan {
    fader: Option<(f32, Option<Automation>)>,
    target: usize,
    gain: f32,
    delay: DelayLine,
    automation: Option<Automation>,
    muted: bool,
}

impl RoutePlan {
    fn new(route: &model::Route, session: &model::Session) -> Result<Self, EngineError> {
        let target = bus_index(session, &route.to)
            .ok_or(EngineError::InvalidGraph("route targets an unknown bus"))?;
        if !route.gain_db.is_finite() {
            return Err(EngineError::InvalidGraph("route gain must be finite"));
        }
        Ok(Self {
            fader: if route.pre {
                None
            } else {
                let output = session
                    .tracks
                    .iter()
                    .find(|t| t.sends.iter().any(|r| r.id == route.id))
                    .map(|t| &t.output)
                    .or_else(|| {
                        session
                            .buses
                            .iter()
                            .find(|b| b.sends.iter().any(|r| r.id == route.id))
                            .and_then(|b| b.output.as_ref())
                    });
                output.map(|o| {
                    (
                        10.0_f32.powf(o.gain_db / 20.),
                        session
                            .extras
                            .automation
                            .iter()
                            .find(|a| a.target == o.id.as_str())
                            .cloned(),
                    )
                })
            },
            target,
            muted: false,
            gain: 10.0_f32.powf(route.gain_db / 20.0),
            delay: DelayLine::default(),
            automation: session
                .extras
                .automation
                .iter()
                .find(|a| a.target == route.id.as_str())
                .cloned(),
        })
    }
}

struct StereoScratch {
    left: Box<[f32; MAX_AUDIO_FRAMES]>,
    right: Box<[f32; MAX_AUDIO_FRAMES]>,
}

impl StereoScratch {
    fn new() -> Self {
        Self {
            left: Box::new([0.0; MAX_AUDIO_FRAMES]),
            right: Box::new([0.0; MAX_AUDIO_FRAMES]),
        }
    }

    fn clear(&mut self, frames: usize) {
        self.left[..frames].fill(0.0);
        self.right[..frames].fill(0.0);
    }
}

fn validate_config(config: AudioConfig) -> Result<(), EngineError> {
    if !config.sample_rate.is_finite() || config.sample_rate <= 0.0 {
        return Err(EngineError::InvalidConfig(
            "sample_rate must be finite and positive",
        ));
    }
    if config.max_frames == 0 {
        return Err(EngineError::InvalidConfig("max_frames must be positive"));
    }
    if config.max_frames > MAX_AUDIO_FRAMES {
        return Err(EngineError::InvalidConfig(
            "max_frames exceeds the fixed engine capacity",
        ));
    }
    Ok(())
}

fn validate_graph_capacity(session: &model::Session) -> Result<(), EngineError> {
    session
        .validate_graph_budget()
        .map_err(EngineError::Preflight)
}

fn validate_bus_shape(session: &model::Session) -> Result<(), EngineError> {
    if session.master.output.is_some() || !session.master.sends.is_empty() {
        return Err(EngineError::InvalidGraph(
            "the master bus cannot have outputs or sends",
        ));
    }
    if session.buses.iter().any(|bus| bus.output.is_none()) {
        return Err(EngineError::InvalidGraph(
            "every non-master bus must have an output",
        ));
    }
    Ok(())
}

fn bus_index(session: &model::Session, id: &model::Id) -> Option<usize> {
    if session.master.id == *id {
        return Some(0);
    }
    session
        .buses
        .iter()
        .position(|bus| bus.id == *id)
        .map(|index| index + 1)
}

fn topological_bus_order(buses: &[BusRuntime]) -> Result<Vec<usize>, EngineError> {
    let mut indegree = vec![0usize; buses.len()];
    for bus in buses {
        for route in &bus.routes {
            indegree[route.target] = indegree[route.target]
                .checked_add(1)
                .ok_or(EngineError::InvalidGraph("bus route count overflow"))?;
        }
    }

    let mut emitted = vec![false; buses.len()];
    let mut order = Vec::with_capacity(buses.len());
    while order.len() < buses.len() {
        let Some(source) = (0..buses.len()).find(|&index| !emitted[index] && indegree[index] == 0)
        else {
            return Err(EngineError::InvalidGraph(
                "bus route graph contains a cycle",
            ));
        };
        emitted[source] = true;
        order.push(source);
        for route in &buses[source].routes {
            indegree[route.target] -= 1;
        }
    }
    Ok(order)
}

fn fade_gain(direction: FadeDirection, frame: usize, frames: usize) -> f32 {
    if frames <= 1 {
        return match direction {
            FadeDirection::Out => 0.0,
            FadeDirection::In => 1.0,
        };
    }
    let progress = frame as f32 / (frames - 1) as f32;
    match direction {
        FadeDirection::Out => 1.0 - progress,
        FadeDirection::In => progress,
    }
}

fn mix_into_bus(
    source: &StereoScratch,
    target: &mut StereoScratch,
    start: usize,
    end: usize,
    hardware_frames: usize,
    route: &mut RoutePlan,
    context: ProcessContext,
    fade: Option<EngineFade>,
) {
    for frame in start..end {
        let source_gain = fade.map_or(1.0, |mask| {
            fade_gain(mask.direction, frame, hardware_frames)
        });
        let seconds = (context.transport.project_frame + (frame - start) as f64)
            / context.transport.sample_rate;
        let gain = route
            .automation
            .as_ref()
            .map_or(route.gain, |a| 10.0f32.powf(a.value_at(seconds) / 20.0))
            * route.fader.as_ref().map_or(1., |(g, a)| {
                a.as_ref()
                    .map_or(*g, |a| 10.0f32.powf(a.value_at(seconds) / 20.))
            });
        let samples = route.delay.sample(
            source.left[frame] * gain * source_gain * if route.muted { 0.0 } else { 1.0 },
            source.right[frame] * gain * source_gain * if route.muted { 0.0 } else { 1.0 },
        );
        target.left[frame] += samples[0];
        target.right[frame] += samples[1];
    }
}
fn mix_bus_route(
    buses: &mut [BusRuntime],
    source_index: usize,
    route_index: usize,
    start: usize,
    end: usize,
    hardware_frames: usize,
    context: ProcessContext,
    fade: Option<EngineFade>,
) {
    let target = buses[source_index].routes[route_index].target;
    let (source, target) = if source_index < target {
        let (a, b) = buses.split_at_mut(target);
        (&mut a[source_index], &mut b[0])
    } else {
        let (a, b) = buses.split_at_mut(source_index);
        (&mut b[0], &mut a[target])
    };
    mix_into_bus(
        &source.scratch,
        &mut target.scratch,
        start,
        end,
        hardware_frames,
        &mut source.routes[route_index],
        context,
        fade,
    );
}

impl DeviceRuntime {
    fn process(
        &mut self,
        ctx: ProcessContext,
        events: &[super::DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        for i in 0..ctx.frames {
            let v = self.input_delay.sample(left[i], right[i]);
            left[i] = v[0];
            right[i] = v[1];
        }
        if self.automation.is_empty() {
            return if self.sidechain.is_some() {
                self.processor.process_sidechain(
                    ctx,
                    events,
                    left,
                    right,
                    &self.detector.left[..ctx.frames],
                    &self.detector.right[..ctx.frames],
                )
            } else {
                self.processor.process(ctx, events, left, right)
            };
        }
        if self.processor.accepts_parameter_offsets() {
            for i in 0..ctx.frames {
                let seconds = (ctx.transport.project_frame + i as f64) / ctx.transport.sample_rate;
                for (name, a, last) in &mut self.automation {
                    let value = a.value_at(seconds);
                    if value != *last {
                        self.processor.set_parameter_at(name, value, i as u32)?;
                        *last = value;
                    }
                }
            }
            return self.processor.process(ctx, events, left, right);
        }
        let mut index = 0;
        for i in 0..ctx.frames {
            let mut one = ctx;
            one.frames = 1;
            one.block_start_sample += i as u64;
            one.transport.project_frame += i as f64;
            one.transport.sample_position += i as u64;
            one.transport.beat_position +=
                i as f64 * ctx.transport.bpm / (60.0 * ctx.transport.sample_rate);
            for (name, a, last) in &mut self.automation {
                let value = a.value_at(one.transport.project_frame / ctx.transport.sample_rate);
                if value != *last {
                    self.processor.set_parameter(name, value)?;
                    *last = value;
                }
            }
            let mut es = super::ScheduledEvents::new();
            while index < events.len() && events[index].offset as usize == i {
                let mut ev = events[index];
                ev.offset = 0;
                es.push(ev);
                index += 1;
            }
            if self.sidechain.is_some() {
                self.processor.process_sidechain(
                    one,
                    &es,
                    &mut left[i..i + 1],
                    &mut right[i..i + 1],
                    &self.detector.left[i..i + 1],
                    &self.detector.right[i..i + 1],
                )?;
            } else {
                self.processor
                    .process(one, &es, &mut left[i..i + 1], &mut right[i..i + 1])?;
            }
        }
        Ok(())
    }
}
impl AudioEngine {
    pub fn latency_samples(&self) -> usize {
        match self.tap {
            Some((true, i)) => self.track_latency[i],
            Some((false, i)) => self.bus_latency[i],
            None => self.latency,
        }
    }
    pub fn set_tap(&mut self, id: Option<&str>) -> Result<(), EngineError> {
        self.tap = if let Some(id) = id {
            if let Some(i) = self.tracks.iter().position(|t| t.id.as_str() == id) {
                Some((true, i))
            } else if let Some(i) = self.buses.iter().position(|b| b.id.as_str() == id) {
                Some((false, i))
            } else {
                return Err(EngineError::InvalidGraph("unknown render tap"));
            }
        } else {
            None
        };
        Ok(())
    }
    pub fn set_solo(&mut self, names: &[String]) -> Result<(), EngineError> {
        for name in names {
            if !self.tracks.iter().any(|t| {
                t.id.as_str() == name
                    || t.id
                        .as_str()
                        .strip_prefix(name)
                        .is_some_and(|s| s.starts_with('.'))
            }) {
                return Err(EngineError::InvalidGraph("unknown solo track"));
            }
        }
        for t in &mut self.tracks {
            let muted = !names.is_empty()
                && !names.iter().any(|name| {
                    t.id.as_str() == name
                        || t.id
                            .as_str()
                            .strip_prefix(name)
                            .is_some_and(|s| s.starts_with('.'))
                });
            for r in std::iter::once(&mut t.output).chain(&mut t.sends) {
                r.muted = muted;
            }
        }
        Ok(())
    }
    fn prepare_compensation(&mut self) {
        let mut track_lat = vec![0usize; self.tracks.len()];
        for &i in &self.track_order {
            let t = &mut self.tracks[i];
            let mut latency = t.instrument.processor.debug_state().latency_samples as usize;
            for d in &mut t.inserts {
                if let Some(source) = d.sidechain {
                    let sc = track_lat[source];
                    d.input_delay.set_length(sc.saturating_sub(latency));
                    d.detector_delay.set_length(latency.saturating_sub(sc));
                    latency = latency.max(sc);
                }
                latency += d.processor.debug_state().latency_samples as usize;
            }
            track_lat[i] = latency;
        }
        let mut input = vec![0usize; self.buses.len()];
        let mut output = vec![0usize; self.buses.len()];
        for (i, t) in self.tracks.iter().enumerate() {
            for r in std::iter::once(&t.output).chain(&t.sends) {
                input[r.target] = input[r.target].max(track_lat[i]);
            }
        }
        for &i in &self.bus_order {
            let mut latency = input[i];
            for d in &mut self.buses[i].inserts {
                if let Some(track) = d.sidechain {
                    let sc = track_lat[track];
                    d.input_delay.set_length(sc.saturating_sub(latency));
                    d.detector_delay.set_length(latency.saturating_sub(sc));
                    latency = latency.max(sc);
                }
                latency += d.processor.debug_state().latency_samples as usize;
            }
            output[i] = latency;
            for r in &self.buses[i].routes {
                input[r.target] = input[r.target].max(output[i]);
            }
        }
        for (i, t) in self.tracks.iter_mut().enumerate() {
            for r in std::iter::once(&mut t.output).chain(&mut t.sends) {
                r.delay
                    .set_length(input[r.target].saturating_sub(track_lat[i]));
            }
        }
        for (i, b) in self.buses.iter_mut().enumerate() {
            for r in &mut b.routes {
                r.delay
                    .set_length(input[r.target].saturating_sub(output[i]));
            }
        }
        self.latency = output[0];
        self.track_latency = track_lat;
        self.bus_latency = output;
    }
    fn validate_automation(&mut self, session: &model::Session) -> Result<(), EngineError> {
        let available = automation_targets(session);
        let mut targets = std::collections::BTreeSet::new();
        for a in &session.extras.automation {
            if a.target.ends_with(".lookahead_ms") {
                return Err(EngineError::Source(
                    crate::lang::Diagnostic::new(format!(
                        "automation target '{}' changes latency",
                        a.target
                    ))
                    .help("latency changes require a source reload, not an automation curve")
                    .origin(a.origin.as_ref())
                    .to_string(),
                ));
            }
            if !targets.insert(&a.target) {
                return Err(EngineError::Source(
                    crate::lang::Diagnostic::new(format!(
                        "automation target '{}' has more than one lane",
                        a.target
                    ))
                    .help("merge the curves into one automation(...) entry")
                    .origin(a.origin.as_ref())
                    .to_string(),
                ));
            }
            let mut found = false;
            for t in &mut self.tracks {
                for r in std::iter::once(&t.output).chain(&t.sends) {
                    found |= r.automation.as_ref().is_some_and(|v| v.target == a.target);
                }
                for d in std::iter::once(&mut t.instrument).chain(&mut t.inserts) {
                    for (name, lane, _) in &d.automation {
                        if lane.target == a.target {
                            for p in &a.points {
                                d.processor.set_parameter(name, p.value)?;
                            }
                            found = true;
                        }
                    }
                }
            }
            for b in &mut self.buses {
                for r in &b.routes {
                    found |= r.automation.as_ref().is_some_and(|v| v.target == a.target);
                }
                for d in &mut b.inserts {
                    for (name, lane, _) in &d.automation {
                        if lane.target == a.target {
                            for p in &a.points {
                                d.processor.set_parameter(name, p.value)?;
                            }
                            found = true;
                        }
                    }
                }
            }
            if !found {
                let mut diagnostic = crate::lang::Diagnostic::new(format!(
                    "automation target '{}' is unknown",
                    a.target
                ));
                if let Some(closest) =
                    crate::lang::closest(&a.target, available.iter().map(String::as_str))
                {
                    diagnostic = diagnostic.help(format!("did you mean '{closest}'?"));
                }
                return Err(EngineError::Source(
                    diagnostic
                        .help(available_targets_hint(&a.target, &available))
                        .help("targets name a route (`<track>.out` or `<track>.send.<bus>`) or a device parameter (`<device>.<parameter>`)")
                        .origin(a.origin.as_ref())
                        .to_string(),
                ));
            }
        }
        Ok(())
    }
}

impl DeviceRuntime {
    fn feed_detector(&mut self, ctx: ProcessContext, l: &[f32], r: &[f32]) {
        for i in 0..ctx.frames {
            let v = self.detector_delay.sample(l[i], r[i]);
            self.detector.left[i] = v[0];
            self.detector.right[i] = v[1];
        }
    }
}
impl AudioEngine {
    fn prepare_sidechains(&mut self, session: &model::Session) -> Result<(), EngineError> {
        let resolve = |d: &model::Device| -> Result<Option<usize>, EngineError> {
            if let Some(s) = &d.sidechain {
                if !matches!(
                    d.kind,
                    model::DeviceKind::Compressor | model::DeviceKind::Rack
                ) {
                    return Err(EngineError::InvalidGraph(
                        "external sidechain requires a compressor or rack follower",
                    ));
                }
                Ok(Some(
                    session
                        .tracks
                        .iter()
                        .position(|t| t.id.as_str() == s)
                        .ok_or(EngineError::InvalidGraph(
                            "sidechain source must name a track (kit voices use kit.voice)",
                        ))?,
                ))
            } else {
                Ok(None)
            }
        };
        for (i, t) in session.tracks.iter().enumerate() {
            for (j, d) in t.inserts.iter().enumerate() {
                self.tracks[i].inserts[j].sidechain = resolve(d)?;
            }
        }
        for (i, b) in std::iter::once(&session.master)
            .chain(&session.buses)
            .enumerate()
        {
            for (j, d) in b.inserts.iter().enumerate() {
                self.buses[i].inserts[j].sidechain = resolve(d)?;
            }
        }
        let mut done = vec![false; self.tracks.len()];
        while self.track_order.len() < self.tracks.len() {
            let before = self.track_order.len();
            for i in 0..self.tracks.len() {
                if !done[i]
                    && self.tracks[i]
                        .inserts
                        .iter()
                        .all(|d| d.sidechain.is_none_or(|j| done[j]))
                {
                    done[i] = true;
                    self.track_order.push(i);
                }
            }
            if before == self.track_order.len() {
                return Err(EngineError::InvalidGraph("sidechain routing cycle"));
            }
        }
        Ok(())
    }
}
