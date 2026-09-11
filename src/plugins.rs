use crate::audio::vst3::{PreparedVst3, Vst3ClassId};
use anyhow::Result;
use std::path::{Path, PathBuf};
pub fn is_clap(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("clap"))
}
pub fn open_clap(
    path: &Path,
    class: Option<&str>,
    rate: u32,
    block: usize,
) -> Result<crate::audio::clap::PreparedClap> {
    crate::audio::clap::PreparedClap::open(
        path,
        class,
        crate::audio::AudioConfig {
            sample_rate: rate as f32,
            max_frames: block,
            offline: false,
        },
        0,
    )
}
pub fn open(path: &Path, class: Option<&str>, rate: u32, block: usize) -> Result<PreparedVst3> {
    Ok(PreparedVst3::prepare_config(
        path,
        class
            .map(str::parse)
            .transpose()?
            .unwrap_or(Vst3ClassId([0; 16])),
        None,
        rate as f64,
        block,
        false,
    )?)
}
pub fn installed() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut paths = Vec::new();
    for dir in [
        PathBuf::from("/usr/lib/vst3"),
        PathBuf::from("/usr/local/lib/vst3"),
        PathBuf::from("/usr/lib/clap"),
        PathBuf::from(format!("{home}/.vst3")),
        PathBuf::from(format!("{home}/.clap")),
    ] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("vst3" | "clap")
                ) {
                    paths.push(p);
                }
            }
        }
    }
    if let Ok(aliases) = configured_aliases() {
        paths.extend(
            aliases
                .values()
                .filter_map(|v| v.get("path").and_then(serde_json::Value::as_str))
                .map(PathBuf::from)
                .filter(|p| p.exists()),
        );
    }
    paths.sort();
    paths.dedup();
    paths
}
pub fn native_names() -> &'static [&'static str] {
    &[
        "studio_synth",
        "poly_synth",
        "sampler",
        "voice_patch",
        "eq",
        "highpass",
        "lowpass",
        "compressor",
        "limiter",
        "bitcrusher",
        "chorus",
        "gate",
        "drive",
        "gain",
        "stereo",
        "delay",
        "reverb",
        "rack",
    ]
}
pub fn native(name: &str) -> Option<serde_json::Value> {
    let name = name.strip_prefix("builtin.").unwrap_or(name);
    if !native_names().contains(&name) {
        return None;
    }
    let kind: crate::model::DeviceKind =
        serde_json::from_value(serde_json::json!(format!("builtin.{name}"))).ok()?;
    Some(
        serde_json::json!({"kind":kind,"parameters":crate::source::parameter_specs(kind).iter().map(|p|serde_json::json!({"name":p.name,"min":p.min,"max":p.max,"default":p.default})).collect::<Vec<_>>(),"dynamic_controls":matches!(kind,crate::model::DeviceKind::VoicePatch|crate::model::DeviceKind::Rack)}),
    )
}

/// Location of the user plugin alias file: `MUZ_PLUGIN_CONFIG`, else
/// `$XDG_CONFIG_HOME/muz/plugins.json` (normally `~/.config/muz/plugins.json`).
pub fn config_path() -> PathBuf {
    std::env::var_os("MUZ_PLUGIN_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
                });
            base.join("muz/plugins.json")
        })
}
/// User-owned aliases keep machine locations and plugin class identifiers out of recipes.
fn configured_aliases() -> Result<serde_json::Map<String, serde_json::Value>> {
    let config = config_path();
    if !config.exists() {
        return Ok(serde_json::Map::new());
    }
    let aliases: serde_json::Value = serde_json::from_slice(&std::fs::read(&config)?)?;
    let aliases = aliases.as_object().ok_or_else(|| {
        anyhow::anyhow!("plugin aliases must be a JSON object: {}", config.display())
    })?;
    let mut resolved = serde_json::Map::new();
    for (name, alias) in aliases {
        let path = alias
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("plugin alias '{name}' needs a path"))?;
        let mut alias = alias.clone();
        if !Path::new(path).is_absolute() {
            alias["path"] = serde_json::Value::String(
                config
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(path)
                    .display()
                    .to_string(),
            );
        }
        resolved.insert(name.clone(), alias);
    }
    Ok(resolved)
}
pub fn configured_alias(name: &str) -> Result<Option<serde_json::Value>> {
    Ok(configured_aliases()?.remove(name))
}
/// Path behind a user alias, so command-line device arguments can name the same
/// alias a source file uses instead of repeating a machine path.
pub fn alias_path(name: &str) -> Result<Option<PathBuf>> {
    Ok(configured_alias(name)?.and_then(|alias| {
        alias
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
    }))
}
