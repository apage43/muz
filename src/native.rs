//! Source-independent, thread-affine native playback coordinator.
//!
//! Hosts retain accepted and candidate descriptions and promote them only after
//! `RevisionApplied`. Construction, preparation, polling and destruction belong to
//! the same coordinator thread; the audio callback never owns host source state.
use std::{collections::{BTreeMap, BTreeSet, VecDeque}, fmt, marker::PhantomData, rc::Rc, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Instant};
use crate::{Session, host::HostContext, model::{Id, DeviceKind, Vst3Config}, audio::{AudioConfig, PipeWireStatus, TransportSnapshot, DeliveredEvents, PipeWireOutput, PreparedTransaction, AudioEngine, DeviceDebugState}};
use crate::audio::pipewire::{OperationGuard, OutputOperation, OutputPayload, OutputControl};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeOperationId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisionTicket { pub operation: NativeOperationId, pub base_revision: u64, pub revision: u64, pub source_generation: u64 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlTicket { pub operation: NativeOperationId, pub expected_revision: u64 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginStamp { pub generation: u64 }
#[derive(Clone, Debug)]
pub struct PluginRestartRequest { pub expected_revision: u64, pub stamp: PluginStamp, pub instance_tokens: Vec<u64> }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeErrorCode { Cancelled, Superseded, StaleRevision, SessionMismatch, Busy, QueueFull, InvalidRequest, UnknownTrack, ResourceLimit, RevisionOverflow, Prepare, Apply, AudioUnavailable, OutputFault, Shutdown, UnknownOperation }
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct NativeError { pub code: NativeErrorCode, pub message: String }
impl NativeError {
    fn new(code: NativeErrorCode, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelDisposition { Prevented, PendingOutcome, AlreadyResolved }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeControlKind { Running, Restart, Panic, Seek, Loop, Audibility, Observation }
#[derive(Clone, Copy, Debug)]
pub struct NativeTransportStatus { pub generation: u64, pub snapshot: TransportSnapshot, pub loop_range: Option<(u64, u64)>, pub end_tick: u64, pub end_project_frame: f64 }
#[derive(Clone, Debug, serde::Serialize)]
pub struct RuntimeDeviceStatus {
    pub restart_flags: u32, pub id: Id, pub kind: DeviceKind, pub instance_token: u64,
    pub process_count: u64, pub gain_reduction_db: f32, pub latency_samples: u32,
    pub tail_samples: u32, pub is_plugin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<Vst3Config>,
}
#[derive(Clone, Debug)]
pub struct NativeStatus {
    pub accepted_revision: u64, pub runtime_revision: u64, pub source_generation: u64,
    pub pending_revision: Option<RevisionTicket>, pub transport: NativeTransportStatus,
    pub last_peak: f32, pub last_rms: f32, pub delivered_events: DeliveredEvents,
    pub devices: Vec<RuntimeDeviceStatus>, pub runtime_mapping_pending: bool,
    pub stream_start_count: u64, pub audio: PipeWireStatus, pub observation_dropped: u64,
}
#[derive(Clone, Debug)]
pub enum NativeEvent {
    RevisionApplied { ticket: RevisionTicket, transport: NativeTransportStatus, callback_count: u64, latency_ms: f64, structural: bool, faded: bool, plugin_stamp: PluginStamp },
    RevisionRejected { ticket: RevisionTicket, error: NativeError },
    ControlApplied { ticket: ControlTicket, kind: NativeControlKind, transport: NativeTransportStatus },
    ControlRejected { ticket: ControlTicket, kind: NativeControlKind, error: NativeError },
    PluginRestartRequested(PluginRestartRequest), OutputFault(NativeError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrainCount { pub written: usize, pub remaining: bool }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservationTarget { Master, Track(String) }
#[derive(Clone, Debug)]
pub struct ObservationSelection { pub meter_tracks: Vec<String>, pub master_meter: bool, pub analysis: Option<ObservationTarget>, pub meter_hz: u8, pub analysis_hz: u8 }
#[derive(Clone, Debug)]
pub struct TrackMeter { pub id: String, pub peak: f32, pub rms: f32 }
#[derive(Clone, Debug)]
pub struct ObservationFrame {
    pub revision: u64, pub transport_generation: u64, pub sequence: u64,
    pub sample_position: u64, pub sample_rate: f32, pub running: bool,
    pub master: Option<[f32; 2]>, pub tracks: Vec<TrackMeter>,
    pub target: Option<ObservationTarget>, pub mono_samples: Vec<f32>, pub dropped: u64,
}
pub struct ShutdownReport { pub events: Vec<NativeEvent>, pub status: NativeStatus, pub error: Option<NativeError> }

impl PluginStamp {
    pub fn apply(&self, session: &mut Session) {
        for device in session.tracks.iter_mut()
            .flat_map(|t| std::iter::once(&mut t.instrument).chain(&mut t.inserts))
            .chain(session.buses.iter_mut().flat_map(|b| &mut b.inserts))
            .chain(&mut session.master.inserts)
        {
            if matches!(device.kind, DeviceKind::Vst3 | DeviceKind::Clap) || device.rack.is_some() {
                device.generation = self.generation;
            }
        }
    }
    fn matches(&self, session: &Session) -> bool {
        session.tracks.iter().flat_map(|t| std::iter::once(&t.instrument).chain(&t.inserts))
            .chain(session.buses.iter().flat_map(|b| &b.inserts))
            .chain(&session.master.inserts)
            .filter(|d| matches!(d.kind, DeviceKind::Vst3 | DeviceKind::Clap) || d.rack.is_some())
            .all(|d| d.generation == self.generation)
    }
}

#[derive(Clone)]
struct DeviceIdentity { id: Id, kind: DeviceKind, plugin: Option<Vst3Config> }
struct Baseline { signature: u64, tracks: Vec<String>, devices: Vec<DeviceIdentity> }
impl Baseline {
    fn from_session(session: &Session) -> Result<Self, NativeError> {
        Ok(Self {
            signature: signature(session)?,
            tracks: session.tracks.iter().map(|t| t.id.to_string()).collect(),
            devices: session.master.inserts.iter().chain(session.buses.iter().flat_map(|b| &b.inserts))
                .chain(session.tracks.iter().flat_map(|t| std::iter::once(&t.instrument).chain(&t.inserts)))
                .map(|d| DeviceIdentity { id: d.id.clone(), kind: d.kind, plugin: d.vst3.clone() }).collect(),
        })
    }
}
fn signature(session: &Session) -> Result<u64, NativeError> {
    crate::snapshot::signature(session).map_err(|e| NativeError::new(NativeErrorCode::Prepare, e.to_string()))
}
fn cancelled(context: &HostContext) -> Result<(), NativeError> {
    if context.is_cancelled() { Err(NativeError::new(NativeErrorCode::Cancelled, "operation cancelled")) } else { Ok(()) }
}

pub struct PreparedNativeRevision {
    ticket: RevisionTicket,
    transaction: Option<Box<PreparedTransaction>>,
    guard: OperationGuard,
    cancellation: Arc<AtomicBool>,
    preparation_generation: u64,
    policy_generation: u64,
    stamp: PluginStamp,
    baseline: Option<Baseline>,
    restart_tokens: Vec<u64>,
    owner: Rc<()>,
    submitted: bool,
    observation: Option<crate::audio::observation::ObservationConsumer>,
}
impl PreparedNativeRevision { pub fn ticket(&self) -> RevisionTicket { self.ticket } }
impl Drop for PreparedNativeRevision {
    fn drop(&mut self) {
        if !self.submitted { self.guard.cancel(); }
    }
}
impl fmt::Debug for PreparedNativeRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct("PreparedNativeRevision").field("ticket", &self.ticket).finish_non_exhaustive() }
}
pub struct SubmitRevisionError { pub error: NativeError, pub prepared: PreparedNativeRevision }
impl fmt::Debug for SubmitRevisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct("SubmitRevisionError").field("error", &self.error).field("prepared", &self.prepared).finish() }
}
impl fmt::Display for SubmitRevisionError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { fmt::Display::fmt(&self.error, f) } }
impl std::error::Error for SubmitRevisionError {}

struct PendingRevision { ticket: RevisionTicket, baseline: Baseline, stamp: PluginStamp, restart_tokens: Vec<u64> }
struct PendingControl { ticket: ControlTicket, kind: NativeControlKind, policy: Option<ControlPolicy> }
enum ControlPolicy { Audibility(Vec<String>), Observation(ObservationSelection) }
/// Owns native output and plugin lifetimes, but never a caller's accepted Session.
pub struct NativeRuntime {
    output: PipeWireOutput,
    baseline: Baseline,
    revision: u64,
    source_generation: u64,
    stamp: PluginStamp,
    next_operation: u64,
    preparation_generation: u64,
    policy_generation: u64,
    prepared: Option<(NativeOperationId, OperationGuard)>,
    pending_revision: Option<PendingRevision>,
    controls: BTreeMap<u64, PendingControl>,
    guards: BTreeMap<u64, OperationGuard>,
    resolved: VecDeque<NativeOperationId>,
    events: VecDeque<NativeEvent>,
    control_reservations: usize,
    revision_reserved: bool,
    restart_notice: Option<PluginRestartRequest>,
    restart_notice_queued: bool,
    audibility: Option<Vec<String>>,
    observation: Option<ObservationSelection>,
    output_faults: (u64, u64),
    owner: Rc<()>,
    _thread_affine: PhantomData<Rc<()>>,
}
impl fmt::Debug for NativeRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct("NativeRuntime").field("revision", &self.revision).field("source_generation", &self.source_generation).finish_non_exhaustive() }
}

impl NativeRuntime {
    pub fn start(session: &Session, context: &HostContext, running: bool) -> Result<Self, NativeError> {
        Self::start_backend(session, context, running, false)
    }

    pub(crate) fn start_backend(session: &Session, context: &HostContext, running: bool, headless: bool) -> Result<Self, NativeError> {
        cancelled(context)?;
        let stamp = PluginStamp { generation: 0 };
        if !stamp.matches(session) {
            return Err(NativeError::new(NativeErrorCode::SessionMismatch, "initial plugin generation must be zero"));
        }
        let baseline = Baseline::from_session(session)?;
        let output = context.run(|| PipeWireOutput::start_backend(session, running, headless))
            .map_err(|e| NativeError::new(NativeErrorCode::AudioUnavailable, e.to_string()))?;
        cancelled(context)?;
        Ok(Self {
            output, baseline, revision: 0, source_generation: 0, stamp,
            next_operation: 0, preparation_generation: 0, policy_generation: 0,
            prepared: None, pending_revision: None, controls: BTreeMap::new(),
            guards: BTreeMap::new(), resolved: VecDeque::with_capacity(64),
            events: VecDeque::with_capacity(67), control_reservations: 0,
            revision_reserved: false, restart_notice: None, restart_notice_queued: false,
            audibility: None, observation: None, output_faults: (0, 0),
            owner: Rc::new(()), _thread_affine: PhantomData,
        })
    }

    pub fn audio_config(&self) -> AudioConfig { self.output.audio_config() }
    pub fn plugin_stamp(&self) -> PluginStamp { self.stamp }

    pub fn status(&self) -> NativeStatus {
        let runtime = self.output.runtime_snapshot();
        let mapping_pending = runtime.runtime_revision != self.revision
            || runtime.devices.len() != self.baseline.devices.len();
        let devices = if mapping_pending { Vec::new() } else {
            self.baseline.devices.iter().zip(&runtime.devices).map(|(id, state)| runtime_device(id, state)).collect()
        };
        NativeStatus {
            accepted_revision: self.revision, runtime_revision: runtime.runtime_revision,
            source_generation: self.source_generation,
            pending_revision: self.pending_revision.as_ref().map(|p| p.ticket),
            transport: NativeTransportStatus {
                generation: runtime.transport_generation, snapshot: runtime.transport,
                loop_range: runtime.loop_range, end_tick: runtime.end_tick,
                end_project_frame: runtime.end_project_frame,
            },
            last_peak: runtime.last_peak, last_rms: runtime.last_rms,
            delivered_events: runtime.delivered_events, devices,
            runtime_mapping_pending: mapping_pending,
            stream_start_count: runtime.stream_start_count, audio: self.output.status(),
            observation_dropped: self.output.observation_dropped(),
        }
    }

    fn check_revision(&self, expected: u64) -> Result<(), NativeError> {
        if expected != self.revision {
            Err(NativeError::new(NativeErrorCode::StaleRevision, "expected revision is not the accepted revision"))
        } else { Ok(()) }
    }
    fn check_session(&self, accepted: &Session, expected: u64) -> Result<(), NativeError> {
        self.check_revision(expected)?;
        if signature(accepted)? != self.baseline.signature {
            return Err(NativeError::new(NativeErrorCode::SessionMismatch, "borrowed Session does not match the accepted baseline"));
        }
        Ok(())
    }
    fn operation_id(&mut self) -> Result<NativeOperationId, NativeError> {
        self.next_operation = self.next_operation.checked_add(1)
            .ok_or_else(|| NativeError::new(NativeErrorCode::RevisionOverflow, "operation identity exhausted"))?;
        Ok(NativeOperationId(self.next_operation))
    }
    fn resolve(&mut self, operation: NativeOperationId) {
        self.guards.remove(&operation.0);
        if self.resolved.len() == 64 { self.resolved.pop_front(); }
        self.resolved.push_back(operation);
    }
    fn invalidate_preparation(&mut self) -> Result<(), NativeError> {
        self.preparation_generation = self.preparation_generation.checked_add(1)
            .ok_or_else(|| NativeError::new(NativeErrorCode::RevisionOverflow, "preparation generation exhausted"))?;
        if let Some((operation, guard)) = self.prepared.take() {
            guard.cancel();
            self.resolve(operation);
            self.restart_notice = None;
            self.restart_notice_queued = false;
            self.events.retain(|event| !matches!(event, NativeEvent::PluginRestartRequested(_)));
        }
        Ok(())
    }

    pub fn prepare_revision(&mut self, accepted: &Session, candidate: &Session, base_revision: u64, source_generation: u64, context: &HostContext) -> Result<PreparedNativeRevision, NativeError> {
        self.prepare(accepted, candidate, base_revision, source_generation, self.stamp, Vec::new(), context)
    }

    fn prepare(&mut self, accepted: &Session, candidate: &Session, base_revision: u64, source_generation: u64, stamp: PluginStamp, restart_tokens: Vec<u64>, context: &HostContext) -> Result<PreparedNativeRevision, NativeError> {
        self.check_session(accepted, base_revision)?;
        cancelled(context)?;
        if self.revision_reserved || !self.controls.is_empty() {
            return Err(NativeError::new(NativeErrorCode::Busy, "consume pending revision/control outcomes before preparing"));
        }
        if !stamp.matches(candidate) {
            return Err(NativeError::new(NativeErrorCode::SessionMismatch, "candidate plugin stamp differs from runtime"));
        }
        let revision = base_revision.checked_add(1)
            .ok_or_else(|| NativeError::new(NativeErrorCode::RevisionOverflow, "revision exhausted"))?;
        self.invalidate_preparation()?;
        let ticket = RevisionTicket { operation: self.operation_id()?, base_revision, revision, source_generation };
        let guard = OperationGuard::with_cancellation(Some(context.cancelled.clone()));
        let config = self.audio_config();
        let started = Instant::now();
        let loop_range = self.output.transport_status().loop_range;
        let (transaction, observation, baseline) = context.run(|| {
            let plan = crate::plan_reconciliation(base_revision, accepted, candidate)
                .map_err(|e| NativeError::new(NativeErrorCode::Prepare, e.to_string()))?;
            let mut transaction = PreparedTransaction::prepare(accepted, candidate, &plan, source_generation, started, config)
                .map_err(|e| NativeError::new(if context.is_cancelled() { NativeErrorCode::Cancelled } else { NativeErrorCode::Prepare }, e.to_string()))?;
            transaction.prepare_runtime_policy(accepted, candidate, loop_range, config.sample_rate as u64 * 60 * 60, self.audibility.as_deref())
                .map_err(|e| NativeError::new(if context.is_cancelled() { NativeErrorCode::Cancelled } else { NativeErrorCode::Prepare }, e.to_string()))?;
            let baseline = Baseline::from_session(candidate)?;
            let selection = intersect_observation(self.observation.as_ref(), &baseline.tracks);
            let (capture, consumer) = self.prepare_observation(&baseline.tracks, selection.as_ref())?;
            transaction.replace_observation(capture);
            cancelled(context)?;
            Ok::<_, NativeError>((Box::new(transaction), consumer, baseline))
        })?;
        cancelled(context)?;
        self.check_session(accepted, base_revision)?;
        self.prepared = Some((ticket.operation, guard.clone()));
        Ok(PreparedNativeRevision {
            ticket, transaction: Some(transaction), guard,
            cancellation: context.cancelled.clone(),
            preparation_generation: self.preparation_generation,
            policy_generation: self.policy_generation, stamp,
            baseline: Some(baseline), restart_tokens, owner: self.owner.clone(),
            submitted: false, observation,
        })
    }

    pub fn submit_revision(&mut self, mut prepared: PreparedNativeRevision) -> Result<RevisionTicket, SubmitRevisionError> {
        let refusal = if !Rc::ptr_eq(&prepared.owner, &self.owner) {
            Some(NativeError::new(NativeErrorCode::InvalidRequest, "prepared revision belongs to a different runtime"))
        } else if prepared.preparation_generation != self.preparation_generation || prepared.policy_generation != self.policy_generation {
            Some(NativeError::new(NativeErrorCode::Superseded, "prepared revision or its listening policy was superseded"))
        } else if prepared.cancellation.load(Ordering::Acquire) || !prepared.guard.is_pending() {
            Some(NativeError::new(NativeErrorCode::Cancelled, "prepared revision cancelled"))
        } else if prepared.ticket.base_revision != self.revision {
            Some(NativeError::new(NativeErrorCode::StaleRevision, "prepared baseline no longer accepted"))
        } else if self.revision_reserved {
            Some(NativeError::new(NativeErrorCode::Busy, "a revision outcome is still outstanding"))
        } else { None };
        if let Some(error) = refusal { return Err(SubmitRevisionError { error, prepared }); }
        let operation = OutputOperation {
            guard: prepared.guard.clone(),
            payload: OutputPayload::Revision { ticket: prepared.ticket, transaction: prepared.transaction.take().expect("unsubmitted transaction") },
            observation: prepared.observation.take(),
        };
        if let Err(operation) = self.output.submit_operation(operation) {
            if let OutputPayload::Revision { transaction, .. } = operation.payload { prepared.transaction = Some(transaction); }
            prepared.observation = operation.observation;
            return Err(SubmitRevisionError { error: NativeError::new(NativeErrorCode::QueueFull, "native output queue full; prepared ownership retained"), prepared });
        }
        self.guards.insert(prepared.ticket.operation.0, prepared.guard.clone());
        self.pending_revision = Some(PendingRevision {
            ticket: prepared.ticket, baseline: prepared.baseline.take().expect("prepared baseline"),
            stamp: prepared.stamp, restart_tokens: std::mem::take(&mut prepared.restart_tokens),
        });
        self.prepared = None;
        self.revision_reserved = true;
        prepared.submitted = true;
        Ok(prepared.ticket)
    }

    pub fn cancel_operation(&mut self, operation: NativeOperationId) -> Result<CancelDisposition, NativeError> {
        if self.prepared.as_ref().is_some_and(|(id, _)| *id == operation) {
            let (_, guard) = self.prepared.take().expect("matching prepared operation");
            let disposition = if guard.is_pending() { guard.cancel() } else { CancelDisposition::AlreadyResolved };
            self.resolve(operation);
            self.restart_notice = None;
            return Ok(disposition);
        }
        if let Some(guard) = self.guards.get(&operation.0) { return Ok(guard.cancel()); }
        if self.resolved.contains(&operation) { return Ok(CancelDisposition::AlreadyResolved); }
        Err(NativeError::new(NativeErrorCode::UnknownOperation, "operation is unknown or outside the retained outcome window"))
    }
}

fn runtime_device(identity: &DeviceIdentity, state: &DeviceDebugState) -> RuntimeDeviceStatus {
    RuntimeDeviceStatus {
        id: identity.id.clone(), kind: identity.kind, plugin: identity.plugin.clone(),
        instance_token: state.instance_token, process_count: state.process_count,
        gain_reduction_db: if state.gain_reduction_db.is_finite() { state.gain_reduction_db } else { 0.0 },
        latency_samples: state.latency_samples, tail_samples: state.tail_samples,
        restart_flags: state.restart_flags, is_plugin: state.is_plugin,
    }
}

fn intersect_observation(selection: Option<&ObservationSelection>, tracks: &[String]) -> Option<ObservationSelection> {
    selection.map(|selection| {
        let mut selection = selection.clone();
        selection.meter_tracks.retain(|id| tracks.contains(id));
        if matches!(&selection.analysis, Some(ObservationTarget::Track(id)) if !tracks.contains(id)) {
            selection.analysis = None;
        }
        selection
    })
}

impl NativeRuntime {
    pub fn service_plugins(&mut self, accepted: &Session, expected_revision: u64) -> Result<(), NativeError> {
        self.check_session(accepted, expected_revision)?;
        if !self.restart_notice_queued && self.prepared.as_ref().is_some_and(|(_, guard)| !guard.is_pending()) {
            self.restart_notice = None;
        }
        crate::audio::clap::service_main_thread();
        let status = self.status();
        if status.runtime_mapping_pending {
            return Err(NativeError::new(NativeErrorCode::Busy, "promote the callback receipt before mapping plugin requests"));
        }
        let tokens: Vec<_> = status.devices.iter().filter(|d| d.restart_flags != 0).map(|d| d.instance_token).collect();
        if tokens.is_empty() { return Ok(()); }
        let generation = self.stamp.generation.checked_add(1)
            .ok_or_else(|| NativeError::new(NativeErrorCode::RevisionOverflow, "plugin generation exhausted"))?;
        let request = PluginRestartRequest { expected_revision, stamp: PluginStamp { generation }, instance_tokens: tokens };
        if let Some(notice) = &mut self.restart_notice {
            if notice.expected_revision == expected_revision {
                for token in request.instance_tokens {
                    if !notice.instance_tokens.contains(&token) { notice.instance_tokens.push(token); }
                }
                // A queued notice is replaced in place, never multiplied.
                for event in &mut self.events {
                    if let NativeEvent::PluginRestartRequested(queued) = event { *queued = notice.clone(); }
                }
                return Ok(());
            }
        }
        self.events.retain(|e| !matches!(e, NativeEvent::PluginRestartRequested(_)));
        self.restart_notice = Some(request.clone());
        self.restart_notice_queued = true;
        self.events.push_back(NativeEvent::PluginRestartRequested(request));
        Ok(())
    }

    pub fn prepare_plugin_restart(&mut self, accepted: &Session, expected_revision: u64, source_generation: u64, request: &PluginRestartRequest, context: &HostContext) -> Result<PreparedNativeRevision, NativeError> {
        // Consume the notice, not the processor's restart flags. Any refusal can
        // be rediscovered against the fresh accepted baseline.
        self.restart_notice = None;
        self.restart_notice_queued = false;
        self.events.retain(|event| !matches!(event, NativeEvent::PluginRestartRequested(_)));
        self.check_session(accepted, expected_revision)?;
        if request.expected_revision != expected_revision
            || self.stamp.generation.checked_add(1) != Some(request.stamp.generation)
        {
            return Err(NativeError::new(NativeErrorCode::StaleRevision, "plugin restart baseline or stamp expired"));
        }
        let status = self.status();
        if status.runtime_mapping_pending {
            return Err(NativeError::new(NativeErrorCode::Busy, "plugin mapping awaits its revision receipt"));
        }
        let unique: BTreeSet<_> = request.instance_tokens.iter().copied().collect();
        if unique.is_empty() || unique.len() != request.instance_tokens.len()
            || unique.iter().any(|token| !status.devices.iter().any(|d| d.instance_token == *token && d.restart_flags != 0))
        {
            return Err(NativeError::new(NativeErrorCode::InvalidRequest, "plugin restart tokens are missing, duplicate, or no longer requesting restart"));
        }
        let mut candidate = accepted.clone();
        request.stamp.apply(&mut candidate);
        let result = self.prepare(accepted, &candidate, expected_revision, source_generation, request.stamp, request.instance_tokens.clone(), context);
        if result.is_ok() {
            self.restart_notice = Some(request.clone());
        }
        result
    }

    fn check_control(&self, expected_revision: u64) -> Result<(), NativeError> {
        self.check_revision(expected_revision)?;
        if self.control_reservations >= 64 {
            return Err(NativeError::new(NativeErrorCode::QueueFull, "consume reliable control outcomes before submitting more controls"));
        }
        if self.policy_generation == u64::MAX {
            return Err(NativeError::new(NativeErrorCode::RevisionOverflow, "control policy generation exhausted"));
        }
        Ok(())
    }
    fn submit_control(&mut self, expected_revision: u64, kind: NativeControlKind, command: OutputControl, policy: Option<ControlPolicy>, observation: Option<crate::audio::observation::ObservationConsumer>, context: Option<&HostContext>) -> Result<ControlTicket, NativeError> {
        self.check_control(expected_revision)?;
        let ticket = ControlTicket { operation: self.operation_id()?, expected_revision };
        let guard = OperationGuard::with_cancellation(context.map(|context| context.cancelled.clone()));
        let operation = OutputOperation { guard: guard.clone(), payload: OutputPayload::Control { ticket, command }, observation };
        let result = match kind {
            NativeControlKind::Audibility => self.output.submit_audibility(operation),
            NativeControlKind::Observation => self.output.set_observation(operation),
            _ => self.output.submit_operation(operation),
        };
        if result.is_err() {
            return Err(NativeError::new(NativeErrorCode::QueueFull, "native output command queue full"));
        }
        self.policy_generation += 1; // checked before admission; no fallible work after publication
        self.controls.insert(ticket.operation.0, PendingControl { ticket, kind, policy });
        self.guards.insert(ticket.operation.0, guard);
        self.control_reservations += 1;
        Ok(ticket)
    }

    pub fn set_running(&mut self, expected_revision: u64, running: bool) -> Result<ControlTicket, NativeError> {
        self.submit_control(expected_revision, NativeControlKind::Running, OutputControl::Running(running), None, None, None)
    }
    pub fn panic(&mut self, expected_revision: u64) -> Result<ControlTicket, NativeError> {
        self.submit_control(expected_revision, NativeControlKind::Panic, OutputControl::Panic, None, None, None)
    }
    pub fn restart(&mut self, accepted: &Session, expected_revision: u64, context: &HostContext) -> Result<ControlTicket, NativeError> {
        self.check_session(accepted, expected_revision)?;
        let range = self.output.transport_status().loop_range;
        self.seek(accepted, expected_revision, range.map_or(0, |r| r.0), range, NativeControlKind::Restart, context)
    }
    pub fn seek_ticks(&mut self, accepted: &Session, expected_revision: u64, tick: u64, context: &HostContext) -> Result<ControlTicket, NativeError> {
        self.check_session(accepted, expected_revision)?;
        let range = self.output.transport_status().loop_range;
        self.seek(accepted, expected_revision, tick, range, NativeControlKind::Seek, context)
    }
    pub fn set_loop(&mut self, accepted: &Session, expected_revision: u64, range: Option<(u64, u64)>, context: &HostContext) -> Result<ControlTicket, NativeError> {
        self.check_session(accepted, expected_revision)?;
        self.check_control(expected_revision)?;
        cancelled(context)?;
        if let Some((start, end)) = range {
            if start >= end || end > self.output.transport_status().end_tick {
                return Err(NativeError::new(NativeErrorCode::InvalidRequest, "loop must be a nonempty half-open range within the accepted session"));
            }
            if AudioEngine::session_uses_sfz(accepted) {
                return self.seek(accepted, expected_revision, start, range, NativeControlKind::Loop, context);
            }
        }
        self.submit_control(expected_revision, NativeControlKind::Loop, OutputControl::Loop(range), None, None, Some(context))
    }

    fn seek(&mut self, accepted: &Session, expected_revision: u64, tick: u64, range: Option<(u64, u64)>, kind: NativeControlKind, context: &HostContext) -> Result<ControlTicket, NativeError> {
        self.check_control(expected_revision)?;
        cancelled(context)?;
        if self.pending_revision.is_some() || !self.controls.is_empty() {
            return Err(NativeError::new(NativeErrorCode::Busy, "consume pending revision/transport outcomes before preparing transport"));
        }
        let tick = range.map_or(tick, |(start, end)| if (start..end).contains(&tick) { tick } else { start });
        if AudioEngine::session_uses_sfz(accepted) {
            let mut engine = context.run(|| self.prepare_sfz_transport(accepted, tick, range, context))?;
            let (capture, observation) = self.prepare_observation(&self.baseline.tracks, self.observation.as_ref())?;
            engine.replace_observation(capture);
            cancelled(context)?;
            self.submit_control(expected_revision, kind, OutputControl::PreparedTransport { engine: Box::new(engine) }, None, observation, Some(context))
        } else {
            let command = if kind == NativeControlKind::Restart { OutputControl::Restart } else { OutputControl::Seek(tick) };
            self.submit_control(expected_revision, kind, command, None, None, Some(context))
        }
    }

    fn prepare_sfz_transport(&self, accepted: &Session, tick: u64, range: Option<(u64, u64)>, context: &HostContext) -> Result<AudioEngine, NativeError> {
        let config = self.audio_config();
        let budget = config.sample_rate as u64 * 60 * 60;
        let checked = crate::description::ValidatedSession::new(accepted)
            .map_err(|e| NativeError::new(NativeErrorCode::Prepare, e.to_string()))?;
        let result = if let Some((start, end)) = range {
            AudioEngine::prepare_loop_with_audibility(&checked, config, start, end, budget, self.audibility.as_deref()).and_then(|mut candidate| {
                let timeline = crate::audio::transport::TempoTimeline::compile(config.sample_rate as f64, &accepted.transport, &accepted.tracks)
                    .map_err(crate::audio::EngineError::InvalidGraph)?;
                let target = timeline.tick_to_project_frame(tick as f64).round() as u64;
                let mut current = candidate.status().transport.project_frame.round() as u64;
                if target > budget {
                    return Err(crate::audio::EngineError::Preflight("seek exceeds the one-hour replay budget".into()));
                }
                let mut scratch = vec![0.; config.max_frames * 2];
                while current < target {
                    crate::host::check_cancelled().map_err(|e| crate::audio::EngineError::Preflight(e.to_string()))?;
                    let frames = (target - current).min(config.max_frames as u64) as usize;
                    candidate.render_interleaved(&mut scratch[..frames * 2], 2)?;
                    current += frames as u64;
                }
                Ok(candidate)
            })
        } else { AudioEngine::prepare_seek_with_audibility(&checked, config, tick, budget, self.audibility.as_deref()) };
        let engine = result.map_err(|e| NativeError::new(if context.is_cancelled() { NativeErrorCode::Cancelled } else { NativeErrorCode::Prepare }, e.to_string()))?;
        Ok(engine)
    }

    pub fn set_audibility(&mut self, expected_revision: u64, exact_ids: &[String]) -> Result<ControlTicket, NativeError> {
        self.check_control(expected_revision)?;
        validate_ids(&self.baseline.tracks, exact_ids)?;
        let mask = crate::audio::observation::PreparedAudibility::from_track_ids(&self.baseline.tracks, exact_ids)
            .map_err(|e| NativeError::new(NativeErrorCode::Prepare, e.to_string()))?;
        self.submit_control(expected_revision, NativeControlKind::Audibility, OutputControl::Audibility(mask), Some(ControlPolicy::Audibility(exact_ids.to_vec())), None, None)
    }
    pub fn set_observation(&mut self, expected_revision: u64, selection: &ObservationSelection) -> Result<ControlTicket, NativeError> {
        self.check_control(expected_revision)?;
        validate_observation(&self.baseline.tracks, selection)?;
        let (capture, consumer) = self.prepare_observation(&self.baseline.tracks, Some(selection))?;
        self.submit_control(expected_revision, NativeControlKind::Observation, OutputControl::Observation(capture), Some(ControlPolicy::Observation(selection.clone())), consumer, None)
    }
    fn prepare_observation(&self, tracks: &[String], selection: Option<&ObservationSelection>) -> Result<(Option<crate::audio::observation::PreparedObservation>, Option<crate::audio::observation::ObservationConsumer>), NativeError> {
        use crate::audio::observation::PreparedObservation;
        let Some(selection) = selection else { return Ok((None, None)); };
        let pair = if let Some(previous) = self.output.observation_consumer() {
            PreparedObservation::from_track_ids_continuing(tracks, self.audio_config(), selection, previous)
        } else {
            PreparedObservation::from_track_ids(tracks, self.audio_config(), selection)
        }.map_err(|e| NativeError::new(NativeErrorCode::Prepare, e.to_string()))?;
        Ok((Some(pair.0), Some(pair.1)))
    }
    pub fn drain_observations(&mut self, output: &mut [Option<ObservationFrame>]) -> DrainCount {
        self.output.drain_observations(output)
    }
}

fn validate_ids(tracks: &[String], ids: &[String]) -> Result<(), NativeError> {
    let mut unique = BTreeSet::new();
    for id in ids {
        if !tracks.contains(id) { return Err(NativeError::new(NativeErrorCode::UnknownTrack, format!("unknown physical track `{id}`"))); }
        if !unique.insert(id) { return Err(NativeError::new(NativeErrorCode::InvalidRequest, format!("duplicate physical track `{id}`"))); }
    }
    Ok(())
}
fn validate_observation(tracks: &[String], selection: &ObservationSelection) -> Result<(), NativeError> {
    if selection.meter_tracks.len() > 1000 {
        return Err(NativeError::new(NativeErrorCode::ResourceLimit, "at most 1000 physical track meters may be selected"));
    }
    if selection.meter_hz > 20 || selection.analysis_hz > 10 {
        return Err(NativeError::new(NativeErrorCode::InvalidRequest, "meter frequency must be 0..=20 and analysis frequency 0..=10"));
    }
    validate_ids(tracks, &selection.meter_tracks)?;
    if let Some(ObservationTarget::Track(id)) = &selection.analysis {
        validate_ids(tracks, std::slice::from_ref(id))?;
    }
    Ok(())
}

impl NativeRuntime {
    fn collect_receipts(&mut self) {
        if self.prepared.as_ref().is_some_and(|(_, guard)| !guard.is_pending()) {
            let (operation, _) = self.prepared.take().expect("cancelled prepared operation");
            self.resolve(operation);
            self.restart_notice = None;
        }
        while let Some(receipt) = self.output.poll_operation_receipt() {
            match &receipt.operation.payload {
                OutputPayload::Revision { ticket, transaction } => {
                    let pending = self.pending_revision.take().expect("admitted revision has a reserved outcome");
                    debug_assert_eq!(pending.ticket, *ticket);
                    match receipt.result {
                        Ok(()) => {
                            self.revision = ticket.revision;
                            self.source_generation = ticket.source_generation;
                            self.baseline = pending.baseline;
                            self.stamp = pending.stamp;
                            if let Some(ids) = &mut self.audibility { ids.retain(|id| self.baseline.tracks.contains(id)); }
                            self.observation = intersect_observation(self.observation.as_ref(), &self.baseline.tracks);
                            self.restart_notice = None;
                            self.restart_notice_queued = false;
                            self.events.retain(|e| !matches!(e, NativeEvent::PluginRestartRequested(_)));
                            let latency_ms = transaction.event_started().map_or(0.0, |started| receipt.committed_at.saturating_duration_since(started).as_secs_f64() * 1000.0);
                            self.events.push_back(NativeEvent::RevisionApplied {
                                ticket: *ticket, transport: receipt.transport, callback_count: receipt.callback_count,
                                latency_ms, structural: receipt.structural, faded: receipt.faded, plugin_stamp: self.stamp,
                            });
                        }
                        Err(code) => {
                            if !pending.restart_tokens.is_empty() { self.restart_notice = None; }
                            self.events.push_back(NativeEvent::RevisionRejected { ticket: *ticket, error: outcome_error(code) });
                        }
                    }
                    self.resolve(ticket.operation);
                }
                OutputPayload::Control { ticket, .. } => {
                    let pending = self.controls.remove(&ticket.operation.0).expect("admitted control has a reserved outcome");
                    debug_assert_eq!(pending.ticket, *ticket);
                    match receipt.result {
                        Ok(()) => {
                            match pending.policy {
                                Some(ControlPolicy::Audibility(ids)) => self.audibility = Some(ids),
                                Some(ControlPolicy::Observation(selection)) => self.observation = Some(selection),
                                None => {}
                            }
                            self.events.push_back(NativeEvent::ControlApplied { ticket: *ticket, kind: pending.kind, transport: receipt.transport });
                        }
                        Err(code) => self.events.push_back(NativeEvent::ControlRejected { ticket: *ticket, kind: pending.kind, error: outcome_error(code) }),
                    }
                    self.resolve(ticket.operation);
                }
            }
            // This returns engines, captures, masks and transactions to their
            // coordinator thread even when the caller supplied no empty slots.
            drop(receipt);
        }
        let audio = self.output.status();
        let faults = (audio.render_faults, audio.stream_errors);
        if faults != self.output_faults {
            self.output_faults = faults;
            if !self.events.iter().any(|e| matches!(e, NativeEvent::OutputFault(_))) {
                self.events.push_back(NativeEvent::OutputFault(NativeError::new(NativeErrorCode::OutputFault,
                    format!("native output reports {} render faults and {} stream errors", faults.0, faults.1))));
            }
        }
    }

    pub fn poll(&mut self, output: &mut [Option<NativeEvent>]) -> DrainCount {
        self.collect_receipts();
        let mut written = 0;
        for slot in output.iter_mut().filter(|slot| slot.is_none()) {
            let Some(event) = self.events.pop_front() else { break; };
            match &event {
                NativeEvent::RevisionApplied { .. } | NativeEvent::RevisionRejected { .. } => self.revision_reserved = false,
                NativeEvent::ControlApplied { .. } | NativeEvent::ControlRejected { .. } => self.control_reservations -= 1,
                NativeEvent::PluginRestartRequested(_) => self.restart_notice_queued = false,
                NativeEvent::OutputFault(_) => {}
            }
            *slot = Some(event);
            written += 1;
        }
        DrainCount { written, remaining: !self.events.is_empty() }
    }

    pub fn shutdown(mut self) -> ShutdownReport {
        if let Some((_, guard)) = &self.prepared { guard.cancel(); }
        self.output.quiesce();
        self.collect_receipts();
        // Backends can close before their first callback. After quiescence no
        // queued operation can still apply, so every reserved outcome is final.
        if let Some(pending) = self.pending_revision.take() {
            self.events.push_back(NativeEvent::RevisionRejected { ticket: pending.ticket, error: outcome_error(NativeErrorCode::Shutdown) });
            self.resolve(pending.ticket.operation);
        }
        for (_, pending) in std::mem::take(&mut self.controls) {
            self.events.push_back(NativeEvent::ControlRejected { ticket: pending.ticket, kind: pending.kind, error: outcome_error(NativeErrorCode::Shutdown) });
            self.resolve(pending.ticket.operation);
        }
        let status = self.status();
        let error = if status.audio.render_faults != 0 || status.audio.stream_errors != 0 {
            Some(NativeError::new(NativeErrorCode::OutputFault, "native output stopped after reporting audio faults"))
        } else { None };
        ShutdownReport { events: self.events.drain(..).collect(), status, error }
    }
}
impl Drop for NativeRuntime {
    fn drop(&mut self) {
        if let Some((_, guard)) = &self.prepared { guard.cancel(); }
        self.output.quiesce();
        // PipeWireOutput's callback retirement owns all queued payloads. Its
        // quiescence guarantees their destructors run here, never on callback.
    }
}

fn outcome_error(code: NativeErrorCode) -> NativeError {
    NativeError::new(code, match code {
        NativeErrorCode::Cancelled => "operation cancelled before callback mutation",
        NativeErrorCode::StaleRevision => "queued operation no longer matches the callback revision",
        NativeErrorCode::Shutdown => "output shut down before operation applied",
        NativeErrorCode::Superseded => "operation was superseded",
        NativeErrorCode::RevisionOverflow => "native transport generation exhausted",
        _ => "native callback rejected the operation",
    })
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
