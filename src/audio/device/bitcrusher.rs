//! General amplitude quantization and synchronized stereo sample-and-hold.
use super::*;

pub(super) struct Bitcrusher {
    core: ProcessorCore,
    sample_rate: f64,
    steps: Option<f32>,
    period: f64,
    remaining: f64,
    held: [f32; 2],
    mix: f32,
}

impl Bitcrusher {
    pub(super) fn new(d: &model::Device, c: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let mut s = Self {
            core: ProcessorCore::new(d.kind, token, c.max_frames),
            sample_rate: c.sample_rate as f64,
            steps: None,
            period: 1.0,
            remaining: 0.0,
            held: [0.0; 2],
            mix: 1.0,
        };
        for (name, value) in &d.params {
            s.set_parameter(name, *value)?;
        }
        Ok(s)
    }

    fn sample(&mut self, dry: [f32; 2]) -> [f32; 2] {
        if self.remaining <= 1e-9 {
            self.held = dry;
            self.remaining += self.period;
        }
        self.remaining -= 1.0;
        // Keep the hold clock running during dry bypass for smooth live edits.
        if self.mix == 0.0 {
            return dry;
        }
        std::array::from_fn(|ch| {
            let wet = match self.steps {
                Some(steps) => (self.held[ch] * steps).round() / steps,
                None => self.held[ch],
            };
            if self.mix == 1.0 {
                wet
            } else {
                dry[ch] + (wet - dry[ch]) * self.mix
            }
        })
    }
}

impl DeviceProcessor for Bitcrusher {
    fn kind(&self) -> model::DeviceKind {
        self.core.kind
    }
    fn debug_state(&self) -> DeviceDebugState {
        self.core.debug_state()
    }
    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        let spec = crate::source::parameter_specs(self.kind())
            .iter()
            .find(|s| s.name == name)
            .ok_or(DeviceError::UnknownParameter { kind: self.kind() })?;
        let v = parameter_value(self.kind(), spec.name, value, spec.min, spec.max)?;
        match name {
            "bits" => {
                self.steps = if v == 0.0 {
                    None
                } else {
                    Some(2.0_f32.powf(v - 1.0))
                }
            }
            "rate_hz" => {
                let period = if v == 0.0 {
                    1.0
                } else {
                    (self.sample_rate / v as f64).max(1.0)
                };
                // Preserve normalized clock position when the rate changes.
                self.remaining *= period / self.period;
                self.period = period;
            }
            "mix" => self.mix = v,
            _ => unreachable!(),
        }
        Ok(())
    }
    fn reset(&mut self) {
        self.remaining = 0.0;
        self.held = [0.0; 2];
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
        for frame in 0..ctx.frames {
            [left[frame], right[frame]] = self.sample([left[frame], right[frame]]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn crusher() -> Bitcrusher {
        let device: model::Device = serde_json::from_value(serde_json::json!({
            "id":"crush", "kind":"builtin.bitcrusher", "params":{}
        }))
        .unwrap();
        Bitcrusher::new(
            &device,
            AudioConfig {
                sample_rate: 48000.0,
                max_frames: 256,
                offline: false,
            },
            1,
        )
        .unwrap()
    }
    #[test]
    fn quantization_hold_and_stereo_are_independent() {
        let mut s = crusher();
        s.set_parameter("bits", 3.0).unwrap();
        assert_eq!(s.sample([0.31, -0.64]), [0.25, -0.75]);
        s.set_parameter("bits", 0.0).unwrap();
        s.set_parameter("rate_hz", 16000.0).unwrap();
        s.reset();
        assert_eq!(s.sample([0.31, -0.64]), [0.31, -0.64]);
        assert_eq!(s.sample([0.99, 0.99]), [0.31, -0.64]);
        assert_eq!(s.sample([0.99, 0.99]), [0.31, -0.64]);
        assert_eq!(s.sample([0.2, -0.3]), [0.2, -0.3]);
    }
    #[test]
    fn reset_and_parameter_edits_preserve_clock_as_specified() {
        let mut s = crusher();
        s.set_parameter("rate_hz", 12000.0).unwrap();
        s.sample([0.2, -0.3]);
        s.set_parameter("bits", 4.0).unwrap();
        assert_eq!(s.sample([0.9, 0.8]), [0.25, -0.25]);
        s.reset();
        assert_eq!(s.sample([0.9, 0.8]), [0.875, 0.75]);
        s.set_parameter("mix", 0.0).unwrap();
        assert_eq!(s.sample([0.12345, -0.6789]), [0.12345, -0.6789]);
    }
    #[test]
    fn bypass_range_and_invalid_parameters() {
        let mut s = crusher();
        assert_eq!(s.sample([1.7, -2.3]), [1.7, -2.3]);
        s.set_parameter("bits", 3.0).unwrap();
        // Quantization does not impose an unrelated hard clip.
        assert_eq!(s.sample([1.7, -2.3]), [1.75, -2.25]);
        for (name, value) in [
            ("bits", -1.0),
            ("bits", 25.0),
            ("rate_hz", -1.0),
            ("rate_hz", 192001.0),
            ("mix", 1.1),
            ("bits", f32::NAN),
            ("mix", f32::INFINITY),
        ] {
            assert!(s.set_parameter(name, value).is_err());
        }
        assert!(s.set_parameter("unknown", 0.0).is_err());
    }
}
