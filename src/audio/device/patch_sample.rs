//! Zone selection and bounded readers for programmable voices.
use crate::model::SampleZone;
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
pub(super) type Assets = BTreeMap<PathBuf, (f32, Arc<[[f32; 2]]>)>;
struct Zone {
    source: SampleZone,
    rate: f32,
    audio: Arc<[[f32; 2]]>,
    gain: f32,
}
pub(super) struct Reader {
    zones: Vec<Zone>,
    channel: u8,
    offset: f64,
    end: Option<f64>,
    crossfade: f64,
}
#[derive(Clone, Copy, Default)]
pub(super) struct State {
    zone: usize,
    pos: f64,
    step: f64,
    pitch: f32,
    started: bool,
    pub done: bool,
}
pub(super) fn asset(
    path: &std::path::Path,
    assets: &mut Assets,
    frames: &mut usize,
    budget: usize,
) -> Result<(f32, Arc<[[f32; 2]]>)> {
    let path = crate::assets::resolve(path)?;
    if let Some(a) = assets.get(&path) {
        return Ok(a.clone());
    }
    let (info, audio) = crate::audio_file::load_shared(&path, budget.saturating_sub(*frames))
        .with_context(|| {
            format!(
                "patch sample_budget_frames={budget}, already decoded {} frames, while loading {}",
                *frames,
                path.display()
            )
        })?;
    ensure!(!audio.is_empty(), "sample is empty");
    *frames += audio.len();
    let a = (info.rate as f32, audio);
    assets.insert(path, a.clone());
    Ok(a)
}
/// Metadata preflight counts canonical files once, before any decoded audio allocation.
pub(super) fn preflight(paths: &[PathBuf], budget: usize) -> Result<()> {
    let mut unique = std::collections::BTreeSet::new();
    let mut required = 0u64;
    for path in paths {
        let path = crate::assets::resolve(path).with_context(|| path.display().to_string())?;
        if unique.insert(path.clone()) {
            let info =
                crate::audio_file::info(&path).with_context(|| path.display().to_string())?;
            required = required
                .checked_add(info.frames)
                .context("sample frame count overflow")?;
        }
    }
    ensure!(
        required <= budget as u64,
        "patch samples require {required} decoded frames ({} bytes); sample_budget_frames allows {budget} frames ({} bytes). Set sample_budget_frames to at least {required} (maximum {}), or reduce the selected recordings",
        required.saturating_mul(8),
        budget * 8,
        crate::model::MAX_PATCH_SAMPLE_FRAMES
    );
    Ok(())
}

impl Reader {
    pub fn prepare(
        sources: &[SampleZone],
        channel: Option<&str>,
        offset: f64,
        end: Option<f64>,
        crossfade: f64,
        assets: &mut Assets,
        frames: &mut usize,
        budget: usize,
    ) -> Result<Self> {
        let channel = match channel.unwrap_or("mono") {
            "mono" => 0,
            "left" => 1,
            "right" => 2,
            _ => anyhow::bail!("reader channel must be mono, left or right"),
        };
        let mut zones = Vec::new();
        for source in sources.iter().cloned() {
            let (rate, audio) = asset(std::path::Path::new(&source.path), assets, frames, budget)?;
            let start = (source.offset_seconds + offset) * rate as f64;
            let finish = end.map_or(audio.len() as f64, |end| end * rate as f64);
            ensure!(
                finish.is_finite() && finish > start && finish <= audio.len() as f64,
                "reader region is outside recording"
            );
            if let Some([a, b]) = source.loop_seconds {
                ensure!(
                    a.is_finite()
                        && b.is_finite()
                        && a * rate as f64 >= offset * rate as f64
                        && b > a
                        && b * rate as f64 <= finish
                        && crossfade * 2. < b - a,
                    "invalid reader loop/crossfade"
                );
            } else {
                ensure!(crossfade == 0., "loop_crossfade requires a loop");
            }
            zones.push(Zone {
                gain: 10f32.powf(source.gain_db / 20.),
                source,
                rate,
                audio,
            });
        }
        Ok(Self {
            zones,
            channel,
            offset,
            end,
            crossfade,
        })
    }
    pub fn start(
        &self,
        s: &mut State,
        key: u8,
        pitch: f32,
        velocity: f32,
        choice: Option<usize>,
        count: usize,
        elapsed: u64,
        rate: f32,
    ) {
        let matches = |z: &&Zone| z.source.matches(key, velocity);
        let len = self.zones.iter().filter(matches).count();
        if len == 0 {
            *s = State {
                done: true,
                ..State::default()
            };
            return;
        }
        let index = choice.unwrap_or_else(|| {
            self.zones
                .iter()
                .enumerate()
                .filter(|(_, z)| matches(z))
                .nth(count % len)
                .unwrap()
                .0
        });
        let Some(zone) = self
            .zones
            .get(index)
            .filter(|z| z.source.matches(key, velocity))
        else {
            *s = State {
                done: true,
                ..State::default()
            };
            return;
        };
        let step =
            zone.rate as f64 / rate as f64 * 2f64.powf((pitch as f64 - zone.source.root) / 12.);
        *s = State {
            zone: index,
            pos: elapsed as f64 * step,
            step,
            pitch,
            started: false,
            done: false,
        };
    }
    pub fn frame(&self, s: &mut State, pitch: f32, speed: f32, released: bool) -> f32 {
        if s.done {
            return 0.;
        }
        let z = &self.zones[s.zone];
        let a = self.offset * z.rate as f64;
        let b = self
            .end
            .map_or(z.audio.len() as f64, |end| end * z.rate as f64);
        let speed = speed.clamp(-64., 64.) as f64;
        if !s.started {
            s.pos = if speed < 0. {
                (b - 1.).max(a) - s.pos * speed.abs()
            } else {
                a + z.source.offset_seconds * z.rate as f64 + s.pos * speed
            };
            s.started = true;
        }
        let looped = if !released || z.source.one_shot {
            z.source
                .loop_seconds
                .map(|[a, b]| [a * z.rate as f64, b * z.rate as f64])
        } else {
            None
        };
        let fade = self.crossfade * z.rate as f64;
        if let Some([a, b]) = looped {
            if speed >= 0. && s.pos >= b {
                s.pos = a + fade + (s.pos - b).rem_euclid(b - a - fade);
            }
            if speed < 0. && s.pos < a {
                s.pos = b - fade - (a - s.pos).rem_euclid(b - a - fade);
            }
        }
        if s.pos < a || s.pos >= b {
            s.done = true;
            return 0.;
        }
        let sample = |pos: f64| {
            let pair = super::super::sampler::interpolate(
                &z.audio,
                pos,
                looped.filter(|[a, b]| pos >= *a && pos < *b),
            );
            match self.channel {
                1 => pair[0],
                2 => pair[1],
                _ => (pair[0] + pair[1]) * 0.5,
            }
        };
        let mut x = sample(s.pos);
        if fade > 0. {
            if let Some([a, b]) = looped {
                if speed >= 0. && s.pos >= b - fade {
                    let p = (s.pos - (b - fade)) / fade;
                    x = x * (1. - p as f32) + sample(a + s.pos - (b - fade)) * p as f32;
                }
                if speed < 0. && s.pos < a + fade {
                    let p = (a + fade - s.pos) / fade;
                    x = x * (1. - p as f32) + sample(b - fade + s.pos - a) * p as f32;
                }
            }
        }
        s.pos += s.step * 2f64.powf((pitch - s.pitch) as f64 / 12.) * speed;
        x * z.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_readers_share_decoded_storage_and_charge_it_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.wav");
        let mut w = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..16 {
            w.write_sample(0.1f32).unwrap();
        }
        w.finalize().unwrap();
        let mut assets = Assets::new();
        let mut frames = 0;
        let (_, a) = asset(&path, &mut assets, &mut frames, 16).unwrap();
        let (_, b) = asset(
            &dir.path().join("./shared.wav"),
            &mut assets,
            &mut frames,
            16,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(frames, 16);
        assert_eq!(assets.len(), 1);
    }
}
