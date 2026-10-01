//! Versioned, bounded immutable asset reads for off-thread preparation.
use crate::model::Device;
use anyhow::{Context, Result};
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
    /// SFZ compatibility lookup; custom hosts may supply an unambiguous case index.
    fn resolve_case_insensitive(&self, path: &std::path::Path) -> Result<PathBuf> {
        self.resolve(path)
    }
    fn version(&self, path: &std::path::Path) -> Result<(u64, u128)>;
    fn open(&self, path: &std::path::Path) -> Result<Box<dyn AssetReader>>;
    fn snapshot(&self, path: &std::path::Path, max: usize) -> Result<AssetSnapshot> {
        crate::host::check_cancelled()?;
        let before = self.version(path)?;
        anyhow::ensure!(
            before.0 <= max as u64,
            "asset byte limit exceeded: {}",
            path.display()
        );
        let mut reader = self.open(path)?;
        let mut bytes = Vec::new();
        let mut chunk = [0; 65536];
        loop {
            crate::host::check_cancelled()?;
            let count = reader.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            anyhow::ensure!(
                count <= max.saturating_sub(bytes.len()),
                "asset byte limit exceeded: {}",
                path.display()
            );
            bytes.extend_from_slice(&chunk[..count]);
        }
        anyhow::ensure!(
            before == self.version(path)? && before.0 == bytes.len() as u64,
            "stale asset while reading: {}",
            path.display()
        );
        Ok(AssetSnapshot {
            version: before,
            bytes: bytes.into(),
        })
    }
}
pub struct AssetSnapshot {
    pub version: (u64, u128),
    pub bytes: Arc<[u8]>,
}
thread_local! { static EXPECTED: std::cell::RefCell<std::collections::BTreeMap<PathBuf, (u64,u128)>> = const { std::cell::RefCell::new(std::collections::BTreeMap::new()) }; }
pub(crate) fn with_versions<T>(device: &Device, f: impl FnOnce() -> T) -> T {
    struct Restore(std::collections::BTreeMap<PathBuf, (u64, u128)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            EXPECTED.with(|e| *e.borrow_mut() = std::mem::take(&mut self.0));
        }
    }
    let old = EXPECTED.with(|e| {
        let old = e.borrow().clone();
        e.borrow_mut().extend(
            paths(device)
                .into_iter()
                .map(|path| resolver().resolve(&path).unwrap_or(path))
                .zip(device.asset_versions.iter().copied()),
        );
        old
    });
    let _restore = Restore(old);
    f()
}
pub fn snapshot(path: &std::path::Path, max: usize) -> Result<AssetSnapshot> {
    crate::host::check_cancelled()?;
    let resolver = resolver();
    let identity = resolver
        .resolve(path)
        .with_context(|| format!("missing asset {}", path.display()))?;
    let snapshot = resolver
        .snapshot(&identity, max)
        .with_context(|| format!("reading asset {}", path.display()))?;
    crate::host::check_cancelled()?;
    anyhow::ensure!(
        snapshot.bytes.len() <= max && snapshot.version.0 == snapshot.bytes.len() as u64,
        "invalid/oversized asset snapshot: {}",
        path.display()
    );
    let expected = EXPECTED.with(|e| e.borrow().get(&identity).copied());
    anyhow::ensure!(
        expected.is_none_or(|v| v == snapshot.version),
        "stale asset revision: {}",
        path.display()
    );
    Ok(snapshot)
}
pub struct FileAssets;
impl AssetResolver for FileAssets {
    fn resolve(&self, path: &std::path::Path) -> Result<PathBuf> {
        Ok(path.canonicalize()?)
    }
    fn resolve_case_insensitive(&self, path: &std::path::Path) -> Result<PathBuf> {
        if let Ok(exact) = self.resolve(path) {
            return Ok(exact);
        }
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut resolved = PathBuf::new();
        for component in absolute.components() {
            match component {
                std::path::Component::Normal(name) => {
                    let exact = resolved.join(name);
                    if exact.exists() {
                        resolved = exact;
                        continue;
                    }
                    let wanted = name.to_string_lossy();
                    let mut matches =
                        std::fs::read_dir(&resolved)?
                            .filter_map(|e| e.ok())
                            .filter(|e| {
                                e.file_name()
                                    .to_string_lossy()
                                    .eq_ignore_ascii_case(&wanted)
                            });
                    let found = matches
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("missing SFZ asset {}", path.display()))?;
                    anyhow::ensure!(
                        matches.next().is_none(),
                        "ambiguous SFZ path case: {}",
                        path.display()
                    );
                    resolved = found.path();
                }
                std::path::Component::ParentDir => {
                    resolved.pop();
                }
                std::path::Component::CurDir => {}
                other => resolved.push(other.as_os_str()),
            }
        }
        Ok(resolved.canonicalize()?)
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
/// Native saved-state access granted by the host, independently of serialized paths.
/// Canonical identities must remain inside one explicitly authorized directory.
pub struct ScopedFileAssets {
    roots: Vec<PathBuf>,
}
impl ScopedFileAssets {
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Result<Self> {
        let roots = roots
            .into_iter()
            .map(|p| std::fs::canonicalize(p))
            .collect::<std::io::Result<Vec<_>>>()?;
        anyhow::ensure!(
            roots.iter().all(|p| p.is_dir()),
            "asset grants must name directories"
        );
        Ok(Self { roots })
    }
    pub fn configured_sfz() -> Result<Self> {
        let mut roots = std::env::var_os("MUZ_SFZ_ASSET_ROOTS")
            .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
            .unwrap_or_default();
        if let Ok(contrib) = crate::lang::contrib_root() {
            roots.extend(Self::installed_sfz_roots(&contrib));
        }
        Self::new(roots)
    }
    pub(crate) fn installed_sfz_roots(contrib: &std::path::Path) -> Vec<PathBuf> {
        [
            "sonatina/assets",
            "virtuosity-drums/assets",
            "karoryfer/assets",
            "unreal/standard-guitar/assets",
            "unreal/metal-gtx/assets",
        ]
        .into_iter()
        .map(|relative| contrib.join(relative))
        .filter(|path| path.is_dir())
        .collect()
    }
    fn checked(&self, path: &std::path::Path) -> Result<PathBuf> {
        let canonical = std::fs::canonicalize(path)?;
        anyhow::ensure!(
            self.roots.iter().any(|root| canonical.starts_with(root)),
            "linked SFZ asset outside authorized roots: {} (configure MUZ_SFZ_ASSET_ROOTS or supply a scoped resolver)",
            path.display()
        );
        Ok(canonical)
    }
}
impl AssetResolver for ScopedFileAssets {
    fn resolve(&self, path: &std::path::Path) -> Result<PathBuf> {
        self.checked(path)
    }
    fn resolve_case_insensitive(&self, path: &std::path::Path) -> Result<PathBuf> {
        let mut lexical = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    lexical.pop();
                }
                std::path::Component::CurDir => {}
                part => lexical.push(part.as_os_str()),
            }
        }
        let root = self
            .roots
            .iter()
            .find(|root| {
                let mut components = lexical.components();
                root.components().all(|part| {
                    components.next().is_some_and(|candidate| {
                        candidate
                            .as_os_str()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&part.as_os_str().to_string_lossy())
                    })
                })
            })
            .ok_or_else(|| anyhow::anyhow!("SFZ case lookup outside authorized roots"))?;
        let remaining = lexical.components().skip(root.components().count());
        let mut resolved = root.clone();
        for component in remaining {
            let name = component.as_os_str();
            let exact = resolved.join(name);
            let candidate = if exact.exists() {
                exact
            } else {
                let mut matches = std::fs::read_dir(&resolved)?
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&name.to_string_lossy())
                    });
                let found = matches
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing SFZ asset {}", path.display()))?;
                anyhow::ensure!(matches.next().is_none(), "ambiguous SFZ path case");
                found.path()
            };
            resolved = self.checked(&candidate)?;
        }
        Ok(resolved)
    }

    fn version(&self, path: &std::path::Path) -> Result<(u64, u128)> {
        FileAssets.version(&self.checked(path)?)
    }
    fn open(&self, path: &std::path::Path) -> Result<Box<dyn AssetReader>> {
        FileAssets.open(&self.checked(path)?)
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
    fn resolve_case_insensitive(&self, path: &std::path::Path) -> Result<PathBuf> {
        if let Ok(exact) = self.resolve(path) {
            return Ok(exact);
        }
        let wanted = path.to_string_lossy();
        let mut matches = self
            .entries
            .keys()
            .filter(|p| p.to_string_lossy().eq_ignore_ascii_case(&wanted));
        let found = matches
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing SFZ memory asset {}", path.display()))?;
        anyhow::ensure!(
            matches.next().is_none(),
            "ambiguous SFZ memory path case: {}",
            path.display()
        );
        Ok(found.clone())
    }
    fn version(&self, path: &std::path::Path) -> Result<(u64, u128)> {
        let (data, v) = self.entry(path)?;
        Ok((data.len() as u64, *v))
    }
    fn open(&self, path: &std::path::Path) -> Result<Box<dyn AssetReader>> {
        Ok(Box::new(std::io::Cursor::new(self.entry(path)?.0.clone())))
    }
    fn snapshot(&self, path: &std::path::Path, max: usize) -> Result<AssetSnapshot> {
        crate::host::check_cancelled()?;
        let (bytes, version) = self.entry(path)?;
        anyhow::ensure!(
            bytes.len() <= max,
            "asset byte limit exceeded: {}",
            path.display()
        );
        Ok(AssetSnapshot {
            bytes: bytes.clone(),
            version: (bytes.len() as u64, *version),
        })
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
pub fn resolve_sfz(path: &std::path::Path) -> Result<PathBuf> {
    resolver().resolve_case_insensitive(path)
}
pub fn read_bounded(path: &std::path::Path, max: usize) -> Result<Vec<u8>> {
    Ok(snapshot(path, max)?.bytes.to_vec())
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
    if let Some(config) = &d.sfz {
        paths.extend(config.program.dependencies.iter().cloned());
        paths.extend(
            config
                .program
                .regions
                .iter()
                .filter_map(|r| r.sample.clone()),
        );
    }
    if let Some(zones) = &d.sample {
        paths.extend(zones.iter().map(|z| PathBuf::from(&z.path)));
    }
    if let Some(p) = &d.vst3
        && let Some(state) = &p.state
    {
        paths.push(PathBuf::from(state));
    }
    if let Some(p) = &d.patch {
        let root = PathBuf::from(p["_module_dir"].as_str().unwrap_or("."));
        if let Some(nodes) = p["nodes"].as_array() {
            for n in nodes {
                if n["op"] == "reader"
                    && let Some(zones) = n["zones"].as_array()
                {
                    for z in zones {
                        if let Some(path) = z["path"].as_str() {
                            paths.push(PathBuf::from(path));
                        }
                    }
                }
                if n["op"].as_str() == Some("sample")
                    && let Some(path) = n["path"].as_str()
                {
                    paths.push(root.join(path));
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opened_revision_must_match_prepared_device_even_after_metadata_check() {
        struct Replaced;
        impl AssetResolver for Replaced {
            fn resolve(&self, p: &std::path::Path) -> Result<PathBuf> {
                Ok(p.into())
            }
            fn version(&self, _: &std::path::Path) -> Result<(u64, u128)> {
                Ok((4, 1))
            }
            fn open(&self, _: &std::path::Path) -> Result<Box<dyn AssetReader>> {
                unreachable!()
            }
            fn snapshot(&self, _: &std::path::Path, _: usize) -> Result<AssetSnapshot> {
                Ok(AssetSnapshot {
                    version: (4, 2),
                    bytes: Arc::from([0; 4]),
                })
            }
        }
        let device = Device {
            sfz: None,
            id: crate::model::Id::new("p"),
            kind: crate::model::DeviceKind::VoicePatch,
            patch: Some(
                serde_json::json!({"nodes":[{"id":"a","op":"sample","path":"/a.wav"}],"output":"a"}),
            ),
            params: Default::default(),
            rack: None,
            sample: None,
            vst3: None,
            generation: 0,
            sidechain: None,
            asset_versions: vec![(4, 1)],
        };
        let context = crate::host::HostContext {
            assets: Arc::new(Replaced),
            ..Default::default()
        };
        context.run(|| {
            validate_versions(&device).unwrap();
            with_versions(&device, || {
                let result = snapshot(std::path::Path::new("/a.wav"), 4);
                assert!(result.is_err_and(|e| e.to_string().contains("stale asset revision")));
            });
        });
    }
    #[test]
    fn installed_sfz_scope_excludes_unrelated_files_and_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let contrib = dir.path().join("xdg/muz/contrib");
        let installed = contrib.join("sonatina/assets");
        std::fs::create_dir_all(&installed).unwrap();
        let mapping = installed.join("program.sfz");
        std::fs::write(&mapping, "<region> sample=*silence").unwrap();
        let private = dir.path().join("private");
        std::fs::write(&private, "outside").unwrap();
        let scoped =
            ScopedFileAssets::new(ScopedFileAssets::installed_sfz_roots(&contrib)).unwrap();
        assert!(scoped.open(&mapping).is_ok());
        assert!(scoped.open(&private).is_err());
        assert!(scoped.resolve_case_insensitive(&private).is_err());
        #[cfg(unix)]
        {
            let escape = installed.join("escape.sfz");
            std::os::unix::fs::symlink(&private, &escape).unwrap();
            assert!(scoped.open(&escape).is_err());
            assert!(scoped.resolve_case_insensitive(&escape).is_err());
        }
        let extra = ScopedFileAssets::new([dir.path().to_owned()]).unwrap();
        assert!(extra.open(&private).is_ok());
    }
}
