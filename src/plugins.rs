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
        PathBuf::from(format!("{home}/Documents/Pianoteq 9/x86-64bit")),
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
