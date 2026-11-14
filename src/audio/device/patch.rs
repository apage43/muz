//! A fixed per-voice graph. Nodes are prepared once into a flat schedule: those
//! whose value can change only at an event run when one arrives, the rest every
//! frame. The schedule is also compiled to native code when it can be (see
//! `jit`); this interpreter is the reference it matches bit for bit, and the
//! fallback.
use super::*;
use crate::patch_description::{Operation, Output, Signal};
use anyhow::{Result, ensure};
use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::sync::Arc;
/// The `Note` of voice `$v` at frequency `$freq`, borrowing only its expression.
macro_rules! note_of {
    ($v:expr, $freq:expr) => {
        Note {
            expression: &$v.expression,
            age: $v.age,
            note_start: $v.note_start,
            velocity: $v.velocity,
            target_frequency: $v.target_frequency,
            released: $v.released.is_some(),
            choked: $v.choked,
            pitch: $v.pitch,
            freq: $freq,
        }
    };
}
#[path = "patch_envelope.rs"]
mod envelope;
#[cfg(feature = "jit")]
#[path = "patch_jit.rs"]
mod jit;
#[path = "patch_sample.rs"]
mod sample;
#[path = "patch_shape.rs"]
mod shape;
/// Without the `jit` feature every patch is interpreted.
#[cfg(not(feature = "jit"))]
mod jit {
    pub(super) enum Compiled {}
    impl Compiled {
        pub(super) fn frame(
            &self,
            _: &[super::Step],
            _: &super::Shared,
            _: &mut super::PatchVoice,
            _: f32,
            _: [&mut u32; 2],
        ) -> f32 {
            match *self {}
        }
    }
    pub fn set_enabled(_: bool) {}
}
pub use jit::set_enabled as set_patch_jit;
#[cfg(test)]
#[path = "patch_tests.rs"]
mod tests;
/// A node input as the description gives it.
#[derive(Clone, Copy)]
enum Input {
    Constant(f32),
    Node(usize),
}
/// Where a prepared node reads an input in its voice's value table: a node's
/// output, or one of the patch's constants after the nodes.
type Slot = u32;
/// When a node's output can change. A pure node changes no more often than its
/// fastest input.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rate {
    /// Never: a pure function of constants.
    Constant,
    /// Only at an event: a note's start or legato transfer, a note expression,
    /// a parameter change, or a frame of glide. Parameters, the note's
    /// frequencies, velocity and expressions are the sources.
    Control,
    /// Every frame until it settles: a smoother (slew, filter, antialiased
    /// shape) of event-rate or settling inputs, or a pure function of such
    /// nodes. Once a whole frame of these leaves every output and state
    /// unchanged, every later frame would too, so they rest until an event.
    Settling,
    /// Every frame.
    Audio,
}
#[repr(u8)]
enum Op {
    Param(usize),
    Frequency,
    TargetFrequency,
    Velocity,
    NoteAge,
    Expression(u8),
    Osc {
        wave: u8,
        ratio: Slot,
        detune: Slot,
        detune_factor: Option<f32>,
        hz: Option<Slot>,
        fm: Slot,
        width: Slot,
        phase: Slot,
    },
    Noise {
        /// Feeds only probes: draws from the probe generator.
        probe: bool,
    },
    Envelope {
        one_shot: bool,
        attack: Slot,
        decay: Slot,
        sustain: Slot,
        release: Slot,
    },
    /// An antialiased shape: its output depends on the previous input too.
    Shape {
        input: Slot,
        shape: Box<shape::Shape>,
    },
    /// A raw shape: a pure function of its input.
    RawShape {
        input: Slot,
        shape: Box<shape::Shape>,
    },
    Resonator {
        input: Slot,
        frequency: Slot,
        decay: Slot,
        coefficients: Option<(f64, f64)>,
    },
    Mseg(Box<envelope::Envelope>),
    Reader {
        reader: Box<sample::Reader>,
        speed: Slot,
    },
    Clamp {
        input: Slot,
        min: Slot,
        max: Slot,
    },
    Abs(Slot),
    Reciprocal(Slot),
    Exp2(Slot),
    Log2(Slot),
    Sqrt(Slot),
    Hold {
        input: Slot,
        hz: Slot,
    },
    Slew {
        input: Slot,
        rise: Slot,
        fall: Slot,
    },
    /// A sum or product of one input.
    Copy(Slot),
    Add2(Slot, Slot),
    Add3(Slot, Slot, Slot),
    Mul2(Slot, Slot),
    Mul3(Slot, Slot, Slot),
    /// More inputs, at `start..start + len` in the patch's argument list.
    Sum {
        start: u32,
        len: u32,
    },
    Product {
        start: u32,
        len: u32,
    },
    Drive {
        input: Slot,
        amount: Slot,
    },
    Filter {
        input: Slot,
        cutoff: Slot,
        q: Slot,
        coefficients: Option<(f32, f32, f32)>,
        mode: u8,
    },
    /// The line's input is written after the frame (see `VoicePatch::delays`).
    Delay {
        seconds: Slot,
        feedback: Slot,
        damping: Option<Slot>,
        max_feedback: f32,
        cubic: bool,
        fixed_cubic: Option<(usize, [f32; 4])>,
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
/// One node of the schedule: its operation and the slot it writes.
struct Step {
    node: u32,
    op: Op,
}
/// Lowers description inputs to slots, collecting the patch's distinct
/// constants, and tracks each node's rate.
///
/// Every slot comes from here and is below `nodes + constants.len()`, the
/// length of each voice's value table, which is allocated once lowering is
/// complete and never resized. `run` relies on this to read slots unchecked.
struct Lowering {
    nodes: usize,
    constants: Vec<f32>,
    by_bits: BTreeMap<u32, Slot>,
    rates: Vec<Rate>,
    /// Each node's event sources (see [`Sources`]).
    sources: Vec<u64>,
    /// The sources of the inputs last passed to `pure` or `smoother`.
    last: u64,
}
impl Lowering {
    fn slot(&mut self, input: Input) -> Slot {
        match input {
            Input::Node(i) => {
                assert!(i < self.nodes, "checked graphs resolve inputs to nodes");
                i as Slot
            }
            Input::Constant(x) => *self.by_bits.entry(x.to_bits()).or_insert_with(|| {
                self.constants.push(x);
                (self.nodes + self.constants.len() - 1) as Slot
            }),
        }
    }
    fn rate(&self, input: Input) -> Rate {
        match input {
            Input::Constant(_) => Rate::Constant,
            Input::Node(i) => self.rates[i],
        }
    }
    /// The rate of a smoother of `inputs`.
    fn smoother(&mut self, inputs: &[Input]) -> Rate {
        if self.pure(inputs) == Rate::Audio {
            Rate::Audio
        } else {
            Rate::Settling
        }
    }
    /// The rate of a pure function of `inputs`.
    fn pure(&mut self, inputs: &[Input]) -> Rate {
        self.last = inputs
            .iter()
            .map(|x| match x {
                Input::Constant(_) => 0,
                Input::Node(i) => self.sources[*i],
            })
            .fold(0, |a, b| a | b);
        inputs
            .iter()
            .map(|x| self.rate(*x))
            .max()
            .unwrap_or(Rate::Constant)
    }
}
/// The event sources a node's value follows, one bit each: parameters, the
/// frequency (pitch, glide and tuning), the target frequency, velocity, then
/// each expression slot, and a voice's start. A voice records which changed; an event-rate or
/// settling node runs only when one of its own changed.
struct Sources;
impl Sources {
    const PARAMETERS: u64 = 1;
    const FREQUENCY: u64 = 2;
    const TARGET: u64 = 4;
    const VELOCITY: u64 = 8;
    /// A fresh voice: every node runs, constants included.
    const START: u64 = 1 << 63;
    const ALL: u64 = u64::MAX;
    fn expression(slot: u8) -> u64 {
        // Slot 2, tuning, moves the frequency too.
        16 << slot | if slot == 2 { Self::FREQUENCY } else { 0 }
    }
}
/// How the node that ends a voice's lifetime reports completion.
#[derive(Clone, Copy)]
enum Completion {
    Envelope { one_shot: bool, attack: Slot },
    Mseg,
    Reader,
    Never,
}
/// Halves the rate of a stereo stream through a Kaiser-windowed FIR lowpass,
/// then keeps every other frame (only kept frames are filtered).
///
/// `half_band`: 47 taps, about 80 dB down from 0.3 of the input rate; for an
/// earlier stage of 4x oversampling, whose 0.25-0.3 leakage the last stage removes.
/// `steep`: 135 taps, flat to 0.208 (20 kHz from 96 kHz) and about 85 dB down
/// from 0.25, the output's Nyquist frequency: nothing above it folds back.
///
/// A block is filtered at once. Each output frame sums its taps in order,
/// newest frame first, so the result does not depend on the block size; four
/// outputs accumulate side by side, independent chains the processor overlaps.
struct Halver {
    taps: Vec<f32>,
    /// The previous block's last `taps.len() - 1` frames, oldest first, then
    /// room for a block of input.
    history: Vec<[f32; 2]>,
}
impl Halver {
    fn half_band(block: usize) -> Self {
        Self::design(47, 0.25, 8., block)
    }
    fn steep(block: usize) -> Self {
        Self::design(135, 0.229, 8.6, block)
    }
    /// `count` odd taps of a lowpass at `cutoff` (a fraction of the input rate)
    /// under a Kaiser window of shape `beta`, for blocks of up to `block` frames.
    fn design(count: usize, cutoff: f64, beta: f64, block: usize) -> Self {
        fn bessel(x: f64) -> f64 {
            let (mut sum, mut term) = (1., 1.);
            for k in 1..40 {
                term *= (x / (2. * k as f64)).powi(2);
                sum += term;
            }
            sum
        }
        let half = (count / 2) as f64;
        let mut taps: Vec<f64> = (0..count)
            .map(|n| {
                let m = n as f64 - half;
                let ideal = if m == 0. {
                    2. * cutoff
                } else {
                    (std::f64::consts::TAU * cutoff * m).sin() / (std::f64::consts::PI * m)
                };
                ideal * bessel(beta * (1. - (m / half).powi(2)).sqrt()) / bessel(beta)
            })
            .collect();
        let dc: f64 = taps.iter().sum();
        taps.iter_mut().for_each(|t| *t /= dc);
        Self {
            taps: taps.into_iter().map(|t| t as f32).collect(),
            history: vec![[0.; 2]; count - 1 + block],
        }
    }
    /// Filter `input` and keep its odd frames into `output`, half as long.
    fn decimate(&mut self, input: &[[f32; 2]], output: &mut [[f32; 2]]) {
        let count = self.taps.len();
        let keep = count - 1;
        let len = keep + input.len();
        self.history[keep..len].copy_from_slice(input);
        let frames = &self.history[..len];
        let mut j = 0;
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx") {
            // SAFETY: the processor has AVX.
            j = unsafe { decimate_avx(&self.taps, frames, output) };
        }
        // Output `j` reads the `count` frames ending at input frame 2j + 1.
        let window = |j: usize| frames[2 * j + 1..2 * j + 1 + count].iter().rev();
        let mut chunks = output[j..].chunks_exact_mut(4);
        for chunk in &mut chunks {
            let mut out = [[0f32; 2]; 4];
            let taps = self.taps.iter().zip(
                window(j)
                    .zip(window(j + 1))
                    .zip(window(j + 2).zip(window(j + 3))),
            );
            for (tap, ((a, b), (c, d))) in taps {
                for (out, frame) in out.iter_mut().zip([a, b, c, d]) {
                    out[0] += frame[0] * tap;
                    out[1] += frame[1] * tap;
                }
            }
            chunk.copy_from_slice(&out);
            j += 4;
        }
        for frame in chunks.into_remainder() {
            let mut out = [0f32; 2];
            for (tap, x) in self.taps.iter().zip(window(j)) {
                out[0] += x[0] * tap;
                out[1] += x[1] * tap;
            }
            *frame = out;
            j += 1;
        }
        self.history.copy_within(len - keep..len, 0);
    }
    fn clear(&mut self) {
        self.history.fill([0.; 2]);
    }
}
/// `Halver::decimate`'s outputs in groups of 16, eight lanes (four outputs'
/// two channels) to a vector: each lane sums its taps in the same order with
/// the same operations, so the results are the same. Returns how many
/// outputs it wrote.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
unsafe fn decimate_avx(taps: &[f32], frames: &[[f32; 2]], output: &mut [[f32; 2]]) -> usize {
    use std::arch::x86_64::*;
    let count = taps.len();
    let groups = output.len() / 16;
    let floats = frames.as_ptr().cast::<f32>();
    for g in 0..groups {
        let j = 16 * g;
        // Accumulator `k` holds outputs j + 4k + (0, 2, 1, 3), both channels.
        let mut acc = [_mm256_setzero_ps(); 4];
        for (t, &tap) in taps.iter().enumerate() {
            let tap = _mm256_set1_ps(tap);
            // Output `j` reads frame 2j + count - t for tap `t`.
            let q = 2 * j + count - t;
            for (k, acc) in acc.iter_mut().enumerate() {
                // Frames q + 8k .. q + 8k + 3 and q + 8k + 3 .. q + 8k + 6:
                // no further than the scalar windows read.
                let at = 2 * (q + 8 * k);
                debug_assert!(at + 14 <= 2 * frames.len());
                // SAFETY: as asserted.
                let (a, b) = unsafe {
                    let at = floats.add(at);
                    (_mm256_loadu_ps(at), _mm256_loadu_ps(at.add(6)))
                };
                // Frames q + 8k and + 4 in the low half, + 2 and + 6 in the high.
                let x = _mm256_shuffle_ps::<0b11_10_01_00>(a, b);
                *acc = _mm256_add_ps(*acc, _mm256_mul_ps(x, tap));
            }
        }
        for (k, acc) in acc.iter().enumerate() {
            let mut lanes = [0f32; 8];
            // SAFETY: `lanes` holds eight floats.
            unsafe { _mm256_storeu_ps(lanes.as_mut_ptr(), *acc) };
            let at = j + 4 * k;
            output[at] = [lanes[0], lanes[1]];
            output[at + 2] = [lanes[2], lanes[3]];
            output[at + 1] = [lanes[4], lanes[5]];
            output[at + 3] = [lanes[6], lanes[7]];
        }
    }
    16 * groups
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
///
/// Compiled patches read and refill it in place (see `jit`), so its fields
/// are plain: `valid`, then the key's bits.
#[derive(Clone, Copy)]
struct Memo<T: Copy + Default, const K: usize> {
    valid: bool,
    key: [u32; K],
    value: T,
}
impl<T: Copy + Default, const K: usize> Default for Memo<T, K> {
    fn default() -> Self {
        Self {
            valid: false,
            key: [0; K],
            value: T::default(),
        }
    }
}
impl<T: Copy + Default, const K: usize> Memo<T, K> {
    #[inline]
    fn get(&mut self, inputs: [f32; K], compute: impl FnOnce() -> T) -> T {
        let key = inputs.map(f32::to_bits);
        if !self.valid || self.key != key {
            self.value = compute();
            self.key = key;
            self.valid = true;
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
    /// Compiled patches only: its cluster's last run changed no smoother's
    /// output or state, so the cluster rests until an input moves.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    resting: bool,
}
struct PatchVoice {
    active: bool,
    /// Event sources changed since the event-rate nodes last ran.
    changed: u64,
    /// Sources whose settling nodes have not come to rest; the others' last
    /// frame changed nothing.
    unsettled: u64,
    id: u64,
    channel: u8,
    pitch: f32,
    target_pitch: f32,
    target_frequency: f32,
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
    /// Every node's latest output, then the patch's constants. Event-rate
    /// outputs persist here between the events that change them.
    values: Vec<f32>,
    delay: Vec<f32>,
    last_level: f32,
    completed: Option<u64>,
    frequency: Memo<f32, 2>,
    velocity_gain: Memo<f32, 2>,
    pan: Memo<(f32, f32), 1>,
    /// The frequency of `pitch` and the tuning expression (see `retune`).
    freq: f32,
    /// The velocity's gain times the device's and the volume and level
    /// expressions', and the pan's left and right gains (see `remix`).
    gain: f32,
    pan_gains: (f32, f32),
}
impl PatchVoice {
    fn new(
        nodes: usize,
        constants: &[f32],
        delay: usize,
        defaults: &[f32; crate::expression::SLOT_COUNT],
    ) -> Self {
        let mut values = vec![0.; nodes + constants.len()];
        values[nodes..].copy_from_slice(constants);
        Self {
            active: false,
            changed: Sources::ALL,
            unsettled: Sources::ALL,
            id: 0,
            channel: 0,
            pitch: 60.,
            target_pitch: 60.,
            target_frequency: 261.62555,
            glide_left: 0,
            velocity: 0.,
            age: 0,
            note_start: 0,
            released: None,
            choked: false,
            expression: *defaults,
            state: vec![State::default(); nodes],
            values,
            delay: vec![0.; delay],
            last_level: 0.,
            completed: None,
            frequency: Memo::default(),
            velocity_gain: Memo::default(),
            pan: Memo::default(),
            freq: 0.,
            gain: 0.,
            pan_gains: (0., 0.),
        }
    }
    /// Follow a change of `pitch` or the tuning expression.
    fn retune(&mut self) {
        let (pitch, bend) = (self.pitch, self.expression[2]);
        self.freq = self.frequency.get([pitch, bend], || {
            440. * 2f32.powf((pitch + bend - 69.) / 12.)
        });
    }
    /// Follow a change of the velocity, the device's gain or velocity
    /// tracking, or the volume, pan or level expressions. `stereo`: whether
    /// the patch has a right output.
    fn remix(&mut self, gain: f32, track: f32, stereo: bool) {
        let velocity = self.velocity;
        self.gain = self
            .velocity_gain
            .get([velocity, track], || velocity.powf(track))
            * gain
            * self.expression[0]
            * self.expression[4];
        let pan = self.expression[1].clamp(0., 1.);
        self.pan_gains = if stereo {
            self.pan
                .get([pan], || ((2. * (1. - pan)).sqrt(), (2. * pan).sqrt()))
        } else {
            self.pan.get([pan], || {
                (
                    (pan * std::f32::consts::FRAC_PI_2).cos(),
                    (pan * std::f32::consts::FRAC_PI_2).sin(),
                )
            })
        };
    }
    fn reset(&mut self) {
        self.active = false;
        self.changed = Sources::ALL;
        self.completed = None;
        self.delay.fill(0.);
        self.state.fill(State::default());
    }
}
pub struct VoicePatch {
    core: ProcessorCore,
    rate: f32,
    /// Nodes evaluated each frame, in graph order: the event-rate ones
    /// (`..control`) whenever a voice is dirty, then the rest. Only nodes before
    /// the probes run, unless a probe is heard.
    steps: Vec<Step>,
    control: usize,
    /// `control..settling` are the settling nodes, run while a voice is unsettled.
    settling: usize,
    /// The event sources of the steps before `settling`.
    sources: Vec<u64>,
    /// Inputs of sums and products of more than three.
    args: Vec<Slot>,
    /// Delay lines, written once the frame's signals are known: the node, the
    /// line's input, its base and length.
    delays: Vec<(usize, Slot, usize, usize)>,
    /// Steps that read samples.
    readers: Vec<usize>,
    output: Slot,
    right: Option<Slot>,
    lifetime: Option<(usize, Completion, u64)>,
    parameters: Vec<Parameter>,
    voices: Vec<PatchVoice>,
    /// The active voices, one bit each.
    sounding: u32,
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
    note_controls: crate::expression::Schema,
    /// Per-sample decay of a choked envelope.
    choke_decay: f32,
    /// Graph frames per output frame, and the stages that bring them back down.
    oversample: usize,
    halvers: Vec<Halver>,
    /// Graph frames of a block and the first stage's output, when oversampled.
    staging: [Vec<[f32; 2]>; 2],
    /// The probe emitted, raw and summed over voices, in place of the output.
    listen: Option<Slot>,
    /// Noise feeding only probes draws here, so the sounding noise is the same.
    probe_rng: u32,
    /// The audio-rate steps and delay writes as native code, unless this patch
    /// is interpreted.
    compiled: Option<jit::Compiled>,
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
        let count = checked.nodes().len();
        let mut l = Lowering {
            nodes: count,
            constants: Vec::new(),
            by_bits: BTreeMap::new(),
            rates: Vec::with_capacity(count),
            sources: Vec::with_capacity(count),
            last: 0,
        };
        let mut ops = Vec::with_capacity(count);
        let mut args = Vec::new();
        let mut delay_inputs = Vec::new();
        let mut parameters = Vec::new();
        let mut delay = 0;
        let sample_budget = checked.sample_budget();
        sample::preflight(&crate::assets::paths(d), sample_budget)?;
        let mut sample_frames = 0;
        let mut assets = BTreeMap::<std::path::PathBuf, (f32, Arc<[[f32; 2]]>)>::new();
        for (index, row) in checked.nodes().iter().enumerate() {
            crate::host::check_cancelled()?;
            let i = |signal: &Option<Signal>, default| input(signal.as_ref(), default);
            let (operation, rate) = match &row.operation {
                Operation::Param { value, min, max } => {
                    parameters.push(Parameter {
                        name: row.id.clone(),
                        value: value.unwrap_or(0.) as f32,
                        min: min.unwrap_or(0.) as f32,
                        max: max.unwrap_or(1.) as f32,
                    });
                    (Op::Param(parameters.len() - 1), Rate::Control)
                }
                Operation::Frequency => (Op::Frequency, Rate::Control),
                Operation::TargetFrequency => (Op::TargetFrequency, Rate::Control),
                Operation::Velocity => (Op::Velocity, Rate::Control),
                Operation::NoteAge => (Op::NoteAge, Rate::Audio),
                Operation::Noise => (
                    Op::Noise {
                        probe: index >= checked.probe_from(),
                    },
                    Rate::Audio,
                ),
                Operation::Expression { kind } => (
                    Op::Expression(checked.note_controls().resolve(kind)?),
                    Rate::Control,
                ),
                Operation::Osc {
                    wave,
                    ratio,
                    detune,
                    hz,
                    fm,
                    width,
                    phase,
                } => {
                    let detune = i(detune, 0.);
                    let op = Op::Osc {
                        wave: match wave.as_deref().unwrap_or("sine") {
                            "saw" => 1,
                            "pulse" => 2,
                            "triangle" => 3,
                            _ => 0,
                        },
                        ratio: l.slot(i(ratio, 1.)),
                        detune: l.slot(detune),
                        detune_factor: match detune {
                            Input::Constant(x) => Some(2f32.powf(x / 1200.)),
                            _ => None,
                        },
                        hz: hz.as_ref().map(|v| l.slot(input(Some(v), 0.))),
                        fm: l.slot(i(fm, 0.)),
                        width: l.slot(i(width, 0.5)),
                        phase: l.slot(i(phase, 0.)),
                    };
                    (op, Rate::Audio)
                }
                Operation::Adsr {
                    attack,
                    decay,
                    sustain,
                    release,
                    one_shot,
                } => (
                    Op::Envelope {
                        one_shot: one_shot.unwrap_or(false),
                        attack: l.slot(i(attack, 0.005)),
                        decay: l.slot(i(decay, 0.15)),
                        sustain: l.slot(i(sustain, 0.7)),
                        release: l.slot(i(release, 0.2)),
                    },
                    Rate::Audio,
                ),
                Operation::Shape {
                    input,
                    points,
                    quality,
                } => {
                    let x = i(input, 0.);
                    let shape = Box::new(shape::Shape::prepare(points, quality.as_deref())?);
                    let input = l.slot(x);
                    if shape.antialias {
                        (Op::Shape { input, shape }, l.smoother(&[x]))
                    } else {
                        (Op::RawShape { input, shape }, l.pure(&[x]))
                    }
                }
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
                    let op = Op::Resonator {
                        input: l.slot(i(input, 0.)),
                        frequency: l.slot(frequency),
                        decay: l.slot(decay),
                        coefficients,
                    };
                    (op, Rate::Audio)
                }
                Operation::Reader {
                    zones,
                    channel,
                    speed,
                    offset,
                    end,
                    loop_crossfade,
                    ..
                } => {
                    let op = Op::Reader {
                        reader: Box::new(sample::Reader::prepare(
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
                        )?),
                        speed: l.slot(i(speed, 1.)),
                    };
                    (op, Rate::Audio)
                }
                Operation::Mseg {
                    attack,
                    release,
                    sustain,
                    one_shot,
                } => (
                    Op::Mseg(Box::new(envelope::Envelope::prepare(
                        attack,
                        release,
                        *sustain,
                        one_shot.unwrap_or(false),
                        c.sample_rate,
                    )?)),
                    Rate::Audio,
                ),
                Operation::Map {
                    input,
                    min,
                    max,
                    kind,
                } => {
                    let x = i(input, 0.);
                    let slot = l.slot(x);
                    match kind.as_deref().unwrap_or("clamp") {
                        "abs" => (Op::Abs(slot), l.pure(&[x])),
                        "reciprocal" => (Op::Reciprocal(slot), l.pure(&[x])),
                        "exp2" => (Op::Exp2(slot), l.pure(&[x])),
                        "log2" => (Op::Log2(slot), l.pure(&[x])),
                        "sqrt" => (Op::Sqrt(slot), l.pure(&[x])),
                        _ => {
                            let (min, max) = (i(min, 0.), i(max, 1.));
                            let op = Op::Clamp {
                                input: slot,
                                min: l.slot(min),
                                max: l.slot(max),
                            };
                            (op, l.pure(&[x, min, max]))
                        }
                    }
                }
                Operation::Hold { input, rate_hz } => (
                    Op::Hold {
                        input: l.slot(i(input, 0.)),
                        hz: l.slot(i(rate_hz, 1.)),
                    },
                    Rate::Audio,
                ),
                Operation::Slew { input, rise, fall } => {
                    let inputs = [i(input, 0.), i(rise, 0.04), i(fall, 0.04)];
                    let op = Op::Slew {
                        input: l.slot(inputs[0]),
                        rise: l.slot(inputs[1]),
                        fall: l.slot(inputs[2]),
                    };
                    (op, l.smoother(&inputs))
                }
                Operation::Sum { inputs } | Operation::Mul { inputs } => {
                    let inputs: Vec<_> = inputs.iter().map(|v| input(Some(v), 0.)).collect();
                    let slots: Vec<_> = inputs.iter().map(|x| l.slot(*x)).collect();
                    let sum = matches!(row.operation, Operation::Sum { .. });
                    let op = match (sum, &slots[..]) {
                        (_, [a]) => Op::Copy(*a),
                        (true, [a, b]) => Op::Add2(*a, *b),
                        (true, [a, b, c]) => Op::Add3(*a, *b, *c),
                        (false, [a, b]) => Op::Mul2(*a, *b),
                        (false, [a, b, c]) => Op::Mul3(*a, *b, *c),
                        _ => {
                            let (start, len) = (args.len() as u32, slots.len() as u32);
                            args.extend_from_slice(&slots);
                            if sum {
                                Op::Sum { start, len }
                            } else {
                                Op::Product { start, len }
                            }
                        }
                    };
                    (op, l.pure(&inputs))
                }
                Operation::Drive { input, amount } => {
                    let (x, amount) = (i(input, 0.), i(amount, 1.));
                    let op = Op::Drive {
                        input: l.slot(x),
                        amount: l.slot(amount),
                    };
                    (op, l.pure(&[x, amount]))
                }
                Operation::Filter {
                    input,
                    cutoff,
                    q,
                    mode,
                } => {
                    let cutoff = i(cutoff, 4000.);
                    let q = i(q, 0.707);
                    let x = i(input, 0.);
                    let rate = l.smoother(&[x, cutoff, q]);
                    let op = Op::Filter {
                        input: l.slot(x),
                        cutoff: l.slot(cutoff),
                        q: l.slot(q),
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
                    };
                    (op, rate)
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
                    let seconds = i(seconds, 0.1);
                    let cubic = interpolation.as_deref() == Some("cubic");
                    let fixed_cubic = match seconds {
                        Input::Constant(seconds) if cubic => {
                            Some(cubic_taps(seconds, c.sample_rate, length))
                        }
                        _ => None,
                    };
                    delay_inputs.push((index, l.slot(i(input, 0.))));
                    let op = Op::Delay {
                        seconds: l.slot(seconds),
                        feedback: l.slot(i(feedback, 0.)),
                        damping: damping
                            .as_ref()
                            .map(|v| l.slot(self::input(Some(v), 4000.))),
                        max_feedback: max_feedback.unwrap_or(0.98) as f32,
                        cubic,
                        fixed_cubic,
                        base,
                        length,
                    };
                    (op, Rate::Audio)
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
                    let op = Op::Sample {
                        audio,
                        rate,
                        root: root.unwrap_or(60.) as f32,
                        looped: looped.unwrap_or(false),
                        channel: match channel.as_deref().unwrap_or("mono") {
                            "left" => 1,
                            "right" => 2,
                            _ => 0,
                        },
                    };
                    (op, Rate::Audio)
                }
            };
            let sources = match &operation {
                Op::Param(_) => Sources::PARAMETERS,
                Op::Frequency => Sources::FREQUENCY,
                Op::TargetFrequency => Sources::TARGET,
                Op::Velocity => Sources::VELOCITY,
                Op::Expression(slot) => Sources::expression(*slot),
                _ if rate == Rate::Audio => Sources::ALL,
                _ => l.last | Sources::START,
            };
            l.last = 0;
            l.sources.push(sources);
            l.rates.push(rate);
            ops.push(operation);
        }
        let (output, right) = match checked.output() {
            Output::Mono(signal) => (l.slot(input(Some(signal), 0.)), None),
            Output::Stereo { left, right } => (
                l.slot(input(Some(left), 0.)),
                Some(l.slot(input(Some(right), 0.))),
            ),
        };
        let listen = checked
            .listen()
            .map(|signal| l.slot(input(Some(signal), 0.)));
        let evaluated = if listen.is_some() {
            count
        } else {
            checked.probe_from()
        };
        let lifetime = checked.lifetime().map(|(index, tail)| {
            let completion = match &ops[index] {
                Op::Envelope {
                    one_shot, attack, ..
                } => Completion::Envelope {
                    one_shot: *one_shot,
                    attack: *attack,
                },
                Op::Mseg(_) => Completion::Mseg,
                Op::Reader { .. } => Completion::Reader,
                _ => Completion::Never,
            };
            (
                index,
                completion,
                (tail * f64::from(c.sample_rate)).ceil() as u64,
            )
        });
        let has_envelope = ops
            .iter()
            .any(|n| matches!(n, Op::Envelope { .. } | Op::Mseg(_)));
        let one_shot = has_envelope
            && ops.iter().all(|n| match n {
                Op::Envelope { one_shot, .. } => *one_shot,
                Op::Mseg(e) => e.one_shot,
                _ => true,
            });
        // Event-rate nodes depend only on each other and constants, so running
        // them first keeps every input written before it is read.
        // Settling nodes depend only on event-rate and settling nodes.
        let mut control = Vec::new();
        let mut settling = Vec::new();
        let mut audio = Vec::new();
        for (node, op) in ops.into_iter().enumerate().take(evaluated) {
            let step = Step {
                node: node as u32,
                op,
            };
            match l.rates[node] {
                Rate::Constant | Rate::Control => control.push(step),
                Rate::Settling => settling.push(step),
                Rate::Audio => audio.push(step),
            }
        }
        let control_steps = control.len();
        let settling_steps = control_steps + settling.len();
        let steps: Vec<Step> = control.into_iter().chain(settling).chain(audio).collect();
        let sources = steps[..settling_steps]
            .iter()
            .map(|s| l.sources[s.node as usize])
            .collect();
        let readers = steps
            .iter()
            .enumerate()
            .filter_map(|(i, s)| matches!(s.op, Op::Reader { .. }).then_some(i))
            .collect();
        let mut s = Self {
            core: ProcessorCore::new(d.kind, token, max_frames),
            rate: c.sample_rate,
            oversample,
            // The last stage sets the output's band edge; an earlier one only
            // has to keep its leakage above what the last removes.
            halvers: (0..oversample.trailing_zeros())
                .map(|k| {
                    let block = max_frames * (oversample >> k);
                    if k + 1 == oversample.trailing_zeros() {
                        Halver::steep(block)
                    } else {
                        Halver::half_band(block)
                    }
                })
                .collect(),
            staging: if oversample > 1 {
                [
                    vec![[0.; 2]; max_frames * oversample],
                    vec![[0.; 2]; max_frames * oversample / 2],
                ]
            } else {
                [Vec::new(), Vec::new()]
            },
            delays: delay_inputs
                .into_iter()
                .filter(|(node, _)| *node < evaluated)
                .map(|(node, input)| {
                    match &steps.iter().find(|s| s.node as usize == node).unwrap().op {
                        Op::Delay { base, length, .. } => (node, input, *base, *length),
                        _ => unreachable!("delay inputs belong to delay nodes"),
                    }
                })
                .collect(),
            steps,
            control: control_steps,
            settling: settling_steps,
            sources,
            args,
            readers,
            listen,
            probe_rng: PROBE_SEED,
            output,
            right,
            lifetime,
            parameters,
            sounding: 0,
            voices: (0..16)
                .map(|_| {
                    PatchVoice::new(
                        count,
                        &l.constants,
                        delay,
                        checked.note_controls().defaults(),
                    )
                })
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
            note_controls: checked.note_controls().clone(),
            choke_decay: (-9.21 / (0.008 * c.sample_rate)).exp(),
            compiled: None,
        };
        for (k, v) in checked.controls().iter().chain(&d.params) {
            s.set_parameter(k, *v)?;
        }
        #[cfg(feature = "jit")]
        if jit::enabled() {
            s.compiled = jit::compile(&s, count, &l.constants)
                .inspect_err(jit::unavailable)
                .ok();
        }
        Ok(s)
    }
    /// Every voice follows a change of the device's gain or velocity tracking.
    fn remix(&mut self) {
        let stereo = self.right.is_some();
        for v in &mut self.voices {
            v.remix(self.gain, self.velocity_track, stereo);
        }
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
                    v.changed = Sources::ALL;
                    v.id = note_id;
                    v.channel = channel;
                    v.velocity = velocity;
                    v.note_start = v.age;
                    v.target_pitch = pitch;
                    v.target_frequency = 440. * 2f32.powf((pitch - 69.) / 12.);
                    v.glide_left = (self.glide_ms * 0.001 * self.rate).round() as u64;
                    if v.glide_left == 0 {
                        v.pitch = pitch;
                    }
                    v.expression = *self.note_controls.defaults();
                    v.retune();
                    v.remix(self.gain, self.velocity_track, self.right.is_some());
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
                self.sounding |= 1 << index;
                v.id = note_id;
                v.channel = channel;
                v.pitch = previous_pitch.unwrap_or(pitch);
                v.target_pitch = pitch;
                v.target_frequency = 440. * 2f32.powf((pitch - 69.) / 12.);
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
                v.retune();
                v.remix(self.gain, self.velocity_track, self.right.is_some());
                for &step in &self.readers {
                    let step = &self.steps[step];
                    if let Op::Reader { reader, .. } = &step.op {
                        let node = step.node as usize;
                        reader.start(
                            &mut v.state[node].reader,
                            sample::VoiceStart {
                                key,
                                pitch,
                                velocity,
                            },
                            sample_zone,
                            self.reader_counts[node],
                            elapsed_frames,
                            self.rate,
                        );
                        self.reader_counts[node] = self.reader_counts[node].wrapping_add(1);
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
                        let slot = &mut v.expression[expression as usize];
                        if slot.to_bits() != (value as f32).to_bits() {
                            *slot = value as f32;
                            v.changed |= Sources::expression(expression as u8);
                            v.retune();
                            v.remix(self.gain, self.velocity_track, self.right.is_some());
                        }
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
        let shared = Shared {
            parameters: &self.parameters,
            args: &self.args,
            rate: self.rate,
            choke_decay: self.choke_decay,
        };
        let (control, rest) = self.steps.split_at(self.control);
        let (settling, audio) = rest.split_at(self.settling - self.control);
        let mut voices = self.sounding;
        while voices != 0 {
            let voice = voices.trailing_zeros() as usize;
            voices &= voices - 1;
            let v = &mut self.voices[voice];
            if v.glide_left > 0 {
                v.pitch += (v.target_pitch - v.pitch) / v.glide_left as f32;
                v.glide_left -= 1;
                v.changed |= Sources::FREQUENCY;
                v.retune();
            }
            let freq = v.freq;
            let rngs = [&mut self.rng, &mut self.probe_rng];
            let env_level = if let Some(compiled) = &self.compiled {
                compiled.frame(&self.steps, &shared, v, freq, rngs)
            } else {
                interpret(
                    [control, settling, audio],
                    &self.sources,
                    &self.delays,
                    &shared,
                    v,
                    freq,
                    rngs,
                )
            };
            let values = &v.values;
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
            let gain = v.gain * gate * choke;
            let x = values[self.output as usize] * gain;
            if let Some(probe) = self.listen {
                // A probe is heard raw; voice stealing still follows the output.
                let p = values[probe as usize];
                out[0] += p;
                out[1] += p;
                v.last_level = match self.right {
                    Some(right) => x.abs().max((values[right as usize] * gain).abs()),
                    None => x.abs(),
                };
            } else if let Some(right) = self.right {
                let y = values[right as usize] * gain;
                let (l, r) = v.pan_gains;
                out[0] += x * l;
                out[1] += y * r;
                v.last_level = x.abs().max(y.abs());
            } else {
                let (l, r) = v.pan_gains;
                out[0] += x * l;
                out[1] += x * r;
                v.last_level = x.abs();
            }
            v.age += 1;
            if let Some((index, completion, tail)) = self.lifetime {
                let done = match completion {
                    Completion::Envelope { one_shot, attack } => {
                        (v.released.is_some()
                            || (one_shot
                                && v.age as f32 / self.rate >= v.values[attack as usize].max(0.)))
                            && v.values[index].abs() < 1e-5
                    }
                    Completion::Mseg => v.state[index].segment.done,
                    Completion::Reader => v.state[index].reader.done,
                    Completion::Never => false,
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
                    self.sounding &= !(1 << voice);
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
                self.sounding &= !(1 << voice);
            }
        }
        out
    }
}
/// What every step of a frame shares.
struct Shared<'a> {
    parameters: &'a [Parameter],
    args: &'a [Slot],
    rate: f32,
    choke_decay: f32,
}
/// One voice's frame as its steps see it.
struct Env<'a> {
    values: &'a mut [f32],
    state: &'a mut [State],
    delay: &'a mut [f32],
    note: Note<'a>,
    /// The sounding and the probe noise generators.
    rngs: [&'a mut u32; 2],
    /// The loudest envelope this frame.
    env_level: f32,
}
/// What a frame's steps read of their voice's note.
struct Note<'a> {
    expression: &'a [f32; crate::expression::SLOT_COUNT],
    age: u64,
    note_start: u64,
    velocity: f32,
    target_frequency: f32,
    released: bool,
    choked: bool,
    pitch: f32,
    freq: f32,
}

/// The state a smoother carries between frames.
fn smoother_state(s: &State) -> [u64; 4] {
    [
        s.phase.to_bits(),
        u64::from(s.env.to_bits()) << 32 | u64::from(s.low.to_bits()),
        u64::from(s.band.to_bits()),
        s.index as u64,
    ]
}
/// Run `steps` for one frame of voice `e`: those whose `sources` (if given)
/// meet `active`. With `SETTLE`, returns whether the frame left every output
/// and state it ran as it was.
#[inline(always)]
fn run<const SETTLE: bool>(
    steps: &[Step],
    sources: &[u64],
    active: u64,
    c: &Shared,
    e: &mut Env,
) -> bool {
    let mut unchanged = true;
    // The voice's tables stay in registers across the loop.
    let Env {
        ref mut values,
        ref mut state,
        ref delay,
        ref note,
        ref mut rngs,
        ref mut env_level,
    } = *e;
    for (k, step) in steps.iter().enumerate() {
        if !sources.is_empty() && sources[k] & active == 0 {
            continue;
        }
        let index = step.node as usize;
        let before = if SETTLE {
            smoother_state(&state[index])
        } else {
            [0; 4]
        };
        let x = eval(
            &step.op, index, c, values, state, delay, note, rngs, env_level,
        );
        // Signals also carry Hz, cents and envelope times. An audio-amplitude
        // clamp here corrupts otherwise valid controls (e.g. 440 Hz -> 100 Hz).
        // Consumers enforce their own domains; keep the non-finite guard.
        let x = if x.is_finite() { x } else { 0. };
        if SETTLE {
            unchanged &=
                x.to_bits() == values[index].to_bits() && smoother_state(&state[index]) == before;
        }
        values[index] = x;
    }
    unchanged
}
/// One frame of voice `v`'s schedule, interpreted: the event-rate steps whose
/// sources changed, the settling steps that have not come to rest, the
/// audio-rate steps and the delay writes. Returns the loudest envelope.
#[inline(never)]
fn interpret(
    [control, settling, audio]: [&[Step]; 3],
    sources: &[u64],
    delays: &[(usize, Slot, usize, usize)],
    c: &Shared,
    v: &mut PatchVoice,
    freq: f32,
    rngs: [&mut u32; 2],
) -> f32 {
    let mut e = Env {
        note: note_of!(v, freq),
        values: &mut v.values,
        state: &mut v.state,
        delay: &mut v.delay,
        rngs,
        env_level: 0.,
    };
    if v.changed != 0 {
        let changed = v.changed;
        run::<false>(control, &sources[..control.len()], changed, c, &mut e);
        v.unsettled |= changed;
        v.changed = 0;
    }
    if v.unsettled != 0 {
        let unsettled = v.unsettled;
        let sources = &sources[control.len()..];
        if run::<true>(settling, sources, unsettled, c, &mut e) {
            v.unsettled = 0;
        }
    }
    run::<false>(audio, &[], Sources::ALL, c, &mut e);
    write_delays(delays, &mut e);
    e.env_level
}
/// Write each delay line's input, once the frame's signals are known.
#[inline(always)]
fn write_delays(delays: &[(usize, Slot, usize, usize)], e: &mut Env) {
    for &(index, input, base, length) in delays {
        let state = &mut e.state[index];
        let x = e.values[input as usize] + state.env;
        e.delay[base + state.index] = if x.is_finite() { x } else { 0. };
        state.index = (state.index + 1) % length;
    }
}
impl State {
    /// A filter's coefficients for a cutoff and Q that are not constants.
    #[inline(always)]
    fn filter_memo(&mut self, cutoff: f32, q: f32, rate: f32) -> (f32, f32, f32) {
        let [g, k, a] = self.memo.get([cutoff, q], || {
            let (g, k, a) = filter_coefficients(cutoff, q, rate);
            [g as f64, k as f64, a as f64]
        });
        (g as f32, k as f32, a as f32)
    }
    /// A delay's feedback lowpass pole for a damping cutoff.
    #[inline(always)]
    fn damping_memo(&mut self, cutoff: f32, rate: f32) -> f32 {
        self.memo.get([cutoff, 0.], || {
            [
                (-TAU * cutoff.clamp(1., rate * 0.45) / rate).exp() as f64,
                0.,
                0.,
            ]
        })[0] as f32
    }
    /// An oscillator's frequency factor for a detune in cents.
    #[inline(always)]
    fn detune_memo(&mut self, cents: f32) -> f32 {
        self.memo
            .get([cents, 0.], || [2f32.powf(cents / 1200.) as f64, 0., 0.])[0] as f32
    }
    /// An envelope's per-frame release factor for a release time.
    #[inline(always)]
    fn release_memo(&mut self, release: f32, rate: f32) -> f32 {
        self.memo.get([release, 0.], || {
            [(-9.21 / (release * rate)).exp() as f64, 0., 0.]
        })[0] as f32
    }
    /// A resonator's coefficients for a frequency and decay that are not
    /// constants.
    #[inline(always)]
    fn resonator_memo(&mut self, frequency: f32, decay: f32, rate: f32) -> (f64, f64) {
        let [a, b, _] = self.memo.get([frequency, decay], || {
            let (a, b) = resonator_coefficients(frequency, decay, rate);
            [a, b, 0.]
        });
        (a, b)
    }
    /// A slew's pole for a nonzero time constant.
    #[inline(always)]
    fn slew_memo(&mut self, time: f32, rate: f32) -> f32 {
        self.memo
            .get([time, 0.], || [(-1. / (time * rate)).exp() as f64, 0., 0.])[0] as f32
    }
}
/// One step's output for a voice, before the non-finite guard.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn eval(
    op: &Op,
    index: usize,
    c: &Shared,
    values: &mut [f32],
    state: &mut [State],
    delay: &[f32],
    note: &Note,
    rngs: &mut [&mut u32; 2],
    env_level: &mut f32,
) -> f32 {
    let Note {
        expression,
        age,
        note_start,
        velocity,
        target_frequency,
        released,
        choked,
        pitch,
        freq,
    } = *note;
    // SAFETY: every slot indexes the value table (see `Lowering`).
    let get = |x: Slot| unsafe { *values.get_unchecked(x as usize) };
    match op {
        Op::Param(index) => c.parameters[*index].value,
        Op::Frequency => freq,
        Op::TargetFrequency => target_frequency,
        Op::Velocity => velocity,
        Op::NoteAge => (age - note_start) as f32 / c.rate,
        Op::Expression(k) => expression[*k as usize],
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
            let s = &mut state[index];
            let hz = hz.map(get).unwrap_or(freq)
                * get(*ratio)
                * detune_factor.unwrap_or_else(|| s.detune_memo(get(*detune)))
                + get(*fm);
            let dt = (hz / c.rate).clamp(0., 0.45);
            let p = (s.phase as f32 + get(*phase)).rem_euclid(1.);
            let width = get(*width).clamp(0.02, 0.98);
            let y = match wave {
                0 => (p * TAU).sin(),
                1 => 2. * p - 1. - blep(p, dt),
                2 => {
                    (if p < width { 1. } else { -1. }) + blep(p, dt)
                        - blep((p + 1. - width) % 1., dt)
                }
                _ => {
                    let square =
                        (if p < 0.5 { 1. } else { -1. }) + blep(p, dt) - blep((p + 0.5) % 1., dt);
                    s.low = dt * 4. * square + (1. - dt * 4.) * s.low;
                    s.low
                }
            };
            s.phase = (s.phase + dt as f64) % 1.;
            y
        }
        Op::Noise { probe } => {
            let rng = &mut *rngs[usize::from(*probe)];
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
            let s = &mut state[index];
            let a = get(*attack).clamp(0., 30.);
            let d = get(*decay).clamp(0.0001, 60.);
            let sustain = get(*sustain).clamp(0., 1.);
            if choked {
                s.env *= c.choke_decay;
            } else if released && !one_shot {
                s.env *= s.release_memo(get(*release).clamp(0.0001, 30.), c.rate);
            } else {
                let t = age as f32 / c.rate;
                s.env = if t < a {
                    t / a
                } else {
                    sustain + (1. - sustain) * (-5. * (t - a) / d).exp()
                };
            }
            *env_level = env_level.max(s.env);
            s.env
        }
        Op::Shape { input, shape } => {
            let s = &mut state[index];
            let x = get(*input) as f64;
            let y = if s.index == 0 {
                shape.value(x)
            } else {
                shape.process(x, s.phase)
            };
            s.phase = x;
            s.index = 1;
            y as f32
        }
        Op::RawShape { input, shape } => shape.value(get(*input) as f64) as f32,
        Op::Resonator {
            input,
            frequency,
            decay,
            coefficients,
        } => {
            let s = &mut state[index];
            let (a, b) = coefficients
                .unwrap_or_else(|| s.resonator_memo(get(*frequency), get(*decay), c.rate));
            let re = a * s.phase - b * s.imaginary + get(*input) as f64;
            s.imaginary = b * s.phase + a * s.imaginary;
            s.phase = re;
            re as f32
        }
        Op::Reader { reader, speed } => {
            let s = &mut state[index];
            reader.frame(&mut s.reader, expression[2] + pitch, get(*speed), released)
        }
        Op::Mseg(envelope) => {
            let s = &mut state[index];
            let x = envelope.step(&mut s.segment, released, choked, c.rate);
            *env_level = env_level.max(if s.segment.done {
                0.
            } else {
                x.abs().max(1e-4)
            });
            x
        }
        Op::Clamp { input, min, max } => {
            let x = get(*input);
            let a = get(*min);
            let b = get(*max);
            x.clamp(a.min(b), a.max(b))
        }
        Op::Abs(x) => get(*x).abs(),
        Op::Reciprocal(x) => {
            let x = get(*x);
            if x.abs() < 1e-20 { 0. } else { 1. / x }
        }
        Op::Exp2(x) => get(*x).clamp(-100., 100.).exp2(),
        Op::Log2(x) => get(*x).max(1e-20).log2(),
        Op::Sqrt(x) => get(*x).max(0.).sqrt(),
        Op::Hold { input, hz } => {
            let s = &mut state[index];
            if s.index == 0 || s.phase >= 1. {
                s.env = get(*input);
                s.phase = s.phase.fract();
                s.index = 1;
            }
            s.phase += (get(*hz) / c.rate).clamp(0., 1.) as f64;
            s.env
        }
        Op::Slew { input, rise, fall } => {
            let s = &mut state[index];
            let x = get(*input);
            if s.index == 0 {
                s.env = x;
                s.index = 1;
            }
            let time = get(if x > s.env { *rise } else { *fall }).clamp(0., 60.);
            let k = if time == 0. {
                0.
            } else {
                s.slew_memo(time, c.rate)
            };
            s.env = x + (s.env - x) * k;
            s.env
        }
        // Sums and products fold from -0 and 1, as `Iterator::sum` and
        // `product` do: the first input is exact.
        Op::Copy(a) => get(*a),
        Op::Add2(a, b) => get(*a) + get(*b),
        Op::Add3(a, b, d) => get(*a) + get(*b) + get(*d),
        Op::Mul2(a, b) => get(*a) * get(*b),
        Op::Mul3(a, b, d) => get(*a) * get(*b) * get(*d),
        Op::Sum { start, len } => c.args[*start as usize..(*start + *len) as usize]
            .iter()
            .map(|x| get(*x))
            .sum(),
        Op::Product { start, len } => c.args[*start as usize..(*start + *len) as usize]
            .iter()
            .map(|x| get(*x))
            .product(),
        Op::Drive { input, amount } => (get(*input) * get(*amount).clamp(0., 100.)).tanh(),
        Op::Filter {
            input,
            cutoff,
            q,
            coefficients,
            mode,
        } => {
            let s = &mut state[index];
            let (g, k, a) =
                coefficients.unwrap_or_else(|| s.filter_memo(get(*cutoff), get(*q), c.rate));
            let x = get(*input);
            if *mode >= 4 {
                // One pole, trapezoidal: no resonance, and a low-frequency
                // delay of 1/(2*pi*cutoff) seconds at any sample rate.
                let step = (x - s.low) * g / (1. + g);
                let low = s.low + step;
                s.low = low + step;
                match mode {
                    4 => low,
                    5 => x - low,
                    _ => 2. * low - x,
                }
            } else {
                let v3 = x - s.low;
                let band = a * s.band + g * a * v3;
                let low = s.low + g * band;
                s.band = 2. * band - s.band;
                s.low = 2. * low - s.low;
                match mode {
                    1 => x - k * band - low,
                    2 => band,
                    3 => x - k * band,
                    _ => low,
                }
            }
        }
        Op::Delay {
            seconds,
            feedback,
            damping,
            max_feedback,
            cubic,
            fixed_cubic,
            base,
            length,
        } => {
            let s = &mut state[index];
            // Position the read head in f64. In f32, a delay time just
            // below an integer sample (e.g. 0.008 s) can make
            // rem_euclid round up to exactly `length`, indexing past the
            // end of the buffer. f64 keeps the fractional position and the
            // truncating cast below safely in [0, length).
            let line = &delay[*base..*base + *length];
            let x = if *cubic {
                // Third-order Lagrange over the samples written n-1..n+2
                // frames ago; the slot at `s.index` is this frame's.
                let (whole, weights) =
                    fixed_cubic.unwrap_or_else(|| cubic_taps(get(*seconds), c.rate, *length));
                let first = s.index + *length - (whole - 1);
                let mut index = if first >= *length {
                    first - *length
                } else {
                    first
                };
                let mut output = 0.;
                for weight in weights {
                    output += line[index] * weight;
                    index = if index == 0 { *length - 1 } else { index - 1 };
                }
                output
            } else {
                let offset = (get(*seconds) * c.rate).clamp(1., (*length - 4) as f32) as f64;
                let pos_f = (s.index as f64 - offset).rem_euclid(*length as f64);
                let i = pos_f as usize;
                let frac = (pos_f - i as f64) as f32;
                line[i] * (1. - frac) + line[(i + 1) % length] * frac
            };
            let returned = if let Some(cutoff) = damping {
                let a = s.damping_memo(get(*cutoff), c.rate);
                s.low = x + (s.low - x) * a;
                s.low
            } else {
                x
            };
            // The line is written after every node has run, so its input
            // may be a later node: that edge closes a feedback loop.
            s.env = returned * get(*feedback).clamp(-max_feedback, *max_feedback);
            x
        }
        Op::Sample {
            audio,
            channel,
            rate,
            root,
            looped,
        } => {
            let s = &mut state[index];
            let pos = s.phase;
            let i = pos as usize;
            let x = if i + 1 < audio.len() {
                let read = |i: usize| match channel {
                    1 => audio[i][0],
                    2 => audio[i][1],
                    _ => (audio[i][0] + audio[i][1]) * 0.5,
                };
                read(i) + (read(i + 1) - read(i)) * (pos - i as f64) as f32
            } else {
                0.
            };
            let bend = expression[2];
            let ratio = s.memo.get([pitch, bend], || {
                [2f32.powf((pitch + bend - root) / 12.) as f64, 0., 0.]
            })[0] as f32;
            s.phase += (*rate / c.rate * ratio) as f64;
            if *looped {
                s.phase %= audio.len() as f64;
            }
            x
        }
    }
}
fn cubic_taps(seconds: f32, rate: f32, length: usize) -> (usize, [f32; 4]) {
    let offset = (seconds * rate).clamp(2., (length - 5) as f32) as f64;
    let whole = offset.floor();
    let f = (offset - whole) as f32;
    (
        whole as usize,
        [
            -f * (f - 1.) * (f - 2.) / 6.,
            (f + 1.) * (f - 1.) * (f - 2.) / 2.,
            -(f + 1.) * f * (f - 2.) / 2.,
            (f + 1.) * f * (f - 1.) / 6.,
        ],
    )
}

/// A nonzero xorshift seed from a device id (FNV-1a), so each track's noise is
/// its own: two instances of one patch in unison do not sum coherently.
fn noise_seed(id: &str) -> u32 {
    let hash = id.bytes().fold(0x811c9dc5u32, |h, b| {
        (h ^ b as u32).wrapping_mul(0x01000193)
    });
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
            self.remix();
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
            self.remix();
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
        if p.value.to_bits() != value.to_bits() {
            p.value = value;
            for v in &mut self.voices {
                v.changed |= Sources::PARAMETERS;
            }
        }
        Ok(())
    }
    fn reset(&mut self) {
        for v in &mut self.voices {
            v.reset();
        }
        self.sounding = 0;
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
        let factor = self.oversample;
        let mut staging = std::mem::take(&mut self.staging);
        for frame in 0..ctx.frames {
            while event < events.len() && events[event].offset as usize == frame {
                self.event(events[event].kind);
                event += 1;
            }
            if factor == 1 {
                [left[frame], right[frame]] = self.frame();
            } else {
                for x in &mut staging[0][frame * factor..(frame + 1) * factor] {
                    *x = self.frame();
                }
            }
        }
        if factor > 1 {
            let [graph, half] = &mut staging;
            let graph = &mut graph[..ctx.frames * factor];
            let output = if factor == 2 {
                self.halvers[0].decimate(graph, &mut half[..ctx.frames]);
                &half[..ctx.frames]
            } else {
                let half = &mut half[..ctx.frames * 2];
                self.halvers[0].decimate(graph, half);
                self.halvers[1].decimate(half, &mut graph[..ctx.frames]);
                &graph[..ctx.frames]
            };
            for (frame, x) in output.iter().enumerate() {
                left[frame] = x[0];
                right[frame] = x[1];
            }
        }
        self.staging = staging;
        Ok(())
    }
}
