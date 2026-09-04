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
    phase: f32,
    hz: f32,
    depth: f32,
    mix: f32,
}
impl Chorus {
    pub(super) fn new(d: &model::Device, c: AudioConfig, t: u64) -> Result<Self, DeviceError> {
        let mut s = Self {
            core: ProcessorCore::new(d.kind, t, c.max_frames),
            rate: c.sample_rate,
            delay: vec![[0.0; 2]; (c.sample_rate * 0.06).ceil() as usize + 2],
            cursor: 0,
            phase: 0.0,
            hz: 0.4,
            depth: 5.0,
            mix: 0.3,
        };
        for (k, v) in &d.params {
            s.set_parameter(k, *v)?;
        }
        Ok(s)
    }
}
impl DeviceProcessor for Chorus {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }
    fn set_parameter(&mut self, n: &str, v: f32) -> Result<(), DeviceError> {
        match n {
            "rate_hz" => self.hz = parameter_value(self.kind(), "rate_hz", v, 0.01, 10.0)?,
            "depth_ms" => self.depth = parameter_value(self.kind(), "depth_ms", v, 0.0, 15.0)?,
            "mix" => self.mix = parameter_value(self.kind(), "mix", v, 0.0, 1.0)?,
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }
    fn reset(&mut self) {
        self.delay.fill([0.0; 2]);
        self.cursor = 0;
        self.phase = 0.0;
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
        for i in 0..ctx.frames {
            let input = [l[i], r[i]];
            self.delay[self.cursor] = input;
            let mut out = [0.0; 2];
            for ch in 0..2 {
                let ms = 18.0 + (self.phase + ch as f32 * 1.5707964).sin() * self.depth;
                let p = (self.cursor as f32 + len as f32 - ms * 0.001 * self.rate)
                    .rem_euclid(len as f32);
                let j = p.floor() as usize;
                let f = p - j as f32;
                let wet = self.delay[j][ch] * (1.0 - f) + self.delay[(j + 1) % len][ch] * f;
                out[ch] = input[ch] * (1.0 - self.mix) + wet * self.mix;
            }
            l[i] = out[0];
            r[i] = out[1];
            self.cursor = (self.cursor + 1) % len;
            self.phase =
                (self.phase + self.hz * std::f32::consts::TAU / self.rate) % std::f32::consts::TAU;
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
