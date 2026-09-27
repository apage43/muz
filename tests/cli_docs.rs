//! The documentation command is an offline interface: it needs no project or devices.
#![cfg(feature = "desktop")]

use std::process::{Command, Output};

fn docs(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_muz"))
        .arg("docs")
        .args(args)
        .current_dir(std::env::temp_dir())
        .output()
        .expect("run documentation command")
}

#[test]
fn default_opens_the_documentation_index() {
    let default = docs(&[]);
    let index = docs(&["index"]);
    assert!(default.status.success());
    assert!(index.status.success());
    assert_eq!(default.stdout, index.stdout);
    let text = String::from_utf8(default.stdout).unwrap();
    assert!(text.starts_with("# muz documentation\n"));
    assert!(text.contains("muz docs getting-started"));
}

#[test]
fn existing_and_new_topics_are_available_offline_and_listed_in_help() {
    let help = docs(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for topic in [
        "language",
        "performance",
        "production",
        "synthesis",
        "workflow",
        "getting-started",
        "instruments",
        "dawproject",
        "dawproject-validation",
        "embedding",
    ] {
        let output = docs(&[topic]);
        assert!(output.status.success(), "{topic}: {:?}", output.stderr);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.starts_with("# "), "{topic} has no document");
        assert!(text.contains(&format!("muz docs {topic}")));
        assert!(help.contains(topic), "help omits {topic}");
    }
}

#[test]
fn unknown_topic_fails_with_available_topics_and_a_suggestion() {
    let output = docs(&["instrumnts"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("instrumnts"));
    assert!(error.contains("did you mean"));
    assert!(error.contains("instruments"));
    assert!(error.contains("getting-started"));
    assert!(error.contains("dawproject-validation"));
}
