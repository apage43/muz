//! Stable, bounded state shared by DAWProject export and the Muz CLAP wrapper.
//! The evaluated device is authoritative on restore; source is retained for explicit edits.
use std::collections::{BTreeMap, BTreeSet};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use anyhow::{Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    description::{self, ParameterUnit},
    lang, model,
};

pub const INSTRUMENT_ID: &str = "com.plausiblyreliable.muz.instrument";
pub const FX_ID: &str = "com.plausiblyreliable.muz.fx";
pub const STATE_VERSION: u32 = 3;
pub const MAX_STATE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ASSET_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRole {
    Instrument,
    Fx,
}

impl DeviceRole {
    pub fn plugin_id(self) -> &'static str {
        match self {
            Self::Instrument => INSTRUMENT_ID,
            Self::Fx => FX_ID,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterIdentity {
    pub path: String,
    pub id: u32,
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedAsset {
    pub path: String,
    pub sha256: String,
    pub data_base64: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceState {
    /// Local dependencies remain external unless explicitly licensed for embedding.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub linked_assets: BTreeMap<String, String>,
    pub version: u32,
    pub engine_version: String,
    pub role: DeviceRole,
    /// Authored or generated standalone Muz device source. Evaluated state is authoritative.
    pub source: String,
    pub device: model::Device,
    pub parameters: Vec<ParameterIdentity>,
    #[serde(default)]
    pub assets: Vec<EmbeddedAsset>,
    /// Original author text is provenance only; `source` is the executable module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_source: Option<String>,
}

impl DeviceState {
    /// Snapshot assets, replace machine paths, and generate a standalone Muz module.
    pub fn from_device(role: DeviceRole, mut device: model::Device) -> Result<Self> {
        validate_supported_rack(&device)?;
        let linked = device.sfz.as_ref().is_some_and(|s| !s.embed_assets);
        let assets = if linked {
            Vec::new()
        } else {
            package_assets(&mut device)?
        };
        let source = source_for_device(&device)?;
        Self::build(role, source, device, assets, None)
    }

    pub fn new(role: DeviceRole, source: String, device: model::Device) -> Result<Self> {
        Self::build(role, source, device, Vec::new(), None)
    }

    fn build(
        role: DeviceRole,
        source: String,
        mut device: model::Device,
        assets: Vec<EmbeddedAsset>,
        original_source: Option<String>,
    ) -> Result<Self> {
        description::validate_device(&device)?;
        validate_supported_rack(&device)?;
        ensure!(
            device.sidechain.is_none()
                || matches!(
                    device.kind,
                    model::DeviceKind::Compressor | model::DeviceKind::Rack
                ),
            "standalone detector input requires compressor or rack"
        );
        ensure!(
            !matches!(
                device.kind,
                model::DeviceKind::Vst3 | model::DeviceKind::Clap
            ),
            "Muz plugin state cannot host another plugin"
        );
        ensure!(
            assets.is_empty()
                || crate::assets::paths(&device)
                    .iter()
                    .all(|p| p.starts_with("/muz-assets")),
            "embedded assets require virtual paths"
        );
        ensure!(!source.trim().is_empty(), "Muz plugin source is required");
        let reconstructed = reconstruct(&source, device.id.as_str())?;
        ensure!(
            same_structure(&reconstructed, &device),
            "Muz source does not reconstruct evaluated device"
        );
        // Materialize every host-visible control before processor preparation. This
        // keeps default controls addressable without inserting into a map on audio.
        for (name, value) in realtime_controls(&device) {
            device.params.entry(name).or_insert(value);
        }
        let parameters = realtime_controls(&device)
            .into_iter()
            .map(|(path, value)| ParameterIdentity {
                id: stable_id(&path),
                path,
                value,
            })
            .collect();
        let linked_assets = if device.sfz.as_ref().is_some_and(|s| !s.embed_assets) {
            crate::assets::paths(&device)
                .iter()
                .map(|p| {
                    Ok((
                        p.display().to_string(),
                        digest_reader(crate::assets::resolver().open(p)?)?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?
        } else {
            BTreeMap::new()
        };
        if !linked_assets.is_empty() {
            device.asset_versions = crate::assets::paths(&device)
                .iter()
                .map(|p| {
                    let hash = &linked_assets[&p.display().to_string()];
                    Ok((
                        crate::assets::resolver().version(p)?.0,
                        asset_version(hash)?,
                    ))
                })
                .collect::<Result<_>>()?;
        }
        let state = Self {
            linked_assets,
            version: STATE_VERSION,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            role,
            source,
            device,
            parameters,
            assets,
            original_source,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == STATE_VERSION,
            "unsupported Muz device state version {}",
            self.version
        );
        ensure!(
            self.engine_version == env!("CARGO_PKG_VERSION"),
            "incompatible Muz engine version {}",
            self.engine_version
        );
        description::validate_device(&self.device)?;
        validate_supported_rack(&self.device)?;
        ensure!(
            self.device.sidechain.is_none()
                || matches!(
                    self.device.kind,
                    model::DeviceKind::Compressor | model::DeviceKind::Rack
                ),
            "standalone detector input requires compressor or rack"
        );
        ensure!(
            !matches!(
                self.device.kind,
                model::DeviceKind::Vst3 | model::DeviceKind::Clap
            ),
            "nested plugin state unsupported"
        );
        validate_assets(self)?;
        ensure!(
            self.device.kind.is_instrument() == (self.role == DeviceRole::Instrument),
            "device role does not match plugin role"
        );
        ensure!(
            !self.source.trim().is_empty(),
            "Muz plugin source is required"
        );
        ensure!(
            same_structure(
                &reconstruct(&self.source, self.device.id.as_str())?,
                &self.device
            ),
            "Muz source does not reconstruct evaluated device"
        );
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let values: BTreeMap<_, _> = realtime_controls(&self.device).into_iter().collect();
        ensure!(
            self.parameters.len() == values.len(),
            "parameter table differs from evaluated device"
        );
        for p in &self.parameters {
            ensure!(
                p.id != u32::MAX && ids.insert(p.id),
                "duplicate or invalid parameter ID"
            );
            ensure!(paths.insert(&p.path), "duplicate parameter path");
            ensure!(
                values.get(&p.path).is_some_and(|v| *v == p.value),
                "parameter table differs from evaluated device"
            );
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        ensure!(bytes.len() <= MAX_STATE_BYTES, "Muz device state too large");
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_STATE_BYTES, "Muz device state too large");
        let state: Self = serde_json::from_slice(bytes)?;
        state.validate()?;
        Ok(state)
    }

    /// Keep existing host automation values and IDs when replacing evaluated source.
    pub fn replace_device(&self, source: String, device: model::Device) -> Result<Self> {
        let mut next = Self::build(
            self.role,
            source,
            device,
            self.assets.clone(),
            self.original_source.clone(),
        )?;
        for p in &mut next.parameters {
            if let Some(old) = self.parameters.iter().find(|old| old.path == p.path) {
                p.id = old.id;
                p.value = old.value;
                next.device.params.insert(p.path.clone(), old.value);
            }
        }
        if next
            .parameters
            .iter()
            .map(|p| p.id)
            .collect::<BTreeSet<_>>()
            .len()
            != next.parameters.len()
        {
            bail!("parameter ID collision on source replacement");
        }
        next.validate()?;
        Ok(next)
    }

    pub fn host_context(&self) -> Result<crate::host::HostContext> {
        self.host_context_with_resolver(Arc::new(crate::assets::FileAssets))
    }

    /// Restore linked dependencies through an embedding host's resolver (including WASM).
    pub fn host_context_with_resolver(
        &self,
        linked: Arc<dyn crate::assets::AssetResolver>,
    ) -> Result<crate::host::HostContext> {
        self.validate()?;
        let mut memory = crate::assets::MemoryAssets::default();
        for asset in &self.assets {
            let bytes: Arc<[u8]> = STANDARD.decode(&asset.data_base64)?.into();
            memory.insert(
                PathBuf::from(&asset.path),
                bytes,
                asset_version(&asset.sha256)?,
            );
        }
        Ok(crate::host::HostContext {
            assets: if self.linked_assets.is_empty() {
                Arc::new(memory)
            } else {
                for (path, hash) in &self.linked_assets {
                    ensure!(
                        digest_reader(linked.open(Path::new(path))?)? == *hash,
                        "missing or modified linked SFZ asset {path}"
                    );
                }
                let mut pins = BTreeMap::new();
                for (path, stamp) in crate::assets::paths(&self.device)
                    .into_iter()
                    .zip(self.device.asset_versions.iter().copied())
                {
                    pins.insert(path.clone(), (linked.version(&path)?, stamp));
                }
                Arc::new(VerifiedLinkedAssets {
                    inner: linked,
                    pins,
                })
            },
            ..Default::default()
        })
    }
}

struct VerifiedLinkedAssets {
    inner: Arc<dyn crate::assets::AssetResolver>,
    pins: BTreeMap<PathBuf, ((u64, u128), (u64, u128))>,
}
impl crate::assets::AssetResolver for VerifiedLinkedAssets {
    fn resolve(&self, path: &Path) -> Result<PathBuf> {
        ensure!(
            self.pins.contains_key(path),
            "unlisted linked SFZ asset {}",
            path.display()
        );
        Ok(path.to_owned())
    }
    fn version(&self, path: &Path) -> Result<(u64, u128)> {
        let (metadata, content) = self
            .pins
            .get(path)
            .ok_or_else(|| anyhow::anyhow!("unlisted linked asset"))?;
        ensure!(
            self.inner.version(path)? == *metadata,
            "linked SFZ asset changed after verification: {}",
            path.display()
        );
        Ok(*content)
    }
    fn open(&self, path: &Path) -> Result<Box<dyn crate::assets::AssetReader>> {
        self.version(path)?;
        self.inner.open(path)
    }
}
fn digest_reader(mut reader: Box<dyn crate::assets::AssetReader>) -> Result<String> {
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 65536];
    loop {
        crate::host::check_cancelled()?;
        let n = std::io::Read::read(&mut reader, &mut chunk)?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn asset_version(hash: &str) -> Result<u128> {
    ensure!(hash.len() == 64, "invalid embedded asset digest");
    Ok(u128::from_str_radix(&hash[..32], 16)?)
}
fn validate_assets(state: &DeviceState) -> Result<()> {
    let referenced = crate::assets::paths(&state.device);
    if !state.linked_assets.is_empty() {
        ensure!(
            state.device.sfz.as_ref().is_some_and(|s| !s.embed_assets) && state.assets.is_empty(),
            "linked assets require non-embedded SFZ state"
        );
        ensure!(
            referenced.len() == state.linked_assets.len()
                && referenced.len() == state.device.asset_versions.len(),
            "linked asset table differs from device dependencies"
        );
        for path in referenced {
            let hash = state
                .linked_assets
                .get(&path.display().to_string())
                .ok_or_else(|| anyhow::anyhow!("missing linked dependency {}", path.display()))?;
            ensure!(
                path.is_absolute()
                    && hash.len() == 64
                    && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid linked SFZ dependency"
            );
        }
        return Ok(());
    }
    ensure!(
        referenced.len() == state.assets.len(),
        "embedded asset table differs from device paths"
    );
    ensure!(
        referenced.len() == state.device.asset_versions.len(),
        "embedded asset versions differ from paths"
    );
    let mut total = 0usize;
    let mut by_path = BTreeMap::new();
    for asset in &state.assets {
        ensure!(
            asset.path.starts_with("/muz-assets/") && !asset.path.contains(".."),
            "invalid embedded asset path"
        );
        ensure!(
            asset.data_base64.len() <= MAX_ASSET_BYTES * 4 / 3 + 8,
            "embedded asset exceeds byte limit"
        );
        let bytes = STANDARD.decode(&asset.data_base64)?;
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| anyhow::anyhow!("embedded asset size overflow"))?;
        ensure!(
            total <= MAX_ASSET_BYTES,
            "embedded asset bytes exceed limit"
        );
        ensure!(
            digest(&bytes) == asset.sha256,
            "embedded asset digest mismatch"
        );
        ensure!(
            by_path
                .insert(
                    PathBuf::from(&asset.path),
                    (bytes.len() as u64, asset_version(&asset.sha256)?)
                )
                .is_none(),
            "duplicate embedded asset path"
        );
    }
    for (path, stamp) in referenced.iter().zip(&state.device.asset_versions) {
        ensure!(
            by_path.get(path) == Some(stamp),
            "missing or stale embedded asset {}",
            path.display()
        );
    }
    Ok(())
}
fn package_assets(device: &mut model::Device) -> Result<Vec<EmbeddedAsset>> {
    let originals = crate::assets::paths(device);
    let mut mapping = BTreeMap::new();
    let mut embedded = BTreeMap::new();
    let mut total = 0usize;
    for original in originals {
        let ext = original
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        ensure!(
            matches!(ext.as_str(), "wav" | "flac")
                || device
                    .sfz
                    .as_ref()
                    .is_some_and(|s| s.program.dependencies.contains(&original)),
            "unsupported portable sample extension: {}",
            original.display()
        );
        let remaining = MAX_ASSET_BYTES.saturating_sub(total);
        let snapshot = crate::assets::snapshot(&original, remaining)?;
        total += snapshot.bytes.len();
        let hash = digest(&snapshot.bytes);
        let path = format!("/muz-assets/{hash}.{ext}");
        mapping.insert(original, path.clone());
        embedded.entry(path.clone()).or_insert(EmbeddedAsset {
            path,
            sha256: hash,
            data_base64: STANDARD.encode(&snapshot.bytes),
        });
    }
    if let Some(config) = &mut device.sfz {
        for path in &mut config.program.dependencies {
            *path = PathBuf::from(
                mapping
                    .get(path)
                    .ok_or_else(|| anyhow::anyhow!("missing SFZ dependency mapping"))?,
            );
        }
        for region in &mut config.program.regions {
            if let Some(path) = &mut region.sample {
                *path = PathBuf::from(
                    mapping
                        .get(path)
                        .ok_or_else(|| anyhow::anyhow!("missing SFZ sample mapping"))?,
                );
            }
        }
    }
    if let Some(zones) = &mut device.sample {
        for zone in zones {
            zone.path = mapping
                .get(Path::new(&zone.path))
                .ok_or_else(|| anyhow::anyhow!("missing sample asset mapping"))?
                .clone();
        }
    }
    if let Some(patch) = &mut device.patch {
        let root = PathBuf::from(patch["_module_dir"].as_str().unwrap_or("."));
        if let Some(nodes) = patch["nodes"].as_array_mut() {
            for node in nodes {
                if node["op"] == "reader" {
                    if let Some(zones) = node["zones"].as_array_mut() {
                        for zone in zones {
                            if let Some(old) = zone["path"].as_str() {
                                zone["path"] = mapping
                                    .get(Path::new(old))
                                    .ok_or_else(|| anyhow::anyhow!("missing reader asset mapping"))?
                                    .clone()
                                    .into();
                            }
                        }
                    }
                } else if node["op"] == "sample" {
                    if let Some(old) = node["path"].as_str() {
                        node["path"] = mapping
                            .get(&root.join(old))
                            .ok_or_else(|| anyhow::anyhow!("missing patch sample mapping"))?
                            .clone()
                            .into();
                    }
                }
            }
        }
        patch["_module_dir"] = "/".into();
    }
    let assets: Vec<_> = embedded.into_values().collect();
    let stamps: BTreeMap<_, _> = assets
        .iter()
        .map(|a| {
            Ok((
                PathBuf::from(&a.path),
                (
                    STANDARD.decode(&a.data_base64)?.len() as u64,
                    asset_version(&a.sha256)?,
                ),
            ))
        })
        .collect::<Result<_>>()?;
    device.asset_versions = crate::assets::paths(device)
        .iter()
        .map(|p| {
            stamps
                .get(p)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("unmapped packaged asset {}", p.display()))
        })
        .collect::<Result<_>>()?;
    Ok(assets)
}

fn same_structure(a: &model::Device, b: &model::Device) -> bool {
    a.kind == b.kind
        && match (&a.patch, &b.patch) {
            (Some(a), Some(b)) => same_json_semantic(a, b),
            (None, None) => true,
            _ => false,
        }
        && a.rack == b.rack
        && a.sample == b.sample
        && a.sfz == b.sfz
        && a.sidechain == b.sidechain
        && a.params
            .iter()
            .filter(|(name, _)| !is_realtime_control(a, name))
            .eq(b
                .params
                .iter()
                .filter(|(name, _)| !is_realtime_control(b, name)))
}

fn contains_sfz(device: &model::Device) -> bool {
    device.sfz.is_some()
        || device
            .rack
            .as_ref()
            .is_some_and(|rack| rack.branches.iter().flatten().any(contains_sfz))
}

fn validate_supported_rack(device: &model::Device) -> Result<()> {
    if device.kind != model::DeviceKind::Rack {
        return Ok(());
    }
    let rack = device
        .rack
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing rack topology"))?;
    ensure!(
        !rack.branches.is_empty()
            && rack.branches.len() <= 8
            && rack.branches.iter().map(Vec::len).sum::<usize>() <= 32,
        "standalone rack exceeds branch/device limits"
    );
    ensure!(
        !rack.branches.iter().flatten().any(contains_sfz),
        "SFZ instruments inside effect racks are unsupported; save the SFZ instrument separately to preserve its linked-asset policy"
    );
    ensure!(
        crate::assets::paths(device).is_empty(),
        "sampled rack children are not yet portable"
    );
    for child in rack.branches.iter().flatten() {
        ensure!(
            !child.kind.is_instrument()
                && !matches!(
                    child.kind,
                    model::DeviceKind::Rack | model::DeviceKind::Vst3 | model::DeviceKind::Clap
                )
                && child.sidechain.is_none(),
            "standalone rack supports only native effects without nested sidechains"
        );
    }
    for name in rack.expose.keys() {
        ensure!(
            name != "mix" && name != "gain_db",
            "rack expose name conflicts with built-in control {name}"
        );
        ensure!(
            control_range(device, name).is_some(),
            "rack exposure {name} does not target a realtime native control"
        );
    }
    for name in device.params.keys() {
        ensure!(
            name == "mix" || name == "gain_db" || rack.expose.contains_key(name),
            "unknown rack control {name}"
        );
    }
    for (name, value) in &device.params {
        let (min, max, _) = control_range(device, name)
            .ok_or_else(|| anyhow::anyhow!("unknown rack control {name}"))?;
        ensure!(
            value.is_finite() && (min..=max).contains(value),
            "rack control {name} outside {min}..={max}"
        );
    }
    Ok(())
}

fn same_json_semantic(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => a.as_f64() == b.as_f64(),
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_json_semantic(a, b))
        }
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|w| same_json_semantic(v, w)))
        }
        _ => a == b,
    }
}

fn realtime_controls(device: &model::Device) -> BTreeMap<String, f32> {
    if device.kind == model::DeviceKind::Rack {
        let mut values = BTreeMap::from([("mix".into(), 1.0), ("gain_db".into(), 0.0)]);
        if let Some(rack) = &device.rack {
            for name in rack.expose.keys() {
                if let Some((_, _, default)) = control_range(device, name) {
                    values.insert(name.clone(), default);
                }
            }
        }
        values.extend(
            device
                .params
                .iter()
                .filter(|(name, _)| is_realtime_control(device, name))
                .map(|(name, value)| (name.clone(), *value)),
        );
        return values;
    }
    let mut values: BTreeMap<String, f32> = description::parameter_specs(device.kind)
        .iter()
        .filter(|spec| spec.effect == description::ParameterEffect::Control)
        .map(|spec| (spec.name.to_owned(), spec.default))
        .collect();
    values.extend(
        device
            .control_values()
            .into_iter()
            .filter(|(name, _)| is_realtime_control(device, name)),
    );
    values
}

fn is_realtime_control(device: &model::Device, name: &str) -> bool {
    if device.kind == model::DeviceKind::Rack {
        return name == "mix"
            || name == "gain_db"
            || device
                .rack
                .as_ref()
                .is_some_and(|r| r.expose.contains_key(name));
    }
    device.kind == model::DeviceKind::Sfz
        && name
            .strip_prefix("cc")
            .and_then(|n| n.parse::<u16>().ok())
            .is_some_and(|cc| cc < 128)
        || device.kind == model::DeviceKind::VoicePatch
        || description::parameter_specs(device.kind)
            .iter()
            .any(|spec| spec.name == name && spec.effect == description::ParameterEffect::Control)
}

/// Range and authored default of an exposed, realtime rack control.
pub fn control_range(device: &model::Device, name: &str) -> Option<(f32, f32, f32)> {
    if device.kind != model::DeviceKind::Rack {
        return None;
    }
    match name {
        "mix" => return Some((0.0, 1.0, 1.0)),
        "gain_db" => return Some((-120.0, 24.0, 0.0)),
        _ => {}
    }
    let path = device.rack.as_ref()?.expose.get(name)?;
    let mut parts = path.splitn(3, '.');
    let branch = parts.next()?.parse::<usize>().ok()?;
    let child = parts.next()?.parse::<usize>().ok()?;
    let parameter = parts.next()?;
    let child = device.rack.as_ref()?.branches.get(branch)?.get(child)?;
    let spec = description::parameter_specs(child.kind)
        .iter()
        .find(|s| s.name == parameter && s.effect == description::ParameterEffect::Control)?;
    Some((
        spec.min,
        spec.max,
        child.params.get(parameter).copied().unwrap_or(spec.default),
    ))
}

struct InlineSource {
    text: String,
}
impl lang::SourceLoader for InlineSource {
    fn resolve(&self, path: &Path) -> Result<PathBuf> {
        ensure!(
            path == Path::new("/muz-state/main.muz"),
            "standalone device source cannot import modules"
        );
        Ok(path.to_owned())
    }
    fn read(&self, path: &Path) -> Result<String> {
        ensure!(
            path == Path::new("/muz-state/main.muz"),
            "standalone device source cannot import modules"
        );
        Ok(self.text.clone())
    }
    fn contrib_root(&self) -> Result<PathBuf> {
        bail!("standalone device source cannot import contrib modules")
    }
}

pub fn reconstruct(source: &str, id: &str) -> Result<model::Device> {
    ensure!(
        source.len() <= MAX_STATE_BYTES / 2,
        "standalone device source too large"
    );
    let (value, _) = lang::load_with_loader(
        Path::new("/muz-state/main.muz"),
        Rc::new(InlineSource {
            text: source.into(),
        }),
    )?;
    crate::compile::lower_standalone_device(&value, id)
}

fn source_for_device(device: &model::Device) -> Result<String> {
    if device.kind == model::DeviceKind::Rack {
        let rack = device
            .rack
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing rack topology"))?;
        let mut source = String::from("let main = {type: \"rack\", branches:[");
        for (branch_index, branch) in rack.branches.iter().enumerate() {
            if branch_index > 0 {
                source.push(',');
            }
            source.push('[');
            for (child_index, child) in branch.iter().enumerate() {
                if child_index > 0 {
                    source.push(',');
                }
                ensure!(
                    !child.kind.is_instrument()
                        && !matches!(
                            child.kind,
                            model::DeviceKind::Rack
                                | model::DeviceKind::Vst3
                                | model::DeviceKind::Clap
                        )
                        && child.sidechain.is_none(),
                    "standalone rack supports only native effects without nested sidechains"
                );
                let prefix = format!("{}.{}.", device.id, branch_index);
                let local_id = child
                    .id
                    .as_str()
                    .strip_prefix(&prefix)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("rack child ID {} is not under {prefix}", child.id)
                    })?;
                let module = source_for_device(child)?;
                let record = module
                    .strip_prefix("let main = {")
                    .and_then(|s| s.strip_suffix("};"))
                    .ok_or_else(|| anyhow::anyhow!("rack child source is not a record"))?;
                source.push_str(&format!(
                    "{{id:{},{} }}",
                    serde_json::to_string(local_id)?,
                    record
                ));
            }
            source.push(']');
        }
        source.push(']');
        if let Some(sidechain) = &device.sidechain {
            source.push_str(&format!(",sidechain:{}", serde_json::to_string(sidechain)?));
        }
        if !rack.expose.is_empty() {
            source.push_str(&format!(
                ",expose:{}",
                json_muz(&serde_json::to_value(&rack.expose)?)?
            ));
        }
        if !rack.modulate.is_empty() {
            source.push_str(&format!(
                ",modulate:{}",
                json_muz(&serde_json::to_value(&rack.modulate)?)?
            ));
        }
        for (name, value) in &device.params {
            ensure!(
                name == "mix" || name == "gain_db" || rack.expose.contains_key(name),
                "unknown rack control {name}"
            );
            source.push_str(&format!(",{name}:{value}"));
        }
        source.push_str("};");
        return Ok(source);
    }
    if device.kind == model::DeviceKind::Sfz {
        let config = device
            .sfz
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing SFZ config"))?;
        return Ok(format!(
            "let main = {{type:\"sfz\",path:{},defines:{},max_voices:{},seed:{},embed_assets:{},program:{},source_overlays:{},sample_budget_frames:{}}};",
            serde_json::to_string(&config.path)?,
            json_muz(&serde_json::to_value(&config.defines)?)?,
            config.max_voices,
            config.seed,
            config.embed_assets,
            json_muz(&serde_json::to_value(&config.program)?)?,
            json_muz(&serde_json::to_value(&config.source_overlays)?)?,
            config.max_sample_frames
        ));
    }
    if device.kind == model::DeviceKind::Sampler {
        let zones = device
            .sample
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing sample zones"))?;
        let mut source = String::from("let main = {type: \"sample\", name: [");
        for (i, zone) in zones.iter().enumerate() {
            if i > 0 {
                source.push(',');
            }
            source.push_str(&format!(
                "{{path:{},root:{},gain_db:{},keys:[{},{}],velocity:[{},{}],offset:{},one_shot:{}",
                serde_json::to_string(&zone.path)?,
                zone.root,
                zone.gain_db,
                zone.keys[0],
                zone.keys[1],
                zone.velocity[0],
                zone.velocity[1],
                zone.offset_seconds,
                zone.one_shot
            ));
            if let Some([start, end]) = zone.loop_seconds {
                source.push_str(&format!(",loop:[{start}s,{end}s]"));
            }
            source.push('}');
        }
        source.push(']');
        append_params(&mut source, device)?;
        source.push_str("};");
        return Ok(source);
    }
    if device.kind == model::DeviceKind::VoicePatch {
        let patch = device
            .patch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing voice patch"))?;
        let mut source = format!("let main = {};", json_muz(patch)?);
        if !source.contains("type:") {
            source = source.replacen("let main = {", "let main = {type:\"voice_patch\",", 1);
        }
        return Ok(source);
    }
    ensure!(
        device.patch.is_none()
            && device.rack.is_none()
            && device.sample.is_none()
            && device.vst3.is_none(),
        "automatic source generation supports simple native devices only"
    );
    let (kind, name) = match device.kind {
        model::DeviceKind::StudioSynth => ("synth", ""),
        model::DeviceKind::Eq => ("fx", "eq"),
        model::DeviceKind::Bitcrusher => ("fx", "bitcrusher"),
        model::DeviceKind::Chorus => ("fx", "chorus"),
        model::DeviceKind::Gate => ("fx", "gate"),
        model::DeviceKind::Reverb => ("fx", "reverb"),
        model::DeviceKind::Stereo => ("fx", "stereo"),
        model::DeviceKind::Lowpass => ("fx", "lowpass"),
        model::DeviceKind::Highpass => ("fx", "highpass"),
        model::DeviceKind::Drive => ("fx", "drive"),
        model::DeviceKind::Gain => ("fx", "gain"),
        model::DeviceKind::Delay => ("fx", "delay"),
        model::DeviceKind::Compressor => ("fx", "compressor"),
        model::DeviceKind::Limiter => ("fx", "limiter"),
        _ => bail!(
            "automatic standalone source unavailable for {:?}",
            device.kind
        ),
    };
    let mut source = format!("let main = {{type: \"{kind}\", name: \"{name}\"");
    if let Some(sidechain) = &device.sidechain {
        ensure!(
            device.kind == model::DeviceKind::Compressor,
            "standalone detector input requires compressor"
        );
        source.push_str(&format!(
            ", sidechain: {}",
            serde_json::to_string(sidechain)?
        ));
    }
    append_params(&mut source, device)?;
    source.push_str("};");
    Ok(source)
}

fn append_params(source: &mut String, device: &model::Device) -> Result<()> {
    for (key, value) in &device.params {
        let spec = description::parameter_specs(device.kind)
            .iter()
            .find(|p| p.name == key)
            .ok_or_else(|| anyhow::anyhow!("unknown device parameter {key}"))?;
        let unit = match spec.unit {
            ParameterUnit::Scalar => "",
            ParameterUnit::Milliseconds => "ms",
            ParameterUnit::Seconds => "s",
            ParameterUnit::Hertz => "hz",
            ParameterUnit::Decibels => "db",
            ParameterUnit::Beats => "b",
        };
        source.push_str(&format!(", {key}: {value}{unit}"));
    }
    Ok(())
}

fn json_muz(value: &serde_json::Value) -> Result<String> {
    Ok(match value {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => serde_json::to_string(s)?,
        serde_json::Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(json_muz)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        serde_json::Value::Object(o) => {
            let mut fields = Vec::new();
            for (key, value) in o {
                ensure!(
                    key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !key.is_empty(),
                    "unsupported source key {key}"
                );
                fields.push(format!("{key}:{}", json_muz(value)?));
            }
            format!("{{{}}}", fields.join(","))
        }
    })
}

/// Stable FNV-1a assignment; collisions are rejected at state construction.
fn stable_id(path: &str) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in path.as_bytes() {
        hash = (hash ^ u32::from(*byte)).wrapping_mul(0x01000193);
    }
    hash & i32::MAX as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    fn gain() -> model::Device {
        serde_json::from_value(
            serde_json::json!({"id":"gain", "kind":"builtin.gain", "params":{"gain_db":-6.0}}),
        )
        .unwrap()
    }
    fn sfz_device(root: &Path, embedded: bool) -> model::Device {
        let path = root.join("program.sfz");
        std::fs::write(
            &path,
            "<control> set_cc1=64\n<region> sample=*silence key=60\n",
        )
        .unwrap();
        let program = crate::sfz::load(&path, &Default::default()).unwrap();
        let mut device = gain();
        device.kind = model::DeviceKind::Sfz;
        device.params.clear();
        device.sfz = Some(model::SfzConfig {
            max_sample_frames: model::DEFAULT_SFZ_SAMPLE_FRAMES,
            source_overlays: Vec::new(),
            path: path.display().to_string(),
            program,
            defines: BTreeMap::new(),
            max_voices: 16,
            seed: 42,
            embed_assets: embedded,
        });
        crate::assets::stamp(&mut device).unwrap();
        device
    }
    #[test]
    fn nested_sfz_in_effect_rack_is_rejected_before_asset_packaging() {
        let dir = tempfile::tempdir().unwrap();
        let sfz = sfz_device(dir.path(), false);
        let mut rack = gain();
        rack.kind = model::DeviceKind::Rack;
        rack.rack = Some(model::Rack {
            branches: vec![vec![sfz]],
            expose: BTreeMap::new(),
            modulate: Vec::new(),
        });
        // Even when missing on disk, the failure must be the topology/policy gate,
        // not an attempted asset read or implicit embedding of licensed mappings.
        std::fs::remove_file(dir.path().join("program.sfz")).unwrap();
        let error = DeviceState::from_device(DeviceRole::Fx, rack.clone()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("SFZ instruments inside effect racks"),
            "{error}"
        );
        let error = DeviceState::new(DeviceRole::Fx, "let main = {};".into(), rack).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("SFZ instruments inside effect racks"),
            "{error}"
        );
    }
    #[test]
    fn sfz_linked_state_checks_dependency_bytes_before_restore() {
        let dir = tempfile::tempdir().unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, sfz_device(dir.path(), false))
            .unwrap();
        assert_eq!(state.linked_assets.len(), 1);
        assert!(state.assets.is_empty());
        assert_eq!(state.parameters.len(), 128);
        assert_eq!(
            state
                .parameters
                .iter()
                .find(|p| p.path == "cc1")
                .unwrap()
                .value,
            64.
        );
        let restored = DeviceState::decode(&state.encode().unwrap()).unwrap();
        restored.host_context().unwrap();
        std::fs::write(
            dir.path().join("program.sfz"),
            "<region> sample=*silence key=61",
        )
        .unwrap();
        assert!(restored.host_context().is_err());
    }
    #[test]
    fn sfz_embedded_state_preserves_normalized_program_without_original_files() {
        let dir = tempfile::tempdir().unwrap();
        let state =
            DeviceState::from_device(DeviceRole::Instrument, sfz_device(dir.path(), true)).unwrap();
        assert!(state.linked_assets.is_empty());
        assert_eq!(state.assets.len(), 1);
        std::fs::remove_file(dir.path().join("program.sfz")).unwrap();
        let restored = DeviceState::decode(&state.encode().unwrap()).unwrap();
        let host = restored.host_context().unwrap();
        host.run(|| crate::assets::validate_versions(&restored.device))
            .unwrap();
    }
    #[test]
    fn standalone_source_and_state_roundtrip() {
        let state = DeviceState::from_device(DeviceRole::Fx, gain()).unwrap();
        assert!(state.source.contains("let main"));
        let restored = DeviceState::decode(&state.encode().unwrap()).unwrap();
        assert_eq!(restored, state);
        assert!(restored.parameters.iter().all(|p| p.id <= i32::MAX as u32));
    }
    #[test]
    fn parallel_rack_state_roundtrip_and_impulse_parity() {
        let authored = r#"let main = {type:"rack",branches:[
            [{type:"fx",name:"gain",id:"trim",gain_db:-6},{type:"fx",name:"gain",id:"trim2",gain_db:-3}],
            [{type:"fx",name:"limiter",id:"limit",lookahead_ms:5ms,ceiling_db:0}]
        ],expose:{trim:"0.0.gain_db"},modulate:[{target:"0.1.gain_db",base:-3,depth:1,rate_hz:1,min:-6,max:0}],trim:-6,mix:0.75,gain_db:-3};"#;
        let device = reconstruct(authored, "rack").unwrap();
        let state = DeviceState::from_device(DeviceRole::Fx, device.clone()).unwrap();
        let restored = DeviceState::decode(&state.encode().unwrap()).unwrap();
        assert_eq!(restored.device.rack, device.rack);
        assert_eq!(restored.device.params, device.params);
        assert_eq!(
            restored
                .parameters
                .iter()
                .map(|p| p.path.as_str())
                .collect::<Vec<_>>(),
            ["gain_db", "mix", "trim"]
        );
        assert_eq!(
            control_range(&restored.device, "trim"),
            Some((-120.0, 24.0, -6.0))
        );
        assert_eq!(
            reconstruct(&restored.source, "rack").unwrap().rack,
            device.rack
        );
        let config = crate::audio::AudioConfig {
            sample_rate: 48000.0,
            max_frames: 512,
            offline: true,
        };
        let mut original = crate::audio::create_processor(&device, config).unwrap();
        let mut portable = restored
            .host_context()
            .unwrap()
            .run(|| crate::audio::create_processor(&restored.device, config).unwrap());
        assert_eq!(original.debug_state().latency_samples, 240);
        assert_eq!(portable.debug_state().latency_samples, 240);
        let ctx = crate::audio::ProcessContext {
            frames: 512,
            block_start_sample: 0,
            transport: crate::audio::TransportSnapshot {
                sample_rate: 48000.0,
                running: true,
                sample_position: 0,
                beat_position: 0.0,
                current_tick: 0.0,
                project_frame: 0.0,
                bpm: 120.0,
                meter: [4, 4],
                loop_ticks: 0,
                ended: false,
            },
        };
        let mut a = vec![0.0f32; 512];
        a[0] = 1.0;
        let mut b = a.clone();
        let mut ar = a.clone();
        let mut br = b.clone();
        original.process(ctx, &[], &mut a, &mut ar).unwrap();
        portable.process(ctx, &[], &mut b, &mut br).unwrap();
        assert_eq!(a, b);
        assert_eq!(ar, br);
        assert!(a[240].abs() > 0.1);
    }
    #[test]
    fn rack_cannot_expose_latency_changing_control() {
        let source = r#"let main = {type:"rack",branches:[[{type:"fx",name:"limiter",id:"lim",lookahead_ms:5ms}]],expose:{lookahead:"0.0.lookahead_ms"}};"#;
        let device = reconstruct(source, "r").unwrap();
        assert!(DeviceState::from_device(DeviceRole::Fx, device).is_err());
    }
    #[test]
    fn rejects_bad_version_and_tampered_source() {
        let state = DeviceState::from_device(DeviceRole::Fx, gain()).unwrap();
        let mut wrong = state.clone();
        wrong.version += 1;
        assert!(DeviceState::decode(&serde_json::to_vec(&wrong).unwrap()).is_err());
        let mut wrong = state.clone();
        wrong.source = "let main = {type: \"fx\", name: \"delay\"};".into();
        assert!(DeviceState::decode(&serde_json::to_vec(&wrong).unwrap()).is_err());
    }
    #[test]
    fn stable_ids_and_host_values_survive_replacement() {
        let state = DeviceState::from_device(DeviceRole::Fx, gain()).unwrap();
        let mut changed = gain();
        changed.params.insert("gain_db".into(), -2.0);
        let next = state
            .replace_device(source_for_device(&changed).unwrap(), changed)
            .unwrap();
        assert_eq!(next.parameters[0].id, state.parameters[0].id);
        assert_eq!(next.parameters[0].value, -6.0);
        assert_eq!(next.device.params["gain_db"], -6.0);
    }
    #[test]
    fn instrument_source_reconstructs() {
        let device = serde_json::from_value(
            serde_json::json!({"id":"synth", "kind":"builtin.studio_synth", "params":{}}),
        )
        .unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        assert_eq!(state.role, DeviceRole::Instrument);
        assert_eq!(
            DeviceState::decode(&state.encode().unwrap()).unwrap(),
            state
        );
    }
    #[test]
    fn structural_parameter_source_tamper_is_rejected() {
        let device = serde_json::from_value(serde_json::json!({"id":"lim", "kind":"builtin.limiter", "params":{"lookahead_ms":5.0,"ceiling_db":-1.0}})).unwrap();
        let mut state = DeviceState::from_device(DeviceRole::Fx, device).unwrap();
        assert_eq!(state.parameters.len(), 2);
        state.source = state.source.replace("5ms", "10ms");
        assert!(DeviceState::decode(&serde_json::to_vec(&state).unwrap()).is_err());
    }
    #[test]
    fn default_gain_control_has_stable_id_and_host_value_precedence() {
        let device: model::Device = serde_json::from_value(
            serde_json::json!({"id":"gain", "kind":"builtin.gain", "params":{}}),
        )
        .unwrap();
        let mut state = DeviceState::from_device(DeviceRole::Fx, device).unwrap();
        assert_eq!(state.parameters.len(), 1);
        assert_eq!(state.parameters[0].path, "gain_db");
        assert_eq!(state.parameters[0].value, 0.0);
        assert_eq!(state.device.params["gain_db"], 0.0);
        let id = state.parameters[0].id;
        state.parameters[0].value = -4.0;
        state.device.params.insert("gain_db".into(), -4.0);
        let restored = DeviceState::decode(&serde_json::to_vec(&state).unwrap()).unwrap();
        let mut new_device = gain();
        new_device.params.insert("gain_db".into(), -2.0);
        let replaced = restored
            .replace_device(source_for_device(&new_device).unwrap(), new_device)
            .unwrap();
        assert_eq!(replaced.parameters[0].id, id);
        assert_eq!(replaced.parameters[0].value, -4.0);
        assert_eq!(replaced.device.params["gain_db"], -4.0);
    }
    #[test]
    fn sampler_restores_after_original_asset_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("tone.wav");
        let mut writer = hound::WavWriter::create(
            &wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..512 {
            writer.write_sample(0.25f32).unwrap();
        }
        writer.finalize().unwrap();
        let device: model::Device = serde_json::from_value(serde_json::json!({
            "id":"sample", "kind":"builtin.sampler", "params":{}, "sample":[{
                "path": wav.display().to_string(), "gain_db":0.0, "root":60.0,
                "keys":[0,127], "velocity":[0.0,1.0], "offset_seconds":0.0,
                "loop_seconds":null, "one_shot":false
            }]
        }))
        .unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        assert_eq!(state.assets.len(), 1);
        assert!(!state.source.contains(&wav.display().to_string()));
        let mut corrupt = state.clone();
        corrupt.assets[0].data_base64.replace_range(0..1, "A");
        assert!(DeviceState::decode(&serde_json::to_vec(&corrupt).unwrap()).is_err());
        let bytes = state.encode().unwrap();
        std::fs::remove_file(&wav).unwrap();
        let restored = DeviceState::decode(&bytes).unwrap();
        let ctx = restored.host_context().unwrap();
        ctx.run(|| {
            let config = crate::audio::AudioConfig {
                sample_rate: 48000.0,
                max_frames: 128,
                offline: true,
            };
            let mut processor = crate::audio::create_processor(&restored.device, config).unwrap();
            let transport = crate::audio::TransportSnapshot {
                sample_rate: 48000.0,
                running: true,
                sample_position: 0,
                beat_position: 0.0,
                current_tick: 0.0,
                project_frame: 0.0,
                bpm: 120.0,
                meter: [4, 4],
                loop_ticks: 0,
                ended: false,
            };
            let event = crate::audio::DeviceEvent {
                offset: 0,
                kind: crate::audio::DeviceEventKind::NoteOn {
                    sample_zone: None,
                    pitch: 60.0,
                    elapsed_frames: 0,
                    note_id: 1,
                    channel: 0,
                    key: 60,
                    velocity: 1.0,
                },
            };
            let mut left = [0.0; 128];
            let mut right = [0.0; 128];
            processor
                .process(
                    crate::audio::ProcessContext {
                        frames: 128,
                        block_start_sample: 0,
                        transport,
                    },
                    &[event],
                    &mut left,
                    &mut right,
                )
                .unwrap();
            assert!(left.iter().any(|v| v.abs() > 0.01));
        });
    }
    #[test]
    fn sampled_voice_patch_restores_after_original_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("tone.wav");
        let mut writer = hound::WavWriter::create(
            &wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..512 {
            writer.write_sample(0.25f32).unwrap();
        }
        writer.finalize().unwrap();
        let device: model::Device = serde_json::from_value(serde_json::json!({
            "id":"patch", "kind":"builtin.voice_patch", "params":{}, "patch":{
                "type":"voice_patch", "name":"patch", "_module_dir":dir.path().display().to_string(),
                "nodes":[{"id":"x","op":"sample","path":"tone.wav","root":60}], "output":"x"
            }
        })).unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        assert_eq!(state.assets.len(), 1);
        std::fs::remove_file(&wav).unwrap();
        let restored = DeviceState::decode(&state.encode().unwrap()).unwrap();
        restored.host_context().unwrap().run(|| {
            crate::audio::create_processor(
                &restored.device,
                crate::audio::AudioConfig {
                    sample_rate: 48000.0,
                    max_frames: 128,
                    offline: true,
                },
            )
            .unwrap();
        });
    }
    #[test]
    fn reader_patch_restores_without_original_file() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("reader.wav");
        let mut writer = hound::WavWriter::create(
            &wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..512 {
            writer.write_sample(0.25f32).unwrap();
        }
        writer.finalize().unwrap();
        let device: model::Device = serde_json::from_value(serde_json::json!({
            "id":"reader", "kind":"builtin.voice_patch", "params":{}, "patch":{
                "type":"voice_patch", "name":"reader", "nodes":[{"id":"r","op":"reader","channel":"mono","zones":[{
                    "path":wav.display().to_string(),"root":60.0,"gain_db":0.0,"keys":[0,127],"velocity":[0.0,1.0],"offset_seconds":0.0,"loop_seconds":null,"one_shot":false
                }]}], "output":"r"
            }
        })).unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        std::fs::remove_file(&wav).unwrap();
        let restored = DeviceState::decode(&state.encode().unwrap()).unwrap();
        restored.host_context().unwrap().run(|| {
            crate::audio::create_processor(
                &restored.device,
                crate::audio::AudioConfig {
                    sample_rate: 48000.0,
                    max_frames: 128,
                    offline: true,
                },
            )
            .unwrap();
        });
    }
}
