//! A fixed per-voice graph. Nodes are prepared once; audio evaluates a flat schedule.
use super::*;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::collections::BTreeMap;
use std::f32::consts::TAU;
const N: usize = 64;
#[derive(Clone, Copy)]
enum Input {
    Constant(f32),
    Node(usize),
}
impl Input {
    fn get(self, v: &[f32; N]) -> f32 {
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
    Expression(u8),
    Osc {
        wave: u8,
        ratio: Input,
        detune: Input,
        hz: Option<Input>,
        fm: Input,
        width: Input,
    },
    Noise,
    Envelope {
        attack: Input,
        decay: Input,
        sustain: Input,
        release: Input,
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
        high: bool,
    },
    Delay {
        input: Input,
        seconds: Input,
        feedback: Input,
        base: usize,
        length: usize,
    },
    Sample {
        audio: Vec<f32>,
        rate: f32,
        root: f32,
        looped: bool,
    },
}
struct Parameter {
    name: String,
    value: f32,
    min: f32,
    max: f32,
}
#[derive(Clone, Copy, Default)]
struct State {
    phase: f64,
    env: f32,
    low: f32,
    band: f32,
    index: usize,
}
struct PatchVoice {
    active: bool,
    id: u64,
    channel: u8,
    pitch: f32,
    velocity: f32,
    age: u64,
    released: Option<u64>,
    expression: [f32; 7],
    state: [State; N],
    delay: Vec<f32>,
    last_level: f32,
}
impl PatchVoice {
    fn new(delay: usize) -> Self {
        Self {
            active: false,
            id: 0,
            channel: 0,
            pitch: 60.,
            velocity: 0.,
            age: 0,
            released: None,
            expression: [1., 0.5, 0., 0., 1., 0.5, 0.],
            state: [State::default(); N],
            delay: vec![0.; delay],
            last_level: 0.,
        }
    }
    fn reset(&mut self) {
        self.active = false;
        self.delay.fill(0.);
        self.state.fill(State::default());
    }
}
pub struct VoicePatch {
    core: ProcessorCore,
    rate: f32,
    nodes: Vec<Op>,
    output: Input,
    parameters: Vec<Parameter>,
    voices: Vec<PatchVoice>,
    gain: f32,
    rng: u32,
    has_envelope: bool,
}
fn number(r: &serde_json::Map<String, Value>, key: &str, default: f32) -> Result<f32> {
    let n = r
        .get(key)
        .map(|x| x.as_f64().context("expected number"))
        .transpose()?
        .unwrap_or(default as f64) as f32;
    ensure!(n.is_finite(), "nonfinite graph value");
    Ok(n)
}
fn input(v: Option<&Value>, default: f32, ids: &BTreeMap<String, usize>) -> Result<Input> {
    match v {
        None => Ok(Input::Constant(default)),
        Some(Value::Number(n)) => {
            let v = n.as_f64().context("invalid value")? as f32;
            ensure!(v.is_finite(), "nonfinite graph value");
            Ok(Input::Constant(v))
        }
        Some(Value::String(s)) => Ok(Input::Node(*ids.get(s).with_context(|| {
            format!("unknown/forward graph node '{s}'; put dependencies first")
        })?)),
        _ => bail!("graph input needs a number or node id"),
    }
}
impl VoicePatch {
    pub fn new(d: &model::Device, c: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        Self::prepare(d, c, token).map_err(|e| {
            eprintln!("voice patch {}: {e:#}", d.id);
            DeviceError::InvalidConfig("invalid voice graph")
        })
    }
    fn prepare(d: &model::Device, c: AudioConfig, token: u64) -> Result<Self> {
        let graph = d
            .patch
            .as_ref()
            .context("voice_patch needs nodes and output")?;
        let rows = graph["nodes"]
            .as_array()
            .context("nodes must be an array")?;
        ensure!(
            !rows.is_empty() && rows.len() <= N,
            "voice patch needs 1..64 nodes"
        );
        let mut ids = BTreeMap::new();
        let mut nodes = Vec::new();
        let mut parameters = Vec::new();
        let mut delay = 0;
        let mut sample_frames = 0;
        for row in rows {
            let r = row.as_object().context("node must be a record")?;
            let id = r
                .get("id")
                .and_then(Value::as_str)
                .context("node needs id")?;
            ensure!(!ids.contains_key(id), "duplicate node '{id}'");
            let op = r
                .get("op")
                .and_then(Value::as_str)
                .context("node needs op")?;
            let fields: &[&str] = match op {
                "param" => &["value", "min", "max"],
                "osc" => &["wave", "ratio", "detune", "hz", "fm", "width"],
                "adsr" => &["attack", "decay", "sustain", "release"],
                "sum" | "mul" => &["inputs"],
                "drive" => &["input", "amount"],
                "filter" => &["input", "cutoff", "q", "mode"],
                "delay" => &["input", "seconds", "feedback", "max_seconds"],
                "sample" => &["path", "root", "loop"],
                "expression" => &["kind"],
                "noise" | "frequency" | "velocity" => &[],
                _ => bail!("unknown graph operation '{op}'"),
            };
            for k in r.keys() {
                ensure!(
                    k == "id" || k == "op" || fields.contains(&k.as_str()),
                    "unknown field '{k}' on {op} node '{id}'"
                );
            }
            let i = |k, default| input(r.get(k), default, &ids);
            let operation = match op {
                "param" => {
                    let value = number(r, "value", 0.)?;
                    let min = number(r, "min", 0.)?;
                    let max = number(r, "max", 1.)?;
                    ensure!(
                        min < max && (min..=max).contains(&value),
                        "invalid parameter range"
                    );
                    parameters.push(Parameter {
                        name: id.into(),
                        value,
                        min,
                        max,
                    });
                    Op::Param {
                        index: parameters.len() - 1,
                    }
                }
                "frequency" => Op::Frequency,
                "velocity" => Op::Velocity,
                "expression" => Op::Expression(crate::expression::kind(
                    r.get("kind")
                        .and_then(Value::as_str)
                        .context("expression needs kind")?,
                )?),
                "osc" => Op::Osc {
                    wave: match r.get("wave").and_then(Value::as_str).unwrap_or("sine") {
                        "sine" => 0,
                        "saw" => 1,
                        "pulse" => 2,
                        "triangle" => 3,
                        _ => bail!("wave must be sine, saw, pulse or triangle"),
                    },
                    ratio: i("ratio", 1.)?,
                    detune: i("detune", 0.)?,
                    hz: r.get("hz").map(|v| input(Some(v), 0., &ids)).transpose()?,
                    fm: i("fm", 0.)?,
                    width: i("width", 0.5)?,
                },
                "noise" => Op::Noise,
                "adsr" => Op::Envelope {
                    attack: i("attack", 0.005)?,
                    decay: i("decay", 0.15)?,
                    sustain: i("sustain", 0.7)?,
                    release: i("release", 0.2)?,
                },
                "sum" | "mul" => {
                    let vs = r
                        .get("inputs")
                        .and_then(Value::as_array)
                        .context("sum/mul needs inputs")?;
                    ensure!(
                        !vs.is_empty() && vs.len() <= 16,
                        "sum/mul needs 1..16 inputs"
                    );
                    let vs = vs
                        .iter()
                        .map(|v| input(Some(v), 0., &ids))
                        .collect::<Result<Vec<_>>>()?;
                    if op == "sum" {
                        Op::Sum(vs)
                    } else {
                        Op::Product(vs)
                    }
                }
                "drive" => Op::Drive {
                    input: i("input", 0.)?,
                    amount: i("amount", 1.)?,
                },
                "filter" => {
                    let mode = r.get("mode").and_then(Value::as_str).unwrap_or("lowpass");
                    ensure!(
                        matches!(mode, "lowpass" | "highpass"),
                        "filter mode must be lowpass/highpass"
                    );
                    Op::Filter {
                        input: i("input", 0.)?,
                        cutoff: i("cutoff", 4000.)?,
                        q: i("q", 0.707)?,
                        high: mode == "highpass",
                    }
                }
                "delay" => {
                    let seconds = number(r, "max_seconds", 0.25)?;
                    ensure!(
                        seconds > 0. && seconds <= 1.,
                        "per-voice delay max_seconds must be >0..1"
                    );
                    let length = (seconds * c.sample_rate).ceil() as usize + 2;
                    let base = delay;
                    delay += length;
                    ensure!(
                        delay <= c.sample_rate as usize * 2,
                        "voice graph delay budget is two seconds per voice"
                    );
                    Op::Delay {
                        input: i("input", 0.)?,
                        seconds: i("seconds", 0.1)?,
                        feedback: i("feedback", 0.)?,
                        base,
                        length,
                    }
                }
                "sample" => {
                    let path = r
                        .get("path")
                        .and_then(Value::as_str)
                        .context("sample node needs path")?;
                    let root = std::path::Path::new(graph["_module_dir"].as_str().unwrap_or("."));
                    let (info, audio) =
                        crate::audio_file::load(&root.join(path), 8 * 1024 * 1024 - sample_frames)?;
                    sample_frames += audio.len();
                    Op::Sample {
                        audio: audio.into_iter().map(|x| (x[0] + x[1]) * 0.5).collect(),
                        rate: info.rate as f32,
                        root: number(r, "root", 60.)?,
                        looped: r.get("loop").and_then(Value::as_bool).unwrap_or(false),
                    }
                }
                _ => unreachable!(),
            };
            ids.insert(id.to_string(), nodes.len());
            nodes.push(operation);
        }
        let output = input(graph.get("output"), 0., &ids)?;
        let has_envelope = nodes.iter().any(|n| matches!(n, Op::Envelope { .. }));
        let mut s = Self {
            core: ProcessorCore::new(d.kind, token, c.max_frames),
            rate: c.sample_rate,
            nodes,
            output,
            parameters,
            voices: (0..16).map(|_| PatchVoice::new(delay)).collect(),
            gain: 0.2,
            rng: 0x31415927,
            has_envelope,
        };
        for (k, v) in &d.params {
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
                ..
            } => {
                let index = self
                    .voices
                    .iter()
                    .position(|v| !v.active)
                    .unwrap_or_else(|| {
                        self.voices
                            .iter()
                            .enumerate()
                            .min_by(|(_, a), (_, b)| a.last_level.total_cmp(&b.last_level))
                            .unwrap()
                            .0
                    });
                let v = &mut self.voices[index];
                v.reset();
                v.active = true;
                v.id = note_id;
                v.channel = channel;
                v.pitch = pitch;
                v.velocity = velocity;
                v.age = elapsed_frames;
                v.released = None;
                v.expression = [1., 0.5, 0., 0., 1., 0.5, 0.];
            }
            DeviceEventKind::NoteOff { note_id, .. } => {
                for v in &mut self.voices {
                    if v.id == note_id && v.released.is_none() {
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
                    if v.id == note_id && (expression as usize) < 7 {
                        v.expression[expression as usize] = value as f32;
                    }
                }
            }
            DeviceEventKind::Controller {
                channel,
                controller: 120 | 123,
                ..
            } => {
                for v in &mut self.voices {
                    if v.channel == channel {
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
        for v in &mut self.voices {
            if !v.active {
                continue;
            }
            let mut values = [0.; N];
            let freq = 440. * 2f32.powf((v.pitch + v.expression[2] - 69.) / 12.);
            let mut env_level = 0f32;
            for (index, op) in self.nodes.iter().enumerate() {
                let state = &mut v.state[index];
                let get = |x: Input| x.get(&values);
                let x = match op {
                    Op::Param { index } => self.parameters[*index].value,
                    Op::Frequency => freq,
                    Op::Velocity => v.velocity,
                    Op::Expression(k) => v.expression[*k as usize],
                    Op::Osc {
                        wave,
                        ratio,
                        detune,
                        hz,
                        fm,
                        width,
                    } => {
                        let hz = hz.map(get).unwrap_or(freq)
                            * get(*ratio)
                            * 2f32.powf(get(*detune) / 1200.)
                            + get(*fm);
                        let dt = (hz / self.rate).clamp(0., 0.45);
                        let p = state.phase as f32;
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
                        self.rng ^= self.rng << 13;
                        self.rng ^= self.rng >> 17;
                        self.rng ^= self.rng << 5;
                        self.rng as f32 / u32::MAX as f32 * 2. - 1.
                    }
                    Op::Envelope {
                        attack,
                        decay,
                        sustain,
                        release,
                    } => {
                        let a = get(*attack).clamp(0., 30.);
                        let d = get(*decay).clamp(0.0001, 30.);
                        let sustain = get(*sustain).clamp(0., 1.);
                        if v.released.is_some() {
                            state.env *=
                                (-9.21 / (get(*release).clamp(0.0001, 30.) * self.rate)).exp();
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
                    Op::Sum(vs) => vs.iter().map(|x| get(*x)).sum(),
                    Op::Product(vs) => vs.iter().map(|x| get(*x)).product(),
                    Op::Drive { input, amount } => {
                        (get(*input) * get(*amount).clamp(0., 100.)).tanh()
                    }
                    Op::Filter {
                        input,
                        cutoff,
                        q,
                        high,
                    } => {
                        let g = (std::f32::consts::PI * get(*cutoff).clamp(10., self.rate * 0.45)
                            / self.rate)
                            .tan();
                        let k = 1. / get(*q).clamp(0.2, 20.);
                        let a = 1. / (1. + g * (g + k));
                        let v3 = get(*input) - state.low;
                        let band = a * state.band + g * a * v3;
                        let low = state.low + g * band;
                        state.band = 2. * band - state.band;
                        state.low = 2. * low - state.low;
                        if *high {
                            get(*input) - k * band - low
                        } else {
                            low
                        }
                    }
                    Op::Delay {
                        input,
                        seconds,
                        feedback,
                        base,
                        length,
                    } => {
                        let offset = (get(*seconds) * self.rate).clamp(1., (*length - 2) as f32);
                        let pos = (state.index as f32 - offset).rem_euclid(*length as f32);
                        let i = pos as usize;
                        let frac = pos - i as f32;
                        let x = v.delay[base + i] * (1. - frac)
                            + v.delay[base + (i + 1) % length] * frac;
                        v.delay[base + state.index] =
                            get(*input) + x * get(*feedback).clamp(-0.98, 0.98);
                        state.index = (state.index + 1) % length;
                        x
                    }
                    Op::Sample {
                        audio,
                        rate,
                        root,
                        looped,
                    } => {
                        let pos = state.phase;
                        let i = pos as usize;
                        let x = if i + 1 < audio.len() {
                            audio[i] + (audio[i + 1] - audio[i]) * (pos - i as f64) as f32
                        } else {
                            0.
                        };
                        state.phase += (*rate / self.rate
                            * 2f32.powf((v.pitch + v.expression[2] - root) / 12.))
                            as f64;
                        if *looped {
                            state.phase %= audio.len() as f64;
                        }
                        x
                    }
                };
                values[index] = if x.is_finite() {
                    x.clamp(-100., 100.)
                } else {
                    0.
                };
            }
            let gate = if self.has_envelope {
                1.
            } else {
                (v.age as f32 / (self.rate * 0.003)).min(1.)
                    * v.released.map_or(1., |r| {
                        (-9.21 * (v.age - r) as f32 / (self.rate * 0.03)).exp()
                    })
            };
            let x = self.output.get(&values)
                * v.velocity
                * self.gain
                * v.expression[0]
                * v.expression[4]
                * gate;
            let pan = v.expression[1].clamp(0., 1.) * std::f32::consts::FRAC_PI_2;
            out[0] += x * pan.cos();
            out[1] += x * pan.sin();
            v.last_level = x.abs();
            v.age += 1;
            if v.released.is_some()
                && if self.has_envelope {
                    env_level < 1e-5
                } else {
                    gate < 1e-5
                }
            {
                v.active = false;
            }
        }
        out
    }
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
    fn accepts_note_expression(&self) -> bool {
        true
    }
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }
    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
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
        self.rng = 0x31415927;
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
            let x = self.frame();
            left[frame] = x[0];
            right[frame] = x[1];
        }
        Ok(())
    }
}
