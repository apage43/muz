//! Prepared piecewise-linear transfer table and its exact integral for first-order ADAA.
use anyhow::{Result, ensure};
const SIZE: usize = 2049;
pub(super) struct Shape {
    values: Vec<f64>,
    integral: Vec<f64>,
    low: f64,
    step: f64,
    pub antialias: bool,
}
impl Shape {
    pub fn prepare(points: &[[f64; 2]], quality: Option<&str>) -> Result<Self> {
        let points: Vec<_> = points.iter().map(|p| (p[0], p[1])).collect();
        let low = points[0].0;
        let step = (points.last().unwrap().0 - low) / (SIZE - 1) as f64;
        ensure!(step > 1e-12, "shape domain is too small");
        let mut values = Vec::with_capacity(SIZE);
        let mut index = 0;
        for i in 0..SIZE {
            let x = low + i as f64 * step;
            while index + 2 < points.len() && x > points[index + 1].0 {
                index += 1;
            }
            let (a, b) = (points[index], points[index + 1]);
            values.push(a.1 + (b.1 - a.1) * ((x - a.0) / (b.0 - a.0)).clamp(0., 1.));
        }
        let mut integral = vec![0.; SIZE];
        for i in 1..SIZE {
            integral[i] = integral[i - 1] + (values[i - 1] + values[i]) * 0.5 * step;
        }
        let antialias = match quality.unwrap_or("adaa") {
            "adaa" => true,
            "raw" => false,
            _ => anyhow::bail!("shape quality must be adaa or raw"),
        };
        Ok(Self {
            values,
            integral,
            low,
            step,
            antialias,
        })
    }
    fn at(&self, x: f64) -> (usize, f64) {
        let p = ((x - self.low) / self.step).clamp(0., (SIZE - 1) as f64);
        let i = (p as usize).min(SIZE - 2);
        (i, p - i as f64)
    }
    pub fn value(&self, x: f64) -> f64 {
        let (i, f) = self.at(x);
        self.values[i] + f * (self.values[i + 1] - self.values[i])
    }
    fn integral(&self, x: f64) -> f64 {
        let high = self.low + (SIZE - 1) as f64 * self.step;
        if x < self.low {
            return (x - self.low) * self.values[0];
        }
        if x > high {
            return self.integral[SIZE - 1] + (x - high) * self.values[SIZE - 1];
        }
        let (i, f) = self.at(x);
        self.integral[i]
            + self.step * (self.values[i] * f + 0.5 * (self.values[i + 1] - self.values[i]) * f * f)
    }
    pub fn process(&self, x: f64, previous: f64) -> f64 {
        let dx = x - previous;
        if !self.antialias {
            self.value(x)
        } else if dx.abs() < 1e-8 {
            self.value((x + previous) * 0.5)
        } else {
            (self.integral(x) - self.integral(previous)) / dx
        }
    }
}
