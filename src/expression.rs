//! Compact per-note programs. Copying sounding-note obligations never allocates or frees memory.
use crate::diagnostic::{Diagnostic, suggest_vocabulary};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub phase: f32,
    pub value: f32,
    pub kind: u8,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Program {
    pub points: [Point; 32],
    pub len: u8,
}
impl Default for Program {
    fn default() -> Self {
        Self {
            points: [Point::default(); 32],
            len: 0,
        }
    }
}
pub fn kind(name: &str) -> Result<u8> {
    Ok(match name {
        "volume" => 0,
        "pan" => 1,
        "tuning" => 2,
        "vibrato" => 3,
        "expression" => 4,
        "brightness" => 5,
        "pressure" => 6,
        _ => {
            return Err(Diagnostic::new(format!("unknown note expression '{name}'"))
                .helps(suggest_vocabulary(
                    "note expressions",
                    name,
                    [
                        "volume",
                        "pan",
                        "tuning",
                        "vibrato",
                        "expression",
                        "brightness",
                        "pressure",
                    ],
                ))
                .err());
        }
    })
}
impl Program {
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn parse(value: Option<&serde_json::Value>) -> Result<Self> {
        let mut p = Self::default();
        let Some(value) = value else { return Ok(p) };
        for (name, curve) in value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("note expression must be a record"))?
        {
            let k = kind(name)?;
            let points = if let Some(v) = curve.as_f64() {
                vec![(0., v)]
            } else {
                curve
                    .as_array()
                    .ok_or_else(|| {
                        anyhow::anyhow!("expression requires a value or [[phase,value],...]")
                    })?
                    .iter()
                    .map(|v| -> Result<_> {
                        let v = v
                            .as_array()
                            .ok_or_else(|| anyhow::anyhow!("expression point requires a pair"))?;
                        ensure!(v.len() == 2, "expression point requires a pair");
                        Ok((
                            v[0].as_f64()
                                .ok_or_else(|| anyhow::anyhow!("phase must be numeric"))?,
                            v[1].as_f64()
                                .ok_or_else(|| anyhow::anyhow!("value must be numeric"))?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            ensure!(!points.is_empty(), "expression curve is empty");
            let mut previous = -1.;
            for (phase, value) in points {
                ensure!(
                    phase.is_finite() && (0.0..=1.).contains(&phase) && phase > previous,
                    "expression phases must increase in 0..1"
                );
                let (lo, hi) = match k {
                    0 => (0., 4.),
                    2 => (-120., 120.),
                    _ => (0., 1.),
                };
                ensure!(
                    value.is_finite() && (lo..=hi).contains(&value),
                    "{name} expression must be {lo}..{hi}"
                );
                ensure!(
                    (p.len as usize) < p.points.len(),
                    "at most 32 expression points per note"
                );
                p.points[p.len as usize] = Point {
                    phase: phase as f32,
                    value: value as f32,
                    kind: k,
                };
                p.len += 1;
                previous = phase;
            }
        }
        Ok(p)
    }
    pub fn value(&self, kind: u8, phase: f32) -> Option<f32> {
        let mut points = self.points[..self.len as usize]
            .iter()
            .filter(|p| p.kind == kind);
        let mut last = points.next()?;
        if phase <= last.phase {
            return Some(last.value);
        }
        for p in points {
            if phase < p.phase {
                return Some(
                    last.value
                        + (p.value - last.value) * (phase - last.phase) / (p.phase - last.phase),
                );
            }
            last = p;
        }
        Some(last.value)
    }
    pub fn changing(&self) -> bool {
        self.points[..self.len as usize]
            .windows(2)
            .any(|p| p[0].kind == p[1].kind && p[0].value != p[1].value)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Performance {
    pub pitch: f64,
    pub velocity: f64,
    #[serde(default, skip_serializing_if = "Program::is_empty")]
    pub expression: Program,
}

impl Serialize for Program {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.points[..self.len as usize].serialize(s)
    }
}
impl<'de> Deserialize<'de> for Program {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut points = Vec::<Point>::deserialize(d)?;
        if points.len() > 32 {
            return Err(serde::de::Error::custom(
                "at most 32 note expression points",
            ));
        }
        points.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.phase.total_cmp(&b.phase)));
        let mut last = [-1.; 7];
        for p in &points {
            let (lo, hi) = match p.kind {
                0 => (0., 4.),
                2 => (-120., 120.),
                _ => (0., 1.),
            };
            if p.kind > 6
                || !p.phase.is_finite()
                || !(0.0..=1.).contains(&p.phase)
                || p.phase <= last[p.kind as usize]
                || !p.value.is_finite()
                || !(lo..=hi).contains(&p.value)
            {
                return Err(serde::de::Error::custom("invalid note expression point"));
            }
            last[p.kind as usize] = p.phase;
        }
        let mut program = Self::default();
        program.len = points.len() as u8;
        program.points[..points.len()].copy_from_slice(&points);
        Ok(program)
    }
}
