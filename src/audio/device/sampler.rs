//! Prepared WAV zones, velocity layers and round-robin selection. All sample I/O is off-thread.
use super::*;
use crate::model::SampleZone;
struct Zone {
    source: SampleZone,
    audio: Vec<[f32; 2]>,
    rate: f64,
}
#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    id: u64,
    zone: usize,
    pos: f64,
    step: f64,
    gain: f32,
    envelope: f32,
    releasing: bool,
}
pub struct Sampler {
    zones: Vec<Zone>,
    voices: [Voice; 32],
    rate: f64,
    next: usize,
    gain: f32,
    attack: f32,
    release: f32,
    count: u64,
    token: u64,
}
impl Sampler {
    pub fn new(d: &model::Device, c: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let zones = d
            .sample
            .as_ref()
            .ok_or(DeviceError::InvalidConfig("sampler needs zones"))?;
        let result = (|| -> anyhow::Result<Vec<Zone>> {
            let mut loaded = Vec::new();
            let mut total = 0usize;
            for source in zones {
                let reader = hound::WavReader::open(&source.path)?;
                let spec = reader.spec();
                anyhow::ensure!(
                    spec.channels == 1 || spec.channels == 2,
                    "samples must be mono or stereo WAV"
                );
                total = total.saturating_add(reader.len() as usize);
                anyhow::ensure!(total <= 128 * 1024 * 1024, "sample device exceeds 512 MiB");
                let audio = match spec.sample_format {
                    hound::SampleFormat::Float => reader
                        .into_samples::<f32>()
                        .collect::<Result<Vec<_>, _>>()?,
                    hound::SampleFormat::Int => reader
                        .into_samples::<i32>()
                        .map(|v| v.map(|v| v as f32 / (1u64 << (spec.bits_per_sample - 1)) as f32))
                        .collect::<Result<Vec<_>, _>>()?,
                };
                anyhow::ensure!(
                    audio.iter().all(|v| v.is_finite()),
                    "non-finite sample data"
                );
                let audio: Vec<_> = audio
                    .chunks_exact(spec.channels as usize)
                    .map(|f| [f[0], *f.get(1).unwrap_or(&f[0])])
                    .collect();
                anyhow::ensure!(!audio.is_empty(), "sample is empty");
                if let Some([a, b]) = source.loop_seconds {
                    anyhow::ensure!(
                        a >= 0.0 && b > a && b * spec.sample_rate as f64 <= audio.len() as f64,
                        "invalid sample loop"
                    );
                }
                loaded.push(Zone {
                    source: source.clone(),
                    audio,
                    rate: spec.sample_rate as f64,
                });
            }
            Ok(loaded)
        })()
        .map_err(|e| {
            eprintln!("sample {}: {e:#}", d.id);
            DeviceError::InvalidConfig("cannot prepare sample zones")
        })?;
        let mut s = Self {
            zones: result,
            voices: [Voice::default(); 32],
            rate: c.sample_rate as f64,
            next: 0,
            gain: 1.0,
            attack: 2.0,
            release: 35.0,
            count: 0,
            token,
        };
        for (k, v) in &d.params {
            s.set_parameter(k, *v)?;
        }
        Ok(s)
    }
    fn event(&mut self, e: DeviceEventKind) {
        match e {
            DeviceEventKind::NoteOn {
                note_id,
                key,
                velocity,
                elapsed_frames,
                ..
            } => {
                let matches = |z: &&Zone| {
                    key >= z.source.keys[0]
                        && key <= z.source.keys[1]
                        && velocity >= z.source.velocity[0]
                        && velocity <= z.source.velocity[1]
                };
                let count = self.zones.iter().filter(matches).count();
                if count == 0 {
                    return;
                }
                let zone = self
                    .zones
                    .iter()
                    .enumerate()
                    .filter(|(_, z)| matches(z))
                    .nth(self.next % count)
                    .unwrap()
                    .0;
                self.next = self.next.wrapping_add(1);
                let z = &self.zones[zone];
                let step =
                    z.rate / self.rate * 2.0f64.powf((key as f64 - z.source.root as f64) / 12.0);
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
                self.voices[i] = Voice {
                    active: true,
                    id: note_id,
                    zone,
                    pos: z.source.offset_seconds * z.rate + elapsed_frames as f64 * step,
                    step,
                    gain: velocity,
                    envelope: if elapsed_frames > 0 { 1.0 } else { 0.0 },
                    releasing: false,
                };
            }
            DeviceEventKind::NoteOff { note_id, .. } => {
                for v in &mut self.voices {
                    if v.id == note_id && !self.zones[v.zone].source.one_shot {
                        v.releasing = true;
                    }
                }
            }
            DeviceEventKind::Flush => self.reset(),
            _ => {}
        }
    }
}
impl DeviceProcessor for Sampler {
    fn kind(&self) -> model::DeviceKind {
        model::DeviceKind::Sampler
    }
    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            instance_token: self.token,
            process_count: self.count,
            gain_reduction_db: 0.0,
            latency_samples: 0,
            tail_samples: (self.rate * self.release as f64 / 1000.) as u32,
            is_plugin: false,
        }
    }
    fn reset(&mut self) {
        self.voices.fill(Voice::default());
    }
    fn set_parameter(&mut self, k: &str, v: f32) -> Result<(), DeviceError> {
        match k {
            "gain_db" => {
                self.gain =
                    10.0f32.powf(parameter_value(self.kind(), "gain_db", v, -120., 24.)? / 20.)
            }
            "attack_ms" => self.attack = parameter_value(self.kind(), "attack_ms", v, 0., 5000.)?,
            "release_ms" => {
                self.release = parameter_value(self.kind(), "release_ms", v, 0., 10000.)?
            }
            _ => return Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
        Ok(())
    }
    fn process(
        &mut self,
        c: ProcessContext,
        events: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) -> Result<(), DeviceError> {
        let mut event = 0;
        for frame in 0..c.frames {
            while event < events.len() && events[event].offset as usize == frame {
                self.event(events[event].kind);
                event += 1;
            }
            let mut pair = [0.0; 2];
            for v in &mut self.voices {
                if !v.active {
                    continue;
                }
                let z = &self.zones[v.zone];
                if !v.releasing {
                    if let Some([a, b]) = z.source.loop_seconds {
                        let a = a * z.rate;
                        let b = b * z.rate;
                        if v.pos >= b {
                            v.pos = a + (v.pos - a) % (b - a);
                        }
                    }
                }
                if v.pos >= z.audio.len() as f64 || v.pos < 0.0 {
                    v.active = false;
                    continue;
                }
                if v.releasing {
                    v.envelope = (v.envelope
                        - 1.0 / (self.rate as f32 * self.release / 1000.).max(1.))
                    .max(0.0);
                } else {
                    v.envelope = (v.envelope
                        + 1.0 / (self.rate as f32 * self.attack / 1000.).max(1.))
                    .min(1.0);
                }
                if v.envelope == 0.0 && v.releasing {
                    v.active = false;
                    continue;
                }
                let i = v.pos.floor() as isize;
                let f = (v.pos - i as f64) as f32;
                for ch in 0..2 {
                    let sample =
                        |n: isize| z.audio[n.clamp(0, z.audio.len() as isize - 1) as usize][ch];
                    let (a, b, c, d) = (sample(i - 1), sample(i), sample(i + 1), sample(i + 2));
                    let y = b + 0.5
                        * f
                        * (c - a + f * (2. * a - 5. * b + 4. * c - d + f * (3. * (b - c) + d - a)));
                    pair[ch] += y * v.envelope * v.gain * self.gain;
                }
                v.pos += v.step;
            }
            l[frame] = pair[0];
            r[frame] = pair[1];
        }
        self.count += 1;
        Ok(())
    }
}
