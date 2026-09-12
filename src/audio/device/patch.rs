//! A fixed per-voice graph. Nodes are prepared once; audio evaluates a flat schedule.
use super::*;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::sync::Arc;
#[path = "patch_envelope.rs"]
mod envelope;
#[path = "patch_sample.rs"]
mod sample;
#[path = "patch_shape.rs"]
mod shape;
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
struct Parameter {
    name: String,
    value: f32,
    min: f32,
    max: f32,
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
    released: Option<u64>,
    choked: bool,
    expression: [f32; 7],
    state: [State; N],
    delay: Vec<f32>,
    last_level: f32,
    completed: Option<u64>,
}
impl PatchVoice {
    fn new(delay: usize) -> Self {
        Self {
            active: false,
            id: 0,
            channel: 0,
            pitch: 60.,
            target_pitch: 60.,
            glide_left: 0,
            velocity: 0.,
            age: 0,
            released: None,
            choked: false,
            expression: [1., 0.5, 0., 0., 1., 0.5, 0.],
            state: [State::default(); N],
            delay: vec![0.; delay],
            last_level: 0.,
            completed: None,
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
    output: Input,
    right: Option<Input>,
    lifetime: Option<(usize, u64)>,
    parameters: Vec<Parameter>,
    voices: Vec<PatchVoice>,
    gain: f32,
    rng: u32,
    has_envelope: bool,
    one_shot: bool,
    voice_mode: u8,
    glide_ms: f32,
    velocity_track: f32,
    reader_counts: [usize; N],
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
        let sample_budget = model::patch_sample_budget(graph).map_err(anyhow::Error::msg)?;
        sample::preflight(&crate::assets::paths(d), sample_budget)?;
        let mut sample_frames = 0;
        let mut assets = BTreeMap::<std::path::PathBuf, (f32, Arc<[[f32; 2]]>)>::new();
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
            let fields = crate::patch_source::fields(op)
                .with_context(|| format!("unknown graph operation '{op}'"))?;
            for k in r.keys() {
                ensure!(
                    k == "id" || k == "op" || fields.contains(&k.as_str()),
                    "unknown field '{k}' on {op} node '{id}'"
                );
            }
            let i = |k, default| input(r.get(k), default, &ids);
            let operation = match op {
                "param" => {
                    ensure!(
                        id != "sample_budget_frames",
                        "sample_budget_frames is a preparation setting, not a control"
                    );
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
                    detune_factor: match i("detune", 0.)? {
                        Input::Constant(x) => Some(2f32.powf(x / 1200.)),
                        _ => None,
                    },
                    hz: r.get("hz").map(|v| input(Some(v), 0., &ids)).transpose()?,
                    fm: i("fm", 0.)?,
                    width: i("width", 0.5)?,
                    phase: {
                        let phase = i("phase", 0.)?;
                        ensure!(
                            r.get("wave").and_then(Value::as_str).unwrap_or("sine") == "sine"
                                || matches!(phase, Input::Constant(_)),
                            "signal phase modulation currently requires a sine oscillator; other waves accept constant phase"
                        );
                        phase
                    },
                },
                "noise" => Op::Noise,
                "adsr" => Op::Envelope {
                    one_shot: r
                        .get("one_shot")
                        .map(|v| v.as_bool().context("one_shot must be boolean"))
                        .transpose()?
                        .unwrap_or(false),
                    attack: i("attack", 0.005)?,
                    decay: i("decay", 0.15)?,
                    sustain: i("sustain", 0.7)?,
                    release: i("release", 0.2)?,
                },
                "shape" => Op::Shape {
                    input: i("input", 0.)?,
                    shape: shape::Shape::prepare(r)?,
                },
                "resonator" => {
                    let frequency = i("frequency", 440.)?;
                    let decay = i("decay", 1.)?;
                    let coefficients =
                        if let (Input::Constant(f), Input::Constant(d)) = (frequency, decay) {
                            Some(resonator_coefficients(f, d, c.sample_rate))
                        } else {
                            None
                        };
                    Op::Resonator {
                        input: i("input", 0.)?,
                        frequency,
                        decay,
                        coefficients,
                    }
                }
                "reader" => Op::Reader {
                    reader: sample::Reader::prepare(
                        r,
                        &mut assets,
                        &mut sample_frames,
                        sample_budget,
                    )?,
                    speed: i("speed", 1.)?,
                },
                "mseg" => Op::Mseg(envelope::Envelope::prepare(r, c.sample_rate)?),
                "map" => Op::Map {
                    input: i("input", 0.)?,
                    min: i("min", 0.)?,
                    max: i("max", 1.)?,
                    kind: match r.get("kind").and_then(Value::as_str).unwrap_or("clamp") {
                        "clamp" => 0,
                        "abs" => 1,
                        "reciprocal" => 2,
                        "exp2" => 3,
                        "log2" => 4,
                        _ => bail!("map kind must be clamp, abs, reciprocal, exp2 or log2"),
                    },
                },
                "hold" => Op::Hold {
                    input: i("input", 0.)?,
                    hz: i("rate_hz", 1.)?,
                },
                "slew" => Op::Slew {
                    input: i("input", 0.)?,
                    rise: i("rise", 0.04)?,
                    fall: i("fall", 0.04)?,
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
                        matches!(mode, "lowpass" | "highpass" | "bandpass" | "notch"),
                        "filter mode must be lowpass, highpass, bandpass or notch"
                    );
                    Op::Filter {
                        input: i("input", 0.)?,
                        cutoff: i("cutoff", 4000.)?,
                        q: i("q", 0.707)?,
                        coefficients: match (i("cutoff", 4000.)?, i("q", 0.707)?) {
                            (Input::Constant(cutoff), Input::Constant(q)) => {
                                Some(filter_coefficients(cutoff, q, c.sample_rate))
                            }
                            _ => None,
                        },
                        mode: match mode {
                            "highpass" => 1,
                            "bandpass" => 2,
                            "notch" => 3,
                            _ => 0,
                        },
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
                        damping: r
                            .get("damping")
                            .map(|v| input(Some(v), 4000., &ids))
                            .transpose()?,
                        max_feedback: {
                            let x = number(r, "max_feedback", 0.98)?;
                            ensure!(
                                (0.0..=0.99999).contains(&x),
                                "max_feedback must be 0..0.99999"
                            );
                            x
                        },
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
                    let path = root.join(path).canonicalize()?;
                    let (rate, audio) =
                        sample::asset(&path, &mut assets, &mut sample_frames, sample_budget)?;
                    let channel = match r.get("channel").and_then(Value::as_str).unwrap_or("mono") {
                        "mono" => 0,
                        "left" => 1,
                        "right" => 2,
                        _ => bail!("sample channel must be mono, left or right"),
                    };
                    Op::Sample {
                        audio,
                        rate,
                        channel,
                        root: number(r, "root", 60.)?,
                        looped: r.get("loop").and_then(Value::as_bool).unwrap_or(false),
                    }
                }
                _ => unreachable!(),
            };
            ids.insert(id.to_string(), nodes.len());
            nodes.push(operation);
        }
        ensure!(graph.get("output").is_some(), "patch needs output");
        let (output, right) = if let Some(pair) = graph["output"].as_object() {
            ensure!(
                pair.len() == 2 && pair.contains_key("left") && pair.contains_key("right"),
                "stereo output needs left and right"
            );
            (
                input(pair.get("left"), 0., &ids)?,
                Some(input(pair.get("right"), 0., &ids)?),
            )
        } else {
            (input(graph.get("output"), 0., &ids)?, None)
        };
        let lifetime = graph
            .get("lifetime")
            .map(|value| -> Result<_> {
                let r = value
                    .as_object()
                    .context("lifetime needs {envelope, tail}")?;
                ensure!(
                    r.keys().all(|k| matches!(k.as_str(), "envelope" | "tail")),
                    "unknown lifetime field"
                );
                let name = r
                    .get("envelope")
                    .and_then(Value::as_str)
                    .context("lifetime needs envelope node id")?;
                let index = *ids.get(name).context("unknown lifetime envelope")?;
                ensure!(
                    matches!(
                        nodes[index],
                        Op::Envelope { .. } | Op::Mseg(_) | Op::Reader { .. }
                    ),
                    "lifetime envelope must be adsr, mseg or reader"
                );
                let tail = number(r, "tail", 0.)?;
                ensure!(
                    (0.0..=60.0).contains(&tail),
                    "lifetime tail must be 0..60 seconds"
                );
                Ok((index, (tail * c.sample_rate).ceil() as u64))
            })
            .transpose()?;
        let has_envelope = nodes
            .iter()
            .any(|n| matches!(n, Op::Envelope { .. } | Op::Mseg(_)));
        let one_shot = has_envelope
            && nodes.iter().all(|n| match n {
                Op::Envelope { one_shot, .. } => *one_shot,
                Op::Mseg(e) => e.one_shot,
                _ => true,
            });
        let mut s = Self {
            core: ProcessorCore::new(d.kind, token, c.max_frames),
            rate: c.sample_rate,
            nodes,
            output,
            right,
            lifetime,
            parameters,
            voices: (0..16).map(|_| PatchVoice::new(delay)).collect(),
            gain: 0.2,
            rng: 0x31415927,
            one_shot,
            has_envelope,
            voice_mode: match graph
                .get("voice_mode")
                .and_then(Value::as_str)
                .unwrap_or("poly")
            {
                "poly" => 0,
                "legato" => 1,
                "retrigger" => 2,
                _ => bail!("voice_mode must be poly, legato or retrigger"),
            },
            glide_ms: 0.,
            velocity_track: 1.,
            reader_counts: [0; N],
        };
        for (k, v) in &d.control_values() {
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
                if self.voice_mode == 1 {
                    if let Some(index) = continuing {
                        let v = &mut self.voices[index];
                        v.id = note_id;
                        v.channel = channel;
                        v.velocity = velocity;
                        v.target_pitch = pitch;
                        v.glide_left = (self.glide_ms * 0.001 * self.rate).round() as u64;
                        if v.glide_left == 0 {
                            v.pitch = pitch;
                        }
                        v.expression = [1., 0.5, 0., 0., 1., 0.5, 0.];
                        return;
                    }
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
                v.age = elapsed_frames;
                v.released = None;
                v.choked = false;
                v.expression = [1., 0.5, 0., 0., 1., 0.5, 0.];
                for (i, op) in self.nodes.iter().enumerate() {
                    if let Op::Reader { reader, .. } = op {
                        reader.start(
                            &mut v.state[i].reader,
                            key,
                            pitch,
                            velocity,
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
        for v in &mut self.voices {
            if !v.active {
                continue;
            }
            if v.glide_left > 0 {
                v.pitch += (v.target_pitch - v.pitch) / v.glide_left as f32;
                v.glide_left -= 1;
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
                        detune_factor,
                        hz,
                        fm,
                        width,
                        phase,
                    } => {
                        let hz = hz.map(get).unwrap_or(freq)
                            * get(*ratio)
                            * detune_factor.unwrap_or_else(|| 2f32.powf(get(*detune) / 1200.))
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
                        self.rng ^= self.rng << 13;
                        self.rng ^= self.rng >> 17;
                        self.rng ^= self.rng << 5;
                        self.rng as f32 / u32::MAX as f32 * 2. - 1.
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
                            state.env *= (-9.21 / (0.008 * self.rate)).exp();
                        } else if v.released.is_some() && !one_shot {
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
                            resonator_coefficients(get(*frequency), get(*decay), self.rate)
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
                            (-1. / (time * self.rate)).exp()
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
                            filter_coefficients(get(*cutoff), get(*q), self.rate)
                        });
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
                        input,
                        seconds,
                        feedback,
                        damping,
                        max_feedback,
                        base,
                        length,
                    } => {
                        // Position the read head in f64. In f32, a delay time just
                        // below an integer sample (e.g. 0.008 s) can make
                        // rem_euclid round up to exactly `length`, indexing past the
                        // end of the buffer. f64 keeps the fractional position and the
                        // truncating cast below safely in [0, length).
                        let offset =
                            (get(*seconds) * self.rate).clamp(1., (*length - 2) as f32) as f64;
                        let pos_f = (state.index as f64 - offset).rem_euclid(*length as f64);
                        let i = pos_f as usize;
                        let frac = (pos_f - i as f64) as f32;
                        let x = v.delay[base + i] * (1. - frac)
                            + v.delay[base + (i + 1) % length] * frac;
                        let returned = if let Some(cutoff) = damping {
                            let a =
                                (-TAU * get(*cutoff).clamp(1., self.rate * 0.45) / self.rate).exp();
                            state.low = x + (state.low - x) * a;
                            state.low
                        } else {
                            x
                        };
                        v.delay[base + state.index] = get(*input)
                            + returned * get(*feedback).clamp(-max_feedback, *max_feedback);
                        state.index = (state.index + 1) % length;
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
                        state.phase += (*rate / self.rate
                            * 2f32.powf((v.pitch + v.expression[2] - root) / 12.))
                            as f64;
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
            let gain = v.velocity.powf(self.velocity_track)
                * self.gain
                * v.expression[0]
                * v.expression[4]
                * gate
                * choke;
            let x = self.output.get(&values) * gain;
            let pan = v.expression[1].clamp(0., 1.);
            if let Some(right) = self.right {
                let y = right.get(&values) * gain;
                out[0] += x * (2. * (1. - pan)).sqrt();
                out[1] += y * (2. * pan).sqrt();
                v.last_level = x.abs().max(y.abs());
            } else {
                out[0] += x * (pan * std::f32::consts::FRAC_PI_2).cos();
                out[1] += x * (pan * std::f32::consts::FRAC_PI_2).sin();
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
        out
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
    fn accepts_note_expression(&self, _kind: u8) -> bool {
        true
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
        self.rng = 0x31415927;
        self.reader_counts.fill(0);
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
