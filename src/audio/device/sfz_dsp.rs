//! Prepared SFZ signal processing. All strings, curves and routes are resolved off
//! the audio thread; per-voice state contains only fixed-size arrays and scalars.
//! Units follow sfzformat.com: envelope seconds/percent, pitch/filter cents,
//! volume/resonance/EQ dB, and normalized MIDI controller values.
use std::collections::BTreeMap;
use std::f32::consts::TAU;

type Ops = BTreeMap<String, String>;
/// Borrowed controller view: no 576-byte controller materialization per voice/frame.
struct ControllerSources<'a> {
    physical: &'a [f32; 128],
    virtual_sources: &'a [f32; 16],
}
impl std::ops::Index<usize> for ControllerSources<'_> {
    type Output = f32;
    #[inline]
    fn index(&self, i: usize) -> &f32 {
        if i < 128 {
            &self.physical[i]
        } else {
            &self.virtual_sources[i - 128]
        }
    }
}
#[derive(Clone, Debug)]
struct Route {
    cc: usize,
    amount: f32,
    curve: [f32; 128],
    raw: bool,
}
#[derive(Clone, Debug)]
struct Param {
    base: f32,
    routes: Vec<Route>,
}
impl Param {
    fn compile(
        ops: &Ops,
        name: &str,
        default: f32,
        curves: &BTreeMap<u16, Vec<f32>>,
    ) -> Result<Self, String> {
        let base = number(ops, name, default)?;
        let mut routes = Vec::new();
        for (k, v) in ops {
            let Some(rest) = k.strip_prefix(name) else {
                continue;
            };
            let cc = rest
                .strip_prefix("_oncc")
                .or_else(|| rest.strip_prefix("_cc"))
                .or_else(|| rest.strip_prefix("cc"));
            let Some(cc) = cc.and_then(|x| x.parse::<usize>().ok()) else {
                continue;
            };
            if cc >= 128 && ![131, 133, 135, 140].contains(&cc) {
                return Err(format!(
                    "{k}: extended controller semantics are not yet verified"
                ));
            }
            let amount = parse_num(k, v)?;
            let curve_id = number(ops, &format!("{name}_curvecc{cc}"), 0.)?;
            if curve_id.fract() != 0. || !(0. ..=255.).contains(&curve_id) {
                return Err(format!("{name}: invalid curve index"));
            }
            routes.push(Route {
                cc,
                amount,
                curve: curve_table(curve_id as u16, curves)?,
                raw: cc >= 128 && curve_id == 0.,
            });
        }
        Ok(Self { base, routes })
    }
    fn product(&self, cc: &impl std::ops::Index<usize, Output = f32>) -> f32 {
        self.base
            * self
                .routes
                .iter()
                .map(|r| {
                    r.amount / 100.
                        * if r.raw {
                            cc[r.cc]
                        } else {
                            lookup(&r.curve, cc[r.cc])
                        }
                })
                .product::<f32>()
    }
    fn get(&self, cc: &impl std::ops::Index<usize, Output = f32>) -> f32 {
        self.base
            + self
                .routes
                .iter()
                .map(|r| {
                    r.amount
                        * if r.raw {
                            cc[r.cc]
                        } else {
                            lookup(&r.curve, cc[r.cc])
                        }
                })
                .sum::<f32>()
    }
}
fn parse_num(k: &str, v: &str) -> Result<f32, String> {
    let x = v
        .parse::<f32>()
        .map_err(|_| format!("{k}: expected finite numeric value, got {v}"))?;
    if !x.is_finite() {
        return Err(format!("{k}: nonfinite value"));
    }
    Ok(x)
}
fn number(ops: &Ops, k: &str, d: f32) -> Result<f32, String> {
    ops.get(k).map(|v| parse_num(k, v)).unwrap_or(Ok(d))
}
fn key_number(ops: &Ops, k: &str, d: f32) -> Result<f32, String> {
    let Some(v) = ops.get(k) else { return Ok(d) };
    if let Ok(x) = v.parse::<f32>() {
        if x.is_finite() {
            return Ok(x);
        }
    }
    let lower = v.to_ascii_lowercase();
    let b = lower.as_bytes();
    let semitone = match b.first() {
        Some(b'c') => 0,
        Some(b'd') => 2,
        Some(b'e') => 4,
        Some(b'f') => 5,
        Some(b'g') => 7,
        Some(b'a') => 9,
        Some(b'b') => 11,
        _ => return Err(format!("{k}: invalid note {v}")),
    };
    let accidental = match b.get(1) {
        Some(b'#') => 1,
        Some(b'b') => -1,
        _ => 0,
    };
    let offset = if accidental == 0 { 1 } else { 2 };
    let octave = lower[offset..]
        .parse::<i32>()
        .map_err(|_| format!("{k}: invalid note {v}"))?;
    let key = (octave + 1) * 12 + semitone + accidental;
    if !(0..=127).contains(&key) {
        return Err(format!("{k}: note outside MIDI range"));
    }
    Ok(key as f32)
}
pub(super) fn curve_table(id: u16, curves: &BTreeMap<u16, Vec<f32>>) -> Result<[f32; 128], String> {
    if let Some(v) = curves.get(&id) {
        if v.len() != 128 || v.iter().any(|x| !x.is_finite()) {
            return Err(format!(
                "curve {id}: expected 128 finite interpolated points"
            ));
        }
        return Ok(std::array::from_fn(|i| v[i]));
    }
    if id <= 6 {
        return Ok(std::array::from_fn(|i| {
            let x = i as f32 / 127.;
            match id {
                0 => x,
                1 => 2. * x - 1.,
                2 => 1. - x,
                3 => 1. - 2. * x,
                4 => x * x,
                5 => x.sqrt(),
                6 => (1. - x).sqrt(),
                _ => unreachable!(),
            }
        }));
    }
    Err(format!(
        "curve {id}: no verified built-in or user definition"
    ))
}
fn lookup(table: &[f32; 128], x: f32) -> f32 {
    let p = x.clamp(0., 1.) * 127.;
    let i = p as usize;
    let j = (i + 1).min(127);
    table[i] + (table[j] - table[i]) * (p - i as f32)
}
fn db(x: f32) -> f32 {
    10f32.powf(x / 20.)
}

#[derive(Clone, Debug)]
struct Envelope {
    time: [Param; 5],
    sustain: Param,
    start: Param,
    vel: [f32; 6],
    shape: [f32; 3],
    dynamic: bool,
    legacy_decay: bool,
    depth: Param,
}
impl Envelope {
    fn compile(o: &Ops, p: &str, c: &BTreeMap<u16, Vec<f32>>) -> Result<Self, String> {
        let names = ["delay", "attack", "hold", "decay", "release"];
        let mut times = Vec::new();
        let mut vel = [0.; 6];
        for (i, n) in names.iter().enumerate() {
            times.push(Param::compile(
                o,
                &format!("{p}_{n}"),
                if *n == "release" { 0.001 } else { 0. },
                c,
            )?);
            vel[i] = number(o, &format!("{p}_vel2{n}"), 0.)?;
        }
        vel[5] = number(o, &format!("{p}_vel2sustain"), 0.)?;
        let dynamic = number(o, &format!("{p}_dynamic"), 0.)?;
        if dynamic != 0. && dynamic != 1. {
            return Err(format!("{p}_dynamic: expected 0 or 1"));
        }
        Ok(Self {
            time: times.try_into().unwrap(),
            sustain: Param::compile(o, &format!("{p}_sustain"), 100., c)?,
            start: Param::compile(o, &format!("{p}_start"), 0., c)?,
            vel,
            shape: [
                number(o, &format!("{p}_attack_shape"), 0.)?,
                number(o, &format!("{p}_decay_shape"), -10.3616)?,
                number(o, &format!("{p}_release_shape"), -10.3616)?,
            ],
            dynamic: dynamic == 1.,
            legacy_decay: !o.contains_key(&format!("{p}_decay_shape")),
            depth: Param::compile(o, &format!("{p}_depth"), 0., c)?,
        })
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct EnvState {
    stage: u8,
    elapsed: f64,
    value: f32,
    origin: f32,
    times: [f32; 5],
    sustain: f32,
    initialized: bool,
}
fn shaped(p: f32, s: f32) -> f32 {
    if s.abs() < 1e-5 {
        p
    } else if s > 0. {
        // Equivalent normalized exponential with nonpositive arguments; large
        // valid curvature values cannot produce inf/inf on the callback.
        ((s * (p - 1.)).exp() - (-s).exp()) / (-(-s).exp_m1())
    } else {
        (s * p).exp_m1() / s.exp_m1()
    }
}
impl EnvState {
    fn update_parameters(
        &mut self,
        e: &Envelope,
        cc: &impl std::ops::Index<usize, Output = f32>,
        vel: f32,
    ) {
        for i in 0..5 {
            self.times[i] = (e.time[i].get(cc) + e.vel[i] * vel).max(0.);
        }
        self.sustain = (e.sustain.get(cc) + e.vel[5] * vel).clamp(0., 100.) / 100.;
    }
    fn latch(&mut self, e: &Envelope, cc: &impl std::ops::Index<usize, Output = f32>, vel: f32) {
        self.update_parameters(e, cc, vel);
        self.value = e.start.get(cc).clamp(0., 100.) / 100.;
        self.origin = self.value;
        self.initialized = true;
    }
    fn release(&mut self) {
        if self.stage < 5 {
            self.stage = 5;
            self.elapsed = 0.;
            self.origin = self.value;
        }
    }
    fn step(
        &mut self,
        e: &Envelope,
        cc: &impl std::ops::Index<usize, Output = f32>,
        vel: f32,
        rate: f64,
        controls_changed: bool,
    ) -> f32 {
        if !self.initialized {
            self.latch(e, cc, vel)
        } else if e.dynamic && controls_changed {
            self.update_parameters(e, cc, vel)
        }
        // At most six instantaneous stage transitions, regardless of sample rate.
        for _ in 0..6 {
            if self.stage == 4 {
                self.value = self.sustain;
                // A zero-sustain SFZ envelope is free-running. Do not retain a
                // silent looping voice until the host eventually sends note-off.
                if self.sustain <= 0. {
                    self.stage = 5;
                    self.elapsed = 0.;
                    self.origin = 0.;
                    continue;
                }
                return self.value;
            }
            if self.stage == 6 {
                self.value = 0.;
                return 0.;
            }
            let idx = match self.stage {
                0 => 0,
                1 => 1,
                2 => 2,
                3 => 3,
                5 => 4,
                _ => unreachable!(),
            };
            let duration = self.times[idx] as f64;
            if duration <= self.elapsed + 1e-9 {
                self.value = match self.stage {
                    0 => self.value,
                    1 | 2 => 1.,
                    3 => self.sustain,
                    5 => 0.,
                    _ => 0.,
                };
                self.stage += 1;
                self.elapsed = 0.;
                self.origin = self.value;
                continue;
            }
            self.elapsed = (self.elapsed + 1. / rate).min(duration);
            let p = (self.elapsed / duration) as f32;
            self.value = match self.stage {
                0 => self.origin,
                1 => self.origin + (1. - self.origin) * shaped(p, e.shape[0]),
                2 => 1.,
                // SFZ1 decay time describes the fullscale-to-zero exponential
                // rate, not a fixed-duration interpolation to sustain. Verified
                // against sfizz 1.2.3 ADSREnvelope.cpp and synthetic decay render.
                3 if e.legacy_decay => (-9. * p).exp().max(self.sustain),
                3 => 1. + (self.sustain - 1.) * shaped(p, e.shape[1]),
                5 => self.origin * (1. - shaped(p, e.shape[2])),
                _ => 0.,
            };
            if self.stage == 3 && e.legacy_decay && self.value <= self.sustain {
                self.stage = 4;
            }
            return self.value;
        }
        self.value
    }
}
#[derive(Clone, Debug)]
struct Lfo {
    enabled: bool,
    freq: Param,
    delay: Param,
    fade: Param,
    phase: Param,
    pitch: Param,
    volume: Param,
    cutoff: Param,
    wave: u8,
    cross: Vec<(usize, Param)>,
    eq_freq: [Param; 3],
    eq_gain: [Param; 3],
    secondary: Option<(u8, f32, f32, f32)>,
}
impl Lfo {
    fn compile(
        o: &Ops,
        p: &str,
        c: &BTreeMap<u16, Vec<f32>>,
        legacy: bool,
    ) -> Result<Self, String> {
        let wave = number(o, &format!("{p}_wave"), if legacy { 1. } else { 0. })?;
        if wave.fract() != 0. || !(0. ..=7.).contains(&wave) {
            return Err(format!("{p}_wave: unsupported waveform {wave}"));
        }
        let secondary = if o.contains_key(&format!("{p}_wave2")) {
            let w = number(o, &format!("{p}_wave2"), 1.)?;
            if w.fract() != 0. || !(0. ..=7.).contains(&w) {
                return Err(format!("{p}_wave2: unsupported waveform {w}"));
            }
            Some((
                w as u8,
                number(o, &format!("{p}_ratio2"), 1.)?,
                number(o, &format!("{p}_scale2"), 1.)?,
                number(o, &format!("{p}_offset2"), 0.)?,
            ))
        } else {
            None
        };
        let mut cross = Vec::new();
        for source in 1..=3 {
            let field = format!("{p}_freq_lfo{source:02}");
            if o.keys()
                .any(|k| k == &field || k.starts_with(&format!("{field}_")))
            {
                cross.push((source - 1, Param::compile(o, &field, 0., c)?));
            }
        }
        let mut eq_freq = Vec::new();
        let mut eq_gain = Vec::new();
        for n in 1..=3 {
            eq_freq.push(Param::compile(o, &format!("{p}_eq{n}freq"), 0., c)?);
            eq_gain.push(Param::compile(o, &format!("{p}_eq{n}gain"), 0., c)?);
        }
        Ok(Self {
            enabled: o.keys().any(|k| k.starts_with(&format!("{p}_"))),
            cross,
            eq_freq: eq_freq.try_into().unwrap(),
            eq_gain: eq_gain.try_into().unwrap(),
            freq: Param::compile(o, &format!("{p}_freq"), 0., c)?,
            delay: Param::compile(o, &format!("{p}_delay"), 0., c)?,
            fade: Param::compile(o, &format!("{p}_fade"), 0., c)?,
            phase: Param::compile(o, &format!("{p}_phase"), 0., c)?,
            pitch: Param::compile(
                o,
                &format!("{p}_{}", if p == "pitchlfo" { "depth" } else { "pitch" }),
                0.,
                c,
            )?,
            volume: Param::compile(
                o,
                &format!("{p}_{}", if p == "amplfo" { "depth" } else { "volume" }),
                0.,
                c,
            )?,
            cutoff: Param::compile(
                o,
                &format!("{p}_{}", if p == "fillfo" { "depth" } else { "cutoff" }),
                0.,
                c,
            )?,
            wave: wave as u8,
            secondary,
        })
    }
}
fn wave(w: u8, p: f64) -> f32 {
    let p = p.rem_euclid(1.) as f32;
    match w {
        0 => {
            if p < 0.25 {
                4. * p
            } else if p < 0.75 {
                2. - 4. * p
            } else {
                4. * p - 4.
            }
        }
        1 => (TAU * p).sin(),
        2 => {
            if p < 0.75 {
                1.
            } else {
                -1.
            }
        }
        3 => {
            if p < 0.5 {
                1.
            } else {
                -1.
            }
        }
        4 => {
            if p < 0.25 {
                1.
            } else {
                -1.
            }
        }
        5 => {
            if p < 0.125 {
                1.
            } else {
                -1.
            }
        }
        6 => 2. * p - 1.,
        7 => 1. - 2. * p,
        _ => unreachable!(),
    }
}
#[derive(Clone, Debug)]
struct Filter {
    cutoff: Param,
    resonance: Param,
    kind: u8,
    keytrack: f32,
    keycenter: f32,
    veltrack: f32,
}
impl Filter {
    fn compile(o: &Ops, n: usize, c: &BTreeMap<u16, Vec<f32>>) -> Result<Option<Self>, String> {
        let suffix = if n == 0 { "" } else { "2" };
        let ck = format!("cutoff{suffix}");
        if !o
            .keys()
            .any(|k| k == &ck || k.starts_with(&format!("{ck}_")))
        {
            return Ok(None);
        }
        let fk = if n == 0 { "fil_type" } else { "fil2_type" };
        let kind = match o.get(fk).map(String::as_str).unwrap_or("lpf_2p") {
            "lpf_1p" => 0,
            "hpf_1p" => 1,
            "lpf_2p" => 2,
            "hpf_2p" => 3,
            "bpf_2p" => 4,
            "brf_2p" => 5,
            x => return Err(format!("{fk}: unsupported filter {x}")),
        };
        Ok(Some(Self {
            cutoff: Param::compile(o, &ck, 20000., c)?,
            resonance: Param::compile(o, &format!("resonance{suffix}"), 0., c)?,
            kind,
            keytrack: if n == 0 {
                number(o, "fil_keytrack", 0.)?
            } else {
                0.
            },
            keycenter: key_number(o, "fil_keycenter", 60.)?,
            veltrack: if n == 0 {
                number(o, "fil_veltrack", 0.)?
            } else {
                0.
            },
        }))
    }
}
#[derive(Clone, Debug)]
struct Eq {
    freq: Param,
    bw: Param,
    gain: Param,
}
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    z: [[f32; 2]; 2],
    coefficient_key: [u32; 5],
    coefficients: ([f32; 3], [f32; 2]),
    coefficient_valid: bool,
}
impl Biquad {
    fn store_process(&mut self, x: [f32; 2], key: [u32; 5], b: [f32; 3], a: [f32; 2]) -> [f32; 2] {
        self.coefficient_key = key;
        self.coefficients = (b, a);
        self.coefficient_valid = true;
        self.process(x, b, a)
    }
    fn process(&mut self, x: [f32; 2], b: [f32; 3], a: [f32; 2]) -> [f32; 2] {
        std::array::from_fn(|i| {
            let y = b[0] * x[i] + self.z[i][0];
            self.z[i][0] = b[1] * x[i] - a[0] * y + self.z[i][1];
            self.z[i][1] = b[2] * x[i] - a[1] * y;
            if y.abs() < 1e-30 {
                0.
            } else {
                y
            }
        })
    }
    fn filter(&mut self, x: [f32; 2], kind: u8, f: f32, r: f32, rate: f32) -> [f32; 2] {
        let key = [0, kind as u32, f.to_bits(), r.to_bits(), rate.to_bits()];
        if self.coefficient_valid && self.coefficient_key == key {
            return self.process(x, self.coefficients.0, self.coefficients.1);
        }
        let f = f.clamp(1., rate * 0.49);
        if kind < 2 {
            let a = (-TAU * f / rate).exp();
            let b = if kind == 0 {
                [1. - a, 0., 0.]
            } else {
                [(1. + a) * 0.5, -(1. + a) * 0.5, 0.]
            };
            return self.store_process(x, key, b, [-a, 0.]);
        }
        let w = TAU * f / rate;
        let cs = w.cos();
        let alpha = w.sin() / (2. * (db(r.clamp(0., 40.))));
        let inv = 1. / (1. + alpha);
        let b = match kind {
            2 => [(1. - cs) * 0.5, 1. - cs, (1. - cs) * 0.5],
            3 => [(1. + cs) * 0.5, -(1. + cs), (1. + cs) * 0.5],
            4 => [alpha, 0., -alpha],
            5 => [1., -2. * cs, 1.],
            _ => unreachable!(),
        };
        self.store_process(
            x,
            key,
            b.map(|v| v * inv),
            [-2. * cs * inv, (1. - alpha) * inv],
        )
    }
    fn eq(&mut self, x: [f32; 2], f: f32, bw: f32, g: f32, rate: f32) -> [f32; 2] {
        if g.abs() < 1e-6 {
            return x;
        }
        let key = [1, f.to_bits(), bw.to_bits(), g.to_bits(), rate.to_bits()];
        if self.coefficient_valid && self.coefficient_key == key {
            return self.process(x, self.coefficients.0, self.coefficients.1);
        }
        let w = TAU * f.clamp(1., rate * 0.49) / rate;
        let sn = w.sin();
        let cs = w.cos();
        let a = 10f32.powf(g / 40.);
        let alpha = sn * ((2f32.ln() / 2.) * bw.clamp(0.01, 16.) * w / sn).sinh();
        let inv = 1. / (1. + alpha / a);
        self.store_process(
            x,
            key,
            [
                (1. + alpha * a) * inv,
                -2. * cs * inv,
                (1. - alpha * a) * inv,
            ],
            [-2. * cs * inv, (1. - alpha / a) * inv],
        )
    }
}

// ARIA variables: https://sfzformat.com/opcodes/varNN_mod/.
// mult combines normalized shaped sources by product; add combines by sum.
#[derive(Clone, Debug)]
struct Variable {
    sources: Param,
    mult: bool,
    cutoff: f32,
}
impl Variable {
    fn compile(o: &Ops, p: &str, c: &BTreeMap<u16, Vec<f32>>) -> Result<Self, String> {
        let mult = match o
            .get(&format!("{p}_mod"))
            .map(String::as_str)
            .unwrap_or("add")
        {
            "mult" => true,
            "add" => false,
            x => return Err(format!("{p}_mod: unsupported method {x}")),
        };
        let sources = Param::compile(o, p, 0., c)?;
        for r in &sources.routes {
            if !(0. ..=1.).contains(&r.amount) {
                return Err(format!("{p}_oncc{}: source depth must be 0..1", r.cc));
            }
        }
        Ok(Self {
            sources,
            mult,
            cutoff: number(o, &format!("{p}_cutoff"), 0.)?,
        })
    }
    fn value(&self, cc: &impl std::ops::Index<usize, Output = f32>) -> f32 {
        let vals = self.sources.routes.iter().map(|r| {
            r.amount
                * if r.raw {
                    cc[r.cc]
                } else {
                    lookup(&r.curve, cc[r.cc])
                }
        });
        if self.mult {
            vals.product()
        } else {
            vals.sum()
        }
    }
}
#[derive(Clone, Debug)]
struct Fade {
    source: usize,
    lo: f32,
    hi: f32,
    out: bool,
    power: bool,
}
impl Fade {
    fn gain(&self, key: u8, velocity: f32, cc: &impl std::ops::Index<usize, Output = f32>) -> f32 {
        let x = match self.source {
            144 => key as f32,
            145 => velocity * 127.,
            n => cc[n] * 127.,
        };
        let p = if self.hi == self.lo {
            if x >= self.hi {
                1.
            } else {
                0.
            }
        } else {
            ((x - self.lo) / (self.hi - self.lo)).clamp(0., 1.)
        };
        let p = if self.out { 1. - p } else { p };
        if self.power {
            p.sqrt()
        } else {
            p
        }
    }
}
fn compile_fades(o: &Ops) -> Result<Vec<Fade>, String> {
    let mut result = Vec::new();
    for (source, suffix, curve) in [(144, "key", "xf_keycurve"), (145, "vel", "xf_velcurve")]
        .into_iter()
        .chain((0..128).map(|n| (n, "cc", "xf_cccurve")))
    {
        let power = match o.get(curve).map(String::as_str).unwrap_or("power") {
            "power" => true,
            "gain" => false,
            x => return Err(format!("{curve}: unknown fade curve {x}")),
        };
        for (out, p) in [(false, "xfin"), (true, "xfout")] {
            let tail = if suffix == "cc" {
                format!("cc{source}")
            } else {
                suffix.to_string()
            };
            let low = format!("{p}_lo{tail}");
            let high = format!("{p}_hi{tail}");
            if !o.contains_key(&low) && !o.contains_key(&high) {
                continue;
            }
            let lo = if suffix == "key" {
                key_number(o, &low, 0.)?
            } else {
                number(o, &low, 0.)?
            };
            let hi = if suffix == "key" {
                key_number(o, &high, 127.)?
            } else {
                number(o, &high, 127.)?
            };
            if !(0. ..=127.).contains(&lo) || !(lo..=127.).contains(&hi) {
                return Err(format!("{low}/{high}: invalid fade range"));
            }
            result.push(Fade {
                source,
                lo,
                hi,
                out,
                power,
            });
        }
    }
    Ok(result)
}
fn canonical_modulation(k: &str) -> String {
    if let Some(rest) = k.strip_prefix("lfo") {
        if let Some((n, f)) = rest.split_once('_') {
            if let Ok(n) = n.parse::<usize>() {
                let field = if let Some(rest) = f.strip_prefix("freq_lfo") {
                    let (src, tail) = rest
                        .split_once('_')
                        .map(|(a, b)| (a, format!("_{b}")))
                        .unwrap_or((rest, String::new()));
                    if let Ok(src) = src.parse::<usize>() {
                        format!("freq_lfo{src:02}{tail}")
                    } else {
                        f.to_string()
                    }
                } else {
                    f.to_string()
                };
                return format!("lfo{n:02}_{field}");
            }
        }
    }
    k.to_string()
}
// Corpus numbered envelopes use at most three points; the public bound is eight.
// Bounds are preparation errors, never callback-time resizing.
#[derive(Clone, Debug)]
struct MultiEg {
    index: usize,
    time: Vec<Param>,
    level: Vec<Param>,
    curves: Vec<[f32; 128]>,
    sustain: usize,
    pitch: Param,
}
#[derive(Clone, Copy, Debug, Default)]
struct MultiState {
    point: usize,
    elapsed: f64,
    origin: f32,
    value: f32,
    initialized: bool,
    released: bool,
    release_started: bool,
}
fn eg_name(k: &str) -> Option<(usize, &str)> {
    let k = k.strip_prefix("eg")?;
    let (n, field) = k.split_once('_')?;
    let n = n.parse::<usize>().ok()?;
    if !(1..=32).contains(&n) {
        return None;
    }
    Some((n - 1, field))
}
impl MultiEg {
    fn compile(index: usize, o: &Ops, c: &BTreeMap<u16, Vec<f32>>) -> Result<Self, String> {
        let p = format!("eg{}_", index + 1);
        let mut count = 1;
        for k in o.keys() {
            if let Some((n, f)) = eg_name(k) {
                if n != index {
                    continue;
                }
                let field = f.split('_').next().unwrap();
                for stem in ["time", "level", "curve"] {
                    if let Some(i) = field
                        .strip_prefix(stem)
                        .and_then(|x| x.parse::<usize>().ok())
                    {
                        if i >= 8 {
                            return Err(format!("{k}: envelope point exceeds supported bound 7"));
                        }
                        count = count.max(i + 1);
                    }
                }
            }
        }
        let mut time = Vec::new();
        let mut level = Vec::new();
        let mut curves = Vec::new();
        for i in 0..count {
            time.push(Param::compile(o, &format!("{p}time{i}"), 0., c)?);
            level.push(Param::compile(o, &format!("{p}level{i}"), 0., c)?);
            let curve = number(o, &format!("{p}curve{i}"), 0.)?;
            if curve.fract() != 0. || !(0. ..=255.).contains(&curve) {
                return Err(format!("{p}curve{i}: invalid curve index"));
            }
            curves.push(curve_table(curve as u16, c)?);
        }
        let sustain = number(o, &format!("{p}sustain"), (count - 1) as f32)?;
        if sustain.fract() != 0. || sustain < 0. || sustain >= count as f32 {
            return Err(format!("{p}sustain: invalid point"));
        }
        Ok(Self {
            index,
            time,
            level,
            curves,
            sustain: sustain as usize,
            pitch: Param::compile(o, &format!("{p}pitch"), 0., c)?,
        })
    }
    fn step(
        &self,
        s: &mut MultiState,
        cc: &impl std::ops::Index<usize, Output = f32>,
        rate: f64,
    ) -> f32 {
        if !s.initialized {
            s.value = self.level[0].get(cc).clamp(-1., 1.);
            s.origin = s.value;
            s.point = 0;
            s.initialized = true;
        }
        if s.released && !s.release_started {
            s.release_started = true;
            s.point = self.sustain + 1;
            s.elapsed = 0.;
            s.origin = s.value;
        }
        for _ in 0..8 {
            if s.point >= self.time.len() {
                return s.value;
            }
            if s.point == self.sustain && !s.released {
                // sustain after arriving, not before the preceding segment
                if s.elapsed >= self.time[s.point].get(cc).max(0.) as f64 {
                    return s.value;
                }
            }
            let duration = self.time[s.point].get(cc).max(0.) as f64;
            if s.elapsed + 1e-9 >= duration {
                s.value = self.level[s.point].get(cc).clamp(-1., 1.);
                if s.point == self.sustain && !s.released {
                    return s.value;
                }
                s.origin = s.value;
                s.point += 1;
                s.elapsed = 0.;
                continue;
            }
            s.elapsed = (s.elapsed + 1. / rate).min(duration);
            let p = lookup(&self.curves[s.point], (s.elapsed / duration) as f32);
            s.value = s.origin + (self.level[s.point].get(cc).clamp(-1., 1.) - s.origin) * p;
            return s.value;
        }
        s.value
    }
}
/// Immutable, prepared region DSP. Sharing this data does not copy curves per voice.
#[derive(Clone, Debug)]
pub struct RegionDsp {
    volume: Param,
    scope_volume: f32,
    scope_tune: f32,
    pitch: Param,
    amplitude: Param,
    pan: Param,
    width: Param,
    velocity_curve: [f32; 128],
    implicit_gain: [bool; 2],
    veltrack: Param,
    pitch_veltrack: Param,
    keytrack: f32,
    keycenter: f32,
    envelopes: [Envelope; 3],
    envelope_active: [bool; 3],
    constant_pitch: Option<f32>,
    multi: Vec<MultiEg>,
    lfos: [Lfo; 6],
    lfo_order: Vec<usize>,
    lfo_active: Vec<usize>,
    variables: [Variable; 2],
    fades: Vec<Fade>,
    filters: [Option<Filter>; 2],
    eqs: [Eq; 3],
}
impl RegionDsp {
    #[cfg(test)]
    pub fn compile(o: &Ops) -> Result<Self, String> {
        Self::compile_with_curves(o, &BTreeMap::new())
    }
    pub fn compile_with_curves(o: &Ops, c: &BTreeMap<u16, Vec<f32>>) -> Result<Self, String> {
        let mut normalized = o.clone();
        for (k, v) in o {
            if let Some((i, f)) = eg_name(k) {
                let canonical = format!("eg{}_{f}", i + 1);
                if canonical != *k {
                    if normalized.get(&canonical).is_some_and(|other| other != v) {
                        return Err(format!("{k}: conflicting numbered envelope alias"));
                    }
                    normalized.insert(canonical, v.clone());
                }
            }
        }
        let mut aliases = BTreeMap::new();
        for (k, v) in &normalized {
            let canonical = canonical_modulation(k);
            if aliases
                .insert(canonical.clone(), v.clone())
                .is_some_and(|old| old != *v)
            {
                return Err(format!("{k}: conflicting LFO alias"));
            }
        }
        let mut resolved = aliases.clone();
        for (k, v) in &aliases {
            let canonical = k
                .strip_prefix("gain_cc")
                .map(|cc| format!("volume_cc{cc}"))
                .or_else(|| k.strip_prefix("tune_").map(|f| format!("pitch_{f}")));
            if let Some(canonical) = canonical {
                if resolved.get(&canonical).is_some_and(|old| old != v) {
                    return Err(format!("{k}: conflicting modulation alias"));
                }
                resolved.insert(canonical, v.clone());
            }
        }
        let o = &resolved;
        let mut velocity_curve = std::array::from_fn(|i| (i as f32 / 127.).powi(2));
        let mut anchors = BTreeMap::new();
        anchors.insert(0, 0.);
        anchors.insert(127, 1.);
        let mut custom = false;
        for (k, v) in o {
            if let Some(i) = k
                .strip_prefix("amp_velcurve_")
                .and_then(|s| s.parse::<usize>().ok())
            {
                if i > 127 {
                    return Err(format!("{k}: invalid velocity"));
                }
                let v = parse_num(k, v)?;
                if !(0. ..=1.).contains(&v) {
                    return Err(format!("{k}: expected 0..1"));
                }
                anchors.insert(i, v);
                custom = true;
            }
        }
        if custom {
            let a: Vec<_> = anchors.into_iter().collect();
            for pair in a.windows(2) {
                let ((i, x), (j, y)) = (pair[0], pair[1]);
                for (n, out) in velocity_curve.iter_mut().enumerate().take(j + 1).skip(i) {
                    *out = x + (y - x) * (n - i) as f32 / (j - i) as f32;
                }
            }
        }
        let mut lfos = [
            Lfo::compile(o, "lfo01", c, false)?,
            Lfo::compile(o, "lfo02", c, false)?,
            Lfo::compile(o, "lfo03", c, false)?,
            Lfo::compile(o, "pitchlfo", c, true)?,
            Lfo::compile(o, "amplfo", c, true)?,
            Lfo::compile(o, "fillfo", c, true)?,
        ];
        // SFZ lfoN_freq_lfoX names SOURCE N and TARGET X. See
        // https://sfzformat.com/tutorials/vibrato/ ("Affect rate of other LFO").
        // Store incoming routes on each target for current-sample evaluation.
        // Sforzando DC clock probes confirm additive Hz at corpus depth 1.
        let outgoing: [Vec<(usize, Param)>; 6] =
            std::array::from_fn(|i| std::mem::take(&mut lfos[i].cross));
        for (source, targets) in outgoing.into_iter().enumerate() {
            for (target, depth) in targets {
                lfos[target].cross.push((source, depth));
            }
        }
        // Current-sample acyclic evaluation; source precedes destination. Cyclic
        // frequency modulation has no verified semantics and is rejected.
        let mut lfo_order = [0; 6];
        let mut ready = [false; 6];
        for slot in &mut lfo_order {
            let Some(i) =
                (0..6).find(|&i| !ready[i] && lfos[i].cross.iter().all(|(src, _)| ready[*src]))
            else {
                return Err("LFO frequency modulation cycle".into());
            };
            *slot = i;
            ready[i] = true;
        }
        let lfo_order = lfo_order
            .into_iter()
            .filter(|&i| {
                lfos[i].enabled
                    || lfos
                        .iter()
                        .any(|l| l.cross.iter().any(|(src, _)| *src == i))
            })
            .collect();
        let variables = [
            Variable::compile(o, "var01", c)?,
            Variable::compile(o, "var02", c)?,
        ];
        let fades = compile_fades(o)?;
        let mut multi = Vec::new();
        for n in 0..32 {
            if o.keys().any(|k| eg_name(k).is_some_and(|(i, _)| i == n)) {
                multi.push(MultiEg::compile(n, o, c)?);
            }
        }
        let mut eqs = Vec::new();
        for n in 1..=3 {
            eqs.push(Eq {
                freq: Param::compile(o, &format!("eq{n}_freq"), [50., 500., 5000.][n - 1], c)?,
                bw: Param::compile(o, &format!("eq{n}_bw"), 1., c)?,
                gain: Param::compile(o, &format!("eq{n}_gain"), 0., c)?,
            });
        }
        let amplitude = Param::compile(o, "amplitude", 100., c)?;
        let implicit_gain = [
            !amplitude.routes.iter().any(|r| r.cc == 7),
            !amplitude.routes.iter().any(|r| r.cc == 11),
        ];
        let mut pitch = Param::compile(o, "pitch", 0., c)?;
        if let Some(tune) = o.get("tune") {
            if o.get("pitch").is_some_and(|p| p != tune) {
                return Err("pitch/tune aliases require ordered source normalization".into());
            }
            pitch.base = 0.;
        }
        let envelopes = [
            Envelope::compile(o, "ampeg", c)?,
            Envelope::compile(o, "pitcheg", c)?,
            Envelope::compile(o, "fileg", c)?,
        ];
        let envelope_active = [
            true,
            envelopes[1].depth.base != 0. || !envelopes[1].depth.routes.is_empty(),
            envelopes[2].depth.base != 0. || !envelopes[2].depth.routes.is_empty(),
        ];
        let constant_pitch = if pitch.routes.is_empty()
            && multi.is_empty()
            && !envelope_active[1]
            && lfos
                .iter()
                .all(|l| l.pitch.base == 0. && l.pitch.routes.is_empty())
            && !o.keys().any(|k| k.starts_with("pitch_veltrack"))
        {
            Some(
                number(o, "global_tune", 0.)?
                    + number(o, "master_tune", 0.)?
                    + number(o, "group_tune", 0.)?
                    + pitch.base,
            )
        } else {
            None
        };
        Ok(Self {
            envelope_active,
            constant_pitch,
            scope_volume: number(o, "global_volume", 0.)?
                + number(o, "master_volume", 0.)?
                + number(o, "group_volume", 0.)?,
            scope_tune: number(o, "global_tune", 0.)?
                + number(o, "master_tune", 0.)?
                + number(o, "group_tune", 0.)?,
            pitch,
            implicit_gain,
            volume: Param::compile(o, "volume", 0., c)?,
            amplitude,
            pan: Param::compile(o, "pan", 0., c)?,
            width: Param::compile(o, "width", 100., c)?,
            velocity_curve,
            veltrack: Param::compile(o, "amp_veltrack", 100., c)?,
            pitch_veltrack: Param::compile(o, "pitch_veltrack", 0., c)?,
            keytrack: number(o, "amp_keytrack", 0.)?,
            keycenter: key_number(o, "amp_keycenter", 60.)?,
            multi,
            envelopes,
            lfo_active: (0..lfos.len()).filter(|&i| lfos[i].enabled).collect(),
            lfos,
            lfo_order,
            variables,
            fades,
            filters: [Filter::compile(o, 0, c)?, Filter::compile(o, 1, c)?],
            eqs: eqs.try_into().unwrap(),
        })
    }
    pub fn start(&self, note: u8, velocity: u8, sample_rate: f64) -> VoiceDsp {
        VoiceDsp {
            note,
            #[cfg(test)]
            force_destination_paths: false,
            vel: velocity as f32 / 127.,
            rate: sample_rate.max(1.),
            time: 0.,
            env: [EnvState::default(); 3],
            phases: [0.; 6],
            lfo_values: [0.; 6],
            filters: [Biquad::default(); 2],
            filter_hz_cache: [None; 2],
            controller_cache: ControllerCache::default(),
            eqs: [Biquad::default(); 3],
            initialized: false,
            velocity_gain: 1.,
            pitch_velocity: 0.,
            volume_cache: (f32::NAN, 0.),
            pan_cache: (f32::NAN, [0.; 2]),
            choked: false,
            choke_gain: 1.,
            virtual_sources: [0.; 16],
            multi: [MultiState::default(); 32],
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct LfoControl {
    freq: f32,
    delay: f32,
    fade: f32,
    volume: f32,
    pitch: f32,
    cutoff: f32,
    eq_freq: [f32; 3],
    eq_gain: [f32; 3],
    cross: [f32; 3],
}
#[derive(Clone, Copy, Debug, Default)]
struct ControllerCache {
    generation: Option<u64>,
    volume: f32,
    amplitude: f32,
    width: f32,
    pan: f32,
    implicit_gain: [f32; 2],
    fade: f32,
    lfos: [LfoControl; 6],
    filter_route: [f32; 2],
    resonance: [f32; 2],
    eqs: [[f32; 3]; 3],
    filter_env_depth: f32,
    pitch: f32,
    pitch_env_depth: f32,
    multi_pitch: [f32; 32],
    eq_active: [bool; 3],
    volume_modulated: bool,
    filter_modulated: bool,
    pitch_modulated: bool,
}
/// Fixed-size callback state. `start`, `next`, release and expression use no heap.
#[derive(Clone, Debug)]
pub struct VoiceDsp {
    #[cfg(test)]
    force_destination_paths: bool,
    note: u8,
    vel: f32,
    rate: f64,
    time: f64,
    env: [EnvState; 3],
    phases: [f64; 6],
    lfo_values: [f32; 6],
    filters: [Biquad; 2],
    filter_hz_cache: [Option<(u32, f32)>; 2],
    controller_cache: ControllerCache,
    eqs: [Biquad; 3],
    initialized: bool,
    velocity_gain: f32,
    pitch_velocity: f32,
    volume_cache: (f32, f32),
    pan_cache: (f32, [f32; 2]),
    choked: bool,
    choke_gain: f32,
    virtual_sources: [f32; 16],
    multi: [MultiState; 32],
}
impl VoiceDsp {
    /// Capture note-on controls at the event boundary without advancing time.
    /// Hosts must call this immediately after `start`; lazy initialization in
    /// `next` exists for isolated DSP use, not event ordering inside a callback.
    /// Repeated calls preserve a restored voice's existing latched state.
    pub fn latch(&mut self, d: &RegionDsp, cc: &[f32; 128], virtual_sources: &[f32; 16]) {
        self.virtual_sources = *virtual_sources;
        self.controller_cache.generation = None;
        if self.initialized {
            return;
        }
        let sources = ControllerSources {
            physical: cc,
            virtual_sources: &self.virtual_sources,
        };
        let cc = &sources;
        let tracking = d.veltrack.get(cc) / 100.;
        let curve = lookup(&d.velocity_curve, self.vel);
        self.velocity_gain = if tracking < 0. {
            tracking.abs() * (1. - curve)
        } else {
            1. - tracking * (1. - curve)
        };
        self.pitch_velocity = self.vel * d.pitch_veltrack.get(cc);
        for (s, e) in self.env.iter_mut().zip(&d.envelopes) {
            s.latch(e, cc, self.vel);
        }
        for &i in &d.lfo_order {
            self.phases[i] = d.lfos[i].phase.get(cc) as f64;
        }
        for e in &d.multi {
            let s = &mut self.multi[e.index];
            s.value = e.level[0].get(cc).clamp(-1., 1.);
            s.origin = s.value;
            s.initialized = true;
        }
        self.initialized = true;
    }
    /// Controller-triggered regions retain event strength for tracking envelopes,
    /// filters and pitch, but SFZ amp_veltrack applies only to note-on velocity.
    pub fn latch_controller_trigger(
        &mut self,
        d: &RegionDsp,
        cc: &[f32; 128],
        virtual_sources: &[f32; 16],
    ) {
        self.latch(d, cc, virtual_sources);
        self.velocity_gain = 1.;
    }
    #[cfg(test)]
    pub fn set_virtual_sources(&mut self, sources: &[f32; 16]) {
        self.virtual_sources = *sources;
        self.controller_cache.generation = None;
    }
    pub fn release(&mut self) {
        for e in &mut self.env {
            e.release();
        }
        for s in &mut self.multi {
            s.released = true;
        }
    }
    #[cfg(test)]
    pub fn choke(&mut self) {
        self.choked = true;
        self.release();
    }
    pub fn finished(&self) -> bool {
        self.env[0].stage == 6
    }
    pub fn pitch_cents_with_generation(
        &self,
        d: &RegionDsp,
        cc: &[f32; 128],
        generation: u64,
    ) -> f32 {
        if let Some(pitch) = d.constant_pitch {
            return pitch;
        }
        if self.controller_cache.generation != Some(generation) || !self.initialized {
            // CC events occur before next() refreshes coefficients: evaluate the
            // new controller depths against the original previous-frame state.
            return self.pitch_cents(d, cc);
        }
        let cached = &self.controller_cache;
        cached.pitch
            + d.multi
                .iter()
                .map(|e| self.multi[e.index].value * cached.multi_pitch[e.index])
                .sum::<f32>()
            + self.env[1].value * cached.pitch_env_depth
            + if cached.pitch_modulated {
                d.lfo_active
                    .iter()
                    .map(|&j| self.lfo_values[j] * cached.lfos[j].pitch)
                    .sum::<f32>()
            } else {
                0.
            }
    }
    pub fn pitch_cents(&self, d: &RegionDsp, cc: &[f32; 128]) -> f32 {
        if let Some(pitch) = d.constant_pitch {
            return pitch;
        }
        let sources = ControllerSources {
            physical: cc,
            virtual_sources: &self.virtual_sources,
        };
        let cc = &sources;
        d.scope_tune
            + d.pitch.get(cc)
            + if self.initialized {
                self.pitch_velocity
            } else {
                self.vel * d.pitch_veltrack.get(cc)
            }
            + d.multi
                .iter()
                .map(|e| self.multi[e.index].value * e.pitch.get(cc))
                .sum::<f32>()
            + self.env[1].value * d.envelopes[1].depth.get(cc)
            + d.lfo_active
                .iter()
                .map(|&j| self.lfo_values[j] * d.lfos[j].pitch.get(cc))
                .sum::<f32>()
    }
    #[cfg(test)]
    pub fn next(
        &mut self,
        d: &RegionDsp,
        input: [f32; 2],
        cc: &[f32; 128],
        _pitch_expression: f32,
        gain_expression: f32,
    ) -> [f32; 2] {
        self.next_inner(d, input, cc, gain_expression, None)
    }
    pub fn next_with_generation(
        &mut self,
        d: &RegionDsp,
        input: [f32; 2],
        cc: &[f32; 128],
        _pitch_expression: f32,
        gain_expression: f32,
        generation: u64,
    ) -> [f32; 2] {
        self.next_inner(d, input, cc, gain_expression, Some(generation))
    }
    fn next_inner(
        &mut self,
        d: &RegionDsp,
        mut input: [f32; 2],
        cc: &[f32; 128],
        gain_expression: f32,
        generation: Option<u64>,
    ) -> [f32; 2] {
        if !self.initialized {
            let virtual_sources = self.virtual_sources;
            self.latch(d, cc, &virtual_sources);
        }
        let sources = ControllerSources {
            physical: cc,
            virtual_sources: &self.virtual_sources,
        };
        let cc = &sources;
        let controls_changed =
            generation.is_none() || self.controller_cache.generation != generation;
        if controls_changed {
            self.controller_cache = ControllerCache {
                generation,
                eq_active: std::array::from_fn(|i| {
                    d.eqs[i].gain.get(cc).abs() >= 1e-6
                        || d.lfo_active
                            .iter()
                            .any(|&j| d.lfos[j].eq_gain[i].get(cc) != 0.)
                }),
                volume_modulated: d.lfo_active.iter().any(|&i| d.lfos[i].volume.get(cc) != 0.),
                filter_modulated: d.lfo_active.iter().any(|&i| d.lfos[i].cutoff.get(cc) != 0.),
                pitch_modulated: d.lfo_active.iter().any(|&i| d.lfos[i].pitch.get(cc) != 0.),
                lfos: std::array::from_fn(|i| {
                    let l = &d.lfos[i];
                    LfoControl {
                        freq: l.freq.get(cc),
                        delay: l.delay.get(cc),
                        fade: l.fade.get(cc),
                        volume: l.volume.get(cc),
                        pitch: l.pitch.get(cc),
                        cutoff: l.cutoff.get(cc),
                        eq_freq: std::array::from_fn(|j| l.eq_freq[j].get(cc)),
                        eq_gain: std::array::from_fn(|j| l.eq_gain[j].get(cc)),
                        cross: std::array::from_fn(|j| {
                            l.cross.get(j).map(|(_, p)| p.get(cc)).unwrap_or(0.)
                        }),
                    }
                }),
                filter_route: std::array::from_fn(|i| {
                    d.filters[i]
                        .as_ref()
                        .map(|f| {
                            f.cutoff.get(cc) - f.cutoff.base
                                + if i == 0 {
                                    d.variables
                                        .iter()
                                        .map(|v| v.value(cc) * v.cutoff)
                                        .sum::<f32>()
                                } else {
                                    0.
                                }
                        })
                        .unwrap_or(0.)
                }),
                resonance: std::array::from_fn(|i| {
                    d.filters[i]
                        .as_ref()
                        .map(|f| f.resonance.get(cc))
                        .unwrap_or(0.)
                }),
                eqs: std::array::from_fn(|i| {
                    [
                        d.eqs[i].freq.get(cc),
                        d.eqs[i].bw.get(cc),
                        d.eqs[i].gain.get(cc),
                    ]
                }),
                filter_env_depth: d.envelopes[2].depth.get(cc),
                pitch: d.scope_tune + d.pitch.get(cc) + self.pitch_velocity,
                pitch_env_depth: d.envelopes[1].depth.get(cc),
                multi_pitch: {
                    let mut depths = [0.; 32];
                    for e in &d.multi {
                        depths[e.index] = e.pitch.get(cc);
                    }
                    depths
                },
                volume: d.scope_volume
                    + d.volume.get(cc)
                    + (self.note as f32 - d.keycenter) * d.keytrack,
                amplitude: d.amplitude.product(cc),
                width: d.width.get(cc).clamp(-100., 100.),
                pan: d.pan.get(cc).clamp(-100., 100.),
                implicit_gain: std::array::from_fn(|i| {
                    if d.implicit_gain[i] {
                        cc[if i == 0 { 7 } else { 11 }].clamp(0., 1.).powi(2)
                    } else {
                        1.
                    }
                }),
                fade: d
                    .fades
                    .iter()
                    .map(|f| f.gain(self.note, self.vel, cc))
                    .product::<f32>(),
            };
        }
        #[cfg(test)]
        if self.force_destination_paths {
            self.controller_cache.eq_active = [true; 3];
            self.controller_cache.volume_modulated = true;
            self.controller_cache.filter_modulated = true;
            self.controller_cache.pitch_modulated = true;
        }
        let cached = &self.controller_cache;
        let mut env = [0.; 3];
        for (i, e) in d.envelopes.iter().enumerate() {
            if d.envelope_active[i] {
                env[i] = self.env[i].step(e, cc, self.vel, self.rate, controls_changed);
            }
        }
        for e in &d.multi {
            e.step(&mut self.multi[e.index], cc, self.rate);
        }
        if self.choked {
            self.choke_gain *= (-1. / (0.008 * self.rate)).exp() as f32;
            env[0] *= self.choke_gain;
            if self.choke_gain < 1e-6 {
                self.env[0].stage = 6;
            }
        }
        for &i in &d.lfo_order {
            let l = &d.lfos[i];
            let t = self.time - cached.lfos[i].delay.max(0.) as f64;
            let fade = cached.lfos[i].fade.max(0.) as f64;
            let strength = if t < 0. {
                0.
            } else if fade > 0. {
                (t / fade).min(1.) as f32
            } else {
                1.
            };
            let mut value = wave(l.wave, self.phases[i]);
            if let Some((w, ratio, scale, offset)) = l.secondary {
                value += scale * wave(w, self.phases[i] * ratio as f64 + offset as f64);
            }
            self.lfo_values[i] = value * strength;
            if t >= 0. {
                self.phases[i] += (cached.lfos[i].freq
                    + l.cross
                        .iter()
                        .enumerate()
                        .map(|(k, (src, _))| self.lfo_values[*src] * cached.lfos[i].cross[k])
                        .sum::<f32>())
                .max(0.) as f64
                    / self.rate;
            }
        }
        let volume = cached.volume
            + if cached.volume_modulated {
                d.lfo_active
                    .iter()
                    .map(|&j| self.lfo_values[j] * cached.lfos[j].volume)
                    .sum::<f32>()
            } else {
                0.
            };
        if self.volume_cache.0.to_bits() != volume.to_bits() {
            self.volume_cache = (volume, db(volume));
        }
        let gain = self.volume_cache.1 * cached.amplitude / 100.
            * self.velocity_gain
            * env[0]
            * gain_expression
            * cached.implicit_gain[0]
            * cached.implicit_gain[1]
            * cached.fade;
        let mid = (input[0] + input[1]) * 0.5;
        let side = (input[0] - input[1]) * 0.5 * cached.width / 100.;
        input = [mid + side, mid - side];
        let pan = cached.pan / 100.;
        if self.pan_cache.0.to_bits() != pan.to_bits() {
            self.pan_cache = (pan, [(1. - pan).max(0.).sqrt(), (1. + pan).max(0.).sqrt()]);
        }
        input[0] *= self.pan_cache.1[0] * gain;
        input[1] *= self.pan_cache.1[1] * gain;
        for (i, f) in d.filters.iter().enumerate() {
            if let Some(f) = f {
                let cents = (self.note as f32 - f.keycenter) * f.keytrack
                    + self.vel * f.veltrack
                    + if i == 0 {
                        env[2] * cached.filter_env_depth
                            + if cached.filter_modulated {
                                d.lfo_active
                                    .iter()
                                    .map(|&j| self.lfo_values[j] * cached.lfos[j].cutoff)
                                    .sum::<f32>()
                            } else {
                                0.
                            }
                    } else {
                        0.
                    };
                // Cutoff CC amounts are cents, whereas the base cutoff is Hz.
                let base = f.cutoff.base;
                let route_cents = cached.filter_route[i];
                let exponent = (cents + route_cents) / 1200.;
                let key = exponent.to_bits();
                let hz = match self.filter_hz_cache[i] {
                    Some((cached_key, hz)) if cached_key == key => hz,
                    _ => {
                        let hz = base * 2f32.powf(exponent);
                        self.filter_hz_cache[i] = Some((key, hz));
                        hz
                    }
                };
                input = self.filters[i].filter(
                    input,
                    f.kind,
                    hz,
                    cached.resonance[i],
                    self.rate as f32,
                );
            }
        }
        for i in 0..d.eqs.len() {
            // An EQ bypass never advances its delay state in the original path.
            // Keep that state unchanged until a controller enables a gain route.
            if !cached.eq_active[i] {
                continue;
            }
            input = self.eqs[i].eq(
                input,
                cached.eqs[i][0]
                    + d.lfo_active
                        .iter()
                        .map(|&j| self.lfo_values[j] * cached.lfos[j].eq_freq[i])
                        .sum::<f32>(),
                cached.eqs[i][1],
                cached.eqs[i][2]
                    + d.lfo_active
                        .iter()
                        .map(|&j| self.lfo_values[j] * cached.lfos[j].eq_gain[i])
                        .sum::<f32>(),
                self.rate as f32,
            );
        }
        self.time += 1. / self.rate;
        input
    }
}
/// Explicit registry. Unknown or unverified features are not silent no-ops.
pub fn supports_opcode(k: &str) -> bool {
    if k == "pitch" {
        return true;
    }
    if matches!(
        k,
        "global_volume"
            | "master_volume"
            | "group_volume"
            | "global_tune"
            | "master_tune"
            | "group_tune"
    ) {
        return true;
    }
    if k.strip_prefix("gain_cc")
        .is_some_and(|s| s.parse::<u16>().is_ok())
    {
        return true;
    }
    if k == "pitch_veltrack"
        || [
            "pitch_veltrack_oncc",
            "pitch_veltrack_cc",
            "pitch_veltrack_curvecc",
        ]
        .iter()
        .any(|p| k.strip_prefix(p).is_some_and(|n| n.parse::<u16>().is_ok()))
    {
        return true;
    }
    if k.starts_with("pitch_") || k.starts_with("tune_") {
        let canonical = k
            .strip_prefix("tune_")
            .map(|s| format!("pitch_{s}"))
            .unwrap_or(k.to_string());
        return ["pitch_cc", "pitch_oncc", "pitch_curvecc"].iter().any(|p| {
            canonical
                .strip_prefix(p)
                .is_some_and(|s| s.parse::<u16>().is_ok())
        });
    }
    if matches!(k, "xf_keycurve" | "xf_velcurve" | "xf_cccurve") {
        return true;
    }
    for p in ["xfin_lo", "xfin_hi", "xfout_lo", "xfout_hi"] {
        if let Some(t) = k.strip_prefix(p) {
            return matches!(t, "key" | "vel")
                || t.strip_prefix("cc")
                    .is_some_and(|n| n.parse::<u8>().is_ok());
        }
    }
    for p in ["var01_", "var02_"] {
        if let Some(t) = k.strip_prefix(p) {
            return matches!(t, "mod" | "cutoff")
                || t.strip_prefix("oncc")
                    .or_else(|| t.strip_prefix("curvecc"))
                    .is_some_and(|n| n.parse::<u16>().is_ok());
        }
    }
    let canonical = canonical_modulation(k);
    let k = canonical.as_str();
    if let Some((_, field)) = eg_name(k) {
        let base = field.split("_oncc").next().unwrap();
        if matches!(base, "pitch" | "sustain") {
            return true;
        }
        for p in ["time", "level", "curve"] {
            if base
                .strip_prefix(p)
                .is_some_and(|s| s.parse::<u8>().is_ok())
            {
                return true;
            }
        }
        return false;
    }
    let base = if let Some((p, n)) = k
        .rsplit_once("_oncc")
        .or_else(|| k.rsplit_once("_cc"))
        .or_else(|| k.rsplit_once("_curvecc"))
    {
        if n.parse::<u16>().is_err() {
            return false;
        }
        p
    } else if let Some((p, n)) = k.rsplit_once("cc") {
        if n.parse::<u16>().is_err() {
            return false;
        }
        p
    } else {
        k
    };
    if base != k
        && (matches!(
            base,
            "amp_keytrack"
                | "amp_keycenter"
                | "fil_type"
                | "fil2_type"
                | "fil_keytrack"
                | "fil_keycenter"
                | "fil_veltrack"
        ) || base.ends_with("_dynamic")
            || base.ends_with("_shape")
            || base.ends_with("_wave")
            || base.ends_with("_wave2")
            || base.ends_with("_ratio2")
            || base.ends_with("_scale2")
            || base.ends_with("_offset2")
            || base.contains("_vel2"))
    {
        return false;
    }
    if matches!(
        base,
        "volume"
            | "amplitude"
            | "pan"
            | "width"
            | "amp_veltrack"
            | "amp_keytrack"
            | "amp_keycenter"
            | "cutoff"
            | "cutoff2"
            | "resonance"
            | "resonance2"
            | "fil_type"
            | "fil2_type"
            | "fil_keytrack"
            | "fil_keycenter"
            | "fil_veltrack"
    ) {
        return true;
    }
    if k.strip_prefix("amp_velcurve_")
        .is_some_and(|n| n.parse::<u8>().is_ok())
    {
        return true;
    }
    for p in ["ampeg", "pitcheg", "fileg"] {
        if let Some(n) = base.strip_prefix(&format!("{p}_")) {
            if p == "ampeg" && n == "depth" {
                return false;
            }
            return matches!(
                n,
                "delay"
                    | "start"
                    | "attack"
                    | "hold"
                    | "decay"
                    | "sustain"
                    | "release"
                    | "dynamic"
                    | "depth"
                    | "vel2delay"
                    | "vel2attack"
                    | "vel2hold"
                    | "vel2decay"
                    | "vel2sustain"
                    | "vel2release"
                    | "attack_shape"
                    | "decay_shape"
                    | "release_shape"
            );
        }
    }
    for p in ["lfo01", "lfo02", "lfo03", "pitchlfo", "amplfo", "fillfo"] {
        if let Some(n) = base.strip_prefix(&format!("{p}_")) {
            if n == "depth" && p.starts_with("lfo") {
                return false;
            }
            return matches!(
                n,
                "freq"
                    | "delay"
                    | "fade"
                    | "phase"
                    | "pitch"
                    | "volume"
                    | "cutoff"
                    | "wave"
                    | "wave2"
                    | "ratio2"
                    | "scale2"
                    | "offset2"
                    | "depth"
                    | "eq1freq"
                    | "eq1gain"
                    | "eq2freq"
                    | "eq2gain"
                    | "eq3freq"
                    | "eq3gain"
            ) || n
                .strip_prefix("freq_lfo")
                .is_some_and(|x| x.parse::<u8>().is_ok_and(|v| (1..=3).contains(&v)));
        }
    }
    for p in ["eq1", "eq2", "eq3"] {
        if let Some(n) = base.strip_prefix(&format!("{p}_")) {
            return matches!(n, "freq" | "bw" | "gain");
        }
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ops(rows: &[(&str, &str)]) -> Ops {
        rows.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
    fn neutral_cc() -> [f32; 128] {
        let mut cc = [0.; 128];
        cc[7] = 1.;
        cc[11] = 1.;
        cc
    }
    #[test]
    fn numbered_envelope_clamps_and_pitch_keydelta_uses_semitones() {
        let d = RegionDsp::compile(&ops(&[
            ("eg01_level0", "-100"),
            ("eg01_level1", "0"),
            ("eg01_time1", "0.01"),
            ("eg01_sustain", "1"),
            ("eg01_pitch_oncc140", "100"),
        ]))
        .unwrap();
        let mut v = d.start(60, 127, 1000.);
        let mut ext = [0.; 16];
        ext[12] = 12.;
        v.set_virtual_sources(&ext);
        v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        assert!((v.pitch_cents(&d, &neutral_cc()) + 1080.).abs() < 0.01);
        for _ in 0..10 {
            v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        }
        assert_eq!(v.pitch_cents(&d, &neutral_cc()), 0.);
    }
    #[test]
    fn built_in_curves_and_release_shape_have_expected_direction() {
        let c = BTreeMap::new();
        assert_eq!(curve_table(4, &c).unwrap()[127], 1.);
        assert!(shaped(0.5, -6.) > 0.9);
        assert!(shaped(0.5, 6.) < 0.1);
        for shape in [-1000., 1000.] {
            assert!(shaped(0.5, shape).is_finite());
            assert_eq!(shaped(0., shape), 0.);
            assert_eq!(shaped(1., shape), 1.);
        }
    }
    #[test]
    fn amplitude_cc_is_multiplicative_and_fades_keep_power() {
        let d = RegionDsp::compile(&ops(&[
            ("amplitude_oncc7", "100"),
            ("xfin_locc10", "0"),
            ("xfin_hicc10", "127"),
        ]))
        .unwrap();
        let mut v = d.start(60, 127, 1000.);
        let mut cc = neutral_cc();
        cc[7] = 0.;
        assert_eq!(v.next(&d, [1.; 2], &cc, 0., 1.), [0.; 2]);
        cc[7] = 0.5;
        cc[10] = 0.5;
        assert!((v.next(&d, [1.; 2], &cc, 0., 1.)[0] - 0.5 * 0.5f32.sqrt()).abs() < 1e-6);
    }
    #[test]
    fn variable_product_and_cross_lfo_follow_documented_equations() {
        let d = RegionDsp::compile(&ops(&[
            ("var01_mod", "mult"),
            ("var01_oncc1", "1"),
            ("var01_oncc131", "1"),
            ("var01_cutoff", "6000"),
            ("lfo01_phase", "0.25"),
            ("lfo01_wave", "1"),
            ("lfo02_freq", "2"),
            ("lfo01_freq_lfo2_oncc2", "4"),
        ]))
        .unwrap();
        let mut cc = [0.; 144];
        cc[1] = 0.5;
        cc[131] = 0.25;
        assert_eq!(d.variables[0].value(&cc) * d.variables[0].cutoff, 750.);
        let mut v = d.start(60, 127, 1000.);
        let mut cc = neutral_cc();
        cc[2] = 1.;
        v.next(&d, [1.; 2], &cc, 0., 1.);
        assert!((v.phases[1] - 0.006).abs() < 1e-8);
        assert!((v.phases[0] - 0.25 - d.lfos[0].freq.base as f64 / 1000.).abs() < 1e-8);
        assert!(
            RegionDsp::compile(&ops(&[("lfo01_freq_lfo2", "1"), ("lfo02_freq_lfo1", "1")]))
                .is_err()
        );
    }
    #[test]
    fn implicit_channel_gains_apply_once() {
        let d = RegionDsp::compile(&Ops::new()).unwrap();
        let mut v = d.start(60, 127, 1000.);
        let mut cc = neutral_cc();
        cc[7] = 0.5;
        cc[11] = 0.5;
        assert!((v.next(&d, [1.; 2], &cc, 0., 1.)[0] - 0.0625).abs() < 1e-6);
        let d = RegionDsp::compile(&ops(&[("amplitude_oncc7", "100")])).unwrap();
        let mut v = d.start(60, 127, 1000.);
        assert!((v.next(&d, [1.; 2], &cc, 0., 1.)[0] - 0.125).abs() < 1e-6);
    }
    #[test]
    fn legacy_decay_reaches_sustain_before_full_zero_decay_duration() {
        let d =
            RegionDsp::compile(&ops(&[("ampeg_decay", "0.2"), ("ampeg_sustain", "25")])).unwrap();
        let mut v = d.start(60, 127, 1000.);
        for _ in 0..40 {
            v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        }
        assert!((v.env[0].value - 0.25).abs() < 1e-6);
    }
    #[test]
    fn dynamic_sustain_follows_cc_and_static_sustain_latches() {
        for dynamic in ["0", "1"] {
            let d = RegionDsp::compile(&ops(&[
                ("ampeg_dynamic", dynamic),
                ("ampeg_sustain_oncc1", "-100"),
            ]))
            .unwrap();
            let mut v = d.start(60, 127, 1000.);
            let mut cc = neutral_cc();
            v.next(&d, [1.; 2], &cc, 0., 1.);
            cc[1] = 1.;
            let y = v.next(&d, [1.; 2], &cc, 0., 1.);
            assert_eq!(y[0], if dynamic == "1" { 0. } else { 1. });
        }
    }
    #[test]
    fn choke_is_cumulative_and_callback_state_has_no_heap() {
        let d = RegionDsp::compile(&ops(&[("ampeg_release", "100")])).unwrap();
        let mut v = d.start(60, 127, 1000.);
        v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        v.choke();
        for _ in 0..120 {
            v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        }
        assert!(v.finished());
        assert!(std::mem::size_of::<VoiceDsp>() < 4096);
        assert!(d.lfo_order.is_empty());
    }
    #[test]
    fn zero_sustain_envelope_finishes_without_note_off() {
        let d = RegionDsp::compile(&ops(&[
            ("ampeg_decay", "0.004"),
            ("ampeg_sustain", "0"),
            ("ampeg_release", "0.002"),
        ]))
        .unwrap();
        let mut v = d.start(60, 127, 1000.);
        for _ in 0..10 {
            v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        }
        assert!(v.finished());
    }
    #[test]
    fn triangle_phase_agrees_with_sine_quadrants() {
        for (p, target) in [(0., 0.), (0.25, 1.), (0.5, 0.), (0.75, -1.), (1., 0.)] {
            assert!((wave(0, p) - target).abs() < 1e-6);
            assert!((wave(1, p) - target).abs() < 1e-6);
        }
    }
    #[test]
    fn scoped_levels_gain_alias_and_pitch_cc_are_additive() {
        let d = RegionDsp::compile(&ops(&[
            ("volume", "-6"),
            ("group_volume", "-6"),
            ("master_volume", "-6"),
            ("gain_cc1", "12"),
            ("group_tune", "-20"),
            ("pitch_cc2", "100"),
        ]))
        .unwrap();
        let mut v = d.start(60, 127, 1000.);
        let mut cc = neutral_cc();
        cc[1] = 1.;
        cc[2] = 0.5;
        assert!((v.next(&d, [1.; 2], &cc, 0., 1.)[0] - db(-6.)).abs() < 1e-6);
        assert_eq!(v.pitch_cents(&d, &cc), 30.);
        assert!(supports_opcode("gain_cc25"));
        assert!(supports_opcode("pitch_cc20"));
    }
    #[test]
    fn numbered_envelope_early_release_skips_attack_to_release_segment() {
        let d = RegionDsp::compile(&ops(&[
            ("ampeg_release", "1"),
            ("eg1_level0", "0"),
            ("eg1_level1", "1"),
            ("eg1_time1", "1"),
            ("eg1_sustain", "1"),
            ("eg1_level2", "0"),
            ("eg1_time2", "0.004"),
            ("eg1_pitch", "100"),
        ]))
        .unwrap();
        let mut v = d.start(60, 127, 1000.);
        for _ in 0..10 {
            v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        }
        assert!(v.pitch_cents(&d, &neutral_cc()) > 0.);
        v.release();
        for _ in 0..5 {
            v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        }
        assert_eq!(v.pitch_cents(&d, &neutral_cc()), 0.);
    }
    #[test]
    fn fractional_and_negative_velocity_tracking_are_gain_blends() {
        let v = 64f32 / 127.;
        for (tracking, expected) in [
            ("20", 1. - 0.2 * (1. - v * v)),
            ("50", 1. - 0.5 * (1. - v * v)),
            ("-100", 1. - v * v),
        ] {
            let d = RegionDsp::compile(&ops(&[("amp_veltrack", tracking)])).unwrap();
            let mut voice = d.start(60, 64, 1000.);
            assert!((voice.next(&d, [1.; 2], &neutral_cc(), 0., 1.)[0] - expected).abs() < 1e-6);
        }
    }
    #[test]
    fn velocity_cc_modifiers_latch_on_note_and_pitch_depth_is_cents() {
        let d = RegionDsp::compile(&ops(&[
            ("amp_veltrack", "0"),
            ("amp_veltrack_oncc99", "100"),
            ("pitch_veltrack", "8"),
        ]))
        .unwrap();
        let mut v = d.start(60, 64, 1000.);
        let cc = neutral_cc();
        let cents = 8. * 64. / 127.;
        assert!((v.pitch_cents(&d, &cc) - cents).abs() < 1e-6);
        assert_eq!(v.next(&d, [1.; 2], &cc, 0., 1.)[0], 1.);
        let mut changed = cc;
        changed[99] = 1.;
        assert_eq!(v.next(&d, [1.; 2], &changed, 0., 1.)[0], 1.);
        let mut new = d.start(60, 64, 1000.);
        assert!((new.next(&d, [1.; 2], &changed, 0., 1.)[0] - (64f32 / 127.).powi(2)).abs() < 1e-6);
        assert!(supports_opcode("amp_veltrack_oncc99"));
        assert!(supports_opcode("pitch_veltrack"));
    }
    fn response(mut process: impl FnMut([f32; 2]) -> [f32; 2], freq: f32) -> f32 {
        let mut input = 0.;
        let mut output = 0.;
        for i in 0..9600 {
            let x = (TAU * freq * i as f32 / 48000.).sin();
            let y = process([x, x])[0];
            if i >= 4800 {
                input += x * x;
                output += y * y;
            }
        }
        (output / input).sqrt()
    }
    #[test]
    fn filter_cutoff_and_eq_center_have_expected_frequency_response() {
        for kind in [2, 3] {
            let mut filter = Biquad::default();
            let gain = response(|x| filter.filter(x, kind, 1000., 0., 48000.), 1000.);
            // SFZ resonance is dB gain at the cutoff, so zero resonance means
            // Q=1 and unity at fc, not Butterworth Q=1/sqrt(2).
            assert!((20. * gain.log10()).abs() < 0.02);
        }
        let mut eq = Biquad::default();
        let gain = response(|x| eq.eq(x, 1000., 1., 6., 48000.), 1000.);
        assert!((20. * gain.log10() - 6.).abs() < 0.02);
        let mut filter = Biquad::default();
        let low = response(|x| filter.filter(x, 2, 1000., 0., 48000.), 100.);
        assert!(20. * low.log10() > -0.1);
        let mut filter = Biquad::default();
        let high = response(|x| filter.filter(x, 2, 1000., 0., 48000.), 10000.);
        assert!(20. * high.log10() < -40.);
    }
    #[test]
    fn secondary_lfo_does_not_reset_at_main_wave_wrap() {
        let d = RegionDsp::compile(&ops(&[
            ("lfo01_freq", "1"),
            ("lfo01_wave", "1"),
            ("lfo01_wave2", "1"),
            ("lfo01_ratio2", "0.5"),
            ("lfo01_scale2", "1"),
        ]))
        .unwrap();
        let mut v = d.start(60, 127, 1000.);
        for _ in 0..1251 {
            v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        }
        let expected = wave(1, 1.25) + wave(1, 0.625);
        assert!((v.lfo_values[0] - expected).abs() < 0.0001);
    }
    #[test]
    fn filter_resonance_is_cutoff_gain_db_and_velocity_tracks_from_zero() {
        for resonance in [0., 6., 18.] {
            let mut f = Biquad::default();
            let gain = response(|x| f.filter(x, 2, 1000., resonance, 48000.), 1000.);
            assert!((20. * gain.log10() - resonance).abs() < 0.025);
        }
        let d = RegionDsp::compile(&ops(&[
            ("amp_veltrack", "0"),
            ("cutoff", "120"),
            ("fil_veltrack", "4800"),
        ]))
        .unwrap();
        let mut voice = d.start(60, 127, 48000.);
        let gain = response(|x| voice.next(&d, x, &neutral_cc(), 0., 1.), 1920.);
        assert!((20. * gain.log10()).abs() < 0.02);
    }
    #[test]
    fn explicit_note_latch_preserves_same_offset_event_order_without_time_advance() {
        let d = RegionDsp::compile(&ops(&[
            ("amp_veltrack", "0"),
            ("amp_veltrack_oncc1", "100"),
            ("pitch_veltrack_oncc1", "100"),
            ("ampeg_attack_oncc1", "0.01"),
            ("lfo01_phase_oncc1", "0.25"),
            ("volume_oncc2", "6"),
        ]))
        .unwrap();
        let mut cc = neutral_cc();
        let mut early = d.start(60, 64, 1000.);
        early.latch(&d, &cc, &[0.; 16]);
        cc[1] = 1.;
        let mut late = d.start(60, 64, 1000.);
        late.latch(&d, &cc, &[0.; 16]);
        assert_eq!(early.time, 0.);
        assert_eq!(late.time, 0.);
        assert_eq!(early.env[0].stage, 0);
        assert_eq!(late.env[0].stage, 0);
        assert_eq!(early.env[0].elapsed, 0.);
        assert_eq!(late.env[0].elapsed, 0.);
        assert_eq!(early.phases[0], 0.);
        assert_eq!(late.phases[0], 0.25);
        assert_eq!(early.pitch_cents(&d, &cc), 0.);
        assert!((late.pitch_cents(&d, &cc) - 100. * 64. / 127.).abs() < 1e-6);
        cc[2] = 1.;
        let gain = db(6.);
        let early_y = early.next(&d, [1.; 2], &cc, 0., 1.)[0];
        let late_y = late.next(&d, [1.; 2], &cc, 0., 1.)[0];
        assert!((early_y - gain).abs() < 1e-6);
        assert!((late_y - gain * (64f32 / 127.).powi(2) * 0.1).abs() < 1e-6);
        // Re-latching a cloned checkpoint must not reset any envelope or phase.
        let mut restored = early.clone();
        restored.latch(&d, &cc, &[0.; 16]);
        assert_eq!(restored.time, early.time);
        assert_eq!(restored.phases, early.phases);
        assert_eq!(restored.env[0].elapsed, early.env[0].elapsed);
    }
    #[test]
    fn dynamic_envelope_retains_live_controls_after_explicit_latch() {
        let d = RegionDsp::compile(&ops(&[
            ("ampeg_dynamic", "1"),
            ("ampeg_attack_oncc1", "0.01"),
        ]))
        .unwrap();
        let mut cc = neutral_cc();
        let mut v = d.start(60, 127, 1000.);
        v.latch(&d, &cc, &[0.; 16]);
        cc[1] = 1.;
        assert!((v.next(&d, [1.; 2], &cc, 0., 1.)[0] - 0.1).abs() < 1e-6);
    }
    #[test]
    fn velocity_is_squared_and_expression_is_multiplicative() {
        let d = RegionDsp::compile(&Ops::new()).unwrap();
        let mut v = d.start(60, 64, 1000.);
        let y = v.next(&d, [1., 1.], &neutral_cc(), 0., 0.5);
        assert!((y[0] - 0.5 * (64f32 / 127.).powi(2)).abs() < 1e-6);
    }
    #[test]
    fn envelope_release_finishes_at_configured_time() {
        let d = RegionDsp::compile(&ops(&[
            ("ampeg_release", "0.01"),
            ("ampeg_release_shape", "0"),
        ]))
        .unwrap();
        let mut v = d.start(60, 127, 1000.);
        v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        v.release();
        for _ in 0..11 {
            v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        }
        assert!(v.finished());
    }
    #[test]
    fn sustain_and_attack_have_percent_and_second_units() {
        let d = RegionDsp::compile(&ops(&[("ampeg_attack", "0.004"), ("ampeg_sustain", "25")]))
            .unwrap();
        let mut v = d.start(60, 127, 1000.);
        assert!((v.next(&d, [1.; 2], &neutral_cc(), 0., 1.)[0] - 0.25).abs() < 1e-6);
        for _ in 0..5 {
            v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
        }
        assert!((v.env[0].value - 0.25).abs() < 1e-6);
    }
    #[test]
    fn custom_controller_curve_interpolates_and_unknown_curve_errors() {
        let mut c = BTreeMap::new();
        c.insert(12, vec![0.25; 128]);
        let d = RegionDsp::compile_with_curves(
            &ops(&[("volume_oncc7", "20"), ("volume_curvecc7", "12")]),
            &c,
        )
        .unwrap();
        assert_eq!(d.volume.get(&[1.; 144]), 5.);
        assert!(
            RegionDsp::compile(&ops(&[("volume_oncc7", "20"), ("volume_curvecc7", "12")])).is_err()
        );
    }
    #[test]
    fn filters_remain_finite_and_attenuate_dc() {
        let d = RegionDsp::compile(&ops(&[
            ("cutoff", "200"),
            ("fil_type", "hpf_2p"),
            ("cutoff2", "1000"),
            ("fil2_type", "lpf_1p"),
        ]))
        .unwrap();
        let mut v = d.start(60, 127, 48000.);
        let mut y = [0.; 2];
        for _ in 0..48000 {
            y = v.next(&d, [1.; 2], &neutral_cc(), 0., 1.);
            assert!(y.iter().all(|x| x.is_finite()));
        }
        assert!(y[0].abs() < 1e-4);
    }
    #[test]
    fn width_zero_makes_mono_and_pan_routes_work() {
        let d = RegionDsp::compile(&ops(&[("width", "0"), ("pan_oncc10", "100")])).unwrap();
        let mut v = d.start(60, 127, 1000.);
        let mut cc = neutral_cc();
        cc[10] = 1.;
        let y = v.next(&d, [1., -1.], &cc, 0., 1.);
        assert_eq!(y, [0.; 2]);
        let y = v.next(&d, [1., 1.], &cc, 0., 1.);
        assert_eq!(y[0], 0.);
        assert!((y[1] - 2f32.sqrt()).abs() < 1e-6);
    }
    #[test]
    fn registry_rejects_unverified_and_compile_rejects_nonfinite() {
        assert!(supports_opcode("var01_mod"));
        assert!(supports_opcode("lfo03_freq_lfo2_oncc117"));
        assert!(supports_opcode("ampeg_decay_curvecc64"));
        assert!(RegionDsp::compile(&ops(&[("volume", "NaN")])).is_err());
        assert!(RegionDsp::compile(&ops(&[("fil_type", "comb"), ("cutoff", "100")])).is_err());
    }
    #[test]
    fn phase_and_pitch_modulation_repeat_deterministically() {
        let d = RegionDsp::compile(&ops(&[
            ("lfo01_freq", "10"),
            ("lfo01_pitch", "100"),
            ("lfo01_wave", "1"),
        ]))
        .unwrap();
        let mut a = d.start(60, 127, 1000.);
        let mut b = a.clone();
        for _ in 0..100 {
            assert_eq!(
                a.next(&d, [1.; 2], &neutral_cc(), 0., 1.),
                b.next(&d, [1.; 2], &neutral_cc(), 0., 1.)
            );
            assert_eq!(
                a.pitch_cents(&d, &neutral_cc()),
                b.pitch_cents(&d, &neutral_cc())
            );
        }
    }
    #[test]
    fn unchanged_coefficient_and_gain_caches_preserve_exact_pcm() {
        let mut o = Ops::new();
        for (k, v) in [
            ("cutoff", "1200"),
            ("cutoff_oncc1", "2400"),
            ("resonance", "6"),
            ("eq1_gain", "0"),
            ("eq1_gain_oncc1", "4"),
            ("eq1_freq", "700"),
            ("volume_oncc1", "3"),
            ("pan_oncc1", "40"),
            ("lfo01_freq", "2"),
            ("lfo02_freq", "4"),
            ("lfo01_freq_lfo2_oncc1", "1"),
            ("lfo02_volume_oncc1", "2"),
            ("lfo02_pitch_oncc1", "20"),
            ("pitch_oncc1", "8"),
            ("pitcheg_depth_oncc1", "30"),
            ("pitch_veltrack", "5"),
            ("eg1_pitch", "100"),
            ("eg1_level0", "-1"),
            ("eg1_level1", "1"),
            ("eg1_time1", "0.01"),
            ("lfo02_eq1gain_oncc1", "3"),
            ("ampeg_dynamic", "1"),
            ("ampeg_sustain_oncc1", "-20"),
            ("fillfo_freq", "3"),
            ("fillfo_depth_oncc1", "300"),
            ("fileg_attack", "0.01"),
            ("fileg_depth_oncc1", "500"),
            ("ampeg_release", "0.02"),
        ] {
            o.insert(k.into(), v.into());
        }
        let d = RegionDsp::compile(&o).unwrap();
        let mut full = d.clone();
        full.envelope_active = [true; 3];
        full.constant_pitch = None;
        let mut cached = d.start(60, 100, 48000.);
        let mut uncached = cached.clone();
        uncached.force_destination_paths = true;
        let mut cc = [0.; 128];
        cc[7] = 1.;
        cc[11] = 1.;
        for frame in 0..4096 {
            if frame == 1024 {
                cc[1] = 0.6;
            }
            if frame == 3072 {
                cc[1] = 0.;
            }
            if frame == 2048 {
                cached.release();
                uncached.release();
            }
            for b in uncached.filters.iter_mut().chain(uncached.eqs.iter_mut()) {
                b.coefficient_valid = false;
            }
            uncached.filter_hz_cache = [None; 2];
            uncached.volume_cache.0 = f32::NAN;
            uncached.pan_cache.0 = f32::NAN;
            assert_eq!(
                cached
                    .pitch_cents_with_generation(
                        &d,
                        &cc,
                        if frame < 1024 {
                            0
                        } else if frame < 3072 {
                            1
                        } else {
                            2
                        }
                    )
                    .to_bits(),
                uncached.pitch_cents(&full, &cc).to_bits()
            );
            let input = [
                (frame as f32 * 0.017).sin() * 0.2,
                (frame as f32 * 0.013).cos() * 0.1,
            ];
            assert_eq!(
                cached
                    .next_with_generation(
                        &d,
                        input,
                        &cc,
                        0.,
                        1.,
                        if frame < 1024 {
                            0
                        } else if frame < 3072 {
                            1
                        } else {
                            2
                        }
                    )
                    .map(f32::to_bits),
                uncached.next(&full, input, &cc, 0., 1.).map(f32::to_bits)
            );
        }
    }
    #[test]
    fn cross_lfo_source_to_target_clock_matches_aria_measurement() {
        // Actual Sforzando 2.1.2.4 DC volume-LFO positive upcross at .367176871s.
        // Unit sine source: phase(t)=2t+(1-cos(2*pi*t))/(2*pi), crosses 1
        // analytically at .367037252s; the reference sine approximation differs
        // slightly. This asymmetric graph must alter target 2, never source 3.
        let d = RegionDsp::compile(&ops(&[
            ("lfo03_freq", "1"),
            ("lfo03_wave", "1"),
            ("lfo02_freq", "2"),
            ("lfo02_volume", "6"),
            ("lfo03_freq_lfo2_oncc117", "1"),
        ]))
        .unwrap();
        assert_eq!(d.lfos[1].cross[0].0, 2);
        assert!(d.lfos[2].cross.is_empty());
        let mut cc = neutral_cc();
        cc[117] = 1.;
        let mut v = d.start(60, 127, 48000.);
        let mut crossing = 0.;
        for frame in 0..24000 {
            v.next(&d, [1.; 2], &cc, 0., 1.);
            if v.phases[1] >= 1. {
                crossing = (frame + 1) as f64 / 48000.;
                break;
            }
        }
        assert!((crossing - 0.367037252).abs() < 0.00005);
        assert!((crossing - 0.367176871).abs() < 0.0002);
        assert!((v.phases[2] - crossing).abs() < 1e-10);
    }
    #[test]
    fn borrowed_controller_view_matches_materialized_sources_exactly() {
        let o = ops(&[
            ("volume_oncc1", "12"),
            ("volume_oncc131", "3"),
            ("volume_oncc140", "2"),
            ("volume_curvecc1", "4"),
            ("amplitude_oncc7", "100"),
            ("amplitude_oncc135", "20"),
            ("var01_mod", "mult"),
            ("var01_oncc133", "1"),
            ("var01_oncc1", "0.5"),
        ]);
        let d = RegionDsp::compile(&o).unwrap();
        for value in [0., 0.1, 0.5, 1.] {
            let physical = [value; 128];
            let mut virtual_sources = [value; 16];
            virtual_sources[12] = -7.;
            let view = ControllerSources {
                physical: &physical,
                virtual_sources: &virtual_sources,
            };
            let mut materialized = [0.; 144];
            materialized[..128].copy_from_slice(&physical);
            materialized[128..].copy_from_slice(&virtual_sources);
            for i in 0..144 {
                assert_eq!(view[i].to_bits(), materialized[i].to_bits());
            }
            assert_eq!(
                d.volume.get(&view).to_bits(),
                d.volume.get(&materialized).to_bits()
            );
            assert_eq!(
                d.amplitude.product(&view).to_bits(),
                d.amplitude.product(&materialized).to_bits()
            );
            assert_eq!(
                d.variables[0].value(&view).to_bits(),
                d.variables[0].value(&materialized).to_bits()
            );
        }
    }
    #[test]
    fn controller_generation_cache_preserves_latch_and_checkpoint_order() {
        let d = RegionDsp::compile(&ops(&[
            ("amplitude_oncc131", "100"),
            ("volume_oncc1", "3"),
            ("width_oncc1", "-40"),
            ("pan_oncc1", "20"),
            ("amp_veltrack_oncc1", "-50"),
        ]))
        .unwrap();
        let mut cc = neutral_cc();
        let mut virt = [0.; 16];
        virt[3] = 0.5;
        let mut a = d.start(60, 64, 48000.);
        a.latch(&d, &cc, &virt);
        let mut reference = a.clone();
        a.next_with_generation(&d, [0.2, -0.1], &cc, 0., 1., 0);
        reference.next(&d, [0.2, -0.1], &cc, 0., 1.);
        let checkpoint = a.clone();
        cc[1] = 1.;
        for _ in 0..32 {
            assert_eq!(
                a.next_with_generation(&d, [0.2, -0.1], &cc, 0., 1., 1)
                    .map(f32::to_bits),
                reference
                    .next(&d, [0.2, -0.1], &cc, 0., 1.)
                    .map(f32::to_bits)
            );
        }
        let mut restored = checkpoint.clone();
        let mut full = checkpoint;
        cc[1] = 0.;
        assert_eq!(
            restored
                .next_with_generation(&d, [0.2, -0.1], &cc, 0., 1., 0)
                .map(f32::to_bits),
            full.next(&d, [0.2, -0.1], &cc, 0., 1.).map(f32::to_bits)
        );
        assert_eq!(a.velocity_gain.to_bits(), reference.velocity_gain.to_bits());
    }
    #[test]
    fn controller_trigger_bypasses_only_note_velocity_amplitude_gain() {
        let d = RegionDsp::compile(&ops(&[
            ("pitch_veltrack", "100"),
            ("ampeg_vel2attack", "0.01"),
        ]))
        .unwrap();
        let cc = neutral_cc();
        let mut v = d.start(60, 64, 48000.);
        v.latch_controller_trigger(&d, &cc, &[0.; 16]);
        assert_eq!(v.velocity_gain, 1.);
        assert_eq!(v.pitch_velocity, (64f32 / 127.) * 100.);
        assert_eq!(v.env[0].times[1], (64f32 / 127.) * 0.01);
    }
}
