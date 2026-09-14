//! Prepared segment envelopes. Segment policy is source data; runtime state is fixed.
use anyhow::Result;
#[derive(Clone, Copy)]
struct Segment {
    frames: u64,
    target: f32,
    shape: u8,
}
pub(super) struct Envelope {
    attack: Vec<Segment>,
    release: Vec<Segment>,
    sustain: Option<usize>,
    pub one_shot: bool,
}
#[derive(Clone, Copy, Default)]
pub(super) struct State {
    index: usize,
    elapsed: u64,
    start: f32,
    pub value: f32,
    releasing: bool,
    pub done: bool,
}
impl Envelope {
    pub fn prepare(
        attack: &[crate::patch_description::Segment],
        release: &[crate::patch_description::Segment],
        sustain: Option<usize>,
        one_shot: bool,
        rate: f32,
    ) -> Result<Self> {
        let segments = |rows: &[crate::patch_description::Segment]| {
            rows.iter()
                .map(|s| Segment {
                    frames: (s.time * f64::from(rate)).round() as u64,
                    target: s.to as f32,
                    shape: match s.curve.as_deref().unwrap_or("linear") {
                        "smooth" => 1,
                        "exp" => 2,
                        _ => 0,
                    },
                })
                .collect()
        };
        let attack = segments(attack);
        let release = segments(release);
        Ok(Self {
            attack,
            release,
            sustain,
            one_shot,
        })
    }
    pub fn step(&self, s: &mut State, released: bool, choked: bool, rate: f32) -> f32 {
        if choked {
            s.value *= (-9.21 / (0.008 * rate)).exp();
            s.done = s.value.abs() < 1e-5;
            return s.value;
        }
        if released && !self.one_shot && !s.releasing {
            s.releasing = true;
            s.index = 0;
            s.elapsed = 0;
            s.start = s.value;
            s.done = false;
        }
        if s.done {
            return s.value;
        }
        let segments = if s.releasing {
            &self.release
        } else {
            &self.attack
        };
        loop {
            if s.index >= segments.len() {
                s.done = s.releasing || self.one_shot;
                return s.value;
            }
            let seg = segments[s.index];
            if s.elapsed >= seg.frames {
                s.value = seg.target;
                if !s.releasing && !self.one_shot && self.sustain == Some(s.index) {
                    return s.value;
                }
                s.start = s.value;
                s.index += 1;
                s.elapsed = 0;
                continue;
            }
            s.elapsed += 1;
            let p = s.elapsed as f32 / seg.frames as f32;
            let p = match seg.shape {
                1 => p * p * (3. - 2. * p),
                2 => (1. - (-5. * p).exp()) / (1. - (-5f32).exp()),
                _ => p,
            };
            s.value = s.start + (seg.target - s.start) * p;
            return s.value;
        }
    }
}
