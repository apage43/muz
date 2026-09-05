//! Fixed parallel schedules. Authoring functions expose controls; no DSP closures run here.
use super::*;
use crate::audio::automation::DelayLine;
use std::collections::BTreeMap;

struct Branch {
    devices: Vec<Box<dyn DeviceProcessor>>,
    delay: DelayLine,
    left: Vec<f32>,
    right: Vec<f32>,
}
struct Target {
    branch: usize,
    device: usize,
    parameter: String,
}
struct Modulation {
    target: Target,
    settings: model::RackModulation,
    envelope: f32,
    attack: f32,
    release: f32,
}
pub(super) struct RackProcessor {
    branches: Vec<Branch>,
    expose: BTreeMap<String, Target>,
    mods: Vec<Modulation>,
    dry: DelayLine,
    mix: f32,
    gain: f32,
    config: AudioConfig,
    state: DeviceDebugState,
}
fn invalid() -> DeviceError {
    DeviceError::InvalidConfig("invalid rack topology, exposed control or modulation")
}
fn target(path: &str, branches: &mut [Branch]) -> Result<Target, DeviceError> {
    let mut parts = path.splitn(3, '.');
    let b = parts
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(invalid)?;
    let d = parts
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(invalid)?;
    let parameter = parts
        .next()
        .filter(|p| !p.is_empty())
        .ok_or_else(invalid)?
        .to_owned();
    if parameter == "lookahead_ms" {
        return Err(DeviceError::InvalidConfig(
            "latency-changing controls cannot be exposed/modulated; change the rack source instead",
        ));
    }
    if branches.get(b).and_then(|b| b.devices.get(d)).is_none() {
        return Err(invalid());
    }
    Ok(Target {
        branch: b,
        device: d,
        parameter,
    })
}
impl RackProcessor {
    pub fn new(
        device: &model::Device,
        config: AudioConfig,
        token: u64,
    ) -> Result<Self, DeviceError> {
        let rack = device.rack.as_ref().ok_or_else(invalid)?;
        let mut branches = Vec::new();
        let mut latency = 0;
        let mut tail = 0;
        for chain in &rack.branches {
            let devices = chain
                .iter()
                .map(|d| create_processor(d, config))
                .collect::<Result<Vec<_>, _>>()?;
            latency = latency.max(
                devices
                    .iter()
                    .map(|d| d.debug_state().latency_samples)
                    .sum::<u32>(),
            );
            tail = tail.max(
                devices
                    .iter()
                    .map(|d| d.debug_state().tail_samples as u64)
                    .sum::<u64>()
                    .min(u32::MAX as u64) as u32,
            );
            branches.push(Branch {
                devices,
                delay: DelayLine::default(),
                left: vec![0.; config.max_frames],
                right: vec![0.; config.max_frames],
            });
        }
        for b in &mut branches {
            b.delay.set_length(
                (latency
                    - b.devices
                        .iter()
                        .map(|d| d.debug_state().latency_samples)
                        .sum::<u32>()) as usize,
            );
        }
        let mut expose = BTreeMap::new();
        for (key, path) in &rack.expose {
            expose.insert(key.clone(), target(path, &mut branches)?);
        }
        let mut mods = Vec::new();
        let mut owned = std::collections::BTreeSet::new();
        for m in &rack.modulate {
            if !owned.insert(&m.target)
                || m.min > m.max
                || [
                    m.base,
                    m.depth,
                    m.rate_hz,
                    m.follower,
                    m.attack_ms,
                    m.release_ms,
                    m.min,
                    m.max,
                ]
                .iter()
                .any(|v| !v.is_finite())
                || !(0.0..=100.).contains(&m.rate_hz)
                || m.attack_ms <= 0.
                || m.release_ms <= 0.
            {
                return Err(invalid());
            }
            let t = target(&m.target, &mut branches)?;
            let p = &mut branches[t.branch].devices[t.device];
            p.set_parameter(&t.parameter, m.min)?;
            p.set_parameter(&t.parameter, m.max)?;
            mods.push(Modulation {
                target: t,
                settings: m.clone(),
                envelope: 0.,
                attack: (-1. / (m.attack_ms * 0.001 * config.sample_rate)).exp(),
                release: (-1. / (m.release_ms * 0.001 * config.sample_rate)).exp(),
            });
        }
        // An exposed value and a follower cannot independently own the same control.
        if rack.expose.values().any(|p| owned.contains(p)) {
            return Err(invalid());
        }
        let mut dry = DelayLine::default();
        dry.set_length(latency as usize);
        let mut out = Self {
            branches,
            expose,
            mods,
            dry,
            mix: 1.,
            gain: 1.,
            config,
            state: DeviceDebugState {
                instance_token: token,
                process_count: 0,
                gain_reduction_db: 0.,
                latency_samples: latency,
                tail_samples: tail,
                restart_flags: 0,
                is_plugin: false,
            },
        };
        for (k, v) in &device.params {
            out.set_parameter(k, *v)?;
        }
        Ok(out)
    }
    fn run(
        &mut self,
        ctx: ProcessContext,
        left: &mut [f32],
        right: &mut [f32],
        dl: &[f32],
        dr: &[f32],
    ) -> Result<(), DeviceError> {
        if ctx.frames > self.config.max_frames {
            return Err(invalid());
        }
        for b in &mut self.branches {
            b.left[..ctx.frames].copy_from_slice(&left[..ctx.frames]);
            b.right[..ctx.frames].copy_from_slice(&right[..ctx.frames]);
        }
        if self.mods.is_empty() {
            for b in &mut self.branches {
                for d in &mut b.devices {
                    d.process(
                        ctx,
                        &[],
                        &mut b.left[..ctx.frames],
                        &mut b.right[..ctx.frames],
                    )?;
                }
            }
        } else {
            for i in 0..ctx.frames {
                let level = if dl.is_empty() {
                    left[i].abs().max(right[i].abs())
                } else {
                    dl[i].abs().max(dr[i].abs())
                };
                let seconds = (ctx.transport.project_frame + i as f64) / ctx.transport.sample_rate;
                for m in &mut self.mods {
                    let c = if level > m.envelope {
                        m.attack
                    } else {
                        m.release
                    };
                    m.envelope = level + c * (m.envelope - level);
                    let s = &m.settings;
                    let value = (s.base
                        + s.depth
                            * (std::f64::consts::TAU * seconds * s.rate_hz as f64).sin() as f32
                        + s.follower * m.envelope)
                        .clamp(s.min, s.max);
                    self.branches[m.target.branch].devices[m.target.device]
                        .set_parameter(&m.target.parameter, value)?;
                }
                let mut one = ctx;
                one.frames = 1;
                one.block_start_sample += i as u64;
                one.transport.project_frame += i as f64;
                for b in &mut self.branches {
                    for d in &mut b.devices {
                        d.process(one, &[], &mut b.left[i..i + 1], &mut b.right[i..i + 1])?;
                    }
                }
            }
        }
        for i in 0..ctx.frames {
            let dry = self.dry.sample(left[i], right[i]);
            let mut wet = [0.; 2];
            for b in &mut self.branches {
                let p = b.delay.sample(b.left[i], b.right[i]);
                wet[0] += p[0];
                wet[1] += p[1];
            }
            left[i] = (dry[0] * (1. - self.mix) + wet[0] * self.mix) * self.gain;
            right[i] = (dry[1] * (1. - self.mix) + wet[1] * self.mix) * self.gain;
        }
        self.state.process_count += 1;
        Ok(())
    }
}
impl DeviceProcessor for RackProcessor {
    fn kind(&self) -> model::DeviceKind {
        model::DeviceKind::Rack
    }
    fn debug_state(&self) -> DeviceDebugState {
        let mut state = self.state;
        for d in self.branches.iter().flat_map(|b| &b.devices) {
            let child = d.debug_state();
            state.restart_flags |= child.restart_flags;
            state.is_plugin |= child.is_plugin;
        }
        state
    }
    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "mix" => self.mix = parameter_value(self.kind(), "mix", value, 0., 1.)?,
            "gain_db" => {
                self.gain =
                    10f32.powf(parameter_value(self.kind(), "gain_db", value, -120., 24.)? / 20.)
            }
            _ => {
                let t = self
                    .expose
                    .get(name)
                    .ok_or(DeviceError::UnknownParameter { kind: self.kind() })?;
                self.branches[t.branch].devices[t.device].set_parameter(&t.parameter, value)?;
            }
        };
        Ok(())
    }
    fn reset(&mut self) {
        self.dry.reset();
        for b in &mut self.branches {
            b.delay.reset();
            for d in &mut b.devices {
                d.reset();
            }
        }
        for m in &mut self.mods {
            m.envelope = 0.;
        }
    }
    fn process(
        &mut self,
        ctx: ProcessContext,
        _: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.run(ctx, l, r, &[], &[])
    }
    fn process_sidechain(
        &mut self,
        ctx: ProcessContext,
        _: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
        dl: &[f32],
        dr: &[f32],
    ) -> Result<(), DeviceError> {
        self.run(ctx, l, r, dl, dr)
    }
}
