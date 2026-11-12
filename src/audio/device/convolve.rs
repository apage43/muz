//! Convolution with an impulse response: a body or bell (tens of milliseconds)
//! or a room (seconds). Zero latency: the first `block` taps run in direct form
//! and the rest as uniformly partitioned overlap-save FFT products, each block's
//! tail ready before the block plays.
use super::*;

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}
impl Complex {
    fn mul_add(&mut self, a: Complex, b: Complex) {
        self.re += a.re * b.re - a.im * b.im;
        self.im += a.re * b.im + a.im * b.re;
    }
}

/// An in-place radix-2 complex FFT of one power-of-two size.
struct Fft {
    size: usize,
    twiddles: Vec<Complex>,
    reversed: Vec<usize>,
}
impl Fft {
    fn new(size: usize) -> Self {
        let bits = size.trailing_zeros();
        Self {
            size,
            twiddles: (0..size / 2)
                .map(|k| {
                    let a = -std::f64::consts::TAU * k as f64 / size as f64;
                    Complex {
                        re: a.cos() as f32,
                        im: a.sin() as f32,
                    }
                })
                .collect(),
            reversed: (0..size)
                .map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (usize::BITS - bits) })
                .collect(),
        }
    }
    /// Forward transform, or inverse (unscaled) with `inverse`.
    fn run(&self, x: &mut [Complex], inverse: bool) {
        for i in 0..self.size {
            let j = self.reversed[i];
            if i < j {
                x.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= self.size {
            let step = self.size / len;
            for start in (0..self.size).step_by(len) {
                for k in 0..len / 2 {
                    let mut w = self.twiddles[k * step];
                    if inverse {
                        w.im = -w.im;
                    }
                    let a = x[start + k];
                    let b = x[start + k + len / 2];
                    let t = Complex {
                        re: b.re * w.re - b.im * w.im,
                        im: b.re * w.im + b.im * w.re,
                    };
                    x[start + k] = Complex {
                        re: a.re + t.re,
                        im: a.im + t.im,
                    };
                    x[start + k + len / 2] = Complex {
                        re: a.re - t.re,
                        im: a.im - t.im,
                    };
                }
            }
            len *= 2;
        }
    }
}

/// A dot product in eight independent lanes (vectorizable).
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0f32; 8];
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let (ra, rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        for k in 0..8 {
            lanes[k] += x[k] * y[k];
        }
    }
    lanes.iter().sum::<f32>() + ra.iter().zip(rb).map(|(x, y)| x * y).sum::<f32>()
}

/// One channel's convolution with one impulse response.
struct Channel {
    head: Vec<f32>,
    history: Vec<f32>,
    at: usize,
    /// Spectra (bins 0..=block) of the tail's partitions, each `block` taps.
    parts: Vec<Vec<Complex>>,
    /// Spectra of the last `parts.len()` input windows, newest at `newest`.
    inputs: Vec<Vec<Complex>>,
    newest: usize,
    window: Vec<f32>,
    tail: Vec<f32>,
    sum: Vec<Complex>,
    scratch: Vec<Complex>,
}
impl Channel {
    fn new(ir: &[f32], block: usize, fft: &Fft) -> Self {
        let head = ir[..ir.len().min(block)].to_vec();
        let bins = block + 1;
        let parts: Vec<Vec<Complex>> = if ir.len() > block {
            ir[block..]
                .chunks(block)
                .map(|taps| {
                    let mut x = vec![Complex::default(); 2 * block];
                    for (slot, tap) in x.iter_mut().zip(taps) {
                        slot.re = *tap;
                    }
                    fft.run(&mut x, false);
                    x.truncate(bins);
                    x
                })
                .collect()
        } else {
            Vec::new()
        };
        // The head reversed, against a history written twice over (at `at` and
        // `at + n`): each sample's direct sum is one contiguous dot product.
        let mut head = head;
        head.reverse();
        Self {
            history: vec![0.; 2 * head.len().max(1)],
            head,
            at: 0,
            inputs: vec![vec![Complex::default(); bins]; parts.len()],
            parts,
            newest: 0,
            window: vec![0.; 2 * block],
            tail: vec![0.; block],
            sum: vec![Complex::default(); bins],
            scratch: vec![Complex::default(); 2 * block],
        }
    }
    fn clear(&mut self) {
        self.history.fill(0.);
        self.window.fill(0.);
        self.tail.fill(0.);
        self.inputs
            .iter_mut()
            .for_each(|x| x.fill(Complex::default()));
    }
    /// One sample at position `pos` of the current block.
    fn step(&mut self, x: f32, pos: usize) -> f32 {
        let n = self.history.len() / 2;
        self.at = (self.at + 1) % n;
        self.history[self.at] = x;
        self.history[self.at + n] = x;
        // history[at + 1 ..= at + n] runs oldest to newest; the head is reversed.
        let recent = &self.history[self.at + 1..=self.at + n];
        let y = self.tail[pos] + dot(&self.head, recent);
        if !self.parts.is_empty() {
            self.window[self.tail.len() + pos] = x;
        }
        y
    }
    /// The block is complete: the tail of the next block's output.
    fn block_done(&mut self, fft: &Fft) {
        if self.parts.is_empty() {
            return;
        }
        let block = self.tail.len();
        for (slot, v) in self.scratch.iter_mut().zip(&self.window) {
            *slot = Complex { re: *v, im: 0. };
        }
        fft.run(&mut self.scratch, false);
        let count = self.parts.len();
        self.newest = (self.newest + 1) % count;
        self.inputs[self.newest].copy_from_slice(&self.scratch[..=block]);
        self.sum.fill(Complex::default());
        for (p, part) in self.parts.iter().enumerate() {
            let input = &self.inputs[(self.newest + count - p) % count];
            for ((s, a), b) in self.sum.iter_mut().zip(part).zip(input) {
                s.mul_add(*a, *b);
            }
        }
        // A real signal's spectrum: the upper half mirrors the lower.
        self.scratch[..=block].copy_from_slice(&self.sum);
        for k in 1..block {
            let c = self.sum[k];
            self.scratch[2 * block - k] = Complex { re: c.re, im: -c.im };
        }
        fft.run(&mut self.scratch, true);
        let scale = 1. / (2 * block) as f32;
        for (t, v) in self.tail.iter_mut().zip(&self.scratch[block..]) {
            *t = v.re * scale;
        }
        self.window.copy_within(block.., 0);
    }
}

/// The block for `taps`: about twice their square root (the direct head's cost
/// balanced against the partitions'), a power of two from 64 to 1024. A
/// response that fits in one block is all head.
fn block_for(taps: usize) -> usize {
    if taps <= 128 {
        return taps.next_power_of_two().max(1);
    }
    let ideal = 2. * (taps as f64).sqrt();
    (1usize << ideal.log2().floor() as u32).clamp(64, 1024)
}

pub(super) struct Convolve {
    core: ProcessorCore,
    fft: Fft,
    channels: [Channel; 2],
    block: usize,
    pos: usize,
    mix: f32,
    gain: f32,
}
impl Convolve {
    pub fn new(d: &model::Device, c: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let impulse = d
            .impulse
            .as_ref()
            .ok_or(DeviceError::InvalidConfig("convolve needs an impulse response"))?;
        let frames: Vec<[f32; 2]> = match &impulse.path {
            Some(path) => {
                let (info, audio) =
                    crate::audio_file::load_shared(std::path::Path::new(path), 10 * 192_000)
                        .map_err(|e| {
                            eprintln!("convolve {}: {e:#}", d.id);
                            DeviceError::InvalidConfig("cannot load the impulse response")
                        })?;
                resample(&audio, info.rate as f64, c.sample_rate as f64)
            }
            None => resample(&impulse.frames, impulse.rate, c.sample_rate as f64),
        };
        if frames.is_empty() {
            return Err(DeviceError::InvalidConfig("an empty impulse response"));
        }
        let block = block_for(frames.len());
        let fft = Fft::new(2 * block);
        let side = |k: usize| frames.iter().map(|f| f[k]).collect::<Vec<f32>>();
        let channels = [
            Channel::new(&side(0), block, &fft),
            Channel::new(&side(1), block, &fft),
        ];
        let mut processor = Self {
            core: ProcessorCore::new(d.kind, token, c.max_frames),
            fft,
            channels,
            block,
            pos: 0,
            mix: 1.,
            gain: 1.,
        };
        for (name, value) in &d.params {
            processor.set_parameter(name, *value)?;
        }
        Ok(processor)
    }
}

/// `frames` at `from` Hz, linearly interpolated to `to` Hz. Each tap scales by
/// `from / to` so the response's gain holds when its tap count changes.
fn resample(frames: &[[f32; 2]], from: f64, to: f64) -> Vec<[f32; 2]> {
    if frames.is_empty() || (from - to).abs() < 1e-9 {
        return frames.to_vec();
    }
    let count = ((frames.len() as f64) * to / from).ceil() as usize;
    let scale = (from / to) as f32;
    (0..count)
        .map(|n| {
            let t = n as f64 * from / to;
            let i = t.floor() as usize;
            let f = (t - i as f64) as f32;
            let a = frames[i.min(frames.len() - 1)];
            let b = frames.get(i + 1).copied().unwrap_or([0.; 2]);
            [
                (a[0] + (b[0] - a[0]) * f) * scale,
                (a[1] + (b[1] - a[1]) * f) * scale,
            ]
        })
        .collect()
}

impl DeviceProcessor for Convolve {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }
    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "mix" => {
                self.mix = parameter_value(self.kind(), "mix", value, 0.0, 1.0)?;
                Ok(())
            }
            "gain_db" => {
                self.gain = db_to_amplitude(parameter_value(
                    self.kind(),
                    "gain_db",
                    value,
                    -120.0,
                    24.0,
                )?);
                Ok(())
            }
            _ => Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
    }
    fn reset(&mut self) {
        self.pos = 0;
        for c in &mut self.channels {
            c.clear();
        }
    }
    fn process(
        &mut self,
        ctx: ProcessContext,
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        let (dry, wet) = (1. - self.mix, self.mix * self.gain);
        for frame in 0..ctx.frames {
            let l = self.channels[0].step(left[frame], self.pos);
            let r = self.channels[1].step(right[frame], self.pos);
            left[frame] = left[frame] * dry + l * wet;
            right[frame] = right[frame] * dry + r * wet;
            self.pos += 1;
            if self.pos == self.block {
                self.pos = 0;
                for c in &mut self.channels {
                    c.block_done(&self.fft);
                }
            }
        }
        Ok(())
    }
}
