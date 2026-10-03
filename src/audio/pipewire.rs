use std::{
    hint::spin_loop,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering, fence},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::Instant,
};

use cpal::{
    BufferSize, SampleFormat, Stream, StreamConfig, SupportedBufferSize,
    SupportedStreamConfigRange,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use serde::Serialize;
use thiserror::Error;

use crate::{
    audio::{
        AudioConfig, DeliveredEvents, DeviceDebugState, MAX_AUDIO_FRAMES, PreparedTransaction,
        TransactionApplyError, TransportSnapshot,
        engine::{AudioEngine, EngineError, EngineFade},
    },
    model,
};
use crate::native::{
    CancelDisposition, ControlTicket, DrainCount, NativeErrorCode,
    NativeTransportStatus, ObservationFrame, RevisionTicket,
};
use super::observation::{ObservationConsumer, PreparedAudibility, PreparedObservation};

const PREFERRED_SAMPLE_RATE: u32 = 48_000;
const PREFERRED_PERIOD: u32 = 256;
const STREAM_GENERATION: u64 = 1;
const TRANSPORT_COMMAND_CAPACITY: usize = 64;
const OUTCOME_CAPACITY: usize = TRANSPORT_COMMAND_CAPACITY + 1;
const PENDING: u8 = 0;
const APPLYING: u8 = 1;
const CANCELLED: u8 = 2;
const RESOLVED: u8 = 3;

/// Shared arbitration only; the owning payload always travels back in its receipt.
#[derive(Clone, Debug)]
pub(crate) struct OperationGuard(Arc<AtomicU8>, Option<Arc<AtomicBool>>);

impl OperationGuard {
    #[cfg(test)]
    pub(crate) fn new() -> Self { Self::with_cancellation(None) }
    pub(crate) fn with_cancellation(cancellation: Option<Arc<AtomicBool>>) -> Self {
        Self(Arc::new(AtomicU8::new(PENDING)), cancellation)
    }
    pub(crate) fn is_pending(&self) -> bool { self.0.load(Ordering::Acquire) == PENDING }
    pub(crate) fn cancel(&self) -> CancelDisposition {
        match self.0.compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) | Err(CANCELLED) => CancelDisposition::Prevented,
            Err(RESOLVED) => CancelDisposition::AlreadyResolved,
            Err(_) => CancelDisposition::PendingOutcome,
        }
    }
    fn begin(&self) -> bool {
        if self.1.as_ref().is_some_and(|token| token.load(Ordering::Acquire)) {
            self.cancel();
        }
        self.0.compare_exchange(PENDING, APPLYING, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
}

pub(crate) enum OutputControl {
    Running(bool),
    Restart,
    Panic,
    Seek(u64),
    Loop(Option<(u64, u64)>),
    PreparedTransport { engine: Box<AudioEngine> },
    Audibility(PreparedAudibility),
    Observation(Option<PreparedObservation>),
}


pub(crate) enum OutputPayload {
    Revision { ticket: RevisionTicket, transaction: Box<PreparedTransaction> },
    Control { ticket: ControlTicket, command: OutputControl },
}

pub(crate) struct OutputOperation {
    pub guard: OperationGuard,
    pub payload: OutputPayload,
    /// Promoted on the coordinator only after the corresponding applied receipt.
    pub observation: Option<ObservationConsumer>,
}

impl OutputOperation {
    fn expected_revision(&self) -> u64 {
        match &self.payload {
            OutputPayload::Revision { ticket, .. } => ticket.base_revision,
            OutputPayload::Control { ticket, .. } => ticket.expected_revision,
        }
    }
    fn is_revision(&self) -> bool { matches!(self.payload, OutputPayload::Revision { .. }) }
}

pub(crate) struct OutputReceipt {
    pub operation: OutputOperation,
    pub result: Result<(), NativeErrorCode>,
    pub transport: NativeTransportStatus,
    pub runtime_revision: u64,
    pub callback_count: u64,
    pub committed_at: Instant,
    pub structural: bool,
    pub faded: bool,
}

#[derive(Debug, Error)]
pub enum PipeWireError {
    #[error("PipeWire host is unavailable: {0}")]
    Host(cpal::Error),
    #[error("PipeWire has no default output device")]
    NoDefaultOutputDevice,
    #[error("failed to enumerate PipeWire output configurations: {0}")]
    OutputConfigurations(cpal::Error),
    #[error(
        "PipeWire default output device has no supported f32 configuration with at least two channels and a bounded period"
    )]
    NoSupportedOutputConfiguration,
    #[error("failed to construct the audio engine: {0}")]
    Engine(#[from] EngineError),
    #[error("failed to build the PipeWire output stream: {0}")]
    BuildStream(cpal::Error),
    #[error("failed to query the negotiated PipeWire period: {0}")]
    QueryPeriod(cpal::Error),
    #[error("PipeWire negotiated an unsupported period of {period} frames (maximum {maximum})")]
    UnsupportedNegotiatedPeriod { period: u32, maximum: usize },
    #[error("failed to start the PipeWire output stream: {0}")]
    PlayStream(cpal::Error),
}


#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PipeWireStatus {
    pub host: &'static str,
    pub device: String,
    pub rate: u32,
    pub channels: u16,
    pub requested_period: Option<u32>,
    pub negotiated_period: Option<u32>,
    pub stream_generation: u64,
    pub callback_count: u64,
    pub render_faults: u64,
    pub stream_errors: u64,
    pub transaction_commits: u64,
    pub transaction_faults: u64,
    pub structural_commits: u64,
    pub structural_transition_active: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeTelemetrySnapshot {
    pub runtime_revision: u64,
    pub transport: TransportSnapshot,
    pub transport_generation: u64,
    pub loop_range: Option<(u64, u64)>,
    pub end_tick: u64,
    pub end_project_frame: f64,
    pub last_peak: f32,
    pub last_rms: f32,
    pub delivered_events: DeliveredEvents,
    pub devices: Vec<DeviceDebugState>,
    pub stream_generation: u64,
    pub stream_start_count: u64,
    pub callback_count: u64,
    pub render_faults: u64,
    pub stream_errors: u64,
    pub transaction_commits: u64,
    pub transaction_faults: u64,
    pub structural_commits: u64,
    pub structural_transition_active: bool,
}

#[derive(Default)]
struct CallbackCounters {
    callback_count: AtomicU64,
    transport_generation: AtomicU64,
    render_faults: AtomicU64,
    stream_errors: AtomicU64,
    transaction_commits: AtomicU64,
    transaction_faults: AtomicU64,
    structural_commits: AtomicU64,
    structural_transition_active: AtomicBool,
}
const EMPTY_DEVICE_DEBUG_STATE: DeviceDebugState = DeviceDebugState {
    instance_token: 0,
    process_count: 0,
    gain_reduction_db: 0.0,
    latency_samples: 0,
    tail_samples: 0,
    restart_flags: 0,
    is_plugin: false,
};

struct RuntimeTelemetry {
    sequence: AtomicU64,
    runtime_revision: AtomicU64,
    transport_generation: AtomicU64,
    loop_enabled: AtomicBool,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    sample_rate_bits: AtomicU64,
    running: AtomicU64,
    sample_position: AtomicU64,
    beat_position_bits: AtomicU64,
    bpm_bits: AtomicU64,
    meter: AtomicU64,
    loop_ticks: AtomicU64,
    current_tick_bits: AtomicU64,
    project_frame_bits: AtomicU64,
    end_tick: AtomicU64,
    end_project_frame_bits: AtomicU64,
    ended: AtomicU64,
    last_peak_bits: AtomicU64,
    last_rms_bits: AtomicU64,
    delivered_note_ons: AtomicU64,
    delivered_note_offs: AtomicU64,
    delivered_controllers: AtomicU64,
    device_count: AtomicU64,
    device_tokens: Vec<AtomicU64>,
    device_process_counts: Vec<AtomicU64>,
    device_gain_reduction_bits: Vec<AtomicU64>,
    device_latency_samples: Vec<AtomicU64>,
    device_tail_samples: Vec<AtomicU64>,
    device_flags: Vec<AtomicU64>,
    stream_generation: AtomicU64,
    stream_start_count: AtomicU64,
    callback_count: AtomicU64,
    render_faults: AtomicU64,
    stream_errors: AtomicU64,
    transaction_commits: AtomicU64,
    transaction_faults: AtomicU64,
    structural_commits: AtomicU64,
    structural_transition_active: AtomicU64,
}

impl RuntimeTelemetry {
    fn new(budget: usize) -> Self {
        let atoms = || (0..budget).map(|_| AtomicU64::new(0)).collect();
        Self {
            sequence: AtomicU64::new(0),
            runtime_revision: AtomicU64::new(0),
            transport_generation: AtomicU64::new(0),
            loop_enabled: AtomicBool::new(false),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            sample_rate_bits: AtomicU64::new(0),
            running: AtomicU64::new(0),
            sample_position: AtomicU64::new(0),
            beat_position_bits: AtomicU64::new(0),
            bpm_bits: AtomicU64::new(0),
            meter: AtomicU64::new(0),
            loop_ticks: AtomicU64::new(0),
            current_tick_bits: AtomicU64::new(0),
            project_frame_bits: AtomicU64::new(0),
            end_tick: AtomicU64::new(0),
            end_project_frame_bits: AtomicU64::new(0),
            ended: AtomicU64::new(0),
            last_peak_bits: AtomicU64::new(0),
            last_rms_bits: AtomicU64::new(0),
            delivered_note_ons: AtomicU64::new(0),
            delivered_note_offs: AtomicU64::new(0),
            delivered_controllers: AtomicU64::new(0),
            device_count: AtomicU64::new(0),
            device_tokens: atoms(),
            device_process_counts: atoms(),
            device_gain_reduction_bits: atoms(),
            device_latency_samples: atoms(),
            device_tail_samples: atoms(),
            device_flags: atoms(),
            stream_generation: AtomicU64::new(0),
            stream_start_count: AtomicU64::new(0),
            callback_count: AtomicU64::new(0),
            render_faults: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
            transaction_commits: AtomicU64::new(0),
            transaction_faults: AtomicU64::new(0),
            structural_commits: AtomicU64::new(0),
            structural_transition_active: AtomicU64::new(0),
        }
    }
}

impl RuntimeTelemetry {
    fn publish(
        &self,
        runtime_revision: u64,
        engine: &AudioEngine,
        counters: &CallbackCounters,
        devices: &mut [DeviceDebugState],
    ) {
        let status = engine.status();
        let device_count = engine.copy_device_debug_states(devices);

        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.runtime_revision
            .store(runtime_revision, Ordering::Relaxed);
        self.transport_generation.store(counters.transport_generation.load(Ordering::Relaxed), Ordering::Relaxed);
        let range = engine.loop_range();
        self.loop_enabled.store(range.is_some(), Ordering::Relaxed);
        self.loop_start.store(range.map_or(0, |r| r.0), Ordering::Relaxed);
        self.loop_end.store(range.map_or(0, |r| r.1), Ordering::Relaxed);
        self.sample_rate_bits
            .store(status.transport.sample_rate.to_bits(), Ordering::Relaxed);
        self.running
            .store(u64::from(status.transport.running), Ordering::Relaxed);
        self.sample_position
            .store(status.transport.sample_position, Ordering::Relaxed);
        self.beat_position_bits
            .store(status.transport.beat_position.to_bits(), Ordering::Relaxed);
        self.bpm_bits
            .store(status.transport.bpm.to_bits(), Ordering::Relaxed);
        self.meter.store(
            u64::from(status.transport.meter[0]) | (u64::from(status.transport.meter[1]) << 8),
            Ordering::Relaxed,
        );
        self.loop_ticks
            .store(status.transport.loop_ticks, Ordering::Relaxed);
        self.current_tick_bits
            .store(status.current_tick.to_bits(), Ordering::Relaxed);
        self.project_frame_bits
            .store(status.transport.project_frame.to_bits(), Ordering::Relaxed);
        self.end_tick.store(status.end_tick, Ordering::Relaxed);
        self.end_project_frame_bits
            .store(status.end_project_frame.to_bits(), Ordering::Relaxed);
        self.ended.store(u64::from(status.ended), Ordering::Relaxed);
        self.last_peak_bits.store(
            u64::from(finite_sample(status.last_peak).to_bits()),
            Ordering::Relaxed,
        );
        self.last_rms_bits.store(
            u64::from(finite_sample(status.last_rms).to_bits()),
            Ordering::Relaxed,
        );
        self.delivered_note_ons
            .store(status.delivered_events.note_ons, Ordering::Relaxed);
        self.delivered_note_offs
            .store(status.delivered_events.note_offs, Ordering::Relaxed);
        self.delivered_controllers
            .store(status.delivered_events.controllers, Ordering::Relaxed);
        for (index, state) in devices[..device_count].iter().enumerate() {
            self.device_tokens[index].store(state.instance_token, Ordering::Relaxed);
            self.device_process_counts[index].store(state.process_count, Ordering::Relaxed);
            self.device_gain_reduction_bits[index].store(
                u64::from(finite_sample(state.gain_reduction_db).to_bits()),
                Ordering::Relaxed,
            );
            self.device_latency_samples[index]
                .store(u64::from(state.latency_samples), Ordering::Relaxed);
            self.device_tail_samples[index].store(u64::from(state.tail_samples), Ordering::Relaxed);
            self.device_flags[index].store(
                u64::from(state.is_plugin) | ((state.restart_flags as u64) << 1),
                Ordering::Relaxed,
            );
        }
        self.device_count
            .store(device_count as u64, Ordering::Relaxed);
        self.stream_generation
            .store(STREAM_GENERATION, Ordering::Relaxed);
        self.stream_start_count.store(1, Ordering::Relaxed);
        self.callback_count.store(
            counters.callback_count.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.render_faults.store(
            counters.render_faults.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.stream_errors.store(
            counters.stream_errors.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.transaction_commits.store(
            counters.transaction_commits.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.transaction_faults.store(
            counters.transaction_faults.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.structural_commits.store(
            counters.structural_commits.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.structural_transition_active.store(
            u64::from(
                counters
                    .structural_transition_active
                    .load(Ordering::Acquire),
            ),
            Ordering::Relaxed,
        );
        self.sequence.fetch_add(1, Ordering::Release);
    }

    fn snapshot(&self) -> RuntimeTelemetrySnapshot {
        let mut devices = vec![EMPTY_DEVICE_DEBUG_STATE; self.device_tokens.len()];
        loop {
            let sequence = self.sequence.load(Ordering::Acquire);
            if sequence & 1 != 0 {
                spin_loop();
                continue;
            }

            let runtime_revision = self.runtime_revision.load(Ordering::Relaxed);
            let transport_generation = self.transport_generation.load(Ordering::Relaxed);
            let loop_range = self.loop_enabled.load(Ordering::Relaxed).then(|| (
                self.loop_start.load(Ordering::Relaxed), self.loop_end.load(Ordering::Relaxed)));
            let transport = TransportSnapshot {
                sample_rate: f64::from_bits(self.sample_rate_bits.load(Ordering::Relaxed)),
                running: self.running.load(Ordering::Relaxed) != 0,
                sample_position: self.sample_position.load(Ordering::Relaxed),
                beat_position: f64::from_bits(self.beat_position_bits.load(Ordering::Relaxed)),
                bpm: f64::from_bits(self.bpm_bits.load(Ordering::Relaxed)),
                meter: {
                    let meter = self.meter.load(Ordering::Relaxed);
                    [meter as u8, (meter >> 8) as u8]
                },
                loop_ticks: self.loop_ticks.load(Ordering::Relaxed),
                current_tick: f64::from_bits(self.current_tick_bits.load(Ordering::Relaxed)),
                project_frame: f64::from_bits(self.project_frame_bits.load(Ordering::Relaxed)),
                ended: self.ended.load(Ordering::Relaxed) != 0,
            };
            let last_peak = f32::from_bits(self.last_peak_bits.load(Ordering::Relaxed) as u32);
            let end_tick = self.end_tick.load(Ordering::Relaxed);
            let end_project_frame =
                f64::from_bits(self.end_project_frame_bits.load(Ordering::Relaxed));
            let last_rms = f32::from_bits(self.last_rms_bits.load(Ordering::Relaxed) as u32);
            let delivered_events = DeliveredEvents {
                note_ons: self.delivered_note_ons.load(Ordering::Relaxed),
                note_offs: self.delivered_note_offs.load(Ordering::Relaxed),
                controllers: self.delivered_controllers.load(Ordering::Relaxed),
            };
            let device_count =
                (self.device_count.load(Ordering::Relaxed) as usize).min(self.device_tokens.len());
            for (index, state) in devices[..device_count].iter_mut().enumerate() {
                state.instance_token = self.device_tokens[index].load(Ordering::Relaxed);
                state.process_count = self.device_process_counts[index].load(Ordering::Relaxed);
                state.gain_reduction_db = f32::from_bits(
                    self.device_gain_reduction_bits[index].load(Ordering::Relaxed) as u32,
                );
                state.latency_samples =
                    self.device_latency_samples[index].load(Ordering::Relaxed) as u32;
                state.tail_samples = self.device_tail_samples[index].load(Ordering::Relaxed) as u32;
                let flags = self.device_flags[index].load(Ordering::Relaxed);
                state.is_plugin = flags & 1 != 0;
                state.restart_flags = (flags >> 1) as u32;
            }
            let stream_generation = self.stream_generation.load(Ordering::Relaxed);
            let stream_start_count = self.stream_start_count.load(Ordering::Relaxed);
            let callback_count = self.callback_count.load(Ordering::Relaxed);
            let render_faults = self.render_faults.load(Ordering::Relaxed);
            let stream_errors = self.stream_errors.load(Ordering::Relaxed);
            let transaction_commits = self.transaction_commits.load(Ordering::Relaxed);
            let transaction_faults = self.transaction_faults.load(Ordering::Relaxed);
            let structural_commits = self.structural_commits.load(Ordering::Relaxed);
            let structural_transition_active =
                self.structural_transition_active.load(Ordering::Relaxed) != 0;

            fence(Ordering::Acquire);
            if self.sequence.load(Ordering::Relaxed) == sequence {
                return RuntimeTelemetrySnapshot {
                    runtime_revision,
                    transport_generation,
                    loop_range,
                    transport,
                    end_tick,
                    end_project_frame,
                    last_peak,
                    last_rms,
                    delivered_events,
                    devices: devices[..device_count].to_vec(),
                    stream_generation,
                    stream_start_count,
                    callback_count,
                    render_faults,
                    stream_errors,
                    transaction_commits,
                    transaction_faults,
                    structural_commits,
                    structural_transition_active,
                };
            }
            spin_loop();
        }
    }
}

type AudioCallback = Box<dyn FnMut(Option<&mut [f32]>) + Send>;

// The callback owns the engine and pending transactions while running. Returning the
// entire closure also returns those objects; plugins must be destroyed on their
// preparation thread. This channel is used once, after processing has stopped.
struct ReturningCallback {
    callback: Option<AudioCallback>,
    sender: SyncSender<AudioCallback>,
}
impl ReturningCallback {
    fn process(&mut self, output: &mut [f32]) {
        self.callback.as_mut().unwrap()(Some(output));
    }
}
impl Drop for ReturningCallback {
    fn drop(&mut self) {
        if let Some(callback) = self.callback.take() {
            let _ = self.sender.send(callback);
        }
    }
}
struct CallbackRetirement(Receiver<AudioCallback>);
impl Drop for CallbackRetirement {
    fn drop(&mut self) {
        // The stream/worker is dropped first, including on failed startup. Waiting
        // also covers a backend that finishes releasing its callback asynchronously.
        if let Ok(mut callback) = self.0.recv() {
            callback(None);
            drop(callback);
        }
    }
}


/// The one callback-side FIFO. Every removed command is either retained here or
/// in the reserved receipt queue; no owning command is discarded on this thread.
pub(crate) struct CallbackProcessor {
    engine: AudioEngine,
    operations: Consumer<OutputOperation>,
    receipts: Producer<OutputReceipt>,
    pending: Option<OutputOperation>,
    pending_receipt: Option<OutputReceipt>,
    generation: u64,
    counters: Arc<CallbackCounters>,
    telemetry: Arc<RuntimeTelemetry>,
    device_scratch: Vec<DeviceDebugState>,
}

impl CallbackProcessor {
    fn transport(&self) -> NativeTransportStatus {
        let status = self.engine.status();
        NativeTransportStatus {
            generation: self.generation,
            snapshot: status.transport,
            loop_range: self.engine.loop_range(),
            end_tick: status.end_tick,
            end_project_frame: status.end_project_frame,
        }
    }

    fn finish(&mut self, operation: OutputOperation, result: Result<(), NativeErrorCode>, faded: bool) {
        let structural = matches!(&operation.payload,
            OutputPayload::Revision { transaction, .. } if transaction.is_structural());
        operation.guard.0.store(RESOLVED, Ordering::Release);
        let receipt = OutputReceipt {
            operation, result, transport: self.transport(),
            runtime_revision: self.engine.revision(),
            callback_count: self.counters.callback_count.load(Ordering::Relaxed),
            committed_at: Instant::now(), structural, faded,
        };
        if let Err(PushError::Full(receipt)) = self.receipts.push(receipt) {
            // Reservation makes this unreachable in ordinary operation. Retain
            // instead of freeing processors if an internal invariant is violated.
            self.pending_receipt = Some(receipt);
        }
    }

    fn retry_receipt(&mut self) -> bool {
        if let Some(receipt) = self.pending_receipt.take()
            && let Err(PushError::Full(receipt)) = self.receipts.push(receipt)
        {
            self.pending_receipt = Some(receipt);
        }
        self.pending_receipt.is_none()
    }

    fn apply(&mut self, operation: &mut OutputOperation) -> Result<(), NativeErrorCode> {
        match &mut operation.payload {
            OutputPayload::Revision { transaction, .. } => {
                let before = self.engine.status().transport;
                let old_loop = self.engine.loop_range();
                // Reserve the counter before mutation as well as the receipt.
                let next = self.generation.checked_add(1).ok_or(NativeErrorCode::RevisionOverflow)?;
                let commit = commit_transaction(&mut self.engine, transaction, &self.counters);
                if commit.failure.is_some() { return Err(NativeErrorCode::Apply); }
                let after = self.engine.status().transport;
                if before.project_frame != after.project_frame || old_loop != self.engine.loop_range()
                    || before.running != after.running
                    || before.sample_position != after.sample_position
                {
                    self.generation = next;
                }
            }
            OutputPayload::Control { command, .. } => {
                let changes_transport = match command {
                    OutputControl::Audibility(_) | OutputControl::Observation(_) => false,
                    OutputControl::Running(running) => *running != self.engine.status().transport.running,
                    OutputControl::Panic => false,
                    _ => true,
                };
                let next = if changes_transport {
                    self.generation.checked_add(1).ok_or(NativeErrorCode::RevisionOverflow)?
                } else { self.generation };
                match command {
                    OutputControl::Running(running) => self.engine.set_running(*running),
                    OutputControl::Restart => {
                        let tick = self.engine.loop_range().map_or(0, |range| range.0);
                        self.engine.seek_ticks(tick);
                    }
                    OutputControl::Panic => self.engine.panic_voices(),
                    OutputControl::Seek(tick) => {
                        let tick = match self.engine.loop_range() {
                            Some((start, end)) if *tick < start || *tick >= end => start,
                            _ => *tick,
                        };
                        self.engine.seek_ticks(tick);
                    }
                    OutputControl::Loop(range) => {
                        self.engine.set_loop(*range);
                        if let Some((start, _)) = *range { self.engine.seek_ticks(start); }
                    }
                    OutputControl::PreparedTransport { engine, .. } => {
                        engine.inherit_transport_revision(self.engine.revision());
                        engine.set_running(self.engine.status().transport.running);
                        std::mem::swap(&mut self.engine, engine.as_mut());
                    }
                    OutputControl::Audibility(mask) => self.engine.apply_prepared_audibility(mask, true)
                        .map_err(|_| NativeErrorCode::Apply)?,
                    OutputControl::Observation(prepared) => {
                        *prepared = self.engine.replace_observation(prepared.take());
                    }
                }
                self.generation = next;
            }
        }
        self.engine.set_observation_identity(self.engine.revision(), self.generation);
        self.counters.transport_generation.store(self.generation, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn process(&mut self, output: &mut [f32], channels: usize) {
        self.counters.callback_count.fetch_add(1, Ordering::Relaxed);
        let mut rendered = false;
        if self.retry_receipt() {
            if let Some(mut operation) = self.pending.take() {
                let result = self.apply(&mut operation);
                let fade = match &operation.payload {
                    OutputPayload::Revision { transaction, .. } if result.is_ok() => transaction.fade_in(),
                    OutputPayload::Revision { transaction, .. } => transaction.fade_recovery(),
                    _ => None,
                };
                render_block(&mut self.engine, output, channels, fade, &self.counters);
                self.generation = self.engine.observation_generation();
                self.counters.transport_generation.store(self.generation, Ordering::Relaxed);
                rendered = true;
                self.counters.structural_transition_active.store(false, Ordering::Release);
                self.finish(operation, result, true);
            } else {
                while self.pending_receipt.is_none() {
                    let Ok(mut operation) = self.operations.pop() else { break };
                    if !operation.guard.begin() {
                        self.finish(operation, Err(NativeErrorCode::Cancelled), false);
                        continue;
                    }
                    if operation.expected_revision() != self.engine.revision() {
                        self.finish(operation, Err(NativeErrorCode::StaleRevision), false);
                        continue;
                    }
                    if let OutputPayload::Revision { transaction, .. } = &operation.payload {
                        if !transaction.matches_runtime(&self.engine) {
                            self.finish(operation, Err(NativeErrorCode::StaleRevision), false);
                            continue;
                        }
                        if transaction.needs_fade() {
                            render_block(&mut self.engine, output, channels, transaction.fade_out(), &self.counters);
                            self.generation = self.engine.observation_generation();
                            self.counters.transport_generation.store(self.generation, Ordering::Relaxed);
                            rendered = true;
                            self.counters.structural_transition_active.store(true, Ordering::Release);
                            self.pending = Some(operation);
                            break;
                        }
                    }
                    let result = self.apply(&mut operation);
                    self.finish(operation, result, false);
                }
            }
        }
        if !rendered { render_block(&mut self.engine, output, channels, None, &self.counters); }
        self.generation = self.engine.observation_generation();
        self.counters.transport_generation.store(self.generation, Ordering::Relaxed);
        self.telemetry.publish(self.engine.revision(), &self.engine, &self.counters, &mut self.device_scratch);
    }

    fn quiesce(&mut self) {
        if !self.retry_receipt() { return; }
        if let Some(operation) = self.pending.take() {
            self.finish(operation, Err(NativeErrorCode::Shutdown), true);
        }
        while self.pending_receipt.is_none() {
            let Ok(operation) = self.operations.pop() else { break };
            let error = if operation.guard.0.load(Ordering::Acquire) == CANCELLED {
                NativeErrorCode::Cancelled
            } else { NativeErrorCode::Shutdown };
            self.finish(operation, Err(error), false);
        }
        self.counters.structural_transition_active.store(false, Ordering::Release);
        self.telemetry.publish(self.engine.revision(), &self.engine, &self.counters, &mut self.device_scratch);
    }
}

pub struct PipeWireOutput {
    stream: Option<Stream>,
    worker: Option<std::thread::JoinHandle<()>>,
    retirement: Option<CallbackRetirement>,
    shutdown: Arc<AtomicBool>,
    device: String,
    rate: u32,
    channels: u16,
    requested_period: Option<u32>,
    counters: Arc<CallbackCounters>,
    telemetry: Arc<RuntimeTelemetry>,
    operation_producer: Producer<OutputOperation>,
    receipt_consumer: Consumer<OutputReceipt>,
    controls_in_flight: usize,
    revision_in_flight: bool,
    observation: Option<ObservationConsumer>,
}

impl PipeWireOutput {
    pub fn start(session: &model::Session, start_playing: bool) -> Result<Self, PipeWireError> {
        Self::start_backend(session, start_playing, false)
    }
    pub fn start_backend(
        session: &model::Session,
        start_playing: bool,
        headless: bool,
    ) -> Result<Self, PipeWireError> {
        let (device, device_name, selected) = if headless {
            (
                None,
                "silent clock".to_owned(),
                SelectedConfig {
                    config: StreamConfig {
                        channels: 2,
                        sample_rate: 48000,
                        buffer_size: BufferSize::Fixed(256),
                    },
                    requested_period: Some(256),
                },
            )
        } else {
            #[cfg(target_os = "linux")]
            let host = cpal::host_from_id(cpal::HostId::PipeWire).map_err(PipeWireError::Host)?;
            #[cfg(not(target_os = "linux"))]
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or(PipeWireError::NoDefaultOutputDevice)?;
            let selected = select_supported_config(
                device
                    .supported_output_configs()
                    .map_err(PipeWireError::OutputConfigurations)?,
            )
            .ok_or(PipeWireError::NoSupportedOutputConfiguration)?;
            let name = device.to_string();
            (Some(device), name, selected)
        };
        let mut engine = AudioEngine::new(
            session,
            AudioConfig {
                sample_rate: selected.config.sample_rate as f32,
                max_frames: MAX_AUDIO_FRAMES,
                offline: false,
            },
        )?;
        engine.set_running(start_playing);

        let counters = Arc::new(CallbackCounters::default());
        let telemetry = Arc::new(RuntimeTelemetry::new(engine.graph_budget()));
        let mut device_scratch = vec![EMPTY_DEVICE_DEBUG_STATE; engine.graph_budget()];
        telemetry.publish(0, &engine, &counters, &mut device_scratch);
        let data_counters = Arc::clone(&counters);
        let error_counters = Arc::clone(&counters);
        let data_telemetry = Arc::clone(&telemetry);
        let (operation_producer, operation_consumer) = RingBuffer::new(OUTCOME_CAPACITY);
        let (receipt_producer, receipt_consumer) = RingBuffer::new(OUTCOME_CAPACITY);
        let channels = usize::from(selected.config.channels);
        let mut processor = CallbackProcessor {
            engine, operations: operation_consumer, receipts: receipt_producer,
            pending: None, pending_receipt: None, generation: 0,
            counters: data_counters, telemetry: data_telemetry, device_scratch,
        };
        let render = move |output: Option<&mut [f32]>| {
            if let Some(output) = output {
                processor.process(output, channels);
            } else {
                processor.quiesce();
            }
        };
        let (sender, receiver) = sync_channel(1);
        let retirement = CallbackRetirement(receiver);
        let mut callback = ReturningCallback {
            callback: Some(Box::new(render)),
            sender,
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let (stream, worker) = if let Some(device) = device {
            let stream = device
                .build_output_stream::<f32, _, _>(
                    selected.config,
                    move |output, _| callback.process(output),
                    move |error| {
                        error_counters.stream_errors.fetch_add(1, Ordering::Relaxed);
                        // CPAL owns the error value. Avoid a possible String deallocation on its
                        // real-time callback; stream errors are exceptional and bounded by stream life.
                        drop(error);
                    },
                    None,
                )
                .map_err(PipeWireError::BuildStream)?;

            let negotiated_period = stream.buffer_size().map_err(PipeWireError::QueryPeriod)?;
            if negotiated_period == 0 || negotiated_period as usize > MAX_AUDIO_FRAMES {
                return Err(PipeWireError::UnsupportedNegotiatedPeriod {
                    period: negotiated_period,
                    maximum: MAX_AUDIO_FRAMES,
                });
            }
            stream.play().map_err(PipeWireError::PlayStream)?;

            (Some(stream), None)
        } else {
            let stop = Arc::clone(&shutdown);
            let worker = std::thread::spawn(move || {
                let mut output = vec![0.0; 256 * channels];
                let period = std::time::Duration::from_secs_f64(256.0 / 48000.0);
                let mut next = Instant::now();
                while !stop.load(Ordering::Acquire) {
                    callback.process(&mut output);
                    next += period;
                    if let Some(wait) = next.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    } else {
                        next = Instant::now();
                    }
                }
            });
            (None, Some(worker))
        };

        Ok(Self {
            stream,
            worker,
            retirement: Some(retirement),
            shutdown,
            device: device_name,
            rate: selected.config.sample_rate,
            channels: selected.config.channels,
            requested_period: selected.requested_period,
            counters,
            telemetry,
            operation_producer,
            receipt_consumer,
            controls_in_flight: 0,
            revision_in_flight: false,
            observation: None,
        })
    }

    pub fn status(&self) -> PipeWireStatus {
        let negotiated_period = self
            .stream
            .as_ref()
            .and_then(|s| s.buffer_size().ok())
            .or(Some(256));
        PipeWireStatus {
            host: if self.stream.is_some() {
                "PipeWire"
            } else {
                "headless"
            },
            device: self.device.clone(),
            rate: self.rate,
            channels: self.channels,
            requested_period: self.requested_period,
            negotiated_period,
            stream_generation: STREAM_GENERATION,
            callback_count: self.counters.callback_count.load(Ordering::Relaxed),
            render_faults: self.counters.render_faults.load(Ordering::Relaxed),
            stream_errors: self.counters.stream_errors.load(Ordering::Relaxed),
            transaction_commits: self.counters.transaction_commits.load(Ordering::Relaxed),
            transaction_faults: self.counters.transaction_faults.load(Ordering::Relaxed),
            structural_commits: self.counters.structural_commits.load(Ordering::Relaxed),
            structural_transition_active: self
                .counters
                .structural_transition_active
                .load(Ordering::Acquire),
        }
    }
    pub fn runtime_snapshot(&self) -> RuntimeTelemetrySnapshot {
        self.telemetry.snapshot()
    }

    /// Admission reserves the reliable receipt and ownership-return slot together.
    pub(crate) fn submit_operation(&mut self, operation: OutputOperation) -> Result<(), OutputOperation> {
        if self.shutdown.load(Ordering::Acquire)
            || (operation.is_revision() && self.revision_in_flight)
            || (!operation.is_revision() && self.controls_in_flight == TRANSPORT_COMMAND_CAPACITY)
        {
            return Err(operation);
        }
        let revision = operation.is_revision();
        match self.operation_producer.push(operation) {
            Ok(()) => {
                if revision { self.revision_in_flight = true; }
                else { self.controls_in_flight += 1; }
                Ok(())
            }
            Err(PushError::Full(operation)) => Err(operation),
        }
    }

    pub(crate) fn submit_audibility(&mut self, operation: OutputOperation) -> Result<(), OutputOperation> {
        self.submit_operation(operation)
    }

    pub(crate) fn set_observation(&mut self, operation: OutputOperation) -> Result<(), OutputOperation> {
        self.submit_operation(operation)
    }

    pub(crate) fn observation_consumer(&self) -> Option<&ObservationConsumer> {
        self.observation.as_ref()
    }

    pub(crate) fn drain_observations(&mut self, output: &mut [Option<ObservationFrame>]) -> DrainCount {
        match &mut self.observation {
            Some(consumer) => consumer.drain(output),
            None => DrainCount { written: 0, remaining: false },
        }
    }

    pub(crate) fn observation_dropped(&self) -> u64 {
        self.observation.as_ref().map_or(0, ObservationConsumer::dropped)
    }

    pub(crate) fn poll_operation_receipt(&mut self) -> Option<OutputReceipt> {
        let mut receipt = self.receipt_consumer.pop().ok()?;
        if receipt.operation.is_revision() { self.revision_in_flight = false; }
        else { self.controls_in_flight -= 1; }
        if receipt.result.is_ok() {
            if let Some(consumer) = receipt.operation.observation.take() {
                self.observation = Some(consumer);
            } else if matches!(&receipt.operation.payload,
                OutputPayload::Revision { .. } |
                OutputPayload::Control {
                    command: OutputControl::Observation(_) | OutputControl::PreparedTransport { .. }, ..
                })
            {
                // Keep the inactive pool's shared counters for a later continuing
                // selection, but never expose retired-engine capture.
                if let Some(consumer) = &mut self.observation { consumer.flush(); }
            }
        }
        Some(receipt)
    }

    pub(crate) fn transport_status(&self) -> NativeTransportStatus {
        let snapshot = self.runtime_snapshot();
        NativeTransportStatus {
            generation: snapshot.transport_generation,
            snapshot: snapshot.transport,
            loop_range: snapshot.loop_range,
            end_tick: snapshot.end_tick,
            end_project_frame: snapshot.end_project_frame,
        }
    }

    /// Stop/join before returning callback-owned objects to this coordinator.
    pub(crate) fn quiesce(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        drop(self.stream.take());
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
        drop(self.retirement.take());
    }
    pub fn audio_config(&self) -> AudioConfig {
        AudioConfig {
            sample_rate: self.rate as f32,
            max_frames: MAX_AUDIO_FRAMES,
            offline: false,
        }
    }

}

fn finite_sample(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

#[derive(Clone, Copy)]
struct CommitMetadata {
    failure: Option<TransactionApplyError>,
}

fn commit_transaction(
    engine: &mut AudioEngine,
    transaction: &mut PreparedTransaction,
    counters: &CallbackCounters,
) -> CommitMetadata {
    let structural = transaction.is_structural();
    let failure = engine.apply_transaction(transaction).err();
    if failure.is_none() {
        counters.transaction_commits.fetch_add(1, Ordering::Relaxed);
        if structural {
            counters.structural_commits.fetch_add(1, Ordering::Relaxed);
        }
    } else {
        counters.transaction_faults.fetch_add(1, Ordering::Relaxed);
    }
    CommitMetadata {
        failure,
    }
}

fn render_block(
    engine: &mut AudioEngine,
    output: &mut [f32],
    channels: usize,
    fade: Option<EngineFade>,
    counters: &CallbackCounters,
) {
    if engine
        .render_interleaved_with_fade(output, channels, fade)
        .is_err()
    {
        output.fill(0.0);
        counters.render_faults.fetch_add(1, Ordering::Relaxed);
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectedConfig {
    config: StreamConfig,
    requested_period: Option<u32>,
}

fn select_supported_config(
    configs: impl IntoIterator<Item = SupportedStreamConfigRange>,
) -> Option<SelectedConfig> {
    configs
        .into_iter()
        .filter_map(candidate_from_range)
        .min_by_key(selection_key)
}

fn candidate_from_range(range: SupportedStreamConfigRange) -> Option<SelectedConfig> {
    if range.sample_format() != SampleFormat::F32
        || range.channels() < 2
        || range.min_sample_rate() == 0
        || range.min_sample_rate() > range.max_sample_rate()
    {
        return None;
    }

    let sample_rate = PREFERRED_SAMPLE_RATE.clamp(range.min_sample_rate(), range.max_sample_rate());
    let (buffer_size, requested_period) = select_period(*range.buffer_size())?;
    Some(SelectedConfig {
        config: StreamConfig {
            channels: range.channels(),
            sample_rate,
            buffer_size,
        },
        requested_period,
    })
}

fn select_period(supported: SupportedBufferSize) -> Option<(BufferSize, Option<u32>)> {
    match supported {
        SupportedBufferSize::Range { min, max } if min > 0 && min <= max => {
            let period = PREFERRED_PERIOD.clamp(min, max);
            (period as usize <= MAX_AUDIO_FRAMES)
                .then_some((BufferSize::Fixed(period), Some(period)))
        }
        SupportedBufferSize::Unknown => Some((BufferSize::Default, None)),
        SupportedBufferSize::Range { .. } => None,
    }
}

fn selection_key(candidate: &SelectedConfig) -> (bool, u32, bool, u16, u32) {
    let rate_distance = candidate.config.sample_rate.abs_diff(PREFERRED_SAMPLE_RATE);
    (
        candidate.config.sample_rate != PREFERRED_SAMPLE_RATE,
        rate_distance,
        candidate.config.channels != 2,
        candidate.config.channels,
        candidate.requested_period.unwrap_or(u32::MAX),
    )
}

impl Drop for PipeWireOutput {
    fn drop(&mut self) {
        self.quiesce();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::native::NativeOperationId;
    use super::super::observation::tests::allocation_activity;

    fn session(source: &str) -> model::Session {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("callback.muz");
        std::fs::write(&path, source).unwrap();
        crate::compile::compile(&path).unwrap().session
    }

    pub(crate) fn harness(session: &model::Session) -> (PipeWireOutput, CallbackProcessor) {
        let mut engine = AudioEngine::new(session, AudioConfig {
            sample_rate: 48_000., max_frames: MAX_AUDIO_FRAMES, offline: false,
        }).unwrap();
        engine.set_running(true);
        engine.render_interleaved(&mut [0.; 512], 2).unwrap();
        let counters = Arc::new(CallbackCounters::default());
        let telemetry = Arc::new(RuntimeTelemetry::new(engine.graph_budget()));
        let mut device_scratch = vec![EMPTY_DEVICE_DEBUG_STATE; engine.graph_budget()];
        telemetry.publish(0, &engine, &counters, &mut device_scratch);
        let (operation_producer, operations) = RingBuffer::new(OUTCOME_CAPACITY);
        let (receipts, receipt_consumer) = RingBuffer::new(OUTCOME_CAPACITY);
        let output = PipeWireOutput {
            stream: None, worker: None, retirement: None,
            shutdown: Arc::new(AtomicBool::new(false)),
            device: "deterministic callback".into(), rate: 48_000, channels: 2,
            requested_period: Some(256), counters: counters.clone(), telemetry: telemetry.clone(),
            operation_producer, receipt_consumer, controls_in_flight: 0,
            revision_in_flight: false, observation: None,
        };
        let processor = CallbackProcessor {
            engine, operations, receipts, pending: None, pending_receipt: None,
            generation: 0, counters, telemetry, device_scratch,
        };
        (output, processor)
    }

    fn control(id: u64, revision: u64, command: OutputControl) -> OutputOperation {
        OutputOperation {
            guard: OperationGuard::new(),
            payload: OutputPayload::Control {
                ticket: ControlTicket { operation: NativeOperationId(id), expected_revision: revision },
                command,
            },
            observation: None,
        }
    }

    fn revision(id: u64, current: &model::Session, candidate: &model::Session) -> OutputOperation {
        let plan = crate::reconcile::plan_reconciliation(0, current, candidate).unwrap();
        let transaction = PreparedTransaction::prepare(current, candidate, &plan, 9, None,
            AudioConfig { sample_rate: 48_000., max_frames: MAX_AUDIO_FRAMES, offline: false }).unwrap();
        OutputOperation {
            guard: OperationGuard::new(),
            payload: OutputPayload::Revision {
                ticket: RevisionTicket { operation: NativeOperationId(id), base_revision: 0,
                    revision: 1, source_generation: 9 },
                transaction: Box::new(transaction),
            },
            observation: None,
        }
    }

    #[test]
    fn cancelled_and_stale_controls_never_mutate_and_keep_fifo_receipts() {
        let source = session("song({tracks:[track(\"lane\",note(60),synth(\"init\"))]})");
        let (mut output, mut callback) = harness(&source);
        let cancelled = control(1, 0, OutputControl::Running(false));
        let guard = cancelled.guard.clone();
        assert!(output.submit_operation(cancelled).is_ok());
        assert_eq!(guard.cancel(), CancelDisposition::Prevented);
        assert!(output.submit_operation(control(2, 1, OutputControl::Seek(1_000_000))).is_ok());
        assert!(output.submit_operation(control(3, 0, OutputControl::Running(false))).is_ok());
        let (_, allocations, frees) = allocation_activity(|| callback.process(&mut [0.; 512], 2));
        assert_eq!((allocations, frees), (0, 0));
        let first = output.poll_operation_receipt().unwrap();
        assert_eq!(first.result, Err(NativeErrorCode::Cancelled));
        assert!(first.transport.snapshot.running);
        let second = output.poll_operation_receipt().unwrap();
        assert_eq!(second.result, Err(NativeErrorCode::StaleRevision));
        assert_eq!(second.transport.generation, 0);
        let third = output.poll_operation_receipt().unwrap();
        assert_eq!(third.result, Ok(()));
        assert!(!third.transport.snapshot.running);
        assert_eq!(third.transport.generation, 1);
        assert_eq!(guard.cancel(), CancelDisposition::AlreadyResolved);
        assert!(output.poll_operation_receipt().is_none());
    }

    #[test]
    fn outcomes_reserve_all_controls_and_revision_without_callback_destruction() {
        let source = session("song({tracks:[track(\"lane\",note(60),synth(\"init\"))]})");
        let (mut output, mut callback) = harness(&source);
        for id in 1..=64 {
            assert!(output.submit_operation(control(id, 0, OutputControl::Running(true))).is_ok());
        }
        let refused = output.submit_operation(control(65, 0, OutputControl::Panic)).err().unwrap();
        assert!(refused.guard.is_pending());
        assert!(output.submit_operation(revision(66, &source, &source)).is_ok());
        let (_, allocations, frees) = allocation_activity(|| {
            for _ in 0..12 { callback.process(&mut [0.; 512], 2); }
        });
        assert_eq!((allocations, frees), (0, 0));
        assert!(output.submit_operation(refused).is_err());
        for id in 1..=64 {
            let receipt = output.poll_operation_receipt().unwrap();
            assert_eq!(receipt.result, Ok(()));
            assert!(matches!(receipt.operation.payload, OutputPayload::Control { ticket, .. }
                if ticket.operation == NativeOperationId(id)));
        }
        let receipt = output.poll_operation_receipt().unwrap();
        assert_eq!(receipt.result, Ok(()));
        assert!(matches!(receipt.operation.payload, OutputPayload::Revision { ticket, .. }
            if ticket.operation == NativeOperationId(66)));
        assert!(output.poll_operation_receipt().is_none());
    }

    #[test]
    fn queued_capture_is_nondestructive_and_audibility_retires_without_allocations() {
        use crate::native::{ObservationSelection, ObservationTarget};
        let source = session("song({tracks:[track(\"lane\",note(60),synth(\"init\"))]})");
        let (mut output, mut callback) = harness(&source);
        let (_reference_output, mut reference) = harness(&source);
        let track = source.tracks[0].id.as_str().to_owned();
        let selection = ObservationSelection {
            meter_tracks: vec![track.clone()], master_meter: true,
            analysis: Some(ObservationTarget::Track(track)), meter_hz: 20, analysis_hz: 10,
        };
        let (prepared, consumer) = PreparedObservation::new(&source, output.audio_config(), &selection).unwrap();
        let mut operation = control(1, 0, OutputControl::Observation(Some(prepared)));
        operation.observation = Some(consumer);
        assert!(output.set_observation(operation).is_ok());
        let mut actual = [0.; 512];
        let mut expected = [0.; 512];
        let (_, allocations, frees) = allocation_activity(|| callback.process(&mut actual, 2));
        assert_eq!((allocations, frees), (0, 0));
        reference.process(&mut expected, 2);
        assert_eq!(actual, expected);
        assert_eq!(output.poll_operation_receipt().unwrap().result, Ok(()));
        let mut frames = [None];
        assert_eq!(output.drain_observations(&mut frames).written, 1);
        let frame = frames[0].take().unwrap();
        assert_eq!(frame.revision, 0);
        assert_eq!(frame.transport_generation, 0);
        assert!(frame.mono_samples.is_empty(), "detail must warm a full rolling window");
        assert_eq!(frame.sample_count, 0);
        assert!(frame.meters_present);

        let mask = PreparedAudibility::new(&source, &[]).unwrap();
        assert!(output.submit_audibility(control(2, 0, OutputControl::Audibility(mask))).is_ok());
        let (_, allocations, frees) = allocation_activity(|| {
            callback.process(&mut actual, 2);
            callback.process(&mut actual, 2);
        });
        assert_eq!((allocations, frees), (0, 0));
        assert!(actual.iter().all(|sample| *sample == 0.0));
        let receipt = output.poll_operation_receipt().unwrap();
        assert_eq!(receipt.result, Ok(()));
        assert_eq!(receipt.transport.generation, 0);
        let (_, allocations, frees) = allocation_activity(|| {
            for _ in 0..100 { callback.process(&mut actual, 2); }
        });
        assert_eq!((allocations, frees), (0, 0));
        assert!(output.observation_dropped() > 0);
    }

    #[test]
    fn structural_cancellation_arbitrates_before_fade_not_after_it() {
        let source = session("song({tracks:[track(\"lane\",note(60),synth(\"init\"))]})");
        let candidate = session("song({tracks:[track(\"lane\",note(60),synth(\"init\")),track(\"new\",note(64),synth(\"init\"))]})");
        let (mut output, mut callback) = harness(&source);
        let operation = revision(1, &source, &candidate);
        let cancelled = operation.guard.clone();
        assert_eq!(cancelled.cancel(), CancelDisposition::Prevented);
        assert!(output.submit_operation(operation).is_ok());
        callback.process(&mut [0.; 512], 2);
        let receipt = output.poll_operation_receipt().unwrap();
        assert_eq!(receipt.result, Err(NativeErrorCode::Cancelled));
        assert!(!receipt.faded);
        assert_eq!(callback.engine.revision(), 0);

        let operation = revision(2, &source, &candidate);
        let applying = operation.guard.clone();
        assert!(output.submit_operation(operation).is_ok());
        callback.process(&mut [0.; 512], 2);
        assert_eq!(applying.cancel(), CancelDisposition::PendingOutcome);
        assert!(output.poll_operation_receipt().is_none());
        assert!(output.submit_operation(control(3, 0, OutputControl::Panic)).is_ok());
        let (_, allocations, frees) = allocation_activity(|| callback.process(&mut [0.; 512], 2));
        assert_eq!((allocations, frees), (0, 0));
        let receipt = output.poll_operation_receipt().unwrap();
        assert_eq!(receipt.result, Ok(()));
        assert!(receipt.faded);
        assert_eq!(callback.engine.revision(), 1);
        callback.process(&mut [0.; 512], 2);
        assert_eq!(output.poll_operation_receipt().unwrap().result, Err(NativeErrorCode::StaleRevision));
    }

    #[test]
    fn scoped_cancellation_is_checked_before_begin_and_not_after_applying() {
        let source = session("song({tracks:[track(\"lane\",note(60),synth(\"init\"))]})");
        let candidate = session("song({tracks:[track(\"other\",note(64),synth(\"init\"))]})");
        let (mut output, mut callback) = harness(&source);
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut operation = revision(1, &source, &candidate);
        operation.guard = OperationGuard::with_cancellation(Some(cancellation.clone()));
        assert!(output.submit_operation(operation).is_ok());
        cancellation.store(true, Ordering::Release);
        let (_, allocations, frees) = allocation_activity(|| callback.process(&mut [0.; 512], 2));
        assert_eq!((allocations, frees), (0, 0));
        let receipt = output.poll_operation_receipt().unwrap();
        assert_eq!(receipt.result, Err(NativeErrorCode::Cancelled));
        assert!(!receipt.faded);
        assert_eq!(callback.engine.revision(), 0);
        drop(receipt);

        cancellation.store(false, Ordering::Release);
        let mut operation = revision(2, &source, &candidate);
        operation.guard = OperationGuard::with_cancellation(Some(cancellation.clone()));
        let guard = operation.guard.clone();
        assert!(output.submit_operation(operation).is_ok());
        callback.process(&mut [0.; 512], 2);
        cancellation.store(true, Ordering::Release);
        assert_eq!(guard.cancel(), CancelDisposition::PendingOutcome);
        let (_, allocations, frees) = allocation_activity(|| callback.process(&mut [0.; 512], 2));
        assert_eq!((allocations, frees), (0, 0));
        let receipt = output.poll_operation_receipt().unwrap();
        assert_eq!(receipt.result, Ok(()));
        assert!(receipt.faded);
        assert_eq!(callback.engine.revision(), 1);
        drop(receipt);

        let mut operation = control(3, 1, OutputControl::Running(false));
        operation.guard = OperationGuard::with_cancellation(Some(cancellation));
        assert!(output.submit_operation(operation).is_ok());
        callback.process(&mut [0.; 512], 2);
        let receipt = output.poll_operation_receipt().unwrap();
        assert_eq!(receipt.result, Err(NativeErrorCode::Cancelled));
        assert!(receipt.transport.snapshot.running);
        assert!(output.poll_operation_receipt().is_none());
    }

    #[test]
    fn signature_guard_rejects_structural_work_before_fade() {
        let source = session("song({tracks:[track(\"lane\",note(60),synth(\"init\"))]})");
        let candidate = session("song({tracks:[track(\"other\",note(64),synth(\"init\"))]})");
        let (mut output, mut callback) = harness(&candidate);
        assert!(output.submit_operation(revision(1, &source, &candidate)).is_ok());
        let (_, allocations, frees) = allocation_activity(|| callback.process(&mut [0.; 512], 2));
        assert_eq!((allocations, frees), (0, 0));
        let receipt = output.poll_operation_receipt().unwrap();
        assert_eq!(receipt.result, Err(NativeErrorCode::StaleRevision));
        assert!(!receipt.faded);
        assert!(callback.pending.is_none());
        assert_eq!(callback.engine.revision(), 0);
    }

    #[test]
    fn shutdown_returns_pending_and_fading_operations_once() {
        let source = session("song({tracks:[track(\"lane\",note(60),synth(\"init\"))]})");
        let candidate = session("song({tracks:[track(\"other\",note(64),synth(\"init\"))]})");
        let (mut output, mut callback) = harness(&source);
        assert!(output.submit_operation(revision(1, &source, &candidate)).is_ok());
        callback.process(&mut [0.; 512], 2);
        assert!(output.submit_operation(control(2, 0, OutputControl::Panic)).is_ok());
        callback.quiesce();
        assert_eq!(output.poll_operation_receipt().unwrap().result, Err(NativeErrorCode::Shutdown));
        assert_eq!(output.poll_operation_receipt().unwrap().result, Err(NativeErrorCode::Shutdown));
        callback.quiesce();
        assert!(output.poll_operation_receipt().is_none());
    }

    #[test]
    fn telemetry_publishes_every_device_above_the_former_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.muz");
        let chain = vec!["fx(\"gain\")"; 129].join(",");
        std::fs::write(
            &path,
            format!(
                "song({{tracks:[track(\"lane\",note(60),synth(\"init\"),{{chain:[{chain}]}})]}})"
            ),
        )
        .unwrap();
        let session = crate::compile::compile(&path).unwrap().session;
        let mut engine = AudioEngine::new(
            &session,
            AudioConfig {
                sample_rate: 48000.,
                max_frames: 256,
                offline: false,
            },
        )
        .unwrap();
        engine.render_interleaved(&mut [0.; 512], 2).unwrap();
        let telemetry = RuntimeTelemetry::new(engine.graph_budget());
        let mut scratch = vec![EMPTY_DEVICE_DEBUG_STATE; engine.graph_budget()];
        telemetry.publish(7, &engine, &CallbackCounters::default(), &mut scratch);
        let snapshot = telemetry.snapshot();
        let expected = engine.device_debug_states();
        assert_eq!(snapshot.runtime_revision, 7);
        assert_eq!(snapshot.devices.len(), 130);
        for (actual, (_, expected)) in snapshot.devices.iter().zip(expected) {
            assert_eq!(actual.instance_token, expected.instance_token);
            assert_eq!(actual.process_count, expected.process_count);
        }
    }

    #[test]
    fn callback_resources_return_to_the_preparation_thread() {
        struct Probe(std::thread::ThreadId, Arc<AtomicBool>);
        impl Drop for Probe {
            fn drop(&mut self) {
                assert_eq!(std::thread::current().id(), self.0);
                self.1.store(true, Ordering::Release);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let probe = Probe(std::thread::current().id(), dropped.clone());
        let (sender, receiver) = sync_channel(1);
        let retirement = CallbackRetirement(receiver);
        let mut callback = ReturningCallback {
            callback: Some(Box::new(move |_| {
                let _ = &probe;
            })),
            sender,
        };
        std::thread::spawn(move || callback.process(&mut []))
            .join()
            .unwrap();
        assert!(!dropped.load(Ordering::Acquire));
        drop(retirement);
        assert!(dropped.load(Ordering::Acquire));
    }
}
