//! Focused production kernels. EQ coefficients follow the RBJ / W3C Audio EQ Cookbook.
use super::*;
use std::f64::consts::TAU;
pub(super) struct Eq {
    core: ProcessorCore,
    rate: f64,
    frequency: f64,
    gain: f64,
    q: f64,
    mode: u8,
    c: [f64; 5],
    state: [[f64; 2]; 2],
}
impl Eq {
    pub(super) fn new(d: &model::Device, c: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut s = Self {
            core: ProcessorCore::new(d.kind, token, c.max_frames),
            rate: c.sample_rate as f64,
            frequency: 1000.0,
            gain: 0.0,
            q: 0.707,
            mode: 0,
            c: [1.0, 0.0, 0.0, 0.0, 0.0],
            state: [[0.0; 2]; 2],
        };
        for (k, v) in &d.params {
            s.set_parameter(k, *v)?;
        }
        s.coefficients();
        Ok(s)
    }
    fn coefficients(&mut self) {
        let w = TAU * self.frequency.min(self.rate * 0.45) / self.rate;
        let (c, s) = (w.cos(), w.sin());
        let a = 10f64.powf(self.gain / 40.0);
        let alpha = s / (2.0 * self.q);
        let beta = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match self.mode {
            1 => (
                a * ((a + 1.0) - (a - 1.0) * c + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - beta),
                (a + 1.0) + (a - 1.0) * c + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - beta,
            ),
            2 => (
                a * ((a + 1.0) + (a - 1.0) * c + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - beta),
                (a + 1.0) - (a - 1.0) * c + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - beta,
            ),
            _ => (
                1.0 + alpha * a,
                -2.0 * c,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * c,
                1.0 - alpha / a,
            ),
        };
        self.c = [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0];
    }
}
impl DeviceProcessor for Eq {
    /// Verified by the engine segmentation test.
    fn block_invariant(&self) -> bool {
        true
    }
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }
    fn set_parameter(&mut self, name: &str, v: f32) -> Result<(), DeviceError> {
        match name {
            "frequency_hz" => {
                self.frequency =
                    parameter_value(self.kind(), "frequency_hz", v, 20.0, 20000.0)? as f64
            }
            "gain_db" => {
                self.gain = parameter_value(self.kind(), "gain_db", v, -24.0, 24.0)? as f64
            }
            "q" => self.q = parameter_value(self.kind(), "q", v, 0.1, 12.0)? as f64,
            "mode" => {
                if v.fract() != 0.0 {
                    return Err(DeviceError::UnknownParameter { kind: self.kind() });
                }
                self.mode = parameter_value(self.kind(), "mode", v, 0.0, 2.0)? as u8;
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        self.coefficients();
        Ok(())
    }
    fn reset(&mut self) {
        self.state = [[0.0; 2]; 2];
    }
    fn process(
        &mut self,
        ctx: ProcessContext,
        _: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core.begin_process(ctx.frames, l.len(), r.len())?;
        for i in 0..ctx.frames {
            for (ch, x) in [(&mut l[i]), (&mut r[i])].into_iter().enumerate() {
                let y = self.c[0] * *x as f64 + self.state[ch][0];
                self.state[ch][0] = self.c[1] * *x as f64 - self.c[3] * y + self.state[ch][1];
                self.state[ch][1] = self.c[2] * *x as f64 - self.c[4] * y;
                *x = y as f32;
            }
        }
        Ok(())
    }
}
pub(super) struct Chorus {
    core: ProcessorCore,
    rate: f32,
    delay: Vec<[f32; 2]>,
    cursor: usize,
    hz: f32,
    depth: f32,
    mix: f32,
    /// The centre of the sweep, ms (a latency control).
    centre: f32,
    /// Offset of the sine, cycles.
    phase: f64,
    /// Whether the dry path waits for the centre delay, which is reported.
    compensate: bool,
    /// The sine's cycles are `anchor_cycles + hz * (t - anchor_seconds)` at
    /// song time t: the song's, not the device's, until the rate changes.
    anchor_seconds: f64,
    anchor_cycles: f64,
    /// Song time of the last frame processed.
    last_seconds: f64,
}
impl Chorus {
    pub(super) fn new(d: &model::Device, c: AudioConfig, t: u64) -> Result<Self, DeviceError> {
        let mut s = Self {
            core: ProcessorCore::new(d.kind, t, c.max_frames),
            rate: c.sample_rate,
            delay: vec![[0.0; 2]; (c.sample_rate * 0.06).ceil() as usize + 2],
            cursor: 0,
            hz: 0.4,
            depth: 5.0,
            mix: 0.3,
            centre: 18.0,
            phase: 0.0,
            compensate: false,
            anchor_seconds: 0.0,
            anchor_cycles: 0.0,
            last_seconds: 0.0,
        };
        for (k, v) in &d.params {
            s.set_parameter(k, *v)?;
        }
        Ok(s)
    }
    fn cycles_at(&self, seconds: f64) -> f64 {
        self.anchor_cycles + self.hz as f64 * (seconds - self.anchor_seconds)
    }
    /// The dry path's delay in whole samples.
    fn latency(&self) -> usize {
        if self.compensate {
            (self.centre * 0.001 * self.rate).round() as usize
        } else {
            0
        }
    }
}
impl DeviceProcessor for Chorus {
    /// Verified by the engine segmentation test.
    fn block_invariant(&self) -> bool {
        true
    }
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            latency_samples: self.latency() as u32,
            ..self.core.debug_state()
        }
    }
    fn set_parameter(&mut self, n: &str, v: f32) -> Result<(), DeviceError> {
        match n {
            "rate_hz" => {
                let hz = parameter_value(self.kind(), "rate_hz", v, 0.01, 10.0)?;
                // Keep the sine continuous: the new rate runs from the last frame.
                self.anchor_cycles = self.cycles_at(self.last_seconds);
                self.anchor_seconds = self.last_seconds;
                self.hz = hz;
            }
            "depth_ms" => self.depth = parameter_value(self.kind(), "depth_ms", v, 0.0, 15.0)?,
            "mix" => self.mix = parameter_value(self.kind(), "mix", v, 0.0, 1.0)?,
            "delay_ms" => self.centre = parameter_value(self.kind(), "delay_ms", v, 0.0, 25.0)?,
            "phase" => self.phase = parameter_value(self.kind(), "phase", v, 0.0, 1.0)? as f64,
            "compensate" => {
                self.compensate = parameter_value(self.kind(), "compensate", v, 0.0, 1.0)? >= 0.5
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }
    fn reset(&mut self) {
        self.delay.fill([0.0; 2]);
        self.cursor = 0;
        self.anchor_seconds = 0.0;
        self.anchor_cycles = 0.0;
        self.last_seconds = 0.0;
    }
    fn process(
        &mut self,
        ctx: ProcessContext,
        _: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core.begin_process(ctx.frames, l.len(), r.len())?;
        let len = self.delay.len();
        let latency = self.latency();
        for i in 0..ctx.frames {
            let seconds = (ctx.transport.project_frame + i as f64) / ctx.transport.sample_rate;
            let phase = ((self.cycles_at(seconds) + self.phase).fract() * TAU) as f32;
            self.last_seconds = seconds;
            let input = [l[i], r[i]];
            self.delay[self.cursor] = input;
            let dry = self.delay[(self.cursor + len - latency) % len];
            let mut out = [0.0; 2];
            for ch in 0..2 {
                let ms = (self.centre + (phase + ch as f32 * 1.5707964).sin() * self.depth).max(0.0);
                // Between the frames `whole` and `whole + 1` back, read
                // apart from where the ring's cursor happens to be.
                let back = ms * 0.001 * self.rate;
                let whole = back.floor();
                let f = back - whole;
                let newer = self.delay[(self.cursor + len - whole as usize) % len][ch];
                let older = self.delay[(self.cursor + 2 * len - whole as usize - 1) % len][ch];
                let wet = newer * (1.0 - f) + older * f;
                out[ch] = dry[ch] * (1.0 - self.mix) + wet * self.mix;
            }
            l[i] = out[0];
            r[i] = out[1];
            self.cursor = (self.cursor + 1) % len;
        }
        Ok(())
    }
}
pub(super) struct Gate {
    core: ProcessorCore,
    rate: f32,
    threshold: f32,
    ratio: f32,
    attack: f32,
    release: f32,
    envelope: f32,
    gain: f32,
}
impl Gate {
    pub(super) fn new(d: &model::Device, c: AudioConfig, t: u64) -> Result<Self, DeviceError> {
        let mut s = Self {
            core: ProcessorCore::new(d.kind, t, c.max_frames),
            rate: c.sample_rate,
            threshold: -40.0,
            ratio: 4.0,
            attack: 2.0,
            release: 120.0,
            envelope: 0.0,
            gain: 0.0,
        };
        for (k, v) in &d.params {
            s.set_parameter(k, *v)?;
        }
        Ok(s)
    }
}
impl DeviceProcessor for Gate {
    /// Verified by the engine segmentation test.
    fn block_invariant(&self) -> bool {
        true
    }
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            gain_reduction_db: -20.0 * self.gain.max(1e-6).log10(),
            ..self.core.debug_state()
        }
    }
    fn set_parameter(&mut self, n: &str, v: f32) -> Result<(), DeviceError> {
        match n {
            "threshold_db" => {
                self.threshold = parameter_value(self.kind(), "threshold_db", v, -80.0, 0.0)?
            }
            "ratio" => self.ratio = parameter_value(self.kind(), "ratio", v, 1.0, 20.0)?,
            "attack_ms" => self.attack = parameter_value(self.kind(), "attack_ms", v, 0.1, 100.0)?,
            "release_ms" => {
                self.release = parameter_value(self.kind(), "release_ms", v, 1.0, 2000.0)?
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }
    fn reset(&mut self) {
        self.envelope = 0.0;
        self.gain = 0.0;
    }
    fn process(
        &mut self,
        ctx: ProcessContext,
        _: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core.begin_process(ctx.frames, l.len(), r.len())?;
        let ac = (-1.0 / (self.attack * 0.001 * self.rate)).exp();
        let rc = (-1.0 / (self.release * 0.001 * self.rate)).exp();
        for i in 0..ctx.frames {
            let level = l[i].abs().max(r[i].abs());
            let c = if level > self.envelope { ac } else { rc };
            self.envelope = level + c * (self.envelope - level);
            let db = 20.0 * self.envelope.max(1e-12).log10();
            let target = 10f32
                .powf(((db - self.threshold).min(0.0) * (self.ratio - 1.0)).max(-100.0) / 20.0);
            let c = if target > self.gain { ac } else { rc };
            self.gain = target + c * (self.gain - target);
            l[i] *= self.gain;
            r[i] *= self.gain;
        }
        Ok(())
    }
}
