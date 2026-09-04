//! Prepared, allocation-free parameter curves and compensation delay lines.
use crate::compile::Automation;
impl Automation {
    pub fn value_at(&self, seconds: f64) -> f32 {
        let i = self
            .points
            .partition_point(|p| p.seconds <= seconds)
            .saturating_sub(1);
        let p = &self.points[i];
        let Some(next) = self.points.get(i + 1) else {
            return p.value;
        };
        if self.shape == "step" || seconds <= p.seconds {
            return p.value;
        }
        let mut t = ((seconds - p.seconds) / (next.seconds - p.seconds)).clamp(0.0, 1.0) as f32;
        if self.shape == "smooth" {
            t = t * t * (3.0 - 2.0 * t);
        }
        p.value + (next.value - p.value) * t
    }
}
#[derive(Default)]
pub(super) struct DelayLine {
    samples: Vec<[f32; 2]>,
    position: usize,
}
impl DelayLine {
    pub fn set_length(&mut self, n: usize) {
        self.samples = vec![[0.0, 0.0]; n];
        self.position = 0;
    }
    pub fn sample(&mut self, left: f32, right: f32) -> [f32; 2] {
        if self.samples.is_empty() {
            return [left, right];
        }
        let out = self.samples[self.position];
        self.samples[self.position] = [left, right];
        self.position = (self.position + 1) % self.samples.len();
        out
    }
    pub fn reset(&mut self) {
        self.samples.fill([0.0, 0.0]);
        self.position = 0;
    }
}
