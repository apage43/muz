//! A voice patch's schedule as native code, compiled with Cranelift when the
//! patch is prepared, off the audio thread.
//!
//! Three functions run one voice's frame: the event-rate steps whose sources
//! changed, the settling steps that have not come to rest (and whether they
//! changed anything), and the audio-rate steps with the delay writes. They
//! compute exactly what `run` and `write_delays` compute, with the same float
//! operations, so the output is the same bit for bit.
//!
//! Settling steps, and audio-rate steps that read only earlier tiers,
//! envelopes, holds, delays of such signals and each other (see `Quiet`),
//! run in clusters: a smoother with the arithmetic reading only it, or a node
//! with its one reader. A cluster rests while none of its inputs moved this
//! frame and its last run changed no smoother's output or state, which is
//! exact: it would compute what it computed last. Each node read by another
//! cluster reports whether its output moved, through memory when the reader
//! runs in a later function (see `Gen::rouse`), so an event wakes only the
//! clusters whose inputs it changed. The order of independent steps changes
//! no result: none of them draws noise, and the loudest envelope is a
//! maximum. Raw shapes of one smoother over one domain share their lookup
//! and read one table of their entries side by side (see `Tables`), and an
//! input past the domain reads the end's value without one. A slew
//! that moves only shapes, once it and its input lie past the same end of
//! each, runs alone (see `Gen::tail_of`): it can no longer move them.
//!
//! The other audio-rate steps run in an order of their own (see `Order`)
//! that keeps their values briefly live: each step as late as its first
//! reader allows, sums folded as their inputs become known, and a delay line
//! written as soon as its node has run and its input is known (nothing else
//! reads the line). Signals stay in registers and constants become
//! immediates; only values read elsewhere (the voice's outputs, a probe, the
//! lifetime node and inputs of called steps) are stored to the value table.
//! A guard against non-finite values is left out where bounds on the inputs
//! show it would change nothing, and the others are checked a group at a
//! time (see `Gen::flush`). These steps are compiled twice: the bounded copy
//! may assume that every line sample and state field it reads back is within
//! `LIMIT`, and so leaves out almost every guard; it runs while the values
//! the frames before it stored, and its inputs from other tiers, are within
//! that limit (see `Gen::audio`). The bounded copy also runs packs: steps of
//! one kind that do not depend on each other, such as a lattice's junctions,
//! computed in the lanes of one vector with each lane's operations those of
//! its step (see `pack`). Packs of filters and delays keep their lanes'
//! state and coefficients side by side after the delay lines, where every
//! function finds them (see `pack::Layout`).
//!
//! Memos of filters, damped delays and slews whose inputs change only at
//! earlier tiers are refilled where those inputs change, instead of being
//! tested every frame; a slew with constant times uses its two poles as
//! constants. Memos are caches, so skipping one changes no output. Compiled
//! code keeps a filter's coefficients in its memo as f32, with the product a
//! frame would compute from them.
//!
//! Every operation but parameters, note data, antialiased shapes and sample
//! readers is emitted inline: oscillators, envelopes, holds, resonators,
//! delays, filters, smoothers and arithmetic. Coefficient refills and the
//! rarer turns of a segment envelope call the interpreter's own code; the
//! rest call `step`, which evaluates one step with the interpreter's `eval`.
//! A filter's tangent is muz's own (see `tan`), whose f64 operations are
//! emitted inline. libm functions are called through Rust, so they are the
//! interpreter's own, except where the result is exact without them:
//! `x % 1.` is `x` less its whole part (with `x`'s sign on a zero), and
//! `exp` of a number below -110 is +0, so an envelope's settled decay calls
//! nothing. A delay's taps read a copy of the line that follows it (see
//! `LINE_MIRROR`), so they never wrap around it.
//!
//! Rust's `f32::min`, `max` and `clamp` are written as comparisons and
//! selects with the same operand order, which also fixes the sign of a zero
//! result; Cranelift's `fmin`/`fmax` order zeros differently and are not used.
//! An addition or product with a constant takes it second, where it can be
//! read from memory; with a number for one operand, either order gives the
//! same result.
//!
//! `MUZ_JIT_DUMP=1` prints each compiled function's machine code.
use super::*;
use anyhow::{Context as _, anyhow};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{F32, F64, I8, I32, I64};
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlagsData, Value};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
#[path = "patch_pack.rs"]
mod pack;
use pack::{Field, Packs, packing};
#[cfg(test)]
pub(super) use pack::{packed, set_off as pack_nothing};

/// 0 follows `MUZ_JIT`; 1 compiles; 2 interprets.
static MODE: AtomicU8 = AtomicU8::new(0);

/// Whether patches prepared from now on are compiled. Overrides `MUZ_JIT`
/// for this process.
pub fn set_enabled(enabled: bool) {
    MODE.store(if enabled { 1 } else { 2 }, Ordering::Relaxed);
}

pub(super) fn enabled() -> bool {
    match MODE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            static ENV: OnceLock<bool> = OnceLock::new();
            *ENV.get_or_init(|| {
                !matches!(
                    std::env::var("MUZ_JIT").as_deref(),
                    Ok("0" | "off" | "false" | "no" | "interpret")
                )
            })
        }
    }
}

/// Say once per process why patches run interpreted.
pub(super) fn unavailable(error: &anyhow::Error) {
    static SAID: OnceLock<()> = OnceLock::new();
    SAID.get_or_init(|| eprintln!("voice patches run interpreted: {error:#}"));
}

/// What the compiled functions read of a voice's frame.
#[repr(C)]
struct Frame {
    values: *mut f32,
    state: *mut State,
    delay: *mut f32,
    rng: *mut u32,
    probe_rng: *mut u32,
    age: u64,
    note_start: u64,
    freq: f32,
    /// The loudest envelope this frame.
    env_level: f32,
    released: u8,
    choked: u8,
    /// Frames until every line sample and state field that the audio-rate
    /// steps read back is known to lie within `LIMIT`.
    wild: u32,
    /// The patch's steps, for `step`.
    steps: *const Step,
    shared: *const c_void,
    /// The `PatchVoice`, for `step`.
    voice: *mut c_void,
}

/// The event-rate steps that follow the event sources given, or the
/// settling steps that have not come to rest given the sources that changed
/// this frame; the latter returns whether they left every output and state as
/// it was.
type Tier = unsafe extern "C" fn(*mut Frame, u64) -> u8;

/// The audio-rate steps, given `FRESH`, `CONTROL_RAN` and `SETTLING_MOVED`.
type Audio = unsafe extern "C" fn(*mut Frame, u64);
/// The voice's first frame since it started: every step runs.
const FRESH: u64 = 1;
/// The event-rate steps ran this frame.
const CONTROL_RAN: u64 = 2;
/// A settling output or state changed this frame.
const SETTLING_MOVED: u64 = 4;

/// A patch's compiled schedule. Dropping it frees the code.
pub(super) struct Compiled {
    module: Option<JITModule>,
    /// Interleaved shape tables the code reads (see `Gen::raw_shapes`).
    _tables: Tables,
    /// Where packs keep their lanes' state, if the code packs steps.
    layout: Option<pack::Layout>,
    control: Tier,
    settling: Tier,
    audio: Audio,
}
// SAFETY: the module is finalized and only its code is used, by whichever
// thread owns the patch; nothing in it refers to the compiling thread.
unsafe impl Send for Compiled {}
unsafe impl Sync for Compiled {}
impl Drop for Compiled {
    fn drop(&mut self) {
        if let Some(module) = self.module.take() {
            // SAFETY: the patch that called this code is going away with it.
            unsafe { module.free_memory() };
        }
    }
}
impl Compiled {
    /// Run one frame of voice `v`'s schedule, as `VoicePatch::frame` does:
    /// the event-rate steps whose sources changed, the settling steps that
    /// have not come to rest, the audio-rate steps and the delay writes.
    /// Returns the frame's loudest envelope.
    #[inline(never)]
    pub(super) fn frame(
        &self,
        steps: &[Step],
        c: &Shared,
        v: &mut PatchVoice,
        freq: f32,
        [rng, probe_rng]: [&mut u32; 2],
    ) -> f32 {
        let mut frame = Frame {
            values: v.values.as_mut_ptr(),
            state: v.state.as_mut_ptr(),
            delay: v.delay.as_mut_ptr(),
            rng,
            probe_rng,
            age: v.age,
            note_start: v.note_start,
            freq,
            env_level: 0.,
            released: v.released.is_some().into(),
            choked: v.choked.into(),
            wild: v.wild,
            steps: steps.as_ptr(),
            shared: (c as *const Shared).cast(),
            voice: (v as *mut PatchVoice).cast(),
        };
        let v = frame.voice.cast::<PatchVoice>();
        // SAFETY: the code was compiled for this patch's steps, value table,
        // state and delay layout, which every voice shares. It and `step`
        // reach the voice only through `frame`.
        unsafe {
            let changed = (*v).changed;
            if changed != 0 {
                (self.control)(&mut frame, changed);
                (*v).unsettled |= changed;
                (*v).changed = 0;
            }
            let mut flags = if changed & Sources::START != 0 {
                FRESH | CONTROL_RAN | SETTLING_MOVED
            } else if changed != 0 {
                CONTROL_RAN
            } else {
                0
            };
            if (*v).unsettled != 0 {
                if (self.settling)(&mut frame, changed) != 0 {
                    (*v).unsettled = 0;
                } else {
                    flags |= SETTLING_MOVED;
                }
            }
            (self.audio)(&mut frame, flags);
            (*v).wild = frame.wild;
        }
        frame.env_level
    }

    /// The length of a voice's delay memory with the rows where packs keep
    /// their lanes' state (see `pack::Layout`), if they keep any.
    pub(super) fn memory(&self) -> Option<usize> {
        self.layout.as_ref().map(|l| l.memory)
    }

    /// Copy the fields that packs keep in rows into voice `v`'s nodes'
    /// state, where the interpreter keeps them. Compiled code reads the
    /// rows, so it may go on.
    #[cfg(test)]
    pub(super) fn unpack(&self, v: &mut PatchVoice) {
        let Some(layout) = &self.layout else { return };
        for (node, field, at) in layout.fields() {
            let x = v.delay[at];
            let s = &mut v.state[node];
            match field {
                pack::Field::Low => s.low = x,
                pack::Field::Band => s.band = x,
                pack::Field::Env => s.env = x,
                pack::Field::Index => s.index = x.to_bits() as usize,
                pack::Field::Memo(_) => unreachable!("memos are caches"),
            }
        }
    }

    /// Copy those fields from voice `v`'s nodes' state into their rows, as
    /// `unpack` leaves them.
    #[cfg(test)]
    pub(super) fn pack(&self, v: &mut PatchVoice) {
        let Some(layout) = &self.layout else { return };
        for (node, field, at) in layout.fields() {
            let s = &v.state[node];
            v.delay[at] = match field {
                pack::Field::Low => s.low,
                pack::Field::Band => s.band,
                pack::Field::Env => s.env,
                pack::Field::Index => f32::from_bits(s.index as u32),
                pack::Field::Memo(_) => unreachable!("memos are caches"),
            };
        }
    }
}

/// Evaluate step `step` with the interpreter: the operations not emitted inline.
unsafe extern "C" fn step(frame: *mut Frame, step: *const Step) -> f32 {
    // SAFETY: `frame` is the one `Compiled::frame` built, whose voice and
    // `shared` outlive the call; compiled code holds no references.
    unsafe {
        let frame = &mut *frame;
        let step = &*step;
        let v = &mut *frame.voice.cast::<PatchVoice>();
        let note = note_of!(v, frame.freq);
        eval(
            &step.op,
            step.node as usize,
            &*frame.shared.cast::<Shared>(),
            &mut v.values,
            &mut v.state,
            &v.delay,
            &note,
            &mut [&mut *frame.rng, &mut *frame.probe_rng],
            &mut frame.env_level,
        )
    }
}
// Memo refills, called through `trampoline`s that clobber no register.
unsafe extern "C" fn slew_memo(s: *mut State, time: f32, rate: f32) {
    unsafe { (*s).slew_memo(time, rate) };
}
unsafe extern "C" fn detune_memo(s: *mut State, cents: f32) {
    unsafe { (*s).detune_memo(cents) };
}
unsafe extern "C" fn release_memo(s: *mut State, release: f32, rate: f32) {
    unsafe { (*s).release_memo(release, rate) };
}
unsafe extern "C" fn resonator_memo(s: *mut State, frequency: f32, decay: f32, rate: f32) {
    unsafe { (*s).resonator_memo(frequency, decay, rate) };
}
/// A segment envelope's frame, where the inline path does not cover it.
unsafe extern "C" fn mseg_step(
    envelope: *const envelope::Envelope,
    s: *mut State,
    released: u8,
    choked: u8,
    rate: f32,
) {
    unsafe { (*envelope).step(&mut (*s).segment, released != 0, choked != 0, rate) };
}
/// An exponential segment's progress, as `Envelope::step` has it.
extern "C" fn mseg_exponential(p: f32) -> f32 {
    envelope::exponential(p)
}
/// A slew's pole for a nonzero time constant, as `State::slew_memo` has it.
extern "C" fn slew_pole(time: f32, rate: f32) -> f32 {
    State::default().slew_memo(time, rate)
}
extern "C" fn exp2(x: f32) -> f32 {
    x.exp2()
}
extern "C" fn log2(x: f32) -> f32 {
    x.log2()
}
extern "C" fn tanh(x: f32) -> f32 {
    x.tanh()
}
extern "C" fn tan(x: f32) -> f32 {
    super::tan::tan(x)
}
extern "C" fn exp(x: f32) -> f32 {
    x.exp()
}
extern "C" fn sin(x: f32) -> f32 {
    x.sin()
}

/// Whether `op` keeps state that a settling frame must leave unchanged.
fn stateful(op: &Op) -> bool {
    !matches!(
        op,
        Op::Param(_)
            | Op::Frequency
            | Op::TargetFrequency
            | Op::Velocity
            | Op::NoteAge
            | Op::Expression(_)
            | Op::Noise { .. }
            | Op::RawShape { .. }
            | Op::Clamp { .. }
            | Op::Abs(_)
            | Op::Reciprocal(_)
            | Op::Exp2(_)
            | Op::Log2(_)
            | Op::Sqrt(_)
            | Op::Copy(_)
            | Op::Add2(..)
            | Op::Add3(..)
            | Op::Mul2(..)
            | Op::Mul3(..)
            | Op::Sum { .. }
            | Op::Product { .. }
            | Op::Drive { .. }
    )
}

/// Whether `op` is emitted inline; the rest call `step`.
fn inline(op: &Op) -> bool {
    match op {
        Op::Noise { .. }
        | Op::NoteAge
        | Op::Clamp { .. }
        | Op::Abs(_)
        | Op::Reciprocal(_)
        | Op::Exp2(_)
        | Op::Log2(_)
        | Op::Sqrt(_)
        | Op::Slew { .. }
        | Op::Copy(_)
        | Op::Add2(..)
        | Op::Add3(..)
        | Op::Mul2(..)
        | Op::Mul3(..)
        | Op::Sum { .. }
        | Op::Product { .. }
        | Op::Drive { .. }
        | Op::Filter { .. }
        | Op::RawShape { .. }
        | Op::Osc { .. }
        | Op::Envelope { .. }
        | Op::Mseg(_)
        | Op::Hold { .. }
        | Op::Resonator { .. }
        | Op::Delay { .. } => true,
        Op::Param(_)
        | Op::Frequency
        | Op::TargetFrequency
        | Op::Velocity
        | Op::Expression(_)
        | Op::Shape { .. }
        | Op::Reader { .. }
        | Op::Sample { .. } => false,
    }
}

/// The slots `op` reads.
fn inputs(op: &Op, args: &[Slot]) -> Vec<Slot> {
    match op {
        Op::Param(_)
        | Op::Frequency
        | Op::TargetFrequency
        | Op::Velocity
        | Op::NoteAge
        | Op::Expression(_)
        | Op::Noise { .. }
        | Op::Mseg(_)
        | Op::Sample { .. } => vec![],
        Op::Osc {
            ratio,
            detune,
            hz,
            fm,
            width,
            phase,
            ..
        } => [*ratio, *detune, *fm, *width, *phase]
            .into_iter()
            .chain(*hz)
            .collect(),
        Op::Envelope {
            attack,
            decay,
            sustain,
            release,
            ..
        } => vec![*attack, *decay, *sustain, *release],
        Op::Shape { input, .. } | Op::RawShape { input, .. } => vec![*input],
        Op::Resonator {
            input,
            frequency,
            decay,
            ..
        } => vec![*input, *frequency, *decay],
        Op::Reader { speed, .. } => vec![*speed],
        Op::Clamp { input, min, max } => vec![*input, *min, *max],
        Op::Abs(x) | Op::Reciprocal(x) | Op::Exp2(x) | Op::Log2(x) | Op::Sqrt(x) | Op::Copy(x) => {
            vec![*x]
        }
        Op::Hold { input, hz } => vec![*input, *hz],
        Op::Slew { input, rise, fall } => vec![*input, *rise, *fall],
        Op::Add2(a, b) | Op::Mul2(a, b) => vec![*a, *b],
        Op::Add3(a, b, c) | Op::Mul3(a, b, c) => vec![*a, *b, *c],
        Op::Sum { start, len } | Op::Product { start, len } => {
            args[*start as usize..(*start + *len) as usize].to_vec()
        }
        Op::Drive { input, amount } => vec![*input, *amount],
        Op::Filter {
            input, cutoff, q, ..
        } => vec![*input, *cutoff, *q],
        Op::Delay {
            seconds,
            feedback,
            damping,
            ..
        } => [*seconds, *feedback].into_iter().chain(*damping).collect(),
    }
}

/// Compile `p`'s schedule. `constants` follow its `nodes` in the value table.
pub(super) fn compile(p: &VoicePatch, nodes: usize, constants: &[f32]) -> anyhow::Result<Compiled> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(p, nodes, constants)))
        .map_err(|_| anyhow!("code generation panicked"))?
}

#[cfg(test)]
thread_local! {
    static FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// How many packs of filters found their state and input zero and skipped
/// their steps (see `pack::pack_filter`).
#[cfg(test)]
pub(super) static SILENT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// How many frames ran the bounded and the unbounded copy of the audio-rate
/// steps, and how many groups of guards found a value not finite.
#[cfg(test)]
pub(super) static RAN: [std::sync::atomic::AtomicUsize; 3] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; 3];
/// Make this thread's next compilation fail, as a host refusing executable
/// memory would.
#[cfg(test)]
pub(super) fn fail_next_compile() {
    FAIL.with(|f| f.set(true));
}

fn build(p: &VoicePatch, nodes: usize, constants: &[f32]) -> anyhow::Result<Compiled> {
    #[cfg(test)]
    ensure!(
        !FAIL.with(|f| f.replace(false)),
        "executable memory refused"
    );
    let mut flags = settings::builder();
    flags.set("opt_level", "speed")?;
    flags.set("use_colocated_libcalls", "false")?;
    flags.set(
        "is_pic",
        if cfg!(target_arch = "x86_64") {
            "true"
        } else {
            "false"
        },
    )?;
    flags.set("enable_probestack", "true")?;
    flags.set("probestack_strategy", "inline")?;
    flags.set(
        "enable_verifier",
        if cfg!(debug_assertions) {
            "true"
        } else {
            "false"
        },
    )?;
    let isa = cranelift_native::builder()
        .map_err(|e| anyhow!("no native code generator for this machine: {e}"))?
        .finish(settings::Flags::new(flags))?;
    let mut module = JITModule::new(JITBuilder::with_isa(isa, default_libcall_names()));
    ensure!(
        module.target_config().pointer_type() == I64,
        "native code needs 64-bit pointers"
    );
    // Which tier changes each node: constants and nodes that never run do
    // not change.
    let mut tier = vec![CONSTANT; nodes];
    for (k, s) in p.steps.iter().enumerate() {
        tier[s.node as usize] = if k < p.control {
            CONTROL
        } else if k < p.settling {
            SETTLING
        } else {
            AUDIO
        };
    }
    let tier_of = |slot: &Slot| tier.get(*slot as usize).copied().unwrap_or(CONSTANT);
    // Audio-rate memos whose inputs change only in an earlier tier are
    // refilled at the end of the last tier their inputs come from.
    let mut hoisted = vec![CONSTANT; nodes];
    for s in &p.steps[p.settling..] {
        let inputs = match &s.op {
            Op::Filter {
                coefficients: None,
                cutoff,
                q,
                ..
            } => tier_of(cutoff).max(tier_of(q)),
            Op::Delay {
                damping: Some(cutoff),
                ..
            } => tier_of(cutoff),
            _ => AUDIO,
        };
        if inputs < AUDIO {
            hoisted[s.node as usize] = inputs.max(CONTROL);
        }
    }
    // So are the poles of slews whose times change only at events.
    for s in &p.steps[p.control..] {
        if let Op::Slew { rise, fall, .. } = &s.op
            && tier_of(rise).max(tier_of(fall)) == CONTROL
        {
            hoisted[s.node as usize] = CONTROL;
        }
    }
    // Audio-rate nodes that can stop moving (see `Quiet`), and their clusters.
    let mut quiet = vec![Quiet::No; nodes];
    let line_input: std::collections::HashMap<usize, Slot> = p
        .delays
        .iter()
        .map(|&(node, input, ..)| (node, input))
        .collect();
    let earlier = |slot: &Slot| tier_of(slot) < AUDIO;
    loop {
        let mut grew = false;
        for s in &p.steps[p.settling..] {
            let node = s.node as usize;
            if quiet[node] != Quiet::No {
                continue;
            }
            let reads = inputs(&s.op, &p.args);
            let settles =
                |quiet: &[Quiet], slot: &Slot| earlier(slot) || quiet[*slot as usize] != Quiet::No;
            let kind = match &s.op {
                // A source runs before every cluster, so it reads only
                // earlier tiers; a delay's line is written afterwards.
                Op::Delay { .. }
                    if reads.iter().all(earlier)
                        && line_input.get(&node).is_none_or(|i| settles(&quiet, i)) =>
                {
                    Quiet::Source
                }
                Op::Envelope { .. } | Op::Mseg(_) | Op::Hold { .. }
                    if reads.iter().all(earlier) =>
                {
                    Quiet::Source
                }
                Op::Slew { .. } | Op::Filter { .. } if reads.iter().all(|i| settles(&quiet, i)) => {
                    Quiet::Resting
                }
                op if !stateful(op)
                    && inline(op)
                    && !matches!(op, Op::Noise { .. } | Op::NoteAge)
                    && reads.iter().all(|i| settles(&quiet, i)) =>
                {
                    Quiet::Pure
                }
                _ => continue,
            };
            quiet[node] = kind;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    // Settling steps are all quiet: smoothers rest, the rest are pure.
    for s in &p.steps[p.control..p.settling] {
        quiet[s.node as usize] = if stateful(&s.op) {
            Quiet::Resting
        } else {
            Quiet::Pure
        };
    }
    let (clusters, cluster) = clusters(p, &quiet, &tier);
    // Event-rate nodes that clusters read, and settling nodes that
    // audio-rate clusters read, flag whether they moved (see `Gen::cluster`).
    let mut flagged = vec![false; nodes];
    for (c, members) in clusters.iter().enumerate() {
        for &k in members {
            for i in inputs(&p.steps[k].op, &p.args) {
                let i = i as usize;
                if i < nodes
                    && cluster[i] != c
                    && (tier[i] == CONTROL || tier[i] == SETTLING && members[0] >= p.settling)
                {
                    flagged[i] = true;
                }
            }
        }
    }
    // Audio-rate memos whose inputs are constants and outputs of quiet
    // clusters are refilled at the end of each such cluster's run, which
    // is whenever they can change, instead of being tested every frame.
    let mut refills_after = vec![Vec::new(); clusters.len()];
    for s in &p.steps[p.settling..] {
        let node = s.node as usize;
        let keys = match &s.op {
            Op::Filter {
                coefficients: None,
                cutoff,
                q,
                ..
            } => vec![*cutoff, *q],
            Op::Delay {
                damping: Some(cutoff),
                ..
            } => vec![*cutoff],
            _ => continue,
        };
        // A quiet memo node runs inside the clusters; it tests its memo.
        if hoisted[node] != CONSTANT || quiet[node] != Quiet::No {
            continue;
        }
        let constant = |k: &&Slot| **k as usize >= nodes;
        let from: Option<Vec<usize>> = keys
            .iter()
            .filter(|k| !constant(k))
            .map(|k| {
                Some(cluster[*k as usize])
                    .filter(|&c| c != usize::MAX && tier[*k as usize] == AUDIO)
            })
            .collect();
        if let Some(mut from) = from.filter(|from| !from.is_empty()) {
            from.sort_unstable();
            from.dedup();
            hoisted[node] = AUDIO;
            for c in from {
                refills_after[c].push(node);
            }
        }
    }
    // Memos hoisted to the settling tier are refilled after the runs of the
    // settling clusters their inputs come from, and when event-rate inputs
    // may have changed, after all of them (see `Gen::settle`).
    for s in &p.steps[p.settling..] {
        let node = s.node as usize;
        if hoisted[node] != SETTLING {
            continue;
        }
        let mut from: Vec<usize> = match &s.op {
            Op::Filter { cutoff, q, .. } => vec![*cutoff, *q],
            Op::Delay {
                damping: Some(cutoff),
                ..
            } => vec![*cutoff],
            _ => unreachable!("only filters and damped delays are hoisted"),
        }
        .into_iter()
        .filter(|&k| tier.get(k as usize) == Some(&SETTLING))
        .map(|k| cluster[k as usize])
        .collect();
        from.sort_unstable();
        from.dedup();
        for c in from {
            refills_after[c].push(node);
        }
    }
    // Audio-rate memos refill rarely, between tests that run every frame.
    // Through these they clobber no register, so values live across a test
    // stay in registers instead of spilling around it.
    let mut refill = |name, params: &[ir::Type], target: *const ()| {
        trampoline(&mut module, name, params, target)
    };
    let refills = Refills {
        slew: refill("slew_memo", &[I64, F32, F32], slew_memo as *const ())?,
        detune: refill("detune_memo", &[I64, F32], detune_memo as *const ())?,
        release: refill("release_memo", &[I64, F32, F32], release_memo as *const ())?,
        resonator: refill(
            "resonator_memo",
            &[I64, F32, F32, F32],
            resonator_memo as *const (),
        )?,
        mseg: refill(
            "mseg_step",
            &[I64, I64, I8, I8, F32],
            mseg_step as *const (),
        )?,
    };
    // The bounded copy of the audio-rate steps runs packs (see `pack`),
    // which keep their lanes' state in rows after the delay lines, where
    // every function that reads or refills it finds it.
    let packed = packing().then(|| {
        let mut packs = pack::packs(p, &quiet, &hoisted, nodes);
        let order = Order::new(p, &quiet, nodes, Some(&mut packs));
        let lines = p.voices.first().map_or(0, |v| v.delay.len());
        pack::Layout::new(p, &packs, nodes, lines).map(|layout| (packs, order, layout))
    });
    let shape = Shape {
        p,
        nodes,
        constants,
        packed: packed.flatten(),
        hoisted,
        quiet,
        clusters,
        cluster,
        refills_after,
        refills,
        tiers: tier,
        statics: statics(p, nodes, constants),
        flagged,
        tables: Default::default(),
    };
    let control = shape.function(&mut module, "control", Function::Tier(0..p.control, false))?;
    let settling = shape.function(
        &mut module,
        "settling",
        Function::Tier(p.control..p.settling, true),
    )?;
    let audio = shape.function(&mut module, "audio", Function::Audio)?;
    module.finalize_definitions()?;
    // SAFETY: each function was declared with the signature it is cast to.
    unsafe {
        let tier = |id| std::mem::transmute::<*const u8, Tier>(module.get_finalized_function(id));
        Ok(Compiled {
            _tables: shape.tables.take(),
            layout: shape.packed.map(|(.., layout)| layout),
            control: tier(control),
            settling: tier(settling),
            audio: std::mem::transmute::<*const u8, Audio>(module.get_finalized_function(audio)),
            module: Some(module),
        })
    }
}

/// A function that calls `target` (a C function of `params`) and preserves
/// every register.
fn trampoline(
    module: &mut JITModule,
    name: &str,
    params: &[ir::Type],
    target: *const (),
) -> anyhow::Result<cranelift_module::FuncId> {
    let signature = |call_conv| {
        let mut s = ir::Signature::new(call_conv);
        s.params.extend(params.iter().map(|t| AbiParam::new(*t)));
        s
    };
    let own = signature(CallConv::PreserveAll);
    let id = module.declare_function(name, Linkage::Local, &own)?;
    let mut ctx = module.make_context();
    ctx.func.signature = own;
    let mut builder_context = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut ctx.func, &mut builder_context);
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let args = b.block_params(entry).to_vec();
    let inner = b.import_signature(signature(module.target_config().default_call_conv));
    let callee = b.ins().iconst(I64, target as i64);
    b.ins().call_indirect(inner, callee, &args);
    b.ins().return_(&[]);
    b.seal_all_blocks();
    b.finalize(module.target_config());
    module
        .define_function(id, &mut ctx)
        .map_err(|e| anyhow!("{e:?}"))
        .with_context(|| format!("compiling {name}"))?;
    module.clear_context(&mut ctx);
    Ok(id)
}

/// Which part of the schedule a function runs.
enum Function {
    /// Steps of the range whose sources meet the second argument; with `true`,
    /// also whether they changed nothing.
    Tier(std::ops::Range<usize>, bool),
    /// The audio-rate steps, then the delay writes.
    Audio,
}

/// What every compiled function of a patch shares.
struct Shape<'a> {
    p: &'a VoicePatch,
    nodes: usize,
    constants: &'a [f32],
    /// The bounded copy's packs, its order with them, and where they keep
    /// their lanes' state, if it runs packs.
    packed: Option<(Packs, Order, pack::Layout)>,
    /// For each audio-rate memo refilled by a tier, that tier.
    hoisted: Vec<u8>,
    quiet: Vec<Quiet>,
    /// Steps that run or rest together (see `clusters`).
    clusters: Vec<Vec<usize>>,
    cluster: Vec<usize>,
    /// For each cluster, the memo nodes it refills (`hoisted` to `AUDIO`).
    refills_after: Vec<Vec<usize>>,
    /// The tier that changes each node.
    tiers: Vec<u8>,
    refills: Refills<cranelift_module::FuncId>,
    /// Each node's bound by its operation (see `statics`).
    statics: Vec<f64>,
    /// Nodes that flag whether they moved (see `Gen::cluster`).
    flagged: Vec<bool>,
    /// Tables of raw shapes that share a lookup, entry by entry (see
    /// `Gen::raw_shapes`), which the compiled code reads.
    tables: std::cell::RefCell<Tables>,
}

/// Tables of shapes that share a lookup, entry by entry, by the addresses
/// of the shapes' own tables.
type Tables = Vec<(Vec<usize>, Box<[f64]>)>;

/// How an audio-rate node can come to rest: when every input is a constant,
/// an event-rate or settling node, or a quiet audio-rate node. Such a node's
/// inputs may all stop moving: a delay of a held signal, an envelope that
/// sustains, a key's slew reaching its stop. Quiet nodes run first, sources
/// and then clusters; a cluster rests while none of its inputs moves and its
/// smoothers rest, which is exact: it would compute what it computed last.
/// Running them first changes no result: no quiet node draws noise, and the
/// loudest envelope is a maximum.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quiet {
    No,
    /// Runs every frame (its state moves on), but may repeat its output.
    Source,
    /// A pure function: skipped while no input moves.
    Pure,
    /// A smoother: skipped while no input moves, once its last run changed
    /// neither its output nor its state.
    Resting,
}

/// Clusters of settling steps and of quiet audio-rate steps (see `Quiet`),
/// in schedule order, and each node's cluster (`usize::MAX` for none). A step
/// joins the cluster of its one input of the same tier when that input is a
/// smoother, whose readers move when it does, or has no other reader;
/// otherwise it starts a cluster. So a cluster's members read nothing of
/// their tier outside it but through its first member.
fn clusters(p: &VoicePatch, quiet: &[Quiet], tier: &[u8]) -> (Vec<Vec<usize>>, Vec<usize>) {
    let member = |node: usize| matches!(quiet[node], Quiet::Pure | Quiet::Resting);
    let same = |a: usize, b: Slot| {
        (b as usize) < quiet.len() && tier[a] == tier[b as usize] && member(b as usize)
    };
    let mut readers = vec![0usize; quiet.len()];
    for s in &p.steps[p.control..] {
        let node = s.node as usize;
        if member(node) {
            let mut reads = inputs(&s.op, &p.args);
            reads.sort_unstable();
            reads.dedup();
            for i in reads.into_iter().filter(|i| same(node, *i)) {
                readers[i as usize] += 1;
            }
        }
    }
    let mut cluster = vec![usize::MAX; quiet.len()];
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for (k, s) in p.steps.iter().enumerate().skip(p.control) {
        let node = s.node as usize;
        if !member(node) {
            continue;
        }
        let mut reads = inputs(&s.op, &p.args);
        reads.sort_unstable();
        reads.dedup();
        reads.retain(|i| same(node, *i));
        let joins = match reads[..] {
            [i] if quiet[i as usize] == Quiet::Resting || readers[i as usize] == 1 => {
                Some(cluster[i as usize])
            }
            _ => None,
        };
        let c = joins.unwrap_or_else(|| {
            clusters.push(Vec::new());
            clusters.len() - 1
        });
        cluster[node] = c;
        clusters[c].push(k);
    }
    (clusters, cluster)
}

/// The most deferred guards checked at once.
const GROUP: usize = 8;
/// How far past the first item in order (in ranks, four to a step) another
/// may run first, to join its group.
const WINDOW: usize = 32;

/// An audio-rate step of `Order`, part of one, a delay line's write, or a
/// pack of steps or writes.
#[derive(Clone, Copy)]
enum Item {
    /// The step at this index of the schedule.
    Step(usize),
    /// The step's sum or product of its inputs up to this one.
    Part(usize, usize),
    /// The line at this index of the patch's delays.
    Write(usize),
    /// The pack at this index of `Packs`.
    Pack(usize),
}

/// Whether `op` folds more than two inputs, which `Order` adds in a part
/// at a time.
fn folds(op: &Op) -> bool {
    matches!(
        op,
        Op::Add3(..) | Op::Mul3(..) | Op::Sum { .. } | Op::Product { .. }
    )
}

/// The audio-rate steps that are not quiet, and the delay writes, with what
/// each must follow, in an order that keeps values briefly live. Steps run
/// depth first from the delay writes, in schedule order, then from the
/// steps that nothing reads: each as late as its first reader allows. A sum
/// or product of more than two inputs adds them in as they become known,
/// and a write runs as soon as its delay's step and its input have run (a
/// delay's step reads the line its write fills, so it comes first). Any
/// such order computes the same: each step reads only its inputs and its
/// own state, and noise is drawn in schedule order. A pack runs as one
/// item, after everything any of its lanes follows.
struct Order {
    items: Vec<Item>,
    /// For each item, the items it follows, and whether it reads the output.
    deps: Vec<Vec<(usize, bool)>>,
    readers: Vec<Vec<usize>>,
    /// Each item's place in the order.
    rank: Vec<usize>,
}

impl Order {
    /// The order of `p`'s audio-rate steps, with `packs` if given. A pack
    /// that would have to run both before and after another item is
    /// dissolved.
    fn new(p: &VoicePatch, quiet: &[Quiet], nodes: usize, mut packs: Option<&mut Packs>) -> Self {
        loop {
            match Self::build(p, quiet, nodes, packs.as_deref()) {
                Ok(order) => return order,
                Err(pack) => (packs.as_deref_mut())
                    .expect("only packs make cycles")
                    .dissolve(pack),
            }
        }
    }

    /// The order, or a pack on a cycle of items that follow each other.
    fn build(
        p: &VoicePatch,
        quiet: &[Quiet],
        nodes: usize,
        packs: Option<&Packs>,
    ) -> Result<Self, usize> {
        let mut items = Vec::new();
        let mut item_of = vec![usize::MAX; nodes];
        let mut position = vec![0; nodes];
        let mut pack_item = vec![usize::MAX; packs.map_or(0, |p| p.packs.len())];
        let step_pack = |k: usize| packs.and_then(|packs| packs.of_step[k]);
        // The items of each step's parts (see `folds`).
        let mut parts: Vec<Vec<usize>> = vec![Vec::new(); p.steps.len()];
        for (k, s) in p.steps.iter().enumerate().skip(p.settling) {
            let node = s.node as usize;
            position[node] = k;
            if quiet[node] != Quiet::No {
                continue;
            }
            if let Some((pack, _)) = step_pack(k) {
                if pack_item[pack] == usize::MAX {
                    pack_item[pack] = items.len();
                    items.push(Item::Pack(pack));
                }
                item_of[node] = pack_item[pack];
                continue;
            }
            if folds(&s.op) {
                for j in 1..inputs(&s.op, &p.args).len() - 1 {
                    parts[k].push(items.len());
                    items.push(Item::Part(k, j));
                }
            }
            item_of[node] = items.len();
            items.push(Item::Step(k));
        }
        let writes_from = items.len();
        let mut write_item = vec![usize::MAX; p.delays.len()];
        for (w, item) in write_item.iter_mut().enumerate() {
            match packs.and_then(|packs| packs.of_write[w]) {
                Some((pack, _)) => {
                    if pack_item[pack] == usize::MAX {
                        pack_item[pack] = items.len();
                        items.push(Item::Pack(pack));
                    }
                    *item = pack_item[pack];
                }
                None => {
                    *item = items.len();
                    items.push(Item::Write(w));
                }
            }
        }
        let of = |slot: Slot| (item_of.get(slot as usize).copied()).filter(|&i| i != usize::MAX);
        let read = |d: &mut Vec<(usize, bool)>, slot: Slot, me: usize| {
            if let Some(i) = of(slot)
                && i != me
                && !d.contains(&(i, true))
            {
                d.retain(|&(j, _)| j != i);
                d.push((i, true));
            }
        };
        let mut deps: Vec<Vec<(usize, bool)>> = vec![Vec::new(); items.len()];
        let mut noise = None;
        for (k, s) in p.steps.iter().enumerate().skip(p.settling) {
            let node = s.node as usize;
            if quiet[node] != Quiet::No {
                continue;
            }
            let me = item_of[node];
            let reads = inputs(&s.op, &p.args);
            let mut d = std::mem::take(&mut deps[me]);
            if step_pack(k).is_none() && folds(&s.op) {
                // Part j folds in inputs 0..=j; the step, the last.
                read(&mut d, reads[0], me);
                for (j, &slot) in reads.iter().enumerate().take(reads.len() - 1).skip(1) {
                    read(&mut d, slot, me);
                    let part = parts[k][j - 1];
                    deps[part] = std::mem::take(&mut d);
                    d.push((part, true));
                }
                read(&mut d, reads[reads.len() - 1], me);
            } else {
                for &slot in &reads {
                    read(&mut d, slot, me);
                }
            }
            if let Op::Noise { .. } = s.op {
                d.extend(noise.map(|n| (n, false)));
                noise = Some(me);
            }
            deps[me] = d;
        }
        for (w, &(line, input, ..)) in p.delays.iter().enumerate() {
            let me = write_item[w];
            let mut d = std::mem::take(&mut deps[me]);
            read(&mut d, input, me);
            if let Some(i) = of(line as Slot)
                && i != me
                && !d.iter().any(|&(j, _)| j == i)
            {
                d.push((i, false));
            }
            deps[me] = d;
        }
        let mut readers = vec![Vec::new(); items.len()];
        for (i, d) in deps.iter().enumerate() {
            for &(j, _) in d {
                readers[j].push(i);
            }
        }
        // The items in an order that follows every dependency, if one does.
        let mut left: Vec<usize> = deps.iter().map(Vec::len).collect();
        let mut sorted: Vec<usize> = (0..items.len()).filter(|&i| left[i] == 0).collect();
        let mut at = 0;
        while at < sorted.len() {
            for &r in &readers[sorted[at]] {
                left[r] -= 1;
                if left[r] == 0 {
                    sorted.push(r);
                }
            }
            at += 1;
        }
        if sorted.len() < items.len() {
            let stuck = (0..items.len()).rev().find_map(|i| match items[i] {
                Item::Pack(pack) if left[i] > 0 => Some(pack),
                _ => None,
            });
            return Err(stuck.expect("steps and writes alone follow the schedule"));
        }
        let is_write = |i: usize| match items[i] {
            Item::Write(_) => true,
            Item::Pack(pack) => {
                packs.is_some_and(|p| matches!(p.packs[pack], pack::Lanes::Writes(_)))
            }
            _ => false,
        };
        // Depth first from the writes' inputs, in the order the schedule
        // knows them, then from the steps nothing reads, at multiples of
        // four; parts, folds and writes right after what they follow.
        let mut writes: Vec<usize> = (0..p.delays.len()).collect();
        writes.sort_by_key(|&w| {
            let (line, input, ..) = p.delays[w];
            position[line].max(position.get(input as usize).copied().unwrap_or(0))
        });
        let roots = (writes.iter())
            .filter_map(|&w| of(p.delays[w].1))
            .chain((0..writes_from).filter(|&i| readers[i].iter().all(|&r| is_write(r))))
            .collect::<Vec<_>>();
        let mut rank = vec![usize::MAX; items.len()];
        let mut next = 0;
        let mut stack = Vec::new();
        for root in roots {
            stack.push((root, 0));
            while let Some((i, at)) = stack.pop() {
                if rank[i] != usize::MAX {
                    continue;
                }
                match deps[i].get(at) {
                    Some(&(j, _)) => {
                        stack.push((i, at + 1));
                        if rank[j] == usize::MAX {
                            stack.push((j, 0));
                        }
                    }
                    None => {
                        rank[i] = 4 * next;
                        next += 1;
                    }
                }
            }
        }
        for i in sorted {
            let eager = match items[i] {
                Item::Step(k) => folds(&p.steps[k].op),
                Item::Part(..) | Item::Write(_) => true,
                Item::Pack(_) => is_write(i),
            };
            if eager || rank[i] == usize::MAX {
                rank[i] = (deps[i].iter().map(|&(j, _)| rank[j] + 1).max()).unwrap_or(0);
            }
        }
        Ok(Self {
            items,
            deps,
            readers,
            rank,
        })
    }
}

/// For a raw shape's table: the greatest input that `Shape::value` reads at
/// the table's first entry and the least that it reads at its last, each
/// with the value it gives there, computed as `Gen::inline` computes it.
fn ends_of(table: &[f64], low: f64, step: f64) -> ((f32, f32), (f32, f32)) {
    let last = (table.len() - 1) as f64;
    let at = |x: f32| (f64::from(x) - low) / step;
    // `at` never decreases as `x` grows.
    let mut above = (low + last * step) as f32;
    while at(above) < last {
        above = above.next_up();
    }
    while at(above.next_down()) >= last {
        above = above.next_down();
    }
    let mut below = low as f32;
    while at(below) > 0. {
        below = below.next_down();
    }
    while at(below.next_up()) <= 0. {
        below = below.next_up();
    }
    let n = table.len();
    let first = (table[0] + 0. * (table[1] - table[0])) as f32;
    let end = (table[n - 2] + 1. * (table[n - 1] - table[n - 2])) as f32;
    ((below, first), (above, end))
}

/// A bound on each node's output in the value table from its operation
/// alone, else infinity: a parameter's range, a raw shape's table, a clamp's
/// limits, an envelope's level, and what sums, products and the other pure
/// operations make of them.
fn statics(p: &VoicePatch, nodes: usize, constants: &[f32]) -> Vec<f64> {
    let mut bounds = vec![f64::INFINITY; nodes];
    for s in &p.steps {
        let of = |slot: &Slot| match (slot as &Slot).checked_sub(nodes as Slot) {
            Some(c) => match constants[c as usize] {
                c if c.is_finite() => f64::from(c.abs()),
                _ => f64::INFINITY,
            },
            None => bounds[*slot as usize],
        };
        let bound = match &s.op {
            Op::Param(i) => {
                let q = &p.parameters[*i];
                f64::from(q.min.abs().max(q.max.abs()).max(q.value.abs()))
            }
            // Within the last level, or a level between 0 and 1.
            Op::Noise { .. } | Op::Envelope { .. } | Op::Drive { .. } => 1.,
            Op::RawShape { shape, .. } => {
                let (table, ..) = shape.table();
                table.iter().fold(0f64, |m, v| m.max(v.abs())) * (1. + 1e-9)
            }
            Op::Clamp { min, max, .. } => of(min).max(of(max)),
            Op::Abs(x) | Op::Copy(x) | Op::Hold { input: x, .. } => of(x),
            Op::Reciprocal(_) => 1e20,
            Op::Exp2(_) => 2f64.powi(100),
            Op::Log2(_) => 128.,
            Op::Sqrt(x) => of(x).sqrt(),
            Op::Add2(..) | Op::Add3(..) | Op::Sum { .. } => {
                inputs(&s.op, &p.args).iter().map(of).sum()
            }
            Op::Mul2(..) | Op::Mul3(..) | Op::Product { .. } => {
                inputs(&s.op, &p.args).iter().map(of).product()
            }
            _ => f64::INFINITY,
        };
        // With room for rounding.
        let bound = bound * (1. + 1e-6);
        bounds[s.node as usize] = if bound <= FINITE {
            bound
        } else {
            f64::INFINITY
        };
    }
    bounds
}

/// Trampolines to the memo refills and the segment envelope's frame.
struct Refills<F> {
    slew: F,
    detune: F,
    release: F,
    resonator: F,
    mseg: F,
}

impl Shape<'_> {
    fn function(
        &self,
        module: &mut JITModule,
        name: &str,
        function: Function,
    ) -> anyhow::Result<cranelift_module::FuncId> {
        let call_conv = module.target_config().default_call_conv;
        let signature = |params: &[ir::Type], returns: &[ir::Type]| {
            let mut s = ir::Signature::new(call_conv);
            s.params.extend(params.iter().map(|t| AbiParam::new(*t)));
            s.returns.extend(returns.iter().map(|t| AbiParam::new(*t)));
            s
        };
        let own = match function {
            Function::Tier(..) => signature(&[I64, I64], &[I8]),
            Function::Audio => signature(&[I64, I64], &[]),
        };
        let id = module.declare_function(name, Linkage::Local, &own)?;
        let mut ctx = module.make_context();
        ctx.func.signature = own;
        let mut builder_context = FunctionBuilderContext::new();
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut builder_context);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let frame = b.block_params(entry)[0];
        let load = |b: &mut FunctionBuilder, field: usize| {
            b.ins()
                .load(I64, MemFlagsData::trusted(), frame, field as i32)
        };
        let values = load(&mut b, offset_of!(Frame, values));
        let state = load(&mut b, offset_of!(Frame, state));
        let delay = load(&mut b, offset_of!(Frame, delay));
        let signatures = Signatures {
            step: b.import_signature(signature(&[I64, I64], &[F32])),
            unary: b.import_signature(signature(&[F32], &[F32])),
            binary: b.import_signature(signature(&[F32, F32], &[F32])),
            refills: {
                let r = &self.refills;
                let mut declare = |id| module.declare_func_in_func(id, b.func);
                Refills {
                    slew: declare(r.slew),
                    detune: declare(r.detune),
                    release: declare(r.release),
                    resonator: declare(r.resonator),
                    mseg: declare(r.mseg),
                }
            },
        };
        let mut g = Gen {
            b,
            frame,
            values,
            state,
            delay,
            nodes: self.nodes,
            constants: self.constants,
            args: &self.p.args,
            computed: vec![None; self.nodes],
            loaded: vec![None; self.nodes],
            stored: vec![false; self.nodes],
            lines: vec![None; self.nodes],
            rate: self.p.rate,
            choke_decay: self.p.choke_decay,
            signatures,
            known: Default::default(),
            bounds: Default::default(),
            changes: None,
            hoisted: &self.hoisted,
            quiet: &self.quiet,
            clusters: &self.clusters,
            cluster: &self.cluster,
            refills_after: &self.refills_after,
            tiers: &self.tiers,
            moved: vec![None; self.nodes],
            defer: false,
            deferred: None,
            pending: Vec::new(),
            partial: Default::default(),
            statics: &self.statics,
            limit: None,
            squares: None,
            counted: 0,
            anchor: None,
            returns: vec![f64::INFINITY; self.nodes],
            flagged: &self.flagged,
            tables: &self.tables,
            packs: self.packed.as_ref().map(|(packs, ..)| packs),
            packed: self.packed.as_ref().map(|(_, order, _)| order),
            layout: self.packed.as_ref().map(|(.., layout)| layout),
            vectors: Default::default(),
            vlines: Default::default(),
            vsquares: None,
            vanchor: None,
        };
        match function {
            Function::Tier(range, settle) => {
                let sources = g.b.block_params(entry)[1];
                let unchanged = if settle {
                    g.settle(self.p, sources)?
                } else {
                    g.tier(self.p, range, sources)?
                };
                g.b.ins().return_(&[unchanged]);
            }
            Function::Audio => {
                let flags = g.b.block_params(entry)[1];
                g.audio(self.p, flags)?;
                g.b.ins().return_(&[]);
            }
        }
        g.b.seal_all_blocks();
        g.b.finalize(module.target_config());
        let dump = std::env::var_os("MUZ_JIT_DUMP").is_some();
        ctx.set_disasm(dump);
        module
            .define_function(id, &mut ctx)
            .map_err(|e| anyhow!("{e:?}"))
            .with_context(|| format!("compiling the voice graph's {name} steps"))?;
        if dump && let Some(code) = ctx.compiled_code().and_then(|c| c.vcode.as_ref()) {
            eprintln!("=== {name}\n{code}");
        }
        module.clear_context(&mut ctx);
        Ok(id)
    }
}

struct Signatures {
    step: ir::SigRef,
    unary: ir::SigRef,
    binary: ir::SigRef,
    refills: Refills<ir::FuncRef>,
}

const STATE: usize = size_of::<State>();
/// Tiers, in the order a frame runs them.
const CONSTANT: u8 = 0;
const CONTROL: u8 = 1;
const SETTLING: u8 = 2;
const AUDIO: u8 = 3;
/// The magnitude of the largest finite f32.
const FINITE: f64 = f32::MAX as f64;
/// The magnitude below which the bounded copy of the audio-rate steps
/// assumes every line sample and state field it reads back to lie (see
/// `Gen::audio`): far above any signal, low enough that most steps are
/// then provably finite.
const LIMIT: f64 = (1u64 << 24) as f64;
/// Whether compiled code has fused multiply-adds, which Cranelift emits on
/// a processor that has them.
fn fused() -> bool {
    #[cfg(target_arch = "x86_64")]
    return std::is_x86_feature_detected!("fma");
    #[cfg(not(target_arch = "x86_64"))]
    return cfg!(target_arch = "aarch64");
}
/// `bound` with room for rounding.
fn margin(bound: f64) -> f64 {
    bound * (1. + 1e-5)
}
const PHASE: i32 = offset_of!(State, phase) as i32;
const ENV: i32 = offset_of!(State, env) as i32;
const LOW: i32 = offset_of!(State, low) as i32;
const BAND: i32 = offset_of!(State, band) as i32;
const INDEX: i32 = offset_of!(State, index) as i32;
const IMAGINARY: i32 = offset_of!(State, imaginary) as i32;
const SEGMENT: usize = offset_of!(State, segment);
const RESTING: i32 = offset_of!(State, resting) as i32;
const MOVED: i32 = offset_of!(State, moved) as i32;
const LEAN: i32 = offset_of!(State, lean) as i32;
const VALID: i32 = offset_of!(State, memo.valid) as i32;
const KEY: i32 = offset_of!(State, memo.key) as i32;
const MEMO: i32 = offset_of!(State, memo.value) as i32;
const _: () = assert!(size_of::<usize>() == 8);

struct Gen<'a, 'b> {
    b: FunctionBuilder<'b>,
    /// Where interleaved shape tables are kept (see `raw_shapes`).
    tables: &'a std::cell::RefCell<Tables>,
    frame: Value,
    values: Value,
    state: Value,
    delay: Value,
    nodes: usize,
    constants: &'a [f32],
    args: &'a [Slot],
    /// Audio-rate outputs computed so far this frame.
    computed: Vec<Option<Value>>,
    /// Outputs of earlier tiers read so far by this step.
    loaded: Vec<Option<Value>>,
    stored: Vec<bool>,
    /// Each inline delay's read index and feedback signal.
    lines: Vec<Option<(Value, Value)>>,
    rate: f32,
    /// Per-frame decay of a choked envelope.
    choke_decay: f32,
    signatures: Signatures,
    /// Constants made by `f32`.
    known: std::collections::HashMap<Value, f32>,
    /// Bounds on the magnitude of signals, where tighter than `FINITE`.
    bounds: std::collections::HashMap<Value, f64>,
    /// While a settling step is emitted, the state it changes: old and new.
    changes: Option<Vec<(Value, Value)>>,
    /// For each audio-rate filter or damped delay whose memo inputs change
    /// only in an earlier tier, the tier that refills its memo; `AUDIO` for
    /// one refilled by the clusters its inputs come from.
    hoisted: &'a [u8],
    quiet: &'a [Quiet],
    clusters: &'a [Vec<usize>],
    cluster: &'a [usize],
    refills_after: &'a [Vec<usize>],
    tiers: &'a [u8],
    /// For settling and quiet audio-rate nodes run so far: whether the output
    /// moved this frame, as an `I8`.
    moved: Vec<Option<Value>>,
    /// While set, `finite` leaves its value to `flush`, in `deferred`.
    defer: bool,
    deferred: Option<Value>,
    /// Values whose guard `flush` will check, and where they go.
    pending: Vec<(Target, Value)>,
    /// Sums and products folded in parts (see `fold`).
    partial: std::collections::HashMap<usize, (Value, f64, f64, usize)>,
    /// Each node's bound by its operation (see `statics`).
    statics: &'a [f64],
    /// While the bounded copy of the audio-rate steps is emitted, `LIMIT`.
    limit: Option<f64>,
    /// The sum of the squares of the values counted by `account`.
    squares: Option<Value>,
    /// How many values `account` has counted, and where it pins their sum.
    counted: usize,
    anchor: Option<ir::StackSlot>,
    /// For each delay run in this copy, a bound on the feedback it keeps
    /// for its write.
    returns: Vec<f64>,
    flagged: &'a [bool],
    /// The packs that the bounded copy of the audio-rate steps runs, its
    /// order with them, and where packs keep their lanes' state.
    packs: Option<&'a Packs>,
    packed: Option<&'a Order>,
    layout: Option<&'a pack::Layout>,
    /// The vector of each pack emitted so far in this copy.
    vectors: std::collections::HashMap<usize, Value>,
    /// For each delay pack emitted so far, its lanes' write indices and the
    /// vector of their feedback.
    vlines: std::collections::HashMap<usize, (Vec<Value>, Value)>,
    /// The sum of the squares of the vectors counted by `account_lanes`,
    /// lane by lane.
    vsquares: Option<Value>,
    vanchor: Option<ir::StackSlot>,
}

/// Where a value whose guard is deferred goes once checked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    /// A node's output, for its readers.
    Node(usize),
    /// A delay line, at the first address and at the second plus the
    /// offset (see `write_delay`).
    Line(Value, Value, i32),
}

impl Gen<'_, '_> {
    fn f32(&mut self, x: f32) -> Value {
        let v = self.b.ins().f32const(x);
        self.known.insert(v, x);
        v
    }
    /// The value of `v` if it is a constant.
    fn constant(&self, v: Value) -> Option<f32> {
        self.known.get(&v).copied()
    }
    /// A slot's value. Called only on a step's straight-line path, before any
    /// branch, so a value loaded here dominates the rest of the step.
    fn get(&mut self, slot: Slot) -> Value {
        let s = slot as usize;
        if s >= self.nodes {
            return self.f32(self.constants[s - self.nodes]);
        }
        if let Some(v) = self.computed[s].or(self.loaded[s]) {
            return v;
        }
        let v = self
            .b
            .ins()
            .load(F32, MemFlagsData::trusted(), self.values, 4 * s as i32);
        self.loaded[s] = Some(v);
        // Bounded by its operation, and in the bounded copy of the audio-rate
        // steps by `LIMIT`, which the frame checks first (see `audio`).
        let bound = self.statics[s].min(self.limit.unwrap_or(FINITE));
        self.bounds.insert(v, bound);
        v
    }
    /// `v`, a line's sample or a state field that a step reads back from
    /// an earlier frame: in the bounded copy of the audio-rate steps at most
    /// `LIMIT`, else possibly not even finite.
    fn source(&mut self, v: Value) -> Value {
        self.bounds.insert(v, self.limit.unwrap_or(f64::INFINITY));
        v
    }
    /// Count `x`, which a later frame reads back as a `source`, towards the
    /// frame's check that every such value is within `LIMIT`.
    fn account(&mut self, x: Value) {
        if let Some(sum) = self.squares {
            let x = match self.b.func.dfg.value_type(x) {
                F64 => self.b.ins().fdemote(F32, x),
                _ => x,
            };
            let sum = self.square_add(x, sum);
            self.squares = Some(sum);
            // Pure operations are placed where their results are used. A
            // store now and then keeps the sum's inputs from living until
            // the end of the frame.
            self.counted += 1;
            if self.counted.is_multiple_of(4) {
                let slot = *self.anchor.get_or_insert_with(|| {
                    let data = ir::StackSlotData::new(ir::StackSlotKind::ExplicitSlot, 4, 2);
                    self.b.create_sized_stack_slot(data)
                });
                self.b.ins().stack_store(I64, sum, slot, 0);
            }
        }
    }
    /// Count a run of this code in `counter`.
    #[cfg(test)]
    fn count(&mut self, counter: &std::sync::atomic::AtomicUsize) {
        let at = self.b.ins().iconst(I64, counter.as_ptr() as i64);
        let n = self.b.ins().load(I64, MemFlagsData::trusted(), at, 0);
        let n = self.b.ins().iadd_imm_s(n, 1);
        self.b.ins().store(MemFlagsData::trusted(), n, at, 0);
    }
    /// The sum of `terms`, added pairwise, so that its latency grows with
    /// the logarithm of their count. Only where the sum decides whether
    /// each term is finite, or within a bound: then any order decides the
    /// same, as a sum is not finite if a term is not, and a sum of terms of
    /// one sign is at least the largest of them.
    fn pairwise(&mut self, mut terms: Vec<Value>) -> Value {
        while terms.len() > 1 {
            terms = (terms.chunks(2))
                .map(|pair| match *pair {
                    [a, b] => self.b.ins().fadd(a, b),
                    [a] => a,
                    _ => unreachable!(),
                })
                .collect();
        }
        terms[0]
    }

    /// The sum of the squares of `xs`, in four chains added pairwise (see
    /// `pairwise`).
    fn sum_of_squares(&mut self, xs: &[Value]) -> Value {
        let zero = self.f32(0.);
        let chains: Vec<Value> = (0..4.min(xs.len().max(1)))
            .map(|c| (xs.iter().skip(c).step_by(4)).fold(zero, |sum, &x| self.square_add(x, sum)))
            .collect();
        self.pairwise(chains)
    }

    /// `x * x + sum`, fused where the processor can.
    fn square_add(&mut self, x: Value, sum: Value) -> Value {
        if fused() {
            self.b.ins().fma(x, x, sum)
        } else {
            let square = self.b.ins().fmul(x, x);
            self.b.ins().fadd(sum, square)
        }
    }
    /// Store a computed value to the value table, once.
    fn spill(&mut self, slot: Slot) {
        let s = slot as usize;
        if s < self.nodes
            && !self.stored[s]
            && let Some(v) = self.computed[s]
        {
            self.b
                .ins()
                .store(MemFlagsData::trusted(), v, self.values, 4 * s as i32);
            self.stored[s] = true;
        }
    }
    fn call(&mut self, signature: ir::SigRef, function: *const (), args: &[Value]) -> Value {
        let callee = self.b.ins().iconst(I64, function as i64);
        let call = self.b.ins().call_indirect(signature, callee, args);
        self.b.inst_results(call).first().copied().unwrap_or(callee)
    }
    fn state_of(&mut self, node: usize) -> Value {
        self.b.ins().iadd_imm_s(self.state, (node * STATE) as i64)
    }
    /// Where `node`'s `field` is kept: in its pack's row (see
    /// `pack::Layout`), else in its state.
    fn field(&mut self, node: usize, field: Field) -> (Value, i32) {
        match self.layout.and_then(|l| l.place(node, field)) {
            Some(at) => (self.delay, at),
            None => (self.state_of(node), field.offset()),
        }
    }
    /// A delay's write index, as an I64: a `u32` in a row, else a `usize`.
    fn load_index(&mut self, node: usize) -> Value {
        let t = MemFlagsData::trusted();
        match self.layout.and_then(|l| l.place(node, Field::Index)) {
            Some(at) => self.b.ins().uload32(t, self.delay, at),
            None => {
                let s = self.state_of(node);
                self.b.ins().load(I64, t, s, INDEX)
            }
        }
    }
    /// Store a delay's write index, an I64 below its length.
    fn store_index(&mut self, node: usize, index: Value) {
        let t = MemFlagsData::trusted();
        match self.layout.and_then(|l| l.place(node, Field::Index)) {
            Some(at) => {
                let index = self.b.ins().ireduce(I32, index);
                self.b.ins().store(t, index, self.delay, at);
            }
            None => {
                let s = self.state_of(node);
                self.b.ins().store(t, index, s, INDEX);
            }
        }
    }
    fn load_f32(&mut self, base: Value, offset: i32) -> Value {
        self.b
            .ins()
            .load(F32, MemFlagsData::trusted(), base, offset)
    }
    fn store(&mut self, x: Value, base: Value, offset: i32) {
        self.b.ins().store(MemFlagsData::trusted(), x, base, offset);
    }
    /// A bound on `v`'s magnitude: infinite if it may not be finite. Every
    /// signal in the value table is finite, as are inputs of steps.
    fn bound(&self, v: Value) -> f64 {
        match self.constant(v) {
            Some(c) if c.is_finite() => f64::from(c.abs()),
            Some(_) => f64::INFINITY,
            None => self.bounds.get(&v).copied().unwrap_or(FINITE),
        }
    }
    /// Guard `x` unless `bound`, a bound on its exact magnitude, shows it finite:
    /// then the guard would return it unchanged.
    fn guard(&mut self, x: Value, bound: f64) -> Value {
        if bound <= FINITE {
            self.bounds.insert(x, bound);
            x
        } else {
            self.finite(x)
        }
    }
    /// `x` if finite, else zero: the guard on every step's output.
    fn finite(&mut self, x: Value) -> Value {
        if let Some(c) = self.constant(x) {
            return self.f32(if c.is_finite() { c } else { 0. });
        }
        if self.defer {
            self.deferred = Some(x);
            return x;
        }
        // `x - x` is zero for a finite `x` and NaN otherwise.
        let d = self.b.ins().fsub(x, x);
        let finite = self.b.ins().fcmp(FloatCC::Ordered, d, d);
        let zero = self.f32(0.);
        let y = self.b.ins().select(finite, x, zero);
        self.bounds.insert(y, FINITE);
        y
    }
    /// `a.max(b)` for values that are not NaN.
    fn max(&mut self, a: Value, b: Value) -> Value {
        if let (Some(x), Some(y)) = (self.constant(a), self.constant(b)) {
            return self.f32(if x > y { x } else { y });
        }
        let greater = self.b.ins().fcmp(FloatCC::GreaterThan, a, b);
        self.b.ins().select(greater, a, b)
    }
    /// `a.min(b)` for values that are not NaN.
    fn min(&mut self, a: Value, b: Value) -> Value {
        if let (Some(x), Some(y)) = (self.constant(a), self.constant(b)) {
            return self.f32(if x < y { x } else { y });
        }
        let less = self.b.ins().fcmp(FloatCC::LessThan, a, b);
        self.b.ins().select(less, a, b)
    }
    /// `x.clamp(low, high)`.
    fn clamp(&mut self, x: Value, low: Value, high: Value) -> Value {
        if let (Some(c), Some(l), Some(h)) =
            (self.constant(x), self.constant(low), self.constant(high))
        {
            let c = if c < l { l } else { c };
            return self.f32(if c > h { h } else { c });
        }
        let below = self.b.ins().fcmp(FloatCC::LessThan, x, low);
        let x = self.b.ins().select(below, low, x);
        let above = self.b.ins().fcmp(FloatCC::GreaterThan, x, high);
        self.b.ins().select(above, high, x)
    }
    fn clamp_const(&mut self, x: Value, low: f32, high: f32) -> Value {
        let (low, high) = (self.f32(low), self.f32(high));
        self.clamp(x, low, high)
    }
    /// Refill node state `s`'s memo through `function` unless it holds `keys`.
    fn memo(&mut self, s: Value, keys: &[Value], refill: impl FnOnce(&mut Self)) {
        let valid = self.b.ins().uload8(I32, MemFlagsData::trusted(), s, VALID);
        let mut hit = self.b.ins().icmp_imm_s(IntCC::NotEqual, valid, 0);
        for (i, key) in keys.iter().enumerate() {
            let held = self
                .b
                .ins()
                .load(I32, MemFlagsData::trusted(), s, KEY + 4 * i as i32);
            let bits = self.bits(*key);
            let same = self.b.ins().icmp(IntCC::Equal, held, bits);
            hit = self.b.ins().band(hit, same);
        }
        let miss = self.b.create_block();
        let done = self.b.create_block();
        self.b.set_cold_block(miss);
        self.b.ins().brif(hit, done, &[], miss, &[]);
        self.b.switch_to_block(miss);
        refill(self);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }
    /// The memo's `i`th value, held as f64, as f32.
    fn memo_value(&mut self, s: Value, i: i32) -> Value {
        let x = self
            .b
            .ins()
            .load(F64, MemFlagsData::trusted(), s, MEMO + 8 * i);
        self.b.ins().fdemote(F32, x)
    }

    /// A cubic delay's read (`Op::Delay` with `cubic`) at write index
    /// `index` of the line at `line`.
    fn cubic(
        &mut self,
        index: Value,
        line: Value,
        seconds: Slot,
        fixed_cubic: Option<(usize, [f32; 4])>,
        length: usize,
    ) -> Value {
        let length = length as i64;
        // The taps read the samples at first, first - 1, .., first - 3,
        // around the line, where first = index + length - (whole - 1):
        // upward from `base` = first - 3, into the line's tail past its end.
        // `base` is `index`, or `index - whole`, plus `ahead`.
        let (base, ahead, weights) = match fixed_cubic {
            Some((whole, weights)) => (
                index,
                length - whole as i64 - 2,
                weights.map(|w| self.f32(w)),
            ),
            None => {
                // `cubic_taps`.
                let seconds = self.get(seconds);
                let rate = self.f32(self.rate);
                let t = self.b.ins().fmul(seconds, rate);
                let t = self.clamp_const(t, 2., (length - 5) as f32);
                let offset = self.b.ins().fpromote(F64, t);
                let whole = self.b.ins().floor(offset);
                let f = self.b.ins().fsub(offset, whole);
                let f = self.b.ins().fdemote(F32, f);
                // Between 2 and length - 5, where signed conversions are exact.
                let whole = self.b.ins().fcvt_to_sint_sat(I64, whole);
                let base = self.b.ins().isub(index, whole);
                let one = self.f32(1.);
                let two = self.f32(2.);
                let six = self.f32(6.);
                let fm1 = self.b.ins().fsub(f, one);
                let fm2 = self.b.ins().fsub(f, two);
                let fp1 = self.b.ins().fadd(f, one);
                let nf = self.b.ins().fneg(f);
                let w0 = self.b.ins().fmul(nf, fm1);
                let w0 = self.b.ins().fmul(w0, fm2);
                let w0 = self.b.ins().fdiv(w0, six);
                let w1 = self.b.ins().fmul(fp1, fm1);
                let w1 = self.b.ins().fmul(w1, fm2);
                let w1 = self.b.ins().fdiv(w1, two);
                let nfp1 = self.b.ins().fneg(fp1);
                let w2 = self.b.ins().fmul(nfp1, f);
                let w2 = self.b.ins().fmul(w2, fm2);
                let w2 = self.b.ins().fdiv(w2, two);
                let w3 = self.b.ins().fmul(fp1, f);
                let w3 = self.b.ins().fmul(w3, fm1);
                let w3 = self.b.ins().fdiv(w3, six);
                (base, length - 2, [w0, w1, w2, w3])
            }
        };
        // `whole` lies between 2 and length - 5, so `base` between 3 and
        // 2 length - 5: within a mirrored line, whose start takes `ahead`
        // into a load's constant, else within the line once wrapped.
        let (base, ahead) = if line_tail(length as usize) == length as usize {
            (base, ahead)
        } else {
            let base = self.b.ins().iadd_imm_s(base, ahead);
            let wraps = (self.b.ins()).icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, base, length);
            let wrapped = self.b.ins().iadd_imm_s(base, -length);
            (self.b.ins().select(wraps, wrapped, base), 0)
        };
        let line = self.b.ins().iadd_imm_s(line, 4 * ahead);
        let offset = self.b.ins().ishl_imm_u(base, 2);
        let address = self.b.ins().iadd(line, offset);
        let mut x = self.f32(0.);
        for (i, weight) in weights.into_iter().enumerate() {
            let sample = self.load_f32(address, 4 * (3 - i as i32));
            let term = self.b.ins().fmul(sample, weight);
            x = self.b.ins().fadd(x, term);
        }
        x
    }

    /// A linear delay's read: `Op::Delay` without `cubic`.
    fn linear(&mut self, index: Value, line: Value, seconds: Slot, length: usize) -> Value {
        let seconds = self.get(seconds);
        let rate = self.f32(self.rate);
        let offset = self.b.ins().fmul(seconds, rate);
        let offset = self.clamp_const(offset, 1., (length - 4) as f32);
        let offset = self.b.ins().fpromote(F64, offset);
        let at = self.b.ins().fcvt_from_sint(F64, index);
        let x = self.b.ins().fsub(at, offset);
        // `rem_euclid(length)`: `x` lies within `length` of zero (the index
        // is below it, the offset between 1 and `length - 4`), where `%` is
        // `x` itself. In a mirrored line, `x + length` reads the same
        // samples with the same fraction, both sums being exact.
        let span = self.b.ins().f64const(length as f64);
        let wrapped = self.b.ins().fadd(x, span);
        let position = if line_tail(length) == length {
            wrapped
        } else {
            let zero = self.b.ins().f64const(0.);
            let negative = self.b.ins().fcmp(FloatCC::LessThan, x, zero);
            self.b.ins().select(negative, wrapped, x)
        };
        let i = self.b.ins().fcvt_to_sint_sat(I64, position);
        let whole = self.b.ins().fcvt_from_sint(F64, i);
        let frac = self.b.ins().fsub(position, whole);
        let frac = self.b.ins().fdemote(F32, frac);
        let offset = self.b.ins().ishl_imm_u(i, 2);
        let address = self.b.ins().iadd(line, offset);
        let a = self.load_f32(address, 0);
        // The next sample, or past the end the line's first, in its tail.
        let b = self.load_f32(address, 4);
        let one = self.f32(1.);
        let rest = self.b.ins().fsub(one, frac);
        let a = self.b.ins().fmul(a, rest);
        let b = self.b.ins().fmul(b, frac);
        self.b.ins().fadd(a, b)
    }

    /// A field of the frame.
    fn frame_load(&mut self, ty: ir::Type, offset: usize) -> Value {
        self.b
            .ins()
            .load(ty, MemFlagsData::trusted(), self.frame, offset as i32)
    }

    /// `x % 1.` (C's `fmod`) of a finite or NaN `x`, exactly: the fraction
    /// carries `x`'s sign, a zero's included.
    fn fmod1(&mut self, x: Value) -> Value {
        let whole = self.b.ins().trunc(x);
        let fraction = self.b.ins().fsub(x, whole);
        self.b.ins().fcopysign(fraction, x)
    }

    /// `blep(t, dt)`: zero but within `dt` of a step.
    fn blep(&mut self, t: Value, dt: Value) -> Value {
        let zero = self.f32(0.);
        let one = self.f32(1.);
        let positive = self.b.ins().fcmp(FloatCC::GreaterThan, dt, zero);
        let after = self.b.ins().fcmp(FloatCC::LessThan, t, dt);
        let end = self.b.ins().fsub(one, dt);
        let before = self.b.ins().fcmp(FloatCC::GreaterThan, t, end);
        let near = self.b.ins().bor(after, before);
        let near = self.b.ins().band(near, positive);
        let edge = self.b.create_block();
        let done = self.b.create_block();
        self.b.append_block_param(done, F32);
        self.b.set_cold_block(edge);
        self.b
            .ins()
            .brif(near, edge, &[], done, &[ir::BlockArg::Value(zero)]);
        self.b.switch_to_block(edge);
        let late = self.b.ins().fsub(t, one);
        let from = self.b.ins().select(after, t, late);
        let x = self.b.ins().fdiv(from, dt);
        let twice = self.b.ins().fadd(x, x);
        let square = self.b.ins().fmul(x, x);
        // After a step: x + x - x * x - 1; before one: x * x + x + x + 1.
        let rising = self.b.ins().fsub(twice, square);
        let rising = self.b.ins().fsub(rising, one);
        let falling = self.b.ins().fadd(square, x);
        let falling = self.b.ins().fadd(falling, x);
        let falling = self.b.ins().fadd(falling, one);
        let y = self.b.ins().select(after, rising, falling);
        self.b.ins().jump(done, &[ir::BlockArg::Value(y)]);
        self.b.switch_to_block(done);
        self.b.block_params(done)[0]
    }

    /// A u64 below 2^63 as a float of type `ty`, rounded as Rust's `as` does.
    fn float_of_u64(&mut self, ty: ir::Type, x: Value) -> Value {
        self.b.ins().fcvt_from_sint(ty, x)
    }

    /// Raise the frame's loudest envelope to `level`, as `f32::max` does
    /// for a level that is not NaN.
    fn envelope_level(&mut self, level: Value) {
        let field = offset_of!(Frame, env_level);
        let loudest = self.frame_load(F32, field);
        let louder = self.b.ins().fcmp(FloatCC::GreaterThan, level, loudest);
        let loudest = self.b.ins().select(louder, level, loudest);
        self.b
            .ins()
            .store(MemFlagsData::trusted(), loudest, self.frame, field as i32);
    }

    /// A block taking one f32 and switched to by the caller later.
    fn join(&mut self) -> ir::Block {
        let block = self.b.create_block();
        self.b.append_block_param(block, F32);
        block
    }

    /// Step `k` of the schedule, guarded like every step's output.
    fn step(&mut self, k: usize, step: &Step) -> Value {
        if inline(&step.op) {
            return self.inline(step.node as usize, &step.op);
        }
        for slot in inputs(&step.op, self.args) {
            self.spill(slot);
        }
        let steps = self.b.ins().load(
            I64,
            MemFlagsData::trusted(),
            self.frame,
            offset_of!(Frame, steps) as i32,
        );
        let at = self
            .b
            .ins()
            .iadd_imm_s(steps, (k * size_of::<Step>()) as i64);
        let x = self.call(
            self.signatures.step,
            self::step as *const (),
            &[self.frame, at],
        );
        self.finite(x)
    }

    /// The audio-rate steps, each output kept as a value, then the delay
    /// writes. Only values read from memory are stored: the voice's outputs,
    /// the probe, the lifetime node, the inputs of called steps and quiet
    /// nodes. A resting quiet node is skipped (see `Quiet`); `flags` holds
    /// `MOVED` and `FRESH`.
    fn audio(&mut self, p: &VoicePatch, flags: Value) -> anyhow::Result<()> {
        let mut store = vec![false; self.nodes];
        let mut export = |slot: Slot| {
            if let Some(s) = store.get_mut(slot as usize) {
                *s = true;
            }
        };
        [Some(p.output), p.right, p.listen]
            .into_iter()
            .flatten()
            .for_each(&mut export);
        if let Some((index, ..)) = p.lifetime {
            export(index as Slot);
        }
        for s in &p.steps[p.settling..] {
            if !inline(&s.op) {
                inputs(&s.op, &p.args).into_iter().for_each(&mut export);
            }
        }
        let watched = self.watched(p);
        let flag = |g: &mut Self, bit: u64| {
            let set = g.b.ins().band_imm_s(flags, bit as i64);
            g.b.ins().icmp_imm_s(IntCC::NotEqual, set, 0)
        };
        let fresh = flag(self, FRESH);
        let control = flag(self, CONTROL_RAN);
        let settled = flag(self, SETTLING_MOVED);
        // Quiet sources, then clusters.
        for (k, s) in p.steps.iter().enumerate().skip(p.settling) {
            let node = s.node as usize;
            if self.quiet[node] != Quiet::Source {
                continue;
            }
            crate::host::check_cancelled()?;
            self.loaded.fill(None);
            let old = watched[node].then(|| self.load_f32(self.values, 4 * node as i32));
            let x = self.step(k, s);
            self.computed[node] = Some(x);
            self.spill(node as Slot);
            if let Some(old) = old {
                let [old, new] = [old, x].map(|v| self.bits(v));
                self.moved[node] = Some(self.b.ins().icmp(IntCC::NotEqual, old, new));
            }
        }
        let audible: Vec<usize> = (0..self.clusters.len())
            .filter(|&c| self.clusters[c][0] >= p.settling)
            .collect();
        self.rouse(p, &audible, [Some(control), Some(settled)]);
        for c in audible {
            crate::host::check_cancelled()?;
            self.cluster(p, c, &watched, fresh, None);
        }
        // Later readers load sources from the value table.
        for s in &p.steps[p.settling..] {
            if self.quiet[s.node as usize] == Quiet::Source {
                self.computed[s.node as usize] = None;
            }
        }
        // The other audio-rate steps and the delay writes run in one of two
        // copies. Where every line sample and state field they read back
        // from an earlier frame, and every input they read from the value
        // table, lies within `LIMIT`, most of their outputs are provably
        // finite, and the bounded copy leaves out their guards. Each copy
        // sums the squares of the values a later frame reads back; while
        // that is not within `LIMIT` squared, `wild` counts the frames until
        // the samples written can no longer be read, and the other copy runs.
        let order = Order::new(p, self.quiet, self.nodes, None);
        // The bounded copy runs packs of steps and writes (see `pack`).
        let (packs, packed) = (self.packs, self.packed);
        let mut inputs_read: Vec<usize> = (order.items.iter())
            .flat_map(|item| match *item {
                Item::Step(k) | Item::Part(k, _) => inputs(&p.steps[k].op, self.args),
                Item::Write(w) => vec![p.delays[w].1],
                Item::Pack(_) => unreachable!("the guarded copy runs no packs"),
            })
            .map(|slot| slot as usize)
            // Outputs of quiet nodes and earlier tiers, beyond what their
            // operations bound.
            .filter(|&s| s < self.nodes && (self.quiet[s] != Quiet::No || self.tiers[s] < AUDIO))
            .filter(|&s| self.statics[s] > LIMIT)
            .collect();
        inputs_read.sort_unstable();
        inputs_read.dedup();
        let zero = self.f32(0.);
        let read: Vec<Value> = (inputs_read.into_iter())
            .map(|s| self.load_f32(self.values, 4 * s as i32))
            .collect();
        let sum = self.sum_of_squares(&read);
        let most = self.f32((LIMIT * LIMIT) as f32);
        let small = self.b.ins().fcmp(FloatCC::LessThanOrEqual, sum, most);
        let wild = self.frame_load(I32, offset_of!(Frame, wild));
        let calm = self.b.ins().icmp_imm_s(IntCC::Equal, wild, 0);
        let calm = self.b.ins().band(calm, small);
        let bounded = self.b.create_block();
        let unbounded = self.b.create_block();
        let done = self.b.create_block();
        self.b.append_block_param(done, F32);
        self.b.ins().brif(calm, bounded, &[], unbounded, &[]);
        let saved = (
            self.computed.clone(),
            self.stored.clone(),
            self.lines.clone(),
        );
        for (block, limit) in [(bounded, Some(LIMIT)), (unbounded, None)] {
            self.b.switch_to_block(block);
            #[cfg(test)]
            self.count(&RAN[usize::from(limit.is_none())]);
            (self.computed, self.stored, self.lines) = saved.clone();
            self.returns.fill(f64::INFINITY);
            self.limit = limit;
            self.squares = Some(zero);
            self.vectors.clear();
            match (limit, packed, packs) {
                (Some(_), Some(packed), Some(packs)) => {
                    self.rest(p, packed, &store, Some(packs))?
                }
                _ => self.rest(p, &order, &store, None)?,
            }
            self.reduce_squares();
            let sum = self.squares.take().expect("summed");
            self.b.ins().jump(done, &[ir::BlockArg::Value(sum)]);
        }
        self.limit = None;
        self.b.switch_to_block(done);
        let sum = self.b.block_params(done)[0];
        let small = self.b.ins().fcmp(FloatCC::LessThanOrEqual, sum, most);
        let moving = self.b.ins().icmp_imm_s(IntCC::NotEqual, wild, 0);
        let moving = self.b.ins().uextend(I32, moving);
        let calmer = self.b.ins().isub(wild, moving);
        let span = p.delays.iter().map(|d| d.3).max().unwrap_or(0) as i64 + 1;
        let span = self.b.ins().iconst(I32, span);
        let wild = self.b.ins().select(small, calmer, span);
        self.b.ins().store(
            MemFlagsData::trusted(),
            wild,
            self.frame,
            offset_of!(Frame, wild) as i32,
        );
        Ok(())
    }

    /// The audio-rate steps that are not quiet and the delay writes, in an
    /// order of their own (see `Order`), with their guards checked a group
    /// at a time (see `flush`).
    fn rest(
        &mut self,
        p: &VoicePatch,
        order: &Order,
        store: &[bool],
        packs: Option<&Packs>,
    ) -> anyhow::Result<()> {
        let mut left = order.deps.iter().map(Vec::len).collect::<Vec<_>>();
        let mut ready: std::collections::BTreeSet<(usize, usize)> = (0..order.items.len())
            .filter(|&i| left[i] == 0)
            .map(|i| (order.rank[i], i))
            .collect();
        let mut last = None;
        while let Some(&(head, _)) = ready.first() {
            crate::host::check_cancelled()?;
            // The first item in order that reads no unchecked value, within
            // the window; else check the group and take the first.
            let pending = &self.pending;
            let unchecked = |i: usize| {
                order.deps[i].iter().any(|&(j, value)| {
                    value
                        && match order.items[j] {
                            Item::Step(k) => pending
                                .iter()
                                .any(|(t, _)| *t == Target::Node(p.steps[k].node as usize)),
                            Item::Part(..) | Item::Write(_) | Item::Pack(_) => false,
                        }
                })
            };
            let pick = ready
                .iter()
                .take_while(|&&(rank, _)| rank <= head + WINDOW)
                .find(|&&(_, i)| !unchecked(i))
                .copied();
            let Some((rank, i)) = pick else {
                self.flush(store);
                continue;
            };
            ready.remove(&(rank, i));
            // A load from the value table is as cheap as reloading a spill.
            self.loaded.fill(None);
            match order.items[i] {
                Item::Step(k) => {
                    let s = &p.steps[k];
                    let node = s.node as usize;
                    self.defer = true;
                    let x = self.step(k, s);
                    self.defer = false;
                    self.computed[node] = Some(x);
                    match self.deferred.take() {
                        Some(x) => self.pending.push((Target::Node(node), x)),
                        None if store[node] => self.spill(node as Slot),
                        None => {}
                    }
                }
                Item::Part(k, j) => {
                    let s = &p.steps[k];
                    self.fold(s.node as usize, &s.op, j + 1);
                }
                Item::Write(d) => {
                    let (line, input, base, length) = p.delays[d];
                    // Right after its delay's step its values are at hand;
                    // later, reading them back is cheaper than keeping them.
                    let fresh =
                        matches!(last, Some(Item::Step(k)) if p.steps[k].node as usize == line);
                    self.defer = true;
                    self.write_delay(line, input, base, length, fresh);
                    self.defer = false;
                }
                Item::Pack(pack) => {
                    let packs = packs.expect("packs run in the bounded copy");
                    self.pack(p, packs, pack, store);
                }
            }
            last = Some(order.items[i]);
            for &r in &order.readers[i] {
                left[r] -= 1;
                if left[r] == 0 {
                    ready.insert((order.rank[r], r));
                }
            }
            if self.pending.len() >= GROUP {
                self.flush(store);
            }
        }
        self.flush(store);
        Ok(())
    }

    /// Check the outputs whose guards were deferred at once: if their sum is
    /// finite, so is each of them, and the guards would return them as they
    /// are. Otherwise a cold path guards each one as the interpreter does.
    /// A deferred line value is written here, and an exported output stored.
    fn flush(&mut self, store: &[bool]) {
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return;
        }
        let raw: Vec<Value> = pending.iter().map(|&(_, x)| x).collect();
        let sum = self.pairwise(raw.clone());
        let d = self.b.ins().fsub(sum, sum);
        let finite = self.b.ins().fcmp(FloatCC::Ordered, d, d);
        let fix = self.b.create_block();
        let join = self.b.create_block();
        for _ in &raw {
            self.b.append_block_param(join, F32);
            self.b.append_block_param(fix, F32);
        }
        self.b.set_cold_block(fix);
        // Both ways the values are passed on, so they may stay where they are.
        let args: Vec<_> = raw.iter().map(|&x| ir::BlockArg::Value(x)).collect();
        self.b.ins().brif(finite, join, &args, fix, &args);
        self.b.switch_to_block(fix);
        #[cfg(test)]
        self.count(&RAN[2]);
        let fixed: Vec<_> = (self.b.block_params(fix).to_vec().into_iter())
            .map(|x| ir::BlockArg::Value(self.finite(x)))
            .collect();
        self.b.ins().jump(join, &fixed);
        self.b.switch_to_block(join);
        let checked = self.b.block_params(join).to_vec();
        for ((target, _), x) in pending.into_iter().zip(checked) {
            self.bounds.insert(x, FINITE);
            match target {
                Target::Node(node) => {
                    self.computed[node] = Some(x);
                    if store[node] {
                        self.spill(node as Slot);
                    }
                }
                Target::Line(address, tail, offset) => {
                    self.store(x, address, 0);
                    self.store(x, tail, offset);
                    self.account(x);
                }
            }
        }
    }

    /// `run` over the event-rate steps in `range`: those whose sources meet
    /// `active`. Each reads its inputs from and writes its output to the
    /// value table, where they persist between frames.
    fn tier(
        &mut self,
        p: &VoicePatch,
        range: std::ops::Range<usize>,
        active: Value,
    ) -> anyhow::Result<Value> {
        // Whether each node a cluster reads moved this frame, for the
        // clusters (see `Gen::cluster`): not unless it runs.
        let still = self.b.ins().iconst(I8, 0);
        for s in &p.steps[range.clone()] {
            if self.flagged[s.node as usize] {
                let state = self.state_of(s.node as usize);
                self.b
                    .ins()
                    .store(MemFlagsData::trusted(), still, state, MOVED);
            }
        }
        let mut k = range.start;
        // Consecutive steps that follow the same sources share one test.
        while k < range.end {
            let sources = p.sources[k];
            let end = (k..range.end)
                .find(|&j| p.sources[j] != sources)
                .unwrap_or(range.end);
            let run = self.b.create_block();
            let next = self.b.create_block();
            let follows = self.b.ins().band_imm_s(active, sources as i64);
            self.b.ins().brif(follows, run, &[], next, &[]);
            self.b.switch_to_block(run);
            for (j, s) in p.steps.iter().enumerate().take(end).skip(k) {
                crate::host::check_cancelled()?;
                self.loaded.fill(None);
                let node = s.node as usize;
                let old = self.flagged[node].then(|| {
                    self.b
                        .ins()
                        .load(I32, MemFlagsData::trusted(), self.values, 4 * node as i32)
                });
                let x = self.step(j, s);
                self.store(x, self.values, 4 * node as i32);
                self.stored[node] = true;
                self.computed[node] = Some(x);
                if let Some(old) = old {
                    let bits = self.bits(x);
                    let moved = self.b.ins().icmp(IntCC::NotEqual, old, bits);
                    let state = self.state_of(node);
                    self.b
                        .ins()
                        .store(MemFlagsData::trusted(), moved, state, MOVED);
                }
            }
            self.b.ins().jump(next, &[]);
            self.b.switch_to_block(next);
            // Values of the group's block do not reach the next.
            for s in &p.steps[k..end] {
                self.computed[s.node as usize] = None;
            }
            k = end;
        }
        self.refill_hoisted(p, CONTROL);
        Ok(self.b.ins().iconst(I8, 1))
    }

    /// The settling steps, by clusters. A cluster runs when it follows one of
    /// the sources that `changed` this frame, when one of its inputs moved
    /// this frame, or when it has not come to rest: a cluster whose last run
    /// changed no smoother's output or state rests. Returns whether every
    /// step that ran left its output and state as they were.
    fn settle(&mut self, p: &VoicePatch, changed: Value) -> anyhow::Result<Value> {
        // Whether a step changed anything accumulates in memory: a store pins
        // each comparison to its step, where its operands are at hand.
        let differs = self.b.create_sized_stack_slot(ir::StackSlotData::new(
            ir::StackSlotKind::ExplicitSlot,
            1,
            0,
        ));
        let zero = self.b.ins().iconst(I8, 0);
        self.b.ins().stack_store(I64, zero, differs, 0);
        let watched = self.watched(p);
        let settling: Vec<usize> = (0..self.clusters.len())
            .filter(|&c| (p.control..p.settling).contains(&self.clusters[c][0]))
            .collect();
        let control = self.b.ins().icmp_imm_s(IntCC::NotEqual, changed, 0);
        self.rouse(p, &settling, [Some(control), None]);
        for c in settling {
            crate::host::check_cancelled()?;
            let asleep = self.b.ins().iconst(I8, 0);
            self.cluster(p, c, &watched, asleep, Some(differs));
        }
        // Clusters refill the memos that follow them as they run; an
        // event-rate input of one may have changed only when `changed`.
        let refill = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(changed, refill, &[], done, &[]);
        self.b.switch_to_block(refill);
        self.refill_hoisted(p, SETTLING);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        let differ = self.b.ins().stack_load(I64, I8, differs, 0);
        Ok(self.b.ins().icmp_imm_s(IntCC::Equal, differ, 0))
    }

    /// Nodes read by a cluster other than their own, which report whether
    /// they moved.
    fn watched(&self, p: &VoicePatch) -> Vec<bool> {
        let mut watched = vec![false; self.nodes];
        for s in &p.steps[p.control..] {
            let c = self.cluster[s.node as usize];
            if c == usize::MAX {
                continue;
            }
            for input in inputs(&s.op, &p.args) {
                if let Some(w) = watched.get_mut(input as usize) {
                    *w |= self.cluster[input as usize] != c;
                }
            }
        }
        watched
    }

    /// Whether the output moved or any `(old, new)` pair of state differs.
    fn differ(&mut self, moved: Value, pairs: Vec<(Value, Value)>) -> Value {
        let mut differ = moved;
        for (a, b) in pairs {
            let [a, b] = [a, b].map(|v| match self.b.func.dfg.value_type(v) {
                F32 => self.b.ins().bitcast(I32, MemFlagsData::new(), v),
                _ => v,
            });
            let ne = self.b.ins().icmp(IntCC::NotEqual, a, b);
            differ = self.b.ins().bor(differ, ne);
        }
        differ
    }

    /// Inputs of audio-rate memos hoisted to `tier` have their values for
    /// the frame now: refill those memos here, where they can change,
    /// instead of testing them every frame. Few values are live here, so the
    /// refills are plain calls.
    fn refill_hoisted(&mut self, p: &VoicePatch, tier: u8) {
        for s in &p.steps[p.control..] {
            if self.hoisted[s.node as usize] == tier {
                self.loaded.fill(None);
                self.refresh(s);
            }
        }
    }

    /// For cluster `c`, if its slew can pass the ends of the shapes that
    /// read it for good (see `cluster`): the slew's input, and the inputs
    /// from which each of those shapes gives its first and its last value
    /// (see `ends_of`). The cluster's other steps read only the slew, each
    /// other and constants; the slew's input is bounded far below f32's
    /// range by its operation, and its poles need no memo test.
    fn tail_of(&self, p: &VoicePatch, c: usize) -> Option<(Slot, Vec<(f32, f32)>)> {
        let steps = &self.clusters[c];
        let s = &p.steps[steps[0]];
        let root = s.node as usize;
        let Op::Slew { input, rise, fall } = &s.op else {
            return None;
        };
        let constant = |t: &Slot| *t as usize >= self.nodes;
        if self.hoisted[root] == CONSTANT && !(constant(rise) && constant(fall)) {
            return None;
        }
        if !constant(input) && self.statics[*input as usize] > 1e30 {
            return None;
        }
        let mut ends = Vec::new();
        for &k in &steps[1..] {
            let s = &p.steps[k];
            let reads = inputs(&s.op, self.args);
            let within = |i: &Slot| constant(i) || self.cluster[*i as usize] == c;
            if self.quiet[s.node as usize] != Quiet::Pure || !reads.iter().all(within) {
                return None;
            }
            if reads.contains(&(root as Slot)) {
                let Op::RawShape { shape, .. } = &s.op else {
                    return None;
                };
                let (table, low, step) = shape.table();
                let ((below, _), (above, _)) = ends_of(table, low, step);
                if !ends.contains(&(below, above)) {
                    ends.push((below, above));
                }
            }
        }
        (!ends.is_empty()).then_some((*input, ends))
    }

    /// Cluster `c`: run when `wake`, when one of its inputs moved this frame,
    /// or when its last run changed a smoother's output or state; otherwise
    /// it rests. Given `earlier` (the audio-rate tier: whether event-rate and
    /// settling outputs may have changed), those inputs count as moved then.
    /// Given `differs`, every member's output is compared, for the settling
    /// tier's result. Outputs are left in the value table either way.
    fn cluster(
        &mut self,
        p: &VoicePatch,
        c: usize,
        watched: &[bool],
        wake: Value,
        differs: Option<ir::StackSlot>,
    ) {
        let steps = &self.clusters[c];
        let root = p.steps[steps[0]].node as usize;
        let state = self.state_of(root);
        let rests = self
            .b
            .ins()
            .uload8(I32, MemFlagsData::trusted(), state, RESTING);
        let restless = self.b.ins().icmp_imm_s(IntCC::Equal, rests, 0);
        // Inputs of earlier functions roused it already (see `rouse`).
        let mut forced = wake;
        let mut seen = Vec::new();
        for &k in steps {
            for input in inputs(&p.steps[k].op, self.args) {
                let i = input as usize;
                if i >= self.nodes || seen.contains(&i) || self.cluster[i] == c {
                    continue;
                }
                seen.push(i);
                if let Some(moved) = self.moved[i] {
                    forced = self.b.ins().bor(forced, moved);
                }
            }
        }
        let run = self.b.ins().bor(forced, restless);
        let go = self.b.create_block();
        let next = self.b.create_block();
        let tail = self.tail_of(p, c);
        let exported: Vec<usize> = steps
            .iter()
            .map(|&k| p.steps[k].node as usize)
            .filter(|&n| watched[n])
            .collect();
        for _ in &exported {
            self.b.append_block_param(next, I8);
        }
        let still = self.b.ins().iconst(I8, 0);
        let rest = vec![ir::BlockArg::Value(still); exported.len()];
        match &tail {
            None => {
                self.b.ins().brif(run, go, &[], next, &rest);
            }
            Some((input, ends)) => {
                // The slew moves towards its input and never past it. Once
                // both lie past an end of each shape that reads the slew,
                // the shapes keep their values: the slew alone moves. Its
                // input, and so every value it holds, is far below f32's
                // range, and its first frame has passed.
                let check = self.b.create_block();
                let lean = self.b.create_block();
                let t = MemFlagsData::trusted();
                // A slew whose input changes only at events, once it runs
                // alone, does so until its input moves, when `rouse` clears
                // the flag: it stays past the ends it passed.
                let flagged = self.lean_flag(p, c);
                if flagged {
                    let known = self.b.create_block();
                    self.b.ins().brif(run, known, &[], next, &rest);
                    self.b.switch_to_block(known);
                    let flag = self.b.ins().uload8(I32, t, state, LEAN);
                    self.b.ins().brif(flag, lean, &[], check, &[]);
                } else {
                    self.b.ins().brif(run, check, &[], next, &rest);
                }
                self.b.switch_to_block(check);
                self.loaded.fill(None);
                let x = self.get(*input);
                let held = self.load_f32(state, ENV);
                let index = self.b.ins().load(I64, t, state, INDEX);
                let mut lean_now = self.b.ins().icmp_imm_s(IntCC::NotEqual, index, 0);
                for &(below, above) in ends {
                    let [below, above] = [below, above].map(|e| self.f32(e));
                    let [low, high] = [
                        (FloatCC::LessThanOrEqual, below),
                        (FloatCC::GreaterThanOrEqual, above),
                    ]
                    .map(|(cc, end)| {
                        let a = self.b.ins().fcmp(cc, x, end);
                        let b = self.b.ins().fcmp(cc, held, end);
                        self.b.ins().band(a, b)
                    });
                    let past = self.b.ins().bor(low, high);
                    lean_now = self.b.ins().band(lean_now, past);
                }
                let one = self.b.ins().iconst(I8, 1);
                if flagged {
                    let mark = self.b.create_block();
                    self.b.ins().brif(lean_now, mark, &[], go, &[]);
                    self.b.switch_to_block(mark);
                    self.b.ins().store(t, one, state, LEAN);
                    self.b.ins().jump(lean, &[]);
                } else {
                    self.b.ins().brif(lean_now, lean, &[], go, &[]);
                }
                self.b.switch_to_block(lean);
                let changed = self.tail_step(p, steps[0], *input, state);
                let moved: Vec<_> = (exported.iter())
                    .map(|&n| ir::BlockArg::Value(if n == root { changed } else { still }))
                    .collect();
                let rests = self.b.ins().bxor(changed, one);
                self.b.ins().store(t, rests, state, RESTING);
                if let Some(differs) = differs {
                    let all = self.b.ins().stack_load(I64, I8, differs, 0);
                    let all = self.b.ins().bor(all, changed);
                    self.b.ins().stack_store(I64, all, differs, 0);
                }
                self.b.ins().jump(next, &moved);
            }
        }
        self.b.switch_to_block(go);
        let mut moved = Vec::new();
        let mut shared = std::collections::HashMap::new();
        // Whether a smoother changed, so that the cluster must run again,
        // and whether that or an output another reads moved.
        let mut differ = still;
        let mut report = still;
        for &k in steps {
            let s = &p.steps[k];
            let node = s.node as usize;
            self.loaded.fill(None);
            let smoother = self.quiet[node] == Quiet::Resting;
            let old = (smoother || watched[node]).then(|| {
                self.b
                    .ins()
                    .load(I32, MemFlagsData::trusted(), self.values, 4 * node as i32)
            });
            // Inline operations report the state they change; a called one
            // is compared whole.
            self.changes = smoother.then(Vec::new);
            let before = (smoother && !inline(&s.op)).then(|| self.smoother_state(node));
            // Raw shapes of one input over one domain, as a cluster's
            // shapes of a smoother often are, share their lookup.
            let domain = |s: &Step| match &s.op {
                Op::RawShape { input, shape } => {
                    let (table, low, step) = shape.table();
                    Some((*input, table.len(), low.to_bits(), step.to_bits()))
                }
                _ => None,
            };
            if let Some(d) = domain(s)
                && !shared.contains_key(&k)
            {
                let group: Vec<usize> = (steps.iter().copied())
                    .skip_while(|&j| j != k)
                    .take_while(|&j| domain(&p.steps[j]) == Some(d))
                    .collect();
                let shapes: Vec<&shape::Shape> = (group.iter())
                    .map(|&j| match &p.steps[j].op {
                        Op::RawShape { shape, .. } => &**shape,
                        _ => unreachable!(),
                    })
                    .collect();
                let ys = self.raw_shapes(d.0, &shapes, true);
                shared.extend(group.into_iter().zip(ys));
            }
            let x = match shared.get(&k) {
                Some(&x) => x,
                None => self.step(k, s),
            };
            self.computed[node] = Some(x);
            self.store(x, self.values, 4 * node as i32);
            let Some(old) = old else { continue };
            let bits = self.bits(x);
            let changed = self.b.ins().icmp(IntCC::NotEqual, old, bits);
            if watched[node] {
                moved.push(ir::BlockArg::Value(changed));
                report = self.b.ins().bor(report, changed);
            }
            if smoother {
                let mut pairs = self.changes.take().unwrap_or_default();
                if let Some(before) = before {
                    pairs.extend(before.into_iter().zip(self.smoother_state(node)));
                }
                let changed = self.differ(changed, pairs);
                differ = self.b.ins().bor(differ, changed);
            }
        }
        self.changes = None;
        for &node in &self.refills_after[c] {
            let k = p.steps.iter().position(|s| s.node as usize == node);
            self.loaded.fill(None);
            self.refresh(&p.steps[k.expect("a memo node runs")]);
        }
        // A cluster without smoothers rests once it has run.
        let rests = self.b.ins().icmp_imm_s(IntCC::Equal, differ, 0);
        self.b
            .ins()
            .store(MemFlagsData::trusted(), rests, state, RESTING);
        if let Some(differs) = differs {
            let report = self.b.ins().bor(report, differ);
            let all = self.b.ins().stack_load(I64, I8, differs, 0);
            let all = self.b.ins().bor(all, report);
            self.b.ins().stack_store(I64, all, differs, 0);
        }
        self.b.ins().jump(next, &moved);
        self.b.switch_to_block(next);
        // Outputs are read back from the value table from here on.
        for &k in steps {
            let node = p.steps[k].node as usize;
            self.computed[node] = None;
            self.stored[node] = true;
        }
        for (n, moved) in exported.into_iter().zip(self.b.block_params(next).to_vec()) {
            self.moved[n] = Some(moved);
            // For the audio-rate clusters that read it.
            if self.flagged[n] && self.tiers[n] == SETTLING {
                let state = self.state_of(n);
                self.b
                    .ins()
                    .store(MemFlagsData::trusted(), moved, state, MOVED);
            }
        }
    }

    /// Whether cluster `c` keeps a flag that its slew runs alone (see
    /// `cluster`): its slew may (see `tail_of`), and its input is a
    /// constant or an event-rate node, which `rouse` sees move.
    fn lean_flag(&self, p: &VoicePatch, c: usize) -> bool {
        self.tail_of(p, c).is_some_and(|(input, _)| {
            input as usize >= self.nodes || self.tiers[input as usize] == CONTROL
        })
    }

    /// Wake `clusters` whose inputs from earlier functions moved: event-rate
    /// nodes when `gates[0]` (that function ran), settling nodes when
    /// `gates[1]`. Each flags whether it moved (see `flagged`); a cluster
    /// with one that did no longer rests. Nothing runs unless a gate is set.
    fn rouse(&mut self, p: &VoicePatch, clusters: &[usize], gates: [Option<Value>; 2]) {
        let mut wakes = Vec::new();
        for &c in clusters {
            let mut reads = [Vec::new(), Vec::new()];
            for &k in &self.clusters[c] {
                for i in inputs(&p.steps[k].op, self.args) {
                    let i = i as usize;
                    if i < self.nodes && self.cluster[i] != c {
                        match self.tiers[i] {
                            CONTROL if gates[0].is_some() => reads[0].push(i),
                            SETTLING if gates[1].is_some() => reads[1].push(i),
                            _ => {}
                        }
                    }
                }
            }
            if reads.iter().any(|r| !r.is_empty()) {
                wakes.push((c, reads));
            }
        }
        if wakes.is_empty() {
            return;
        }
        let still = self.b.ins().iconst(I8, 0);
        let any = (gates.iter().flatten()).fold(still, |any, &g| self.b.ins().bor(any, g));
        let rouse = self.b.create_block();
        let done = self.b.create_block();
        self.b.set_cold_block(rouse);
        self.b.ins().brif(any, rouse, &[], done, &[]);
        self.b.switch_to_block(rouse);
        for (c, reads) in wakes {
            let mut moved = still;
            for (mut nodes, gate) in reads.into_iter().zip(gates) {
                if nodes.is_empty() {
                    continue;
                }
                nodes.sort_unstable();
                nodes.dedup();
                let gate = gate.expect("the earlier function flags the inputs of clusters");
                let mut any = still;
                for i in nodes {
                    let state = self.state_of(i);
                    let flag = self.b.ins().load(I8, MemFlagsData::trusted(), state, MOVED);
                    any = self.b.ins().bor(any, flag);
                }
                let any = self.b.ins().band(any, gate);
                moved = self.b.ins().bor(moved, any);
            }
            let root = self.state_of(p.steps[self.clusters[c][0]].node as usize);
            let t = MemFlagsData::trusted();
            let rests = self.b.ins().load(I8, t, root, RESTING);
            let one = self.b.ins().iconst(I8, 1);
            let still_resting = self.b.ins().bxor(moved, one);
            let rests = self.b.ins().band(rests, still_resting);
            self.b.ins().store(t, rests, root, RESTING);
            if self.lean_flag(p, c) {
                let lean = self.b.ins().load(I8, t, root, LEAN);
                let lean = self.b.ins().band(lean, still_resting);
                self.b.ins().store(t, lean, root, LEAN);
            }
        }
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// Bring hoisted step `s`'s memo up to date.
    fn refresh(&mut self, s: &Step) {
        let state = self.state_of(s.node as usize);
        match &s.op {
            Op::Filter {
                cutoff, q, mode, ..
            } => {
                let cutoff = self.get(*cutoff);
                let q = self.get(*q);
                self.filter_coefficients(state, s.node as usize, cutoff, q, *mode >= 4);
            }
            Op::Delay {
                damping: Some(cutoff),
                ..
            } => {
                let cutoff = self.get(*cutoff);
                self.damping(state, cutoff);
            }
            Op::Slew { rise, fall, .. } => {
                // Its two poles, for the times as `eval` clamps them, keyed
                // by both; zero for a time of zero.
                let times = [*rise, *fall].map(|t| self.get(t));
                self.memo(state, &times, |g| {
                    let poles = times.map(|time| {
                        let time = g.clamp_const(time, 0., 60.);
                        let rate = g.f32(g.rate);
                        let pole =
                            g.call(g.signatures.binary, slew_pole as *const (), &[time, rate]);
                        let zero = g.f32(0.);
                        let instant = g.b.ins().fcmp(FloatCC::Equal, time, zero);
                        g.b.ins().select(instant, zero, pole)
                    });
                    g.fill_f32(state, s.node as usize, &times, &poles);
                });
            }
            _ => unreachable!("only filters, damped delays and slews are hoisted"),
        }
    }

    /// `State::filter_memo`: node `node`'s filter's coefficients for
    /// `cutoff` and `q`, with its memo's key in `state`. A one-pole filter
    /// reads only the first, the gain, and refills only it.
    fn filter_coefficients(
        &mut self,
        state: Value,
        node: usize,
        cutoff: Value,
        q: Value,
        one_pole: bool,
    ) {
        self.memo(state, &[cutoff, q], |g| {
            // `filter_coefficients`.
            let c = g.clamp_const(cutoff, FILTER_FLOOR, g.rate * 0.45);
            let pi = g.f32(std::f32::consts::PI);
            let t = g.b.ins().fmul(pi, c);
            let rate = g.f32(g.rate);
            let t = g.b.ins().fdiv(t, rate);
            let gain = g.tan(t);
            let one = g.f32(1.);
            if one_pole {
                // And 1 + g, by which a step is divided.
                let divisor = g.b.ins().fadd(gain, one);
                g.fill_f32(state, node, &[cutoff, q], &[gain, divisor]);
                return;
            }
            let qc = g.clamp_const(q, FILTER_Q[0], FILTER_Q[1]);
            let k = g.b.ins().fdiv(one, qc);
            let d = g.b.ins().fadd(gain, k);
            let d = g.b.ins().fmul(gain, d);
            let d = g.b.ins().fadd(one, d);
            let a = g.b.ins().fdiv(one, d);
            // And g a, by which the band takes its input.
            let ga = g.b.ins().fmul(gain, a);
            g.fill_f32(state, node, &[cutoff, q], &[gain, k, a, ga]);
        });
    }

    /// `tan::tan` of a filter's argument, `pi c / rate` with `c` clamped
    /// to `0.45 rate` at most: inline, the f64 operations of
    /// `tan::approximate` rounded to f32, where every such argument lies
    /// within the tangent's domain, as it does at any rate above 23 Hz.
    fn tan(&mut self, x: Value) -> Value {
        let largest = std::f32::consts::PI * (self.rate * 0.45) / self.rate;
        if largest > tan::DOMAIN {
            return self.call(self.signatures.unary, tan as *const (), &[x]);
        }
        let x = self.b.ins().fpromote(F64, x);
        let z = self.b.ins().fmul(x, x);
        let z2 = self.b.ins().fmul(z, z);
        // (k0 + k1 z) + z2 ((k2 + k3 z) + z2 (k4 + k5 z)), where k5 z is
        // -z in the numerator.
        let estrin = |g: &mut Self, k: [f64; 6]| {
            let pair = |g: &mut Self, a: f64, b: f64| {
                let a = g.b.ins().f64const(a);
                if b == -1. {
                    return g.b.ins().fsub(a, z);
                }
                let b = g.b.ins().f64const(b);
                let bz = g.b.ins().fmul(b, z);
                g.b.ins().fadd(a, bz)
            };
            let [low, middle, high] = [0, 2, 4].map(|i| pair(g, k[i], k[i + 1]));
            let high = g.b.ins().fmul(z2, high);
            let inner = g.b.ins().fadd(middle, high);
            let inner = g.b.ins().fmul(z2, inner);
            g.b.ins().fadd(low, inner)
        };
        let p = estrin(self, tan::P);
        let q = estrin(self, tan::Q);
        let xp = self.b.ins().fmul(x, p);
        let y = self.b.ins().fdiv(xp, q);
        self.b.ins().fdemote(F32, y)
    }

    /// Fill a memo with f32 values, which only compiled code reads: a
    /// filter's coefficients and the products a frame of it would compute,
    /// or a slew's two poles.
    /// Its key goes to `state`, node `node`'s; its values where the node
    /// keeps them (see `field`), and in a pack's first lane, to the lanes
    /// past the pack's last too.
    fn fill_f32(&mut self, state: Value, node: usize, keys: &[Value], values: &[Value]) {
        let t = MemFlagsData::trusted();
        let valid = self.b.ins().iconst(I8, 1);
        self.b.ins().store(t, valid, state, VALID);
        for (i, key) in keys.iter().enumerate() {
            let bits = self.bits(*key);
            self.b.ins().store(t, bits, state, KEY + 4 * i as i32);
        }
        for (i, &x) in values.iter().enumerate() {
            let field = Field::Memo(i as i32);
            let (base, at) = self.field(node, field);
            self.b.ins().store(t, x, base, at);
            for at in self.layout.map_or(Vec::new(), |l| l.beyond(node, field)) {
                self.b.ins().store(t, x, self.delay, at);
            }
        }
    }

    /// `State::damping_memo`: a damped delay's feedback pole for `cutoff`.
    fn damping(&mut self, state: Value, cutoff: Value) {
        let zero_bits = self.b.ins().iconst(I32, 0);
        self.memo(state, &[cutoff, zero_bits], |g| {
            let c = g.clamp_const(cutoff, 1., g.rate * 0.45);
            let tau = g.f32(-TAU);
            let t = g.b.ins().fmul(tau, c);
            let rate = g.f32(g.rate);
            let t = g.b.ins().fdiv(t, rate);
            let a = g.call(g.signatures.unary, exp as *const (), &[t]);
            let zero = g.f32(0.);
            g.fill(state, &[cutoff, zero_bits], [a, zero, zero]);
        });
    }

    /// Fill a memo: its key's bits and its values, held as f64.
    fn fill(&mut self, state: Value, keys: &[Value], values: [Value; 3]) {
        let t = MemFlagsData::trusted();
        let valid = self.b.ins().iconst(I8, 1);
        self.b.ins().store(t, valid, state, VALID);
        for (i, key) in keys.iter().enumerate() {
            let bits = self.bits(*key);
            self.b.ins().store(t, bits, state, KEY + 4 * i as i32);
        }
        for (i, x) in values.into_iter().enumerate() {
            let x = self.b.ins().fpromote(F64, x);
            self.b.ins().store(t, x, state, MEMO + 8 * i as i32);
        }
    }

    /// An f32 signal's bits, or an i32 as it is.
    fn bits(&mut self, v: Value) -> Value {
        if self.b.func.dfg.value_type(v) == I32 {
            v
        } else if let Some(c) = self.constant(v) {
            self.b.ins().iconst(I32, i64::from(c.to_bits()))
        } else {
            self.b.ins().bitcast(I32, MemFlagsData::new(), v)
        }
    }

    /// Store a state field that a settling frame compares, from `old` to `new`.
    fn state_store(&mut self, s: Value, offset: i32, old: Value, new: Value) {
        self.b.ins().store(MemFlagsData::trusted(), new, s, offset);
        if let Some(changes) = &mut self.changes {
            changes.push((old, new));
        }
    }

    /// The fields `smoother_state` compares, as integers.
    fn smoother_state(&mut self, node: usize) -> [Value; 5] {
        let s = self.state_of(node);
        let t = MemFlagsData::trusted();
        [
            self.b.ins().load(I64, t, s, PHASE),
            self.b.ins().load(I32, t, s, ENV),
            self.b.ins().load(I32, t, s, LOW),
            self.b.ins().load(I32, t, s, BAND),
            self.b.ins().load(I64, t, s, INDEX),
        ]
    }

    /// Fold sum or product `op`'s inputs up to `upto` into its partial result
    /// in `partial`: its value, the bounds on its magnitude and on any
    /// partial result's, and how many inputs it holds.
    fn fold(&mut self, node: usize, op: &Op, upto: usize) {
        let (slots, sum) = match op {
            Op::Add2(a, b) => (vec![*a, *b], true),
            Op::Add3(a, b, c) => (vec![*a, *b, *c], true),
            Op::Mul2(a, b) => (vec![*a, *b], false),
            Op::Mul3(a, b, c) => (vec![*a, *b, *c], false),
            Op::Copy(a) => (vec![*a], true),
            Op::Sum { .. } => (inputs(op, self.args), true),
            _ => (inputs(op, self.args), false),
        };
        // Folding from -0 (or 1) leaves the first input exact. No partial
        // result can overflow while its bound is finite.
        let (mut x, mut bound, mut worst, from) = self.partial.remove(&node).unwrap_or_else(|| {
            let x = self.get(slots[0]);
            let bound = self.bound(x);
            (x, bound, bound, 1)
        });
        for slot in &slots[from..upto] {
            let y = self.get(*slot);
            let b = self.bound(y);
            (x, bound) = if sum {
                (self.b.ins().fadd(x, y), bound + b)
            } else {
                (self.b.ins().fmul(x, y), bound * b)
            };
            worst = worst.max(bound);
        }
        self.partial.insert(node, (x, bound, worst, upto));
    }

    fn inline(&mut self, node: usize, op: &Op) -> Value {
        match op {
            Op::Noise { probe } => {
                let field = if *probe {
                    offset_of!(Frame, probe_rng)
                } else {
                    offset_of!(Frame, rng)
                };
                let rng = self
                    .b
                    .ins()
                    .load(I64, MemFlagsData::trusted(), self.frame, field as i32);
                let mut r = self.b.ins().load(I32, MemFlagsData::trusted(), rng, 0);
                let t = self.b.ins().ishl_imm_u(r, 13);
                r = self.b.ins().bxor(r, t);
                let t = self.b.ins().ushr_imm_u(r, 17);
                r = self.b.ins().bxor(r, t);
                let t = self.b.ins().ishl_imm_u(r, 5);
                r = self.b.ins().bxor(r, t);
                self.b.ins().store(MemFlagsData::trusted(), r, rng, 0);
                let x = self.b.ins().fcvt_from_uint(F32, r);
                let scale = self.f32(u32::MAX as f32);
                let x = self.b.ins().fdiv(x, scale);
                let two = self.f32(2.);
                let x = self.b.ins().fmul(x, two);
                let one = self.f32(1.);
                let y = self.b.ins().fsub(x, one);
                // Between -1 and 1.
                self.bounds.insert(y, 1.);
                y
            }
            Op::NoteAge => {
                let field = |f: usize| f as i32;
                let age = self.b.ins().load(
                    I64,
                    MemFlagsData::trusted(),
                    self.frame,
                    field(offset_of!(Frame, age)),
                );
                let start = self.b.ins().load(
                    I64,
                    MemFlagsData::trusted(),
                    self.frame,
                    field(offset_of!(Frame, note_start)),
                );
                let frames = self.b.ins().isub(age, start);
                let frames = self.b.ins().fcvt_from_uint(F32, frames);
                let rate = self.f32(self.rate);
                let x = self.b.ins().fdiv(frames, rate);
                // At most 2^64 frames over a rate of at least 1.
                self.guard(x, 2f64.powi(64))
            }
            Op::Clamp { input, min, max } => {
                let x = self.get(*input);
                let a = self.get(*min);
                let b = self.get(*max);
                let bound = self
                    .bound(a)
                    .max(self.bound(b))
                    .max(if self.bound(x).is_finite() {
                        0.
                    } else {
                        f64::INFINITY
                    });
                let low = self.min(a, b);
                let high = self.max(a, b);
                let x = self.clamp(x, low, high);
                self.guard(x, bound)
            }
            Op::Abs(x) => {
                let x = self.get(*x);
                let bound = self.bound(x);
                let x = self.b.ins().fabs(x);
                self.guard(x, bound)
            }
            Op::Reciprocal(x) => {
                let x = self.get(*x);
                let bound = if self.bound(x).is_finite() {
                    1e20
                } else {
                    f64::INFINITY
                };
                let magnitude = self.b.ins().fabs(x);
                let tiny = self.f32(1e-20);
                let small = self.b.ins().fcmp(FloatCC::LessThan, magnitude, tiny);
                let one = self.f32(1.);
                let y = self.b.ins().fdiv(one, x);
                let zero = self.f32(0.);
                let x = self.b.ins().select(small, zero, y);
                self.guard(x, bound)
            }
            Op::Exp2(x) => {
                let x = self.get(*x);
                let bound = if self.bound(x).is_finite() {
                    2f64.powi(100)
                } else {
                    f64::INFINITY
                };
                let x = self.clamp_const(x, -100., 100.);
                let y = self.call(self.signatures.unary, exp2 as *const (), &[x]);
                self.guard(y, bound)
            }
            Op::Log2(x) => {
                let x = self.get(*x);
                let floor = self.f32(1e-20);
                let bound = if self.bound(x).is_finite() {
                    128.
                } else {
                    f64::INFINITY
                };
                let x = self.max(x, floor);
                let y = self.call(self.signatures.unary, log2 as *const (), &[x]);
                self.guard(y, bound)
            }
            Op::Sqrt(x) => {
                let x = self.get(*x);
                let bound = self.bound(x).sqrt();
                let zero = self.f32(0.);
                let x = self.max(x, zero);
                let x = self.b.ins().sqrt(x);
                self.guard(x, bound)
            }
            Op::Drive { input, amount } => {
                let x = self.get(*input);
                let amount = self.get(*amount);
                // tanh of a finite product, or of its overflow to infinity.
                let bound = if self.bound(x).max(self.bound(amount)).is_finite() {
                    1.
                } else {
                    f64::INFINITY
                };
                let amount = self.clamp_const(amount, 0., 100.);
                let x = self.b.ins().fmul(x, amount);
                let y = self.call(self.signatures.unary, tanh as *const (), &[x]);
                self.guard(y, bound)
            }
            Op::Copy(a) => {
                let x = self.get(*a);
                let bound = self.bound(x);
                self.guard(x, bound)
            }
            Op::Add2(..)
            | Op::Add3(..)
            | Op::Mul2(..)
            | Op::Mul3(..)
            | Op::Sum { .. }
            | Op::Product { .. } => {
                let len = inputs(op, self.args).len();
                self.fold(node, op, len);
                let (x, bound, worst, _) = self.partial.remove(&node).expect("folded");
                if worst <= FINITE {
                    self.guard(x, bound)
                } else {
                    self.finite(x)
                }
            }
            Op::Slew { input, rise, fall } => {
                let x = self.get(*input);
                let rise = self.get(*rise);
                let fall = self.get(*fall);
                let s = self.state_of(node);
                let index = self.b.ins().load(I64, MemFlagsData::trusted(), s, INDEX);
                let first = self.b.ins().icmp_imm_s(IntCC::Equal, index, 0);
                let stored = self.load_f32(s, ENV);
                let stored = self.source(stored);
                let held = self.b.ins().select(first, x, stored);
                let one = self.b.ins().iconst(I64, 1);
                self.state_store(s, INDEX, index, one);
                let rising = self.b.ins().fcmp(FloatCC::GreaterThan, x, held);
                // `x` plus at most the difference: k lies between 0 and 1.
                let bound = margin(self.bound(stored).max(self.bound(x)) + 2. * self.bound(x));
                if let Some([rise, fall]) = self.slew_poles(node, s, rise, fall) {
                    let k = self.b.ins().select(rising, rise, fall);
                    let d = self.b.ins().fsub(held, x);
                    let d = self.b.ins().fmul(d, k);
                    let y = self.b.ins().fadd(x, d);
                    self.state_store(s, ENV, stored, y);
                    self.account(y);
                    return self.guard(y, bound);
                }
                let (time, never_instant) = {
                    let time = self.b.ins().select(rising, rise, fall);
                    (self.clamp_const(time, 0., 60.), false)
                };
                let zero = self.f32(0.);
                let smooth = self.b.create_block();
                let done = self.b.create_block();
                self.b.append_block_param(done, F32);
                if never_instant {
                    self.b.ins().jump(smooth, &[]);
                } else {
                    let instant = self.b.ins().fcmp(FloatCC::Equal, time, zero);
                    self.b
                        .ins()
                        .brif(instant, done, &[ir::BlockArg::Value(zero)], smooth, &[]);
                }
                self.b.switch_to_block(smooth);
                let zero_bits = self.b.ins().iconst(I32, 0);
                self.memo(s, &[time, zero_bits], |g| {
                    let rate = g.f32(g.rate);
                    g.b.ins().call(g.signatures.refills.slew, &[s, time, rate]);
                });
                let k = self.memo_value(s, 0);
                self.b.ins().jump(done, &[ir::BlockArg::Value(k)]);
                self.b.switch_to_block(done);
                let k = self.b.block_params(done)[0];
                let d = self.b.ins().fsub(held, x);
                let d = self.b.ins().fmul(d, k);
                let y = self.b.ins().fadd(x, d);
                self.state_store(s, ENV, stored, y);
                self.account(y);
                self.guard(y, bound)
            }
            Op::Filter {
                input,
                cutoff,
                q,
                coefficients,
                mode,
            } => {
                let x = self.get(*input);
                // Bounds on the coefficients' magnitudes: g = tan(pi f / rate)
                // for f from 10 Hz to 0.45 rate, k = 1 / q for q from
                // `FILTER_Q[0]`, and a = 1 / (1 + g (g + k)).
                let [gb, kb, ab] = match coefficients {
                    Some((g, k, a)) => [g.abs(), k.abs(), a.abs()].map(f64::from),
                    None => [6.32, margin(1. / f64::from(FILTER_Q[0])), 1.0001],
                };
                // The coefficients, and 1 + g or g a (see `fill_f32`).
                let [g, k, a, more] = match coefficients {
                    Some((g, k, a)) => {
                        let more = if *mode >= 4 { g + 1. } else { g * a };
                        [*g, *k, *a, more].map(|c| self.f32(c))
                    }
                    None => {
                        if self.hoisted[node] == CONSTANT {
                            let cutoff = self.get(*cutoff);
                            let q = self.get(*q);
                            let s = self.state_of(node);
                            self.filter_coefficients(s, node, cutoff, q, *mode >= 4);
                        }
                        let at = |g: &mut Self, i: i32| {
                            let (base, at) = g.field(node, Field::Memo(i));
                            g.load_f32(base, at)
                        };
                        if *mode >= 4 {
                            // k and a are unused.
                            let zero = self.f32(0.);
                            [at(self, 0), zero, zero, at(self, 1)]
                        } else {
                            [0, 1, 2, 3].map(|i| at(self, i))
                        }
                    }
                };
                let (s, at_low) = self.field(node, Field::Low);
                let low = self.load_f32(s, at_low);
                let low = self.source(low);
                let (bx, bl) = (self.bound(x), self.bound(low));
                let (y, bound) = if *mode >= 4 {
                    let d = self.b.ins().fsub(x, low);
                    let d = self.b.ins().fmul(d, g);
                    let step = self.b.ins().fdiv(d, more);
                    let low = self.b.ins().fadd(low, step);
                    let next = self.b.ins().fadd(low, step);
                    self.state_store(s, at_low, low, next);
                    self.account(next);
                    // 1 + g is at least 1.
                    let bound = margin(bl + (bx + bl) * gb);
                    match mode {
                        4 => (low, bound),
                        5 => (self.b.ins().fsub(x, low), margin(bx + bound)),
                        _ => {
                            let two = self.f32(2.);
                            let twice = self.b.ins().fmul(low, two);
                            (self.b.ins().fsub(twice, x), margin(2. * bound + bx))
                        }
                    }
                } else {
                    let (sb, at_band) = self.field(node, Field::Band);
                    let band = self.load_f32(sb, at_band);
                    let band = self.source(band);
                    let band_bound = margin(ab * self.bound(band) + gb * ab * (bx + bl));
                    let low_bound = margin(bl + gb * band_bound);
                    let v3 = self.b.ins().fsub(x, low);
                    let ab = self.b.ins().fmul(a, band);
                    let gav = self.b.ins().fmul(more, v3);
                    let next_band = self.b.ins().fadd(ab, gav);
                    let gb = self.b.ins().fmul(g, next_band);
                    let next_low = self.b.ins().fadd(low, gb);
                    let two = self.f32(2.);
                    let t = self.b.ins().fmul(next_band, two);
                    let stored_band = self.b.ins().fsub(t, band);
                    self.state_store(sb, at_band, band, stored_band);
                    let t = self.b.ins().fmul(next_low, two);
                    let stored_low = self.b.ins().fsub(t, low);
                    self.state_store(s, at_low, low, stored_low);
                    self.account(stored_band);
                    self.account(stored_low);
                    let k_band = |g: &mut Self| g.b.ins().fmul(k, next_band);
                    match mode {
                        1 => {
                            let k_band = k_band(self);
                            let t = self.b.ins().fsub(x, k_band);
                            let y = self.b.ins().fsub(t, next_low);
                            (y, margin(bx + kb * band_bound + low_bound))
                        }
                        2 => (next_band, band_bound),
                        3 => {
                            let k_band = k_band(self);
                            (self.b.ins().fsub(x, k_band), margin(bx + kb * band_bound))
                        }
                        _ => (next_low, low_bound),
                    }
                };
                self.guard(y, bound)
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
                let s = self.state_of(node);
                let index = self.load_index(node);
                let line = self.b.ins().iadd_imm_s(self.delay, (*base * 4) as i64);
                let x = if *cubic {
                    self.cubic(index, line, *seconds, *fixed_cubic, *length)
                } else {
                    self.linear(index, line, *seconds, *length)
                };
                // A line's samples are sources; a read weighs four of them
                // by at most 1.13 in all, or two by 1.
                let weight = match fixed_cubic {
                    Some((_, weights)) => weights.iter().map(|w| f64::from(w.abs())).sum(),
                    None => 1.2,
                };
                let line_bound = self.limit.unwrap_or(f64::INFINITY);
                self.bounds.insert(x, margin(weight * line_bound));
                let returned = match damping {
                    Some(cutoff) => {
                        if self.hoisted[node] == CONSTANT {
                            let cutoff = self.get(*cutoff);
                            self.damping(s, cutoff);
                        }
                        let a = self.memo_value(s, 0);
                        let low = self.load_f32(s, LOW);
                        let low = self.source(low);
                        // The pole lies between 0 and 1.
                        let bound = margin(self.bound(low) + 2. * self.bound(x));
                        let d = self.b.ins().fsub(low, x);
                        let d = self.b.ins().fmul(d, a);
                        let low = self.b.ins().fadd(x, d);
                        self.store(low, s, LOW);
                        self.account(low);
                        self.bounds.insert(low, bound);
                        low
                    }
                    None => x,
                };
                let feedback = self.get(*feedback);
                let feedback = self.clamp_const(feedback, -max_feedback, *max_feedback);
                let env = self.b.ins().fmul(returned, feedback);
                let bound = margin(self.bound(returned) * f64::from(max_feedback.abs()));
                self.bounds.insert(env, bound);
                self.returns[node] = bound;
                let (base, at) = self.field(node, Field::Env);
                self.store(env, base, at);
                self.lines[node] = Some((index, env));
                let bound = self.bound(x);
                self.guard(x, bound)
            }
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
                let base = match hz {
                    Some(hz) => self.get(*hz),
                    None => self.frame_load(F32, offset_of!(Frame, freq)),
                };
                let ratio = self.get(*ratio);
                let cents = self.get(*detune);
                let fm = self.get(*fm);
                let width = self.get(*width);
                let offset = self.get(*phase);
                let s = self.state_of(node);
                let factor = match detune_factor {
                    Some(factor) => self.f32(*factor),
                    None => {
                        let zero_bits = self.b.ins().iconst(I32, 0);
                        self.memo(s, &[cents, zero_bits], |g| {
                            g.b.ins().call(g.signatures.refills.detune, &[s, cents]);
                        });
                        self.memo_value(s, 0)
                    }
                };
                // With a finite frequency and phase the wave is bounded; the
                // voice's own frequency may not be finite.
                let base_bound = match hz {
                    Some(_) => self.bound(base),
                    None => f64::INFINITY,
                };
                let factor_bound = match detune_factor {
                    Some(factor) => f64::from(factor.abs()),
                    None => 2f64.powf(self.bound(cents) / 1200.),
                };
                // The product of frequency and ratio must not overflow either.
                let product = base_bound * self.bound(ratio);
                let hz_bound = product.max(product * factor_bound) + self.bound(fm);
                let mut periodic = margin(hz_bound) <= FINITE && self.bound(offset) <= FINITE;
                let hz = self.b.ins().fmul(base, ratio);
                let hz = self.b.ins().fmul(hz, factor);
                let hz = self.b.ins().fadd(hz, fm);
                let rate = self.f32(self.rate);
                let dt = self.b.ins().fdiv(hz, rate);
                let dt = self.clamp_const(dt, 0., 0.45);
                let cycle = self.b.ins().load(F64, MemFlagsData::trusted(), s, PHASE);
                // An earlier frame's frequency may not have been finite, and
                // then neither is the phase.
                let cycle = self.source(cycle);
                periodic &= self.bound(cycle) <= FINITE;
                let p = self.b.ins().fdemote(F32, cycle);
                let p = self.b.ins().fadd(p, offset);
                // `rem_euclid(1.)`.
                let p = self.fmod1(p);
                let zero = self.f32(0.);
                let one = self.f32(1.);
                let negative = self.b.ins().fcmp(FloatCC::LessThan, p, zero);
                let wrapped = self.b.ins().fadd(p, one);
                let p = self.b.ins().select(negative, wrapped, p);
                let width = self.clamp_const(width, 0.02, 0.98);
                let minus = self.f32(-1.);
                let mut bound = [1., 2., 3.].get(usize::from(*wave)).copied();
                let y = match wave {
                    0 => {
                        let tau = self.f32(TAU);
                        let angle = self.b.ins().fmul(p, tau);
                        self.call(self.signatures.unary, sin as *const (), &[angle])
                    }
                    1 => {
                        let two = self.f32(2.);
                        let ramp = self.b.ins().fmul(p, two);
                        let ramp = self.b.ins().fsub(ramp, one);
                        let correction = self.blep(p, dt);
                        self.b.ins().fsub(ramp, correction)
                    }
                    2 => {
                        let high = self.b.ins().fcmp(FloatCC::LessThan, p, width);
                        let level = self.b.ins().select(high, one, minus);
                        let rise = self.blep(p, dt);
                        let level = self.b.ins().fadd(level, rise);
                        let t = self.b.ins().fadd(p, one);
                        let t = self.b.ins().fsub(t, width);
                        let t = self.fmod1(t);
                        let fall = self.blep(t, dt);
                        self.b.ins().fsub(level, fall)
                    }
                    _ => {
                        let half = self.f32(0.5);
                        let high = self.b.ins().fcmp(FloatCC::LessThan, p, half);
                        let level = self.b.ins().select(high, one, minus);
                        let rise = self.blep(p, dt);
                        let level = self.b.ins().fadd(level, rise);
                        let t = self.b.ins().fadd(p, half);
                        let t = self.fmod1(t);
                        let fall = self.blep(t, dt);
                        let square = self.b.ins().fsub(level, fall);
                        let four = self.f32(4.);
                        let gain = self.b.ins().fmul(dt, four);
                        let low = self.load_f32(s, LOW);
                        let low = self.source(low);
                        // The square is at most 3, the gain at most 1.8.
                        bound = Some(5.4 + self.bound(low));
                        let a = self.b.ins().fmul(gain, square);
                        let keep = self.b.ins().fsub(one, gain);
                        let b = self.b.ins().fmul(keep, low);
                        let low = self.b.ins().fadd(a, b);
                        self.store(low, s, LOW);
                        self.account(low);
                        low
                    }
                };
                let dt = self.b.ins().fpromote(F64, dt);
                let cycle = self.b.ins().fadd(cycle, dt);
                let cycle = self.fmod1(cycle);
                self.b.ins().store(MemFlagsData::trusted(), cycle, s, PHASE);
                self.account(cycle);
                let bound = bound.filter(|_| periodic).map_or(f64::INFINITY, margin);
                self.guard(y, bound)
            }
            Op::Envelope {
                one_shot,
                attack,
                decay,
                sustain,
                release,
            } => {
                let attack = self.get(*attack);
                let decay = self.get(*decay);
                let sustain = self.get(*sustain);
                let release = self.get(*release);
                let attack = self.clamp_const(attack, 0., 30.);
                let decay = self.clamp_const(decay, 0.0001, 60.);
                let sustain = self.clamp_const(sustain, 0., 1.);
                let s = self.state_of(node);
                let env = self.load_f32(s, ENV);
                let choked = self.frame_load(I8, offset_of!(Frame, choked));
                let released = self.frame_load(I8, offset_of!(Frame, released));
                let done = self.join();
                let choking = self.b.create_block();
                let playing = self.b.create_block();
                let holding = self.b.create_block();
                self.b.set_cold_block(choking);
                self.b.ins().brif(choked, choking, &[], playing, &[]);
                self.b.switch_to_block(choking);
                let factor = self.f32(self.choke_decay);
                let y = self.b.ins().fmul(env, factor);
                self.b.ins().jump(done, &[ir::BlockArg::Value(y)]);
                self.b.switch_to_block(playing);
                if *one_shot {
                    self.b.ins().jump(holding, &[]);
                } else {
                    let releasing = self.b.create_block();
                    self.b.ins().brif(released, releasing, &[], holding, &[]);
                    self.b.switch_to_block(releasing);
                    let release = self.clamp_const(release, 0.0001, 30.);
                    let zero_bits = self.b.ins().iconst(I32, 0);
                    self.memo(s, &[release, zero_bits], |g| {
                        let rate = g.f32(g.rate);
                        let refill = g.signatures.refills.release;
                        g.b.ins().call(refill, &[s, release, rate]);
                    });
                    let factor = self.memo_value(s, 0);
                    let y = self.b.ins().fmul(env, factor);
                    self.b.ins().jump(done, &[ir::BlockArg::Value(y)]);
                }
                self.b.switch_to_block(holding);
                let age = self.frame_load(I64, offset_of!(Frame, age));
                let t = self.float_of_u64(F32, age);
                let rate = self.f32(self.rate);
                let t = self.b.ins().fdiv(t, rate);
                let rising = self.b.ins().fcmp(FloatCC::LessThan, t, attack);
                let up = self.b.create_block();
                let down = self.b.create_block();
                self.b.ins().brif(rising, up, &[], down, &[]);
                self.b.switch_to_block(up);
                let y = self.b.ins().fdiv(t, attack);
                self.b.ins().jump(done, &[ir::BlockArg::Value(y)]);
                self.b.switch_to_block(down);
                let minus_five = self.f32(-5.);
                let since = self.b.ins().fsub(t, attack);
                let x = self.b.ins().fmul(since, minus_five);
                let x = self.b.ins().fdiv(x, decay);
                // Far below where `exp` underflows to +0, skip the call.
                let floor = self.f32(-110.);
                let gone = self.b.ins().fcmp(FloatCC::LessThan, x, floor);
                let decaying = self.b.create_block();
                let decayed = self.join();
                let zero = self.f32(0.);
                self.b
                    .ins()
                    .brif(gone, decayed, &[ir::BlockArg::Value(zero)], decaying, &[]);
                self.b.switch_to_block(decaying);
                let e = self.call(self.signatures.unary, exp as *const (), &[x]);
                self.b.ins().jump(decayed, &[ir::BlockArg::Value(e)]);
                self.b.switch_to_block(decayed);
                let e = self.b.block_params(decayed)[0];
                let one = self.f32(1.);
                let rest = self.b.ins().fsub(one, sustain);
                let y = self.b.ins().fmul(rest, e);
                let y = self.b.ins().fadd(sustain, y);
                self.b.ins().jump(done, &[ir::BlockArg::Value(y)]);
                self.b.switch_to_block(done);
                let y = self.b.block_params(done)[0];
                self.store(y, s, ENV);
                self.envelope_level(y);
                // A level rises to at most 1 and decays by factors below 1.
                self.guard(y, 1.)
            }
            Op::Mseg(envelope) => {
                self.mseg(node, envelope);
                let s = self.state_of(node);
                let at = (SEGMENT + offset_of!(envelope::State, value)) as i32;
                let x = self.load_f32(s, at);
                let at = (SEGMENT + offset_of!(envelope::State, done)) as i32;
                let finished = self.b.ins().uload8(I32, MemFlagsData::trusted(), s, at);
                let magnitude = self.b.ins().fabs(x);
                let floor = self.f32(1e-4);
                let above = self.b.ins().fcmp(FloatCC::GreaterThan, magnitude, floor);
                let level = self.b.ins().select(above, magnitude, floor);
                let zero = self.f32(0.);
                let level = self.b.ins().select(finished, zero, level);
                self.envelope_level(level);
                self.finite(x)
            }
            Op::Hold { input, hz } => {
                let x = self.get(*input);
                let hz = self.get(*hz);
                let s = self.state_of(node);
                let index = self.b.ins().load(I64, MemFlagsData::trusted(), s, INDEX);
                let cycle = self.b.ins().load(F64, MemFlagsData::trusted(), s, PHASE);
                let held = self.load_f32(s, ENV);
                // An earlier input, finite like every signal.
                let earlier = self.limit.unwrap_or(FINITE);
                self.bounds.insert(held, earlier);
                let first = self.b.ins().icmp_imm_s(IntCC::Equal, index, 0);
                let one = self.b.ins().f64const(1.);
                let due = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, cycle, one);
                let capture = self.b.ins().bor(first, due);
                let held = self.b.ins().select(capture, x, held);
                let whole = self.b.ins().trunc(cycle);
                let fraction = self.b.ins().fsub(cycle, whole);
                let cycle = self.b.ins().select(capture, fraction, cycle);
                self.store(held, s, ENV);
                self.account(held);
                let started = self.b.ins().iconst(I64, 1);
                self.b
                    .ins()
                    .store(MemFlagsData::trusted(), started, s, INDEX);
                let rate = self.f32(self.rate);
                let step = self.b.ins().fdiv(hz, rate);
                let step = self.clamp_const(step, 0., 1.);
                let step = self.b.ins().fpromote(F64, step);
                let cycle = self.b.ins().fadd(cycle, step);
                self.b.ins().store(MemFlagsData::trusted(), cycle, s, PHASE);
                let bound = self.bound(x).max(earlier);
                self.guard(held, bound)
            }
            Op::Resonator {
                input,
                frequency,
                decay,
                coefficients,
            } => {
                let x = self.get(*input);
                let frequency = self.get(*frequency);
                let decay = self.get(*decay);
                let s = self.state_of(node);
                let (a, b) = match coefficients {
                    Some((a, b)) => (self.b.ins().f64const(*a), self.b.ins().f64const(*b)),
                    None => {
                        self.memo(s, &[frequency, decay], |g| {
                            let rate = g.f32(g.rate);
                            let refill = g.signatures.refills.resonator;
                            g.b.ins().call(refill, &[s, frequency, decay, rate]);
                        });
                        let t = MemFlagsData::trusted();
                        (
                            self.b.ins().load(F64, t, s, MEMO),
                            self.b.ins().load(F64, t, s, MEMO + 8),
                        )
                    }
                };
                let t = MemFlagsData::trusted();
                let real = self.b.ins().load(F64, t, s, PHASE);
                let imaginary = self.b.ins().load(F64, t, s, IMAGINARY);
                // Both coefficients are at most 1 in magnitude.
                let bound = margin(2. * self.limit.unwrap_or(f64::INFINITY) + self.bound(x));
                let ar = self.b.ins().fmul(a, real);
                let bi = self.b.ins().fmul(b, imaginary);
                let next = self.b.ins().fsub(ar, bi);
                let x = self.b.ins().fpromote(F64, x);
                let next = self.b.ins().fadd(next, x);
                let br = self.b.ins().fmul(b, real);
                let ai = self.b.ins().fmul(a, imaginary);
                let imaginary = self.b.ins().fadd(br, ai);
                self.b.ins().store(t, imaginary, s, IMAGINARY);
                self.b.ins().store(t, next, s, PHASE);
                self.account(imaginary);
                self.account(next);
                let y = self.b.ins().fdemote(F32, next);
                self.guard(y, bound)
            }
            Op::RawShape { input, shape } => self.raw_shapes(*input, &[shape], false)[0],
            _ => unreachable!("`inline` lists the operations emitted here"),
        }
    }

    /// The rising and falling poles of slew `node` (state `s`) of times
    /// `rise` and `fall`, where they need no memo test: constants, or
    /// refilled where the times change (see `refresh`).
    fn slew_poles(
        &mut self,
        node: usize,
        s: Value,
        rise: Value,
        fall: Value,
    ) -> Option<[Value; 2]> {
        if self.hoisted[node] != CONSTANT {
            let t = MemFlagsData::trusted();
            return Some([MEMO, MEMO + 4].map(|at| self.b.ins().load(F32, t, s, at)));
        }
        // Constant times are clamped here, and never zero, usually; then so
        // is the pole, and the memo is only a cache.
        let [Some(rise), Some(fall)] = [rise, fall].map(|t| self.constant(t)) else {
            return None;
        };
        let rate = self.rate;
        let pole = |t: f32| {
            let t = if t < 0. { 0. } else { t };
            let t = if t > 60. { 60. } else { t };
            if t == 0. {
                0.
            } else {
                State::default().slew_memo(t, rate)
            }
        };
        Some([self.f32(pole(rise)), self.f32(pole(fall))])
    }

    /// A frame of slew step `k` (input `input`, state `s`) in its tail (see
    /// `cluster`), as `inline` computes it: its output and state, and
    /// whether they changed. The last frame left its output in its state,
    /// finite, and its index past the first frame.
    fn tail_step(&mut self, p: &VoicePatch, k: usize, input: Slot, s: Value) -> Value {
        let node = p.steps[k].node as usize;
        let Op::Slew { rise, fall, .. } = &p.steps[k].op else {
            unreachable!("a tail is a slew's");
        };
        self.loaded.fill(None);
        let x = self.get(input);
        let [rise, fall] = [*rise, *fall].map(|t| self.get(t));
        let held = self.load_f32(s, ENV);
        let rising = self.b.ins().fcmp(FloatCC::GreaterThan, x, held);
        let [rise, fall] = self
            .slew_poles(node, s, rise, fall)
            .expect("poles without a memo test");
        let k = self.b.ins().select(rising, rise, fall);
        let d = self.b.ins().fsub(held, x);
        let d = self.b.ins().fmul(d, k);
        let y = self.b.ins().fadd(x, d);
        self.store(y, s, ENV);
        self.store(y, self.values, 4 * node as i32);
        let [held, y] = [held, y].map(|v| self.bits(v));
        self.b.ins().icmp(IntCC::NotEqual, held, y)
    }

    /// Raw shapes of one input over the same domain (`Shape::value`, in
    /// f64), which share the position in their tables. With `ends`, as in a
    /// cluster, whose input moves slowly, an input past an end of the
    /// domain gives each shape its value there without a lookup.
    fn raw_shapes(&mut self, input: Slot, shapes: &[&shape::Shape], ends: bool) -> Vec<Value> {
        let (table, low, step) = shapes[0].table();
        let len = table.len();
        let last = (len - 1) as f64;
        let x = self.get(input);
        let done = ends.then(|| {
            let done = self.b.create_block();
            for _ in shapes {
                self.b.append_block_param(done, F32);
            }
            done
        });
        if let Some(done) = done {
            let ((below, _), (above, _)) = ends_of(table, low, step);
            let [first, last] = [0, 1].map(|end| {
                (shapes.iter())
                    .map(|shape| {
                        let (table, low, step) = shape.table();
                        let (first, last) = ends_of(table, low, step);
                        ir::BlockArg::Value(self.f32([first, last][end].1))
                    })
                    .collect::<Vec<_>>()
            });
            let [below, above] = [below, above].map(|x| self.f32(x));
            let under = self.b.ins().fcmp(FloatCC::LessThanOrEqual, x, below);
            let over = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, x, above);
            let inside = self.b.create_block();
            let lookup = self.b.create_block();
            self.b.ins().brif(under, done, &first, inside, &[]);
            self.b.switch_to_block(inside);
            self.b.ins().brif(over, done, &last, lookup, &[]);
            self.b.switch_to_block(lookup);
        }
        let x = self.b.ins().fpromote(F64, x);
        let low = self.b.ins().f64const(low);
        let x = self.b.ins().fsub(x, low);
        // Dividing by a power of two (a domain of one, say) is multiplying
        // by its reciprocal, exactly, short of the extremes of f64's range.
        let power = step.to_bits() & ((1 << 52) - 1) == 0;
        let p = if power && step.is_normal() && (1. / step).is_normal() {
            let scale = self.b.ins().f64const(1. / step);
            self.b.ins().fmul(x, scale)
        } else {
            let step = self.b.ins().f64const(step);
            self.b.ins().fdiv(x, step)
        };
        let zero = self.b.ins().f64const(0.);
        let below = self.b.ins().fcmp(FloatCC::LessThan, p, zero);
        let p = self.b.ins().select(below, zero, p);
        let high = self.b.ins().f64const(last);
        let above = self.b.ins().fcmp(FloatCC::GreaterThan, p, high);
        let p = self.b.ins().select(above, high, p);
        // `p` lies in 0..=last, where signed conversions are exact.
        let i = self.b.ins().fcvt_to_sint_sat(I64, p);
        let top = self.b.ins().iconst(I64, len as i64 - 2);
        let i = self.b.ins().umin(i, top);
        let whole = self.b.ins().fcvt_from_sint(F64, i);
        let f = self.b.ins().fsub(p, whole);
        // Shapes that share the lookup read one table of their entries side
        // by side, so the entries a frame reads share cache lines.
        let width = shapes.len();
        let base = if width == 1 {
            shapes[0].table().0.as_ptr()
        } else {
            let key: Vec<usize> = (shapes.iter())
                .map(|shape| shape.table().0.as_ptr() as usize)
                .collect();
            let mut tables = self.tables.borrow_mut();
            match tables.iter().find(|(k, _)| *k == key) {
                Some((_, rows)) => rows.as_ptr(),
                None => {
                    let mut rows = vec![0.; len * width];
                    for (k, shape) in shapes.iter().enumerate() {
                        for (i, &v) in shape.table().0.iter().enumerate() {
                            rows[i * width + k] = v;
                        }
                    }
                    let rows = rows.into_boxed_slice();
                    let base = rows.as_ptr();
                    tables.push((key, rows));
                    base
                }
            }
        };
        let offset = self.b.ins().imul_imm_u(i, 8 * width as i64);
        let base = self.b.ins().iconst(I64, base as i64);
        let at = self.b.ins().iadd(base, offset);
        let row = 8 * width as i32;
        let mut ys: Vec<Value> = (0..width)
            .map(|k| {
                let k = 8 * k as i32;
                let a = self.b.ins().load(F64, MemFlagsData::trusted(), at, k);
                let b = self.b.ins().load(F64, MemFlagsData::trusted(), at, row + k);
                let d = self.b.ins().fsub(b, a);
                let d = self.b.ins().fmul(f, d);
                let y = self.b.ins().fadd(a, d);
                self.b.ins().fdemote(F32, y)
            })
            .collect();
        if let Some(done) = done {
            let args: Vec<_> = ys.iter().map(|&y| ir::BlockArg::Value(y)).collect();
            self.b.ins().jump(done, &args);
            self.b.switch_to_block(done);
            ys = self.b.block_params(done).to_vec();
        }
        for (&y, shape) in ys.iter().zip(shapes) {
            // Between two entries, give or take rounding.
            let table = shape.table().0;
            let bound = table.iter().fold(0f64, |m, v| m.max(v.abs())) * (1. + 1e-9);
            self.guard(y, bound);
        }
        ys
    }

    /// A segment envelope's frame (`Envelope::step`): inline while it moves
    /// along a segment, holds its sustain or has finished, otherwise through
    /// `mseg_step`.
    fn mseg(&mut self, node: usize, envelope: &envelope::Envelope) {
        use envelope::{Segment, State as SegmentState};
        let t = MemFlagsData::trusted();
        let s = self.state_of(node);
        let field = |f: usize| (SEGMENT + f) as i32;
        let choked = self.frame_load(I8, offset_of!(Frame, choked));
        let released = self.frame_load(I8, offset_of!(Frame, released));
        let releasing = self
            .b
            .ins()
            .uload8(I32, t, s, field(offset_of!(SegmentState, releasing)));
        let finished = self
            .b
            .ins()
            .uload8(I32, t, s, field(offset_of!(SegmentState, done)));
        let slow = self.b.create_block();
        let done = self.b.create_block();
        self.b.set_cold_block(slow);
        // Choked, or released before its release began: the full step.
        let mut special = self.b.ins().icmp_imm_s(IntCC::NotEqual, choked, 0);
        if !envelope.one_shot {
            let released = self.b.ins().icmp_imm_s(IntCC::NotEqual, released, 0);
            let attacking = self.b.ins().icmp_imm_s(IntCC::Equal, releasing, 0);
            let starts = self.b.ins().band(released, attacking);
            special = self.b.ins().bor(special, starts);
        }
        let ordinary = self.b.create_block();
        self.b.ins().brif(special, slow, &[], ordinary, &[]);
        self.b.switch_to_block(ordinary);
        let moving = self.b.create_block();
        self.b.ins().brif(finished, done, &[], moving, &[]);
        self.b.switch_to_block(moving);
        let index = self
            .b
            .ins()
            .load(I64, t, s, field(offset_of!(SegmentState, index)));
        let elapsed = self
            .b
            .ins()
            .load(I64, t, s, field(offset_of!(SegmentState, elapsed)));
        let table = |g: &mut Self, segments: &[Segment]| {
            (
                g.b.ins().iconst(I64, segments.as_ptr() as i64),
                g.b.ins().iconst(I64, segments.len() as i64),
            )
        };
        let (attack, attack_len) = table(self, &envelope.attack);
        let (release, release_len) = table(self, &envelope.release);
        let segments = self.b.ins().select(releasing, release, attack);
        let len = self.b.ins().select(releasing, release_len, attack_len);
        let past = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
        let within = self.b.create_block();
        self.b.ins().brif(past, slow, &[], within, &[]);
        self.b.switch_to_block(within);
        let offset = self.b.ins().imul_imm_s(index, size_of::<Segment>() as i64);
        let at = self.b.ins().iadd(segments, offset);
        let frames = self
            .b
            .ins()
            .load(I64, t, at, offset_of!(Segment, frames) as i32);
        let target = self.load_f32(at, offset_of!(Segment, target) as i32);
        let curve = self
            .b
            .ins()
            .uload8(I32, t, at, offset_of!(Segment, shape) as i32);
        let over = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, elapsed, frames);
        let ending = self.b.create_block();
        let stepping = self.b.create_block();
        self.b.ins().brif(over, ending, &[], stepping, &[]);
        self.b.switch_to_block(ending);
        match envelope.sustain {
            // Holding the sustain: the segment's target.
            Some(sustain) if !envelope.one_shot => {
                let attacking = self.b.ins().icmp_imm_s(IntCC::Equal, releasing, 0);
                let at_sustain = self.b.ins().icmp_imm_s(IntCC::Equal, index, sustain as i64);
                let holds = self.b.ins().band(attacking, at_sustain);
                let hold = self.b.create_block();
                self.b.ins().brif(holds, hold, &[], slow, &[]);
                self.b.switch_to_block(hold);
                self.store(target, s, field(offset_of!(SegmentState, value)));
                self.b.ins().jump(done, &[]);
            }
            _ => {
                self.b.ins().jump(slow, &[]);
            }
        }
        self.b.switch_to_block(stepping);
        let elapsed = self.b.ins().iadd_imm_s(elapsed, 1);
        self.b
            .ins()
            .store(t, elapsed, s, field(offset_of!(SegmentState, elapsed)));
        let p = self.float_of_u64(F32, elapsed);
        let span = self.float_of_u64(F32, frames);
        let p = self.b.ins().fdiv(p, span);
        let exponential = self.b.ins().icmp_imm_s(IntCC::Equal, curve, 2);
        let curved = self.join();
        let shaped = self.b.create_block();
        let rising = self.b.create_block();
        self.b.ins().brif(exponential, rising, &[], shaped, &[]);
        // Exponential: the interpreter's own function, through `exp`.
        self.b.switch_to_block(rising);
        let q = self.call(self.signatures.unary, mseg_exponential as *const (), &[p]);
        self.b.ins().jump(curved, &[ir::BlockArg::Value(q)]);
        self.b.switch_to_block(shaped);
        // Smooth: p * p * (3 - 2 * p).
        let square = self.b.ins().fmul(p, p);
        let two = self.f32(2.);
        let three = self.f32(3.);
        let twice = self.b.ins().fmul(p, two);
        let rest = self.b.ins().fsub(three, twice);
        let smooth = self.b.ins().fmul(square, rest);
        let smoothed = self.b.ins().icmp_imm_s(IntCC::Equal, curve, 1);
        let p = self.b.ins().select(smoothed, smooth, p);
        self.b.ins().jump(curved, &[ir::BlockArg::Value(p)]);
        self.b.switch_to_block(curved);
        let p = self.b.block_params(curved)[0];
        let start = self.load_f32(s, field(offset_of!(SegmentState, start)));
        let span = self.b.ins().fsub(target, start);
        let span = self.b.ins().fmul(span, p);
        let value = self.b.ins().fadd(start, span);
        self.store(value, s, field(offset_of!(SegmentState, value)));
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(slow);
        let envelope = self
            .b
            .ins()
            .iconst(I64, envelope as *const envelope::Envelope as i64);
        let rate = self.f32(self.rate);
        let step = self.signatures.refills.mseg;
        self.b
            .ins()
            .call(step, &[envelope, s, released, choked, rate]);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// `write_delays` for one line.
    /// `fresh`: right after the delay's own step, whose values are at hand;
    /// later, reading them back is cheaper than keeping them.
    fn write_delay(&mut self, node: usize, input: Slot, base: usize, length: usize, fresh: bool) {
        let (index, env) = match self.lines[node].filter(|_| fresh) {
            Some(line) => line,
            None => {
                let index = self.load_index(node);
                let (base, at) = self.field(node, Field::Env);
                let env = self.load_f32(base, at);
                // As the delay's step left it this frame.
                self.bounds.insert(env, self.returns[node]);
                (index, env)
            }
        };
        let x = self.get(input);
        let bound = margin(self.bound(x) + self.bound(env));
        let x = self.b.ins().fadd(x, env);
        let x = self.guard(x, bound);
        let line = self.b.ins().iadd_imm_s(self.delay, (base * 4) as i64);
        let offset = self.b.ins().ishl_imm_u(index, 2);
        let address = self.b.ins().iadd(line, offset);
        // Every sample again in a mirrored line's copy; else the first
        // samples in the line's tail, and this one again.
        let (tail, offset) = if line_tail(length) == length {
            (address, 4 * length as i32)
        } else {
            let head = (self.b.ins()).icmp_imm_s(IntCC::UnsignedLessThan, index, LINE_TAIL as i64);
            let tail = self.b.ins().iadd_imm_s(address, 4 * length as i64);
            (self.b.ins().select(head, tail, address), 0)
        };
        match self.deferred.take() {
            Some(x) => self.pending.push((Target::Line(address, tail, offset), x)),
            None => {
                self.store(x, address, 0);
                self.store(x, tail, offset);
                self.account(x);
            }
        }
        let next = self.b.ins().iadd_imm_s(index, 1);
        let end = self.b.ins().icmp_imm_s(IntCC::Equal, next, length as i64);
        let zero = self.b.ins().iconst(I64, 0);
        let next = self.b.ins().select(end, zero, next);
        self.store_index(node, next);
    }
}
