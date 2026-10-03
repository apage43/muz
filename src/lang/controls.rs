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
            let time = if seconds {
                p[0].number()?
            } else {
                real(p[0].beats()?)
            };
            if time < 0. || points.last().is_some_and(|(t, _)| *t >= time) {
                bail!("curve points must increase");
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
