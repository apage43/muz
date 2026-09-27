//! Bounded, deterministic instruments and spatial processing; no callback allocation.
use super::*;
use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};

#[derive(Clone, Copy)]
struct StudioVoice {
    active: bool,
    id: u64,
    key: u8,
    velocity: f32,
    frequency: f32,
    note_volume: f32,
    note_expression: f32,
    note_tuning: f32,
    note_pan: [f32; 2],
    phase: [f32; 5],
    sub: f32,
    fm: f32,
    age: u64,
    released: bool,
    choked: bool,
    envelope: f32,
    low: [f32; 2],
    band: [f32; 2],
    noise_lp: f32,
    air_lp: f32,
}
impl StudioVoice {
    const EMPTY: Self = Self {
        active: false,
        id: 0,
        key: 0,
        velocity: 0.0,
        frequency: 0.0,
        note_volume: 1.0,
        note_expression: 1.0,
        note_tuning: 1.0,
        note_pan: [1.0; 2],
        phase: [0.0; 5],
        sub: 0.0,
        fm: 0.0,
        age: 0,
        released: false,
        choked: false,
        envelope: 0.0,
        low: [0.0; 2],
        band: [0.0; 2],
        noise_lp: 0.0,
        air_lp: 0.0,
    };
}

pub(super) struct StudioSynth {
    core: ProcessorCore,
    rate: f32,
    voices: [StudioVoice; 16],
    rng: u32,
    mode: u8,
    gain: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cutoff: f32,
    resonance: f32,
    filter_env: f32,
    detune: [f32; 5],
    unison: usize,
    width: f32,
    sub: f32,
    fm_ratio: f32,
    fm_index: f32,
    vibrato_cents: f32,
    vibrato_hz: f32,
    drive: f32,
    expression: f32,
    expression_target: f32,
    brightness: f32,
    brightness_target: f32,
}
impl StudioSynth {
    pub(super) fn new(d: &model::Device, c: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut s = Self {
            core: ProcessorCore::new(d.kind, token, c.max_frames),
            rate: c.sample_rate,
            voices: [StudioVoice::EMPTY; 16],
            rng: 0x6d2b79f5,
            mode: 0,
            gain: 0.2,
            attack: 0.005,
            decay: 0.2,
            sustain: 0.65,
            release: 0.2,
            cutoff: 4000.0,
            resonance: 0.15,
            filter_env: 2.0,
            detune: [1.0; 5],
            unison: 1,
            width: 0.7,
            sub: 0.0,
            fm_ratio: 2.0,
            fm_index: 2.0,
            vibrato_cents: 0.0,
            vibrato_hz: 5.5,
            drive: 1.0,
            expression: 1.0,
            expression_target: 1.0,
            brightness: 1.0,
            brightness_target: 1.0,
        };
        for spec in crate::source::parameter_specs(d.kind) {
            s.set_parameter(spec.name, spec.default)?;
        }
        for (k, v) in &d.params {
            s.set_parameter(k, *v)?;
        }
        Ok(s)
    }
    fn event(&mut self, e: DeviceEventKind) {
        match e {
            DeviceEventKind::NoteOn {
                pitch,
                note_id,
                key,
                velocity,
                ..
            } => {
                let i = self
                    .voices
                    .iter()
                    .position(|v| !v.active)
                    .unwrap_or_else(|| {
                        self.voices
                            .iter()
                            .enumerate()
                            .min_by(|(_, a), (_, b)| a.envelope.total_cmp(&b.envelope))
                            .unwrap()
                            .0
                    });
                let mut v = StudioVoice {
                    active: true,
                    id: note_id,
                    key,
                    velocity,
                    frequency: 440.0 * 2.0_f32.powf((pitch - 69.0) / 12.0),
                    ..StudioVoice::EMPTY
                };
                if self.mode < 3 {
                    for (j, p) in v.phase.iter_mut().enumerate() {
                        *p = ((note_id.wrapping_mul(17).wrapping_add(j as u64 * 29)) % 101) as f32
                            / 101.0;
                    }
                }
                self.voices[i] = v;
            }
            DeviceEventKind::NoteExpression {
                note_id,
                expression,
                value,
                ..
            } => {
                for v in &mut self.voices {
                    if v.active && v.id == note_id {
                        match expression {
                            0 => v.note_volume = value as f32,
                            1 => {
                                let pan = value as f32;
                                v.note_pan = [(2.0 * (1.0 - pan)).sqrt(), (2.0 * pan).sqrt()];
                            }
                            2 => v.note_tuning = 2.0_f32.powf(value as f32 / 12.0),
                            4 => v.note_expression = value as f32,
                            _ => {}
                        }
                    }
                }
            }
            DeviceEventKind::NoteOff { note_id, key, .. } => {
                for v in &mut self.voices {
                    if v.id == note_id && v.key == key {
                        v.released = true;
                    }
                }
            }
            DeviceEventKind::NoteChoke { note_id, key, .. } => {
                for v in &mut self.voices {
                    if v.id == note_id && v.key == key {
                        v.choked = true;
                        v.released = true;
                    }
                }
            }
            DeviceEventKind::Controller {
                controller: 11,
                value,
                ..
            } => self.expression_target = value as f32 / 127.0,
            DeviceEventKind::Controller {
                controller: 74,
                value,
                ..
            } => self.brightness_target = 2.0_f32.powf((value as f32 - 64.0) / 24.0),
            DeviceEventKind::Controller {
                controller: 120, ..
            } => {
                for v in &mut self.voices {
                    v.choked = true;
                    v.released = true;
                }
            }
            DeviceEventKind::Flush => self.reset(),
            _ => {}
        }
    }
    fn sample(&mut self) -> [f32; 2] {
        self.expression += (self.expression_target - self.expression) * 0.003;
        self.brightness += (self.brightness_target - self.brightness) * 0.002;
        let mut output = [0.0; 2];
        for v in &mut self.voices {
            if !v.active {
                continue;
            }
            let t = v.age as f32 / self.rate;
            v.age += 1;
            let percussive = (3..=6).contains(&self.mode);
            if v.choked {
                v.envelope *= (-9.21 / (0.008 * self.rate)).exp();
            } else if percussive {
                v.envelope = (-t / self.decay).exp() * (t / 0.0007).min(1.0);
            } else if v.released {
                v.envelope *= (-6.9078 / (self.release * self.rate)).exp();
            } else {
                v.envelope = if t < self.attack {
                    t / self.attack.max(1.0 / self.rate)
                } else {
                    self.sustain + (1.0 - self.sustain) * (-(t - self.attack) / self.decay).exp()
                };
            }
            if (v.released || percussive) && v.envelope < 1e-5 && v.age > 2 {
                v.active = false;
                continue;
            }
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 17;
            self.rng ^= self.rng << 5;
            let noise = self.rng as i32 as f32 / 2147483648.0;
            v.noise_lp += 0.15 * (noise - v.noise_lp);
            v.air_lp += 0.63 * (noise - v.noise_lp - v.air_lp);
            let bright_noise = v.air_lp;
            let vibrato = self.vibrato_cents
                * ((t - 0.12) / 0.2).clamp(0.0, 1.0)
                * (TAU * self.vibrato_hz * t).sin();
            let frequency = if self.vibrato_cents == 0.0 {
                v.frequency * v.note_tuning
            } else {
                v.frequency * v.note_tuning * 2.0_f32.powf(vibrato / 1200.0)
            };
            let mut s = [0.0; 2];
            match self.mode {
                0 | 1 => {
                    for j in 0..self.unison {
                        let dt = (frequency
                            * self.detune[if self.unison == 1 {
                                2
                            } else {
                                j * 4 / (self.unison - 1)
                            }]
                            / self.rate)
                            .min(0.4);
                        let p = v.phase[j];
                        let x = if self.mode == 0 {
                            2.0 * p - 1.0 - blep(p, dt)
                        } else {
                            (if p < 0.5 { 1.0 } else { -1.0 }) + blep(p, dt)
                                - blep((p + 0.5).fract(), dt)
                        };
                        let pan = if self.unison == 1 {
                            0.0
                        } else {
                            (j as f32 / (self.unison - 1) as f32 * 2.0 - 1.0) * self.width
                        };
                        s[0] += x * (1.0 - pan).sqrt() * FRAC_1_SQRT_2 / self.unison as f32;
                        s[1] += x * (1.0 + pan).sqrt() * FRAC_1_SQRT_2 / self.unison as f32;
                        v.phase[j] = (p + dt).fract();
                    }
                    let sub = (v.sub * TAU).sin() * self.sub;
                    v.sub = (v.sub + v.frequency * v.note_tuning * 0.5 / self.rate).fract();
                    s[0] += sub;
                    s[1] += sub;
                }
                2 | 6 => {
                    let index = self.fm_index * (-t / self.decay.max(0.03)).exp();
                    let x = (v.phase[0] * TAU + index * (v.fm * TAU).sin()).sin();
                    v.phase[0] = (v.phase[0] + (frequency / self.rate).min(0.4)).fract();
                    v.fm = (v.fm + (frequency * self.fm_ratio / self.rate).min(0.4)).fract();
                    s = [x; 2];
                }
                _ => {
                    s = [noise, bright_noise];
                }
            }
            if self.mode < 3 || self.mode == 7 {
                // Topology-preserving state-variable lowpass, stable under cutoff sweeps.
                let env = (-t / self.decay).exp() * self.filter_env;
                let cutoff = (self.cutoff * self.brightness * 2.0_f32.powf(env))
                    .clamp(25.0, self.rate * 0.40);
                let g = (PI * cutoff / self.rate).tan();
                let k = 2.0 - 1.85 * self.resonance;
                let a = 1.0 / (1.0 + g * (g + k));
                for (ch, x) in s.iter_mut().enumerate() {
                    let input = (*x * self.drive).tanh() / self.drive.sqrt();
                    let b = a * (v.band[ch] + g * (input - v.low[ch]));
                    let l = v.low[ch] + g * b;
                    v.band[ch] = 2.0 * b - v.band[ch];
                    v.low[ch] = 2.0 * l - v.low[ch];
                    *x = l;
                }
            }
            if (4..=6).contains(&self.mode) && self.drive > 1.0 {
                for x in &mut s {
                    *x = (*x * self.drive).tanh() / self.drive.sqrt();
                }
            }
            for ch in 0..2 {
                output[ch] += s[ch]
                    * v.envelope
                    * v.velocity
                    * self.gain
                    * self.expression
                    * v.note_volume
                    * v.note_expression
                    * v.note_pan[ch];
            }
        }
        output
    }
}
fn blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

fn checked(kind: model::DeviceKind, name: &str, value: f32) -> Result<f32, DeviceError> {
    let spec = crate::source::parameter_specs(kind)
        .iter()
        .find(|s| s.name == name)
        .ok_or(DeviceError::UnknownParameter { kind })?;
    parameter_value(kind, spec.name, value, spec.min, spec.max)
}
impl DeviceProcessor for StudioSynth {
    fn has_note(&self, note_id: u64) -> bool {
        self.voices.iter().any(|v| v.active && v.id == note_id)
    }
    fn accepts_note_expression(&self, kind: u8) -> bool {
        matches!(kind, 0 | 1 | 2 | 4)
    }
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }
    fn set_parameter(&mut self, n: &str, v: f32) -> Result<(), DeviceError> {
        let v = checked(self.kind(), n, v)?;
        match n {
            "mode" => {
                if (3.0..=5.0).contains(&v) {
                    return Err(DeviceError::InvalidConfig(
                        "percussion modes 3..5 moved to source voice patches; use synth with named kick, snare, or cymbal mode",
                    ));
                }
                if v.fract() != 0.0 {
                    return Err(DeviceError::InvalidParameterValue {
                        kind: self.kind(),
                        parameter: "mode",
                    });
                }
                self.mode = v as u8;
            }
            "gain_db" => self.gain = db_to_amplitude(v),
            "attack_ms" => self.attack = v / 1000.0,
            "decay_ms" => self.decay = v / 1000.0,
            "sustain" => self.sustain = v,
            "release_ms" => self.release = v / 1000.0,
            "cutoff_hz" => self.cutoff = v,
            "resonance" => self.resonance = v,
            "filter_env" => self.filter_env = v,
            "detune_cents" => {
                for (i, d) in self.detune.iter_mut().enumerate() {
                    *d = 2.0_f32.powf((i as f32 - 2.0) * v / 2400.0);
                }
            }
            "unison" => {
                if v.fract() != 0.0 {
                    return Err(DeviceError::InvalidParameterValue {
                        kind: self.kind(),
                        parameter: "unison",
                    });
                }
                self.unison = v as usize;
            }
            "width" => self.width = v,
            "sub" => self.sub = v,
            "fm_ratio" => self.fm_ratio = v,
            "fm_index" => self.fm_index = v,
            "vibrato_cents" => self.vibrato_cents = v,
            "vibrato_hz" => self.vibrato_hz = v,
            "drive_db" => self.drive = db_to_amplitude(v),
            _ => unreachable!(),
        }
        Ok(())
    }
    fn reset(&mut self) {
        self.voices = [StudioVoice::EMPTY; 16];
        self.rng = 0x6d2b79f5;
        self.expression = 1.0;
        self.expression_target = 1.0;
        self.brightness = 1.0;
        self.brightness_target = 1.0;
    }
    fn process(
        &mut self,
        c: ProcessContext,
        e: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core.begin_process(c.frames, l.len(), r.len())?;
        let mut next = 0;
        for i in 0..c.frames {
            while next < e.len() && e[next].offset as usize <= i {
                self.event(e[next].kind);
                next += 1;
            }
            let s = self.sample();
            l[i] = s[0];
            r[i] = s[1];
        }
        Ok(())
    }
}

pub(super) struct Stereo {
    core: ProcessorCore,
    rate: f32,
    pan: f32,
    width: f32,
    duck: f32,
    period: f32,
}
impl Stereo {
    pub(super) fn new(d: &model::Device, c: AudioConfig, t: u64) -> Result<Self, DeviceError> {
        let mut s = Self {
            core: ProcessorCore::new(d.kind, t, c.max_frames),
            rate: c.sample_rate,
            pan: 0.0,
            width: 1.0,
            duck: 0.0,
            period: 1.0,
        };
        for (k, v) in &d.params {
            s.set_parameter(k, *v)?;
        }
        Ok(s)
    }
}
impl DeviceProcessor for Stereo {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }
    fn reset(&mut self) {}
    fn set_parameter(&mut self, n: &str, v: f32) -> Result<(), DeviceError> {
        let v = checked(self.kind(), n, v)?;
        match n {
            "pan" => self.pan = v,
            "width" => self.width = v,
            "duck" => self.duck = v,
            "period_beats" => self.period = v,
            _ => unreachable!(),
        }
        Ok(())
    }
    fn process(
        &mut self,
        c: ProcessContext,
        _: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core.begin_process(c.frames, l.len(), r.len())?;
        for i in 0..c.frames {
            let beat =
                c.transport.beat_position + i as f64 * c.transport.bpm / (60.0 * self.rate as f64);
            let phase = (beat / self.period as f64).rem_euclid(1.0) as f32;
            let dip = if phase < 0.025 {
                0.5 - 0.5 * (PI * phase / 0.025).cos()
            } else {
                (-(phase - 0.025) * 7.0).exp() * (1.0 - ((phase - 0.9) / 0.1).clamp(0.0, 1.0))
            };
            let gain = 1.0 - self.duck * if c.transport.running { dip } else { 0.0 };
            let mid = (l[i] + r[i]) * 0.5;
            let side = (l[i] - r[i]) * 0.5 * self.width;
            l[i] = (mid + side) * (1.0 - self.pan).sqrt() * gain;
            r[i] = (mid - side) * (1.0 + self.pan).sqrt() * gain;
        }
        Ok(())
    }
}

pub(super) struct Reverb {
    core: ProcessorCore,
    rate: f32,
    lines: [Vec<f32>; 8],
    cursors: [usize; 8],
    damp: [f32; 8],
    diffusers: [Vec<f32>; 4],
    diff_pos: [usize; 4],
    pre: Vec<f32>,
    pre_pos: usize,
    decay: f32,
    damping: f32,
    mix: f32,
    predelay: f32,
    feedback: [f32; 8],
    hp: f32,
}
impl Reverb {
    pub(super) fn new(d: &model::Device, c: AudioConfig, t: u64) -> Result<Self, DeviceError> {
        let mut s = Self {
            core: ProcessorCore::new(d.kind, t, c.max_frames),
            rate: c.sample_rate,
            lines: std::array::from_fn(|i| {
                vec![
                    0.0;
                    ([1423, 1559, 1747, 1999, 2137, 2411, 2683, 2953][i] as f32 * c.sample_rate
                        / 48000.0)
                        .round()
                        .max(1.0) as usize
                ]
            }),
            cursors: [0; 8],
            damp: [0.0; 8],
            diffusers: std::array::from_fn(|i| {
                vec![
                    0.0;
                    ([149, 211, 263, 293][i] as f32 * c.sample_rate / 48000.0)
                        .round()
                        .max(1.0) as usize
                ]
            }),
            diff_pos: [0; 4],
            pre: vec![0.0; (c.sample_rate * 0.201) as usize + 1],
            pre_pos: 0,
            decay: 2.0,
            damping: 6000.0,
            mix: 0.25,
            predelay: 0.02,
            feedback: [0.0; 8],
            hp: 0.0,
        };
        s.update();
        for (k, v) in &d.params {
            s.set_parameter(k, *v)?;
        }
        Ok(s)
    }
    fn update(&mut self) {
        for i in 0..8 {
            self.feedback[i] =
                (-6.9078 * self.lines[i].len() as f32 / (self.decay * self.rate)).exp();
        }
    }
}
impl DeviceProcessor for Reverb {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            tail_samples: (self.decay * self.rate) as u32,
            ..self.core.debug_state()
        }
    }
    fn set_parameter(&mut self, n: &str, v: f32) -> Result<(), DeviceError> {
        let v = checked(self.kind(), n, v)?;
        match n {
            "decay_s" => self.decay = v,
            "damping_hz" => self.damping = v,
            "mix" => self.mix = v,
            "predelay_ms" => self.predelay = v / 1000.0,
            _ => unreachable!(),
        }
        self.update();
        Ok(())
    }
    fn reset(&mut self) {
        for l in &mut self.lines {
            l.fill(0.0);
        }
        for l in &mut self.diffusers {
            l.fill(0.0);
        }
        self.cursors = [0; 8];
        self.diff_pos = [0; 4];
        self.damp = [0.0; 8];
        self.pre.fill(0.0);
        self.pre_pos = 0;
        self.hp = 0.0;
    }
    fn process(
        &mut self,
        c: ProcessContext,
        _: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core.begin_process(c.frames, l.len(), r.len())?;
        let damp = 1.0 - (-TAU * self.damping / self.rate).exp();
        let high = 1.0 - (-TAU * 180.0 / self.rate).exp();
        let pre_samples = (self.predelay * self.rate).round() as usize;
        for frame in 0..c.frames {
            let input = (l[frame] + r[frame]) * 0.5;
            self.hp += high * (input - self.hp);
            self.pre[self.pre_pos] = input - self.hp;
            let mut x = self.pre[(self.pre_pos + self.pre.len() - pre_samples) % self.pre.len()];
            self.pre_pos = (self.pre_pos + 1) % self.pre.len();
            for j in 0..4 {
                let d = self.diffusers[j][self.diff_pos[j]];
                let y = d - 0.6 * x;
                self.diffusers[j][self.diff_pos[j]] = x + 0.6 * y;
                self.diff_pos[j] = (self.diff_pos[j] + 1) % self.diffusers[j].len();
                x = y;
            }
            let taps: [f32; 8] = std::array::from_fn(|i| self.lines[i][self.cursors[i]]);
            let sum = taps.iter().sum::<f32>() * 0.25;
            for (i, tap) in taps.iter().enumerate() {
                self.damp[i] += damp * (*tap - sum - self.damp[i]);
                self.lines[i][self.cursors[i]] = x * 0.35 + self.damp[i] * self.feedback[i];
                self.cursors[i] = (self.cursors[i] + 1) % self.lines[i].len();
            }
            let wet_l = (taps[0] + taps[2] - taps[4] + taps[6]) * 0.5;
            let wet_r = (taps[1] + taps[3] - taps[5] + taps[7]) * 0.5;
            l[frame] = l[frame] * (1.0 - self.mix) + wet_l * self.mix;
            r[frame] = r[frame] * (1.0 - self.mix) + wet_r * self.mix;
        }
        Ok(())
    }
}
