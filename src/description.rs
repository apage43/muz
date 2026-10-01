//! Host-neutral device parameter descriptions.
use crate::diagnostic::Origin;
use crate::model::DeviceKind;
use serde::Serialize;

/// Immutable checked description. Construction performs configuration-independent
/// validation before any processor or external resource is prepared. The borrow
/// prevents callers from mutating the DTO while relying on its validation.
pub struct ValidatedSession<'a> {
    session: &'a crate::Session,
    context: crate::host::HostContext,
}

impl<'a> ValidatedSession<'a> {
    pub fn new(session: &'a crate::Session) -> anyhow::Result<Self> {
        let context = crate::host::current().unwrap_or_else(crate::host::HostContext::cli);
        context.run(|| {
            Self::validate(session)?;
            Ok(Self {
                session,
                context: context.clone(),
            })
        })
    }
    pub(crate) fn context(&self) -> &crate::host::HostContext {
        &self.context
    }
    fn validate(session: &crate::Session) -> anyhow::Result<()> {
        use anyhow::ensure;
        crate::snapshot::validate_events(session)?;
        session
            .validate_graph_budget()
            .map_err(anyhow::Error::msg)?;
        session
            .validate_sample_coverage()
            .map_err(anyhow::Error::msg)?;
        ensure!(
            session.master.output.is_none() && session.master.sends.is_empty(),
            "the master bus cannot have outputs or sends"
        );
        ensure!(
            session.buses.iter().all(|bus| bus.output.is_some()),
            "every non-master bus must have an output"
        );
        for device in session
            .tracks
            .iter()
            .flat_map(|track| std::iter::once(&track.instrument).chain(&track.inserts))
            .chain(
                std::iter::once(&session.master)
                    .chain(&session.buses)
                    .flat_map(|bus| &bus.inserts),
            )
        {
            validate_device(device)?;
        }
        Ok(())
    }

    pub fn description(&self) -> &'a crate::Session {
        self.session
    }
}

/// Checked tagged view of the legacy wire DTO. The public DTO remains available
/// for compatibility; preparation must pass through this boundary.
#[derive(Debug)]
pub enum DevicePayload<'a> {
    Native(DeviceKind),
    VoicePatch(crate::patch_description::ValidatedPatch),
    Sampler(&'a [crate::model::SampleZone]),
    Sfz(&'a crate::model::SfzConfig),
    Rack(&'a crate::model::Rack),
    Plugin {
        kind: DeviceKind,
        config: &'a crate::model::Vst3Config,
    },
}
pub fn validate_device(d: &crate::model::Device) -> anyhow::Result<DevicePayload<'_>> {
    use anyhow::ensure;
    let count = usize::from(d.patch.is_some())
        + usize::from(d.sample.is_some())
        + usize::from(d.rack.is_some())
        + usize::from(d.vst3.is_some())
        + usize::from(d.sfz.is_some());
    let payload = match d.kind {
        DeviceKind::VoicePatch => {
            ensure!(count == 1, "voice patch requires exactly its patch payload");
            DevicePayload::VoicePatch(crate::patch_description::ValidatedPatch::from_json(
                d.patch
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("missing patch"))?,
            )?)
        }
        DeviceKind::Sampler => {
            ensure!(count == 1, "sampler requires exactly its sample payload");
            DevicePayload::Sampler(
                d.sample
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("missing sample zones"))?,
            )
        }
        DeviceKind::Sfz => {
            ensure!(count == 1, "SFZ requires exactly its SFZ payload");
            let config = d
                .sfz
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("missing SFZ config"))?;
            ensure!(
                !config.path.is_empty()
                    && (1..=4096).contains(&config.max_voices)
                    && (1..=crate::model::MAX_SFZ_SAMPLE_FRAMES)
                        .contains(&config.max_sample_frames),
                "invalid SFZ preparation options"
            );
            config.program.validate()?;
            DevicePayload::Sfz(config)
        }
        DeviceKind::Rack => {
            ensure!(count == 1, "rack requires exactly its rack payload");
            let r = d
                .rack
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("missing rack"))?;
            for child in r.branches.iter().flatten() {
                validate_device(child)?;
            }
            DevicePayload::Rack(r)
        }
        DeviceKind::Vst3 | DeviceKind::Clap => {
            ensure!(count == 1, "plugin requires exactly its plugin payload");
            DevicePayload::Plugin {
                kind: d.kind,
                config: d
                    .vst3
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("missing plugin config"))?,
            }
        }
        kind => {
            ensure!(count == 0, "native device has incompatible payload");
            DevicePayload::Native(kind)
        }
    };
    if d.kind == DeviceKind::VoicePatch {
        for (name, value) in d.control_values() {
            validate_control(d, &name, value).map_err(anyhow::Error::msg)?;
        }
    } else if !matches!(
        d.kind,
        DeviceKind::Vst3 | DeviceKind::Clap | DeviceKind::Rack
    ) {
        for (name, value) in &d.params {
            validate_control(d, name, *value).map_err(anyhow::Error::msg)?;
        }
    }
    Ok(payload)
}
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct Section {
    pub name: String,
    pub start: f64,
    pub end: f64,
    pub meter: [u8; 2],
}
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct CurvePoint {
    pub seconds: f64,
    pub value: f32,
}
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct Automation {
    pub target: String,
    pub points: Vec<CurvePoint>,
    pub shape: String,
    /// Where the lane was declared, so graph validation can still name the line.
    #[serde(skip)]
    pub origin: Option<Origin>,
}
/// Authored grouping retained when a logical track expands into physical tracks.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct TrackGroup {
    pub id: String,
    pub kind: String,
    pub members: Vec<TrackGroupMember>,
}
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct TrackGroupMember {
    pub track: String,
    pub label: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct Extras {
    #[serde(default)]
    pub source: Option<PathBuf>,
    pub title: String,
    pub dependencies: Vec<PathBuf>,
    pub sections: Vec<Section>,
    pub automation: Vec<Automation>,
    pub tail: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub track_groups: Vec<TrackGroup>,
}

/// Controls with a fully static, infallible-after-validation callback setter.
pub fn static_controls(kind: DeviceKind) -> bool {
    matches!(
        kind,
        DeviceKind::PolySynth | DeviceKind::Gain | DeviceKind::VoicePatch
    )
}

pub fn validate_control(
    device: &crate::model::Device,
    name: &str,
    value: f32,
) -> Result<(), &'static str> {
    let range = if device.kind == DeviceKind::Sfz {
        name.strip_prefix("cc")
            .and_then(|n| n.parse::<u16>().ok())
            .filter(|cc| *cc < 128)
            .map(|_| (0., 127.))
    } else if device.kind == DeviceKind::VoicePatch {
        return crate::patch_description::ValidatedPatch::from_json(
            device.patch.as_ref().ok_or("missing patch")?,
        )
        .map_err(|_| "invalid voice graph")?
        .validate_control(name, value);
    } else {
        parameter_specs(device.kind)
            .iter()
            .find(|s| s.name == name)
            .map(|s| (s.min, s.max))
    }
    .ok_or("unknown control")?;
    if !value.is_finite() || !(range.0..=range.1).contains(&value) {
        return Err("control outside supported range");
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterUnit {
    Scalar,
    Milliseconds,
    Seconds,
    Hertz,
    Decibels,
    Beats,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterEffect {
    Control,
    Latency,
    Resource,
    Restart,
}
pub struct ParameterSpec {
    pub unit: ParameterUnit,
    pub effect: ParameterEffect,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

const POLY_SYNTH_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "gain_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: -60.0,
        max: 12.0,
        default: -12.0,
    },
    ParameterSpec {
        name: "attack_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 5_000.0,
        default: 10.0,
    },
    ParameterSpec {
        name: "release_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 1.0,
        max: 10_000.0,
        default: 250.0,
    },
];
const LOWPASS_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "cutoff_hz",
        unit: ParameterUnit::Hertz,
        effect: ParameterEffect::Control,
        min: 20.0,
        max: 20_000.0,
        default: 20_000.0,
    },
    ParameterSpec {
        name: "resonance",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
];
const HIGHPASS_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "cutoff_hz",
        unit: ParameterUnit::Hertz,
        effect: ParameterEffect::Control,
        min: 20.0,
        max: 20_000.0,
        default: 20.0,
    },
    ParameterSpec {
        name: "resonance",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.707,
    },
];
const DRIVE_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "drive_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 36.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "mix",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 1.0,
    },
];
const GAIN_PARAMS: &[ParameterSpec] = &[ParameterSpec {
    name: "gain_db",
    unit: ParameterUnit::Decibels,
    effect: ParameterEffect::Control,
    min: -120.0,
    max: 24.0,
    default: 0.0,
}];
const DELAY_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "time_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 48_000.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "time_beats",
        unit: ParameterUnit::Beats,
        effect: ParameterEffect::Control,
        min: 0.03125,
        max: 16.0,
        default: 0.5,
    },
    ParameterSpec {
        name: "feedback",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 0.99,
        default: 0.35,
    },
    ParameterSpec {
        name: "mix",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.25,
    },
];
const COMPRESSOR_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "threshold_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: -60.0,
        max: 0.0,
        default: -18.0,
    },
    ParameterSpec {
        name: "ratio",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 1.0,
        max: 20.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "attack_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 0.1,
        max: 200.0,
        default: 30.0,
    },
    ParameterSpec {
        name: "release_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 10.0,
        max: 2_000.0,
        default: 250.0,
    },
    ParameterSpec {
        name: "knee_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 24.0,
        default: 6.0,
    },
    ParameterSpec {
        name: "makeup_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: -24.0,
        max: 24.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "mix",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 1.0,
    },
];
const LIMITER_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "lookahead_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Latency,
        min: 0.0,
        max: 20.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "ceiling_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: -24.0,
        max: 0.0,
        default: -1.0,
    },
    ParameterSpec {
        name: "release_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 1.0,
        max: 2_000.0,
        default: 100.0,
    },
];

pub fn parameter_specs(kind: DeviceKind) -> &'static [ParameterSpec] {
    match kind {
        DeviceKind::Sampler => SAMPLE_PARAMS,
        DeviceKind::Eq => EQ_PARAMS,
        DeviceKind::Bitcrusher => BITCRUSHER_PARAMS,
        DeviceKind::Chorus => CHORUS_PARAMS,
        DeviceKind::Gate => GATE_PARAMS,
        DeviceKind::StudioSynth => STUDIO_SYNTH_PARAMS,
        DeviceKind::Reverb => REVERB_PARAMS,
        DeviceKind::Stereo => STEREO_PARAMS,
        DeviceKind::PolySynth => POLY_SYNTH_PARAMS,
        DeviceKind::Lowpass => LOWPASS_PARAMS,
        DeviceKind::Highpass => HIGHPASS_PARAMS,
        DeviceKind::Drive => DRIVE_PARAMS,
        DeviceKind::Gain => GAIN_PARAMS,
        DeviceKind::Delay => DELAY_PARAMS,
        DeviceKind::Compressor => COMPRESSOR_PARAMS,
        DeviceKind::Limiter => LIMITER_PARAMS,
        DeviceKind::Vst3
        | DeviceKind::Clap
        | DeviceKind::Rack
        | DeviceKind::VoicePatch
        | DeviceKind::Sfz => &[],
    }
}

const STUDIO_SYNTH_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "vibrato_cents",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 100.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "vibrato_hz",
        unit: ParameterUnit::Hertz,
        effect: ParameterEffect::Control,
        min: 0.1,
        max: 12.0,
        default: 5.5,
    },
    ParameterSpec {
        name: "mode",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 7.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "gain_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: -60.0,
        max: 12.0,
        default: -14.0,
    },
    ParameterSpec {
        name: "attack_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 10000.0,
        default: 5.0,
    },
    ParameterSpec {
        name: "decay_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 1.0,
        max: 10000.0,
        default: 200.0,
    },
    ParameterSpec {
        name: "sustain",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.65,
    },
    ParameterSpec {
        name: "release_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 1.0,
        max: 10000.0,
        default: 200.0,
    },
    ParameterSpec {
        name: "cutoff_hz",
        unit: ParameterUnit::Hertz,
        effect: ParameterEffect::Control,
        min: 20.0,
        max: 20000.0,
        default: 4000.0,
    },
    ParameterSpec {
        name: "resonance",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.15,
    },
    ParameterSpec {
        name: "filter_env",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: -6.0,
        max: 8.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "detune_cents",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 60.0,
        default: 12.0,
    },
    ParameterSpec {
        name: "unison",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 1.0,
        max: 5.0,
        default: 1.0,
    },
    ParameterSpec {
        name: "width",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.7,
    },
    ParameterSpec {
        name: "sub",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "fm_ratio",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.1,
        max: 16.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "fm_index",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 16.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "drive_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 24.0,
        default: 0.0,
    },
];

const REVERB_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "decay_s",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.1,
        max: 15.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "damping_hz",
        unit: ParameterUnit::Hertz,
        effect: ParameterEffect::Control,
        min: 500.0,
        max: 20000.0,
        default: 6000.0,
    },
    ParameterSpec {
        name: "mix",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.25,
    },
    ParameterSpec {
        name: "predelay_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 200.0,
        default: 20.0,
    },
];

const STEREO_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "pan",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: -1.0,
        max: 1.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "width",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 2.0,
        default: 1.0,
    },
    ParameterSpec {
        name: "duck",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "period_beats",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.125,
        max: 16.0,
        default: 1.0,
    },
];

const EQ_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "frequency_hz",
        unit: ParameterUnit::Hertz,
        effect: ParameterEffect::Control,
        min: 20.0,
        max: 20000.0,
        default: 1000.0,
    },
    ParameterSpec {
        name: "gain_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: -24.0,
        max: 24.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "q",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.1,
        max: 12.0,
        default: 0.707,
    },
    ParameterSpec {
        name: "mode",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 2.0,
        default: 0.0,
    },
];
const CHORUS_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "rate_hz",
        unit: ParameterUnit::Hertz,
        effect: ParameterEffect::Control,
        min: 0.01,
        max: 10.0,
        default: 0.4,
    },
    ParameterSpec {
        name: "depth_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 15.0,
        default: 5.0,
    },
    ParameterSpec {
        name: "mix",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 0.3,
    },
];
const GATE_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "threshold_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: -80.0,
        max: 0.0,
        default: -40.0,
    },
    ParameterSpec {
        name: "ratio",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 1.0,
        max: 20.0,
        default: 4.0,
    },
    ParameterSpec {
        name: "attack_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 0.1,
        max: 100.0,
        default: 2.0,
    },
    ParameterSpec {
        name: "release_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 1.0,
        max: 2000.0,
        default: 120.0,
    },
];

const SAMPLE_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "velocity_track",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.,
        max: 2.,
        default: 1.,
    },
    ParameterSpec {
        name: "gain_db",
        unit: ParameterUnit::Decibels,
        effect: ParameterEffect::Control,
        min: -120.,
        max: 24.,
        default: 0.,
    },
    ParameterSpec {
        name: "attack_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 0.,
        max: 5000.,
        default: 2.,
    },
    ParameterSpec {
        name: "release_ms",
        unit: ParameterUnit::Milliseconds,
        effect: ParameterEffect::Control,
        min: 0.,
        max: 10000.,
        default: 35.,
    },
];

const BITCRUSHER_PARAMS: &[ParameterSpec] = &[
    ParameterSpec {
        name: "bits",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 24.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "rate_hz",
        unit: ParameterUnit::Hertz,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 192_000.0,
        default: 0.0,
    },
    ParameterSpec {
        name: "mix",
        unit: ParameterUnit::Scalar,
        effect: ParameterEffect::Control,
        min: 0.0,
        max: 1.0,
        default: 1.0,
    },
];
