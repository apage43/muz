//! File metadata only, to reload changed audio/preset files. No content hashes or replay contract.
use crate::model::Device;
use anyhow::Result;
use std::path::PathBuf;
use std::{
    io::{Read, Seek},
    sync::Arc,
};
pub trait AssetReader: Read + Seek + Send {}
impl<T: Read + Seek + Send> AssetReader for T {}
/// A resolved identity is stable within the host; versions must change whenever
/// bytes change. Reads are independently seekable, even when bytes are shared.
pub trait AssetResolver: Send + Sync {
    fn resolve(&self, path: &std::path::Path) -> Result<PathBuf>;
    fn version(&self, path: &std::path::Path) -> Result<(u64, u128)>;
    fn open(&self, path: &std::path::Path) -> Result<Box<dyn AssetReader>>;
}
pub struct FileAssets;
impl AssetResolver for FileAssets {
    fn resolve(&self, path: &std::path::Path) -> Result<PathBuf> {
        Ok(path.canonicalize()?)
    }
    fn version(&self, path: &std::path::Path) -> Result<(u64, u128)> {
        let m = std::fs::metadata(path)?;
        Ok((
            m.len(),
            m.modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        ))
    }
    fn open(&self, path: &std::path::Path) -> Result<Box<dyn AssetReader>> {
        Ok(Box::new(std::fs::File::open(path)?))
    }
}
#[derive(Default)]
pub struct MemoryAssets {
    entries: std::collections::BTreeMap<PathBuf, (Arc<[u8]>, u128)>,
}
impl MemoryAssets {
    pub fn insert(&mut self, path: PathBuf, bytes: Arc<[u8]>, version: u128) {
        self.entries.insert(path, (bytes, version));
    }
    fn entry(&self, path: &std::path::Path) -> Result<&(Arc<[u8]>, u128)> {
        self.entries
            .get(path)
            .ok_or_else(|| anyhow::anyhow!("missing memory asset {}", path.display()))
    }
}
impl AssetResolver for MemoryAssets {
    fn resolve(&self, path: &std::path::Path) -> Result<PathBuf> {
        self.entry(path)?;
        Ok(path.to_owned())
    }
    fn version(&self, path: &std::path::Path) -> Result<(u64, u128)> {
        let (data, v) = self.entry(path)?;
        Ok((data.len() as u64, *v))
    }
    fn open(&self, path: &std::path::Path) -> Result<Box<dyn AssetReader>> {
        Ok(Box::new(std::io::Cursor::new(self.entry(path)?.0.clone())))
    }
}
pub fn resolver() -> Arc<dyn AssetResolver> {
    crate::host::current()
        .map(|c| c.assets)
        .unwrap_or_else(|| Arc::new(FileAssets))
}
pub fn resolve(path: &std::path::Path) -> Result<PathBuf> {
    resolver().resolve(path)
}
pub fn read_bounded(path: &std::path::Path, max: usize) -> Result<Vec<u8>> {
    let r = resolver();
    anyhow::ensure!(
        r.version(path)?.0 <= max as u64,
        "asset byte limit exceeded"
    );
    let mut data = Vec::new();
    r.open(path)?.take(max as u64 + 1).read_to_end(&mut data)?;
    anyhow::ensure!(data.len() <= max, "asset byte limit exceeded");
    Ok(data)
}
pub fn validate_versions(device: &Device) -> Result<()> {
    if device.asset_versions.is_empty() {
        return Ok(());
    }
    let paths = paths(device);
    anyhow::ensure!(
        paths.len() == device.asset_versions.len(),
        "asset version count mismatch"
    );
    for (path, expected) in paths.iter().zip(&device.asset_versions) {
        anyhow::ensure!(
            resolver().version(path)? == *expected,
            "asset revision changed: {}",
            path.display()
        );
    }
    Ok(())
}
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
                if n["op"] == "reader" {
                    if let Some(zones) = n["zones"].as_array() {
                        for z in zones {
                            if let Some(path) = z["path"].as_str() {
                                paths.push(PathBuf::from(path));
                            }
                        }
                    }
                }
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
            resolver().version(p).map_err(|e| {
                crate::diagnostic::Diagnostic::new(format!("missing asset {}: {e}", p.display()))
                    .err()
            })
        })
        .collect::<Result<_>>()?;
    Ok(())
}
