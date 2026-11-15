//! A rotary speaker: a crossover feeding a spinning horn (highs) and a
//! spinning drum (lows), each heard by two microphones. Each rotor's angle
//! moves the sound's path length (a Doppler pitch sweep), its level (the horn
//! facing a microphone is loudest) and so its place between the microphones.
//! Rotor speeds follow `speed` (0 chorale, 1 tremolo) at a fixed acceleration.
use super::*;

/// One second-order section (transposed direct form II), per channel.
#[derive(Clone, Copy, Default)]
struct Biquad {
    c: [f64; 5],
    s: [f64; 2],
}
impl Biquad {
    /// A Butterworth (Q = 1/√2) lowpass or highpass at `hz`.
    fn butterworth(hz: f64, rate: f64, high: bool) -> Self {
        let w = std::f64::consts::TAU * hz.min(rate * 0.45) / rate;
        let (cos, sin) = (w.cos(), w.sin());
        let alpha = sin / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high {
            ((1.0 + cos) / 2.0, -(1.0 + cos))
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos)
        };
        Self {
            c: [b0 / a0, b1 / a0, b0 / a0, -2.0 * cos / a0, (1.0 - alpha) / a0],
            s: [0.0; 2],
        }
    }
    fn run(&mut self, x: f64) -> f64 {
        let y = self.c[0] * x + self.s[0];
        self.s[0] = self.c[1] * x - self.c[3] * y + self.s[1];
        self.s[1] = self.c[2] * x - self.c[4] * y;
        y
    }
}

/// One rotor and the band it plays.
struct Rotor {
    /// Fourth-order Linkwitz–Riley half of the crossover: two Butterworths.
    filter: [Biquad; 2],
    slow: f64,
    fast: f64,
    /// Rate change per second, Hz/s.
    acceleration: f64,
    /// Peak path-length swing, seconds.
    depth: f64,
    /// Level swing, dB peak to peak.
    level_db: f64,
    rate: f64,
    /// Angle in cycles: 0 faces the listener, between the microphones.
    angle: f64,
    line: Vec<f32>,
}
impl Rotor {
    fn target(&self, speed: f64) -> f64 {
        self.slow + (self.fast - self.slow) * speed
    }
}

pub(super) struct Rotary {
    core: ProcessorCore,
    rate: f64,
    horn: Rotor,
    drum: Rotor,
    speed: f64,
    /// Each microphone's angle from the front, cycles (left −, right +).
    spread: f64,
    mix: f32,
    /// The fixed path every rotor's swing is centred on, in samples: the
    /// device's reported latency. The dry path is delayed to match.
    centre: f64,
    dry: Vec<[f32; 2]>,
    cursor: usize,
}

const PARAMS: &[&str] = &[
    "speed",
    "horn_slow_hz",
    "horn_fast_hz",
    "drum_slow_hz",
    "drum_fast_hz",
    "horn_ramp_s",
    "drum_ramp_s",
    "split_hz",
    "horn_depth_ms",
    "drum_depth_ms",
    "horn_level_db",
    "drum_level_db",
    "spread",
    "mix",
];

impl Rotary {
    pub(super) fn new(d: &model::Device, c: AudioConfig, t: u64) -> Result<Self, DeviceError> {
        let rate = c.sample_rate as f64;
        let get = |name: &str, default: f32| -> Result<f64, DeviceError> {
            let spec = crate::description::parameter_specs(d.kind)
                .iter()
                .find(|s| s.name == name)
                .ok_or(DeviceError::UnknownParameter { kind: d.kind })?;
            let v = d.params.get(name).copied().unwrap_or(default);
            Ok(parameter_value(d.kind, spec.name, v, spec.min, spec.max)? as f64)
        };
        for name in d.params.keys() {
            if !PARAMS.contains(&name.as_str()) {
                return Err(DeviceError::UnknownParameter { kind: d.kind });
            }
        }
        let split = get("split_hz", 800.0)?;
        // The longest swing the controls allow, plus room to interpolate.
        let centre = (MAX_DEPTH_MS * 0.001 * rate).ceil() + 2.0;
        let len = (2.0 * centre) as usize + 4;
        let rotor = |high: bool, slow: f64, fast: f64, ramp: f64, depth_ms: f64, level_db: f64| Rotor {
            filter: [Biquad::butterworth(split, rate, high); 2],
            slow,
            fast,
            acceleration: (fast - slow).abs().max(1e-6) / ramp,
            depth: depth_ms * 0.001,
            level_db,
            rate: slow,
            angle: 0.0,
            line: vec![0.0; len],
        };
        let speed = get("speed", 0.0)?;
        let mut s = Self {
            core: ProcessorCore::new(d.kind, t, c.max_frames),
            rate,
            horn: rotor(
                true,
                get("horn_slow_hz", 0.8)?,
                get("horn_fast_hz", 6.7)?,
                get("horn_ramp_s", 1.0)?,
                get("horn_depth_ms", 0.17)?,
                get("horn_level_db", 3.5)?,
            ),
            drum: rotor(
                false,
                get("drum_slow_hz", 0.7)?,
                get("drum_fast_hz", 5.8)?,
                get("drum_ramp_s", 4.0)?,
                get("drum_depth_ms", 0.1)?,
                get("drum_level_db", 1.5)?,
            ),
            speed,
            spread: get("spread", 0.125)?,
            mix: get("mix", 1.0)? as f32,
            centre,
            dry: vec![[0.0; 2]; centre as usize],
            cursor: 0,
        };
        // Playback starts with each rotor already at its speed.
        s.horn.rate = s.horn.target(speed);
        s.drum.rate = s.drum.target(speed);
        Ok(s)
    }
}

/// Upper bound of `horn_depth_ms` and `drum_depth_ms`.
pub(super) const MAX_DEPTH_MS: f64 = 2.0;

/// A four-point Hermite read `delay` samples behind `write` in a ring.
fn read(line: &[f32], write: usize, delay: f64) -> f32 {
    let len = line.len();
    let whole = delay.floor();
    let f = (delay - whole) as f32;
    let at = |k: isize| line[(write as isize - whole as isize + k).rem_euclid(len as isize) as usize];
    // Newest first: at(1) is one sample newer than at(0).
    let (y0, y1, y2, y3) = (at(1), at(0), at(-1), at(-2));
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * f + c2) * f + c1) * f + y1
}

impl DeviceProcessor for Rotary {
    /// Verified by the engine segmentation test.
    fn block_invariant(&self) -> bool {
        true
    }
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            latency_samples: self.centre as u32,
            ..self.core.debug_state()
        }
    }
    fn set_parameter(&mut self, n: &str, v: f32) -> Result<(), DeviceError> {
        match n {
            "speed" => self.speed = parameter_value(self.kind(), "speed", v, 0.0, 1.0)? as f64,
            "mix" => self.mix = parameter_value(self.kind(), "mix", v, 0.0, 1.0)?,
            name if PARAMS.contains(&name) => {
                return Err(DeviceError::InvalidConfig(
                    "a rotary speaker's rotors and crossover are set when it is built; automate `speed` and `mix`",
                ));
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }
    fn reset(&mut self) {
        for r in [&mut self.horn, &mut self.drum] {
            r.filter.iter_mut().for_each(|f| f.s = [0.0; 2]);
            r.line.fill(0.0);
            r.angle = 0.0;
            r.rate = r.target(self.speed);
        }
        self.dry.fill([0.0; 2]);
        self.cursor = 0;
    }
    fn process(
        &mut self,
        ctx: ProcessContext,
        _: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core.begin_process(ctx.frames, l.len(), r.len())?;
        let tau = std::f64::consts::TAU;
        let mics = [-self.spread, self.spread];
        let len = self.horn.line.len();
        for i in 0..ctx.frames {
            let input = 0.5 * (l[i] as f64 + r[i] as f64);
            let mut wet = [0.0f32; 2];
            for rotor in [&mut self.horn, &mut self.drum] {
                let first = rotor.filter[0].run(input);
                let band = rotor.filter[1].run(first);
                rotor.line[self.cursor] = band as f32;
                for (ch, mic) in mics.iter().enumerate() {
                    // Facing this microphone: the shortest path and the most level.
                    let facing = (tau * (rotor.angle - mic)).cos();
                    let delay = self.centre - facing * rotor.depth * self.rate;
                    let level = 10f64.powf(rotor.level_db * 0.5 * (facing - 1.0) / 20.0);
                    wet[ch] += read(&rotor.line, self.cursor, delay) * level as f32;
                }
                let target = rotor.target(self.speed);
                let step = rotor.acceleration / self.rate;
                rotor.rate += (target - rotor.rate).clamp(-step, step);
                rotor.angle = (rotor.angle + rotor.rate / self.rate).fract();
            }
            let d = self.cursor % self.dry.len();
            let dry = std::mem::replace(&mut self.dry[d], [l[i], r[i]]);
            l[i] = dry[0] * (1.0 - self.mix) + wet[0] * self.mix;
            r[i] = dry[1] * (1.0 - self.mix) + wet[1] * self.mix;
            self.cursor = (self.cursor + 1) % len;
        }
        Ok(())
    }
}
