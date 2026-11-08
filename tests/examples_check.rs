//! Examples and the project template are evaluated, not just format-stable.
#![cfg(feature = "desktop")]

use std::path::Path;
use std::process::Command;

/// Pattern roots (no `song({...})`) are evaluated through MIDI export instead.
const PATTERN_ROOTS: &[&str] = &["midi-messages.muz"];

fn check(path: &Path) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_muz"));
    if path
        .file_name()
        .is_some_and(|n| PATTERN_ROOTS.iter().any(|p| n == *p))
    {
        let out =
            std::env::temp_dir().join(format!("muz-examples-check-{}.mid", std::process::id()));
        let result = command
            .args(["midi", "export"])
            .arg(path)
            .arg("-o")
            .arg(&out)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("run midi export");
        let _ = std::fs::remove_file(out);
        return result;
    }
    command
        .arg("check")
        .arg(path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run check")
}

#[test]
fn examples_and_template_evaluate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("templates/song.muz")];
    for entry in std::fs::read_dir(root.join("examples")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "muz") {
            files.push(path);
        }
    }
    assert!(files.len() > 1);
    let mut failures = Vec::new();
    for file in &files {
        let out = check(file);
        if !out.status.success() {
            failures.push(format!(
                "{}: {}",
                file.display(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
