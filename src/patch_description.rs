//! Typed, configuration-independent patch boundary. Runtime preparation still
//! checks sample resources and rate-dependent storage before activating a graph.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Signal {
    Constant(f64),
    Node(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Output {
    Mono(Signal),
    Stereo { left: Signal, right: Signal },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub time: f64,
    pub to: f64,
    pub curve: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(flatten)]
    pub operation: Operation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Debug)]
pub struct ValidatedPatch {
    nodes: Vec<Node>,
    output: Output,
}
impl ValidatedPatch {
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
        let nodes: Vec<Node> = serde_json::from_value(value["nodes"].clone())?;
        ensure!(
            (1..=64).contains(&nodes.len()),
            "voice patch needs 1..64 nodes"
        );
        let mut ids = BTreeSet::new();
        let mut delay = 0.;
        for node in &nodes {
            ensure!(
                !node.id.is_empty() && !ids.contains(&node.id),
                "invalid/duplicate patch node ID"
            );
            let raw = serde_json::to_value(&node.operation)?;
            // The enum has already enforced field types. Validate all signal edges
            // before resource decoding, retaining authored node order and sharing.
            for (key, v) in raw.as_object().unwrap() {
                if v.is_null() {
                    continue;
                }
                let signal = matches!(
                    key.as_str(),
                    "input"
                        | "ratio"
                        | "detune"
                        | "hz"
                        | "fm"
                        | "width"
                        | "phase"
                        | "amount"
                        | "cutoff"
                        | "q"
                        | "seconds"
                        | "feedback"
                        | "damping"
                        | "frequency"
                        | "speed"
                        | "rate_hz"
                        | "rise"
                        | "fall"
                ) || (matches!(node.operation, Operation::Adsr { .. })
                    && matches!(key.as_str(), "attack" | "decay" | "sustain" | "release"))
                    || (matches!(node.operation, Operation::Resonator { .. }) && key == "decay")
                    || (matches!(node.operation, Operation::Map { .. })
                        && matches!(key.as_str(), "min" | "max"));
                if signal {
                    check_signal(&serde_json::from_value::<Signal>(v.clone())?, &ids)?;
                }
            }
            match &node.operation {
                Operation::Param { value, min, max } => {
                    let (v, a, b) = (value.unwrap_or(0.), min.unwrap_or(0.), max.unwrap_or(1.));
                    ensure!(
                        node.id != "sample_budget_frames"
                            && a.is_finite()
                            && b.is_finite()
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
                    for i in inputs {
                        check_signal(i, &ids)?;
                    }
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
                    ensure!(root.unwrap_or(60.).is_finite(), "invalid sample root");
                    choice(channel.as_deref(), &["mono", "left", "right"])?;
                }
                Operation::Reader {
                    channel,
                    offset,
                    end,
                    loop_crossfade,
                    ..
                } => {
                    choice(channel.as_deref(), &["mono", "left", "right"])?;
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
            ids.insert(node.id.clone());
        }
        let output: Output = serde_json::from_value(value["output"].clone())?;
        match &output {
            Output::Mono(s) => check_signal(s, &ids)?,
            Output::Stereo { left, right } => {
                check_signal(left, &ids)?;
                check_signal(right, &ids)?;
            }
        }
        if let Some(l) = value.get("lifetime") {
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
            ensure!(
                (0.0..=60.).contains(&l["tail"].as_f64().unwrap_or(0.)),
                "invalid lifetime tail"
            );
        }
        crate::model::patch_sample_budget(value).map_err(anyhow::Error::msg)?;
        Ok(Self { nodes, output })
    }
}
fn choice(value: Option<&str>, choices: &[&str]) -> Result<()> {
    ensure!(
        value.is_none_or(|v| choices.contains(&v)),
        "unsupported patch option {value:?}"
    );
    Ok(())
}
fn check_signal(s: &Signal, ids: &BTreeSet<String>) -> Result<()> {
    match s {
        Signal::Constant(n) => ensure!(
            n.is_finite() && (*n as f32).is_finite(),
            "invalid signal constant"
        ),
        Signal::Node(id) => ensure!(ids.contains(id), "unknown/forward signal reference {id}"),
    };
    Ok(())
}
