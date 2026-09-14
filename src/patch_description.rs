//! Typed, configuration-independent patch boundary. Runtime preparation still
//! checks sample resources and rate-dependent storage before activating a graph.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) fn fields(op: &str) -> Option<&'static [&'static str]> {
    Some(match op {
        "shape" => &["input", "points", "quality"],
        "resonator" => &["input", "frequency", "decay"],
        "reader" => &[
            "source",
            "zones",
            "channel",
            "speed",
            "offset",
            "end",
            "loop_crossfade",
        ],
        "mseg" => &["attack", "release", "sustain", "one_shot"],
        "map" => &["input", "kind", "min", "max"],
        "hold" => &["input", "rate_hz"],
        "slew" => &["input", "rise", "fall"],
        "param" => &["value", "min", "max"],
        "osc" => &["wave", "ratio", "detune", "hz", "fm", "width", "phase"],
        "adsr" => &["attack", "decay", "sustain", "release", "one_shot"],
        "sum" | "mul" => &["inputs"],
        "drive" => &["input", "amount"],
        "filter" => &["input", "cutoff", "q", "mode"],
        "delay" => &[
            "input",
            "seconds",
            "feedback",
            "max_seconds",
            "damping",
            "max_feedback",
        ],
        "sample" => &["path", "root", "loop", "channel"],
        "expression" => &["kind"],
        "noise" | "frequency" | "velocity" => &[],
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Signal {
    Constant(f64),
    Node(String),
    #[serde(skip)]
    Resolved(usize),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Output {
    Mono(Signal),
    Stereo { left: Signal, right: Signal },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub time: f64,
    pub to: f64,
    pub curve: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(flatten)]
    pub operation: Operation,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Param {
        value: Option<f64>,
        min: Option<f64>,
        max: Option<f64>,
    },
    Frequency,
    Velocity,
    Noise,
    Expression {
        kind: String,
    },
    Osc {
        wave: Option<String>,
        ratio: Option<Signal>,
        detune: Option<Signal>,
        hz: Option<Signal>,
        fm: Option<Signal>,
        width: Option<Signal>,
        phase: Option<Signal>,
    },
    Adsr {
        attack: Option<Signal>,
        decay: Option<Signal>,
        sustain: Option<Signal>,
        release: Option<Signal>,
        one_shot: Option<bool>,
    },
    Sum {
        inputs: Vec<Signal>,
    },
    Mul {
        inputs: Vec<Signal>,
    },
    Drive {
        input: Option<Signal>,
        amount: Option<Signal>,
    },
    Filter {
        input: Option<Signal>,
        cutoff: Option<Signal>,
        q: Option<Signal>,
        mode: Option<String>,
    },
    Delay {
        input: Option<Signal>,
        seconds: Option<Signal>,
        feedback: Option<Signal>,
        max_seconds: Option<f64>,
        damping: Option<Signal>,
        max_feedback: Option<f64>,
    },
    Sample {
        path: String,
        root: Option<f64>,
        #[serde(rename = "loop")]
        looped: Option<bool>,
        channel: Option<String>,
    },
    Shape {
        input: Option<Signal>,
        points: Vec<[f64; 2]>,
        quality: Option<String>,
    },
    Resonator {
        input: Option<Signal>,
        frequency: Option<Signal>,
        decay: Option<Signal>,
    },
    Reader {
        source: Option<serde_json::Value>,
        zones: Vec<crate::model::SampleZone>,
        channel: Option<String>,
        speed: Option<Signal>,
        offset: Option<f64>,
        end: Option<f64>,
        loop_crossfade: Option<f64>,
    },
    Mseg {
        attack: Vec<Segment>,
        release: Vec<Segment>,
        sustain: Option<usize>,
        one_shot: Option<bool>,
    },
    Map {
        input: Option<Signal>,
        kind: Option<String>,
        min: Option<Signal>,
        max: Option<Signal>,
    },
    Hold {
        input: Option<Signal>,
        rate_hz: Option<Signal>,
    },
    Slew {
        input: Option<Signal>,
        rise: Option<Signal>,
        fall: Option<Signal>,
    },
}
impl Operation {
    fn visit_signals(&mut self, mut visit: impl FnMut(&mut Signal) -> Result<()>) -> Result<()> {
        let signals: Vec<&mut Option<Signal>> = match self {
            Self::Osc {
                ratio,
                detune,
                hz,
                fm,
                width,
                phase,
                ..
            } => vec![ratio, detune, hz, fm, width, phase],
            Self::Adsr {
                attack,
                decay,
                sustain,
                release,
                ..
            } => vec![attack, decay, sustain, release],
            Self::Drive { input, amount } => vec![input, amount],
            Self::Filter {
                input, cutoff, q, ..
            } => vec![input, cutoff, q],
            Self::Delay {
                input,
                seconds,
                feedback,
                damping,
                ..
            } => vec![input, seconds, feedback, damping],
            Self::Shape { input, .. } => vec![input],
            Self::Resonator {
                input,
                frequency,
                decay,
            } => vec![input, frequency, decay],
            Self::Reader { speed, .. } => vec![speed],
            Self::Map {
                input, min, max, ..
            } => vec![input, min, max],
            Self::Hold { input, rate_hz } => vec![input, rate_hz],
            Self::Slew { input, rise, fall } => vec![input, rise, fall],
            Self::Sum { inputs } | Self::Mul { inputs } => {
                for signal in inputs {
                    visit(signal)?;
                }
                return Ok(());
            }
            Self::Param { .. }
            | Self::Frequency
            | Self::Velocity
            | Self::Noise
            | Self::Expression { .. }
            | Self::Sample { .. }
            | Self::Mseg { .. } => vec![],
        };
        for signal in signals.into_iter().filter_map(Option::as_mut) {
            visit(signal)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct ValidatedPatch {
    nodes: Vec<Node>,
    output: Output,
    lifetime: Option<(usize, f64)>,
    sample_budget: usize,
    module_dir: std::path::PathBuf,
    voice_mode: u8,
    controls: BTreeMap<String, f32>,
}
impl ValidatedPatch {
    pub fn validate_control(&self, name: &str, value: f32) -> Result<(), &'static str> {
        let (min, max) = match name {
            "gain_db" => (-90., 24.),
            "glide_ms" => (0., 10000.),
            "velocity_track" => (0., 2.),
            _ => self
                .nodes
                .iter()
                .find_map(|node| match &node.operation {
                    Operation::Param { min, max, .. } if node.id == name => {
                        Some((min.unwrap_or(0.) as f32, max.unwrap_or(1.) as f32))
                    }
                    _ => None,
                })
                .ok_or("unknown control")?,
        };
        if value.is_finite() && (min..=max).contains(&value) {
            Ok(())
        } else {
            Err("control outside supported range")
        }
    }
    pub fn controls(&self) -> &BTreeMap<String, f32> {
        &self.controls
    }

    pub fn same_structure(&self, other: &Self) -> bool {
        self.output == other.output
            && self.lifetime == other.lifetime
            && self.sample_budget == other.sample_budget
            && self.module_dir == other.module_dir
            && self.voice_mode == other.voice_mode
            && self.nodes.len() == other.nodes.len()
            && self.nodes.iter().zip(&other.nodes).all(|(a, b)| {
                a.id == b.id
                    && match (&a.operation, &b.operation) {
                        (
                            Operation::Param {
                                min: a_min,
                                max: a_max,
                                ..
                            },
                            Operation::Param {
                                min: b_min,
                                max: b_max,
                                ..
                            },
                        ) => a_min == b_min && a_max == b_max,
                        (a, b) => a == b,
                    }
            })
    }
    pub fn lifetime(&self) -> Option<(usize, f64)> {
        self.lifetime
    }
    pub fn sample_budget(&self) -> usize {
        self.sample_budget
    }
    pub fn module_dir(&self) -> &std::path::Path {
        &self.module_dir
    }
    pub fn voice_mode(&self) -> u8 {
        self.voice_mode
    }
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    pub fn output(&self) -> &Output {
        &self.output
    }
    pub fn from_json(value: &serde_json::Value) -> Result<Self> {
        for row in value["nodes"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("nodes must be an array"))?
        {
            let op = row["op"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("node needs operation"))?;
            let fields =
                fields(op).ok_or_else(|| anyhow::anyhow!("unknown graph operation {op}"))?;
            ensure!(
                row.as_object()
                    .unwrap()
                    .keys()
                    .all(|k| k == "id" || k == "op" || fields.contains(&k.as_str())),
                "unknown node field"
            );
        }
        let mut nodes: Vec<Node> = serde_json::from_value(value["nodes"].clone())?;
        ensure!(
            (1..=64).contains(&nodes.len()),
            "voice patch needs 1..64 nodes"
        );
        let mut ids = BTreeMap::new();
        let mut delay = 0.;
        for node in &mut nodes {
            ensure!(
                !node.id.is_empty() && !ids.contains_key(&node.id),
                "invalid/duplicate patch node ID"
            );
            match &node.operation {
                Operation::Param { value, min, max } => {
                    let (v, a, b) = (value.unwrap_or(0.), min.unwrap_or(0.), max.unwrap_or(1.));
                    ensure!(
                        node.id != "sample_budget_frames"
                            && a.is_finite()
                            && b.is_finite()
                            && (a as f32).is_finite()
                            && (b as f32).is_finite()
                            && a < b
                            && (a..=b).contains(&v),
                        "invalid parameter range"
                    );
                }
                Operation::Expression { kind } => {
                    crate::expression::kind(kind)?;
                }
                Operation::Sum { inputs } | Operation::Mul { inputs } => {
                    ensure!(
                        (1..=16).contains(&inputs.len()),
                        "sum/mul needs 1..16 inputs"
                    );
                }
                Operation::Osc { wave, phase, .. } => {
                    choice(wave.as_deref(), &["sine", "saw", "pulse", "triangle"])?;
                    ensure!(
                        wave.as_deref().unwrap_or("sine") == "sine"
                            || !matches!(phase, Some(Signal::Node(_))),
                        "signal phase requires sine oscillator"
                    );
                }
                Operation::Filter { mode, .. } => choice(
                    mode.as_deref(),
                    &["lowpass", "highpass", "bandpass", "notch"],
                )?,
                Operation::Map { kind, .. } => choice(
                    kind.as_deref(),
                    &["clamp", "abs", "reciprocal", "exp2", "log2"],
                )?,
                Operation::Delay {
                    max_seconds,
                    max_feedback,
                    ..
                } => {
                    let n = max_seconds.unwrap_or(0.25);
                    ensure!(n > 0. && n <= 1., "invalid delay capacity");
                    delay += n;
                    ensure!(
                        delay <= 2. && (0.0..=0.99999).contains(&max_feedback.unwrap_or(0.98)),
                        "invalid delay budget/feedback"
                    );
                }
                Operation::Sample { root, channel, .. } => {
                    ensure!(
                        (root.unwrap_or(60.) as f32).is_finite(),
                        "invalid sample root"
                    );
                    choice(channel.as_deref(), &["mono", "left", "right"])?;
                }
                Operation::Reader {
                    zones,
                    channel,
                    offset,
                    end,
                    loop_crossfade,
                    ..
                } => {
                    choice(channel.as_deref(), &["mono", "left", "right"])?;
                    ensure!(
                        !zones.is_empty()
                            && zones.len()
                                <= crate::model::graph_budget().map_err(anyhow::Error::msg)?,
                        "invalid reader zone count"
                    );
                    for source in zones {
                        ensure!(
                            source.root.is_finite()
                                && (0.0..=127.0).contains(&source.root)
                                && source.offset_seconds.is_finite()
                                && source.offset_seconds >= 0.
                                && source.gain_db.is_finite()
                                && (-120.0..=120.0).contains(&source.gain_db),
                            "invalid reader calibration"
                        );
                        ensure!(
                            source.keys[0] <= source.keys[1]
                                && source.keys[1] <= 127
                                && source.velocity[0] >= 0.
                                && source.velocity[1] <= 1.
                                && source.velocity[0] <= source.velocity[1],
                            "invalid reader zone range"
                        );
                    }
                    ensure!(
                        offset.unwrap_or(0.) >= 0.
                            && end.is_none_or(|e| e > offset.unwrap_or(0.))
                            && loop_crossfade.unwrap_or(0.) >= 0.,
                        "invalid reader bounds"
                    );
                }
                Operation::Shape {
                    points, quality, ..
                } => {
                    choice(quality.as_deref(), &["adaa", "raw"])?;
                    ensure!(
                        (2..=64).contains(&points.len())
                            && points
                                .iter()
                                .flatten()
                                .all(|v| v.is_finite() && v.abs() <= 1e6)
                            && points.windows(2).all(|w| w[0][0] < w[1][0]),
                        "invalid shape points"
                    );
                    ensure!(
                        (points.last().unwrap()[0] - points[0][0]) / 2048. > 1e-12,
                        "shape domain is too small"
                    );
                }
                Operation::Mseg {
                    attack,
                    release,
                    sustain,
                    one_shot,
                } => {
                    ensure!(
                        (1..=16).contains(&attack.len())
                            && (1..=8).contains(&release.len())
                            && sustain.is_none_or(|i| i < attack.len()),
                        "invalid mseg segments/index"
                    );
                    for s in attack.iter().chain(release) {
                        ensure!(
                            (0.0..=60.).contains(&s.time) && (-100.0..=100.).contains(&s.to),
                            "invalid segment range"
                        );
                        choice(s.curve.as_deref(), &["linear", "smooth", "exp"])?;
                    }
                    ensure!(
                        release.last().unwrap().to == 0.
                            && (!one_shot.unwrap_or(false) || attack.last().unwrap().to == 0.),
                        "envelope must end at zero"
                    );
                }
                _ => {}
            }
            node.operation.visit_signals(|s| resolve_signal(s, &ids))?;
            ids.insert(node.id.clone(), ids.len());
        }
        let mut output: Output = serde_json::from_value(value["output"].clone())?;
        match &mut output {
            Output::Mono(s) => resolve_signal(s, &ids)?,
            Output::Stereo { left, right } => {
                resolve_signal(left, &ids)?;
                resolve_signal(right, &ids)?;
            }
        }
        let lifetime = if let Some(l) = value.get("lifetime") {
            ensure!(
                l.as_object()
                    .is_some_and(|r| r.keys().all(|k| matches!(k.as_str(), "envelope" | "tail"))),
                "unknown lifetime field"
            );
            let name = l["envelope"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("lifetime needs envelope"))?;
            ensure!(
                nodes.iter().any(|n| n.id == name
                    && matches!(
                        n.operation,
                        Operation::Adsr { .. } | Operation::Mseg { .. } | Operation::Reader { .. }
                    )),
                "invalid lifetime envelope"
            );
            let tail = l
                .get("tail")
                .map(|v| {
                    v.as_f64()
                        .ok_or_else(|| anyhow::anyhow!("lifetime tail needs number"))
                })
                .transpose()?
                .unwrap_or(0.);
            ensure!((0.0..=60.).contains(&tail), "invalid lifetime tail");
            Some((nodes.iter().position(|n| n.id == name).unwrap(), tail))
        } else {
            None
        };
        let sample_budget = crate::model::patch_sample_budget(value).map_err(anyhow::Error::msg)?;
        let voice_mode = match value
            .get("voice_mode")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| anyhow::anyhow!("voice_mode needs string"))
            })
            .transpose()?
            .unwrap_or("poly")
        {
            "poly" => 0,
            "legato" => 1,
            "retrigger" => 2,
            _ => anyhow::bail!("voice_mode must be poly, legato or retrigger"),
        };
        let module_dir = value
            .get("_module_dir")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| anyhow::anyhow!("module directory needs string"))
            })
            .transpose()?
            .unwrap_or(".")
            .into();
        let mut controls = BTreeMap::from([
            ("gain_db".to_owned(), 20.0 * 0.2_f32.log10()),
            ("glide_ms".to_owned(), 0.),
            ("velocity_track".to_owned(), 1.),
        ]);
        for node in &nodes {
            if let Operation::Param { value, .. } = node.operation {
                controls.insert(node.id.clone(), value.unwrap_or(0.) as f32);
            }
        }
        for (name, control) in &mut controls {
            if let Some(value) = value.get(name) {
                *control = value
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("control {name} needs number"))?
                    as f32;
                ensure!(control.is_finite(), "nonfinite control {name}");
            }
        }
        let checked = Self {
            nodes,
            output,
            lifetime,
            sample_budget,
            module_dir,
            voice_mode,
            controls,
        };
        for (name, value) in &checked.controls {
            checked
                .validate_control(name, *value)
                .map_err(anyhow::Error::msg)?;
        }
        Ok(checked)
    }
}
fn choice(value: Option<&str>, choices: &[&str]) -> Result<()> {
    ensure!(
        value.is_none_or(|v| choices.contains(&v)),
        "unsupported patch option {value:?}"
    );
    Ok(())
}
fn resolve_signal(s: &mut Signal, ids: &BTreeMap<String, usize>) -> Result<()> {
    match s {
        Signal::Constant(n) => ensure!(
            n.is_finite() && (*n as f32).is_finite(),
            "invalid signal constant"
        ),
        Signal::Node(id) => {
            *s = Signal::Resolved(
                *ids.get(id)
                    .ok_or_else(|| anyhow::anyhow!("unknown/forward signal reference {id}"))?,
            );
        }
        Signal::Resolved(_) => anyhow::bail!("signal was already resolved"),
    };
    Ok(())
}
