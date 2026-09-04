use crate::audio::vst3::{PreparedVst3, Vst3ClassId};
use anyhow::Result;
use std::path::{Path, PathBuf};
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
