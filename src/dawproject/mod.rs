//! DAWProject handoff planner and atomic archive writer.
use crate::{
    compile::{Compiled, PPQ},
    description::{Automation, ParameterEffect, ParameterUnit},
    device_state::{DeviceRole, DeviceState},
    model::{Device, DeviceKind, TrackSource, Transport},
};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{fs, io::Write, path::Path};

const FORMAT_REVISION: &str = "ee4dcdde75940f30e14e55401a26955a58b8322b";

// Bitwig's DAWProject CLAP state entry is a plug-in preset: the `clap`
// signature, a big-endian byte length and UTF-8 plug-in ID, then the bytes
// returned by the plug-in's CLAP state extension. This framing was checked
// against a Bitwig 6.0.11 export of a modified Surge XT CLAP instance.
pub const MAX_CLAP_PRESET_BYTES: usize = crate::device_state::MAX_STATE_BYTES + 8 + 1024;

pub fn clap_preset_payload(bytes: &[u8]) -> Result<(&str, &[u8])> {
    ensure!(bytes.len() >= 8, "CLAP preset header is truncated");
    ensure!(&bytes[..4] == b"clap", "CLAP preset magic is invalid");
    let id_len = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    ensure!(
        id_len > 0 && id_len <= 1024,
        "CLAP preset plug-in ID length is invalid"
    );
    ensure!(
        bytes.len() >= 8 + id_len,
        "CLAP preset plug-in ID is truncated"
    );
    let id = std::str::from_utf8(&bytes[8..8 + id_len])?;
    ensure!(!id.contains('\0'), "CLAP preset plug-in ID contains NUL");
    let state = &bytes[8 + id_len..];
    ensure!(
        state.len() <= crate::device_state::MAX_STATE_BYTES,
        "CLAP preset state is too large"
    );
    Ok((id, state))
}

pub fn write_clap_preset(writer: &mut impl Write, plugin_id: &str, state: &[u8]) -> Result<()> {
    let id = plugin_id.as_bytes();
    ensure!(
        !id.is_empty() && id.len() <= 1024 && !plugin_id.contains('\0'),
        "CLAP plug-in ID is invalid"
    );
    ensure!(
        state.len() <= crate::device_state::MAX_STATE_BYTES,
        "CLAP preset state is too large"
    );
    let len: u32 = id.len().try_into().context("CLAP plug-in ID is too long")?;
    writer.write_all(b"clap")?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(id)?;
    writer.write_all(state)?;
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub code: String,
    pub severity: &'static str,
    pub source_location: Option<crate::lang::Location>,
    pub logical_path: String,
    pub physical_path: String,
    pub feature: String,
    pub intended: String,
    pub exported: String,
    pub outcome: &'static str,
    pub remedy: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub format: &'static str,
    pub format_revision: &'static str,
    pub profile: String,
    pub profile_version: &'static str,
    pub required_plugin_ids: Vec<String>,
    pub external_dependencies: Vec<String>,
    pub entries: Vec<Entry>,
}
impl Report {
    pub fn warnings(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.severity == "warning")
    }
    fn human(&self) -> String {
        let mut out = format!(
            "DAWProject fidelity report\nFormat revision: {}\nProfile: {} {}\nRequired plugins: {}\nExternal dependencies: {}\nWarnings: {}\n",
            self.format_revision,
            self.profile,
            self.profile_version,
            self.required_plugin_ids.join(", "),
            self.external_dependencies.join(", "),
            self.warnings().count()
        );
        for entry in &self.entries {
            out.push_str(&format!("\n{} [{}] {}\n  Logical path: {}\n  Feature: {}\n  Intended: {}\n  Exported: {}\n  Outcome: {}\n  Remedy: {}\n",
                entry.severity,entry.code,entry.physical_path,entry.logical_path,entry.feature,entry.intended,entry.exported,entry.outcome,entry.remedy));
            if let Some(location) = &entry.source_location {
                out.push_str(&format!(
                    "  Source: {}\n",
                    serde_json::to_string(location).unwrap_or_default()
                ));
            }
        }
        out
    }
}

pub struct Plan<'a> {
    compiled: &'a Compiled,
    pub report: Report,
    states: Vec<(String, String, Vec<u8>)>,
    external_states: Vec<ExternalSnapshot>,
    tempo_timeline: crate::audio::transport::TempoTimeline,
    original_source: Option<String>,
    provenance_issue: Option<String>,
}
struct AutomationBinding {
    track: String,
    parameter: String,
    unit: &'static str,
    source_unit: ParameterUnit,
    db_to_linear: bool,
    plugin_range: Option<(f64, f64)>,
}
#[derive(Clone)]
struct ExternalParameter {
    id: u32,
    key: String,
    name: String,
    value: f64,
    min: f64,
    max: f64,
    automatable: bool,
}
struct ExternalSnapshot {
    physical_path: String,
    source_device_id: String,
    archive_path: String,
    bytes: Vec<u8>,
    kind: DeviceKind,
    plugin_id: String,
    name: String,
    vendor: String,
    version: String,
    bundle: String,
    parameters: Vec<ExternalParameter>,
}
fn collect_external(compiled: &Compiled) -> Result<Vec<ExternalSnapshot>> {
    let mut result = Vec::new();
    let mut push = |device: &Device, path: String| -> Result<()> {
        if matches!(device.kind, DeviceKind::Clap | DeviceKind::Vst3) {
            let snapshot = snapshot_external(device, path.clone(), result.len())
                .with_context(|| format!("snapshot external plugin at {path}"))?;
            result.push(snapshot);
        }
        Ok(())
    };
    for track in &compiled.session.tracks {
        let path = format!("track.{}", track.id);
        push(&track.instrument, format!("{path}.instrument"))?;
        for (i, device) in track.inserts.iter().enumerate() {
            push(device, format!("{path}.insert.{i}"))?;
        }
    }
    for bus in compiled
        .session
        .buses
        .iter()
        .chain(std::iter::once(&compiled.session.master))
    {
        let path = format!("bus.{}", bus.id);
        for (i, device) in bus.inserts.iter().enumerate() {
            push(device, format!("{path}.insert.{i}"))?;
        }
    }
    Ok(result)
}
fn vstpreset(class_id: &str, component: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        class_id.len() == 32 && class_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid VST3 class ID"
    );
    ensure!(
        component.len() <= 64 * 1024 * 1024,
        "VST3 component state exceeds 64 MiB"
    );
    let list_offset = 48_u64 + component.len() as u64;
    let mut bytes = Vec::with_capacity(component.len() + 76);
    bytes.extend_from_slice(b"VST3");
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(class_id.as_bytes());
    bytes.extend_from_slice(&list_offset.to_le_bytes());
    bytes.extend_from_slice(component);
    bytes.extend_from_slice(b"List");
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(b"Comp");
    bytes.extend_from_slice(&48_u64.to_le_bytes());
    bytes.extend_from_slice(&(component.len() as u64).to_le_bytes());
    Ok(bytes)
}
#[cfg(feature = "desktop")]
fn verify_effective_overrides(
    device: &Device,
    parameters: &[ExternalParameter],
    restored: &[(u32, f64)],
) -> Result<()> {
    let values: std::collections::BTreeMap<_, _> = restored.iter().copied().collect();
    for (name, intended) in &device.params {
        let parameter = parameters
            .iter()
            .find(|parameter| {
                parameter.key == *name
                    || parameter.name == *name
                    || name.parse::<u32>().ok() == Some(parameter.id)
            })
            .ok_or_else(|| anyhow::anyhow!("external parameter {name} disappeared on restore"))?;
        let actual = values.get(&parameter.id).ok_or_else(|| {
            anyhow::anyhow!("external parameter {name} has no value after state restore")
        })?;
        ensure!(
            (*actual - f64::from(*intended)).abs() < 1e-5,
            "external parameter {name} state restore changed effective value from {intended} to {actual}"
        );
    }
    Ok(())
}
#[cfg(feature = "desktop")]
fn snapshot_external(
    device: &Device,
    physical_path: String,
    index: usize,
) -> Result<ExternalSnapshot> {
    let config = device.vst3.as_ref().ok_or_else(|| {
        anyhow::anyhow!("external device {} has no plugin configuration", device.id)
    })?;
    let bundle = Path::new(&config.bundle_env);
    ensure!(
        bundle.exists(),
        "external device {} plugin bundle missing: {}",
        device.id,
        bundle.display()
    );
    let temp = tempfile::tempdir()?;
    let state_path = temp.path().join("snapshot.bin");
    match device.kind {
        DeviceKind::Clap => {
            let mut host = crate::audio::clap::PreparedClap::open(
                bundle,
                (!config.class_id.is_empty()).then_some(config.class_id.as_str()),
                crate::audio::AudioConfig {
                    sample_rate: 48000.0,
                    max_frames: 256,
                    offline: false,
                },
                0,
            )?;
            if let Some(path) = &config.state {
                host.load_state(Path::new(path))?;
            }
            for (name, value) in &device.params {
                host.set(name, *value as f64)?;
            }
            host.finish_preparation()?;
            let meta = host.metadata();
            ensure!(
                config.expected_version.is_empty() || config.expected_version == meta.version,
                "external device {} CLAP version mismatch: expected {}, found {}",
                device.id,
                config.expected_version,
                meta.version
            );
            let values: std::collections::BTreeMap<_, _> =
                host.parameter_values()?.into_iter().collect();
            let parameters: Vec<ExternalParameter> = host
                .parameters()
                .iter()
                .map(|p| ExternalParameter {
                    id: p.id,
                    key: p.key.clone(),
                    name: p.name.clone(),
                    value: *values.get(&p.id).unwrap_or(&p.default),
                    min: p.min,
                    max: p.max,
                    automatable: p.flags & clap_sys::ext::params::CLAP_PARAM_IS_AUTOMATABLE != 0
                        && p.flags & clap_sys::ext::params::CLAP_PARAM_IS_READONLY == 0,
                })
                .collect();
            let snapshot = (
                meta.id.clone(),
                meta.name.clone(),
                meta.vendor.clone(),
                meta.version.clone(),
            );
            host.save_state(&state_path)?;
            let bytes = crate::assets::read_bounded(&state_path, 64 * 1024 * 1024)?;
            drop(host);
            let mut verifier = crate::audio::clap::PreparedClap::open(
                bundle,
                Some(snapshot.0.as_str()),
                crate::audio::AudioConfig {
                    sample_rate: 48000.0,
                    max_frames: 256,
                    offline: false,
                },
                0,
            )?;
            verifier.load_state(&state_path).with_context(|| {
                format!("captured CLAP state for {} cannot be restored", snapshot.0)
            })?;
            verifier.finish_preparation()?;
            verify_effective_overrides(device, &parameters, &verifier.parameter_values()?)?;
            Ok(ExternalSnapshot {
                physical_path,
                source_device_id: device.id.as_str().into(),
                archive_path: format!("plugins/external-{index}.clap-preset"),
                bytes,
                kind: device.kind,
                plugin_id: snapshot.0,
                name: snapshot.1,
                vendor: snapshot.2,
                version: snapshot.3,
                bundle: config.bundle_env.clone(),
                parameters,
            })
        }
        DeviceKind::Vst3 => {
            let class = if config.class_id.is_empty() {
                crate::audio::vst3::Vst3ClassId([0; 16])
            } else {
                config.class_id.parse()?
            };
            let mut host = crate::audio::vst3::PreparedVst3::prepare_config(
                bundle,
                class,
                (!config.expected_version.is_empty()).then_some(config.expected_version.as_str()),
                48000.0,
                256,
                false,
            )?;
            if let Some(path) = &config.state {
                host.load_state(Path::new(path))?;
            }
            for (name, value) in &device.params {
                host.set_parameter(name, *value as f64)?;
            }
            host.finish_preparation()?;
            let meta = host.metadata();
            let values: std::collections::BTreeMap<_, _> =
                host.parameter_values()?.into_iter().collect();
            let parameters: Vec<ExternalParameter> = host
                .parameters()
                .iter()
                .map(|p| ExternalParameter {
                    id: p.id,
                    key: p.key.clone(),
                    name: p.name.clone(),
                    value: *values.get(&p.id).unwrap_or(&p.default),
                    min: 0.0,
                    max: 1.0,
                    automatable: p.flags & 1 != 0 && p.flags & 2 == 0,
                })
                .collect();
            let snapshot = (
                meta.class_id.to_string(),
                meta.class_name.clone(),
                meta.vendor.clone(),
                meta.version.clone(),
            );
            host.save_state(&state_path)?;
            let component = crate::assets::read_bounded(&state_path, 64 * 1024 * 1024)?;
            drop(host);
            let mut verifier = crate::audio::vst3::PreparedVst3::prepare_config(
                bundle,
                snapshot.0.parse()?,
                (!config.expected_version.is_empty()).then_some(config.expected_version.as_str()),
                48000.0,
                256,
                false,
            )?;
            verifier.load_state(&state_path).with_context(|| {
                format!(
                    "captured VST3 component state for {} cannot be restored",
                    snapshot.0
                )
            })?;
            verifier.finish_preparation()?;
            verify_effective_overrides(device, &parameters, &verifier.parameter_values()?)?;
            let bytes = vstpreset(&snapshot.0, &component)?;
            Ok(ExternalSnapshot {
                physical_path,
                source_device_id: device.id.as_str().into(),
                archive_path: format!("plugins/external-{index}.vstpreset"),
                bytes,
                kind: device.kind,
                plugin_id: snapshot.0,
                name: snapshot.1,
                vendor: snapshot.2,
                version: snapshot.3,
                bundle: config.bundle_env.clone(),
                parameters,
            })
        }
        _ => bail!("snapshot_external requires CLAP or VST3 device"),
    }
}
#[cfg(not(feature = "desktop"))]
fn snapshot_external(
    device: &Device,
    _physical_path: String,
    _index: usize,
) -> Result<ExternalSnapshot> {
    bail!(
        "external device {} export requires the desktop feature",
        device.id
    )
}
fn native_control_spec(device: &Device, name: &str) -> Option<(f32, f32, ParameterUnit)> {
    if device.kind == DeviceKind::VoicePatch {
        let patch =
            crate::patch_description::ValidatedPatch::from_json(device.patch.as_ref()?).ok()?;
        let (min, max) = patch.control_range(name)?;
        let unit = match name {
            "gain_db" => ParameterUnit::Decibels,
            "glide_ms" => ParameterUnit::Milliseconds,
            _ => ParameterUnit::Scalar,
        };
        return Some((min, max, unit));
    }
    let spec = crate::description::parameter_specs(device.kind)
        .iter()
        .find(|spec| spec.name == name && spec.effect == ParameterEffect::Control)?;
    Some((spec.min, spec.max, spec.unit))
}
fn normalized_plugin_value(value: f64, min: f64, max: f64) -> Option<f64> {
    (value.is_finite() && min.is_finite() && max.is_finite() && max > min)
        .then(|| ((value - min) / (max - min)).clamp(0.0, 1.0))
}
fn sampled_points(lane: &Automation, tolerance: f64) -> Option<Vec<(f64, f32)>> {
    if lane.shape != "smooth" {
        return Some(lane.points.iter().map(|p| (p.seconds, p.value)).collect());
    }
    const MAX_SEGMENTS: usize = 256;
    const MAX_POINTS: usize = 4096;
    let mut output = Vec::new();
    for pair in lane.points.windows(2) {
        let delta = (pair[1].value - pair[0].value) as f64;
        let segments = (0.75 * delta.abs() / tolerance).sqrt().ceil().max(1.0) as usize;
        if segments > MAX_SEGMENTS
            || output.len().saturating_add(segments).saturating_add(1) > MAX_POINTS
        {
            return None;
        }
        for i in 0..segments {
            let t = i as f64 / segments as f64;
            let smooth = t * t * (3.0 - 2.0 * t);
            output.push((
                pair[0].seconds + (pair[1].seconds - pair[0].seconds) * t,
                (pair[0].value as f64 + delta * smooth) as f32,
            ));
        }
    }
    output.push((lane.points.last()?.seconds, lane.points.last()?.value));
    Some(output)
}
fn converted_points(
    lane: &Automation,
    db_to_linear: bool,
    plugin_range: Option<(f64, f64)>,
) -> Option<Vec<(f64, f32)>> {
    let tolerance = plugin_range.map_or(0.001, |(min, max)| 0.001 * (max - min));
    let source = sampled_points(lane, tolerance)?;
    if !db_to_linear {
        return Some(source);
    }
    const TOLERANCE: f64 = 0.001;
    const MAX_POINTS: usize = 4096;
    let mut output = Vec::new();
    for pair in source.windows(2) {
        let delta = (pair[1].1 - pair[0].1) as f64;
        let peak = db_to_amp(pair[0].1.max(pair[1].1)) as f64;
        let curvature = peak * (std::f64::consts::LN_10 / 20.0 * delta).powi(2);
        let subdivisions = if lane.shape == "step" {
            1
        } else {
            (curvature / (8.0 * TOLERANCE)).sqrt().ceil().max(1.0) as usize
        };
        if output.len() + subdivisions + 1 > MAX_POINTS {
            return None;
        }
        for segment in 0..subdivisions {
            let phase = segment as f64 / subdivisions as f64;
            let seconds = pair[0].0 + (pair[1].0 - pair[0].0) * phase;
            let db = pair[0].1 as f64 + delta * phase;
            output.push((seconds, 10.0_f64.powf(db / 20.0) as f32));
        }
    }
    let last = source.last()?;
    output.push((last.0, db_to_amp(last.1)));
    Some(output)
}
impl<'a> Plan<'a> {
    pub fn new(compiled: &'a Compiled, profile: &str) -> Result<Self> {
        ensure!(
            profile == "bitwig-linux",
            "unsupported DAWProject profile '{profile}'"
        );
        for track in &compiled.session.tracks {
            for device in std::iter::once(&track.instrument).chain(track.inserts.iter()) {
                crate::description::validate_device(device)?;
                crate::assets::validate_versions(device)?;
            }
        }
        for bus in compiled
            .session
            .buses
            .iter()
            .chain(std::iter::once(&compiled.session.master))
        {
            for device in &bus.inserts {
                crate::description::validate_device(device)?;
                crate::assets::validate_versions(device)?;
            }
        }
        let external_states = collect_external(compiled)?;
        let tempo_timeline = crate::audio::transport::TempoTimeline::compile(
            48000.0,
            &compiled.session.transport,
            &compiled.session.tracks,
        )
        .map_err(|error| anyhow::anyhow!(error))?;
        let (original_source, provenance_issue) = match collect_provenance(compiled) {
            Ok(source) => (Some(source), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let mut plan = Self {
            compiled,
            states: Vec::new(),
            external_states,
            tempo_timeline,
            original_source,
            provenance_issue,
            report: Report {
                format: "dawproject",
                format_revision: FORMAT_REVISION,
                profile: profile.into(),
                profile_version: "Bitwig Studio 6.0.11",
                required_plugin_ids: Vec::new(),
                external_dependencies: Vec::new(),
                entries: Vec::new(),
            },
        };
        plan.inventory();
        plan.validate_plugin_parameter_ranges()?;
        plan.report.external_dependencies = plan
            .unpackageable_dependencies()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        plan.report.external_dependencies.extend(
            plan.external_states
                .iter()
                .map(|state| state.bundle.clone()),
        );
        plan.report.external_dependencies.sort();
        plan.report.external_dependencies.dedup();
        for (i, dependency) in plan.report.external_dependencies.clone().iter().enumerate() {
            if plan
                .external_states
                .iter()
                .any(|state| state.bundle == *dependency)
            {
                continue;
            }
            plan.warn(
                "DEPENDENCY_EXTERNAL",
                format!("dependency.{i}"),
                format!("dependency.{dependency}"),
                "source or asset dependency",
                "portable dependency",
                "external source-machine path in report only",
                "copy or install this dependency on the target machine",
            );
        }
        plan.report.entries.sort_by(|a, b| {
            (&a.physical_path, &a.code, &a.feature).cmp(&(&b.physical_path, &b.code, &b.feature))
        });
        Ok(plan)
    }
    fn validate_plugin_parameter_ranges(&self) -> Result<()> {
        for (path, _, bytes) in &self.states {
            let state = DeviceState::decode(bytes)?;
            for parameter in &state.parameters {
                let Some((min, max, _)) = native_control_spec(&state.device, &parameter.path)
                else {
                    continue;
                };
                ensure!(
                    normalized_plugin_value(parameter.value as f64, min as f64, max as f64)
                        .is_some()
                        && (min..=max).contains(&parameter.value),
                    "native plug-in parameter {}.{} has invalid value/range ({}, {}..{})",
                    path,
                    parameter.path,
                    parameter.value,
                    min,
                    max
                );
            }
        }
        for snapshot in &self.external_states {
            for parameter in snapshot
                .parameters
                .iter()
                .filter(|p| p.automatable && p.id <= i32::MAX as u32)
            {
                ensure!(
                    normalized_plugin_value(parameter.value, parameter.min, parameter.max)
                        .is_some()
                        && (parameter.min..=parameter.max).contains(&parameter.value),
                    "external plug-in parameter {}.{} (ID {}) has invalid value/range ({}, {}..{})",
                    snapshot.physical_path,
                    parameter.name,
                    parameter.id,
                    parameter.value,
                    parameter.min,
                    parameter.max
                );
            }
        }
        Ok(())
    }
    fn warn(
        &mut self,
        code: &str,
        logical: String,
        physical: String,
        feature: &str,
        intended: &str,
        exported: &str,
        remedy: &str,
    ) {
        let source_location = self.compiled.locations.get(&logical).cloned();
        let outcome = if code.ends_with("APPROXIMATED") {
            "approximated"
        } else if code == "EXTERNAL_PLUGIN_DEPENDENCY" || code.ends_with("MANUAL_SETUP") {
            "manual setup required"
        } else if code.ends_with("UNVERIFIED") {
            "unverified"
        } else {
            "omitted"
        };
        self.report.entries.push(Entry {
            code: code.into(),
            severity: "warning",
            source_location,
            logical_path: logical,
            physical_path: physical,
            feature: feature.into(),
            intended: intended.into(),
            exported: exported.into(),
            outcome,
            remedy: remedy.into(),
        });
    }
    fn inventory_device(&mut self, device: &Device, path: String, attach: bool) {
        let feature = if matches!(device.kind, DeviceKind::Clap | DeviceKind::Vst3) {
            format!(
                "{:?} {}",
                device.kind,
                device
                    .vst3
                    .as_ref()
                    .map_or("", |config| config.class_id.as_str())
            )
        } else {
            format!("{:?}", device.kind)
        };
        if let Some(source) = &device.sidechain {
            self.warn(
                "SIDECHAIN_MANUAL_SETUP",
                format!("device.{}", device.id),
                format!("{path}.sidechain"),
                "sidechain",
                &format!("detector signal from {source} at the declared tap and timing"),
                if matches!(device.kind, DeviceKind::Clap | DeviceKind::Vst3) {
                    "DAWProject archive has no aux connection; the declared external detector response is absent until routing is restored"
                } else {
                    "Muz FX exposes a Detector input, but the archive leaves it unconnected; sidechain-driven gain response is absent"
                },
                "if the host exposes the plug-in Detector input, connect the source and verify tap, timing and sound; otherwise use native Muz playback or recreate the routing and processing externally",
            );
        }
        if let Some(snapshot) = self
            .external_states
            .iter()
            .find(|state| state.physical_path == path)
        {
            let id = snapshot.plugin_id.clone();
            let bundle = snapshot.bundle.clone();
            let version = snapshot.version.clone();
            let kind = snapshot.kind;
            let unrepresentable: Vec<u32> = snapshot
                .parameters
                .iter()
                .filter(|p| p.id > i32::MAX as u32)
                .map(|p| p.id)
                .collect();
            if !self.report.required_plugin_ids.contains(&id) {
                self.report.required_plugin_ids.push(id.clone());
            }
            self.warn(
                "PLUGIN_RUNTIME_UNVERIFIED",
                format!("device.{}", device.id),
                path.clone(),
                &feature,
                "original external plugin with effective state and overrides",
                &format!(
                    "{} instance {}, version {}, with saved state",
                    if kind == DeviceKind::Clap {
                        "CLAP"
                    } else {
                        "VST3"
                    },
                    id,
                    version
                ),
                "confirm plugin identity, state and sound after DAW import",
            );
            if kind == DeviceKind::Vst3 {
                self.warn(
                    "VST3_CONTROLLER_STATE_OMITTED",
                    format!("device.{}", device.id),
                    path.clone(),
                    "VST3 controller private state",
                    "component and controller state where the plugin supplies both",
                    "component state and current public parameter values only",
                    "confirm preset and editor settings after import; capture controller state when the host adapter supports it",
                );
            }
            self.warn(
                "EXTERNAL_PLUGIN_DEPENDENCY",
                format!("device.{}", device.id),
                path.clone(),
                "plugin installation",
                &format!("plugin {} version {} at {}", id, version, bundle),
                "plugin binary is not embedded",
                "install the same plugin and version on the destination machine",
            );
            for parameter_id in unrepresentable {
                self.warn(
                    "PARAMETER_ID_OMITTED",
                    format!("device.{}", device.id),
                    format!("{path}.parameter.{parameter_id}"),
                    "plugin parameter identity",
                    &format!("unsigned parameter ID {parameter_id}"),
                    "state only; no DAWProject parameter reference",
                    "keep this parameter in plugin state and avoid host automation for it",
                );
            }
            return;
        }
        if attach && !matches!(device.kind, DeviceKind::Clap | DeviceKind::Vst3) {
            let role = if device.kind.is_instrument() {
                DeviceRole::Instrument
            } else {
                DeviceRole::Fx
            };
            match DeviceState::from_device(role, device.clone()).and_then(|mut state| {
                state.original_source=self.original_source.clone();
                state.encode()
            }) {
                Ok(state) => {
                let name = format!("plugins/{}.clap-preset", self.states.len());
                self.states.push((path.clone(), name, state));
                let id = role.plugin_id().to_owned();
                if !self.report.required_plugin_ids.contains(&id) {
                    self.report.required_plugin_ids.push(id);
                }
                    if device.kind == DeviceKind::StudioSynth {
                        self.warn(
                            "DSP_PARITY_UNVERIFIED",
                            format!("device.{}", device.id),
                            path.clone(),
                            &feature,
                            "muz synth DSP and timing",
                            "Muz Instrument state and playback verified in Bitwig 6.0.11; exact DSP parity remains unmeasured",
                            "compare a short render with muz if exact parity matters",
                        );
                    } else {
                        self.warn(
                            "PLUGIN_RUNTIME_UNVERIFIED",
                            format!("device.{}", device.id),
                            path.clone(),
                            &feature,
                            "muz DSP with editable plugin state",
                            "Muz CLAP instance with validated standalone source and evaluated state",
                            "verify this device's processing in the target DAW",
                        );
                    }
                    if device.kind == DeviceKind::Rack
                        && device.rack.as_ref().is_some_and(|rack| rack.branches.len() > 1)
                    {
                        self.warn(
                            "RACK_PARALLEL_UNVERIFIED",
                            format!("device.{}", device.id),
                            path.clone(),
                            "parallel rack processing",
                            "parallel branches with aligned latency, sum and dry/wet mix",
                            "one Muz FX state retaining the parallel rack graph",
                            "compare a short render and branch timing in the target DAW",
                        );
                    }
                    if let Some(issue)=&self.provenance_issue {
                        self.warn("DEVICE_PROVENANCE_OMITTED",format!("device.{}",device.id),path.clone(),"original source provenance",
                            "original author source modules alongside generated device code",
                            &format!("generated device code only: {issue}"),
                            "retain the original muz source files alongside the handoff");
                    }
                    return;
            },
            Err(error) => self.warn(
                "DEVICE_STATE_UNAVAILABLE",
                format!("device.{}",device.id),
                path.clone(),
                &feature,
                "portable code-backed plugin state",
                &format!("cannot construct state: {error:#}"),
                "use a supported asset-free device or retain the muz project for manual recreation",
            ),
            }
        }
        let code = match device.kind {
            DeviceKind::VoicePatch
            | DeviceKind::Sampler
            | DeviceKind::PolySynth
            | DeviceKind::StudioSynth
            | DeviceKind::Rack
            | DeviceKind::Eq
            | DeviceKind::Bitcrusher
            | DeviceKind::Chorus
            | DeviceKind::Gate
            | DeviceKind::Reverb
            | DeviceKind::Stereo
            | DeviceKind::Lowpass
            | DeviceKind::Highpass
            | DeviceKind::Drive
            | DeviceKind::Gain
            | DeviceKind::Delay
            | DeviceKind::Compressor
            | DeviceKind::Limiter => "NATIVE_DEVICE_UNAVAILABLE",
            DeviceKind::Clap | DeviceKind::Vst3 => "EXTERNAL_PLUGIN_UNAVAILABLE",
        };
        self.warn(
            code,
            format!("device.{}", device.id),
            path.clone(),
            &feature,
            "device DSP and settings",
            "no device instance",
            "install a compatible plugin and recreate this device manually",
        );
        if let Some(rack) = &device.rack {
            for (branch_index, branch) in rack.branches.iter().enumerate() {
                for (device_index, child) in branch.iter().enumerate() {
                    self.inventory_device(
                        child,
                        format!("{path}.branch.{branch_index}.device.{device_index}"),
                        false,
                    );
                }
            }
        }
    }
    fn inventory(&mut self) {
        let s = &self.compiled.session;
        self.warn(
            "TRANSPORT_IMPORT_UNVERIFIED",
            "transport".into(),
            "transport".into(),
            "tempo and meter",
            "compiled initial tempo and declared meter",
            "DAWProject transport; host timeline behavior not yet checked",
            "confirm tempo and meter in the imported Bitwig project",
        );
        if self.tempo_points().len() > 1 {
            self.warn(
                "TEMPO_MAP_IMPORT_UNVERIFIED",
                "transport".into(),
                "transport.tempo".into(),
                "tempo map",
                "compiled tempo changes",
                "beat-based DAWProject tempo points",
                "confirm tempo changes in the imported DAW project",
            );
        }
        if s.extras.tail > 0.0 {
            self.warn(
                "SONG_TAIL_MANUAL_SETUP",
                "transport.tail".into(),
                "transport.tail".into(),
                "render tail",
                &format!("{} seconds of rendering after the final event", s.extras.tail),
                "plug-in states retain their own release behavior; no DAWProject export range is set",
                "extend the DAW export range by the reported tail duration",
            );
        }
        if !s.extras.track_groups.is_empty() {
            for g in &s.extras.track_groups {
                self.warn(
                    "GROUP_IMPORT_UNVERIFIED",
                    format!("group.{}", g.id),
                    format!("group.{}", g.id),
                    "track group",
                    "authored grouping",
                    "nested DAWProject track group",
                    "confirm group membership in the imported DAW project",
                );
                if g.kind == "kit" {
                    for member in &g.members {
                        let has_choke = s.tracks.iter().any(|track| {
                            track.id.as_str() == member.track
                                && matches!(&track.source, TrackSource::Midi(m)
                                    if m.imported.controllers.iter().any(|c| c.controller == 120
                                        && (m.all_channels || c.channel == m.channel)))
                        });
                        if has_choke {
                            self.warn(
                                "KIT_CHOKE_IMPORT_UNVERIFIED",
                                format!("group.{}.choke", g.id),
                                format!("track.{}.choke", member.track),
                                "cross-voice kit choke",
                                "a hit in the choke group rapidly releases older voices",
                                "CC 120 hold points on this physical kit voice",
                                "verify same-tick choke delivery and release sound after import",
                            );
                        }
                    }
                }
            }
        }
        if !s.extras.sections.is_empty() {
            for section in &s.extras.sections {
                self.warn(
                    "SECTION_IMPORT_UNVERIFIED",
                    format!("section.{}", section.name),
                    format!("section.{}", section.name),
                    "section",
                    "authored section name and duration",
                    "named markers at section boundaries; project meter is stored in Transport",
                    "confirm section markers and boundaries in the imported DAW project",
                );
            }
        }
        if matches!(s.transport, Transport::Loop { .. }) {
            self.warn(
                "LOOP_OMITTED",
                "transport".into(),
                "transport".into(),
                "loop",
                "loop transport",
                "linear arrangement",
                "set the loop range in the DAW",
            );
        }
        for t in &s.tracks {
            let path = format!("track.{}", t.id);
            self.inventory_device(&t.instrument, format!("{path}.instrument"), true);
            for (i, d) in t.inserts.iter().enumerate() {
                self.inventory_device(d, format!("{path}.insert.{i}"), true);
            }
            self.inventory_route(&path, &t.output);
            for (i, r) in t.sends.iter().enumerate() {
                self.warn(
                    "SEND_IMPORT_UNVERIFIED",
                    path.clone(),
                    format!("{path}.send.{i}"),
                    "send",
                    &format!("send to {} at {} dB, pre={}", r.to, r.gain_db, r.pre),
                    "DAWProject send with destination, tap type and linear gain",
                    "confirm the send tap and level in the imported DAW project",
                );
            }
            match &t.source {
                TrackSource::Midi(m) => {
                    for n in &m.imported.notes {
                        if !m.all_channels && n.channel != m.channel {
                            continue;
                        }
                        let np = format!("{path}.note.{}", n.id);
                        if let Some(p) = n.performance {
                            if (p.pitch - n.key as f64).abs() > 1e-6
                                || p.expression.kinds().any(|kind| matches!(kind, 2 | 5 | 6))
                            {
                                self.warn(
                                    "NOTE_EXPRESSION_IMPORT_UNVERIFIED",
                                    path.clone(),
                                    np.clone(),
                                    "per-note expression",
                                    "performed note pitch, pressure and timbre",
                                    "note-contained DAWProject expression timelines",
                                    "confirm these curves follow the note through edits and replay in the DAW",
                                );
                                if self.tempo_points().iter().any(|(tick, _)| {
                                    *tick > n.start_tick
                                        && *tick < n.start_tick.saturating_add(n.duration_ticks)
                                }) {
                                    self.warn(
                                        "NOTE_EXPRESSION_TIMING_APPROXIMATED",
                                        path.clone(),
                                        np.clone(),
                                        "expression timing over tempo changes",
                                        "expression phase measured over the performed note's elapsed frames",
                                        "tempo-aware note-local beat points, using a 48 kHz reference frame grid",
                                        "verify expression timing at tempo boundaries in the destination DAW",
                                    );
                                }
                            }
                            for kind in p
                                .expression
                                .kinds()
                                .filter(|kind| !matches!(kind, 2 | 5 | 6))
                            {
                                self.warn(
                                    "NOTE_EXPRESSION_OMITTED",
                                    path.clone(),
                                    format!("{np}.expression.{kind}"),
                                    "per-note expression",
                                    &format!("expression slot {kind}"),
                                    "no equivalent expression timeline",
                                    "retain the muz source or recreate the expression manually",
                                );
                            }
                        }
                        if let Some(zone) = n.annotations.get("sample_zone") {
                            self.warn(
                                "SAMPLE_ZONE_OMITTED",
                                path.clone(),
                                np.clone(),
                                "sampler zone selection",
                                &format!("pinned sample zone {zone}"),
                                "host note with no zone pin",
                                "use the original muz project for pinned playback, or recreate this note's zone choice manually in the DAW",
                            );
                        }
                        if n.annotations.keys().any(|key| key != "sample_zone") {
                            self.warn(
                                "NOTE_ANNOTATIONS_OMITTED",
                                path.clone(),
                                np.clone(),
                                "note annotations",
                                "authored metadata",
                                "no metadata",
                                "retain the muz source for annotation values",
                            );
                        }
                    }
                    for c in &m.imported.controllers {
                        if !m.all_channels && c.channel != m.channel {
                            continue;
                        }
                        let studio_cc11 = c.controller == 11
                            && t.instrument.kind == DeviceKind::StudioSynth
                            && self.report.profile == "bitwig-linux";
                        self.warn(
                            "CONTROLLER_IMPORT_UNVERIFIED",
                            path.clone(),
                            format!("{path}.controller.{}.{}", c.channel, c.source_order),
                            "MIDI controller",
                            &format!("CC {} value {}", c.controller, c.value),
                            if studio_cc11 {
                                "DAWProject channelController hold point with normalized value; in a Bitwig Studio 6.0.11 StudioSynth CC11 probe, 0 and 127 produced byte-identical audio after the first 100 ms"
                            } else {
                                "DAWProject channelController hold point with normalized value"
                            },
                            if studio_cc11 {
                                "use native Muz playback for the intended CC11 response; verify host controller delivery before relying on imported playback"
                            } else {
                                "confirm the controller lane, channel and pedal behavior in the DAW"
                            },
                        );
                    }
                    for msg in &m.imported.messages {
                        if !m.all_channels && (msg.bytes[0] & 0x0f) != m.channel {
                            continue;
                        }
                        self.warn(
                            "MIDI_MESSAGE_OMITTED",
                            path.clone(),
                            format!("{path}.message.{}", msg.source_order),
                            "MIDI message",
                            "raw channel message",
                            "no message",
                            "recreate the MIDI message in the DAW",
                        );
                    }
                    for n in &m.imported.notes {
                        if !m.all_channels && n.channel != m.channel {
                            continue;
                        }
                        if m.imported.notes.iter().any(|other| {
                            other.source_order != n.source_order
                                && other.channel == n.channel
                                && other.key == n.key
                                && other.start_tick <= n.start_tick
                                && other.start_tick.saturating_add(other.duration_ticks)
                                    > n.start_tick
                        }) {
                            self.warn(
                                "NOTE_ORDER_UNVERIFIED",
                                path.clone(),
                                format!("{path}.note.{}", n.id),
                                "overlap and event order",
                                "performed same-key note ordering",
                                "editable overlapping notes without explicit event order",
                                "verify overlapping notes in the DAW",
                            );
                        }
                    }
                }
                TrackSource::Pattern(_) => {
                    self.warn(
                        "PATTERN_SOURCE_UNVERIFIED",
                        path.clone(),
                        path.clone(),
                        "pattern timing",
                        "performed pattern",
                        "model note timing",
                        "verify timing after import",
                    );
                }
            }
        }
        for b in s.buses.iter().chain(std::iter::once(&s.master)) {
            let path = format!("bus.{}", b.id);
            self.warn(
                "BUS_IMPORT_UNVERIFIED",
                path.clone(),
                path.clone(),
                "bus or master channel",
                "muz bus summing, inserts and latency compensation",
                "DAWProject audio channel",
                "confirm bus role, summing and latency in the imported DAW project",
            );
            for (i, d) in b.inserts.iter().enumerate() {
                self.inventory_device(d, format!("{path}.insert.{i}"), true);
            }
            if let Some(r) = &b.output {
                self.inventory_route(&path, r);
            }
            for (i, r) in b.sends.iter().enumerate() {
                self.warn(
                    "SEND_IMPORT_UNVERIFIED",
                    path.clone(),
                    format!("{path}.send.{i}"),
                    "bus send",
                    &format!("send to {} at {} dB, pre={}", r.to, r.gain_db, r.pre),
                    "DAWProject send with destination, tap type and linear gain",
                    "confirm the send tap and level in the imported DAW project",
                );
            }
        }
        for lane in &s.extras.automation {
            let logical = format!("automation.{}", lane.target);
            let binding = self.automation_binding(lane);
            let points = binding.as_ref().and_then(|binding| {
                converted_points(lane, binding.db_to_linear, binding.plugin_range)
            });
            match (binding, points) {
                (Some(binding), Some(points)) => {
                    let (code, representation, remedy) = if lane.shape == "smooth"
                        || (binding.db_to_linear && lane.shape != "step")
                    {
                        (
                            "AUTOMATION_APPROXIMATED",
                            if binding.db_to_linear {
                                format!("{} piecewise-linear points in seconds; dB-to-amplitude conversion error at most 0.001 per segment",points.len())
                            } else {
                                if binding.plugin_range.is_some() {
                                    format!("{} piecewise-linear points in seconds; max normalized parameter error 0.001",points.len())
                                } else {
                                    format!("{} piecewise-linear points in seconds; max source-value error 0.001",points.len())
                                }
                            },
                            "inspect the sampled curve and bounce it in the target DAW",
                        )
                    } else {
                        (
                            "AUTOMATION_IMPORT_UNVERIFIED",
                            format!(
                                "{} DAWProject points in seconds on {}{}",
                                lane.shape,
                                binding.parameter,
                                if binding.plugin_range.is_some() { " (normalized 0..1)" } else { "" }
                            ),
                            "confirm parameter response in the target DAW",
                        )
                    };
                    self.warn(
                        code,
                        logical.clone(),
                        logical.clone(),
                        "automation",
                        &format!("{} curve in seconds", lane.shape),
                        &representation,
                        remedy,
                    );
                    if let Some((min, max)) = binding.plugin_range {
                        if points.iter().any(|(_, value)| (*value as f64) < min || (*value as f64) > max) {
                            self.warn(
                                "AUTOMATION_RANGE_APPROXIMATED",
                                logical.clone(),
                                logical.clone(),
                                "plug-in automation range",
                                &format!("plain values outside {min}..{max}"),
                                "out-of-range values clamped before normalization",
                                "keep automation points within the plug-in parameter range",
                            );
                        }
                    }
                    if lane.shape == "smooth" || binding.db_to_linear {
                        self.warn(
                            "AUTOMATION_IMPORT_UNVERIFIED",
                            logical.clone(),
                            logical.clone(),
                            "automation host delivery",
                            "sampled plain-value parameter events",
                            "DAWProject Points targeting the exported parameter",
                            "confirm the imported curve controls the plugin in Bitwig",
                        );
                    }
                    if binding.source_unit == ParameterUnit::Milliseconds {
                        self.warn(
                            "AUTOMATION_UNIT_UNVERIFIED",
                            logical.clone(),
                            logical.clone(),
                            "millisecond parameter unit",
                            "plain millisecond values",
                            "DAWProject normalized plug-in parameter values derived from milliseconds",
                            "confirm displayed values and automation response in Bitwig",
                        );
                    }
                }
                (Some(_), None) => self.warn(
                    "AUTOMATION_DENSITY_OMITTED",
                    logical.clone(),
                    logical,
                    "automation",
                    &format!("{} curve in seconds", lane.shape),
                    "no automation lane; sampled curve would exceed bounded point budget",
                    "simplify the curve or retain this lane in the muz source",
                ),
                (None, _) => self.warn(
                    "AUTOMATION_TARGET_OMITTED",
                    logical.clone(),
                    logical,
                    "automation",
                    &format!("{} curve in seconds", lane.shape),
                    "no addressable DAWProject parameter for this target",
                    "retain this lane in the muz source or recreate it on a supported host parameter",
                ),
            }
        }
    }
    fn inventory_route(&mut self, path: &str, route: &crate::model::Route) {
        self.warn(
            "ROUTE_IMPORT_UNVERIFIED",
            path.into(),
            format!("{path}.output"),
            "output route",
            &format!(
                "route to {} at {} dB, pre={}",
                route.to, route.gain_db, route.pre
            ),
            "DAWProject channel destination and linear Volume",
            "confirm routing, level and post-send fader behavior in the imported DAW project",
        );
    }
    fn unpackageable_dependencies(&self) -> Vec<std::path::PathBuf> {
        let session = &self.compiled.session;
        let mut covered = std::collections::BTreeSet::new();
        if self.original_source.is_some() {
            covered.extend(
                session
                    .extras
                    .dependencies
                    .iter()
                    .filter(|path| path.extension().is_some_and(|ext| ext == "muz"))
                    .cloned(),
            );
        }
        for track in &session.tracks {
            let path = format!("track.{}", track.id);
            for (physical, device) in
                std::iter::once((format!("{path}.instrument"), &track.instrument)).chain(
                    track
                        .inserts
                        .iter()
                        .enumerate()
                        .map(|(i, d)| (format!("{path}.insert.{i}"), d)),
                )
            {
                if self
                    .states
                    .iter()
                    .any(|(state_path, _, _)| state_path == &physical)
                    || self
                        .external_states
                        .iter()
                        .any(|state| state.physical_path == physical)
                {
                    covered.extend(crate::assets::paths(device));
                }
            }
        }
        for bus in session.buses.iter().chain(std::iter::once(&session.master)) {
            let path = format!("bus.{}", bus.id);
            for (i, device) in bus.inserts.iter().enumerate() {
                if self
                    .states
                    .iter()
                    .any(|(state_path, _, _)| state_path == &format!("{path}.insert.{i}"))
                    || self
                        .external_states
                        .iter()
                        .any(|state| state.physical_path == format!("{path}.insert.{i}"))
                {
                    covered.extend(crate::assets::paths(device));
                }
            }
        }
        session
            .extras
            .dependencies
            .iter()
            .filter(|path| {
                Some(*path) != session.extras.source.as_ref() && !covered.contains(*path)
            })
            .cloned()
            .collect()
    }
    pub fn write(&self, output: &Path, strict: bool, report_path: Option<&Path>) -> Result<()> {
        let default_json = output.with_extension("dawproject.report.json");
        let json_path = report_path.unwrap_or(&default_json);
        let text_path = output.with_extension("dawproject.report.txt");
        let absolute = |p: &Path| -> Result<std::path::PathBuf> {
            if let Ok(canonical) = p.canonicalize() {
                return Ok(canonical);
            }
            let parent = p
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            Ok(parent.canonicalize()?.join(
                p.file_name()
                    .ok_or_else(|| anyhow::anyhow!("invalid report or output path"))?,
            ))
        };
        let output_abs = absolute(output)?;
        let json_abs = absolute(json_path)?;
        let text_abs = absolute(&text_path)?;
        ensure!(
            output_abs != json_abs && output_abs != text_abs && json_abs != text_abs,
            "DAWProject output, JSON report and text report must use distinct paths"
        );
        fs::write(json_path, serde_json::to_vec_pretty(&self.report)?)?;
        fs::write(&text_path, self.report.human())?;
        if strict && self.report.warnings().next().is_some() {
            bail!(
                "strict DAWProject export rejected {} fidelity warnings",
                self.report.warnings().count()
            );
        }
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        {
            let mut zip = zip::ZipWriter::new(temp.as_file_mut());
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("project.xml", opts)?;
            zip.write_all(self.xml()?.as_bytes())?;
            zip.start_file("metadata.xml", opts)?;
            zip.write_all(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><MetaData><Title>{}</Title></MetaData>",esc(&self.compiled.session.extras.title)).as_bytes())?;
            for (_, name, bytes) in &self.states {
                zip.start_file(name, opts)?;
                let role = DeviceState::decode(bytes)?.role;
                write_clap_preset(&mut zip, role.plugin_id(), bytes)?;
            }
            for snapshot in &self.external_states {
                zip.start_file(&snapshot.archive_path, opts)?;
                if snapshot.kind == DeviceKind::Clap {
                    write_clap_preset(&mut zip, &snapshot.plugin_id, &snapshot.bytes)?;
                } else {
                    zip.write_all(&snapshot.bytes)?;
                }
            }
            zip.finish()?;
        }
        temp.persist(output).map_err(|e| e.error)?;
        Ok(())
    }
    fn xml(&self) -> Result<String> {
        let s = &self.compiled.session;
        let mut x = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Project version=\"1.0\"><Application name=\"muz\" version=\"0.1.0\"/>",
        );
        let tempos = self.tempo_points();
        let bpm = tempos[0].1;
        let meter = s.transport.meter();
        x.push_str(&format!("<Transport><Tempo id=\"tempo-param\" unit=\"bpm\" value=\"{bpm}\"/><TimeSignature id=\"meter-param\" numerator=\"{}\" denominator=\"{}\"/></Transport><Structure>",meter[0],meter[1]));
        let mut grouped = vec![false; s.tracks.len()];
        for (group_index, group) in s.extras.track_groups.iter().enumerate() {
            x.push_str(&format!(
                "<Track id=\"group-{group_index}\" name=\"{}\" contentType=\"tracks\">",
                esc(&group.id)
            ));
            for member in &group.members {
                if let Some((track_index, track)) = s
                    .tracks
                    .iter()
                    .enumerate()
                    .find(|(_, track)| track.id.as_str() == member.track)
                {
                    grouped[track_index] = true;
                    self.xml_track_structure(&mut x, track_index, track);
                }
            }
            x.push_str("</Track>");
        }
        for (i, track) in s.tracks.iter().enumerate() {
            if !grouped[i] {
                self.xml_track_structure(&mut x, i, track);
            }
        }
        for (i, bus) in s.buses.iter().enumerate() {
            self.xml_bus_structure(&mut x, i, bus);
        }
        self.xml_master_structure(&mut x);
        x.push_str("</Structure><Arrangement id=\"arrangement\"><Lanes id=\"arrangement-lanes\" timeUnit=\"beats\">");
        for (i, t) in s.tracks.iter().enumerate() {
            let clip_end = match &t.source {
                TrackSource::Midi(m) => m
                    .imported
                    .notes
                    .iter()
                    .map(|n| n.start_tick.saturating_add(n.duration_ticks) as f64 / PPQ as f64)
                    .fold(m.imported.summary.end_tick as f64 / PPQ as f64, f64::max),
                TrackSource::Pattern(p) => p
                    .notes
                    .iter()
                    .map(|n| {
                        n.start_ticks.saturating_add(n.duration_ticks) as f64
                            / crate::model::TICKS_PER_BEAT as f64
                    })
                    .fold(0.0, f64::max),
            };
            x.push_str(&format!(
                "<Lanes id=\"lanes-t{i}\" track=\"t{i}\"><Clips id=\"clips-t{i}\"><Clip time=\"0\" duration=\"{}\" playStart=\"0\"><Notes id=\"notes-t{i}\" timeUnit=\"beats\">",
                clip_end.max(1.0)
            ));
            match &t.source {
                TrackSource::Midi(m) => {
                    for (note_index, n) in m.imported.notes.iter().enumerate() {
                        if !m.all_channels && n.channel != m.channel {
                            continue;
                        }
                        let vel = n
                            .performance
                            .map_or(n.attack_velocity as f64 / 127.0, |p| p.velocity)
                            .clamp(0., 1.);
                        x.push_str(&format!("<Note time=\"{}\" duration=\"{}\" channel=\"{}\" key=\"{}\" vel=\"{}\" rel=\"{}\">",n.start_tick as f64/PPQ as f64,n.duration_ticks as f64/PPQ as f64,n.channel,n.key,vel,n.release_velocity as f64/127.0));
                        self.xml_note_expression(&mut x, i, note_index, n);
                        x.push_str("</Note>");
                    }
                }
                TrackSource::Pattern(p) => {
                    for n in &p.notes {
                        x.push_str(&format!("<Note time=\"{}\" duration=\"{}\" channel=\"0\" key=\"{}\" vel=\"{}\"/>",n.start_ticks as f64/crate::model::TICKS_PER_BEAT as f64,n.duration_ticks as f64/crate::model::TICKS_PER_BEAT as f64,n.key,n.velocity));
                    }
                }
            }
            x.push_str("</Notes></Clip></Clips>");
            if let TrackSource::Midi(m) = &t.source {
                self.xml_controllers(&mut x, i, m);
            }
            self.xml_automation(&mut x, &format!("t{i}"));
            x.push_str("</Lanes>");
        }
        for i in 0..s.buses.len() {
            let track = format!("bus-track-{i}");
            let mut points = String::new();
            self.xml_automation(&mut points, &track);
            if !points.is_empty() {
                x.push_str(&format!(
                    "<Lanes id=\"lanes-bus-{i}\" track=\"{track}\">{points}</Lanes>"
                ));
            }
        }
        let mut master_points = String::new();
        self.xml_automation(&mut master_points, "master-track");
        if !master_points.is_empty() {
            x.push_str(&format!(
                "<Lanes id=\"lanes-master\" track=\"master-track\">{master_points}</Lanes>"
            ));
        }
        x.push_str("</Lanes>");
        if !s.extras.sections.is_empty() {
            x.push_str("<Markers id=\"section-markers\" timeUnit=\"beats\">");
            for section in &s.extras.sections {
                x.push_str(&format!(
                    "<Marker time=\"{}\" name=\"{}\"/>",
                    section.start,
                    esc(&section.name)
                ));
            }
            if let Some(last) = s.extras.sections.last() {
                x.push_str(&format!("<Marker time=\"{}\" name=\"End\"/>", last.end));
            }
            x.push_str("</Markers>");
        }
        if tempos.len() > 1 {
            x.push_str("<TempoAutomation id=\"tempo-automation\" timeUnit=\"beats\" unit=\"bpm\"><Target parameter=\"tempo-param\"/>");
            for (tick, bpm) in tempos {
                x.push_str(&format!(
                    "<RealPoint time=\"{}\" value=\"{}\" interpolation=\"hold\"/>",
                    tick as f64 / PPQ as f64,
                    bpm
                ));
            }
            x.push_str("</TempoAutomation>");
        }
        x.push_str("</Arrangement></Project>");
        ensure!(x.len() < 64 * 1024 * 1024, "project XML exceeds size limit");
        Ok(x)
    }
    fn xml_note_expression(
        &self,
        x: &mut String,
        track_index: usize,
        note_index: usize,
        note: &crate::midi::MidiNote,
    ) {
        let Some(performance) = note.performance else {
            return;
        };
        let pitch_offset = performance.pitch - note.key as f64;
        let mut lanes = String::new();
        for (kind, expression, unit) in [
            (2_u8, "transpose", "semitones"),
            (5, "timbre", "normalized"),
            (6, "pressure", "normalized"),
        ] {
            if kind != 2 && !performance.expression.kinds().any(|k| k == kind) {
                continue;
            }
            if kind == 2
                && pitch_offset.abs() <= 1e-6
                && !performance.expression.kinds().any(|k| k == kind)
            {
                continue;
            }
            lanes.push_str(&format!("<Points id=\"note-expression-{track_index}-{note_index}-{kind}\" timeUnit=\"beats\" unit=\"{unit}\"><Target expression=\"{expression}\"/>"));
            for (time, value) in self.note_expression_points(note, kind, pitch_offset) {
                lanes.push_str(&format!(
                    "<RealPoint time=\"{time}\" value=\"{value}\" interpolation=\"linear\"/>"
                ));
            }
            lanes.push_str("</Points>");
        }
        if !lanes.is_empty() {
            x.push_str(&format!(
                "<Lanes id=\"note-expression-lanes-{track_index}-{note_index}\">{lanes}</Lanes>"
            ));
        }
    }
    fn note_expression_points(
        &self,
        note: &crate::midi::MidiNote,
        kind: u8,
        pitch_offset: f64,
    ) -> Vec<(f64, f64)> {
        let Some(performance) = note.performance else {
            return Vec::new();
        };
        let start_tick = note.start_tick;
        let end_tick = start_tick.saturating_add(note.duration_ticks);
        let start_frame = self
            .tempo_timeline
            .tick_to_project_frame(start_tick as f64)
            .round();
        let off_frame = self
            .tempo_timeline
            .tick_to_project_frame(end_tick as f64)
            .round()
            .max(start_frame + 1.0);
        let duration_frames = off_frame - start_frame;
        let mut phases = vec![0.0_f64];
        phases.extend(
            performance.expression.points[..performance.expression.len as usize]
                .iter()
                .filter(|point| point.kind == kind && point.phase > 0.0)
                .map(|point| point.phase as f64),
        );
        for (tempo_tick, _) in self.tempo_points() {
            if tempo_tick <= start_tick || tempo_tick >= end_tick {
                continue;
            }
            let frame = self.tempo_timeline.tick_to_project_frame(tempo_tick as f64);
            let phase = (frame - start_frame) / duration_frames;
            if phase > 0.0 && phase < 1.0 {
                phases.push(phase);
            }
        }
        phases.sort_by(f64::total_cmp);
        phases.dedup_by(|a, b| (*a - *b).abs() < 1e-8);
        phases
            .into_iter()
            .map(|phase| {
                let time = if phase == 0.0 {
                    0.0
                } else {
                    ((self
                        .tempo_timeline
                        .project_frame_to_tick(start_frame + phase * duration_frames)
                        - start_tick as f64)
                        / PPQ as f64)
                        .clamp(0.0, note.duration_ticks as f64 / PPQ as f64)
                };
                let value = performance
                    .expression
                    .value(kind, phase as f32)
                    .unwrap_or(0.0) as f64
                    + if kind == 2 { pitch_offset } else { 0.0 };
                (time, value)
            })
            .collect()
    }
    fn xml_controllers(
        &self,
        x: &mut String,
        track_index: usize,
        source: &crate::model::MidiTrackSource,
    ) {
        let mut lanes =
            std::collections::BTreeMap::<(u8, u8), Vec<&crate::midi::MidiController>>::new();
        for controller in &source.imported.controllers {
            if source.all_channels || controller.channel == source.channel {
                lanes
                    .entry((controller.channel, controller.controller))
                    .or_default()
                    .push(controller);
            }
        }
        for ((channel, cc), mut points) in lanes {
            points.sort_by_key(|point| (point.tick, point.source_order));
            x.push_str(&format!("<Points id=\"controller-{track_index}-{channel}-{cc}\" timeUnit=\"beats\" unit=\"normalized\"><Target expression=\"channelController\" channel=\"{channel}\" controller=\"{cc}\"/>"));
            for point in points {
                x.push_str(&format!(
                    "<RealPoint time=\"{}\" value=\"{}\" interpolation=\"hold\"/>",
                    point.tick as f64 / PPQ as f64,
                    point.value as f64 / 127.0
                ));
            }
            x.push_str("</Points>");
        }
    }
    fn target_channel(&self, target: &crate::model::Id) -> String {
        self.compiled
            .session
            .buses
            .iter()
            .position(|bus| &bus.id == target)
            .map(|index| format!("bus-channel-{index}"))
            .unwrap_or_else(|| "master-channel".into())
    }
    fn tempo_points(&self) -> Vec<(u64, f64)> {
        let s = &self.compiled.session;
        if let Transport::Loop { bpm, .. } = &s.transport {
            return vec![(0, *bpm)];
        }
        let mut events: Vec<_> = s
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(track_order, track)| match &track.source {
                TrackSource::Midi(source) => source
                    .imported
                    .tempos
                    .iter()
                    .map(move |tempo| {
                        (
                            tempo.tick,
                            tempo.source_order,
                            track_order,
                            60_000_000.0 / tempo.micros_per_quarter as f64,
                        )
                    })
                    .collect::<Vec<_>>(),
                TrackSource::Pattern(_) => Vec::new(),
            })
            .collect();
        events.sort_unstable_by_key(|(tick, order, track, _)| (*tick, *order, *track));
        let mut points = vec![(0, 120.0)];
        for (tick, _, _, bpm) in events {
            if points.last().is_some_and(|p| p.0 == tick) {
                points.last_mut().unwrap().1 = bpm;
            } else {
                points.push((tick, bpm));
            }
        }
        points
    }
    fn automation_binding(&self, lane: &Automation) -> Option<AutomationBinding> {
        let route_binding = |track: String, parameter: String| AutomationBinding {
            track,
            parameter,
            unit: "linear",
            source_unit: ParameterUnit::Decibels,
            db_to_linear: true,
            plugin_range: None,
        };
        for (index, track) in self.compiled.session.tracks.iter().enumerate() {
            if lane.target == track.output.id.as_str() {
                return Some(route_binding(
                    format!("t{index}"),
                    format!("volume-c{index}"),
                ));
            }
            for (send_index, route) in track.sends.iter().enumerate() {
                if lane.target == route.id.as_str() {
                    return Some(route_binding(
                        format!("t{index}"),
                        format!("send-volume-c{index}-{send_index}"),
                    ));
                }
            }
        }
        for (index, bus) in self.compiled.session.buses.iter().enumerate() {
            if bus
                .output
                .as_ref()
                .is_some_and(|route| lane.target == route.id.as_str())
            {
                return Some(route_binding(
                    format!("bus-track-{index}"),
                    format!("volume-bus-channel-{index}"),
                ));
            }
            for (send_index, route) in bus.sends.iter().enumerate() {
                if lane.target == route.id.as_str() {
                    return Some(route_binding(
                        format!("bus-track-{index}"),
                        format!("send-volume-bus-channel-{index}-{send_index}"),
                    ));
                }
            }
        }
        for (state_index, (path, _, bytes)) in self.states.iter().enumerate() {
            let state = DeviceState::decode(bytes).ok()?;
            let track = self.track_ref_for_path(path)?;
            for (parameter_index, parameter) in state.parameters.iter().enumerate() {
                if lane.target != format!("{}.{}", state.device.id, parameter.path) {
                    continue;
                }
                let (min, max, source_unit) = native_control_spec(&state.device, &parameter.path)?;
                normalized_plugin_value(parameter.value as f64, min as f64, max as f64)?;
                return Some(AutomationBinding {
                    track,
                    parameter: format!("param-{state_index}-{parameter_index}"),
                    unit: "normalized",
                    source_unit,
                    db_to_linear: false,
                    plugin_range: Some((min as f64, max as f64)),
                });
            }
        }
        for (index, snapshot) in self.external_states.iter().enumerate() {
            let track = self.track_ref_for_path(&snapshot.physical_path)?;
            for (parameter_index, parameter) in snapshot.parameters.iter().enumerate() {
                if parameter.id > i32::MAX as u32 || !parameter.automatable {
                    continue;
                }
                if ![parameter.key.as_str(), parameter.name.as_str()]
                    .iter()
                    .any(|name| lane.target == format!("{}.{}", snapshot.source_device_id, name))
                    && lane.target != format!("{}.{}", snapshot.source_device_id, parameter.id)
                {
                    continue;
                }
                normalized_plugin_value(parameter.value, parameter.min, parameter.max)?;
                return Some(AutomationBinding {
                    track,
                    parameter: format!("external-param-{index}-{parameter_index}"),
                    unit: "normalized",
                    source_unit: ParameterUnit::Scalar,
                    db_to_linear: false,
                    plugin_range: Some((parameter.min, parameter.max)),
                });
            }
        }
        None
    }
    fn track_ref_for_path(&self, path: &str) -> Option<String> {
        self.compiled
            .session
            .tracks
            .iter()
            .enumerate()
            .find_map(|(i, t)| {
                path.starts_with(&format!("track.{}.", t.id))
                    .then(|| format!("t{i}"))
            })
            .or_else(|| {
                self.compiled
                    .session
                    .buses
                    .iter()
                    .enumerate()
                    .find_map(|(i, b)| {
                        path.starts_with(&format!("bus.{}.", b.id))
                            .then(|| format!("bus-track-{i}"))
                    })
            })
            .or_else(|| {
                path.starts_with(&format!("bus.{}.", self.compiled.session.master.id))
                    .then(|| "master-track".into())
            })
    }
    fn xml_track_structure(&self, x: &mut String, index: usize, track: &crate::model::Track) {
        let path = format!("track.{}", track.id);
        x.push_str(&format!("<Track id=\"t{index}\" name=\"{}\" contentType=\"notes\"><Channel id=\"c{index}\" destination=\"{}\" role=\"regular\" audioChannels=\"2\">",esc(&track.name),self.target_channel(&track.output.to)));
        let mut devices = String::new();
        self.xml_device(
            &mut devices,
            &format!("{path}.instrument"),
            &track.instrument,
        );
        for (i, device) in track.inserts.iter().enumerate() {
            self.xml_device(&mut devices, &format!("{path}.insert.{i}"), device);
        }
        if !devices.is_empty() {
            x.push_str("<Devices>");
            x.push_str(&devices);
            x.push_str("</Devices>");
        }
        self.xml_sends(x, &track.sends, &format!("c{index}"));
        self.xml_volume(x, track.output.gain_db, &format!("c{index}"));
        x.push_str("</Channel></Track>");
    }
    fn xml_bus_structure(&self, x: &mut String, index: usize, bus: &crate::model::Bus) {
        let path = format!("bus.{}", bus.id);
        let destination = bus
            .output
            .as_ref()
            .map(|route| self.target_channel(&route.to))
            .unwrap_or_else(|| "master-channel".into());
        let session = &self.compiled.session;
        let primary = session.tracks.iter().any(|track| track.output.to == bus.id)
            || session.buses.iter().any(|other| {
                other
                    .output
                    .as_ref()
                    .is_some_and(|route| route.to == bus.id)
            });
        let role = if primary { "submix" } else { "effect" };
        x.push_str(&format!("<Track id=\"bus-track-{index}\" name=\"{}\" contentType=\"audio\"><Channel id=\"bus-channel-{index}\" destination=\"{destination}\" role=\"{role}\" audioChannels=\"2\">",esc(&bus.name)));
        let mut devices = String::new();
        for (i, device) in bus.inserts.iter().enumerate() {
            self.xml_device(&mut devices, &format!("{path}.insert.{i}"), device);
        }
        if !devices.is_empty() {
            x.push_str("<Devices>");
            x.push_str(&devices);
            x.push_str("</Devices>");
        }
        self.xml_sends(x, &bus.sends, &format!("bus-channel-{index}"));
        self.xml_volume(
            x,
            bus.output.as_ref().map_or(0.0, |route| route.gain_db),
            &format!("bus-channel-{index}"),
        );
        x.push_str("</Channel></Track>");
    }
    fn xml_master_structure(&self, x: &mut String) {
        let master = &self.compiled.session.master;
        x.push_str(&format!("<Track id=\"master-track\" name=\"{}\" contentType=\"audio\"><Channel id=\"master-channel\" role=\"master\" audioChannels=\"2\">",esc(&master.name)));
        let mut devices = String::new();
        for (i, device) in master.inserts.iter().enumerate() {
            self.xml_device(
                &mut devices,
                &format!("bus.{}.insert.{i}", master.id),
                device,
            );
        }
        if !devices.is_empty() {
            x.push_str("<Devices>");
            x.push_str(&devices);
            x.push_str("</Devices>");
        }
        self.xml_sends(x, &master.sends, "master-channel");
        x.push_str("</Channel></Track>");
    }
    fn xml_sends(&self, x: &mut String, sends: &[crate::model::Route], prefix: &str) {
        if sends.is_empty() {
            return;
        }
        x.push_str("<Sends>");
        for (index, route) in sends.iter().enumerate() {
            x.push_str(&format!("<Send id=\"send-{prefix}-{index}\" destination=\"{}\" type=\"{}\"><Volume id=\"send-volume-{prefix}-{index}\" unit=\"linear\" value=\"{}\"/></Send>",self.target_channel(&route.to),if route.pre {"pre"} else {"post"},db_to_amp(route.gain_db)));
        }
        x.push_str("</Sends>");
    }
    fn xml_volume(&self, x: &mut String, gain_db: f32, prefix: &str) {
        x.push_str(&format!(
            "<Volume id=\"volume-{prefix}\" unit=\"linear\" value=\"{}\"/>",
            db_to_amp(gain_db)
        ));
    }
    fn xml_automation(&self, x: &mut String, track: &str) {
        for (lane_index, lane) in self.compiled.session.extras.automation.iter().enumerate() {
            let Some(binding) = self.automation_binding(lane) else {
                continue;
            };
            if binding.track != track {
                continue;
            }
            let Some(points) = converted_points(lane, binding.db_to_linear, binding.plugin_range)
            else {
                continue;
            };
            x.push_str(&format!("<Points id=\"automation-{lane_index}\" timeUnit=\"seconds\" unit=\"{}\"><Target parameter=\"{}\"/>",binding.unit,binding.parameter));
            let interpolation = if lane.shape == "step" {
                "hold"
            } else {
                "linear"
            };
            for (seconds, value) in points {
                let value = if let Some((min, max)) = binding.plugin_range {
                    normalized_plugin_value(value as f64, min, max)
                        .expect("validated plug-in parameter range")
                        .to_string()
                } else {
                    value.to_string()
                };
                x.push_str(&format!("<RealPoint time=\"{seconds}\" value=\"{value}\" interpolation=\"{interpolation}\"/>"));
            }
            x.push_str("</Points>");
        }
    }
    fn xml_device(&self, out: &mut String, path: &str, device: &Device) {
        if let Some((index, snapshot)) = self
            .external_states
            .iter()
            .enumerate()
            .find(|(_, state)| state.physical_path == path)
        {
            let tag = if snapshot.kind == DeviceKind::Clap {
                "ClapPlugin"
            } else {
                "Vst3Plugin"
            };
            let role = if path.ends_with(".instrument") {
                "instrument"
            } else {
                "audioFX"
            };
            out.push_str(&format!("<{tag} id=\"external-device-{index}\" name=\"{}\" deviceID=\"{}\" deviceName=\"{}\" deviceVendor=\"{}\" pluginVersion=\"{}\" deviceRole=\"{role}\" loaded=\"true\">",
                esc(&snapshot.name),esc(&snapshot.plugin_id),esc(&snapshot.name),esc(&snapshot.vendor),esc(&snapshot.version)));
            out.push_str("<Parameters>");
            for (parameter_index, parameter) in snapshot.parameters.iter().enumerate() {
                if parameter.id > i32::MAX as u32 || !parameter.automatable {
                    continue;
                }
                let value = normalized_plugin_value(parameter.value, parameter.min, parameter.max)
                    .expect("external plug-in parameter range validated during planning");
                out.push_str(&format!("<RealParameter id=\"external-param-{index}-{parameter_index}\" parameterID=\"{}\" name=\"{}\" unit=\"normalized\" min=\"0\" max=\"1\" value=\"{}\"/>",
                    parameter.id,esc(&parameter.name),value));
            }
            out.push_str("</Parameters>");
            out.push_str(&format!("<Enabled id=\"external-enabled-{index}\" name=\"On/Off\" value=\"true\"/><State path=\"{}\"/></{tag}>",esc(&snapshot.archive_path)));
            return;
        }
        let Some((state_index, (_, name, bytes))) = self
            .states
            .iter()
            .enumerate()
            .find(|(_, (p, _, _))| p == path)
        else {
            return;
        };
        let state = DeviceState::decode(bytes).expect("exporter encoded validated state");
        let role = if device.kind.is_instrument() {
            "instrument"
        } else {
            "audioFX"
        };
        let id = if device.kind.is_instrument() {
            crate::device_state::INSTRUMENT_ID
        } else {
            crate::device_state::FX_ID
        };
        out.push_str(&format!("<ClapPlugin id=\"muz-device-{state_index}\" name=\"Muz\" deviceID=\"{id}\" deviceName=\"Muz\" deviceRole=\"{role}\" loaded=\"true\">"));
        let mut parameters = String::new();
        for (parameter_index, parameter) in state.parameters.iter().enumerate() {
            let Some((min, max, _unit)) = native_control_spec(device, &parameter.path) else {
                continue;
            };
            let value = normalized_plugin_value(parameter.value as f64, min as f64, max as f64)
                .expect("native plug-in parameter range validated during planning");
            parameters.push_str(&format!("<RealParameter id=\"param-{state_index}-{parameter_index}\" parameterID=\"{}\" name=\"{}\" unit=\"normalized\" min=\"0\" max=\"1\" value=\"{}\"/>",parameter.id,esc(&parameter.path),value));
        }
        out.push_str("<Parameters>");
        out.push_str(&parameters);
        out.push_str("</Parameters>");
        out.push_str(&format!("<Enabled id=\"muz-enabled-{state_index}\" name=\"On/Off\" value=\"true\"/><State path=\"{name}\"/></ClapPlugin>"));
    }
}
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn db_to_amp(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
fn collect_provenance(compiled: &Compiled) -> Result<String> {
    const MAX_MODULE_BYTES: usize = 1_048_576;
    const MAX_TOTAL_BYTES: usize = 4 * MAX_MODULE_BYTES;
    let source = compiled
        .session
        .extras
        .source
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("compiled source path unavailable"))?;
    let mut modules = vec![source.clone()];
    modules.extend(
        compiled
            .session
            .extras
            .dependencies
            .iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "muz"))
            .cloned(),
    );
    modules.sort();
    modules.dedup();
    let mut text = String::new();
    for module in modules {
        let bytes = crate::assets::read_bounded(&module, MAX_MODULE_BYTES)?;
        let contents = std::str::from_utf8(&bytes)?;
        ensure!(
            text.len().saturating_add(contents.len()) <= MAX_TOTAL_BYTES,
            "original source exceeds provenance size limit"
        );
        let label = module
            .strip_prefix(source.parent().unwrap_or(Path::new(".")))
            .unwrap_or(&module);
        text.push_str(&format!("// Source module: {}\n", label.display()));
        text.push_str(contents);
        text.push('\n');
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn clap_payload<'a>(bytes: &'a [u8], plugin_id: &str) -> &'a [u8] {
        assert!(bytes.starts_with(b"clap"));
        let len = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
        assert_eq!(&bytes[8..8 + len], plugin_id.as_bytes());
        &bytes[8 + len..]
    }

    #[test]
    fn clap_preset_container_matches_bitwig_header() -> Result<()> {
        let mut bytes = Vec::new();
        write_clap_preset(&mut bytes, "org.surge-synth-team.surge-xt", b"sub3\x01\x02")?;
        assert_eq!(&bytes[..8], b"clap\0\0\0\x1d");
        assert_eq!(
            clap_payload(&bytes, "org.surge-synth-team.surge-xt"),
            b"sub3\x01\x02"
        );
        assert_eq!(
            clap_preset_payload(&bytes)?,
            ("org.surge-synth-team.surge-xt", b"sub3\x01\x02".as_slice())
        );
        assert!(clap_preset_payload(b"clap\0\0\0\x1dshort").is_err());
        assert!(clap_preset_payload(b"nope\0\0\0\x01x").is_err());
        assert!(clap_preset_payload(b"clap\0\0\0\0").is_err());
        Ok(())
    }

    #[test]
    fn editable_notes_and_strict_atomicity() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("song.muz");
        fs::write(
            &source,
            "song({ tempo: 120, tracks: [track(\"lead\", phrase(\"C4:q E4:q G4:h\"), synth(\"pad\"))], tail: 0 })",
        )?;
        let compiled = crate::compile::compile(&source)?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let output = dir.path().join("song.dawproject");
        let report = dir.path().join("song.report.json");
        fs::write(&output, b"existing")?;
        assert!(plan.write(&output, true, Some(&output)).is_err());
        assert_eq!(fs::read(&output)?, b"existing");
        assert!(plan.write(&output, true, Some(&report)).is_err());
        assert_eq!(fs::read(&output)?, b"existing");
        assert!(report.exists());
        assert!(output.with_extension("dawproject.report.txt").exists());
        assert!(
            plan.write(
                &output,
                true,
                Some(&output.with_extension("dawproject.report.txt"))
            )
            .is_err()
        );
        plan.write(&output, false, Some(&report))?;
        let file = fs::File::open(&output)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut xml = String::new();
        archive.by_name("project.xml")?.read_to_string(&mut xml)?;
        assert_eq!(xml.matches("<Note ").count(), 3);
        assert!(xml.contains("<Clip time=\"0\" duration=\"4\" playStart=\"0\">"));
        assert!(xml.contains("<ClapPlugin"));
        assert!(archive.by_name("metadata.xml").is_ok());
        let mut state = Vec::new();
        archive
            .by_name("plugins/0.clap-preset")?
            .read_to_end(&mut state)?;
        let state = DeviceState::decode(clap_payload(&state, crate::device_state::INSTRUMENT_ID))?;
        assert!(
            state
                .original_source
                .as_deref()
                .is_some_and(|source| source.contains("phrase(\"C4:q E4:q G4:h\")"))
        );
        Ok(())
    }

    #[test]
    fn routes_tempo_sections_and_group_are_addressable() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("routing.muz");
        fs::write(
            &source,
            r#"song({
            tempo:120,tempos:[[2b,90]],sections:[section("A",4b),section("B",4b)],
            tracks:[track("lead",phrase("C4:q E4:q G4:h"),synth("pad"),
                {gain:-6,sends:{room:{gain:-12,pre:false}},output:"room"})],
            buses:[bus("room",[])],
            automation:[
                automation("lead.out",curve([[0b,-6],[2b,0]])),
                automation("lead.send.room",curve([[0b,-12],[2b,-3]],"step")),
                automation("room.out",curve([[0b,-2],[2b,0]]))
            ],tail:0
        })"#,
        )?;
        let mut compiled = crate::compile::compile(&source)?;
        compiled
            .session
            .extras
            .track_groups
            .push(crate::description::TrackGroup {
                id: "grouped".into(),
                kind: "authored".into(),
                members: vec![crate::description::TrackGroupMember {
                    track: "lead".into(),
                    label: "Lead".into(),
                }],
            });
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let xml = plan.xml()?;
        assert!(xml.contains(
            "<Track id=\"group-0\" name=\"grouped\" contentType=\"tracks\"><Track id=\"t0\""
        ));
        assert!(xml.contains("destination=\"bus-channel-0\" role=\"regular\""));
        assert!(xml.contains("destination=\"bus-channel-0\" type=\"post\"><Volume"));
        assert!(xml.contains("<Track id=\"bus-track-0\" name=\"room\""));
        assert!(xml.contains("<Marker time=\"4\" name=\"B\"/>"));
        assert!(xml.contains("<RealPoint time=\"2\""));
        assert!(xml.contains("<Target parameter=\"volume-c0\"/>"));
        assert!(xml.contains("<Target parameter=\"send-volume-c0-0\"/>"));
        assert!(xml.contains("<Target parameter=\"volume-bus-channel-0\"/>"));
        assert!(xml.contains("<RealPoint time=\"0\" value=\"0.5011872\""));
        assert!(
            !plan.report.entries.iter().any(|e| e.code == "ROUTE_OMITTED"
                || e.code == "SEND_OMITTED"
                || e.code == "TEMPO_MAP_OMITTED"
                || e.code == "SECTION_OMITTED"
                || e.code == "GROUP_OMITTED"
                || e.code == "AUTOMATION_OMITTED")
        );
        Ok(())
    }
    #[test]
    fn smooth_curve_sampling_has_bounded_value_error() {
        let lane = Automation {
            target: "device.gain_db".into(),
            shape: "smooth".into(),
            origin: None,
            points: vec![
                crate::description::CurvePoint {
                    seconds: 0.0,
                    value: -12.0,
                },
                crate::description::CurvePoint {
                    seconds: 2.0,
                    value: 0.0,
                },
            ],
        };
        let points = sampled_points(&lane, 0.001).unwrap();
        assert!(points.len() <= 4096);
        let mut max_error = 0.0_f64;
        for i in 0..2001 {
            let seconds = i as f64 / 1000.0;
            let j = points.partition_point(|p| p.0 <= seconds).saturating_sub(1);
            let estimate = if let Some(next) = points.get(j + 1) {
                let t = (seconds - points[j].0) / (next.0 - points[j].0);
                points[j].1 as f64 + (next.1 - points[j].1) as f64 * t
            } else {
                points[j].1 as f64
            };
            let t = seconds / 2.0;
            let actual = -12.0 + 12.0 * t * t * (3.0 - 2.0 * t);
            max_error = max_error.max((estimate - actual).abs());
        }
        assert!(max_error <= 0.0011, "max error {max_error}");
    }
    #[test]
    fn route_db_curve_is_sampled_in_linear_amplitude() {
        let lane = Automation {
            target: "lead.out".into(),
            shape: "linear".into(),
            origin: None,
            points: vec![
                crate::description::CurvePoint {
                    seconds: 0.0,
                    value: -90.0,
                },
                crate::description::CurvePoint {
                    seconds: 2.0,
                    value: 24.0,
                },
            ],
        };
        let points = converted_points(&lane, true, None).unwrap();
        assert!(points.len() > 2 && points.len() <= 4096);
        let mut max_error = 0.0_f64;
        for i in 0..2001 {
            let seconds = i as f64 / 1000.0;
            let index = points.partition_point(|p| p.0 <= seconds).saturating_sub(1);
            let estimate = if let Some(next) = points.get(index + 1) {
                let phase = (seconds - points[index].0) / (next.0 - points[index].0);
                points[index].1 as f64 + (next.1 - points[index].1) as f64 * phase
            } else {
                points[index].1 as f64
            };
            let expected = 10.0_f64.powf((-90.0 + 114.0 * seconds / 2.0) / 20.0);
            max_error = max_error.max((estimate - expected).abs());
        }
        assert!(max_error <= 0.0011, "max error {max_error}");
    }
    #[test]
    fn native_control_automation_targets_exposed_parameter() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("automation.muz");
        fs::write(
            &source,
            r#"song({tempo:120,tracks:[track("lead",phrase("C4:q"),synth("pad"),
            {chain:[fx("gain",{id:"level",gain_db:-6})]})],
            automation:[automation("lead.level.gain_db",curve([[0b,-12],[1b,0]],"smooth"))],tail:0})"#,
        )?;
        let compiled = crate::compile::compile(&source)?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let xml = plan.xml()?;
        assert!(xml.contains("<RealParameter id=\"param-1-"));
        assert!(xml.contains("<Points id=\"automation-0\" timeUnit=\"seconds\" unit=\"normalized\"><Target parameter=\"param-1-"));
        assert!(xml.matches("<RealPoint ").count() > 2);
        assert!(
            plan.report
                .entries
                .iter()
                .any(|e| e.code == "AUTOMATION_APPROXIMATED" && e.outcome == "approximated")
        );
        Ok(())
    }
    #[test]
    fn clap_cutoff_uses_host_normalized_value_domain() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("cutoff.muz");
        fs::write(
            &source,
            r#"song({tempo:120,tracks:[track("lead",phrase("C4:q"),synth("pad",{cutoff_hz:800}))],
                automation:[automation("lead.instrument.cutoff_hz",curve([[0b,800],[2b,3500],[4b,1200]]))],tail:0})"#,
        )?;
        let compiled = crate::compile::compile(&source)?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let xml = plan.xml()?;
        let native = DeviceState::decode(&plan.states[0].2)?;
        let cutoff = native
            .parameters
            .iter()
            .find(|p| p.path == "cutoff_hz")
            .unwrap();
        assert_eq!(cutoff.value, 800.0);
        assert!(xml.contains(&format!("parameterID=\"{}\" name=\"cutoff_hz\" unit=\"normalized\" min=\"0\" max=\"1\" value=\"0.039039", cutoff.id)));
        assert!(xml.contains("<Points id=\"automation-0\" timeUnit=\"seconds\" unit=\"normalized\"><Target parameter=\"param-0-1\"/>"));
        assert!(xml.contains("<RealPoint time=\"0\" value=\"0.039039"));
        assert!(xml.contains("<RealPoint time=\"1\" value=\"0.174174"));
        assert!(xml.contains("<RealPoint time=\"2\" value=\"0.059059"));
        Ok(())
    }
    #[test]
    fn wide_smooth_cutoff_keeps_bounded_normalized_curve() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("smooth-cutoff.muz");
        fs::write(
            &source,
            r#"song({tracks:[track("lead",phrase("C4:q"),synth("pad",{cutoff_hz:900}))],
                automation:[automation("lead.instrument.cutoff_hz",curve([[0b,900],[4b,4000]],"smooth"))],tail:0})"#,
        )?;
        let compiled = crate::compile::compile(&source)?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let xml = plan.xml()?;
        assert!(
            xml.contains("<Points id=\"automation-0\" timeUnit=\"seconds\" unit=\"normalized\"")
        );
        assert!(xml.matches("<RealPoint ").count() >= 3);
        assert!(
            plan.report
                .entries
                .iter()
                .any(|entry| entry.code == "AUTOMATION_APPROXIMATED")
        );
        assert!(
            !plan
                .report
                .entries
                .iter()
                .any(|entry| entry.code == "AUTOMATION_DENSITY_OMITTED")
        );
        Ok(())
    }
    #[test]
    fn voice_patch_declared_control_has_parameter_and_automation_target() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("patch.muz");
        fs::write(
            &source,
            r#"let voice=voice_patch("simple",{
            nodes:[
                {id:"cutoff",op:"param",value:2400,min:80,max:10000},
                {id:"osc",op:"osc",wave:"saw"},
                {id:"out",op:"mul",inputs:["osc","cutoff"]}
            ],output:"out"});
            song({tracks:[track("lead",phrase("C4:q"),voice)],
              automation:[automation("lead.instrument.cutoff",curve([[0b,2400],[1b,5000]]))],tail:0})"#,
        )?;
        let compiled = crate::compile::compile(&source)?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let xml = plan.xml()?;
        assert!(
            xml.contains("name=\"cutoff\" unit=\"normalized\" min=\"0\" max=\"1\" value=\"0.23387")
        );
        assert!(xml.contains("<Target parameter=\"param-0-0\"/>"));
        assert!(
            !plan
                .report
                .entries
                .iter()
                .any(|entry| entry.code == "AUTOMATION_OMITTED")
        );
        Ok(())
    }
    #[cfg(feature = "desktop")]
    #[test]
    fn external_clap_snapshot_applies_override_and_embeds_state() -> Result<()> {
        let bundle = Path::new("/usr/lib/clap/ZamEQ2.clap");
        if !bundle.exists() {
            return Ok(());
        }
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("external.muz");
        fs::write(
            &source,
            r#"song({tracks:[track("lead",phrase("C4:q"),synth("pad"),
                {chain:[plugin("/usr/lib/clap/ZamEQ2.clap",
                    {id:"eq",class:"com.zamaudio.ZamEQ2",p0:3})]})],
                automation:[automation("lead.eq.p0",curve([[0b,3],[1b,5]]))],tail:0})"#,
        )?;
        let compiled = crate::compile::compile(&source)?;
        let mut plan = Plan::new(&compiled, "bitwig-linux")?;
        let snapshot = &plan.external_states[0];
        assert_eq!(snapshot.plugin_id, "com.zamaudio.ZamEQ2");
        assert!(
            snapshot
                .parameters
                .iter()
                .any(|p| p.id == 0 && p.value == 3.0)
        );
        assert!(!snapshot.bytes.is_empty());
        let xml = plan.xml()?;
        assert!(xml.contains("<ClapPlugin id=\"external-device-0\""));
        assert!(xml.contains("deviceID=\"com.zamaudio.ZamEQ2\""));
        assert!(xml.contains("parameterID=\"0\" name=\"Boost/Cut 1\" unit=\"normalized\" min=\"0\" max=\"1\" value=\"0.575\""));
        assert!(xml.contains("<Points id=\"automation-0\" timeUnit=\"seconds\" unit=\"normalized\"><Target parameter=\"external-param-0-0\"/>"));
        assert!(xml.contains("<RealPoint time=\"0\" value=\"0.575\""));
        assert!(xml.contains("<RealPoint time=\"0.5\" value=\"0.625\""));
        assert!(
            plan.report
                .required_plugin_ids
                .iter()
                .any(|id| id == "com.zamaudio.ZamEQ2")
        );
        assert!(
            plan.report
                .external_dependencies
                .iter()
                .any(|path| path == "/usr/lib/clap/ZamEQ2.clap")
        );
        assert!(
            plan.report
                .entries
                .iter()
                .any(|entry| entry.code == "EXTERNAL_PLUGIN_DEPENDENCY"
                    && entry.outcome == "manual setup required")
        );
        assert!(
            plan.report
                .entries
                .iter()
                .any(|entry| entry.code == "PLUGIN_RUNTIME_UNVERIFIED"
                    && entry.physical_path == "track.lead.insert.0")
        );
        let output = dir.path().join("external.dawproject");
        plan.write(&output, false, None)?;
        let mut archive = zip::ZipArchive::new(fs::File::open(output)?)?;
        let mut embedded = Vec::new();
        archive
            .by_name("plugins/external-0.clap-preset")?
            .read_to_end(&mut embedded)?;
        assert_eq!(clap_payload(&embedded, &snapshot.plugin_id), snapshot.bytes);
        let state_path = dir.path().join("restored.clap-preset");
        fs::write(&state_path, clap_payload(&embedded, &snapshot.plugin_id))?;
        let mut restored = crate::audio::clap::PreparedClap::open(
            bundle,
            Some("com.zamaudio.ZamEQ2"),
            crate::audio::AudioConfig {
                sample_rate: 48000.0,
                max_frames: 256,
                offline: false,
            },
            0,
        )?;
        restored.load_state(&state_path)?;
        restored.finish_preparation()?;
        assert!(
            restored
                .parameter_values()?
                .iter()
                .any(|(id, value)| *id == 0 && (*value - 3.0).abs() < 1e-6)
        );
        plan.external_states[0].parameters[0].max = plan.external_states[0].parameters[0].min;
        let error = plan
            .validate_plugin_parameter_ranges()
            .unwrap_err()
            .to_string();
        assert!(error.contains("track.lead.insert.0.Boost/Cut 1 (ID 0)"));
        Ok(())
    }
    #[cfg(feature = "desktop")]
    #[test]
    fn external_vst3_component_round_trips_with_effective_override() -> Result<()> {
        let bundle = Path::new("/usr/lib/vst3/CHOWTapeModel.vst3");
        if !bundle.exists() {
            return Ok(());
        }
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("external_vst3.muz");
        fs::write(
            &source,
            r#"song({tracks:[track("lead",phrase("C4:q"),synth("pad"),
            {chain:[plugin("/usr/lib/vst3/CHOWTapeModel.vst3",
                {class:"ABCDEF019182FAEB43686F774A646F78",dry_wet:0.3})]})],tail:0})"#,
        )?;
        let compiled = crate::compile::compile(&source)?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let snapshot = &plan.external_states[0];
        assert_eq!(snapshot.plugin_id, "ABCDEF019182FAEB43686F774A646F78");
        assert_eq!(snapshot.version, "2.11.4");
        assert!(snapshot
            .parameters
            .iter()
            .any(|parameter| parameter.key == "dry_wet" && (parameter.value - 0.3).abs() < 1e-6));
        assert!(snapshot.bytes.starts_with(b"VST3"));
        let xml = plan.xml()?;
        assert!(xml.contains("<Vst3Plugin id=\"external-device-0\""));
        assert!(xml.contains("deviceID=\"ABCDEF019182FAEB43686F774A646F78\""));
        assert!(xml.contains("pluginVersion=\"2.11.4\""));
        assert!(xml.contains("parameterID=\"824435163\" name=\"Dry/Wet\" unit=\"normalized\" min=\"0\" max=\"1\" value=\""));
        assert!(
            plan.report
                .entries
                .iter()
                .any(|entry| entry.code == "VST3_CONTROLLER_STATE_OMITTED")
        );
        let output = dir.path().join("external_vst3.dawproject");
        plan.write(&output, false, None)?;
        let mut archive = zip::ZipArchive::new(fs::File::open(output)?)?;
        let mut embedded = Vec::new();
        archive
            .by_name("plugins/external-0.vstpreset")?
            .read_to_end(&mut embedded)?;
        assert_eq!(embedded, snapshot.bytes);
        Ok(())
    }
    #[test]
    fn vstpreset_wraps_component_with_class_and_chunk_table() -> Result<()> {
        let class = "ABCDEF019182FAEB43686F774A646F78";
        let bytes = vstpreset(class, b"component")?;
        assert_eq!(&bytes[..4], b"VST3");
        assert_eq!(&bytes[8..40], class.as_bytes());
        let list = u64::from_le_bytes(bytes[40..48].try_into()?) as usize;
        assert_eq!(&bytes[48..list], b"component");
        assert_eq!(&bytes[list..list + 4], b"List");
        assert_eq!(&bytes[list + 8..list + 12], b"Comp");
        Ok(())
    }
    #[test]
    fn note_expression_and_pedal_keep_note_and_channel_identity() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("expressive.muz");
        fs::write(
            &source,
            r#"song({tracks:[track("lead",phrase("C4:q"),synth("pad"))],tail:0})"#,
        )?;
        let mut compiled = crate::compile::compile(&source)?;
        let TrackSource::Midi(midi) = &mut compiled.session.tracks[0].source else {
            unreachable!()
        };
        let note = &mut midi.imported.notes[0];
        note.performance = Some(crate::expression::Performance {
            pitch: note.key as f64 + 0.25,
            velocity: 0.7,
            expression: crate::expression::Program::parse(Some(&serde_json::json!({
                "tuning": [[0.0, 0.0], [1.0, 0.5]],
                "brightness": [[0.0, 0.2], [1.0, 0.8]],
                "pressure": [[0.0, 0.1], [1.0, 0.9]],
                "pan": 0.6
            })))?,
        });
        note.annotations
            .insert("sample_zone".into(), serde_json::json!(2));
        midi.imported.controllers.push(crate::midi::MidiController {
            tick: 0,
            channel: note.channel,
            controller: 64,
            value: 127,
            source_order: 0,
        });
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let xml = plan.xml()?;
        assert!(xml.contains("<Target expression=\"transpose\"/>"));
        assert!(xml.contains("<Target expression=\"pressure\"/>"));
        assert!(xml.contains("<Target expression=\"timbre\"/>"));
        assert!(xml.contains(
            "<Target expression=\"channelController\" channel=\"0\" controller=\"64\"/>"
        ));
        assert!(xml.contains("unit=\"semitones\""));
        assert!(xml.contains("value=\"0.25\""));
        assert!(
            plan.report
                .entries
                .iter()
                .any(|e| e.code == "NOTE_EXPRESSION_OMITTED"
                    && e.physical_path.ends_with("expression.1"))
        );
        assert!(
            plan.report
                .entries
                .iter()
                .any(|e| e.code == "SAMPLE_ZONE_OMITTED"
                    && e.remedy.contains("original muz project")
                    && !e.remedy.contains("re-export"))
        );
        assert!(
            !plan
                .report
                .entries
                .iter()
                .any(|e| e.code == "CONTROLLER_OMITTED" || e.code == "NOTE_TUNING_OMITTED")
        );
        Ok(())
    }
    #[test]
    fn channel_filtered_events_do_not_create_loss_warnings() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("channel.muz");
        fs::write(
            &source,
            r#"song({tracks:[track("lead",phrase("C4:q"),synth("pad"))],tail:0})"#,
        )?;
        let mut compiled = crate::compile::compile(&source)?;
        let TrackSource::Midi(midi) = &mut compiled.session.tracks[0].source else {
            unreachable!()
        };
        midi.all_channels = false;
        midi.channel = 0;
        let mut excluded = midi.imported.notes[0].clone();
        excluded.id = "excluded".into();
        excluded.channel = 1;
        excluded.source_order += 1;
        excluded
            .annotations
            .insert("sample_zone".into(), serde_json::json!(0));
        midi.imported.notes.push(excluded);
        midi.imported.controllers.push(crate::midi::MidiController {
            tick: 0,
            channel: 1,
            controller: 64,
            value: 127,
            source_order: 3,
        });
        midi.imported.messages.push(crate::midi::ChannelMessage {
            tick: 0,
            bytes: [0xb1, 7, 127],
            len: 3,
            source_order: 4,
        });
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let xml = plan.xml()?;
        assert_eq!(xml.matches("<Note ").count(), 1);
        assert!(!plan.report.entries.iter().any(|entry| {
            entry.physical_path.contains("excluded")
                || entry.code == "SAMPLE_ZONE_OMITTED"
                || entry.code == "CONTROLLER_IMPORT_UNVERIFIED"
                || entry.code == "MIDI_MESSAGE_OMITTED"
        }));
        Ok(())
    }
    #[test]
    fn kit_choke_reports_cross_voice_delivery_risk() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("kit.muz");
        fs::write(
            &source,
            r#"song({tracks:[track("drums",drums({hat:"x.x.",open_hat:".x.."}),kit())],tail:0})"#,
        )?;
        let compiled = crate::compile::compile(&source)?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let xml = plan.xml()?;
        assert!(xml.contains(
            "<Target expression=\"channelController\" channel=\"0\" controller=\"120\"/>"
        ));
        for voice in ["hat", "open_hat"] {
            assert!(plan.report.entries.iter().any(|entry| {
                entry.code == "KIT_CHOKE_IMPORT_UNVERIFIED"
                    && entry.physical_path == format!("track.drums.{voice}.choke")
            }));
        }
        Ok(())
    }
    #[test]
    fn sidechain_report_preserves_missing_response_and_conditional_remedy() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("sidechain.muz");
        fs::write(
            &source,
            r#"song({tracks:[
                track("kick",phrase("C2:q"),synth("kick")),
                track("bass",phrase("C2:q"),synth("pulse-bass"),{
                    chain:[fx("compressor",{id:"duck",sidechain:"kick",threshold_db:-24,ratio:4})]
                })
            ],tail:0})"#,
        )?;
        let compiled = crate::compile::compile(&source)?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let warning = plan
            .report
            .entries
            .iter()
            .find(|entry| entry.code == "SIDECHAIN_MANUAL_SETUP")
            .expect("sidechain warning");
        assert_eq!(warning.physical_path, "track.bass.insert.0.sidechain");
        assert_eq!(warning.outcome, "manual setup required");
        assert!(warning.exported.contains("response is absent"));
        assert!(warning.remedy.contains("if the host exposes"));
        assert!(warning.remedy.contains("otherwise use native Muz playback"));
        Ok(())
    }
    #[test]
    fn bitwig_studio_cc11_report_records_audible_probe_limit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("controllers.muz");
        fs::write(
            &source,
            r#"song({tracks:[track("lead",stack([note("C4",4b),cc(11,0,at=0b),cc(64,127,at=1b)]),synth("pad"))],tail:0})"#,
        )?;
        let compiled = crate::compile::compile(&source)?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let xml = plan.xml()?;
        assert!(xml.contains(
            "<Target expression=\"channelController\" channel=\"0\" controller=\"11\"/>"
        ));
        assert!(xml.contains(
            "<Target expression=\"channelController\" channel=\"0\" controller=\"64\"/>"
        ));
        let warnings: Vec<_> = plan
            .report
            .entries
            .iter()
            .filter(|entry| entry.code == "CONTROLLER_IMPORT_UNVERIFIED")
            .collect();
        assert_eq!(warnings.len(), 2);
        let cc11 = warnings
            .iter()
            .find(|entry| entry.intended == "CC 11 value 0")
            .unwrap();
        assert_eq!(cc11.outcome, "unverified");
        assert!(cc11.exported.contains("byte-identical audio"));
        assert!(cc11.remedy.contains("use native Muz playback"));
        let cc64 = warnings
            .iter()
            .find(|entry| entry.intended == "CC 64 value 127")
            .unwrap();
        assert!(!cc64.exported.contains("byte-identical audio"));
        assert!(cc64.remedy.contains("confirm the controller lane"));
        Ok(())
    }
    #[test]
    fn expression_phase_crossing_tempo_change_maps_to_elapsed_time() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("tempo_expression.muz");
        fs::write(
            &source,
            r#"song({tempo:120,tempos:[[2b,60]],
            tracks:[track("lead",phrase("C4:w"),synth("pad"))],tail:0})"#,
        )?;
        let mut compiled = crate::compile::compile(&source)?;
        let TrackSource::Midi(midi) = &mut compiled.session.tracks[0].source else {
            unreachable!()
        };
        let note = &mut midi.imported.notes[0];
        note.performance.as_mut().unwrap().expression = crate::expression::Program::parse(Some(
            &serde_json::json!({"pressure":[[0.0,0.0],[0.5,0.5],[1.0,1.0]]}),
        ))?;
        let plan = Plan::new(&compiled, "bitwig-linux")?;
        let TrackSource::Midi(midi) = &compiled.session.tracks[0].source else {
            unreachable!()
        };
        let points = plan.note_expression_points(&midi.imported.notes[0], 6, 0.0);
        assert!(
            points.iter().any(
                |(time, value)| (*time - 2.0).abs() < 1e-6 && (*value - 1.0 / 2.6).abs() < 1e-5
            )
        );
        assert!(
            points
                .iter()
                .any(|(time, value)| (*time - 2.3).abs() < 1e-6 && (*value - 0.5).abs() < 1e-6)
        );
        assert!(
            plan.report
                .entries
                .iter()
                .any(|entry| entry.code == "NOTE_EXPRESSION_TIMING_APPROXIMATED"
                    && entry.outcome == "approximated")
        );
        assert!(plan.xml()?.contains("time=\"2.3\" value=\"0.5\""));
        Ok(())
    }
}
