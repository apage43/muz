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
//! cluster reports whether its output moved. The order of independent steps
//! changes no result: none of them draws noise, and the loudest envelope is
//! a maximum.
//!
//! Within the audio-rate function signals stay in registers and constants
//! become immediates. Only values read elsewhere (the voice's outputs, a
//! probe, the lifetime node and inputs of called steps) are stored to the
//! value table. A delay line is written as soon as its node has run and its
//! input is known; nothing else reads the line. A guard against non-finite
//! values is left out where bounds on the inputs show it would change
//! nothing. Memos of audio-rate filters and damped delays whose inputs change
//! only at earlier tiers are refilled at the end of the last tier those
//! inputs come from, which runs whenever they can change, instead of being
//! tested every frame; a slew with constant times uses its two poles as
//! constants. Memos are caches, so skipping one changes no output.
//!
//! Every operation but parameters, note data, antialiased shapes and sample
//! readers is emitted inline: oscillators, envelopes, holds, resonators,
//! delays, filters, smoothers and arithmetic. Coefficient refills and the
//! rarer turns of a segment envelope call the interpreter's own code; the
//! rest call `step`, which evaluates one step with the interpreter's `eval`.
//! libm functions are called through Rust, so they are the interpreter's own,
//! except where the result is exact without them: `x % 1.` is `x` less its
//! whole part (with `x`'s sign on a zero), and `exp` of a number below -110
//! is +0, so an envelope's settled decay calls nothing.
//!
//! Rust's `f32::min`, `max` and `clamp` are written as comparisons and
//! selects with the same operand order, which also fixes the sign of a zero
//! result; Cranelift's `fmin`/`fmax` order zeros differently and are not used.
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
        }
        frame.env_level
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
    x.tan()
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
    let shape = Shape {
        p,
        nodes,
        constants,
        hoisted,
        quiet,
        clusters,
        cluster,
        refills_after,
        refills,
        tiers: tier,
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
}

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
const PHASE: i32 = offset_of!(State, phase) as i32;
const ENV: i32 = offset_of!(State, env) as i32;
const LOW: i32 = offset_of!(State, low) as i32;
const BAND: i32 = offset_of!(State, band) as i32;
const INDEX: i32 = offset_of!(State, index) as i32;
const IMAGINARY: i32 = offset_of!(State, imaginary) as i32;
const SEGMENT: usize = offset_of!(State, segment);
const RESTING: i32 = offset_of!(State, resting) as i32;
const VALID: i32 = offset_of!(State, memo.valid) as i32;
const KEY: i32 = offset_of!(State, memo.key) as i32;
const MEMO: i32 = offset_of!(State, memo.value) as i32;
const _: () = assert!(size_of::<usize>() == 8);

struct Gen<'a, 'b> {
    b: FunctionBuilder<'b>,
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
    /// Each inline delay's state address, read index and feedback signal.
    lines: Vec<Option<(Value, Value, Value)>>,
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
        v
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
        let (first, weights) = match fixed_cubic {
            Some((whole, weights)) => (
                self.b.ins().iadd_imm_s(index, length - (whole as i64 - 1)),
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
                let whole = self.b.ins().fcvt_to_uint_sat(I64, whole);
                let back = self.b.ins().iadd_imm_s(whole, -1);
                let ahead = self.b.ins().iadd_imm_s(index, length);
                (self.b.ins().isub(ahead, back), [w0, w1, w2, w3])
            }
        };
        let wraps = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, first, length);
        let wrapped = self.b.ins().iadd_imm_s(first, -length);
        let mut at = self.b.ins().select(wraps, wrapped, first);
        let mut x = self.f32(0.);
        for (i, weight) in weights.into_iter().enumerate() {
            let offset = self.b.ins().ishl_imm_u(at, 2);
            let address = self.b.ins().iadd(line, offset);
            let sample = self.load_f32(address, 0);
            let term = self.b.ins().fmul(sample, weight);
            x = self.b.ins().fadd(x, term);
            if i < 3 {
                let zero = self.b.ins().icmp_imm_s(IntCC::Equal, at, 0);
                let last = self.b.ins().iconst(I64, length - 1);
                let previous = self.b.ins().iadd_imm_s(at, -1);
                at = self.b.ins().select(zero, last, previous);
            }
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
        // `x` itself.
        let zero = self.b.ins().f64const(0.);
        let negative = self.b.ins().fcmp(FloatCC::LessThan, x, zero);
        let span = self.b.ins().f64const(length as f64);
        let wrapped = self.b.ins().fadd(x, span);
        let position = self.b.ins().select(negative, wrapped, x);
        let i = self.b.ins().fcvt_to_sint_sat(I64, position);
        let whole = self.b.ins().fcvt_from_sint(F64, i);
        let frac = self.b.ins().fsub(position, whole);
        let frac = self.b.ins().fdemote(F32, frac);
        let offset = self.b.ins().ishl_imm_u(i, 2);
        let address = self.b.ins().iadd(line, offset);
        let a = self.load_f32(address, 0);
        let next = self.b.ins().iadd_imm_s(i, 1);
        let end = self.b.ins().icmp_imm_s(IntCC::Equal, next, length as i64);
        let start = self.b.ins().iconst(I64, 0);
        let next = self.b.ins().select(end, start, next);
        let offset = self.b.ins().ishl_imm_u(next, 2);
        let address = self.b.ins().iadd(line, offset);
        let b = self.load_f32(address, 0);
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
    fn from_u64(&mut self, ty: ir::Type, x: Value) -> Value {
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
        for c in 0..self.clusters.len() {
            if self.clusters[c][0] < p.settling {
                continue;
            }
            crate::host::check_cancelled()?;
            self.cluster(p, c, &watched, fresh, Some([control, settled]), None);
        }
        // Later readers load sources from the value table.
        for s in &p.steps[p.settling..] {
            if self.quiet[s.node as usize] == Quiet::Source {
                self.computed[s.node as usize] = None;
            }
        }
        // A line is read only by its own delay node, so it can be written as
        // soon as that node has run and its input is known, rather than after
        // the frame: the same writes, but the values they need live briefly.
        let mut position = vec![None; self.nodes];
        for (k, s) in p.steps.iter().enumerate().skip(p.settling) {
            position[s.node as usize] = Some(k);
        }
        let mut writes = vec![Vec::new(); p.steps.len()];
        for &(node, input, base, length) in &p.delays {
            let after = position[node].max(position.get(input as usize).copied().flatten());
            let after = after.expect("delays are audio-rate");
            writes[after].push((node, input, base, length));
        }
        for (k, s) in p.steps.iter().enumerate().skip(p.settling) {
            crate::host::check_cancelled()?;
            // A load from the value table is as cheap as reloading a spill.
            self.loaded.fill(None);
            let node = s.node as usize;
            if self.quiet[node] == Quiet::No {
                let x = self.step(k, s);
                self.computed[node] = Some(x);
                if store[node] {
                    self.spill(node as Slot);
                }
            }
            for &(line, input, base, length) in &writes[k] {
                let fresh = line == node && self.quiet[node] == Quiet::No;
                self.write_delay(line, input, base, length, fresh);
            }
        }
        Ok(())
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
                let x = self.step(j, s);
                self.store(x, self.values, 4 * node as i32);
                self.stored[node] = true;
                self.computed[node] = Some(x);
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
        for c in 0..self.clusters.len() {
            let first = self.clusters[c][0];
            if first < p.control || first >= p.settling {
                continue;
            }
            crate::host::check_cancelled()?;
            let sources = self.clusters[c]
                .iter()
                .fold(0, |sources, &k| sources | p.sources[k]);
            let follows = self.b.ins().band_imm_s(changed, sources as i64);
            let woken = self.b.ins().icmp_imm_s(IntCC::NotEqual, follows, 0);
            self.cluster(p, c, &watched, woken, None, Some(differs));
        }
        self.refill_hoisted(p, SETTLING);
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
        for s in &p.steps[p.settling..] {
            if self.hoisted[s.node as usize] == tier {
                self.loaded.fill(None);
                self.refresh(s);
            }
        }
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
        earlier: Option<[Value; 2]>,
        differs: Option<ir::StackSlot>,
    ) {
        let steps = &self.clusters[c];
        let root = p.steps[steps[0]].node as usize;
        let resting = steps
            .iter()
            .any(|&k| self.quiet[p.steps[k].node as usize] == Quiet::Resting);
        let mut run = wake;
        if resting {
            let state = self.state_of(root);
            let rests = self
                .b
                .ins()
                .uload8(I32, MemFlagsData::trusted(), state, RESTING);
            let restless = self.b.ins().icmp_imm_s(IntCC::Equal, rests, 0);
            run = self.b.ins().bor(run, restless);
        }
        let mut seen = Vec::new();
        let mut reads = [false; 2];
        for &k in steps {
            for input in inputs(&p.steps[k].op, self.args) {
                let i = input as usize;
                if i >= self.nodes || seen.contains(&i) || self.cluster[i] == c {
                    continue;
                }
                seen.push(i);
                match (self.moved[i], self.tiers[i], earlier) {
                    (Some(moved), ..) => run = self.b.ins().bor(run, moved),
                    (None, CONSTANT, _) => {}
                    (None, CONTROL, Some(_)) => reads[0] = true,
                    (None, SETTLING, Some(_)) => reads[1] = true,
                    // Followed through the sources of the settling steps.
                    (None, CONTROL, None) => {}
                    _ => unreachable!("a cluster reads earlier clusters and sources"),
                }
            }
        }
        if let Some(earlier) = earlier {
            for (read, moved) in reads.into_iter().zip(earlier) {
                if read {
                    run = self.b.ins().bor(run, moved);
                }
            }
        }
        let go = self.b.create_block();
        let next = self.b.create_block();
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
        self.b.ins().brif(run, go, &[], next, &rest);
        self.b.switch_to_block(go);
        let mut moved = Vec::new();
        let mut differ = still;
        for &k in steps {
            let s = &p.steps[k];
            let node = s.node as usize;
            self.loaded.fill(None);
            let smoother = self.quiet[node] == Quiet::Resting;
            let old = (smoother || watched[node] || differs.is_some()).then(|| {
                self.b
                    .ins()
                    .load(I32, MemFlagsData::trusted(), self.values, 4 * node as i32)
            });
            // Inline operations report the state they change; a called one
            // is compared whole.
            self.changes = smoother.then(Vec::new);
            let before = (smoother && !inline(&s.op)).then(|| self.smoother_state(node));
            let x = self.step(k, s);
            self.computed[node] = Some(x);
            self.store(x, self.values, 4 * node as i32);
            let Some(old) = old else { continue };
            let bits = self.bits(x);
            let changed = self.b.ins().icmp(IntCC::NotEqual, old, bits);
            if watched[node] {
                moved.push(ir::BlockArg::Value(changed));
            }
            if smoother || differs.is_some() {
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
        if resting {
            let state = self.state_of(root);
            let rests = self.b.ins().icmp_imm_s(IntCC::Equal, differ, 0);
            self.b
                .ins()
                .store(MemFlagsData::trusted(), rests, state, RESTING);
        }
        if let Some(differs) = differs {
            let all = self.b.ins().stack_load(I64, I8, differs, 0);
            let all = self.b.ins().bor(all, differ);
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
        }
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
                self.filter_coefficients(state, cutoff, q, *mode >= 4);
            }
            Op::Delay {
                damping: Some(cutoff),
                ..
            } => {
                let cutoff = self.get(*cutoff);
                self.damping(state, cutoff);
            }
            _ => unreachable!("only filters and damped delays are hoisted"),
        }
    }

    /// `State::filter_memo`: a filter's coefficients for `cutoff` and `q`.
    /// A one-pole filter reads only the first, the gain, and refills only it.
    fn filter_coefficients(&mut self, state: Value, cutoff: Value, q: Value, one_pole: bool) {
        self.memo(state, &[cutoff, q], |g| {
            // `filter_coefficients`.
            let c = g.clamp_const(cutoff, 10., g.rate * 0.45);
            let pi = g.f32(std::f32::consts::PI);
            let t = g.b.ins().fmul(pi, c);
            let rate = g.f32(g.rate);
            let t = g.b.ins().fdiv(t, rate);
            let gain = g.call(g.signatures.unary, tan as *const (), &[t]);
            if one_pole {
                let zero = g.f32(0.);
                g.fill(state, &[cutoff, q], [gain, zero, zero]);
                return;
            }
            let qc = g.clamp_const(q, 0.2, 20.);
            let one = g.f32(1.);
            let k = g.b.ins().fdiv(one, qc);
            let d = g.b.ins().fadd(gain, k);
            let d = g.b.ins().fmul(gain, d);
            let d = g.b.ins().fadd(one, d);
            let a = g.b.ins().fdiv(one, d);
            g.fill(state, &[cutoff, q], [gain, k, a]);
        });
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
                self.b.ins().fsub(x, one)
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
                let (slots, sum) = match op {
                    Op::Add2(a, b) => (vec![*a, *b], true),
                    Op::Add3(a, b, c) => (vec![*a, *b, *c], true),
                    Op::Mul2(a, b) => (vec![*a, *b], false),
                    Op::Mul3(a, b, c) => (vec![*a, *b, *c], false),
                    Op::Sum { .. } => (inputs(op, self.args), true),
                    _ => (inputs(op, self.args), false),
                };
                // Folding from -0 (or 1) leaves the first input exact. No
                // partial result can overflow while its bound is finite.
                let mut x = self.get(slots[0]);
                let mut bound = self.bound(x);
                let mut worst = bound;
                for slot in &slots[1..] {
                    let y = self.get(*slot);
                    let b = self.bound(y);
                    (x, bound) = if sum {
                        (self.b.ins().fadd(x, y), bound + b)
                    } else {
                        (self.b.ins().fmul(x, y), bound * b)
                    };
                    worst = worst.max(bound);
                }
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
                let held = self.b.ins().select(first, x, stored);
                let one = self.b.ins().iconst(I64, 1);
                self.state_store(s, INDEX, index, one);
                let rising = self.b.ins().fcmp(FloatCC::GreaterThan, x, held);
                // Constant times are clamped here, and never zero, usually.
                let times = [rise, fall].map(|t| self.constant(t));
                let (time, never_instant) = if let [Some(rise), Some(fall)] = times {
                    // Then so is the pole: the memo is only a cache.
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
                    let (rise, fall) = (self.f32(pole(rise)), self.f32(pole(fall)));
                    let k = self.b.ins().select(rising, rise, fall);
                    let d = self.b.ins().fsub(held, x);
                    let d = self.b.ins().fmul(d, k);
                    let y = self.b.ins().fadd(x, d);
                    self.state_store(s, ENV, stored, y);
                    return self.finite(y);
                } else {
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
                self.finite(y)
            }
            Op::Filter {
                input,
                cutoff,
                q,
                coefficients,
                mode,
            } => {
                let x = self.get(*input);
                let s = self.state_of(node);
                let (g, k, a) = match coefficients {
                    Some((g, k, a)) => (self.f32(*g), self.f32(*k), self.f32(*a)),
                    None => {
                        if self.hoisted[node] == CONSTANT {
                            let cutoff = self.get(*cutoff);
                            let q = self.get(*q);
                            self.filter_coefficients(s, cutoff, q, *mode >= 4);
                        }
                        if *mode >= 4 {
                            // Unused.
                            let zero = self.f32(0.);
                            (self.memo_value(s, 0), zero, zero)
                        } else {
                            (
                                self.memo_value(s, 0),
                                self.memo_value(s, 1),
                                self.memo_value(s, 2),
                            )
                        }
                    }
                };
                let low = self.load_f32(s, LOW);
                let y = if *mode >= 4 {
                    let d = self.b.ins().fsub(x, low);
                    let d = self.b.ins().fmul(d, g);
                    let one = self.f32(1.);
                    let gain = self.b.ins().fadd(one, g);
                    let step = self.b.ins().fdiv(d, gain);
                    let low = self.b.ins().fadd(low, step);
                    let next = self.b.ins().fadd(low, step);
                    self.state_store(s, LOW, low, next);
                    match mode {
                        4 => low,
                        5 => self.b.ins().fsub(x, low),
                        _ => {
                            let two = self.f32(2.);
                            let twice = self.b.ins().fmul(two, low);
                            self.b.ins().fsub(twice, x)
                        }
                    }
                } else {
                    let band = self.load_f32(s, BAND);
                    let v3 = self.b.ins().fsub(x, low);
                    let ab = self.b.ins().fmul(a, band);
                    let ga = self.b.ins().fmul(g, a);
                    let gav = self.b.ins().fmul(ga, v3);
                    let next_band = self.b.ins().fadd(ab, gav);
                    let gb = self.b.ins().fmul(g, next_band);
                    let next_low = self.b.ins().fadd(low, gb);
                    let two = self.f32(2.);
                    let t = self.b.ins().fmul(two, next_band);
                    let stored_band = self.b.ins().fsub(t, band);
                    self.state_store(s, BAND, band, stored_band);
                    let t = self.b.ins().fmul(two, next_low);
                    let stored_low = self.b.ins().fsub(t, low);
                    self.state_store(s, LOW, low, stored_low);
                    let kb = |g: &mut Self| g.b.ins().fmul(k, next_band);
                    match mode {
                        1 => {
                            let kb = kb(self);
                            let t = self.b.ins().fsub(x, kb);
                            self.b.ins().fsub(t, next_low)
                        }
                        2 => next_band,
                        3 => {
                            let kb = kb(self);
                            self.b.ins().fsub(x, kb)
                        }
                        _ => next_low,
                    }
                };
                self.finite(y)
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
                let index = self.b.ins().load(I64, MemFlagsData::trusted(), s, INDEX);
                let line = self.b.ins().iadd_imm_s(self.delay, (*base * 4) as i64);
                let x = if *cubic {
                    self.cubic(index, line, *seconds, *fixed_cubic, *length)
                } else {
                    self.linear(index, line, *seconds, *length)
                };
                let returned = match damping {
                    Some(cutoff) => {
                        if self.hoisted[node] == CONSTANT {
                            let cutoff = self.get(*cutoff);
                            self.damping(s, cutoff);
                        }
                        let a = self.memo_value(s, 0);
                        let low = self.load_f32(s, LOW);
                        let d = self.b.ins().fsub(low, x);
                        let d = self.b.ins().fmul(d, a);
                        let low = self.b.ins().fadd(x, d);
                        self.store(low, s, LOW);
                        low
                    }
                    None => x,
                };
                let feedback = self.get(*feedback);
                let feedback = self.clamp_const(feedback, -max_feedback, *max_feedback);
                let env = self.b.ins().fmul(returned, feedback);
                self.store(env, s, ENV);
                self.lines[node] = Some((s, index, env));
                self.finite(x)
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
                let hz = self.b.ins().fmul(base, ratio);
                let hz = self.b.ins().fmul(hz, factor);
                let hz = self.b.ins().fadd(hz, fm);
                let rate = self.f32(self.rate);
                let dt = self.b.ins().fdiv(hz, rate);
                let dt = self.clamp_const(dt, 0., 0.45);
                let cycle = self.b.ins().load(F64, MemFlagsData::trusted(), s, PHASE);
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
                let y = match wave {
                    0 => {
                        let tau = self.f32(TAU);
                        let angle = self.b.ins().fmul(p, tau);
                        self.call(self.signatures.unary, sin as *const (), &[angle])
                    }
                    1 => {
                        let two = self.f32(2.);
                        let ramp = self.b.ins().fmul(two, p);
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
                        let a = self.b.ins().fmul(gain, square);
                        let keep = self.b.ins().fsub(one, gain);
                        let b = self.b.ins().fmul(keep, low);
                        let low = self.b.ins().fadd(a, b);
                        self.store(low, s, LOW);
                        low
                    }
                };
                let dt = self.b.ins().fpromote(F64, dt);
                let cycle = self.b.ins().fadd(cycle, dt);
                let cycle = self.fmod1(cycle);
                self.b.ins().store(MemFlagsData::trusted(), cycle, s, PHASE);
                self.finite(y)
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
                let t = self.from_u64(F32, age);
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
                let x = self.b.ins().fmul(minus_five, since);
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
                self.finite(y)
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
                let first = self.b.ins().icmp_imm_s(IntCC::Equal, index, 0);
                let one = self.b.ins().f64const(1.);
                let due = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, cycle, one);
                let capture = self.b.ins().bor(first, due);
                let held = self.b.ins().select(capture, x, held);
                let whole = self.b.ins().trunc(cycle);
                let fraction = self.b.ins().fsub(cycle, whole);
                let cycle = self.b.ins().select(capture, fraction, cycle);
                self.store(held, s, ENV);
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
                // A held input, finite like every signal.
                self.guard(held, FINITE)
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
                let y = self.b.ins().fdemote(F32, next);
                self.finite(y)
            }
            Op::RawShape { input, shape } => {
                // `Shape::value`, in f64.
                let (table, low, step) = shape.table();
                let last = (table.len() - 1) as f64;
                let x = self.get(*input);
                let x = self.b.ins().fpromote(F64, x);
                let low = self.b.ins().f64const(low);
                let x = self.b.ins().fsub(x, low);
                let step = self.b.ins().f64const(step);
                let p = self.b.ins().fdiv(x, step);
                let zero = self.b.ins().f64const(0.);
                let below = self.b.ins().fcmp(FloatCC::LessThan, p, zero);
                let p = self.b.ins().select(below, zero, p);
                let high = self.b.ins().f64const(last);
                let above = self.b.ins().fcmp(FloatCC::GreaterThan, p, high);
                let p = self.b.ins().select(above, high, p);
                let i = self.b.ins().fcvt_to_uint_sat(I64, p);
                let top = self.b.ins().iconst(I64, table.len() as i64 - 2);
                let i = self.b.ins().umin(i, top);
                let whole = self.b.ins().fcvt_from_uint(F64, i);
                let f = self.b.ins().fsub(p, whole);
                let base = self.b.ins().iconst(I64, table.as_ptr() as i64);
                let offset = self.b.ins().ishl_imm_u(i, 3);
                let at = self.b.ins().iadd(base, offset);
                let a = self.b.ins().load(F64, MemFlagsData::trusted(), at, 0);
                let b = self.b.ins().load(F64, MemFlagsData::trusted(), at, 8);
                let d = self.b.ins().fsub(b, a);
                let d = self.b.ins().fmul(f, d);
                let y = self.b.ins().fadd(a, d);
                let y = self.b.ins().fdemote(F32, y);
                // Between two entries, give or take rounding.
                let bound = table.iter().fold(0f64, |m, v| m.max(v.abs())) * (1. + 1e-9);
                self.guard(y, bound)
            }
            _ => unreachable!("`inline` lists the operations emitted here"),
        }
    }

    /// A segment envelope's frame (`Envelope::step`): inline while it moves
    /// along a linear or smooth segment, holds its sustain or has finished,
    /// otherwise through `mseg_step`.
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
        let exponential = self.b.ins().icmp_imm_s(IntCC::Equal, curve, 2);
        let shaped = self.b.create_block();
        self.b.ins().brif(exponential, slow, &[], shaped, &[]);
        self.b.switch_to_block(shaped);
        let elapsed = self.b.ins().iadd_imm_s(elapsed, 1);
        self.b
            .ins()
            .store(t, elapsed, s, field(offset_of!(SegmentState, elapsed)));
        let p = self.from_u64(F32, elapsed);
        let span = self.from_u64(F32, frames);
        let p = self.b.ins().fdiv(p, span);
        // Smooth: p * p * (3 - 2 * p).
        let square = self.b.ins().fmul(p, p);
        let two = self.f32(2.);
        let three = self.f32(3.);
        let twice = self.b.ins().fmul(two, p);
        let rest = self.b.ins().fsub(three, twice);
        let smooth = self.b.ins().fmul(square, rest);
        let smoothed = self.b.ins().icmp_imm_s(IntCC::Equal, curve, 1);
        let p = self.b.ins().select(smoothed, smooth, p);
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
        let (s, index, env) = match self.lines[node].filter(|_| fresh) {
            Some(line) => line,
            None => {
                let s = self.state_of(node);
                let index = self.b.ins().load(I64, MemFlagsData::trusted(), s, INDEX);
                let env = self.load_f32(s, ENV);
                (s, index, env)
            }
        };
        let x = self.get(input);
        let x = self.b.ins().fadd(x, env);
        let x = self.finite(x);
        let line = self.b.ins().iadd_imm_s(self.delay, (base * 4) as i64);
        let offset = self.b.ins().ishl_imm_u(index, 2);
        let address = self.b.ins().iadd(line, offset);
        self.store(x, address, 0);
        let next = self.b.ins().iadd_imm_s(index, 1);
        let end = self.b.ins().icmp_imm_s(IntCC::Equal, next, length as i64);
        let zero = self.b.ins().iconst(I64, 0);
        let next = self.b.ins().select(end, zero, next);
        self.b.ins().store(MemFlagsData::trusted(), next, s, INDEX);
    }
}
