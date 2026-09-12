//! Prepared segment envelopes. Segment policy is source data; runtime state is fixed.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
#[derive(Clone, Copy)]
struct Segment {
    frames: u64,
    target: f32,
    shape: u8,
}
pub(super) struct Envelope {
    attack: Vec<Segment>,
    release: Vec<Segment>,
    sustain: Option<usize>,
    pub one_shot: bool,
}
#[derive(Clone, Copy, Default)]
pub(super) struct State {
    index: usize,
    elapsed: u64,
    start: f32,
    pub value: f32,
    releasing: bool,
    pub done: bool,
}
impl Envelope {
    pub fn prepare(r: &serde_json::Map<String, Value>, rate: f32) -> Result<Self> {
        let segments = |key: &str, limit: usize| -> Result<Vec<Segment>> {
            let rows = r
                .get(key)
                .and_then(Value::as_array)
                .with_context(|| format!("mseg {key} needs segments"))?;
            ensure!(
                !rows.is_empty() && rows.len() <= limit,
                "mseg {key} needs 1..{limit} segments"
            );
            rows.iter()
                .map(|v| {
                    let row = v.as_object().context("segment needs record")?;
                    ensure!(
                        row.keys()
                            .all(|k| matches!(k.as_str(), "time" | "to" | "curve")),
                        "unknown segment field"
                    );
                    let time = row
                        .get("time")
                        .and_then(Value::as_f64)
                        .context("segment needs time")?;
                    let target = row
                        .get("to")
                        .and_then(Value::as_f64)
                        .context("segment needs to")?;
                    ensure!(
                        time.is_finite()
                            && (0.0..=60.0).contains(&time)
                            && target.is_finite()
                            && (-100.0..=100.0).contains(&target),
                        "invalid segment time or target"
                    );
                    let shape = match row.get("curve").and_then(Value::as_str).unwrap_or("linear") {
                        "linear" => 0,
                        "smooth" => 1,
                        "exp" => 2,
                        _ => anyhow::bail!("segment curve must be linear, smooth or exp"),
                    };
                    Ok(Segment {
                        frames: (time * rate as f64).round() as u64,
                        target: target as f32,
                        shape,
                    })
                })
                .collect()
        };
        let attack = segments("attack", 16)?;
        let release = segments("release", 8)?;
        let one_shot = r
            .get("one_shot")
            .map(|v| v.as_bool().context("one_shot needs boolean"))
            .transpose()?
            .unwrap_or(false);
        let sustain = r
            .get("sustain")
            .filter(|v| !v.is_null())
            .map(|v| v.as_u64().context("mseg sustain needs endpoint index"))
            .transpose()?
            .map(|v| v as usize);
        ensure!(
            sustain.is_none_or(|i| i < attack.len()),
            "mseg sustain index out of range"
        );
        ensure!(
            release.last().unwrap().target == 0.,
            "mseg release must end at zero"
        );
        ensure!(
            !one_shot || attack.last().unwrap().target == 0.,
            "one-shot mseg must end at zero"
        );
        Ok(Self {
            attack,
            release,
            sustain,
            one_shot,
        })
    }
    pub fn step(&self, s: &mut State, released: bool, choked: bool, rate: f32) -> f32 {
        if choked {
            s.value *= (-9.21 / (0.008 * rate)).exp();
            s.done = s.value.abs() < 1e-5;
            return s.value;
        }
        if released && !self.one_shot && !s.releasing {
            s.releasing = true;
            s.index = 0;
            s.elapsed = 0;
            s.start = s.value;
            s.done = false;
        }
        if s.done {
            return s.value;
        }
        let segments = if s.releasing {
            &self.release
        } else {
            &self.attack
        };
        loop {
            if s.index >= segments.len() {
                s.done = s.releasing || self.one_shot;
                return s.value;
            }
            let seg = segments[s.index];
            if s.elapsed >= seg.frames {
                s.value = seg.target;
                if !s.releasing && !self.one_shot && self.sustain == Some(s.index) {
                    return s.value;
                }
                s.start = s.value;
                s.index += 1;
                s.elapsed = 0;
                continue;
            }
            s.elapsed += 1;
            let p = s.elapsed as f32 / seg.frames as f32;
            let p = match seg.shape {
                1 => p * p * (3. - 2. * p),
                2 => (1. - (-5. * p).exp()) / (1. - (-5f32).exp()),
                _ => p,
            };
            s.value = s.start + (seg.target - s.start) * p;
            return s.value;
        }
    }
}
