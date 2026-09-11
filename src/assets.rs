//! File metadata only, to reload changed audio/preset files. No content hashes or replay contract.
use crate::model::Device;
use anyhow::Result;
use std::path::PathBuf;
pub fn paths(d: &Device) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(zones) = &d.sample {
        paths.extend(zones.iter().map(|z| PathBuf::from(&z.path)));
    }
    if let Some(p) = &d.vst3 {
        if let Some(state) = &p.state {
            paths.push(PathBuf::from(state));
        }
    }
    if let Some(p) = &d.patch {
        let root = PathBuf::from(p["_module_dir"].as_str().unwrap_or("."));
        if let Some(nodes) = p["nodes"].as_array() {
            for n in nodes {
                if n["op"].as_str() == Some("sample") {
                    if let Some(path) = n["path"].as_str() {
                        paths.push(root.join(path));
                    }
                }
            }
        }
    }
    if let Some(r) = &d.rack {
        for d in r.branches.iter().flatten() {
            paths.extend(self::paths(d));
        }
    }
    paths.sort();
    paths.dedup();
    paths
}
pub fn stamp(d: &mut Device) -> Result<()> {
    d.asset_versions = paths(d)
        .iter()
        .map(|p| {
            let m = std::fs::metadata(p).map_err(|error| {
                crate::lang::Diagnostic::new(format!(
                    "missing asset {}: {error}",
                    p.display()
                ))
                .help("asset paths are relative to the module that declares them; check the spelling and location of the file")
                .err()
            })?;
            let modified = m
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            Ok((m.len(), modified))
        })
        .collect::<Result<_>>()?;
    Ok(())
}
