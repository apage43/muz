//! Off-thread source graph lowering. Shared immutable values identify shared state;
//! emitted ids depend only on traversal, never pointer addresses or equal contents.
use crate::lang::{Diagnostic, Record, Unit, Value};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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
fn signal_field(k: &str) -> bool {
    matches!(
        k,
        "ratio"
            | "detune"
            | "hz"
            | "fm"
            | "width"
            | "attack"
            | "decay"
            | "sustain"
            | "release"
            | "input"
            | "amount"
            | "cutoff"
            | "q"
            | "seconds"
            | "feedback"
            | "phase"
            | "damping"
            | "frequency"
            | "speed"
            | "rate_hz"
            | "rise"
            | "fall"
            | "min"
            | "max"
    )
}
fn quantity(k: &str, v: &Value) -> Result<()> {
    if let Value::Num(q) = v {
        let expected = match k {
            "hz" | "fm" | "cutoff" | "rate_hz" | "damping" | "frequency" => Unit::Hz,
            "attack" | "decay" | "release" | "seconds" | "max_seconds" | "tail" | "rise"
            | "fall" | "time" | "offset" | "end" | "loop_crossfade" => Unit::Seconds,
            "value" | "min" | "max" | "input" | "inputs" => return Ok(()),
            _ => Unit::Scalar,
        };
        ensure!(
            q.unit == Unit::Scalar || q.unit == expected,
            "{k} has incompatible units; expected {expected:?}"
        );
    }
    Ok(())
}
#[derive(Default)]
struct Lower {
    seen: BTreeMap<usize, String>,
    ids: BTreeSet<String>,
    nodes: Vec<Json>,
}
impl Lower {
    fn signal(&mut self, v: &Value, depth: usize) -> Result<Json> {
        ensure!(depth <= 64, "signal nesting exceeds 64");
        match v {
            Value::Record(r) => {
                let key = Arc::as_ptr(r) as usize;
                if let Some(id) = self.seen.get(&key) {
                    return Ok(json!(id));
                }
                let op = r.get("op").context("signal needs op")?.text()?;
                let id = if op == "param" {
                    r.get("id")
                        .context("param needs public id")?
                        .text()?
                        .to_owned()
                } else {
                    format!("@{}", self.seen.len())
                };
                if op == "param" {
                    ensure!(
                        !id.starts_with('@') && id != "gain_db",
                        "reserved parameter name '{id}'"
                    );
                }
                ensure!(self.ids.insert(id.clone()), "duplicate graph id '{id}'");
                self.seen.insert(key, id.clone());
                let mut row = self
                    .row(r, true, depth + 1)
                    .map_err(|e| Diagnostic::new(format!("{e:#}")).origin(r.origin()).err())?;
                row["id"] = json!(id);
                ensure!(self.nodes.len() < 64, "voice patch exceeds 64 nodes");
                self.nodes.push(row);
                Ok(json!(id))
            }
            Value::Num(_) | Value::Str(_) => Ok(v.json()),
            _ => bail!("signal needs a number, node reference or signal record"),
        }
    }
    fn row(&mut self, r: &Record, nested: bool, depth: usize) -> Result<Json> {
        let op = r.get("op").context("node needs op")?.text()?;
        let allowed = fields(op).with_context(|| format!("unknown graph operation '{op}'"))?;
        let choices: &[(&str, &[&str])] = match op {
            "osc" => &[("wave", &["sine", "saw", "pulse", "triangle"])],
            "filter" => &[("mode", &["lowpass", "highpass", "bandpass", "notch"])],
            "sample" | "reader" => &[("channel", &["mono", "left", "right"])],
            "shape" => &[("quality", &["raw", "adaa"])],
            "map" => &[("kind", &["clamp", "abs", "reciprocal", "exp2", "log2"])],
            _ => &[],
        };
        for (key, choices) in choices {
            if let Some(value) = r.get(*key) {
                ensure!(
                    choices.contains(&value.text()?),
                    "{op}.{key} must be one of {}",
                    choices.join(", ")
                );
            }
        }
        if let Some(value) = r.get("one_shot") {
            ensure!(matches!(value, Value::Bool(_)), "one_shot needs boolean");
        }
        if op == "param" {
            if let Some(id) = r.get("id") {
                ensure!(
                    ![
                        "gain_db",
                        "glide_ms",
                        "velocity_track",
                        "sample_budget_frames"
                    ]
                    .contains(&id.text()?),
                    "reserved patch control id"
                );
            }
            if let Some(Value::Num(value)) = r.get("value") {
                for key in ["min", "max"] {
                    if let Some(Value::Num(bound)) = r.get(key) {
                        ensure!(
                            bound.unit == Unit::Scalar
                                || value.unit == Unit::Scalar
                                || bound.unit == value.unit,
                            "param {key} units disagree with value"
                        );
                    }
                }
            }
        }
        let mut row = serde_json::Map::new();
        for (k, v) in r.iter() {
            ensure!(
                k == "op" || k == "id" || allowed.contains(&k.as_str()),
                "unknown field '{k}' on {op}"
            );
            if op == "mseg" && matches!(k.as_str(), "attack" | "release") {
                for seg in v.array()? {
                    for (field, value) in seg.record()?.iter() {
                        quantity(field, value)?;
                    }
                }
            } else {
                quantity(k, v)?;
            }
            let value = if nested && signal_field(k) && op != "mseg" && op != "param" {
                self.signal(v, depth)?
            } else if k == "inputs" {
                Json::Array(
                    v.array()?
                        .iter()
                        .map(|x| {
                            if nested {
                                self.signal(x, depth)
                            } else {
                                Ok(x.json())
                            }
                        })
                        .collect::<Result<_>>()?,
                )
            } else {
                v.json()
            };
            row.insert(k.clone(), value);
        }
        if op == "reader" {
            if let Some(source) = r.get("source") {
                let source = source.record()?;
                ensure!(
                    source.get("type").and_then(|v| v.text().ok()) == Some("sample"),
                    "reader source must be sample(...)"
                );
                let zones = crate::compile::sample_zones(source, std::path::Path::new("."))?;
                row.remove("source");
                row.insert("zones".into(), serde_json::to_value(zones)?);
            }
        }
        Ok(Json::Object(row))
    }
    fn output(&mut self, v: &Value) -> Result<Json> {
        if let Value::Record(r) = v {
            if !r.contains_key("op") {
                ensure!(
                    r.len() == 2 && r.contains_key("left") && r.contains_key("right"),
                    "stereo output needs left and right"
                );
                return Ok(
                    json!({"left":self.signal(&r["left"],0)?,"right":self.signal(&r["right"],0)?}),
                );
            }
        }
        self.signal(v, 0)
    }
}
pub(crate) fn lower(r: &Record) -> Result<Json> {
    let result = (|| -> Result<Json> {
        if let Some(value) = r.get("sample_budget_frames") {
            ensure!(
                matches!(value, Value::Num(q) if q.unit == Unit::Scalar),
                "sample_budget_frames requires a unitless integer"
            );
        }
        let mut l = Lower::default();
        let mut graph = Json::Object(r.iter().map(|(k, v)| (k.clone(), v.json())).collect());
        crate::model::patch_sample_budget(&graph).map_err(anyhow::Error::msg)?;
        if let Some(nodes) = r.get("nodes") {
            for node in nodes.array()? {
                let row = node.record()?;
                l.nodes.push(
                    Lower::default().row(row, false, 0).map_err(|e| {
                        Diagnostic::new(format!("{e:#}")).origin(row.origin()).err()
                    })?,
                );
            }
        } else {
            graph["output"] = l.output(r.get("output").context("patch needs output")?)?;
            if let Some(lifetime) = r.get("lifetime") {
                let life = lifetime.record()?;
                graph["lifetime"]["envelope"] =
                    l.signal(life.get("envelope").context("lifetime needs envelope")?, 0)?;
                if let Some(t) = life.get("tail") {
                    quantity("tail", t)?;
                }
            }
        }
        ensure!(
            !l.nodes.is_empty() && l.nodes.len() <= 64,
            "patch needs 1..64 nodes"
        );
        let mut ids = BTreeSet::new();
        for node in &l.nodes {
            let id = node["id"].as_str().context("node needs id")?;
            let op = node["op"].as_str().context("node needs op")?;
            let check = |value: &Json| -> Result<()> {
                if let Some(name) = value.as_str() {
                    ensure!(
                        ids.contains(name),
                        "node '{id}' refers to unknown/forward node '{name}'; put dependencies first"
                    );
                } else {
                    ensure!(
                        value.is_number(),
                        "node '{id}' input must be a number or signal"
                    );
                }
                Ok(())
            };
            for (key, value) in node.as_object().unwrap() {
                if signal_field(key) && op != "param" && op != "mseg" {
                    check(value)?;
                }
                if key == "inputs" {
                    let inputs = value.as_array().context("inputs needs array")?;
                    ensure!(
                        (1..=16).contains(&inputs.len()),
                        "sum/mul needs 1..16 inputs"
                    );
                    for v in inputs {
                        check(v)?;
                    }
                }
            }
            ensure!(ids.insert(id.to_owned()), "duplicate graph id '{id}'");
        }
        ensure!(graph.get("output").is_some(), "patch needs output");
        if let Some(mode) = graph.get("voice_mode") {
            ensure!(
                matches!(mode.as_str(), Some("poly" | "legato" | "retrigger")),
                "voice_mode must be poly, legato or retrigger"
            );
        }
        graph["nodes"] = Json::Array(l.nodes);
        Ok(graph)
    })();
    result.map_err(|e| Diagnostic::new(format!("{e:#}")).origin(r.origin()).err())
}
