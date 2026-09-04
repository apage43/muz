use super::eval::{Evaluator, Quantity, Unit, Value};
use crate::music::{rational, real};
use anyhow::{Result, bail};
use std::collections::BTreeMap;
struct Curve {
    points: Vec<(f64, f64)>,
    shape: String,
    clock: bool,
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
            points.push((time, p[1].number()?));
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
pub fn combine(e: &mut Evaluator, a: &Value, b: &Value, op: &str, step: f64) -> Result<Value> {
    let a = Curve::read(a)?;
    let bcurve = if op == "curve_map" {
        None
    } else {
        Some(Curve::read(b)?)
    };
    if bcurve.as_ref().is_some_and(|b| b.clock != a.clock) {
        bail!("curve arithmetic cannot mix seconds and beats");
    }
    let end = a
        .points
        .last()
        .unwrap()
        .0
        .max(bcurve.as_ref().map_or(0., |b| b.points.last().unwrap().0));
    if step <= 0. || !step.is_finite() || end / step > 100_000. {
        bail!("control resolution exceeds 100000 points");
    }
    let mut times = a.points.iter().map(|p| p.0).collect::<Vec<_>>();
    if let Some(b) = &bcurve {
        times.extend(b.points.iter().map(|p| p.0));
    }
    for curve in std::iter::once(&a).chain(bcurve.iter()) {
        if curve.shape == "step" {
            for p in curve.points.iter().skip(1) {
                times.push((p.0 - 1e-6).max(0.));
            }
        }
    }
    times.extend((0..=(end / step).ceil() as usize).map(|i| (i as f64 * step).min(end)));
    times.sort_by(f64::total_cmp);
    times.dedup();
    let mut points = Vec::with_capacity(times.len());
    for t in times {
        let x = a.at(t);
        let v = if let Some(b) = &bcurve {
            Value::num(if op == "curve_add" {
                x + b.at(t)
            } else {
                x * b.at(t)
            })
        } else {
            e.call(b.clone(), vec![(None, Value::num(x))])?
        };
        v.number()?;
        points.push(Value::Array(vec![
            Value::Num(Quantity {
                value: rational(t)?,
                unit: if a.clock { Unit::Seconds } else { Unit::Beat },
            }),
            v,
        ]));
    }
    Ok(Value::Record(BTreeMap::from([
        ("points".into(), Value::Array(points)),
        ("shape".into(), Value::Str("linear".into())),
    ])))
}
