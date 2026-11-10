use super::automation::{DelayLine, Lane};
#[cfg(feature = "desktop")]
use super::observation::{PreparedAudibility, PreparedObservation};
use thiserror::Error;

use crate::model;

use super::{
    AudioConfig, DeliveredEvents, DeviceDebugState, DeviceError, DeviceProcessor, DeviceSlot,
    MAX_AUDIO_FRAMES, PreparedTransaction, PreparedValueOperation, PreparedValueTransaction,
    ProcessContext, RuntimeTransport, ScheduleError, ScheduledEvents, TransactionApplyError,
    TransportSnapshot, ValueTransactionApplyError,
    transport::{ArrangementScheduler, TempoTimeline},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadeDirection {
    Out,
    In,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineFade {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    InvalidEvents(String),
    InvalidConfig(&'static str),
    InvalidGraph(&'static str),
    /// A source diagnostic retained for structured consumers and terminal display.
    Source(Box<crate::diagnostic::Diagnostic>),
    Preflight(String),
    UnsupportedChannelCount { channels: usize },
    OutputNotFrameAligned { samples: usize, channels: usize },
    FrameCapacityExceeded { frames: usize, max_frames: usize },
    Device(DeviceError),
    Schedule(ScheduleError),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEvents(message) => write!(f, "invalid performed event data: {message}"),
            Self::InvalidConfig(message) => write!(f, "invalid audio engine configuration: {message}"),
            Self::InvalidGraph(message) => write!(f, "invalid static audio graph: {message}"),
            Self::Source(diagnostic) => std::fmt::Display::fmt(diagnostic, f),
            Self::Preflight(message) => write!(f, "invalid static audio graph: {message}"),
            Self::UnsupportedChannelCount { channels } => {
                write!(f, "audio output must have at least two channels; got {channels}")
            }
            Self::OutputNotFrameAligned { samples, channels } => {
                write!(f, "audio output length {samples} is not divisible by its {channels} channels")
            }
            Self::FrameCapacityExceeded { frames, max_frames } => {
                write!(f, "audio block of {frames} frames exceeds configured maximum of {max_frames}")
            }
            Self::Device(error) => std::fmt::Display::fmt(error, f),
            Self::Schedule(error) => std::fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            // Expose the diagnostic, not Box<Diagnostic>, for typed downcasts.
            Self::Source(diagnostic) => Some(diagnostic.as_ref()),
            Self::Device(error) => std::error::Error::source(error),
            Self::Schedule(error) => std::error::Error::source(error),
            _ => None,
        }
    }
}

impl From<DeviceError> for EngineError {
    fn from(error: DeviceError) -> Self {
        Self::Device(error)
    }
}

impl From<ScheduleError> for EngineError {
    fn from(error: ScheduleError) -> Self {
        Self::Schedule(error)
    }
}

pub struct AudioEngine {
    sfz_loop: Option<SfzLoopCheckpoint>,
    graph_budget: usize,
    pub(crate) description_signature: u64,
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
    /// Tracks grouped by sidechain depth; a wave reads only earlier waves.
    track_waves: Vec<Vec<usize>>,
    /// Offline worker threads for track processing; 1 keeps the serial path.
    render_threads: usize,
    track_latency: Vec<usize>,
    bus_latency: Vec<usize>,
    tap: Option<(bool, usize)>,
    transport_generation: u64,
    #[cfg(feature = "desktop")]
    observation: Option<PreparedObservation>,
    #[cfg(feature = "desktop")]
    observation_generation: u64,
    runtime_loop_policy: Option<Option<(u64, u64)>>,
}

struct SfzLoopCheckpoint {
    enabled: bool,
    range: (u64, u64),
    schedules: Vec<(usize, TrackSchedule)>,
}

impl AudioEngine {
    /// Prepare reusable SFZ history at the loop start outside the callback.
    /// Effects retain their normal loop tails; SFZ instruments and their event
    /// schedules return to the exact recorded state on every boundary.
    pub fn prepare_loop(
        checked: &crate::description::ValidatedSession<'_>,
        config: AudioConfig,
        start: u64,
        end: u64,
        max_replay_frames: u64,
    ) -> Result<Self, EngineError> {
        Self::prepare_loop_with_audibility(checked, config, start, end, max_replay_frames, None)
    }

    pub(crate) fn prepare_loop_with_audibility(
        checked: &crate::description::ValidatedSession<'_>,
        config: AudioConfig,
        start: u64,
        end: u64,
        max_replay_frames: u64,
        audibility: Option<&[String]>,
    ) -> Result<Self, EngineError> {
        if end <= start {
            return Err(EngineError::InvalidConfig("loop end must follow start"));
        }
        let mut candidate = Self::prepare_seek_with_audibility(checked, config, start, max_replay_frames, audibility)?;
        if end > candidate.transport.timeline().end_tick() {
            return Err(EngineError::InvalidConfig(
                "loop exceeds the performed timeline",
            ));
        }
        let mut schedules = Vec::new();
        for (index, track) in candidate.tracks.iter_mut().enumerate() {
            if track.instrument.processor.kind() == model::DeviceKind::Sfz {
                track.instrument.processor.prepare_loop_checkpoint()?;
                schedules.push((index, track.schedule.clone()));
            }
        }
        candidate.sfz_loop = Some(SfzLoopCheckpoint {
            enabled: true,
            range: (start, end),
            schedules,
        });
        candidate.transport.set_loop(Some((start, end)));
        candidate.transport_generation = candidate.transport.discontinuity();
        if let Some(checkpoint) = &candidate.sfz_loop {
            for (index, schedule) in &checkpoint.schedules {
                candidate.tracks[*index]
                    .schedule
                    .restore_playback_from(schedule, candidate.transport_generation);
            }
        }
        Ok(candidate)
    }

    /// SFZ selection and release state depends on the complete performed history.
    pub fn session_uses_sfz(session: &model::Session) -> bool {
        fn contains(device: &model::Device) -> bool {
            device.sfz.is_some()
                || device
                    .rack
                    .as_ref()
                    .is_some_and(|rack| rack.branches.iter().flatten().any(contains))
        }
        session
            .tracks
            .iter()
            .any(|track| contains(&track.instrument) || track.inserts.iter().any(contains))
            || std::iter::once(&session.master)
                .chain(&session.buses)
                .any(|bus| bus.inserts.iter().any(contains))
    }

    /// Prepare an exact seek by replaying the accepted history outside the callback.
    ///
    /// This creates an independent engine and processes all events and DSP up to the
    /// target, including released tails, filters, random selection and keyswitches.
    /// The caller owns cutover and off-thread retirement of the previous engine.
    /// `max_replay_frames` is a hard preparation budget, never a callback workload.
    pub fn prepare_seek(
        checked: &crate::description::ValidatedSession<'_>,
        config: AudioConfig,
        tick: u64,
        max_replay_frames: u64,
    ) -> Result<Self, EngineError> {
        Self::prepare_seek_with_audibility(checked, config, tick, max_replay_frames, None)
    }

    pub(crate) fn prepare_seek_with_audibility(
        checked: &crate::description::ValidatedSession<'_>,
        config: AudioConfig,
        tick: u64,
        max_replay_frames: u64,
        audibility: Option<&[String]>,
    ) -> Result<Self, EngineError> {
        checked.context().run(|| {
            let timeline = TempoTimeline::compile(
                config.sample_rate as f64,
                &checked.description().transport,
                &checked.description().tracks,
            )
            .map_err(EngineError::InvalidGraph)?;
            let frame = timeline.tick_to_project_frame(tick as f64);
            if !frame.is_finite() || frame < 0.0 || frame >= u64::MAX as f64 {
                return Err(EngineError::InvalidConfig("seek frame is out of range"));
            }
            let target = frame.round() as u64;
            if target > max_replay_frames {
                return Err(EngineError::InvalidConfig(
                    "seek exceeds replay preparation budget",
                ));
            }
            if frame > timeline.end_project_frame() {
                return Err(EngineError::InvalidConfig(
                    "prepared seek exceeds the performed timeline",
                ));
            }
            let mut candidate = Self::prepare_checked(checked, config)?;
            candidate.prepare_runtime_audibility(audibility)?;
            candidate.set_running(true);
            let mut scratch = vec![0.0; config.max_frames * 2];
            let mut replayed = 0;
            while replayed < target {
                crate::host::check_cancelled()
                    .map_err(|e| EngineError::Preflight(e.to_string()))?;
                let frames = (target - replayed).min(config.max_frames as u64) as usize;
                candidate.render_interleaved(&mut scratch[..frames * 2], 2)?;
                replayed += frames as u64;
            }
            Ok(candidate)
        })
    }

    pub fn new(session: &model::Session, config: AudioConfig) -> Result<Self, EngineError> {
        validate_config(config)?;
        let checked = crate::description::ValidatedSession::new(session)
            .map_err(|e| EngineError::Preflight(e.to_string()))?;
        Self::from_validated(&checked, config)
    }

    /// Prepare rate-dependent state and resources from an immutable checked DTO.
    pub fn from_validated(
        checked: &crate::description::ValidatedSession<'_>,
        config: AudioConfig,
    ) -> Result<Self, EngineError> {
        checked
            .context()
            .run(|| Self::prepare_checked(checked, config))
    }
    fn prepare_checked(
        checked: &crate::description::ValidatedSession<'_>,
        config: AudioConfig,
    ) -> Result<Self, EngineError> {
        crate::host::check_cancelled().map_err(|e| EngineError::Preflight(e.to_string()))?;
        validate_config(config)?;
        let session = checked.description();

        let mut buses = Vec::with_capacity(session.buses.len() + 1);
        buses.push(BusRuntime::new(&session.master, session, config)?);
        for bus in &session.buses {
            buses.push(BusRuntime::new(bus, session, config)?);
        }

        let transport = RuntimeTransport::from_session(f64::from(config.sample_rate), session)
            .map_err(EngineError::InvalidGraph)?;
        let mut tracks = Vec::with_capacity(session.tracks.len());
        for track in &session.tracks {
            tracks.push(TrackRuntime::new(
                track,
                session,
                config,
                transport.timeline(),
            )?);
        }

        let bus_order = topological_bus_order(&buses)?;
        let device_count = buses.iter().map(|bus| bus.inserts.len()).sum::<usize>()
            + tracks
                .iter()
                .map(|track| 1 + track.inserts.len())
                .sum::<usize>();

        let mut engine = Self {
            sfz_loop: None,
            graph_budget: checked.context().graph_units,
            revision: 0,
            description_signature: crate::snapshot::signature(session)
                .map_err(|e| EngineError::Preflight(e.to_string()))?,
            config,
            transport,
            tracks,
            buses,
            bus_order,
            device_count,
            last_peak: 0.0,
            last_rms: 0.0,
            latency: 0,
            track_order: Vec::new(),
            track_waves: Vec::new(),
            render_threads: 1,
            track_latency: Vec::new(),
            bus_latency: Vec::new(),
            tap: None,
            transport_generation: 0,
            #[cfg(feature = "desktop")]
            observation: None,
            #[cfg(feature = "desktop")]
            observation_generation: 0,
            runtime_loop_policy: None,
        };
        engine.prepare_sidechains(session)?;
        engine.prepare_compensation();
        engine.validate_automation(session)?;
        Ok(engine)
    }

    pub fn set_running(&mut self, running: bool) {
        let before = self.transport.snapshot().project_frame;
        self.transport.set_running(running);
        if self.transport.snapshot().project_frame == before {
            let mut preserved = false;
            for track in &mut self.tracks {
                if track.instrument.processor.kind() == model::DeviceKind::Sfz {
                    track
                        .schedule
                        .preserve_discontinuity(self.transport.discontinuity());
                    preserved = true;
                }
            }
            if preserved {
                self.transport_generation = self.transport.discontinuity();
            }
        }
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
    pub(crate) fn loop_range(&self) -> Option<(u64, u64)> {
        self.transport.audition_loop()
    }
    pub fn panic(&mut self) {
        self.transport.set_running(false);
        self.reset_audio();
    }
    pub(crate) fn panic_voices(&mut self) {
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
    pub fn graph_budget(&self) -> usize {
        self.graph_budget
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn accept_revision(&mut self, revision: u64) {
        if self.runtime_loop_policy.is_none() {
            self.invalidate_sfz_loop();
        }
        self.revision = revision;
    }

    pub(crate) fn inherit_transport_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    /// Mark a coordinator-prepared loop policy, including an explicit clear.
    /// A prepared SFZ checkpoint must have been created against this description.
    pub(crate) fn install_runtime_loop_policy(&mut self, range: Option<(u64, u64)>) {
        self.runtime_loop_policy = Some(range);
        self.transport.set_loop(range);
    }

    #[cfg(feature = "desktop")]
    pub(crate) fn replace_observation(
        &mut self,
        observation: Option<PreparedObservation>,
    ) -> Option<PreparedObservation> {
        if let Some(previous) = &self.observation {
            previous.retire();
        }
        std::mem::replace(&mut self.observation, observation)
    }

    #[cfg(feature = "desktop")]
    pub(crate) fn set_observation_identity(&mut self, revision: u64, generation: u64) {
        self.observation_generation = generation;
        if let Some(observation) = &mut self.observation {
            observation.set_identity(revision, generation);
        }
    }
    #[cfg(feature = "desktop")]
    pub(crate) fn observation_generation(&self) -> u64 { self.observation_generation }

    #[cfg(feature = "desktop")]
    /// The output operation revision-guards this index map before invoking it.
    /// No strings are resolved and the owning payload remains in its receipt.
    pub(crate) fn apply_prepared_audibility(
        &mut self,
        mask: &PreparedAudibility,
        ramp: bool,
    ) -> Result<(), EngineError> {
        if mask.enabled.len() != self.tracks.len() {
            return Err(EngineError::InvalidGraph("audibility map no longer matches the graph"));
        }
        for (track, enabled) in self.tracks.iter_mut().zip(&mask.enabled) {
            let target = if *enabled { 1.0 } else { 0.0 };
            for route in std::iter::once(&mut track.output).chain(&mut track.sends) {
                route.audition_target = target;
                if !ramp {
                    route.audition_gain = target;
                }
            }
        }
        Ok(())
    }

    fn invalidate_sfz_loop(&mut self) {
        if let Some(checkpoint) = &mut self.sfz_loop {
            checkpoint.enabled = false;
            self.transport.set_loop(None);
        }
    }

    pub(crate) fn swap_structural(
        &mut self,
        candidate: &mut Self,
        device_retentions: &[DeviceRetention],
        track_retentions: &[TrackRetention],
        _transport: &model::Transport,
    ) -> Result<(), StructuralTransactionApplyError> {
        if self.config != candidate.config || self.graph_budget != candidate.graph_budget {
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
            if !staged.schedule.can_adopt(&current.schedule) {
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
        if candidate.runtime_loop_policy.is_some()
            && candidate.sfz_loop.as_ref().is_some_and(|checkpoint| checkpoint.enabled)
        {
            // The complete replay engine already represents the loop-start DSP
            // history. Adopting only the old position would corrupt that history.
            candidate.set_running(self.transport.snapshot().running);
        } else {
            candidate.transport.adopt_position_from(&self.transport);
            candidate.transport_generation = self.transport_generation;
            if let Some(range) = candidate.runtime_loop_policy {
                candidate.transport.set_loop(range);
            }
        }
        #[cfg(feature = "desktop")]
        if let Some(previous) = &self.observation {
            previous.retire();
        }
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
            || self.description_signature != transaction.current_signature
            || transaction.config.is_some_and(|c| c != self.config)
        {
            return Err(ValueTransactionApplyError::RuntimeMismatch);
        }
        {
            for (index, id, schedule) in &transaction.schedules {
                if self.tracks.get(*index).is_none_or(|t| t.id != *id) {
                    return Err(ValueTransactionApplyError::RuntimeMismatch);
                }
                if !schedule.can_adopt(&self.tracks[*index].schedule) {
                    return Err(ValueTransactionApplyError::RuntimeMismatch);
                }
            }
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
                    PreparedValueOperation::UpdateTransport { .. } => {}
                }
            }
        }
        for (index, _, schedule) in &mut transaction.schedules {
            let current = &mut self.tracks[*index].schedule;
            match (&mut *current, &mut *schedule) {
                (
                    TrackSchedule::Arrangement { scheduler: a, .. },
                    TrackSchedule::Arrangement { scheduler: b, .. },
                ) => b.adopt(a),
            }
            std::mem::swap(current, schedule);
        }
        if let Some(transport) = transaction.transport.as_mut() {
            transport.adopt_position_from(&self.transport);
            std::mem::swap(&mut self.transport, transport);
        }
        if let Some(range) = transaction.runtime_loop_policy {
            self.install_runtime_loop_policy(range);
        } else {
            self.invalidate_sfz_loop();
        }
        #[cfg(feature = "desktop")]
        if let Some(prepared) = transaction.runtime_observation.as_mut() {
            if let Some(previous) = &self.observation {
                previous.retire();
            }
            std::mem::swap(&mut self.observation, prepared);
        }
        self.revision = transaction.revision();
        self.description_signature = transaction.candidate_signature;
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

    /// Returns bounded SFZ counters for host inspection outside the audio callback.
    pub fn device_sfz_statistics(&self) -> Vec<(model::Id, super::device::SfzStatistics)> {
        let mut states = Vec::new();
        for bus in &self.buses {
            for device in &bus.inserts {
                if let Some(state) = device.processor.sfz_statistics() {
                    states.push((device.id.clone(), state));
                }
            }
        }
        for track in &self.tracks {
            if let Some(state) = track.instrument.processor.sfz_statistics() {
                states.push((track.instrument.id.clone(), state));
            }
            for device in &track.inserts {
                if let Some(state) = device.processor.sfz_statistics() {
                    states.push((device.id.clone(), state));
                }
            }
        }
        states
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

    pub fn render_interleaved_with_fade(
        &mut self,
        output: &mut [f32],
        channels: usize,
        fade: Option<EngineFade>,
    ) -> Result<(), EngineError> {
        if channels < 2 {
            return Err(EngineError::UnsupportedChannelCount { channels });
        }
        if !output.len().is_multiple_of(channels) {
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

        #[cfg(feature = "desktop")]
        let capture_start = self.transport.snapshot();
        let mut offset = 0;
        while offset < frames {
            let block = self.transport.block(frames - offset);
            debug_assert!(block.frames != 0);
            self.render_slice(block, offset, frames, fade)?;
            offset += block.frames;
            let discontinuity = self.transport.discontinuity();
            self.transport.advance(block.frames);
            if self.transport.discontinuity() != discontinuity {
                #[cfg(feature = "desktop")]
                {
                    self.observation_generation = self.observation_generation.checked_add(1)
                        .ok_or(EngineError::InvalidConfig("native transport generation exhausted"))?;
                    if let Some(observation) = &mut self.observation {
                        observation.discontinuity_after(self.revision, self.observation_generation, offset);
                    }
                }
                if let Some(checkpoint) = &self.sfz_loop
                    && checkpoint.enabled
                        && self.transport.audition_loop() == Some(checkpoint.range)
                    {
                        for (index, schedule) in &checkpoint.schedules {
                            let track = &mut self.tracks[*index];
                            track.instrument.processor.restore_loop_checkpoint()?;
                            track
                                .schedule
                                .restore_playback_from(schedule, self.transport.discontinuity());
                        }
                        self.transport_generation = self.transport.discontinuity();
                    }
            }
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
        #[cfg(feature = "desktop")]
        if let Some(observation) = &mut self.observation {
            let tracks = &self.tracks;
            let master = &self.buses[0].scratch;
            observation.capture(
                frames,
                capture_start.sample_position,
                capture_start.running,
                |index| (&tracks[index].scratch.left[..], &tracks[index].scratch.right[..]),
                (&master.left[..], &master.right[..]),
                |frame| fade.filter(|mask| mask.master)
                    .map_or(1.0, |mask| fade_gain(mask.direction, frame, frames)),
            );
        }
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
        let span = MixSpan {
            start: offset,
            end,
            hardware_frames,
        };
        let context = ProcessContext {
            frames: block.frames,
            block_start_sample: block.start_sample(),
            transport: block.snapshot,
        };
        let timeline = self.transport.timeline();
        for track in &mut self.tracks {
            if !block.snapshot.running
                && track.instrument.processor.kind() == model::DeviceKind::Sfz
            {
                track.events.clear();
            } else {
                track.events = track.schedule(timeline, block)?;
            }
        }

        // Offline parallel rendering runs every track chain first; mixing only
        // writes bus scratch and route state, which no track chain reads.
        let parallel = self.render_threads > 1;
        if parallel {
            self.process_tracks_parallel(context, offset, end)?;
        }
        for order_index in 0..self.track_order.len() {
            let track_index = self.track_order[order_index];
            if !parallel {
                {
                    let track = &mut self.tracks[track_index];
                    if context.transport.running
                        || track.instrument.processor.kind() != model::DeviceKind::Sfz
                    {
                        track.instrument.process(
                            context,
                            &track.events,
                            &mut track.scratch.left[offset..end],
                            &mut track.scratch.right[offset..end],
                        )?;
                    }
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
            }
            let track = &mut self.tracks[track_index];
            let source_fade = fade.filter(|mask| mask.track(track_index));
            mix_into_bus(
                &track.scratch,
                &mut self.buses[track.output.target].scratch,
                span,
                &mut track.output,
                context,
                source_fade,
            );
            for route in &mut track.sends {
                mix_into_bus(
                    &track.scratch,
                    &mut self.buses[route.target].scratch,
                    span,
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
                    span,
                    context,
                    source_fade,
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) enum TrackSchedule {
    Arrangement {
        source: crate::midi::ImportedMidi,
        scheduler: Box<ArrangementScheduler>,
    },
}

impl TrackSchedule {
    fn preserve_discontinuity(&mut self, discontinuity: u64) {
        match self {
            Self::Arrangement { scheduler, .. } => scheduler.preserve_discontinuity(discontinuity),
        }
    }
    fn restore_playback_from(&mut self, source: &Self, discontinuity: u64) {
        match (self, source) {
            (
                Self::Arrangement {
                    scheduler: target, ..
                },
                Self::Arrangement {
                    scheduler: source, ..
                },
            ) => target.restore_playback_from(source, discontinuity),
        }
    }

    fn can_adopt(&self, previous: &Self) -> bool {
        match (self, previous) {
            (
                Self::Arrangement {
                    scheduler: next, ..
                },
                Self::Arrangement { scheduler: old, .. },
            ) => next.can_adopt(old),
        }
    }
    pub(crate) fn prepare(
        track: &model::Track,
        timeline: &TempoTimeline,
        config: AudioConfig,
    ) -> Result<Self, EngineError> {
        Ok(match &track.source {
            model::TrackSource::Midi(midi) => {
                // Validate before filtering: malformed direct embedding input
                // must not bypass bounded program checks on an unused channel.
                crate::snapshot::validate_midi(&midi.imported)
                    .map_err(|error| EngineError::Preflight(error.to_string()))?;
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
                source.summary.notes = source.notes.len() as u32;
                source.summary.controllers = source.controllers.len() as u32;
                TrackSchedule::Arrangement {
                    scheduler: Box::new(
                        ArrangementScheduler::compile(&source, timeline, config.max_frames)
                            .map_err(EngineError::Preflight)?,
                    ),
                    source,
                }
            }
        })
    }
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
    /// Threads that ran this track's chain on the parallel path.
    #[cfg(test)]
    processed_on: Vec<std::thread::ThreadId>,
}

impl TrackRuntime {
    fn new(
        track: &model::Track,
        session: &model::Session,
        config: AudioConfig,
        timeline: &TempoTimeline,
    ) -> Result<Self, EngineError> {
        let schedule = TrackSchedule::prepare(track, timeline, config)?;
        let instrument = DeviceRuntime::new(&track.instrument, config, &session.extras)?;
        if let model::TrackSource::Midi(m) = &track.source
            && m.imported.notes.iter().any(|n| {
                n.performance.is_some_and(|p| {
                    p.expression
                        .kinds()
                        .any(|kind| !instrument.processor.accepts_note_expression(kind))
                })
            })
        {
            return Err(EngineError::InvalidGraph(
                "instrument does not support a requested note-expression kind",
            ));
        }
        let model::TrackSource::Midi(m) = &track.source;
        for n in &m.imported.notes {
            let Some(p) = n.performance else { continue };
            for point in p
                .expression
                .points
                .iter()
                .take(usize::from(p.expression.len))
            {
                instrument
                    .processor
                    .check_note_expression(point.kind, f64::from(point.value))
                    .map_err(|e| {
                        EngineError::Preflight(format!("track {}, note {}: {e}", track.id, n.id))
                    })?;
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
            #[cfg(test)]
            processed_on: Vec::new(),
        })
    }

    /// The instrument and inserts of one track, as the serial loop runs them.
    /// External sidechain detectors must already be fed for this slice.
    fn process_chain(
        &mut self,
        context: ProcessContext,
        offset: usize,
        end: usize,
    ) -> Result<(), EngineError> {
        #[cfg(test)]
        self.processed_on.push(std::thread::current().id());
        if context.transport.running || self.instrument.processor.kind() != model::DeviceKind::Sfz {
            self.instrument.process(
                context,
                &self.events,
                &mut self.scratch.left[offset..end],
                &mut self.scratch.right[offset..end],
            )?;
        }
        for insert in &mut self.inserts {
            insert.process(
                context,
                &[],
                &mut self.scratch.left[offset..end],
                &mut self.scratch.right[offset..end],
            )?;
        }
        Ok(())
    }

    /// CLAP instances, including those inside racks, stay on the thread that
    /// drives the render and services their main-thread callbacks.
    fn pinned_to_render_thread(&self) -> bool {
        std::iter::once(&self.instrument)
            .chain(&self.inserts)
            .any(|d| match d.processor.kind() {
                model::DeviceKind::Clap => true,
                model::DeviceKind::Rack => d.processor.debug_state().is_plugin,
                _ => false,
            })
    }

    fn schedule(
        &mut self,
        timeline: &TempoTimeline,
        block: super::TransportBlock,
    ) -> Result<ScheduledEvents, ScheduleError> {
        match &mut self.schedule {
            TrackSchedule::Arrangement { source, scheduler } => {
                scheduler.schedule(source, timeline, block)
            }
        }
    }

    fn delivered_events(&self) -> DeliveredEvents {
        match &self.schedule {
            TrackSchedule::Arrangement { scheduler, .. } => scheduler.delivered(),
        }
    }

    fn reset_delivered(&mut self) {
        let TrackSchedule::Arrangement { scheduler, .. } = &mut self.schedule;
        scheduler.reset_delivered();
    }

    fn swap_scheduler_with(&mut self, other: &mut Self) {
        match (&mut self.schedule, &mut other.schedule) {
            (
                TrackSchedule::Arrangement {
                    scheduler: current, ..
                },
                TrackSchedule::Arrangement {
                    scheduler: staged, ..
                },
            ) => staged.adopt(current),
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
    automation: Vec<(String, Lane, f32)>,
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
        let specs = crate::description::parameter_specs(d.kind);
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
                        .map(|name| (name.to_owned(), Lane::new(a), f32::NAN))
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
    fader: Option<(f32, Option<Lane>)>,
    target: usize,
    gain: f32,
    delay: DelayLine,
    automation: Option<Lane>,
    muted: bool,
    audition_gain: f32,
    audition_target: f32,
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
                            .map(Lane::new),
                    )
                })
            },
            target,
            muted: false,
            audition_gain: 1.0,
            audition_target: 1.0,
            gain: 10.0_f32.powf(route.gain_db / 20.0),
            delay: DelayLine::default(),
            automation: session
                .extras
                .automation
                .iter()
                .find(|a| a.target == route.id.as_str())
                .map(Lane::new),
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

#[derive(Clone, Copy)]
struct MixSpan {
    start: usize,
    end: usize,
    hardware_frames: usize,
}

fn mix_into_bus(
    source: &StereoScratch,
    target: &mut StereoScratch,
    span: MixSpan,
    route: &mut RoutePlan,
    context: ProcessContext,
    fade: Option<EngineFade>,
) {
    let audition_step = 1.0 / (context.transport.sample_rate as f32 * 0.005).max(1.0);
    // Hoisted per-block constants. The per-sample products keep their original
    // operand order, and automated gains reuse the conversion of an unchanged
    // value, so the result is bit-identical to evaluating everything per sample.
    let mute = if route.muted { 0.0 } else { 1.0 };
    let fixed_fader = match &route.fader {
        None => Some(1.),
        Some((g, None)) => Some(*g),
        Some((_, Some(_))) => None,
    };
    let fixed = match (&route.automation, fixed_fader) {
        (None, Some(fader)) => Some(route.gain * fader),
        _ => None,
    };
    for frame in span.start..span.end {
        let source_gain = fade.map_or(1.0, |mask| {
            fade_gain(mask.direction, frame, span.hardware_frames)
        });
        let gain = fixed.unwrap_or_else(|| {
            let seconds = (context.transport.project_frame + (frame - span.start) as f64)
                / context.transport.sample_rate;
            route
                .automation
                .as_mut()
                .map_or(route.gain, |a| a.amplitude_at(seconds))
                * fixed_fader.unwrap_or_else(|| match &mut route.fader {
                    Some((_, Some(a))) => a.amplitude_at(seconds),
                    _ => unreachable!("fixed faders are hoisted"),
                })
        });
        let samples = route.delay.sample(
            source.left[frame] * gain * source_gain * mute,
            source.right[frame] * gain * source_gain * mute,
        );
        route.audition_gain +=
            (route.audition_target - route.audition_gain).clamp(-audition_step, audition_step);
        target.left[frame] += samples[0] * route.audition_gain;
        target.right[frame] += samples[1] * route.audition_gain;
    }
}
fn mix_bus_route(
    buses: &mut [BusRuntime],
    source_index: usize,
    route_index: usize,
    span: MixSpan,
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
        span,
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
        self.process_segments(ctx, events, left, right, self.processor.block_invariant())
    }

    /// Apply sample-accurate automation to a device without parameter offsets.
    /// A block-invariant device processes each run of frames over which no
    /// automated value changes in one call; any other device runs one frame per
    /// call. Both make the same parameter changes at the same frames, so for a
    /// block-invariant device the two are bit-identical.
    fn process_segments(
        &mut self,
        ctx: ProcessContext,
        events: &[super::DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
        segment: bool,
    ) -> Result<(), DeviceError> {
        let seconds =
            |i: usize| (ctx.transport.project_frame + i as f64) / ctx.transport.sample_rate;
        let mut index = 0;
        let mut start = 0;
        while start < ctx.frames {
            for (name, a, last) in &mut self.automation {
                let value = a.value_at(seconds(start));
                if value != *last {
                    self.processor.set_parameter(name, value)?;
                    *last = value;
                }
            }
            let mut end = start + 1;
            if segment {
                while end < ctx.frames
                    && self
                        .automation
                        .iter_mut()
                        .all(|(_, a, last)| a.value_at(seconds(end)) == *last)
                {
                    end += 1;
                }
            }
            let mut run = ctx;
            run.frames = end - start;
            run.block_start_sample += start as u64;
            run.transport.project_frame += start as f64;
            run.transport.sample_position += start as u64;
            run.transport.beat_position +=
                start as f64 * ctx.transport.bpm / (60.0 * ctx.transport.sample_rate);
            // Deliver events in order, as the one-frame loop did.
            let mut es = super::ScheduledEvents::new();
            let mut previous = start;
            while index < events.len() && (previous..end).contains(&(events[index].offset as usize))
            {
                let mut ev = events[index];
                previous = ev.offset as usize;
                ev.offset -= start as u32;
                es.push(ev);
                index += 1;
            }
            if self.sidechain.is_some() {
                self.processor.process_sidechain(
                    run,
                    &es,
                    &mut left[start..end],
                    &mut right[start..end],
                    &self.detector.left[start..end],
                    &self.detector.right[start..end],
                )?;
            } else {
                self.processor
                    .process(run, &es, &mut left[start..end], &mut right[start..end])?;
            }
            start = end;
        }
        Ok(())
    }
}
impl AudioEngine {
    /// Process independent tracks on up to `threads` threads during offline
    /// rendering. Tracks run in sidechain-dependency waves, each into its own
    /// scratch buffer; routing then sums them in the fixed serial track order,
    /// so output is bit-identical for every thread count. Live engines keep
    /// one thread.
    pub fn set_render_threads(&mut self, threads: usize) -> Result<(), EngineError> {
        if threads == 0 || threads > 256 {
            return Err(EngineError::InvalidConfig("render threads must be 1..256"));
        }
        if threads > 1 && !self.config.offline {
            return Err(EngineError::InvalidConfig(
                "parallel track rendering is offline only",
            ));
        }
        self.render_threads = threads;
        Ok(())
    }

    fn process_tracks_parallel(
        &mut self,
        context: ProcessContext,
        offset: usize,
        end: usize,
    ) -> Result<(), EngineError> {
        for wave in &self.track_waves {
            // Feed detectors from earlier waves before any track of this wave runs.
            for &track_index in wave {
                for i in 0..self.tracks[track_index].inserts.len() {
                    let Some(source) = self.tracks[track_index].inserts[i].sidechain else {
                        continue;
                    };
                    let (track, detector) = if source < track_index {
                        let (a, b) = self.tracks.split_at_mut(track_index);
                        (&mut b[0], &a[source].scratch)
                    } else {
                        let (a, b) = self.tracks.split_at_mut(source);
                        (&mut a[track_index], &b[0].scratch)
                    };
                    track.inserts[i].feed_detector(
                        context,
                        &detector.left[offset..end],
                        &detector.right[offset..end],
                    );
                }
            }
            if let [track_index] = wave[..] {
                self.tracks[track_index].process_chain(context, offset, end)?;
                continue;
            }
            let mut member = vec![false; self.tracks.len()];
            for &i in wave {
                member[i] = true;
            }
            let mut pinned = Vec::new();
            let mut shared = Vec::new();
            for (i, track) in self.tracks.iter_mut().enumerate() {
                if member[i] {
                    if track.pinned_to_render_thread() {
                        pinned.push((i, track));
                    } else {
                        shared.push((i, track));
                    }
                }
            }
            let workers = (self.render_threads - 1).min(shared.len());
            let queue = std::sync::Mutex::new(shared.into_iter());
            let errors = std::sync::Mutex::new(Vec::new());
            let drain = || {
                loop {
                    let next = queue.lock().unwrap().next();
                    let Some((i, track)) = next else {
                        break;
                    };
                    if let Err(e) = track.process_chain(context, offset, end) {
                        errors.lock().unwrap().push((i, e));
                    }
                }
            };
            std::thread::scope(|scope| {
                for _ in 0..workers {
                    scope.spawn(drain);
                }
                for (i, track) in pinned {
                    if let Err(e) = track.process_chain(context, offset, end) {
                        errors.lock().unwrap().push((i, e));
                    }
                }
                drain();
            });
            // Report the failure the serial order would have reached first.
            if let Some((_, error)) = errors
                .into_inner()
                .unwrap()
                .into_iter()
                .min_by_key(|(i, _)| wave.iter().position(|t| t == i))
            {
                return Err(error);
            }
        }
        Ok(())
    }

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
    /// Temporary exact-track output mask, independent of authored controls.
    /// All track outputs and sends are gated after delay compensation; voices,
    /// inserts, analysis taps and sidechain detectors continue processing.
    /// Hosts reapply the mask after structural transactions. Empty means silence.
    pub fn set_track_audibility(
        &mut self,
        audible: &[String],
        ramp: bool,
    ) -> Result<(), EngineError> {
        if audible
            .iter()
            .any(|id| !self.tracks.iter().any(|t| t.id.as_str() == id))
        {
            return Err(EngineError::InvalidGraph("unknown audition track"));
        }
        for track in &mut self.tracks {
            let target = if audible.iter().any(|id| track.id.as_str() == id) {
                1.0
            } else {
                0.0
            };
            for route in std::iter::once(&mut track.output).chain(&mut track.sends) {
                route.audition_target = target;
                if !ramp {
                    route.audition_gain = target;
                }
            }
        }
        Ok(())
    }

    /// Install the retained host mask before replay can populate routed effect history.
    pub(crate) fn prepare_runtime_audibility(
        &mut self,
        audibility: Option<&[String]>,
    ) -> Result<(), EngineError> {
        let Some(audibility) = audibility else { return Ok(()); };
        if audibility.iter().all(|id| self.tracks.iter().any(|track| track.id.as_str() == id)) {
            return self.set_track_audibility(audibility, false);
        }
        let retained: Vec<_> = audibility.iter()
            .filter(|id| self.tracks.iter().any(|track| track.id.as_str() == *id))
            .cloned().collect();
        self.set_track_audibility(&retained, false)
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
                return Err(EngineError::Source(Box::new(
                    crate::lang::Diagnostic::new(format!(
                        "automation target '{}' changes latency",
                        a.target
                    ))
                    .help("latency changes require a source reload, not an automation curve")
                    .origin(a.origin.as_ref()),
                )));
            }
            if !targets.insert(&a.target) {
                return Err(EngineError::Source(Box::new(
                    crate::lang::Diagnostic::new(format!(
                        "automation target '{}' has more than one lane",
                        a.target
                    ))
                    .help("merge the curves into one automation(...) entry")
                    .origin(a.origin.as_ref()),
                )));
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
                return Err(EngineError::Source(Box::new(
                    diagnostic
                        .help(available_targets_hint(&a.target, &available))
                        .help("targets name a route (`<track>.out` or `<track>.send.<bus>`) or a device parameter (`<device>.<parameter>`)")
                        .origin(a.origin.as_ref()),
                )));
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
        let mut depth = vec![0usize; self.tracks.len()];
        for &i in &self.track_order {
            depth[i] = self.tracks[i]
                .inserts
                .iter()
                .filter_map(|d| d.sidechain)
                .map(|j| depth[j] + 1)
                .max()
                .unwrap_or(0);
        }
        self.track_waves = vec![Vec::new(); depth.iter().max().map_or(0, |d| d + 1)];
        for &i in &self.track_order {
            self.track_waves[depth[i]].push(i);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
