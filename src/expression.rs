//! Compact per-note programs. Copying sounding-note obligations never allocates or frees memory.
use crate::diagnostic::{Diagnostic, suggest_vocabulary};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const STANDARD_COUNT: usize = 7;
pub const SLOT_COUNT: usize = 32;
pub const POINT_COUNT: usize = 32;
const STANDARD_NAMES: [&str; STANDARD_COUNT] = [
    "volume",
    "pan",
    "tuning",
    "vibrato",
    "expression",
    "brightness",
    "pressure",
];

/// Checked destination vocabulary. Custom names have stable lexicographic slots.
/// Keep authored defaults as f64 as well: any schema change is structural identity.
#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    custom: BTreeMap<String, f64>,
    defaults: [f32; SLOT_COUNT],
}
impl Schema {
    pub fn standard() -> Self {
        let mut defaults = [0.; SLOT_COUNT];
        defaults[..STANDARD_COUNT].copy_from_slice(&[1., 0.5, 0., 0., 1., 0.5, 0.]);
        Self {
            custom: BTreeMap::new(),
            defaults,
        }
    }
    pub fn native(value: Option<&serde_json::Value>) -> Result<Self> {
        let mut schema = Self::standard();
        let Some(value) = value else {
            return Ok(schema);
        };
        let record = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("note_controls must be a record"))?;
        ensure!(
            record.len() <= SLOT_COUNT - STANDARD_COUNT,
            "at most 25 custom note controls"
        );
        for (name, value) in record {
            ensure!(!name.is_empty(), "note control name must not be empty");
            ensure!(
                !STANDARD_NAMES.contains(&name.as_str()),
                "standard note expression '{name}' is reserved"
            );
            let value = value
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("note control '{name}' default must be numeric"))?;
            ensure!(
                value.is_finite() && (0.0..=1.).contains(&value),
                "note control '{name}' default must be 0..1"
            );
            schema.custom.insert(name.clone(), value);
        }
        for (index, default) in schema.custom.values().enumerate() {
            schema.defaults[STANDARD_COUNT + index] = *default as f32;
        }
        Ok(schema)
    }
    pub fn resolve(&self, name: &str) -> Result<u8> {
        if let Some(index) = STANDARD_NAMES.iter().position(|n| *n == name) {
            return Ok(index as u8);
        }
        if let Some(index) = self.custom.keys().position(|n| n == name) {
            return Ok((STANDARD_COUNT + index) as u8);
        }
        Err(Diagnostic::new(format!("unknown note expression '{name}'"))
            .helps(suggest_vocabulary(
                "note expressions",
                name,
                STANDARD_NAMES
                    .into_iter()
                    .chain(self.custom.keys().map(String::as_str)),
            ))
            .err())
    }
    pub fn contains(&self, kind: u8) -> bool {
        (kind as usize) < STANDARD_COUNT + self.custom.len()
    }
    pub fn defaults(&self) -> &[f32; SLOT_COUNT] {
        &self.defaults
    }
    pub fn validate_program(&self, program: &Program) -> Result<()> {
        program.validate()?;
        for kind in program.kinds() {
            ensure!(
                self.contains(kind),
                "undeclared note expression slot {kind}"
            );
        }
        Ok(())
    }
}

pub fn kind(name: &str) -> Result<u8> {
    Schema::standard().resolve(name)
}
fn range(kind: u8) -> (f64, f64) {
    match kind {
        0 => (0., 4.),
        2 => (-120., 120.),
        _ => (0., 1.),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub phase: f32,
    pub value: f32,
    pub kind: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Program {
    pub points: [Point; POINT_COUNT],
    pub len: u8,
}

/// Shared source validation, before or after a destination is known.
fn visit_points(
    value: Option<&serde_json::Value>,
    mut visit: impl FnMut(&str, f32, f32) -> Result<()>,
) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    let record = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("note expression must be a record"))?;
    let mut count = 0;
    for (name, curve) in record {
        ensure!(!name.is_empty(), "note expression name must not be empty");
        let k = STANDARD_NAMES
            .iter()
            .position(|n| *n == name)
            .unwrap_or(STANDARD_COUNT) as u8;
        let (lo, hi) = range(k);
        let mut previous = -1.;
        let mut point = |phase: f64, value: f64| -> Result<()> {
            ensure!(
                phase.is_finite() && (0.0..=1.).contains(&phase) && phase as f32 > previous,
                "expression phases must increase in 0..1"
            );
            ensure!(
                value.is_finite() && (lo..=hi).contains(&value),
                "{name} expression must be {lo}..{hi}"
            );
            ensure!(count < POINT_COUNT, "at most 32 expression points per note");
            count += 1;
            previous = phase as f32;
            visit(name, phase as f32, value as f32)
        };
        if let Some(value) = curve.as_f64() {
            point(0., value)?;
        } else {
            let points = curve.as_array().ok_or_else(|| {
                anyhow::anyhow!("expression requires a value or [[phase,value],...]")
            })?;
            ensure!(!points.is_empty(), "expression curve is empty");
            for pair in points {
                let pair = pair
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("expression point requires a pair"))?;
                ensure!(pair.len() == 2, "expression point requires a pair");
                point(
                    pair[0]
                        .as_f64()
                        .ok_or_else(|| anyhow::anyhow!("phase must be numeric"))?,
                    pair[1]
                        .as_f64()
                        .ok_or_else(|| anyhow::anyhow!("value must be numeric"))?,
                )?;
            }
        }
    }
    Ok(())
}
impl Program {
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Validate custom syntax without implying that any destination declares it.
    pub fn validate_syntax(value: Option<&serde_json::Value>) -> Result<()> {
        visit_points(value, |_, _, _| Ok(()))
    }
    pub fn parse(value: Option<&serde_json::Value>) -> Result<Self> {
        Self::parse_with_schema(value, &Schema::standard())
    }
    pub fn parse_with_schema(value: Option<&serde_json::Value>, schema: &Schema) -> Result<Self> {
        let mut program = Self::default();
        visit_points(value, |name, phase, value| {
            let kind = schema.resolve(name)?;
            program.points[program.len as usize] = Point { phase, value, kind };
            program.len += 1;
            Ok(())
        })?;
        Ok(program)
    }
    /// Validate direct/session payloads before they reach the scheduler.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.len as usize <= POINT_COUNT,
            "at most 32 note expression points"
        );
        let mut previous = [-1.; SLOT_COUNT];
        for p in &self.points[..self.len as usize] {
            ensure!(
                (p.kind as usize) < SLOT_COUNT,
                "invalid note expression slot {}",
                p.kind
            );
            let (lo, hi) = range(p.kind);
            ensure!(
                p.phase.is_finite()
                    && (0.0..=1.).contains(&p.phase)
                    && p.phase > previous[p.kind as usize]
                    && p.value.is_finite()
                    && (lo..=hi).contains(&(p.value as f64)),
                "invalid note expression point"
            );
            previous[p.kind as usize] = p.phase;
        }
        Ok(())
    }
    fn active_points(&self) -> &[Point] {
        &self.points[..usize::from(self.len).min(POINT_COUNT)]
    }
    /// Distinct authored slots, in first-point order; no heap storage or slot scan.
    pub fn kinds(&self) -> impl Iterator<Item = u8> + '_ {
        let mut seen = 0u32;
        self.active_points().iter().filter_map(move |point| {
            let bit = 1u32.checked_shl(point.kind.into())?;
            if seen & bit != 0 {
                return None;
            }
            seen |= bit;
            Some(point.kind)
        })
    }
    pub fn value(&self, kind: u8, phase: f32) -> Option<f32> {
        let mut points = self.active_points().iter().filter(|p| p.kind == kind);
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
        self.kinds().any(|kind| {
            let mut values = self
                .active_points()
                .iter()
                .filter(|p| p.kind == kind)
                .map(|p| p.value);
            let first = values.next();
            values.any(|value| Some(value) != first)
        })
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
        self.validate().map_err(serde::ser::Error::custom)?;
        self.active_points().serialize(s)
    }
}
impl<'de> Deserialize<'de> for Program {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ProgramVisitor;
        impl<'de> serde::de::Visitor<'de> for ProgramVisitor {
            type Value = Program;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("at most 32 note expression points")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Program, A::Error> {
                let mut program = Program::default();
                while let Some(point) = seq.next_element::<Point>()? {
                    if program.len as usize == POINT_COUNT {
                        return Err(serde::de::Error::custom(
                            "at most 32 note expression points",
                        ));
                    }
                    program.points[program.len as usize] = point;
                    program.len += 1;
                }
                // Preserve legacy snapshot canonicalization of point order.
                program.points[..program.len as usize]
                    .sort_by(|a, b| a.kind.cmp(&b.kind).then(a.phase.total_cmp(&b.phase)));
                program.validate().map_err(serde::de::Error::custom)?;
                Ok(program)
            }
        }
        d.deserialize_seq(ProgramVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_order_defaults_and_identity() {
        let a = Schema::native(Some(&json!({"z": 0.8, "a": 0.2}))).unwrap();
        let b = Schema::native(Some(&json!({"a": 0.2, "z": 0.8}))).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.resolve("a").unwrap(), 7);
        assert_eq!(a.resolve("z").unwrap(), 8);
        assert_eq!(a.defaults()[7], 0.2);
        assert_eq!(&a.defaults()[..7], &[1., 0.5, 0., 0., 1., 0.5, 0.]);
        assert_ne!(
            a,
            Schema::native(Some(&json!({"a": 0.3, "z": 0.8}))).unwrap()
        );
        for invalid in [
            json!([]),
            json!({"": 0}),
            json!({"pressure": 0}),
            json!({"mute": -0.1}),
            json!({"mute": "x"}),
        ] {
            assert!(Schema::native(Some(&invalid)).is_err());
        }
    }
    #[test]
    fn syntax_defers_binding_but_not_shape_or_budget() {
        let source = json!({"mute": [[0.2, 0.1], [0.8, 0.9]], "pressure": 0.4});
        Program::validate_syntax(Some(&source)).unwrap();
        assert!(Program::parse(Some(&source)).is_err());
        let schema = Schema::native(Some(&json!({"mute": 0.3}))).unwrap();
        let program = Program::parse_with_schema(Some(&source), &schema).unwrap();
        assert_eq!(program.kinds().collect::<Vec<_>>(), vec![7, 6]);
        assert!(program.changing());
        assert_eq!(program.value(7, 0.), Some(0.1));
        assert!((program.value(7, 0.5).unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(program.value(7, 1.), Some(0.9));
        for invalid in [
            json!({"mute": []}),
            json!({"mute": [[0.5, 0], [0.5, 1]]}),
            json!({"mute": 1.1}),
            json!({"mute": [[-0.1, 0]]}),
            json!({"mute": [[0, 0, 0]]}),
            json!({"": 0}),
        ] {
            assert!(Program::validate_syntax(Some(&invalid)).is_err());
        }
        let too_many: Vec<_> = (0..33).map(|i| json!([i as f64 / 32., 0.5])).collect();
        assert!(Program::validate_syntax(Some(&json!({"mute": too_many}))).is_err());
    }
    #[test]
    fn highest_slot_roundtrip_and_malformed_programs() {
        let controls: serde_json::Map<String, serde_json::Value> =
            (0..25).map(|i| (format!("c{i:02}"), json!(0.5))).collect();
        let schema = Schema::native(Some(&serde_json::Value::Object(controls.clone()))).unwrap();
        assert_eq!(schema.resolve("c24").unwrap(), 31);
        let program =
            Program::parse_with_schema(Some(&json!({"c24": [[0, 0], [1, 1]]})), &schema).unwrap();
        let roundtrip: Program =
            serde_json::from_value(serde_json::to_value(program).unwrap()).unwrap();
        assert_eq!(roundtrip, program);
        schema.validate_program(&roundtrip).unwrap();
        assert!(Schema::standard().validate_program(&roundtrip).is_err());
        let mut extra = controls;
        extra.insert("extra".into(), json!(0));
        assert!(Schema::native(Some(&serde_json::Value::Object(extra))).is_err());
        for invalid in [
            json!([{"kind": 32, "phase": 0, "value": 0}]),
            json!([{"kind": 255, "phase": 0, "value": 0}]),
            json!([{"kind": 31, "phase": 0, "value": 2}]),
        ] {
            assert!(serde_json::from_value::<Program>(invalid).is_err());
        }
        let malformed = Program {
            len: 255,
            ..Program::default()
        };
        assert!(malformed.validate().is_err());
        assert!(serde_json::to_value(malformed).is_err());
        assert_eq!(malformed.kinds().collect::<Vec<_>>(), vec![0]);
    }
    #[test]
    fn standard_semantics_and_interleaved_distinct_kinds() {
        let program =
            Program::parse(Some(&json!({"volume": 4, "tuning": -120, "pressure": 1}))).unwrap();
        assert_eq!(program.value(kind("volume").unwrap(), 0.), Some(4.));
        assert!(!program.changing());
        let mut program = Program::default();
        program.len = 3;
        program.points[..3].copy_from_slice(&[
            Point {
                kind: 31,
                phase: 0.,
                value: 0.,
            },
            Point {
                kind: 0,
                phase: 0.,
                value: 1.,
            },
            Point {
                kind: 31,
                phase: 1.,
                value: 1.,
            },
        ]);
        program.validate().unwrap();
        assert_eq!(program.kinds().collect::<Vec<_>>(), vec![31, 0]);
        assert!(program.changing());
        program.points[2].phase = 0.;
        assert!(program.validate().is_err());
    }
}
