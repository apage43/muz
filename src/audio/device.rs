mod studio;

use std::{
    env,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use thiserror::Error;

use crate::{
    audio::{
        MAX_ACTIVE_NOTES, MAX_EVENTS_PER_BLOCK,
        transport::TransportSnapshot,
        vst3::{PreparedVst3, Vst3ClassId, Vst3Event, Vst3TimeContext},
    },
    model,
};

const POLY_SYNTH_VOICES: usize = 16;
const MIN_DELAY_BPM: f64 = 20.0;
const MAX_DELAY_BEATS: f64 = 16.0;
const MAX_DELAY_SECONDS: f64 = MAX_DELAY_BEATS * 60.0 / MIN_DELAY_BPM;
const SILENCE_THRESHOLD: f32 = 1.0e-6;

const VST3_ADAPTER_EVENT_CAPACITY: usize = MAX_EVENTS_PER_BLOCK * 3 + MAX_ACTIVE_NOTES + 3;
static NEXT_INSTANCE_TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioConfig {
    pub sample_rate: f32,
    pub max_frames: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceEvent {
    pub offset: u32,
    pub kind: DeviceEventKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeviceEventKind {
    NoteOn {
        note_id: u64,
        channel: u8,
        key: u8,
        velocity: f32,
    },
    NoteOff {
        note_id: u64,
        channel: u8,
        key: u8,
        velocity: f32,
    },
    Controller {
        channel: u8,
        controller: u8,
        value: u8,
    },
    Flush,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessContext {
    pub frames: usize,
    pub block_start_sample: u64,
    pub transport: TransportSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceDebugState {
    pub instance_token: u64,
    pub process_count: u64,
    pub gain_reduction_db: f32,
    pub latency_samples: u32,
    pub tail_samples: u32,
    pub is_plugin: bool,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum DeviceError {
    #[error("invalid audio configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("unknown parameter for {kind:?}")]
    UnknownParameter { kind: model::DeviceKind },
    #[error("invalid value for {parameter} on {kind:?}")]
    InvalidParameterValue {
        kind: model::DeviceKind,
        parameter: &'static str,
    },
    #[error("audio block of {frames} frames exceeds configured maximum of {max_frames}")]
    FrameCapacityExceeded { frames: usize, max_frames: usize },
    #[error("audio buffers are shorter than the requested frame count")]
    BufferTooShort,
    #[error("could not allocate the maximum delay storage")]
    DelayAllocationFailed,
    #[error("VST3 device has no plugin configuration")]
    MissingVst3Config,
    #[error("VST3 bundle environment variable is unset")]
    MissingVst3BundleEnvironment,
    #[error("VST3 class ID is invalid")]
    InvalidVst3ClassId,
    #[error("VST3 expected version is empty")]
    MissingVst3ExpectedVersion,
    #[error("VST3 supports only a 48 kHz sample rate")]
    IncompatibleVst3AudioConfig,
    #[error("VST3 note ID {0} cannot be represented by the plugin ABI")]
    InvalidVst3NoteId(u64),
    #[error("VST3 active-note storage is full")]
    Vst3ActiveNoteCapacityExceeded,
    #[error("VST3 adapter event storage is full")]
    Vst3EventCapacityExceeded,
    #[error("VST3 preparation failed")]
    Vst3PreparationFailed,
    #[error("VST3 processing failed")]
    Vst3ProcessFailed,
}

pub trait DeviceProcessor: Send {
    fn kind(&self) -> model::DeviceKind;
    fn debug_state(&self) -> DeviceDebugState;
    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError>;
    fn reset(&mut self);
    fn process(
        &mut self,
        ctx: ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError>;
}

pub fn create_processor(
    device: &model::Device,
    config: AudioConfig,
) -> Result<Box<dyn DeviceProcessor>, DeviceError> {
    validate_config(config)?;
    let token = NEXT_INSTANCE_TOKEN.fetch_add(1, Ordering::Relaxed);

    match device.kind {
        model::DeviceKind::StudioSynth => {
            Ok(Box::new(studio::StudioSynth::new(device, config, token)?))
        }
        model::DeviceKind::Reverb => Ok(Box::new(studio::Reverb::new(device, config, token)?)),
        model::DeviceKind::Stereo => Ok(Box::new(studio::Stereo::new(device, config, token)?)),
        model::DeviceKind::PolySynth => Ok(Box::new(PolySynth::new(device, config, token)?)),
        model::DeviceKind::Lowpass => Ok(Box::new(Lowpass::new(device, config, token)?)),
        model::DeviceKind::Highpass => Ok(Box::new(Highpass::new(device, config, token)?)),
        model::DeviceKind::Drive => Ok(Box::new(Drive::new(device, config, token)?)),
        model::DeviceKind::Gain => Ok(Box::new(Gain::new(device, config, token)?)),
        model::DeviceKind::Delay => Ok(Box::new(Delay::new(device, config, token)?)),
        model::DeviceKind::Compressor => Ok(Box::new(Compressor::new(device, config, token)?)),
        model::DeviceKind::Limiter => Ok(Box::new(Limiter::new(device, config, token)?)),
        model::DeviceKind::Vst3 => Ok(Box::new(Vst3Processor::new(device, config, token)?)),
    }
}

fn validate_config(config: AudioConfig) -> Result<(), DeviceError> {
    if !config.sample_rate.is_finite() || config.sample_rate <= 0.0 {
        return Err(DeviceError::InvalidConfig(
            "sample_rate must be finite and positive",
        ));
    }
    if config.max_frames == 0 {
        return Err(DeviceError::InvalidConfig("max_frames must be positive"));
    }
    Ok(())
}

fn parameter_value(
    kind: model::DeviceKind,
    parameter: &'static str,
    value: f32,
    minimum: f32,
    maximum: f32,
) -> Result<f32, DeviceError> {
    if value.is_finite() && (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(DeviceError::InvalidParameterValue { kind, parameter })
    }
}

fn db_to_amplitude(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

struct ProcessorCore {
    kind: model::DeviceKind,
    instance_token: u64,
    process_count: u64,
    max_frames: usize,
}

impl ProcessorCore {
    fn new(kind: model::DeviceKind, instance_token: u64, max_frames: usize) -> Self {
        Self {
            kind,
            instance_token,
            process_count: 0,
            max_frames,
        }
    }

    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            instance_token: self.instance_token,
            process_count: self.process_count,
            gain_reduction_db: 0.0,
            latency_samples: 0,
            tail_samples: 0,
            is_plugin: false,
        }
    }

    fn begin_process(
        &mut self,
        frames: usize,
        left_len: usize,
        right_len: usize,
    ) -> Result<(), DeviceError> {
        if frames > self.max_frames {
            return Err(DeviceError::FrameCapacityExceeded {
                frames,
                max_frames: self.max_frames,
            });
        }
        if left_len < frames || right_len < frames {
            return Err(DeviceError::BufferTooShort);
        }
        self.process_count = self.process_count.wrapping_add(1);
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct ActiveVst3Note {
    note_id: i32,
    channel: u8,
    key: u8,
    active: bool,
}

impl ActiveVst3Note {
    const INACTIVE: Self = Self {
        note_id: 0,
        channel: 0,
        key: 0,
        active: false,
    };
}

const EMPTY_VST3_EVENT: Vst3Event = Vst3Event::Pedal {
    sample_offset: 0,
    controller: 64,
    value: 0,
};

struct Vst3EventAdapter {
    active: [ActiveVst3Note; MAX_ACTIVE_NOTES],
    pending_active: [ActiveVst3Note; MAX_ACTIVE_NOTES],
    pedals: [u8; 3],
    pending_pedals: [u8; 3],
    output: [Vst3Event; VST3_ADAPTER_EVENT_CAPACITY],
    output_len: usize,
}

impl Vst3EventAdapter {
    fn new() -> Self {
        Self {
            active: [ActiveVst3Note::INACTIVE; MAX_ACTIVE_NOTES],
            pending_active: [ActiveVst3Note::INACTIVE; MAX_ACTIVE_NOTES],
            pedals: [0; 3],
            pending_pedals: [0; 3],
            output: [EMPTY_VST3_EVENT; VST3_ADAPTER_EVENT_CAPACITY],
            output_len: 0,
        }
    }

    fn convert(
        &mut self,
        events: &[DeviceEvent],
        pending_flush: bool,
    ) -> Result<&[Vst3Event], DeviceError> {
        self.pending_active = self.active;
        self.pending_pedals = self.pedals;
        self.output_len = 0;
        if pending_flush {
            self.push_flush(0)?;
        }
        for event in events {
            let sample_offset = event.offset as usize;
            match event.kind {
                DeviceEventKind::NoteOn {
                    note_id,
                    channel,
                    key,
                    velocity,
                } => {
                    let note_id = checked_vst3_note_id(note_id)?;
                    self.push(Vst3Event::NoteOn {
                        sample_offset,
                        channel,
                        pitch: key,
                        velocity,
                        note_id,
                    })?;
                    self.activate(note_id, channel, key)?;
                }
                DeviceEventKind::NoteOff {
                    note_id,
                    channel,
                    key,
                    velocity,
                } => {
                    let note_id = checked_vst3_note_id(note_id)?;
                    self.push(Vst3Event::NoteOff {
                        sample_offset,
                        channel,
                        pitch: key,
                        velocity,
                        note_id,
                    })?;
                    self.deactivate(note_id, channel, key);
                }
                DeviceEventKind::Controller {
                    controller, value, ..
                } => {
                    if let Some(index) = pedal_index(controller) {
                        self.push(Vst3Event::Pedal {
                            sample_offset,
                            controller,
                            value,
                        })?;
                        self.pending_pedals[index] = value;
                    }
                }
                DeviceEventKind::Flush => self.push_flush(sample_offset)?,
            }
        }
        Ok(&self.output[..self.output_len])
    }

    fn commit(&mut self) {
        self.active = self.pending_active;
        self.pedals = self.pending_pedals;
    }

    fn push(&mut self, event: Vst3Event) -> Result<(), DeviceError> {
        let Some(slot) = self.output.get_mut(self.output_len) else {
            return Err(DeviceError::Vst3EventCapacityExceeded);
        };
        *slot = event;
        self.output_len += 1;
        Ok(())
    }

    fn activate(&mut self, note_id: i32, channel: u8, key: u8) -> Result<(), DeviceError> {
        let target = self
            .pending_active
            .iter()
            .position(|note| note.active && note.note_id == note_id)
            .or_else(|| self.pending_active.iter().position(|note| !note.active))
            .ok_or(DeviceError::Vst3ActiveNoteCapacityExceeded)?;
        self.pending_active[target] = ActiveVst3Note {
            note_id,
            channel,
            key,
            active: true,
        };
        Ok(())
    }

    fn deactivate(&mut self, note_id: i32, channel: u8, key: u8) {
        for note in &mut self.pending_active {
            if note.active && note.note_id == note_id && note.channel == channel && note.key == key
            {
                *note = ActiveVst3Note::INACTIVE;
            }
        }
    }

    fn push_flush(&mut self, sample_offset: usize) -> Result<(), DeviceError> {
        for index in 0..self.pending_active.len() {
            let note = self.pending_active[index];
            if note.active {
                self.push(Vst3Event::NoteOff {
                    sample_offset,
                    channel: note.channel,
                    pitch: note.key,
                    velocity: 0.0,
                    note_id: note.note_id,
                })?;
                self.pending_active[index] = ActiveVst3Note::INACTIVE;
            }
        }
        for (index, controller) in [64, 66, 67].into_iter().enumerate() {
            self.push(Vst3Event::Pedal {
                sample_offset,
                controller,
                value: 0,
            })?;
            self.pending_pedals[index] = 0;
        }
        Ok(())
    }
}

fn checked_vst3_note_id(note_id: u64) -> Result<i32, DeviceError> {
    i32::try_from(note_id).map_err(|_| DeviceError::InvalidVst3NoteId(note_id))
}

fn pedal_index(controller: u8) -> Option<usize> {
    [64, 66, 67]
        .iter()
        .position(|candidate| *candidate == controller)
}

struct Vst3Processor {
    host: PreparedVst3,
    instance_token: u64,
    max_frames: usize,
    adapter: Vst3EventAdapter,
    reset_pending: bool,
}

impl Vst3Processor {
    fn new(
        device: &model::Device,
        config: AudioConfig,
        instance_token: u64,
    ) -> Result<Self, DeviceError> {
        if config.sample_rate != VST3_SAMPLE_RATE_F32 || config.max_frames == 0 {
            return Err(DeviceError::IncompatibleVst3AudioConfig);
        }
        let plugin = device.vst3.as_ref().ok_or(DeviceError::MissingVst3Config)?;
        if plugin.expected_version.is_empty() {
            return Err(DeviceError::MissingVst3ExpectedVersion);
        }
        let bundle = env::var_os(&plugin.bundle_env)
            .or_else(|| {
                if plugin.bundle_env.starts_with('/') {
                    Some(plugin.bundle_env.clone().into())
                } else {
                    None
                }
            })
            .map(PathBuf::from)
            .ok_or(DeviceError::MissingVst3BundleEnvironment)?;
        let class_id = plugin
            .class_id
            .parse::<Vst3ClassId>()
            .map_err(|_| DeviceError::InvalidVst3ClassId)?;
        let host = PreparedVst3::prepare(&bundle, class_id, Some(&plugin.expected_version))
            .map_err(|_| DeviceError::Vst3PreparationFailed)?;
        Ok(Self {
            host,
            instance_token,
            max_frames: config.max_frames,
            adapter: Vst3EventAdapter::new(),
            reset_pending: false,
        })
    }
}

const VST3_SAMPLE_RATE_F32: f32 = 48_000.0;

impl DeviceProcessor for Vst3Processor {
    fn kind(&self) -> model::DeviceKind {
        model::DeviceKind::Vst3
    }

    fn debug_state(&self) -> DeviceDebugState {
        let metadata = self.host.metadata();
        DeviceDebugState {
            instance_token: self.instance_token,
            process_count: self.host.process_count(),
            gain_reduction_db: 0.0,
            latency_samples: metadata.latency_samples,
            tail_samples: metadata.tail_samples,
            is_plugin: true,
        }
    }

    fn set_parameter(&mut self, _name: &str, _value: f32) -> Result<(), DeviceError> {
        Err(DeviceError::UnknownParameter { kind: self.kind() })
    }

    fn reset(&mut self) {
        self.reset_pending = true;
    }

    fn process(
        &mut self,
        ctx: ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        if ctx.frames > self.max_frames {
            return Err(DeviceError::FrameCapacityExceeded {
                frames: ctx.frames,
                max_frames: self.max_frames,
            });
        }
        if left.len() < ctx.frames || right.len() < ctx.frames {
            return Err(DeviceError::BufferTooShort);
        }
        let context = vst3_time_context(ctx);
        let converted = self.adapter.convert(events, self.reset_pending)?;
        self.host
            .process(
                &mut left[..ctx.frames],
                &mut right[..ctx.frames],
                converted,
                context,
            )
            .map_err(|_| DeviceError::Vst3ProcessFailed)?;
        self.adapter.commit();
        self.reset_pending = false;
        Ok(())
    }
}

fn vst3_time_context(ctx: ProcessContext) -> Vst3TimeContext {
    let beat = ctx.transport.beat_position;
    let beats_per_bar = f64::from(ctx.transport.meter[0]) * 4.0 / f64::from(ctx.transport.meter[1]);
    Vst3TimeContext {
        continuous_time_samples: i64::try_from(ctx.block_start_sample).unwrap_or(i64::MAX),
        project_time_samples: ctx.transport.project_frame.floor() as i64,
        project_time_music: beat,
        bar_position_music: (beat / beats_per_bar).floor() * beats_per_bar,
        tempo: ctx.transport.bpm,
        time_signature_numerator: i32::from(ctx.transport.meter[0]),
        time_signature_denominator: i32::from(ctx.transport.meter[1]),
        playing: ctx.transport.running,
    }
}

#[derive(Clone, Copy)]
struct Voice {
    note_id: u64,
    key: u8,
    phase: f32,
    envelope: f32,
    release_step: f32,
    velocity: f32,
    active: bool,
    releasing: bool,
}

impl Voice {
    const INACTIVE: Self = Self {
        note_id: 0,
        key: 0,
        phase: 0.0,
        envelope: 0.0,
        release_step: 0.0,
        velocity: 0.0,
        active: false,
        releasing: false,
    };
}

struct PolySynth {
    core: ProcessorCore,
    sample_rate: f32,
    gain: f32,
    attack_samples: f32,
    release_samples: f32,
    voices: [Voice; POLY_SYNTH_VOICES],
}

impl PolySynth {
    fn new(device: &model::Device, config: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut processor = Self {
            core: ProcessorCore::new(device.kind, token, config.max_frames),
            sample_rate: config.sample_rate,
            gain: db_to_amplitude(-12.0),
            attack_samples: config.sample_rate * 0.010,
            release_samples: config.sample_rate * 0.250,
            voices: [Voice::INACTIVE; POLY_SYNTH_VOICES],
        };
        for (name, value) in &device.params {
            processor.set_parameter(name, *value)?;
        }
        Ok(processor)
    }

    fn note_on(&mut self, note_id: u64, key: u8, velocity: f32) {
        let mut target = self
            .voices
            .iter()
            .position(|voice| voice.active && voice.note_id == note_id)
            .or_else(|| self.voices.iter().position(|voice| !voice.active));

        if target.is_none() {
            let mut quietest = 0;
            for index in 1..self.voices.len() {
                if self.voices[index].envelope < self.voices[quietest].envelope {
                    quietest = index;
                }
            }
            target = Some(quietest);
        }

        let envelope = if self.attack_samples <= 0.0 { 1.0 } else { 0.0 };
        self.voices[target.expect("a fixed voice array is nonempty")] = Voice {
            note_id,
            key,
            phase: 0.0,
            envelope,
            release_step: 0.0,
            velocity: velocity.clamp(0.0, 1.0),
            active: true,
            releasing: false,
        };
    }

    fn note_off(&mut self, note_id: u64, key: u8) {
        for voice in &mut self.voices {
            if voice.active && voice.note_id == note_id && voice.key == key {
                voice.releasing = true;
                voice.release_step = voice.envelope / self.release_samples.max(1.0);
            }
        }
    }

    fn apply_event(&mut self, event: DeviceEventKind) {
        match event {
            DeviceEventKind::NoteOn {
                note_id,
                key,
                velocity,
                ..
            } => self.note_on(note_id, key, velocity),
            DeviceEventKind::NoteOff { note_id, key, .. } => self.note_off(note_id, key),
            DeviceEventKind::Controller { .. } => {}
            DeviceEventKind::Flush => self.voices = [Voice::INACTIVE; POLY_SYNTH_VOICES],
        }
    }

    fn render_sample(&mut self) -> f32 {
        let mut output = 0.0;
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }

            if voice.releasing {
                voice.envelope = (voice.envelope - voice.release_step).max(0.0);
                if voice.envelope <= SILENCE_THRESHOLD {
                    *voice = Voice::INACTIVE;
                    continue;
                }
            } else if voice.envelope < 1.0 {
                voice.envelope = (voice.envelope + 1.0 / self.attack_samples.max(1.0)).min(1.0);
            }

            output += voice.phase.sin() * voice.velocity * voice.envelope;
            let frequency = 440.0 * 2.0_f32.powf((f32::from(voice.key) - 69.0) / 12.0);
            voice.phase += std::f32::consts::TAU * frequency / self.sample_rate;
            if voice.phase >= std::f32::consts::TAU {
                voice.phase -= std::f32::consts::TAU;
            }
        }
        output * self.gain
    }
}

impl DeviceProcessor for PolySynth {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }

    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }

    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "gain_db" => {
                self.gain =
                    db_to_amplitude(parameter_value(self.kind(), "gain_db", value, -60.0, 12.0)?);
            }
            "attack_ms" => {
                self.attack_samples =
                    parameter_value(self.kind(), "attack_ms", value, 0.0, 5_000.0)?
                        * self.sample_rate
                        / 1_000.0;
            }
            "release_ms" => {
                self.release_samples =
                    parameter_value(self.kind(), "release_ms", value, 1.0, 10_000.0)?
                        * self.sample_rate
                        / 1_000.0;
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.voices = [Voice::INACTIVE; POLY_SYNTH_VOICES];
    }

    fn process(
        &mut self,
        ctx: ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        let mut event_index = 0;
        for frame in 0..ctx.frames {
            while event_index < events.len() && events[event_index].offset as usize <= frame {
                self.apply_event(events[event_index].kind);
                event_index += 1;
            }
            let sample = self.render_sample();
            left[frame] = sample;
            right[frame] = sample;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
struct BiquadState {
    z1: f32,
    z2: f32,
}

#[derive(Clone, Copy)]
struct BiquadCoefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

struct Lowpass {
    core: ProcessorCore,
    sample_rate: f32,
    cutoff_hz: f32,
    resonance: f32,
    coefficients: BiquadCoefficients,
    state: [BiquadState; 2],
}

impl Lowpass {
    fn new(device: &model::Device, config: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut processor = Self {
            core: ProcessorCore::new(device.kind, token, config.max_frames),
            sample_rate: config.sample_rate,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            coefficients: BiquadCoefficients {
                b0: 1.0,
                b1: 0.0,
                b2: 0.0,
                a1: 0.0,
                a2: 0.0,
            },
            state: [BiquadState::default(); 2],
        };
        processor.update_coefficients();
        for (name, value) in &device.params {
            processor.set_parameter(name, *value)?;
        }
        Ok(processor)
    }

    fn update_coefficients(&mut self) {
        let cutoff = self.cutoff_hz.min(self.sample_rate * 0.49);
        let omega = std::f32::consts::TAU * cutoff / self.sample_rate;
        let (sin, cos) = omega.sin_cos();
        let q = 0.5 + self.resonance * 9.5;
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        self.coefficients = BiquadCoefficients {
            b0: (1.0 - cos) * 0.5 / a0,
            b1: (1.0 - cos) / a0,
            b2: (1.0 - cos) * 0.5 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
        };
    }

    fn filter_sample(coefficients: BiquadCoefficients, state: &mut BiquadState, input: f32) -> f32 {
        let output = coefficients.b0 * input + state.z1;
        state.z1 = coefficients.b1 * input - coefficients.a1 * output + state.z2;
        state.z2 = coefficients.b2 * input - coefficients.a2 * output;
        output
    }
}

impl DeviceProcessor for Lowpass {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }

    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }

    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "cutoff_hz" => {
                self.cutoff_hz = parameter_value(self.kind(), "cutoff_hz", value, 20.0, 20_000.0)?;
            }
            "resonance" => {
                self.resonance = parameter_value(self.kind(), "resonance", value, 0.0, 1.0)?;
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        self.update_coefficients();
        Ok(())
    }

    fn reset(&mut self) {
        self.state = [BiquadState::default(); 2];
    }

    fn process(
        &mut self,
        ctx: ProcessContext,
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        for frame in 0..ctx.frames {
            left[frame] = Self::filter_sample(self.coefficients, &mut self.state[0], left[frame]);
            right[frame] = Self::filter_sample(self.coefficients, &mut self.state[1], right[frame]);
        }
        Ok(())
    }
}

struct Highpass {
    core: ProcessorCore,
    sample_rate: f32,
    cutoff_hz: f32,
    resonance: f32,
    coefficients: BiquadCoefficients,
    state: [BiquadState; 2],
}

impl Highpass {
    fn new(device: &model::Device, config: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut processor = Self {
            core: ProcessorCore::new(device.kind, token, config.max_frames),
            sample_rate: config.sample_rate,
            cutoff_hz: 20.0,
            resonance: 0.707,
            coefficients: BiquadCoefficients {
                b0: 1.0,
                b1: 0.0,
                b2: 0.0,
                a1: 0.0,
                a2: 0.0,
            },
            state: [BiquadState::default(); 2],
        };
        processor.update_coefficients();
        for (name, value) in &device.params {
            processor.set_parameter(name, *value)?;
        }
        Ok(processor)
    }

    fn update_coefficients(&mut self) {
        let cutoff = self.cutoff_hz.min(self.sample_rate * 0.49);
        let omega = std::f32::consts::TAU * cutoff / self.sample_rate;
        let (sin, cos) = omega.sin_cos();
        let alpha = sin / (2.0 * self.resonance.max(0.01));
        let a0 = 1.0 + alpha;
        self.coefficients = BiquadCoefficients {
            b0: (1.0 + cos) * 0.5 / a0,
            b1: -(1.0 + cos) / a0,
            b2: (1.0 + cos) * 0.5 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
        };
    }

    fn filter_sample(coefficients: BiquadCoefficients, state: &mut BiquadState, input: f32) -> f32 {
        let output = coefficients.b0 * input + state.z1;
        state.z1 = coefficients.b1 * input - coefficients.a1 * output + state.z2;
        state.z2 = coefficients.b2 * input - coefficients.a2 * output;
        output
    }
}

impl DeviceProcessor for Highpass {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }

    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }

    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "cutoff_hz" => {
                self.cutoff_hz = parameter_value(self.kind(), "cutoff_hz", value, 20.0, 20_000.0)?;
            }
            "resonance" => {
                self.resonance = parameter_value(self.kind(), "resonance", value, 0.0, 1.0)?;
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        self.update_coefficients();
        Ok(())
    }

    fn reset(&mut self) {
        self.state = [BiquadState::default(); 2];
    }

    fn process(
        &mut self,
        ctx: ProcessContext,
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        for frame in 0..ctx.frames {
            left[frame] = Self::filter_sample(self.coefficients, &mut self.state[0], left[frame]);
            right[frame] = Self::filter_sample(self.coefficients, &mut self.state[1], right[frame]);
        }
        Ok(())
    }
}

struct Drive {
    core: ProcessorCore,
    drive: f32,
    mix: f32,
}

impl Drive {
    fn new(device: &model::Device, config: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut processor = Self {
            core: ProcessorCore::new(device.kind, token, config.max_frames),
            drive: 1.0,
            mix: 1.0,
        };
        for (name, value) in &device.params {
            processor.set_parameter(name, *value)?;
        }
        Ok(processor)
    }

    fn shape(&self, input: f32) -> f32 {
        if self.drive == 1.0 {
            input
        } else {
            (input * self.drive).tanh() / self.drive.tanh()
        }
    }
}

impl DeviceProcessor for Drive {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }

    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }

    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "drive_db" => {
                self.drive =
                    db_to_amplitude(parameter_value(self.kind(), "drive_db", value, 0.0, 36.0)?);
            }
            "mix" => {
                self.mix = parameter_value(self.kind(), "mix", value, 0.0, 1.0)?;
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }

    fn reset(&mut self) {}

    fn process(
        &mut self,
        ctx: ProcessContext,
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        let dry = 1.0 - self.mix;
        for frame in 0..ctx.frames {
            left[frame] = left[frame] * dry + self.shape(left[frame]) * self.mix;
            right[frame] = right[frame] * dry + self.shape(right[frame]) * self.mix;
        }
        Ok(())
    }
}

struct Gain {
    core: ProcessorCore,
    gain: f32,
}

impl Gain {
    fn new(device: &model::Device, config: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut processor = Self {
            core: ProcessorCore::new(device.kind, token, config.max_frames),
            gain: 1.0,
        };
        for (name, value) in &device.params {
            processor.set_parameter(name, *value)?;
        }
        Ok(processor)
    }
}

impl DeviceProcessor for Gain {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }

    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }

    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "gain_db" => {
                self.gain =
                    db_to_amplitude(parameter_value(self.kind(), "gain_db", value, -60.0, 12.0)?);
                Ok(())
            }
            _ => Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
    }

    fn reset(&mut self) {}

    fn process(
        &mut self,
        ctx: ProcessContext,
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        for frame in 0..ctx.frames {
            left[frame] *= self.gain;
            right[frame] *= self.gain;
        }
        Ok(())
    }
}

struct Delay {
    core: ProcessorCore,
    sample_rate: f32,
    time_beats: f32,
    feedback: f32,
    mix: f32,
    left_delay: Vec<f32>,
    right_delay: Vec<f32>,
    write_cursor: usize,
}

impl Delay {
    fn new(device: &model::Device, config: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let samples = f64::from(config.sample_rate) * MAX_DELAY_SECONDS;
        if !samples.is_finite() || samples > (usize::MAX - 1) as f64 {
            return Err(DeviceError::InvalidConfig(
                "sample_rate is too large for delay storage",
            ));
        }
        let capacity = samples.ceil() as usize + 1;
        let mut left_delay = Vec::new();
        let mut right_delay = Vec::new();
        left_delay
            .try_reserve_exact(capacity)
            .map_err(|_| DeviceError::DelayAllocationFailed)?;
        right_delay
            .try_reserve_exact(capacity)
            .map_err(|_| DeviceError::DelayAllocationFailed)?;
        left_delay.resize(capacity, 0.0);
        right_delay.resize(capacity, 0.0);

        let mut processor = Self {
            core: ProcessorCore::new(device.kind, token, config.max_frames),
            sample_rate: config.sample_rate,
            time_beats: 0.5,
            feedback: 0.35,
            mix: 0.25,
            left_delay,
            right_delay,
            write_cursor: 0,
        };
        for (name, value) in &device.params {
            processor.set_parameter(name, *value)?;
        }
        Ok(processor)
    }

    fn delay_samples(&self, bpm: f64) -> usize {
        let bpm = if bpm.is_finite() && bpm > 0.0 {
            bpm.max(MIN_DELAY_BPM)
        } else {
            120.0
        };
        let samples = f64::from(self.time_beats) * 60.0 / bpm * f64::from(self.sample_rate);
        (samples.round() as usize).clamp(1, self.left_delay.len() - 1)
    }
}

impl DeviceProcessor for Delay {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }

    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }

    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "time_beats" => {
                self.time_beats = parameter_value(self.kind(), "time_beats", value, 0.03125, 16.0)?;
            }
            "feedback" => {
                self.feedback = parameter_value(self.kind(), "feedback", value, 0.0, 0.99)?;
            }
            "mix" => {
                self.mix = parameter_value(self.kind(), "mix", value, 0.0, 1.0)?;
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.left_delay.fill(0.0);
        self.right_delay.fill(0.0);
        self.write_cursor = 0;
    }

    fn process(
        &mut self,
        ctx: ProcessContext,
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        let delay_samples = self.delay_samples(ctx.transport.bpm);
        let dry = 1.0 - self.mix;
        for frame in 0..ctx.frames {
            let read_cursor =
                (self.write_cursor + self.left_delay.len() - delay_samples) % self.left_delay.len();
            let delayed_left = self.left_delay[read_cursor];
            let delayed_right = self.right_delay[read_cursor];
            let input_left = left[frame];
            let input_right = right[frame];
            self.left_delay[self.write_cursor] = input_left + delayed_left * self.feedback;
            self.right_delay[self.write_cursor] = input_right + delayed_right * self.feedback;
            left[frame] = input_left * dry + delayed_left * self.mix;
            right[frame] = input_right * dry + delayed_right * self.mix;
            self.write_cursor += 1;
            if self.write_cursor == self.left_delay.len() {
                self.write_cursor = 0;
            }
        }
        Ok(())
    }
}

struct Compressor {
    core: ProcessorCore,
    sample_rate: f32,
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    knee_db: f32,
    makeup_gain: f32,
    mix: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
    envelope: f32,
    gain_reduction_db: f32,
}

impl Compressor {
    fn new(device: &model::Device, config: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut processor = Self {
            core: ProcessorCore::new(device.kind, token, config.max_frames),
            sample_rate: config.sample_rate,
            threshold_db: -18.0,
            ratio: 2.0,
            attack_ms: 30.0,
            release_ms: 250.0,
            knee_db: 6.0,
            makeup_gain: 1.0,
            mix: 1.0,
            attack_coefficient: 0.0,
            release_coefficient: 0.0,
            envelope: 0.0,
            gain_reduction_db: 0.0,
        };
        processor.update_time_coefficients();
        for (name, value) in &device.params {
            processor.set_parameter(name, *value)?;
        }
        Ok(processor)
    }

    fn time_coefficient(&self, milliseconds: f32) -> f32 {
        (-1.0 / (milliseconds * 0.001 * self.sample_rate)).exp()
    }

    fn update_time_coefficients(&mut self) {
        self.attack_coefficient = self.time_coefficient(self.attack_ms);
        self.release_coefficient = self.time_coefficient(self.release_ms);
    }

    fn reduction_for_level(&self, level_db: f32) -> f32 {
        let slope = 1.0 - 1.0 / self.ratio;
        let over_db = level_db - self.threshold_db;
        if self.knee_db > 0.0 {
            let half_knee = self.knee_db * 0.5;
            if over_db <= -half_knee {
                0.0
            } else if over_db >= half_knee {
                slope * over_db
            } else {
                let knee_position = over_db + half_knee;
                slope * knee_position * knee_position / (2.0 * self.knee_db)
            }
        } else {
            slope * over_db.max(0.0)
        }
    }
}

impl DeviceProcessor for Compressor {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }

    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            gain_reduction_db: self.gain_reduction_db.max(0.0),
            ..self.core.debug_state()
        }
    }

    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "threshold_db" => {
                self.threshold_db =
                    parameter_value(self.kind(), "threshold_db", value, -60.0, 0.0)?;
            }
            "ratio" => {
                self.ratio = parameter_value(self.kind(), "ratio", value, 1.0, 20.0)?;
            }
            "attack_ms" => {
                self.attack_ms = parameter_value(self.kind(), "attack_ms", value, 0.1, 200.0)?;
                self.update_time_coefficients();
            }
            "release_ms" => {
                self.release_ms = parameter_value(self.kind(), "release_ms", value, 10.0, 2_000.0)?;
                self.update_time_coefficients();
            }
            "knee_db" => {
                self.knee_db = parameter_value(self.kind(), "knee_db", value, 0.0, 24.0)?;
            }
            "makeup_db" => {
                self.makeup_gain = db_to_amplitude(parameter_value(
                    self.kind(),
                    "makeup_db",
                    value,
                    -24.0,
                    24.0,
                )?);
            }
            "mix" => {
                self.mix = parameter_value(self.kind(), "mix", value, 0.0, 1.0)?;
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.envelope = 0.0;
        self.gain_reduction_db = 0.0;
    }

    fn process(
        &mut self,
        ctx: ProcessContext,
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        let dry_mix = 1.0 - self.mix;
        for frame in 0..ctx.frames {
            let input_left = left[frame];
            let input_right = right[frame];
            let detector = input_left.abs().max(input_right.abs());
            let coefficient = if detector > self.envelope {
                self.attack_coefficient
            } else {
                self.release_coefficient
            };
            self.envelope = detector + coefficient * (self.envelope - detector);
            let level_db = 20.0 * self.envelope.max(1.0e-20).log10();
            self.gain_reduction_db = self.reduction_for_level(level_db).max(0.0);
            let wet_gain = db_to_amplitude(-self.gain_reduction_db) * self.makeup_gain * self.mix;
            left[frame] = input_left * (dry_mix + wet_gain);
            right[frame] = input_right * (dry_mix + wet_gain);
        }
        Ok(())
    }
}

struct Limiter {
    core: ProcessorCore,
    sample_rate: f32,
    ceiling: f32,
    release_ms: f32,
    release_coefficient: f32,
    gain: f32,
    gain_reduction_db: f32,
    lookahead: usize,
    delay: Vec<[f32; 2]>,
    cursor: usize,
    hold: usize,
    target: f32,
    attack_coefficient: f32,
}

impl Limiter {
    fn new(device: &model::Device, config: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut processor = Self {
            core: ProcessorCore::new(device.kind, token, config.max_frames),
            sample_rate: config.sample_rate,
            ceiling: db_to_amplitude(-1.0),
            release_ms: 100.0,
            release_coefficient: 0.0,
            gain: 1.0,
            gain_reduction_db: 0.0,
            lookahead: 0,
            delay: vec![[0.0; 2]; (config.sample_rate * 0.020).ceil() as usize + 1],
            cursor: 0,
            hold: 0,
            target: 1.0,
            attack_coefficient: (-1.0 / (0.0005 * config.sample_rate)).exp(),
        };
        processor.update_release_coefficient();
        for (name, value) in &device.params {
            processor.set_parameter(name, *value)?;
        }
        Ok(processor)
    }

    fn update_release_coefficient(&mut self) {
        self.release_coefficient = (-1.0 / (self.release_ms * 0.001 * self.sample_rate)).exp();
    }
}

impl DeviceProcessor for Limiter {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }

    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            gain_reduction_db: self.gain_reduction_db.max(0.0),
            latency_samples: self.lookahead as u32,
            ..self.core.debug_state()
        }
    }

    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "lookahead_ms" => {
                self.lookahead = (parameter_value(self.kind(), "lookahead_ms", value, 0.0, 20.0)?
                    * self.sample_rate
                    / 1000.0)
                    .round() as usize;
            }
            "ceiling_db" => {
                self.ceiling = db_to_amplitude(parameter_value(
                    self.kind(),
                    "ceiling_db",
                    value,
                    -24.0,
                    0.0,
                )?);
            }
            "release_ms" => {
                self.release_ms = parameter_value(self.kind(), "release_ms", value, 1.0, 2_000.0)?;
                self.update_release_coefficient();
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.gain = 1.0;
        self.gain_reduction_db = 0.0;
        self.delay.fill([0.0; 2]);
        self.cursor = 0;
        self.hold = 0;
        self.target = 1.0;
    }

    fn process(
        &mut self,
        ctx: ProcessContext,
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        for frame in 0..ctx.frames {
            let peak = left[frame].abs().max(right[frame].abs());
            let target_gain = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            if self.lookahead > 0 {
                self.delay[self.cursor] = [left[frame], right[frame]];
                let read = (self.cursor + self.delay.len() - self.lookahead) % self.delay.len();
                let delayed = self.delay[read];
                self.cursor = (self.cursor + 1) % self.delay.len();
                if target_gain <= self.target {
                    self.target = target_gain;
                    self.hold = self.lookahead;
                } else if self.hold > 0 {
                    self.hold -= 1;
                } else {
                    self.target = 1.0 + self.release_coefficient * (self.target - 1.0);
                }
                let coefficient = if self.target < self.gain {
                    self.attack_coefficient
                } else {
                    self.release_coefficient
                };
                self.gain = self.target + coefficient * (self.gain - self.target);
                let delayed_peak = delayed[0].abs().max(delayed[1].abs());
                let safe_gain = self.gain.min(self.ceiling / delayed_peak.max(1.0e-20));
                left[frame] = delayed[0] * safe_gain;
                right[frame] = delayed[1] * safe_gain;
                self.gain_reduction_db = (-20.0 * safe_gain.max(1.0e-20).log10()).max(0.0);
                continue;
            }
            if target_gain < self.gain {
                self.gain = target_gain;
            } else {
                self.gain = 1.0 + self.release_coefficient * (self.gain - 1.0);
                self.gain = self.gain.min(target_gain);
            }
            left[frame] = (left[frame] * self.gain).clamp(-self.ceiling, self.ceiling);
            right[frame] = (right[frame] * self.gain).clamp(-self.ceiling, self.ceiling);
            self.gain_reduction_db = (-20.0 * self.gain.max(1.0e-20).log10()).max(0.0);
        }
        Ok(())
    }
}
