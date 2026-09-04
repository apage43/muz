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

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("invalid audio engine configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("invalid static audio graph: {0}")]
    InvalidGraph(&'static str),
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
    config: AudioConfig,
    transport: RuntimeTransport,
    tracks: Vec<TrackRuntime>,
    buses: Vec<BusRuntime>,
    bus_order: Vec<usize>,
    device_count: usize,
    last_peak: f32,
    last_rms: f32,
}

impl AudioEngine {
    pub fn new(session: &model::Session, config: AudioConfig) -> Result<Self, EngineError> {
        validate_config(config)?;
        validate_bus_shape(session)?;
        validate_graph_capacity(session)?;

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

        Ok(Self {
            config,
            transport: RuntimeTransport::from_session(f64::from(config.sample_rate), session)
                .map_err(EngineError::InvalidGraph)?,
            tracks,
            buses,
            bus_order,
            device_count,
            last_peak: 0.0,
            last_rms: 0.0,
        })
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

    pub fn update_transport(&mut self, transport: &model::Transport) {
        self.transport.update_source(transport);
    }

    pub fn config(&self) -> AudioConfig {
        self.config
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
                    PreparedValueOperation::UpdateTransport { transport } => {
                        self.transport.update_source(transport);
                    }
                }
            }
        }
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

        let master = &self.buses[0].scratch;
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

        let (tracks, buses) = (&mut self.tracks, &mut self.buses);
        for (track_index, track) in tracks.iter_mut().enumerate() {
            track.instrument.processor.process(
                context,
                &track.events,
                &mut track.scratch.left[offset..end],
                &mut track.scratch.right[offset..end],
            )?;
            for insert in &mut track.inserts {
                insert.processor.process(
                    context,
                    &[],
                    &mut track.scratch.left[offset..end],
                    &mut track.scratch.right[offset..end],
                )?;
            }
            let source_fade = fade.filter(|mask| mask.track(track_index));
            mix_into_bus(
                &track.scratch,
                &mut buses[track.output.target].scratch,
                offset,
                end,
                hardware_frames,
                track.output.gain,
                source_fade,
            );
            for route in &track.sends {
                mix_into_bus(
                    &track.scratch,
                    &mut buses[route.target].scratch,
                    offset,
                    end,
                    hardware_frames,
                    route.gain,
                    source_fade,
                );
            }
        }

        for order_index in 0..self.bus_order.len() {
            let source_index = self.bus_order[order_index];
            {
                let source = &mut self.buses[source_index];
                for insert in &mut source.inserts {
                    insert.processor.process(
                        context,
                        &[],
                        &mut source.scratch.left[offset..end],
                        &mut source.scratch.right[offset..end],
                    )?;
                }
            }
            for route_index in 0..self.buses[source_index].routes.len() {
                let route = self.buses[source_index].routes[route_index];
                let source_fade = fade.filter(|mask| mask.bus(source_index));
                mix_bus_route(
                    &mut self.buses,
                    source_index,
                    route,
                    offset,
                    end,
                    hardware_frames,
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
                source.notes.retain(|note| note.channel == midi.channel);
                source
                    .controllers
                    .retain(|event| event.channel == midi.channel);
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
        let instrument = DeviceRuntime::new(&track.instrument, config)?;
        let mut inserts = Vec::with_capacity(track.inserts.len());
        for device in &track.inserts {
            inserts.push(DeviceRuntime::new(device, config)?);
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
            inserts.push(DeviceRuntime::new(device, config)?);
        }
        let mut routes = Vec::with_capacity(bus.sends.len() + usize::from(bus.output.is_some()));
        if let Some(route) = &bus.output {
            routes.push(RoutePlan::new(route, session)?);
        }
        for route in &bus.sends {
            routes.push(RoutePlan::new(route, session)?);
        }
        Ok(Self {
            inserts,
            routes,
            scratch: StereoScratch::new(),
        })
    }
}

struct DeviceRuntime {
    id: model::Id,
    processor: Box<dyn DeviceProcessor>,
}

impl DeviceRuntime {
    fn new(device: &model::Device, config: AudioConfig) -> Result<Self, DeviceError> {
        Ok(Self {
            id: device.id.clone(),
            processor: super::create_processor(device, config)?,
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

#[derive(Clone, Copy)]
struct RoutePlan {
    target: usize,
    gain: f32,
}

impl RoutePlan {
    fn new(route: &model::Route, session: &model::Session) -> Result<Self, EngineError> {
        let target = bus_index(session, &route.to)
            .ok_or(EngineError::InvalidGraph("route targets an unknown bus"))?;
        if !route.gain_db.is_finite() {
            return Err(EngineError::InvalidGraph("route gain must be finite"));
        }
        Ok(Self {
            target,
            gain: 10.0_f32.powf(route.gain_db / 20.0),
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
    if session.tracks.len() > model::MAX_TRACKS {
        return Err(EngineError::InvalidGraph(
            "track count exceeds the source cap",
        ));
    }
    if session.buses.len() > model::MAX_BUSES {
        return Err(EngineError::InvalidGraph(
            "bus count exceeds the source cap",
        ));
    }
    let device_count = session.master.inserts.len()
        + session
            .buses
            .iter()
            .map(|bus| bus.inserts.len())
            .sum::<usize>()
        + session
            .tracks
            .iter()
            .map(|track| 1 + track.inserts.len())
            .sum::<usize>();
    if device_count > model::MAX_DEVICES {
        return Err(EngineError::InvalidGraph(
            "device count exceeds the source cap",
        ));
    }
    let route_count = session.master.sends.len()
        + usize::from(session.master.output.is_some())
        + session
            .buses
            .iter()
            .map(|bus| bus.sends.len() + usize::from(bus.output.is_some()))
            .sum::<usize>()
        + session
            .tracks
            .iter()
            .map(|track| track.sends.len() + 1)
            .sum::<usize>();
    if route_count > model::MAX_ROUTES {
        return Err(EngineError::InvalidGraph(
            "route count exceeds the source cap",
        ));
    }
    Ok(())
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
    gain: f32,
    fade: Option<EngineFade>,
) {
    for frame in start..end {
        let source_gain = fade.map_or(1.0, |mask| {
            fade_gain(mask.direction, frame, hardware_frames)
        });
        target.left[frame] += source.left[frame] * gain * source_gain;
        target.right[frame] += source.right[frame] * gain * source_gain;
    }
}

fn mix_bus_route(
    buses: &mut [BusRuntime],
    source_index: usize,
    route: RoutePlan,
    start: usize,
    end: usize,
    hardware_frames: usize,
    fade: Option<EngineFade>,
) {
    debug_assert_ne!(source_index, route.target);
    if source_index < route.target {
        let (before, from_target) = buses.split_at_mut(route.target);
        mix_into_bus(
            &before[source_index].scratch,
            &mut from_target[0].scratch,
            start,
            end,
            hardware_frames,
            route.gain,
            fade,
        );
    } else {
        let (before_source, from_source) = buses.split_at_mut(source_index);
        mix_into_bus(
            &from_source[0].scratch,
            &mut before_source[route.target].scratch,
            start,
            end,
            hardware_frames,
            route.gain,
            fade,
        );
    }
}
