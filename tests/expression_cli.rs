//! `muz eval --expr` and standard-input modules, run through the built binary.
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run(dir: &std::path::Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_muz"))
        .arg("eval")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    if let Some(text) = stdin {
        pipe.write_all(text.as_bytes()).unwrap();
    }
    drop(pipe);
    child.wait_with_output().unwrap()
}

fn json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn expression_sees_module_bindings_and_relative_use() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("lib")).unwrap();
    std::fs::write(dir.path().join("lib/helper.muz"), "let base = 40;\n").unwrap();
    std::fs::write(
        dir.path().join("lib/piece.muz"),
        "use \"helper.muz\" as helper;\nlet two = 2;\nfn double(x) { x * two }\n{unused: 0}\n",
    )
    .unwrap();
    // Run from the parent so relative `use` must resolve against the module file.
    let out = run(
        dir.path(),
        &["lib/piece.muz", "--expr", "double(helper.base) + two"],
        None,
    );
    assert_eq!(json(&out), 82);
    // Without --expr the module value is unchanged.
    let out = run(dir.path(), &["lib/piece.muz"], None);
    assert_eq!(json(&out)["unused"], 0);
}

#[test]
fn stdin_module_resolves_use_against_current_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("helper.muz"), "let base = 40;\n").unwrap();
    let module = "use \"helper.muz\" as helper;\nlet n = helper.base + 1;\nn\n";
    assert_eq!(json(&run(dir.path(), &["-"], Some(module))), 41);
    let out = run(dir.path(), &["-", "--expr", "n + helper.base"], Some(module));
    assert_eq!(json(&out), 81);
}

#[test]
fn expression_errors_point_into_the_expression() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.muz"), "let a = 1;\n").unwrap();
    let out = run(dir.path(), &["m.muz", "--expr", "a + missing_name"], None);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("<expr>:1:"), "{stderr}");
    assert!(stderr.contains("missing_name"), "{stderr}");
    assert!(!stderr.contains("m.muz"), "{stderr}");
}
