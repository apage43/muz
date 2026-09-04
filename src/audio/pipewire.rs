use std::{
    error::Error,
    fmt,
    hint::spin_loop,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering, fence},
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
        TransactionApplyError, TransactionReceipt, TransportSnapshot,
        engine::{AudioEngine, EngineError, EngineFade},
    },
    model,
};

const PREFERRED_SAMPLE_RATE: u32 = 48_000;
const PREFERRED_PERIOD: u32 = 256;
const STREAM_GENERATION: u64 = 1;
const TRANSPORT_COMMAND_CAPACITY: usize = 64;

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

pub struct TransactionQueueFull(Box<PreparedTransaction>);

impl fmt::Debug for TransactionQueueFull {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TransactionQueueFull(..)")
    }
}

impl TransactionQueueFull {
    pub fn into_transaction(self) -> Box<PreparedTransaction> {
        self.0
    }
}

impl fmt::Display for TransactionQueueFull {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an audio transaction is already in flight")
    }
}

impl Error for TransactionQueueFull {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransportCommand {
    Play,
    Stop,
    Restart,
    SeekTicks(u64),
    Loop(Option<(u64, u64)>),
    Panic,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("transport command queue is full")]
pub struct TransportCommandQueueFull;

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
    is_plugin: false,
};

struct RuntimeTelemetry {
    sequence: AtomicU64,
    runtime_revision: AtomicU64,
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
    device_tokens: [AtomicU64; model::MAX_DEVICES],
    device_process_counts: [AtomicU64; model::MAX_DEVICES],
    device_gain_reduction_bits: [AtomicU64; model::MAX_DEVICES],
    device_latency_samples: [AtomicU64; model::MAX_DEVICES],
    device_tail_samples: [AtomicU64; model::MAX_DEVICES],
    device_flags: [AtomicU64; model::MAX_DEVICES],
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

impl Default for RuntimeTelemetry {
    fn default() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            runtime_revision: AtomicU64::new(0),
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
            device_tokens: std::array::from_fn(|_| AtomicU64::new(0)),
            device_process_counts: std::array::from_fn(|_| AtomicU64::new(0)),
            device_gain_reduction_bits: std::array::from_fn(|_| AtomicU64::new(0)),
            device_latency_samples: std::array::from_fn(|_| AtomicU64::new(0)),
            device_tail_samples: std::array::from_fn(|_| AtomicU64::new(0)),
            device_flags: std::array::from_fn(|_| AtomicU64::new(0)),
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
    fn publish(&self, runtime_revision: u64, engine: &AudioEngine, counters: &CallbackCounters) {
        let status = engine.status();
        let mut devices = [EMPTY_DEVICE_DEBUG_STATE; model::MAX_DEVICES];
        let device_count = engine.copy_device_debug_states(&mut devices);

        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.runtime_revision
            .store(runtime_revision, Ordering::Relaxed);
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
            self.device_flags[index].store(u64::from(state.is_plugin), Ordering::Relaxed);
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
        let mut devices = [EMPTY_DEVICE_DEBUG_STATE; model::MAX_DEVICES];
        loop {
            let sequence = self.sequence.load(Ordering::Acquire);
            if sequence & 1 != 0 {
                spin_loop();
                continue;
            }

            let runtime_revision = self.runtime_revision.load(Ordering::Relaxed);
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
                (self.device_count.load(Ordering::Relaxed) as usize).min(model::MAX_DEVICES);
            for (index, state) in devices[..device_count].iter_mut().enumerate() {
                state.instance_token = self.device_tokens[index].load(Ordering::Relaxed);
                state.process_count = self.device_process_counts[index].load(Ordering::Relaxed);
                state.gain_reduction_db = f32::from_bits(
                    self.device_gain_reduction_bits[index].load(Ordering::Relaxed) as u32,
                );
                state.latency_samples =
                    self.device_latency_samples[index].load(Ordering::Relaxed) as u32;
                state.tail_samples = self.device_tail_samples[index].load(Ordering::Relaxed) as u32;
                state.is_plugin = self.device_flags[index].load(Ordering::Relaxed) != 0;
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

pub struct PipeWireOutput {
    stream: Option<Stream>,
    worker: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    device: String,
    rate: u32,
    channels: u16,
    requested_period: Option<u32>,
    counters: Arc<CallbackCounters>,
    telemetry: Arc<RuntimeTelemetry>,
    transport_command_producer: Producer<TransportCommand>,
    transaction_producer: Producer<Box<PreparedTransaction>>,
    receipt_consumer: Consumer<TransactionReceipt>,
    transaction_in_flight: Arc<AtomicBool>,
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
            let host = cpal::host_from_id(cpal::HostId::PipeWire).map_err(PipeWireError::Host)?;
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
            },
        )?;
        engine.set_running(start_playing);

        let counters = Arc::new(CallbackCounters::default());
        let telemetry = Arc::new(RuntimeTelemetry::default());
        telemetry.publish(0, &engine, &counters);
        let data_counters = Arc::clone(&counters);
        let error_counters = Arc::clone(&counters);
        let data_telemetry = Arc::clone(&telemetry);
        let (transport_command_producer, mut transport_command_consumer) =
            RingBuffer::<TransportCommand>::new(TRANSPORT_COMMAND_CAPACITY);
        let transaction_in_flight = Arc::new(AtomicBool::new(false));
        let (transaction_producer, mut transaction_consumer) =
            RingBuffer::<Box<PreparedTransaction>>::new(1);
        let (mut receipt_producer, receipt_consumer) = RingBuffer::<TransactionReceipt>::new(1);

        let mut pending_receipt = None;
        let mut pending_switch: Option<Box<PreparedTransaction>> = None;
        let mut runtime_revision = 0;
        let channels = usize::from(selected.config.channels);
        let mut callback = move |output: &mut [f32]| {
            apply_transport_commands(&mut engine, &mut transport_command_consumer);
            let callback_count = data_counters
                .callback_count
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);

            if let Some(receipt) = pending_receipt.take() {
                if let Err(PushError::Full(receipt)) = receipt_producer.push(receipt) {
                    pending_receipt = Some(receipt);
                }
            }

            let mut rendered = false;
            if pending_receipt.is_none() {
                if let Some(mut transaction) = pending_switch.take() {
                    let commit = commit_transaction(&mut engine, &mut transaction, &data_counters);
                    if commit.failure.is_none() {
                        runtime_revision = transaction.revision();
                    }
                    let fade = if commit.failure.is_none() {
                        transaction.fade_in()
                    } else {
                        transaction.fade_recovery()
                    };
                    render_block(&mut engine, output, channels, fade, &data_counters);
                    rendered = true;
                    data_counters
                        .structural_transition_active
                        .store(false, Ordering::Release);
                    let receipt = make_receipt(transaction, commit, callback_count, &data_counters);
                    if let Err(PushError::Full(receipt)) = receipt_producer.push(receipt) {
                        pending_receipt = Some(receipt);
                    }
                } else if let Ok(mut transaction) = transaction_consumer.pop() {
                    if transaction.needs_fade() {
                        render_block(
                            &mut engine,
                            output,
                            channels,
                            transaction.fade_out(),
                            &data_counters,
                        );
                        rendered = true;
                        data_counters
                            .structural_transition_active
                            .store(true, Ordering::Release);
                        pending_switch = Some(transaction);
                    } else {
                        let commit =
                            commit_transaction(&mut engine, &mut transaction, &data_counters);
                        if commit.failure.is_none() {
                            runtime_revision = transaction.revision();
                        }
                        render_block(&mut engine, output, channels, None, &data_counters);
                        rendered = true;
                        let receipt =
                            make_receipt(transaction, commit, callback_count, &data_counters);
                        if let Err(PushError::Full(receipt)) = receipt_producer.push(receipt) {
                            pending_receipt = Some(receipt);
                        }
                    }
                }
            }

            if !rendered {
                render_block(&mut engine, output, channels, None, &data_counters);
            }
            data_telemetry.publish(runtime_revision, &engine, &data_counters);
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let (stream, worker) = if let Some(device) = device {
            let stream = device
                .build_output_stream::<f32, _, _>(
                    selected.config,
                    move |output, _| callback(output),
                    move |error| {
                        error_counters.stream_errors.fetch_add(1, Ordering::Relaxed);
                        // CPAL owns the error value. Avoid a possible String deallocation on its
                        // real-time callback; stream errors are exceptional and bounded by stream life.
                        std::mem::forget(error);
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
                    callback(&mut output);
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
            shutdown,
            device: device_name,
            rate: selected.config.sample_rate,
            channels: selected.config.channels,
            requested_period: selected.requested_period,
            counters,
            telemetry,
            transport_command_producer,
            transaction_producer,
            receipt_consumer,
            transaction_in_flight,
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

    pub fn set_running(&mut self, running: bool) -> Result<(), TransportCommandQueueFull> {
        self.submit_transport_command(if running {
            TransportCommand::Play
        } else {
            TransportCommand::Stop
        })
    }

    pub fn restart(&mut self) -> Result<(), TransportCommandQueueFull> {
        self.submit_transport_command(TransportCommand::Restart)
    }

    pub fn seek_ticks(&mut self, tick: u64) -> Result<(), TransportCommandQueueFull> {
        self.submit_transport_command(TransportCommand::SeekTicks(tick))
    }

    pub fn set_loop(&mut self, range: Option<(u64, u64)>) -> Result<(), TransportCommandQueueFull> {
        self.submit_transport_command(TransportCommand::Loop(range))
    }
    pub fn panic(&mut self) -> Result<(), TransportCommandQueueFull> {
        self.submit_transport_command(TransportCommand::Panic)
    }
    fn submit_transport_command(
        &mut self,
        command: TransportCommand,
    ) -> Result<(), TransportCommandQueueFull> {
        self.transport_command_producer
            .push(command)
            .map_err(|PushError::Full(_)| TransportCommandQueueFull)
    }

    pub fn audio_config(&self) -> AudioConfig {
        AudioConfig {
            sample_rate: self.rate as f32,
            max_frames: MAX_AUDIO_FRAMES,
        }
    }

    pub fn submit_transaction(
        &mut self,
        transaction: Box<PreparedTransaction>,
    ) -> Result<(), TransactionQueueFull> {
        if self
            .transaction_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(TransactionQueueFull(transaction));
        }
        match self.transaction_producer.push(transaction) {
            Ok(()) => Ok(()),
            Err(PushError::Full(transaction)) => {
                self.transaction_in_flight.store(false, Ordering::Release);
                Err(TransactionQueueFull(transaction))
            }
        }
    }

    pub fn poll_transaction_receipt(&mut self) -> Option<TransactionReceipt> {
        let receipt = self.receipt_consumer.pop().ok()?;
        self.transaction_in_flight.store(false, Ordering::Release);
        Some(receipt)
    }
}
fn apply_transport_commands(engine: &mut AudioEngine, commands: &mut Consumer<TransportCommand>) {
    while let Ok(command) = commands.pop() {
        match command {
            TransportCommand::Play => engine.set_running(true),
            TransportCommand::Stop => engine.set_running(false),
            TransportCommand::Restart => engine.restart(),
            TransportCommand::SeekTicks(tick) => engine.seek_ticks(tick),
            TransportCommand::Loop(range) => engine.set_loop(range),
            TransportCommand::Panic => engine.panic(),
        }
    }
}

fn finite_sample(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

#[derive(Clone, Copy)]
struct CommitMetadata {
    committed_at: Instant,
    sample_position: u64,
    transport: crate::audio::TransportSnapshot,
    failure: Option<TransactionApplyError>,
}

fn commit_transaction(
    engine: &mut AudioEngine,
    transaction: &mut PreparedTransaction,
    counters: &CallbackCounters,
) -> CommitMetadata {
    let sample_position = engine.status().transport.sample_position;
    let committed_at = Instant::now();
    let structural = transaction.is_structural();
    let failure = engine.apply_transaction(transaction).err();
    let transport = engine.status().transport;
    if failure.is_none() {
        counters.transaction_commits.fetch_add(1, Ordering::Relaxed);
        if structural {
            counters.structural_commits.fetch_add(1, Ordering::Relaxed);
        }
    } else {
        counters.transaction_faults.fetch_add(1, Ordering::Relaxed);
    }
    CommitMetadata {
        committed_at,
        sample_position,
        transport,
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

fn make_receipt(
    transaction: Box<PreparedTransaction>,
    commit: CommitMetadata,
    callback_count: u64,
    counters: &CallbackCounters,
) -> TransactionReceipt {
    let structural = transaction.is_structural();
    let faded = transaction.needs_fade();
    TransactionReceipt {
        revision: transaction.revision(),
        observed_generation: transaction.observed_generation(),
        committed_at: commit.committed_at,
        sample_position: commit.sample_position,
        transport: commit.transport,
        callback_count,
        render_faults: counters.render_faults.load(Ordering::Relaxed),
        stream_errors: counters.stream_errors.load(Ordering::Relaxed),
        transaction_commits: counters.transaction_commits.load(Ordering::Relaxed),
        transaction_faults: counters.transaction_faults.load(Ordering::Relaxed),
        structural,
        faded,
        committed: commit.failure.is_none(),
        failure: commit.failure,
        transaction,
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
        self.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
