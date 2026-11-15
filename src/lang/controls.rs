use super::eval::{Number, Quantity, Unit, Value};
use crate::music::real;
use anyhow::{Result, bail};
struct Curve {
    points: Vec<(f64, f64)>,
    shape: String,
    clock: bool,
    unit: Unit,
}
impl Curve {
    fn read(v: &Value) -> Result<Self> {
        let r = v.record()?;
        let ps = r
            .get("points")
            .ok_or_else(|| anyhow::anyhow!("curve needs points"))?
            .array()?;
        let mut points = Vec::new();
        let mut clock = None;
        let mut unit = None;
        for p in ps {
            let p = p.array()?;
            if p.len() != 2 {
                bail!("curve points need position and value");
            }
            let seconds = matches!(&p[0],Value::Num(q) if q.unit==Unit::Seconds);
            if clock.is_some_and(|c| c != seconds) {
                bail!("curve arithmetic requires one time unit");
            }
            clock = Some(seconds);
            // Curves are sampled in floating point, so positions keep their
            // float value: rounding them to the exact score-time grid would
            // merge points closer than 1/TIME_GRID beat.
            let time = match &p[0] {
                _ if seconds => p[0].number()?,
                Value::Num(q) if matches!(q.value, Number::Inexact(_)) => match q.unit {
                    Unit::Scalar | Unit::Beat => q.number(),
                    Unit::Bar => q.number() * 4.,
                    _ => real(p[0].beats()?),
                },
                _ => real(p[0].beats()?),
            };
            if time < 0. {
                bail!("curve positions cannot be negative (got {time})");
            }
            if let Some((last, _)) = points.last()
                && *last >= time
            {
                bail!("curve points must increase (position {time} follows {last})");
            }
            let Value::Num(q) = &p[1] else {
                bail!("curve value must be numeric")
            };
            let (dimension, value) = if q.unit == Unit::Bar {
                (Unit::Beat, q.number() * 4.)
            } else {
                (q.unit, q.number())
            };
            if unit.is_some_and(|u| u != dimension) {
                bail!("curve values must have compatible units");
            }
            unit = Some(dimension);
            points.push((time, value));
        }
        if points.is_empty() {
            bail!("empty curve");
        }
        let shape = r
            .get("shape")
            .map(Value::text)
            .transpose()?
            .unwrap_or("linear")
            .to_owned();
        Ok(Self {
            points,
            shape,
            clock: clock.unwrap_or(false),
            unit: unit.unwrap_or(Unit::Scalar),
        })
    }
    fn at(&self, t: f64) -> f64 {
        let i = self.points.partition_point(|p| p.0 <= t).saturating_sub(1);
        let (a, x) = self.points[i];
        if let Some(&(b, y)) = self.points.get(i + 1) {
            if self.shape == "step" || t <= a {
                return x;
            }
            let u = ((t - a) / (b - a)).clamp(0., 1.);
            let u = if self.shape == "smooth" {
                u * u * (3. - 2. * u)
            } else {
                u
            };
            x + (y - x) * u
        } else {
            x
        }
    }
}
impl Curve {
    /// The area under the curve from position 0 to `t`, in value × position
    /// units (beats, or seconds for a curve in seconds). The curve holds its
    /// first value before its first point and its last after its last.
    fn integral(&self, t: f64) -> f64 {
        let (first, v0) = self.points[0];
        if t <= first {
            return v0 * t;
        }
        let mut area = v0 * first;
        for (i, &(a, x)) in self.points.iter().enumerate() {
            let Some(&(b, y)) = self.points.get(i + 1) else {
                return area + x * (t - a);
            };
            let u = ((t - a) / (b - a)).clamp(0., 1.);
            let segment = match self.shape.as_str() {
                "step" => x * u,
                "smooth" => x * u + (y - x) * (u * u * u - u * u * u * u / 2.),
                _ => x * u + (y - x) * u * u / 2.,
            } * (b - a);
            area += segment;
            if t <= b {
                return area;
            }
        }
        area
    }
}
/// The area under a curve from position 0 to one position or a bounded array
/// of positions (see [`Curve::integral`]); a plain number. Integrating a rate
/// gives a phase: cycles, for a curve of Hz in seconds.
pub fn integral(curve: &Value, positions: &Value) -> Result<Value> {
    let curve = Curve::read(curve)?;
    let sample = |position: &Value| -> Result<Value> {
        let time = if curve.clock {
            if !matches!(position, Value::Num(q) if matches!(q.unit, Unit::Seconds | Unit::Scalar))
            {
                bail!("curve position must be seconds");
            }
            position.number()?
        } else {
            real(position.beats()?)
        };
        Ok(Value::Num(Quantity {
            unit: Unit::Scalar,
            value: Number::finite(curve.integral(time))?,
        }))
    };
    if let Value::Array(positions) = positions {
        if positions.len() > 200000 {
            return Err(crate::diagnostic::Diagnostic::new("curve evaluation exceeds point budget").limit().err());
        }
        Ok(Value::Array(
            positions.iter().map(sample).collect::<Result<_>>()?,
        ))
    } else {
        sample(positions)
    }
}
/// Evaluate a validated curve at one position or a bounded array of positions.
/// Point selection and resampling policy belong to source.
pub fn value(curve: &Value, positions: &Value) -> Result<Value> {
    let curve = Curve::read(curve)?;
    let sample = |position: &Value| -> Result<Value> {
        let time = if curve.clock {
            if !matches!(position, Value::Num(q) if matches!(q.unit, Unit::Seconds | Unit::Scalar))
            {
                bail!("curve position must be seconds");
            }
            position.number()?
        } else {
            real(position.beats()?)
        };
        Ok(Value::Num(Quantity {
            unit: curve.unit,
            value: Number::finite(curve.at(time))?,
        }))
    };
    if let Value::Array(positions) = positions {
        if positions.len() > 200000 {
            return Err(crate::diagnostic::Diagnostic::new("curve evaluation exceeds point budget").limit().err());
        }
        Ok(Value::Array(
            positions.iter().map(sample).collect::<Result<_>>()?,
        ))
    } else {
        sample(positions)
    }
}
