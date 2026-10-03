//! A real, isolated CLAP oscillator exercising the host restart ABI in native tests.
use std::{path::PathBuf, process::Command};

pub(crate) struct RestartPlugin {
    pub(crate) directory: tempfile::TempDir,
    library: libloading::Library,
    path: PathBuf,
}
impl RestartPlugin {
    pub(crate) fn build() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("restart.clap");
        let dependencies = std::env::current_exe().unwrap().parent().unwrap().to_owned();
        let clap = std::fs::read_dir(&dependencies).unwrap().map(|entry| entry.unwrap().path())
            .find(|path| path.file_name().unwrap().to_string_lossy().starts_with("libclap_sys-")
                && path.extension().is_some_and(|extension| extension == "rlib"))
            .expect("Cargo test build supplies clap-sys ABI dependency");
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/restart_plugin.rs");
        let result = Command::new("rustc").args(["--edition=2024", "--crate-type=cdylib", "-C", "opt-level=1"])
            .arg(&source).arg("--extern").arg(format!("clap_sys={}", clap.display()))
            .arg("-L").arg(format!("dependency={}", dependencies.display()))
            .arg("-o").arg(&path).output().unwrap();
        assert!(result.status.success(), "CLAP restart fixture compile: {}", String::from_utf8_lossy(&result.stderr));
        let library = unsafe { libloading::Library::new(&path) }.unwrap();
        Self { directory, library, path }
    }
    pub(crate) fn source(&self) -> String {
        let path = serde_json::to_string(&self.path.to_string_lossy()).unwrap();
        format!("song({{tempo:120,tracks:[track(\"plugin\",note(60,16b),plugin(\"test\",{{path:{path},class:\"muz.test.restart\"}})),track(\"retained\",note(64,16b),synth(\"pad\"))],tail:0}})")
    }
    pub(crate) fn session(&self) -> crate::Session {
        let source = self.directory.path().join("restart.muz");
        std::fs::write(&source, self.source()).unwrap();
        crate::compile::compile(&source).unwrap().session
    }
    pub(crate) fn arm(&self) {
        let arm = unsafe { self.library.get::<unsafe extern "C" fn()>(b"arm_restart\0") }.unwrap();
        unsafe { arm(); }
    }
}
