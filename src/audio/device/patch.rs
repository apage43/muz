//! A fixed per-voice graph. Nodes are prepared once; audio evaluates a flat schedule.
use super::*;
use crate::patch_description::{Operation, Output, Signal};
use anyhow::{Result, ensure};
use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::sync::Arc;
#[path = "patch_envelope.rs"]
mod envelope;
#[path = "patch_sample.rs"]
mod sample;
#[path = "patch_shape.rs"]
mod shape;
#[derive(Clone, Copy)]
enum Input {
    Constant(f32),
    Node(usize),
}
impl Input {
    fn get(self, v: &[f32]) -> f32 {
        match self {
            Self::Constant(x) => x,
            Self::Node(i) => v[i],
        }
    }
}
enum Op {
    Param {
        index: usize,
    },
    Frequency,
    Velocity,
    NoteAge,
    Expression(u8),
    Osc {
        wave: u8,
        ratio: Input,
        detune: Input,
        detune_factor: Option<f32>,
        hz: Option<Input>,
        fm: Input,
        width: Input,
        phase: Input,
    },
    Noise,
    Envelope {
        one_shot: bool,
        attack: Input,
        decay: Input,
        sustain: Input,
        release: Input,
    },
    Shape {
        input: Input,
        shape: shape::Shape,
    },
    Resonator {
        input: Input,
        frequency: Input,
        decay: Input,
        coefficients: Option<(f64, f64)>,
    },
    Mseg(envelope::Envelope),
    Reader {
        reader: sample::Reader,
        speed: Input,
    },
    Map {
        input: Input,
        kind: u8,
        min: Input,
        max: Input,
    },
    Hold {
        input: Input,
        hz: Input,
    },
    Slew {
        input: Input,
        rise: Input,
        fall: Input,
    },
    Sum(Vec<Input>),
    Product(Vec<Input>),
    Drive {
        input: Input,
        amount: Input,
    },
    Filter {
        input: Input,
        cutoff: Input,
        q: Input,
        coefficients: Option<(f32, f32, f32)>,
        mode: u8,
    },
    Delay {
        input: Input,
        seconds: Input,
        feedback: Input,
        damping: Option<Input>,
        max_feedback: f32,
        cubic: bool,
        base: usize,
        length: usize,
    },
    Sample {
        audio: Arc<[[f32; 2]]>,
        channel: u8,
        rate: f32,
        root: f32,
        looped: bool,
    },
}
/// Halves the rate of a stereo stream: a 47-tap Kaiser-windowed half-band
/// lowpass (about 80 dB down from 0.3 of the input rate), then every other frame.
struct Halver {
    taps: [f32; 47],
    history: [[f32; 2]; 64],
    at: usize,
}
impl Halver {
    fn new() -> Self {
        fn bessel(x: f64) -> f64 {
            let (mut sum, mut term) = (1., 1.);
            for k in 1..32 {
                term *= (x / (2. * k as f64)).powi(2);
                sum += term;
            }
            sum
        }
        let mut taps = [0.; 47];
        for (n, tap) in taps.iter_mut().enumerate() {
            let m = n as f64 - 23.;
            let ideal = if m == 0. {
                0.5
            } else {
                (std::f64::consts::FRAC_PI_2 * m).sin() / (std::f64::consts::PI * m)
            };
            let window = bessel(8. * (1. - (m / 23.).powi(2)).sqrt()) / bessel(8.);
            *tap = (ideal * window) as f32;
        }
        Self {
            taps,
            history: [[0.; 2]; 64],
            at: 0,
        }
    }
    fn push(&mut self, frame: [f32; 2]) {
        self.at = (self.at + 1) % 64;
        self.history[self.at] = frame;
    }
    fn read(&self) -> [f32; 2] {
        let mut out = [0.; 2];
        for (n, tap) in self.taps.iter().enumerate() {
            let frame = self.history[(self.at + 64 - n) % 64];
            out[0] += frame[0] * tap;
            out[1] += frame[1] * tap;
        }
        out
    }
    fn clear(&mut self) {
        self.history = [[0.; 2]; 64];
    }
}
struct Parameter {
    name: String,
    value: f32,
    min: f32,
    max: f32,
}
/// The last result of a pure per-sample computation, keyed by its inputs'
/// bits. Controls usually hold still for many samples, so this hoists
/// transcendental math out of the sample loop without changing any result.
#[derive(Clone, Copy, Default)]
struct Memo<T: Copy + Default, const K: usize> {
    key: Option<[u32; K]>,
    value: T,
}
impl<T: Copy + Default, const K: usize> Memo<T, K> {
    #[inline]
    fn get(&mut self, inputs: [f32; K], compute: impl FnOnce() -> T) -> T {
        let key = inputs.map(f32::to_bits);
        if self.key != Some(key) {
            self.value = compute();
            self.key = Some(key);
        }
        self.value
    }
}
#[derive(Clone, Copy, Default)]
struct State {
    phase: f64,
    imaginary: f64,
    env: f32,
    low: f32,
    band: f32,
    index: usize,
    segment: envelope::State,
    reader: sample::State,
    /// Coefficients of this node's control inputs (detune, release, filter,
    /// resonator, slew, damping or sample speed).
    memo: Memo<[f64; 3], 2>,
}
struct PatchVoice {
    active: bool,
    id: u64,
    channel: u8,
    pitch: f32,
    target_pitch: f32,
    glide_left: u64,
    velocity: f32,
    age: u64,
    /// `age` when the sounding note took this voice: its attack, or a legato
    /// transfer.
    note_start: u64,
    released: Option<u64>,
    choked: bool,
    expression: [f32; crate::expression::SLOT_COUNT],
    state: Vec<State>,
    delay: Vec<f32>,
    last_level: f32,
    completed: Option<u64>,
    frequency: Memo<f32, 2>,
    velocity_gain: Memo<f32, 2>,
    pan: Memo<(f32, f32), 1>,
}
impl PatchVoice {
    fn new(
        nodes: usize,
        delay: usize,
        defaults: &[f32; crate::expression::SLOT_COUNT],
    ) -> Self {
        Self {
            active: false,
            id: 0,
            channel: 0,
            pitch: 60.,
            target_pitch: 60.,
            glide_left: 0,
            velocity: 0.,
            age: 0,
            note_start: 0,
            released: None,
            choked: false,
            expression: *defaults,
            state: vec![State::default(); nodes],
            delay: vec![0.; delay],
            last_level: 0.,
            completed: None,
            frequency: Memo::default(),
            velocity_gain: Memo::default(),
            pan: Memo::default(),
        }
    }
    fn reset(&mut self) {
        self.active = false;
        self.completed = None;
        self.delay.fill(0.);
        self.state.fill(State::default());
    }
}
pub struct VoicePatch {
    core: ProcessorCore,
    rate: f32,
    nodes: Vec<Op>,
    /// Delay nodes, written once the frame's signals are known.
    delays: Vec<usize>,
    output: Input,
    right: Option<Input>,
    lifetime: Option<(usize, u64)>,
    parameters: Vec<Parameter>,
    voices: Vec<PatchVoice>,
    gain: f32,
    rng: u32,
    /// Where `rng` starts: drawn from the device's id, so two tracks playing the
    /// same patch breathe independently, and a render repeats exactly.
    seed: u32,
    has_envelope: bool,
    one_shot: bool,
    voice_mode: u8,
    glide_ms: f32,
    velocity_track: f32,
    reader_counts: Vec<usize>,
    /// One frame of node outputs, reused by every voice.
    values: Vec<f32>,
    note_controls: crate::expression::Schema,
    /// Per-sample decay of a choked envelope.
    choke_decay: f32,
    /// Graph frames per output frame, and the stages that bring them back down.
    oversample: usize,
    halvers: Vec<Halver>,
    /// Nodes evaluated each frame: all of them while a probe is heard, else
    /// only those before `probe_from`, so declaring probes costs nothing.
    evaluated: usize,
    probe_from: usize,
    /// The probe emitted, raw and summed over voices, in place of the output.
    listen: Option<Input>,
    /// Noise feeding only probes draws here, so the sounding noise is the same.
    probe_rng: u32,
}
const PROBE_SEED: u32 = 0x2718_2818;
fn input(signal: Option<&Signal>, default: f32) -> Input {
    match signal {
        None => Input::Constant(default),
        Some(Signal::Constant(n)) => Input::Constant(*n as f32),
        Some(Signal::Node(_)) => unreachable!("checked graphs resolve every input"),
        Some(Signal::Resolved(index)) => Input::Node(*index),
    }
}
impl VoicePatch {
    pub fn new(
        d: &model::Device,
        checked: &crate::patch_description::ValidatedPatch,
        c: AudioConfig,
        token: u64,
    ) -> Result<Self, DeviceError> {
        Self::prepare(d, checked, c, token).map_err(|e| {
            eprintln!("voice patch {}: {e:#}", d.id);
            DeviceError::InvalidConfig("invalid voice graph")
        })
    }
    fn prepare(
        d: &model::Device,
        checked: &crate::patch_description::ValidatedPatch,
        c: AudioConfig,
        token: u64,
    ) -> Result<Self> {
        // An oversampled graph simply runs at a higher rate: every coefficient,
        // delay length and time below is prepared for that rate.
        let oversample = checked.oversample();
        let max_frames = c.max_frames;
        let c = AudioConfig {
            sample_rate: c.sample_rate * oversample as f32,
            ..c
        };
        let mut nodes = Vec::new();
        let mut parameters = Vec::new();
        let mut delay = 0;
        let sample_budget = checked.sample_budget();
        sample::preflight(&crate::assets::paths(d), sample_budget)?;
        let mut sample_frames = 0;
        let mut assets = BTreeMap::<std::path::PathBuf, (f32, Arc<[[f32; 2]]>)>::new();
        for row in checked.nodes() {
            crate::host::check_cancelled()?;
            let i = |signal: &Option<Signal>, default| input(signal.as_ref(), default);
            let operation = match &row.operation {
                Operation::Param { value, min, max } => {
                    parameters.push(Parameter {
                        name: row.id.clone(),
                        value: value.unwrap_or(0.) as f32,
                        min: min.unwrap_or(0.) as f32,
                        max: max.unwrap_or(1.) as f32,
                    });
                    Op::Param {
                        index: parameters.len() - 1,
                    }
                }
                Operation::Frequency => Op::Frequency,
                Operation::Velocity => Op::Velocity,
                Operation::NoteAge => Op::NoteAge,
                Operation::Noise => Op::Noise,
                Operation::Expression { kind } => {
                    Op::Expression(checked.note_controls().resolve(kind)?)
                }
                Operation::Osc {
                    wave,
                    ratio,
                    detune,
                    hz,
                    fm,
                    width,
                    phase,
                } => Op::Osc {
                    wave: match wave.as_deref().unwrap_or("sine") {
                        "saw" => 1,
                        "pulse" => 2,
                        "triangle" => 3,
                        _ => 0,
                    },
                    ratio: i(ratio, 1.),
                    detune: i(detune, 0.),
                    detune_factor: match i(detune, 0.) {
                        Input::Constant(x) => Some(2f32.powf(x / 1200.)),
                        _ => None,
                    },
                    hz: hz.as_ref().map(|v| input(Some(v), 0.)),
                    fm: i(fm, 0.),
                    width: i(width, 0.5),
                    phase: i(phase, 0.),
                },
                Operation::Adsr {
                    attack,
                    decay,
                    sustain,
                    release,
                    one_shot,
                } => Op::Envelope {
                    one_shot: one_shot.unwrap_or(false),
                    attack: i(attack, 0.005),
                    decay: i(decay, 0.15),
                    sustain: i(sustain, 0.7),
                    release: i(release, 0.2),
                },
                Operation::Shape {
                    input,
                    points,
                    quality,
                } => Op::Shape {
                    input: i(input, 0.),
                    shape: shape::Shape::prepare(points, quality.as_deref())?,
                },
                Operation::Resonator {
                    input,
                    frequency,
                    decay,
                } => {
                    let frequency = i(frequency, 440.);
                    let decay = i(decay, 1.);
                    let coefficients =
                        if let (Input::Constant(f), Input::Constant(d)) = (frequency, decay) {
                            Some(resonator_coefficients(f, d, c.sample_rate))
                        } else {
                            None
                        };
                    Op::Resonator {
                        input: i(input, 0.),
                        frequency,
                        decay,
                        coefficients,
                    }
                }
                Operation::Reader {
                    zones,
                    channel,
                    speed,
                    offset,
                    end,
                    loop_crossfade,
                    ..
                } => Op::Reader {
                    reader: sample::Reader::prepare(
                        zones,
                        channel.as_deref(),
                        sample::Region {
                            offset: offset.unwrap_or(0.),
                            end: *end,
                            crossfade: loop_crossfade.unwrap_or(0.),
                        },
                        &mut assets,
                        &mut sample_frames,
                        sample_budget,
                    )?,
                    speed: i(speed, 1.),
                },
                Operation::Mseg {
                    attack,
                    release,
                    sustain,
                    one_shot,
                } => Op::Mseg(envelope::Envelope::prepare(
                    attack,
                    release,
                    *sustain,
                    one_shot.unwrap_or(false),
                    c.sample_rate,
                )?),
                Operation::Map {
                    input,
                    min,
                    max,
                    kind,
                } => Op::Map {
                    input: i(input, 0.),
                    min: i(min, 0.),
                    max: i(max, 1.),
                    kind: match kind.as_deref().unwrap_or("clamp") {
                        "abs" => 1,
                        "reciprocal" => 2,
                        "exp2" => 3,
                        "log2" => 4,
                        "sqrt" => 5,
                        _ => 0,
                    },
                },
                Operation::Hold { input, rate_hz } => Op::Hold {
                    input: i(input, 0.),
                    hz: i(rate_hz, 1.),
                },
                Operation::Slew { input, rise, fall } => Op::Slew {
                    input: i(input, 0.),
                    rise: i(rise, 0.04),
                    fall: i(fall, 0.04),
                },
                Operation::Sum { inputs } | Operation::Mul { inputs } => {
                    let inputs = inputs.iter().map(|v| input(Some(v), 0.)).collect();
                    if matches!(row.operation, Operation::Sum { .. }) {
                        Op::Sum(inputs)
                    } else {
                        Op::Product(inputs)
                    }
                }
                Operation::Drive { input, amount } => Op::Drive {
                    input: i(input, 0.),
                    amount: i(amount, 1.),
                },
                Operation::Filter {
                    input,
                    cutoff,
                    q,
                    mode,
                } => {
                    let cutoff = i(cutoff, 4000.);
                    let q = i(q, 0.707);
                    Op::Filter {
                        input: i(input, 0.),
                        cutoff,
                        q,
                        coefficients: match (cutoff, q) {
                            (Input::Constant(cutoff), Input::Constant(q)) => {
                                Some(filter_coefficients(cutoff, q, c.sample_rate))
                            }
                            _ => None,
                        },
                        mode: match mode.as_deref().unwrap_or("lowpass") {
                            "highpass" => 1,
                            "bandpass" => 2,
                            "notch" => 3,
                            "lowpass1" => 4,
                            "highpass1" => 5,
                            "allpass1" => 6,
                            _ => 0,
                        },
                    }
                }
                Operation::Delay {
                    input,
                    seconds,
                    feedback,
                    damping,
                    max_seconds,
                    max_feedback,
                    interpolation,
                } => {
                    let length =
                        (max_seconds.unwrap_or(0.25) as f32 * c.sample_rate).ceil() as usize + 4;
                    let base = delay;
                    delay += length;
                    ensure!(
                        delay <= c.sample_rate as usize * 2 + 4 * checked.nodes().len(),
                        "voice graph delay budget is two seconds per voice"
                    );
                    Op::Delay {
                        input: i(input, 0.),
                        seconds: i(seconds, 0.1),
                        feedback: i(feedback, 0.),
                        damping: damping
                            .as_ref()
                            .map(|v| super::patch::input(Some(v), 4000.)),
                        max_feedback: max_feedback.unwrap_or(0.98) as f32,
                        cubic: interpolation.as_deref() == Some("cubic"),
                        base,
                        length,
                    }
                }
                Operation::Sample {
                    path,
                    root,
                    looped,
                    channel,
                } => {
                    let path = crate::assets::resolve(&checked.module_dir().join(path))?;
                    let (rate, audio) =
                        sample::asset(&path, &mut assets, &mut sample_frames, sample_budget)?;
                    Op::Sample {
                        audio,
                        rate,
                        root: root.unwrap_or(60.) as f32,
                        looped: looped.unwrap_or(false),
                        channel: match channel.as_deref().unwrap_or("mono") {
                            "left" => 1,
                            "right" => 2,
                            _ => 0,
                        },
                    }
                }
            };
            nodes.push(operation);
        }
        let (output, right) = match checked.output() {
            Output::Mono(signal) => (input(Some(signal), 0.), None),
            Output::Stereo { left, right } => (input(Some(left), 0.), Some(input(Some(right), 0.))),
        };
        let listen = checked.listen().map(|signal| input(Some(signal), 0.));
        let evaluated = if listen.is_some() {
            nodes.len()
        } else {
            checked.probe_from()
        };
        let lifetime = checked
            .lifetime()
            .map(|(index, tail)| (index, (tail * f64::from(c.sample_rate)).ceil() as u64));
        let has_envelope = nodes
            .iter()
            .any(|n| matches!(n, Op::Envelope { .. } | Op::Mseg(_)));
        let one_shot = has_envelope
            && nodes.iter().all(|n| match n {
                Op::Envelope { one_shot, .. } => *one_shot,
                Op::Mseg(e) => e.one_shot,
                _ => true,
            });
        let count = nodes.len();
        let mut s = Self {
            core: ProcessorCore::new(d.kind, token, max_frames),
            rate: c.sample_rate,
            oversample,
            halvers: (0..oversample.trailing_zeros())
                .map(|_| Halver::new())
                .collect(),
            delays: nodes
                .iter()
                .enumerate()
                .take(evaluated)
                .filter_map(|(i, n)| matches!(n, Op::Delay { .. }).then_some(i))
                .collect(),
            evaluated,
            probe_from: checked.probe_from(),
            listen,
            probe_rng: PROBE_SEED,
            nodes,
            output,
            right,
            lifetime,
            parameters,
            voices: (0..16)
                .map(|_| PatchVoice::new(count, delay, checked.note_controls().defaults()))
                .collect(),
            gain: 0.2,
            rng: noise_seed(d.id.as_str()),
            seed: noise_seed(d.id.as_str()),
            one_shot,
            has_envelope,
            voice_mode: checked.voice_mode(),
            glide_ms: 0.,
            velocity_track: 1.,
            reader_counts: vec![0; count],
            values: vec![0.; count],
            note_controls: checked.note_controls().clone(),
            choke_decay: (-9.21 / (0.008 * c.sample_rate)).exp(),
        };
        for (k, v) in checked.controls().iter().chain(&d.params) {
            s.set_parameter(k, *v)?;
        }
        Ok(s)
    }
    fn event(&mut self, e: DeviceEventKind) {
        match e {
            DeviceEventKind::NoteOn {
                note_id,
                channel,
                pitch,
                velocity,
                elapsed_frames,
                sample_zone,
                key,
                ..
            } => {
                let continuing = if self.voice_mode != 0 {
                    self.voices
                        .iter()
                        .position(|v| v.active && v.released.is_none() && !v.choked)
                } else {
                    None
                };
                if self.voice_mode == 1
                    && let Some(index) = continuing
                {
                    let v = &mut self.voices[index];
                    v.id = note_id;
                    v.channel = channel;
                    v.velocity = velocity;
                    v.note_start = v.age;
                    v.target_pitch = pitch;
                    v.glide_left = (self.glide_ms * 0.001 * self.rate).round() as u64;
                    if v.glide_left == 0 {
                        v.pitch = pitch;
                    }
                    v.expression = *self.note_controls.defaults();
                    return;
                }
                let previous_pitch = continuing.map(|index| self.voices[index].pitch);
                let index = continuing.unwrap_or_else(|| {
                    self.voices
                        .iter()
                        .position(|v| !v.active)
                        .unwrap_or_else(|| {
                            self.voices
                                .iter()
                                .enumerate()
                                .min_by(|(_, a), (_, b)| a.last_level.total_cmp(&b.last_level))
                                .unwrap()
                                .0
                        })
                });
                let v = &mut self.voices[index];
                v.reset();
                v.active = true;
                v.id = note_id;
                v.channel = channel;
                v.pitch = previous_pitch.unwrap_or(pitch);
                v.target_pitch = pitch;
                v.glide_left = if previous_pitch.is_some() {
                    (self.glide_ms * 0.001 * self.rate).round() as u64
                } else {
                    0
                };
                if v.glide_left == 0 {
                    v.pitch = pitch;
                }
                v.velocity = velocity;
                let elapsed_frames = elapsed_frames * self.oversample as u64;
                v.age = elapsed_frames;
                v.note_start = 0;
                v.released = None;
                v.choked = false;
                v.expression = *self.note_controls.defaults();
                for (i, op) in self.nodes.iter().enumerate() {
                    if let Op::Reader { reader, .. } = op {
                        reader.start(
                            &mut v.state[i].reader,
                            sample::VoiceStart {
                                key,
                                pitch,
                                velocity,
                            },
                            sample_zone,
                            self.reader_counts[i],
                            elapsed_frames,
                            self.rate,
                        );
                        self.reader_counts[i] = self.reader_counts[i].wrapping_add(1);
                    }
                }
            }
            DeviceEventKind::NoteOff { note_id, .. } => {
                for v in &mut self.voices {
                    if v.id == note_id && v.released.is_none() {
                        v.released = Some(v.age);
                    }
                }
            }
            DeviceEventKind::NoteChoke { note_id, .. } => {
                for v in &mut self.voices {
                    if v.id == note_id {
                        v.choked = true;
                        v.released = Some(v.age);
                    }
                }
            }
            DeviceEventKind::NoteExpression {
                note_id,
                expression,
                value,
                ..
            } => {
                for v in &mut self.voices {
                    if v.id == note_id
                        && u8::try_from(expression)
                            .is_ok_and(|kind| self.note_controls.contains(kind))
                    {
                        v.expression[expression as usize] = value as f32;
                    }
                }
            }
            DeviceEventKind::Controller {
                channel,
                controller: controller @ (120 | 123),
                ..
            } => {
                for v in &mut self.voices {
                    if v.channel == channel {
                        v.choked |= controller == 120;
                        v.released = Some(v.age);
                    }
                }
            }
            DeviceEventKind::Flush => self.reset(),
            _ => {}
        }
    }
    fn frame(&mut self) -> [f32; 2] {
        let mut out = [0.; 2];
        // Every node writes its slot before a later node or a delay reads it.
        let mut values = std::mem::take(&mut self.values);
        for v in &mut self.voices {
            if !v.active {
                continue;
            }
            if v.glide_left > 0 {
                v.pitch += (v.target_pitch - v.pitch) / v.glide_left as f32;
                v.glide_left -= 1;
            }
            let (pitch, bend) = (v.pitch, v.expression[2]);
            let freq = v.frequency.get([pitch, bend], || {
                440. * 2f32.powf((pitch + bend - 69.) / 12.)
            });
            let mut env_level = 0f32;
            for (index, op) in self.nodes[..self.evaluated].iter().enumerate() {
                let state = &mut v.state[index];
                let get = |x: Input| x.get(&values);
                let x = match op {
                    Op::Param { index } => self.parameters[*index].value,
                    Op::Frequency => freq,
                    Op::Velocity => v.velocity,
                    Op::NoteAge => (v.age - v.note_start) as f32 / self.rate,
                    Op::Expression(k) => v.expression[*k as usize],
                    Op::Osc {
                        wave,
                        ratio,
                        detune,
                        detune_factor,
                        hz,
                        fm,
                        width,
                        phase,
                    } => {
                        let hz = hz.map(get).unwrap_or(freq)
                            * get(*ratio)
                            * detune_factor.unwrap_or_else(|| {
                                let cents = get(*detune);
                                state
                                    .memo
                                    .get([cents, 0.], || [2f32.powf(cents / 1200.) as f64, 0., 0.])
                                    [0] as f32
                            })
                            + get(*fm);
                        let dt = (hz / self.rate).clamp(0., 0.45);
                        let p = (state.phase as f32 + get(*phase)).rem_euclid(1.);
                        let width = get(*width).clamp(0.02, 0.98);
                        let y = match wave {
                            0 => (p * TAU).sin(),
                            1 => 2. * p - 1. - blep(p, dt),
                            2 => {
                                (if p < width { 1. } else { -1. }) + blep(p, dt)
                                    - blep((p + 1. - width) % 1., dt)
                            }
                            _ => {
                                let square = (if p < 0.5 { 1. } else { -1. }) + blep(p, dt)
                                    - blep((p + 0.5) % 1., dt);
                                state.low = dt * 4. * square + (1. - dt * 4.) * state.low;
                                state.low
                            }
                        };
                        state.phase = (state.phase + dt as f64) % 1.;
                        y
                    }
                    Op::Noise => {
                        let rng = if index < self.probe_from {
                            &mut self.rng
                        } else {
                            &mut self.probe_rng
                        };
                        *rng ^= *rng << 13;
                        *rng ^= *rng >> 17;
                        *rng ^= *rng << 5;
                        *rng as f32 / u32::MAX as f32 * 2. - 1.
                    }
                    Op::Envelope {
                        one_shot,
                        attack,
                        decay,
                        sustain,
                        release,
                    } => {
                        let a = get(*attack).clamp(0., 30.);
                        let d = get(*decay).clamp(0.0001, 60.);
                        let sustain = get(*sustain).clamp(0., 1.);
                        if v.choked {
                            state.env *= self.choke_decay;
                        } else if v.released.is_some() && !one_shot {
                            let release = get(*release).clamp(0.0001, 30.);
                            state.env *= state.memo.get([release, 0.], || {
                                [(-9.21 / (release * self.rate)).exp() as f64, 0., 0.]
                            })[0] as f32;
                        } else {
                            let t = v.age as f32 / self.rate;
                            state.env = if t < a {
                                t / a
                            } else {
                                sustain + (1. - sustain) * (-5. * (t - a) / d).exp()
                            };
                        }
                        env_level = env_level.max(state.env);
                        state.env
                    }
                    Op::Shape { input, shape } => {
                        let x = get(*input) as f64;
                        let y = if state.index == 0 {
                            shape.value(x)
                        } else {
                            shape.process(x, state.phase)
                        };
                        state.phase = x;
                        state.index = 1;
                        y as f32
                    }
                    Op::Resonator {
                        input,
                        frequency,
                        decay,
                        coefficients,
                    } => {
                        let (a, b) = coefficients.unwrap_or_else(|| {
                            let (frequency, decay) = (get(*frequency), get(*decay));
                            let [a, b, _] = state.memo.get([frequency, decay], || {
                                let (a, b) = resonator_coefficients(frequency, decay, self.rate);
                                [a, b, 0.]
                            });
                            (a, b)
                        });
                        let re = a * state.phase - b * state.imaginary + get(*input) as f64;
                        state.imaginary = b * state.phase + a * state.imaginary;
                        state.phase = re;
                        re as f32
                    }
                    Op::Reader { reader, speed } => reader.frame(
                        &mut state.reader,
                        v.expression[2] + v.pitch,
                        get(*speed),
                        v.released.is_some(),
                    ),
                    Op::Mseg(e) => {
                        let x = e.step(
                            &mut state.segment,
                            v.released.is_some(),
                            v.choked,
                            self.rate,
                        );
                        env_level = env_level.max(if state.segment.done {
                            0.
                        } else {
                            x.abs().max(1e-4)
                        });
                        x
                    }
                    Op::Map {
                        input,
                        kind,
                        min,
                        max,
                    } => {
                        let x = get(*input);
                        match kind {
                            1 => x.abs(),
                            2 => {
                                if x.abs() < 1e-20 {
                                    0.
                                } else {
                                    1. / x
                                }
                            }
                            3 => x.clamp(-100., 100.).exp2(),
                            4 => x.max(1e-20).log2(),
                            5 => x.max(0.).sqrt(),
                            _ => {
                                let a = get(*min);
                                let b = get(*max);
                                x.clamp(a.min(b), a.max(b))
                            }
                        }
                    }
                    Op::Hold { input, hz } => {
                        if state.index == 0 || state.phase >= 1. {
                            state.env = get(*input);
                            state.phase = state.phase.fract();
                            state.index = 1;
                        }
                        state.phase += (get(*hz) / self.rate).clamp(0., 1.) as f64;
                        state.env
                    }
                    Op::Slew { input, rise, fall } => {
                        let x = get(*input);
                        if state.index == 0 {
                            state.env = x;
                            state.index = 1;
                        }
                        let time = get(if x > state.env { *rise } else { *fall }).clamp(0., 60.);
                        let c = if time == 0. {
                            0.
                        } else {
                            state.memo.get([time, 0.], || {
                                [(-1. / (time * self.rate)).exp() as f64, 0., 0.]
                            })[0] as f32
                        };
                        state.env = x + (state.env - x) * c;
                        state.env
                    }
                    Op::Sum(vs) => vs.iter().map(|x| get(*x)).sum(),
                    Op::Product(vs) => vs.iter().map(|x| get(*x)).product(),
                    Op::Drive { input, amount } => {
                        (get(*input) * get(*amount).clamp(0., 100.)).tanh()
                    }
                    Op::Filter {
                        input,
                        cutoff,
                        q,
                        coefficients,
                        mode,
                    } => {
                        let (g, k, a) = coefficients.unwrap_or_else(|| {
                            let (cutoff, q) = (get(*cutoff), get(*q));
                            let [g, k, a] = state.memo.get([cutoff, q], || {
                                let (g, k, a) = filter_coefficients(cutoff, q, self.rate);
                                [g as f64, k as f64, a as f64]
                            });
                            (g as f32, k as f32, a as f32)
                        });
                        if *mode >= 4 {
                            // One pole, trapezoidal: no resonance, and a low-frequency
                            // delay of 1/(2*pi*cutoff) seconds at any sample rate.
                            let x = get(*input);
                            let step = (x - state.low) * g / (1. + g);
                            let low = state.low + step;
                            state.low = low + step;
                            let y = match mode {
                                4 => low,
                                5 => x - low,
                                _ => 2. * low - x,
                            };
                            values[index] = if y.is_finite() { y } else { 0. };
                            continue;
                        }
                        let v3 = get(*input) - state.low;
                        let band = a * state.band + g * a * v3;
                        let low = state.low + g * band;
                        state.band = 2. * band - state.band;
                        state.low = 2. * low - state.low;
                        match mode {
                            1 => get(*input) - k * band - low,
                            2 => band,
                            3 => get(*input) - k * band,
                            _ => low,
                        }
                    }
                    Op::Delay {
                        seconds,
                        feedback,
                        damping,
                        max_feedback,
                        cubic,
                        base,
                        length,
                        ..
                    } => {
                        // Position the read head in f64. In f32, a delay time just
                        // below an integer sample (e.g. 0.008 s) can make
                        // rem_euclid round up to exactly `length`, indexing past the
                        // end of the buffer. f64 keeps the fractional position and the
                        // truncating cast below safely in [0, length).
                        let line = &v.delay[*base..*base + *length];
                        let x = if *cubic {
                            // Third-order Lagrange over the samples written n-1..n+2
                            // frames ago; the slot at `state.index` is this frame's.
                            let offset =
                                (get(*seconds) * self.rate).clamp(2., (*length - 5) as f32) as f64;
                            let whole = offset.floor();
                            let f = (offset - whole) as f32;
                            let at = |k: usize| {
                                line[(state.index + *length - (whole as usize + k - 1)) % *length]
                            };
                            at(0) * (-f * (f - 1.) * (f - 2.) / 6.)
                                + at(1) * ((f + 1.) * (f - 1.) * (f - 2.) / 2.)
                                + at(2) * (-(f + 1.) * f * (f - 2.) / 2.)
                                + at(3) * ((f + 1.) * f * (f - 1.) / 6.)
                        } else {
                            let offset =
                                (get(*seconds) * self.rate).clamp(1., (*length - 4) as f32) as f64;
                            let pos_f = (state.index as f64 - offset).rem_euclid(*length as f64);
                            let i = pos_f as usize;
                            let frac = (pos_f - i as f64) as f32;
                            line[i] * (1. - frac) + line[(i + 1) % length] * frac
                        };
                        let returned = if let Some(cutoff) = damping {
                            let cutoff = get(*cutoff);
                            let a = state.memo.get([cutoff, 0.], || {
                                [
                                    (-TAU * cutoff.clamp(1., self.rate * 0.45) / self.rate).exp()
                                        as f64,
                                    0.,
                                    0.,
                                ]
                            })[0] as f32;
                            state.low = x + (state.low - x) * a;
                            state.low
                        } else {
                            x
                        };
                        // The line is written after every node has run, so its input
                        // may be a later node: that edge closes a feedback loop.
                        state.env = returned * get(*feedback).clamp(-max_feedback, *max_feedback);
                        x
                    }
                    Op::Sample {
                        audio,
                        channel,
                        rate,
                        root,
                        looped,
                    } => {
                        let pos = state.phase;
                        let i = pos as usize;
                        let x = if i + 1 < audio.len() {
                            {
                                let read = |i: usize| match channel {
                                    1 => audio[i][0],
                                    2 => audio[i][1],
                                    _ => (audio[i][0] + audio[i][1]) * 0.5,
                                };
                                read(i) + (read(i + 1) - read(i)) * (pos - i as f64) as f32
                            }
                        } else {
                            0.
                        };
                        let ratio = state.memo.get([v.pitch, v.expression[2]], || {
                            [
                                2f32.powf((v.pitch + v.expression[2] - root) / 12.) as f64,
                                0.,
                                0.,
                            ]
                        })[0] as f32;
                        state.phase += (*rate / self.rate * ratio) as f64;
                        if *looped {
                            state.phase %= audio.len() as f64;
                        }
                        x
                    }
                };
                // Signals also carry Hz, cents and envelope times. An audio-amplitude
                // clamp here corrupts otherwise valid controls (e.g. 440 Hz -> 100 Hz).
                // Consumers enforce their own domains; keep the non-finite guard.
                values[index] = if x.is_finite() { x } else { 0. };
            }
            for &index in &self.delays {
                if let Op::Delay {
                    input,
                    base,
                    length,
                    ..
                } = &self.nodes[index]
                {
                    let state = &mut v.state[index];
                    let x = input.get(&values) + state.env;
                    v.delay[base + state.index] = if x.is_finite() { x } else { 0. };
                    state.index = (state.index + 1) % length;
                }
            }
            let gate = if self.has_envelope || self.lifetime.is_some() {
                1.
            } else {
                (v.age as f32 / (self.rate * 0.003)).min(1.)
                    * v.released.map_or(1., |r| {
                        (-9.21 * (v.age - r) as f32 / (self.rate * 0.03)).exp()
                    })
            };
            let choke = if self.lifetime.is_some() && v.choked {
                (-9.21 * v.released.map_or(0, |r| v.age - r) as f32 / (0.008 * self.rate)).exp()
            } else {
                1.
            };
            let (velocity, track) = (v.velocity, self.velocity_track);
            let gain = v
                .velocity_gain
                .get([velocity, track], || velocity.powf(track))
                * self.gain
                * v.expression[0]
                * v.expression[4]
                * gate
                * choke;
            let x = self.output.get(&values) * gain;
            let pan = v.expression[1].clamp(0., 1.);
            if let Some(probe) = self.listen {
                // A probe is heard raw; voice stealing still follows the output.
                let p = probe.get(&values);
                out[0] += p;
                out[1] += p;
                v.last_level = match self.right {
                    Some(right) => x.abs().max((right.get(&values) * gain).abs()),
                    None => x.abs(),
                };
            } else if let Some(right) = self.right {
                let y = right.get(&values) * gain;
                let (l, r) = v
                    .pan
                    .get([pan], || ((2. * (1. - pan)).sqrt(), (2. * pan).sqrt()));
                out[0] += x * l;
                out[1] += y * r;
                v.last_level = x.abs().max(y.abs());
            } else {
                let (l, r) = v.pan.get([pan], || {
                    (
                        (pan * std::f32::consts::FRAC_PI_2).cos(),
                        (pan * std::f32::consts::FRAC_PI_2).sin(),
                    )
                });
                out[0] += x * l;
                out[1] += x * r;
                v.last_level = x.abs();
            }
            v.age += 1;
            if let Some((index, tail)) = self.lifetime {
                let done = match &self.nodes[index] {
                    Op::Envelope {
                        one_shot, attack, ..
                    } => {
                        (v.released.is_some()
                            || (*one_shot
                                && v.age as f32 / self.rate >= attack.get(&values).max(0.)))
                            && values[index].abs() < 1e-5
                    }
                    Op::Mseg(_) => v.state[index].segment.done,
                    Op::Reader { .. } => v.state[index].reader.done,
                    _ => false,
                };
                if v.completed.is_none() && v.age > 1 && done {
                    v.completed = Some(v.age);
                }
                if if v.choked {
                    v.released
                        .is_some_and(|at| v.age - at >= (0.01 * self.rate) as u64)
                } else {
                    v.completed.is_some_and(|at| v.age - at >= tail)
                } {
                    v.active = false;
                }
            } else if (self.one_shot || v.released.is_some())
                && v.age > 1
                && if self.has_envelope {
                    env_level < 1e-5
                } else {
                    gate < 1e-5
                }
            {
                v.active = false;
            }
        }
        self.values = values;
        out
    }
}
/// A nonzero xorshift seed from a device id (FNV-1a), so each track's noise is
/// its own: two instances of one patch in unison do not sum coherently.
fn noise_seed(id: &str) -> u32 {
    let hash = id
        .bytes()
        .fold(0x811c9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x01000193));
    match hash ^ 0x31415927 {
        0 => 0x31415927,
        seed => seed,
    }
}
fn filter_coefficients(cutoff: f32, q: f32, rate: f32) -> (f32, f32, f32) {
    let g = (std::f32::consts::PI * cutoff.clamp(10., rate * 0.45) / rate).tan();
    let k = 1. / q.clamp(0.2, 20.);
    let a = 1. / (1. + g * (g + k));
    (g, k, a)
}
fn resonator_coefficients(frequency: f32, decay: f32, rate: f32) -> (f64, f64) {
    let r = (-std::f64::consts::LN_10 * 3. / (decay.clamp(0.001, 60.) as f64 * rate as f64)).exp();
    let angle = std::f64::consts::TAU * frequency.clamp(1., rate * 0.45) as f64 / rate as f64;
    (r * angle.cos(), r * angle.sin())
}
fn blep(t: f32, dt: f32) -> f32 {
    if dt <= 0. {
        0.
    } else if t < dt {
        let x = t / dt;
        x + x - x * x - 1.
    } else if t > 1. - dt {
        let x = (t - 1.) / dt;
        x * x + x + x + 1.
    } else {
        0.
    }
}
impl DeviceProcessor for VoicePatch {
    /// Verified by the engine segmentation test.
    fn block_invariant(&self) -> bool {
        true
    }
    fn has_note(&self, note_id: u64) -> bool {
        self.voices.iter().any(|v| v.active && v.id == note_id)
    }
    fn accepts_note_expression(&self, kind: u8) -> bool {
        self.note_controls.contains(kind)
    }
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }
    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        if name == "velocity_track" {
            if !value.is_finite() || !(0.0..=2.0).contains(&value) {
                return Err(DeviceError::InvalidConfig("velocity_track must be 0..2"));
            }
            self.velocity_track = value;
            return Ok(());
        }
        if name == "glide_ms" {
            if !value.is_finite() || !(0.0..=10000.0).contains(&value) {
                return Err(DeviceError::InvalidConfig("glide_ms must be 0..10000"));
            }
            self.glide_ms = value;
            return Ok(());
        }
        if name == "gain_db" {
            self.gain = db_to_amplitude(parameter_value(self.kind(), "gain_db", value, -90., 24.)?);
            return Ok(());
        }
        let kind = self.kind();
        let p = self
            .parameters
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or(DeviceError::UnknownParameter { kind })?;
        if !value.is_finite() || !(p.min..=p.max).contains(&value) {
            return Err(DeviceError::InvalidConfig("voice parameter out of range"));
        }
        p.value = value;
        Ok(())
    }
    fn reset(&mut self) {
        for v in &mut self.voices {
            v.reset();
        }
        self.rng = self.seed;
        self.probe_rng = PROBE_SEED;
        self.reader_counts.fill(0);
        for halver in &mut self.halvers {
            halver.clear();
        }
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
        let mut event = 0;
        for frame in 0..ctx.frames {
            while event < events.len() && events[event].offset as usize == frame {
                self.event(events[event].kind);
                event += 1;
            }
            let x = match self.oversample {
                1 => self.frame(),
                2 => {
                    for _ in 0..2 {
                        let x = self.frame();
                        self.halvers[0].push(x);
                    }
                    self.halvers[0].read()
                }
                _ => {
                    for _ in 0..2 {
                        for _ in 0..2 {
                            let x = self.frame();
                            self.halvers[0].push(x);
                        }
                        let x = self.halvers[0].read();
                        self.halvers[1].push(x);
                    }
                    self.halvers[1].read()
                }
            };
            left[frame] = x[0];
            right[frame] = x[1];
        }
        Ok(())
    }
}
