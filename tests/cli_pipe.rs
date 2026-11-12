//! A reader that closes standard output early does not turn a passing command
//! into a panic.
use std::process::{Command, Stdio};

fn run_with_closed_stdout(args: &[&str]) -> std::process::Output {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    Command::new(env!("CARGO_BIN_EXE_muz"))
        .args(args)
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

#[test]
fn closed_stdout_keeps_the_commands_own_status() {
    let dir = tempfile::tempdir().unwrap();
    let ok = dir.path().join("ok.muz");
    std::fs::write(
        &ok,
        r#"song({tracks:[track("lead",note(60,1b),synth("init"))]})"#,
    )
    .unwrap();
    let out = run_with_closed_stdout(&["check", ok.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{:?}: {stderr}", out.status);
    assert!(!stderr.contains("panicked"), "{stderr}");

    let bad = dir.path().join("bad.muz");
    std::fs::write(
        &bad,
        "song({tracks:[track(\"lead\",note(60,1b),nothing())]})",
    )
    .unwrap();
    let out = run_with_closed_stdout(&["check", bad.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}
