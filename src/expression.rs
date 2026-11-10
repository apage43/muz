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
    /// Custom slots carry plugin-parameter units, checked against the plugin's
    /// reported ranges when the instrument is prepared rather than as 0..1.
    plugin: bool,
}
impl Schema {
    pub fn standard() -> Self {
        let mut defaults = [0.; SLOT_COUNT];
        defaults[..STANDARD_COUNT].copy_from_slice(&[1., 0.5, 0., 0., 1., 0.5, 0.]);
        Self {
            custom: BTreeMap::new(),
            defaults,
            plugin: false,
        }
    }
    /// Custom slots named by a plugin's `note_controls`, in the same lexicographic
    /// order the plugin adapter binds them.
    pub fn plugin<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<Self> {
        let mut schema = Self::standard();
        schema.plugin = true;
        for name in names {
            ensure!(!name.is_empty(), "note control name must not be empty");
            ensure!(
                !STANDARD_NAMES.contains(&name),
                "standard note expression '{name}' is reserved"
            );
            schema.custom.insert(name.to_owned(), 0.);
        }
        ensure!(
            schema.custom.len() <= SLOT_COUNT - STANDARD_COUNT,
            "at most 25 custom note controls"
        );
        Ok(schema)
    }
    /// The schema a device's note programs are checked against.
    pub fn for_device(device: &crate::model::Device) -> Result<Self> {
        match device.kind {
            crate::model::DeviceKind::VoicePatch => {
                Self::native(device.patch.as_ref().and_then(|p| p.get("note_controls")))
            }
            crate::model::DeviceKind::Clap => Self::plugin(
                device
                    .vst3
                    .iter()
                    .flat_map(|c| c.note_controls.keys().map(String::as_str)),
            ),
            _ => Ok(Self::standard()),
        }
    }
    /// Value bounds checkable without a prepared destination.
    pub fn bounds(&self, kind: u8) -> Option<(f64, f64)> {
        (!self.plugin || (kind as usize) < STANDARD_COUNT).then(|| default_range(kind))
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
        for p in program.active_points() {
            if let Some((lo, hi)) = self.bounds(p.kind) {
                ensure!(
                    (lo..=hi).contains(&f64::from(p.value)),
                    "note expression slot {} must be {lo}..{hi}",
                    p.kind
                );
            }
        }
        Ok(())
    }
}

pub fn kind(name: &str) -> Result<u8> {
    Schema::standard().resolve(name)
}
/// Standard ranges, and 0..1 for custom native controls. Plugin note controls
/// use plugin-parameter units; their ranges are known only to a prepared plugin.
pub fn default_range(kind: u8) -> (f64, f64) {
    match kind {
        0 => (0., 4.),
        2 => (-120., 120.),
        _ => (0., 1.),
    }
}
/// Bounds checkable without a destination: custom slots only need finite values.
fn range(kind: u8) -> (f64, f64) {
    if (kind as usize) < STANDARD_COUNT {
        default_range(kind)
    } else {
        (f64::MIN, f64::MAX)
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

/// Where an authored curve point sits within its note. Phases are fractions of
/// the performed gate; times count from the performed attack. Note data stores a
/// time as `{"seconds": x}` or `{"beats": x}`; a bare number is a phase.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Position {
    Phase(f64),
    Seconds(f64),
    Beats(f64),
}
impl Position {
    fn parse(value: &serde_json::Value) -> Result<Self> {
        if let Some(phase) = value.as_f64() {
            ensure!(
                phase.is_finite() && (0.0..=1.).contains(&phase),
                "expression phases must be 0..1; give a time from the attack with a unit (150ms, 1/2b)"
            );
            return Ok(Self::Phase(phase));
        }
        let time = value.as_object().filter(|o| o.len() == 1).and_then(|o| {
            let (unit, amount) = o.iter().next()?;
            let amount = amount.as_f64()?;
            match unit.as_str() {
                "seconds" => Some(Self::Seconds(amount)),
                "beats" => Some(Self::Beats(amount)),
                _ => None,
            }
        });
        let Some(time) = time else {
            anyhow::bail!(
                "expression point position must be a phase 0..1 or a time from the attack (ms, s or beats)"
            );
        };
        let (Self::Seconds(amount) | Self::Beats(amount)) = time else {
            unreachable!()
        };
        ensure!(amount.is_finite(), "expression times must be finite");
        ensure!(
            amount >= 0.,
            "expression times count from the attack and must not be negative ({time})"
        );
        Ok(time)
    }
    fn same_measure(self, other: Self) -> Option<(f64, f64)> {
        match (self, other) {
            (Self::Phase(a), Self::Phase(b))
            | (Self::Seconds(a), Self::Seconds(b))
            | (Self::Beats(a), Self::Beats(b)) => Some((a, b)),
            _ => None,
        }
    }
}
impl std::fmt::Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Phase(x) => write!(f, "phase {x}"),
            Self::Seconds(x) => write!(f, "{}ms", x * 1000.),
            Self::Beats(x) => write!(f, "{x}b"),
        }
    }
}

/// Maps a time position to a phase of one performed note; may exceed 1.
pub type Resolver<'a> = &'a dyn Fn(Position) -> f64;

/// Shared source validation, before or after a destination is known. Without a
/// resolver only syntax is checked: values, positions, and order among points
/// of the same measure. With one, each curve is resolved to phases, ended at
/// the note's end, checked for increasing order, and visited.
fn visit_points(
    value: Option<&serde_json::Value>,
    resolve: Option<Resolver>,
    mut visit: impl FnMut(&str, f32, f32) -> Result<()>,
) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    let record = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("note expression must be a record"))?;
    let mut count = 0;
    let mut timed = false;
    for (name, curve) in record {
        ensure!(!name.is_empty(), "note expression name must not be empty");
        let k = STANDARD_NAMES
            .iter()
            .position(|n| *n == name)
            .unwrap_or(STANDARD_COUNT) as u8;
        let (lo, hi) = range(k);
        let mut authored = Vec::new();
        let mut point = |position: Position, value: f64| -> Result<()> {
            if (k as usize) < STANDARD_COUNT {
                ensure!(
                    value.is_finite() && (lo..=hi).contains(&value),
                    "{name} expression must be {lo}..{hi}"
                );
            } else {
                // Custom ranges depend on the destination and are checked there.
                ensure!(value.is_finite(), "{name} expression must be finite");
            }
            // Order is checkable now only among points of the same measure.
            if let Some((before, now)) = authored
                .iter()
                .rev()
                .find_map(|(p, _): &(Position, f64)| p.same_measure(position))
                && before >= now
            {
                if matches!(position, Position::Phase(_)) {
                    anyhow::bail!("expression phases must increase in 0..1");
                }
                anyhow::bail!("{name} expression times must increase (at {position})");
            }
            authored.push((position, value));
            Ok(())
        };
        if let Some(value) = curve.as_f64() {
            point(Position::Phase(0.), value)?;
        } else {
            let points = curve.as_array().ok_or_else(|| {
                anyhow::anyhow!("expression requires a value or [[position,value],...]")
            })?;
            ensure!(!points.is_empty(), "expression curve is empty");
            for pair in points {
                let pair = pair
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("expression point requires a pair"))?;
                ensure!(pair.len() == 2, "expression point requires a pair");
                point(
                    Position::parse(&pair[0])?,
                    pair[1]
                        .as_f64()
                        .ok_or_else(|| anyhow::anyhow!("value must be numeric"))?,
                )?;
            }
        }
        let Some(resolve) = resolve else {
            // Time positions may end early on short notes, so their budget is
            // known only for a performed note.
            timed |= authored
                .iter()
                .any(|(p, _)| !matches!(p, Position::Phase(_)));
            count += authored.len();
            ensure!(
                timed || count <= POINT_COUNT,
                "at most 32 expression points per note"
            );
            continue;
        };
        let mut previous: Option<(f32, f64)> = None;
        for (index, (position, value)) in authored.into_iter().enumerate() {
            let phase = match position {
                Position::Phase(phase) => phase,
                time => resolve(time),
            };
            ensure!(
                phase.is_finite() && phase >= 0.,
                "{name} expression point {} ({position}) does not resolve within the note",
                index + 1
            );
            // The first point at or past the note's end ends the curve there.
            let (phase, value, last) = if phase >= 1. || phase as f32 >= 1. {
                let value = match previous {
                    Some((before, held)) if phase > 1. => {
                        held + (value - held) * (1. - f64::from(before))
                            / (phase - f64::from(before))
                    }
                    _ => value,
                };
                (1f32, value, true)
            } else {
                (phase as f32, value, false)
            };
            if let Some((before, _)) = previous {
                ensure!(
                    phase > before,
                    "{name} expression point {} ({position}) falls at phase {phase:.4} of this note, not after the previous point at phase {before:.4}; order phase and time points so they increase on every note length",
                    index + 1
                );
            }
            ensure!(
                count < POINT_COUNT,
                "at most 32 expression points per note after time positions are resolved"
            );
            count += 1;
            visit(name, phase, value as f32)?;
            previous = Some((phase, value));
            if last {
                break;
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
        visit_points(value, None, |_, _, _| Ok(()))
    }
    pub fn parse(value: Option<&serde_json::Value>) -> Result<Self> {
        Self::parse_with_schema(value, &Schema::standard())
    }
    /// Parse phase-only curves; time positions need [`Self::parse_for_note`].
    pub fn parse_with_schema(value: Option<&serde_json::Value>, schema: &Schema) -> Result<Self> {
        Self::parse_for_note(value, schema, &|_| f64::NAN)
    }
    /// Parse curves for one performed note, resolving time positions to phases.
    pub fn parse_for_note(
        value: Option<&serde_json::Value>,
        schema: &Schema,
        resolve: Resolver,
    ) -> Result<Self> {
        let mut program = Self::default();
        visit_points(value, Some(resolve), |name, phase, value| {
            let kind = schema.resolve(name)?;
            if let Some((lo, hi)) = schema.bounds(kind) {
                ensure!(
                    (lo..=hi).contains(&f64::from(value)),
                    "{name} expression must be {lo}..{hi}"
                );
            }
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
            json!({"mute": [[-0.1, 0]]}),
            json!({"mute": [[0, 0, 0]]}),
            json!({"": 0}),
        ] {
            assert!(Program::validate_syntax(Some(&invalid)).is_err());
        }
        // Custom ranges belong to the destination: native controls are 0..1.
        Program::validate_syntax(Some(&json!({"mute": 1.1}))).unwrap();
        assert!(Program::parse_with_schema(Some(&json!({"mute": 1.1})), &schema).is_err());
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
            json!([{"kind": 0, "phase": 0, "value": 5}]),
        ] {
            assert!(serde_json::from_value::<Program>(invalid).is_err());
        }
        // A custom slot's range is the destination's: plain plugin units pass the
        // program, native 0..1 controls reject them.
        let plain: Program =
            serde_json::from_value(json!([{"kind": 31, "phase": 0, "value": 2}])).unwrap();
        assert!(schema.validate_program(&plain).is_err());
        let names: Vec<String> = (0..25).map(|i| format!("c{i:02}")).collect();
        Schema::plugin(names.iter().map(String::as_str))
            .unwrap()
            .validate_program(&plain)
            .unwrap();
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
        let mut program = Program {
            len: 3,
            ..Program::default()
        };
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
