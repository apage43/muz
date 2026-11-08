//! Prepared, allocation-free parameter curves and compensation delay lines.
use crate::compile::{Automation, CurvePoint};

/// Interpolation between two curve points, parsed once at preparation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    Linear,
    Step,
    Smooth,
}

impl Shape {
    /// Unknown shape names interpolate linearly, as the source contract allows.
    fn parse(name: &str) -> Self {
        match name {
            "step" => Self::Step,
            "smooth" => Self::Smooth,
            _ => Self::Linear,
        }
    }
}

/// A prepared automation lane. Audio reads it with nondecreasing times, so a
/// cursor replaces the per-sample binary search; earlier times (seeks, loops)
/// fall back to the search. Values are identical to a stateless lookup.
#[derive(Clone, Debug)]
pub(super) struct Lane {
    pub target: String,
    points: Vec<CurvePoint>,
    shape: Shape,
    cursor: usize,
    /// Last `10^(value/20)` conversion, keyed by the value's bits.
    amplitude: Option<(u32, f32)>,
}

impl Lane {
    pub fn new(automation: &Automation) -> Self {
        Self {
            target: automation.target.clone(),
            points: automation.points.clone(),
            shape: Shape::parse(&automation.shape),
            cursor: 0,
            amplitude: None,
        }
    }

    /// Index of the last point at or before `seconds`, or 0 before the first.
    fn index(&mut self, seconds: f64) -> usize {
        let points = &self.points;
        let mut i = self.cursor.min(points.len() - 1);
        // Earlier times and NaN take the stateless search.
        if i > 0 && (points[i].seconds > seconds || seconds.is_nan()) {
            i = points
                .partition_point(|p| p.seconds <= seconds)
                .saturating_sub(1);
        } else if points.get(i + 1).is_some_and(|p| p.seconds <= seconds) {
            i += 1;
            if points.get(i + 1).is_some_and(|p| p.seconds <= seconds) {
                i += points[i + 1..].partition_point(|p| p.seconds <= seconds);
            }
        }
        self.cursor = i;
        i
    }

    pub fn value_at(&mut self, seconds: f64) -> f32 {
        let i = self.index(seconds);
        interpolate(&self.points, i, self.shape, seconds)
    }

    /// The lane's decibel value at `seconds` as linear amplitude.
    pub fn amplitude_at(&mut self, seconds: f64) -> f32 {
        let value = self.value_at(seconds);
        match self.amplitude {
            Some((bits, amplitude)) if bits == value.to_bits() => amplitude,
            _ => {
                let amplitude = 10.0f32.powf(value / 20.0);
                self.amplitude = Some((value.to_bits(), amplitude));
                amplitude
            }
        }
    }
}

/// The curve value at `seconds`, from the last point at or before it.
fn interpolate(points: &[CurvePoint], i: usize, shape: Shape, seconds: f64) -> f32 {
    let p = &points[i];
    let Some(next) = points.get(i + 1) else {
        return p.value;
    };
    if shape == Shape::Step || seconds <= p.seconds {
        return p.value;
    }
    let mut t = ((seconds - p.seconds) / (next.seconds - p.seconds)).clamp(0.0, 1.0) as f32;
    if shape == Shape::Smooth {
        t = t * t * (3.0 - 2.0 * t);
    }
    p.value + (next.value - p.value) * t
}

impl Automation {
    /// Stateless lookup for inspection and tests; audio uses a prepared lane.
    pub fn value_at(&self, seconds: f64) -> f32 {
        let i = self
            .points
            .partition_point(|p| p.seconds <= seconds)
            .saturating_sub(1);
        interpolate(&self.points, i, Shape::parse(&self.shape), seconds)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The string-shaped, binary-search lookup the lane replaced.
    fn reference(a: &Automation, seconds: f64) -> f32 {
        let i = a
            .points
            .partition_point(|p| p.seconds <= seconds)
            .saturating_sub(1);
        let p = &a.points[i];
        let Some(next) = a.points.get(i + 1) else {
            return p.value;
        };
        if a.shape == "step" || seconds <= p.seconds {
            return p.value;
        }
        let mut t = ((seconds - p.seconds) / (next.seconds - p.seconds)).clamp(0.0, 1.0) as f32;
        if a.shape == "smooth" {
            t = t * t * (3.0 - 2.0 * t);
        }
        p.value + (next.value - p.value) * t
    }

    #[test]
    fn cursor_lookup_matches_binary_search_reference() {
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 11) as f64 / (1u64 << 53) as f64
        };
        for shape in ["step", "linear", "smooth", "other"] {
            for count in [1, 2, 7, 200] {
                let mut at = 0.25;
                let points = (0..count)
                    .map(|i| {
                        at += 0.001 + next() * 0.05;
                        CurvePoint {
                            seconds: if i == 0 { 0.5 } else { at },
                            value: (next() * 40.0 - 20.0) as f32,
                        }
                    })
                    .collect();
                let automation = Automation {
                    target: "t.p".into(),
                    points,
                    shape: shape.into(),
                    origin: None,
                };
                let mut lane = Lane::new(&automation);
                let end = at + 0.5;
                let mut t = 0.0;
                let mut times = Vec::new();
                // Per-sample reads, exact point times, long forward jumps,
                // backward seeks and NaN.
                while t < end {
                    times.push(t);
                    t += if next() < 0.02 {
                        next() * 0.3
                    } else {
                        1.0 / 48000.0
                    };
                    if next() < 0.002 {
                        t = next() * end;
                    }
                }
                times.extend(automation.points.iter().map(|p| p.seconds));
                times.extend([f64::NAN, 0.0, f64::INFINITY, -1.0]);
                for t in times {
                    assert_eq!(
                        lane.value_at(t).to_bits(),
                        reference(&automation, t).to_bits(),
                        "{shape} with {count} points at {t}"
                    );
                    assert_eq!(
                        lane.amplitude_at(t).to_bits(),
                        10.0f32.powf(reference(&automation, t) / 20.0).to_bits()
                    );
                }
            }
        }
    }
}
