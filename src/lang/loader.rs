//! Host-owned module access. Editors can compile unsaved documents without disk I/O.
use anyhow::Result;
use std::path::{Path, PathBuf};

pub trait SourceLoader {
    /// Resolve an identity before cache and cycle checks. Reject paths outside the host's mount.
    fn resolve(&self, path: &Path) -> Result<PathBuf>;
    fn read(&self, path: &Path) -> Result<String>;
    fn contrib_root(&self) -> Result<PathBuf>;
    fn contrib_modules(&self) -> Vec<String> {
        Vec::new()
    }
}

pub struct FileSourceLoader;
impl SourceLoader for FileSourceLoader {
    fn resolve(&self, path: &Path) -> Result<PathBuf> {
        Ok(path.canonicalize()?)
    }
    fn read(&self, path: &Path) -> Result<String> {
        Ok(std::fs::read_to_string(path)?)
    }
    fn contrib_root(&self) -> Result<PathBuf> {
        super::eval::contrib_root()
    }
    fn contrib_modules(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(root) = self.contrib_root()
            && let Ok(packs) = std::fs::read_dir(root)
        {
            for pack in packs.flatten() {
                if let Ok(entries) = std::fs::read_dir(pack.path()) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().is_some_and(|e| e == "muz") {
                            names.push(format!(
                                "contrib/{}/{}",
                                pack.file_name().to_string_lossy(),
                                path.file_stem().unwrap().to_string_lossy()
                            ));
                        }
                    }
                }
            }
        }
        names.sort();
        names
    }
}
