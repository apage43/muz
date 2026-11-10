//! Streaming 4x true-peak estimate, shared by the limiter and `muz analyze`.
//!
//! Each input sample is followed by three interpolated points at 1/4, 1/2 and
//! 3/4 of the sample period, as in ITU-R BS.1770-4 Annex 2. The interpolator is
//! a Kaiser-windowed sinc, 16 taps per phase, normalized to unity DC gain per
//! phase; phase 0 is the input sample itself, so the estimate never reads below
//! the sample peak. A 4x grid under-reads a continuous peak that falls between
//! grid points: by up to 0.69 dB for a full-scale tone at Nyquist, about 0.5 dB at
//! 20 kHz and 0.2 dB at a quarter of the sample rate (48 kHz material).

/// Taps per interpolation phase.
const TAPS: usize = 16;
/// Kaiser window shape; trades passband droop near Nyquist against ripple.
const BETA: f64 = 5.0;

/// Samples between the newest input and the start of the interval the
/// estimate returned by [`TruePeakInterpolator::push`] covers.
pub const TRUE_PEAK_DELAY: usize = TAPS / 2;

#[derive(Clone, Debug)]
pub struct TruePeakInterpolator {
    coefficients: [[f32; TAPS]; 3],
    /// Two copies of the last `TAPS` samples so the window is one slice.
    history: [f32; 2 * TAPS],
    cursor: usize,
}

impl Default for TruePeakInterpolator {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeakInterpolator {
    pub fn new() -> Self {
        let half = (TAPS / 2) as f64;
        let mut coefficients = [[0.0f32; TAPS]; 3];
        for (phase, row) in coefficients.iter_mut().enumerate() {
            let fraction = (phase + 1) as f64 / 4.0;
            // Tap `j` multiplies sample `a - (TAPS/2 - 1) + j`, where the
            // interpolated point lies `fraction` after sample `a`.
            let mut taps = [0.0f64; TAPS];
            for (j, tap) in taps.iter_mut().enumerate() {
                let offset = j as f64 - (half - 1.0);
                let t = fraction - offset;
                let ratio = t / half;
                *tap = sinc(t) * bessel_i0(BETA * (1.0 - ratio * ratio).max(0.0).sqrt())
                    / bessel_i0(BETA);
            }
            let sum: f64 = taps.iter().sum();
            for (out, tap) in row.iter_mut().zip(taps) {
                *out = (tap / sum) as f32;
            }
        }
        Self {
            coefficients,
            history: [0.0; 2 * TAPS],
            cursor: 0,
        }
    }

    pub fn reset(&mut self) {
        self.history = [0.0; 2 * TAPS];
        self.cursor = 0;
    }

    /// Accept sample `n` and return the peak magnitude over `[n - D, n - D + 1)`,
    /// `D` = [`TRUE_PEAK_DELAY`]: the sample `n - D` and the three points after it.
    pub fn push(&mut self, sample: f32) -> f32 {
        self.history[self.cursor] = sample;
        self.history[self.cursor + TAPS] = sample;
        self.cursor = (self.cursor + 1) % TAPS;
        let window = &self.history[self.cursor..self.cursor + TAPS];
        let mut peak = window[TAPS / 2 - 1].abs();
        for row in &self.coefficients {
            let value: f32 = row.iter().zip(window).map(|(c, x)| c * x).sum();
            peak = peak.max(value.abs());
        }
        peak
    }
}

/// Call `visit(index, peak)` for every sample of one channel, where `peak` is
/// the estimate over `[index, index + 1)`; the signal is zero outside `samples`.
pub fn for_each_true_peak(
    samples: impl IntoIterator<Item = f32>,
    mut visit: impl FnMut(usize, f32),
) {
    let mut interpolator = TruePeakInterpolator::new();
    let padded = samples
        .into_iter()
        .chain(std::iter::repeat_n(0.0, TRUE_PEAK_DELAY));
    for (count, sample) in padded.enumerate() {
        let peak = interpolator.push(sample);
        if count >= TRUE_PEAK_DELAY {
            visit(count - TRUE_PEAK_DELAY, peak);
        }
    }
}

fn sinc(t: f64) -> f64 {
    if t.abs() < 1.0e-12 {
        1.0
    } else {
        let x = std::f64::consts::PI * t;
        x.sin() / x
    }
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1.0e-12 * sum {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

#[cfg(test)]
#[path = "true_peak_tests.rs"]
mod tests;
