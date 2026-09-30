use std::path::Path;
use std::process::Command;

fn eval(source: &Path, contrib: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_muz"))
        .env("MUZ_CONTRIB_DIR", contrib)
        .arg("eval")
        .arg(source)
        .output()
        .unwrap()
}

#[test]
fn contrib_modules_resolve_from_the_configured_library() {
    let dir = tempfile::tempdir().unwrap();
    let contrib = dir.path().join("contrib");
    std::fs::create_dir_all(contrib.join("test-pack")).unwrap();
    std::fs::write(
        contrib.join("test-pack/voice.muz"),
        "let unity = 1;\nfn level(factor = unity) = 0.42 * factor;",
    )
    .unwrap();
    std::fs::write(
        contrib.join("test-pack/use.muz"),
        "use \"contrib/test-pack/voice\" as v;\nlet level = v.level(2);",
    )
    .unwrap();
    let source = dir.path().join("song.muz");
    std::fs::write(
        &source,
        "use \"contrib/test-pack/use\" as u;\nsong({tracks: [track(\"lead\", note(60), synth(\"init\"))], level: u.level})",
    )
    .unwrap();
    let output = eval(&source, &contrib);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = String::from_utf8_lossy(&output.stdout);
    assert!(json.contains("0.84"), "{json}");
}

#[test]
fn contrib_imports_reject_missing_modules_and_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let contrib = dir.path().join("contrib");
    std::fs::create_dir_all(contrib.join("test-pack")).unwrap();
    std::fs::write(contrib.join("test-pack/voice.muz"), "let unity = 1;").unwrap();
    let source = dir.path().join("song.muz");
    for (import, expected) in [
        ("contrib/test-pack/absent", "unknown contrib module"),
        ("contrib/../escape", "must name a module inside"),
        ("contrib//absolute", "must name a module inside"),
    ] {
        std::fs::write(&source, format!("use \"{import}\" as m;\nm")).unwrap();
        let output = eval(&source, &contrib);
        assert!(!output.status.success(), "{import} should not resolve");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{import}: {stderr}");
        if expected == "unknown contrib module" {
            // The hint lists the modules the library actually ships.
            assert!(
                stderr.contains("contrib/test-pack/voice"),
                "{import}: {stderr}"
            );
        }
    }
}

#[test]
fn contrib_library_root_is_reported_when_unusable() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("song.muz");
    std::fs::write(&source, "use \"contrib/pack/module\" as m;\nm").unwrap();
    let output = eval(&source, &dir.path().join("not-a-directory"));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("MUZ_CONTRIB_DIR"), "{stderr}");
}

#[test]
fn installed_cli_finds_user_packs_without_an_export_and_honors_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data = dir.path().join("custom-data");
    let installed = data.join("muz/contrib");
    let default = home.join(".local/share/muz/contrib");
    let override_dir = dir.path().join("override");
    for (root, value) in [(&installed, 42), (&default, 24), (&override_dir, 7)] {
        std::fs::create_dir_all(root.join("test-pack")).unwrap();
        std::fs::write(
            root.join("test-pack/voice.muz"),
            format!("let level = {value};"),
        )
        .unwrap();
    }
    // Relocate the real binary so checkout discovery cannot mask the regression.
    let bin = dir.path().join("cargo/bin/muz");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_muz"), &bin).unwrap();
    let project = dir.path().join("unrelated-project");
    std::fs::create_dir_all(&project).unwrap();
    let source = project.join("song.muz");
    std::fs::write(&source, "use \"contrib/test-pack/voice\" as v; v.level").unwrap();
    let run = |xdg: Option<&Path>, override_path: Option<&Path>| {
        let mut command = Command::new(&bin);
        command
            .current_dir(&project)
            .env("HOME", &home)
            .env_remove("MUZ_CONTRIB_DIR")
            .env_remove("XDG_DATA_HOME");
        if let Some(path) = xdg {
            command.env("XDG_DATA_HOME", path);
        }
        if let Some(path) = override_path {
            command.env("MUZ_CONTRIB_DIR", path);
        }
        command.arg("eval").arg(&source).output().unwrap()
    };
    for (xdg, override_path, expected) in [
        (Some(data.as_path()), None, "42"),
        (None, None, "24"),
        (Some(Path::new("relative")), None, "24"),
        (Some(data.as_path()), Some(override_dir.as_path()), "7"),
    ] {
        let output = run(xdg, override_path);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
    }
    let output = run(Some(&data), Some(&dir.path().join("missing")));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("MUZ_CONTRIB_DIR"));
}
