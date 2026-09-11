//! Diagnostics name the best available source location and list mechanical
//! fixes. Small synthetic sources only; pieces stay freely editable.
use std::path::{Path, PathBuf};

fn write(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).unwrap();
    path
}

fn load_error(path: &Path) -> String {
    format!("{:#}", muz::lang::load(path).unwrap_err())
}

fn compile_error(path: &Path) -> String {
    format!("{:#}", muz::compile::compile(path).unwrap_err())
}

#[test]
fn parse_failures_name_the_token_and_how_to_close_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "parse.muz", "let a = 1;\nlet b = \"open;\n");
    let error = load_error(&path);
    assert!(error.contains("parse.muz:2:9"), "{error}");
    assert!(error.contains("unclosed string"), "{error}");
    assert!(error.contains("= help: close the string with a matching \""), "{error}");
    assert!(!error.contains("at byte"), "{error}");

    let path = write(dir.path(), "char.muz", "let a = 1 $ 2;\n");
    let error = load_error(&path);
    assert!(error.contains("char.muz:1:11"), "{error}");
    assert!(error.contains("unexpected character '$'"), "{error}");

    let path = write(dir.path(), "close.muz", "let a = f(1;\n");
    let error = load_error(&path);
    assert!(error.contains("close.muz:1:12"), "{error}");
    assert!(error.contains("expected ')', got ';'"), "{error}");
}

#[test]
fn unknown_names_list_the_accepted_vocabulary() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let path = write(dir, "unit.muz", "let y = 3zz;\n");
    let error = load_error(&path);
    assert!(error.contains("unit.muz:1:9"), "{error}");
    assert!(error.contains("unknown unit 'zz'"), "{error}");
    assert!(error.contains("did you mean 'Hz'?"), "{error}");
    assert!(error.contains("units: b, beat, beats"), "{error}");

    let path = write(dir, "field.muz", "let s = {tempo: 1};\ns.tempos;\n");
    let error = load_error(&path);
    assert!(error.contains("field.muz:2:1"), "{error}");
    assert!(error.contains("record has no field 'tempos'"), "{error}");
    assert!(error.contains("did you mean 'tempo'?"), "{error}");

    let path = write(dir, "call.muz", "let x = notee(60);\n");
    let error = load_error(&path);
    assert!(error.contains("call.muz:1:9"), "{error}");
    assert!(error.contains("unknown function 'notee'"), "{error}");
    assert!(error.contains("did you mean 'note'?"), "{error}");

    let path = write(dir, "module.muz", "use \"std/frogs\" as f;\n1b;\n");
    let error = load_error(&path);
    assert!(error.contains("module.muz:1:1"), "{error}");
    assert!(error.contains("unknown standard module 'std/frogs'"), "{error}");
    assert!(error.contains("std/patterns"), "{error}");
}

#[test]
fn lowering_failures_point_at_the_declaring_call() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let song = |body: &str| format!("song({{{body}}});\n");

    let path = write(
        dir,
        "effect.muz",
        &song("tracks: [track(\"a\", note(60, 1b), piano(\"default\"), {chain: [fx(\"lowpas\")]})]"),
    );
    let error = compile_error(&path);
    assert!(error.contains("effect.muz:1:68"), "{error}");
    assert!(error.contains("unknown effect 'lowpas'"), "{error}");
    assert!(error.contains("did you mean 'lowpass'?"), "{error}");

    let path = write(
        dir,
        "parameter.muz",
        &song("tracks: [track(\"a\", note(60, 1b), piano(\"default\"), {chain: [fx(\"eq\", {freq_hz: 500})]})]"),
    );
    let error = compile_error(&path);
    assert!(error.contains("unknown parameter 'freq_hz'"), "{error}");
    assert!(error.contains("parameters: frequency_hz, gain_db, q, mode"), "{error}");

    let path = write(
        dir,
        "bus.muz",
        &song("tracks: [track(\"a\", note(60, 1b), piano(\"default\"), {output: \"buses\"})]"),
    );
    let error = compile_error(&path);
    assert!(error.contains("targets missing bus buses"), "{error}");
    assert!(error.contains("buses: master"), "{error}");

    let path = write(dir, "empty.muz", &song(""));
    let error = compile_error(&path);
    assert!(error.contains("empty.muz:1:1"), "{error}");
    assert!(error.contains("song needs at least one track"), "{error}");
    assert!(error.contains("= help: add at least one `track(...)` entry"), "{error}");

    let path = write(
        dir,
        "section.muz",
        &song(
            "tracks: [track(\"a\", note(60, 1b), piano(\"default\"))], sections: [section(\"intro\", 4b), section(\"intro\", 4b)]",
        ),
    );
    let error = compile_error(&path);
    assert!(error.contains("duplicate section 'intro'"), "{error}");
    // The second declaration is the one that has to change.
    assert!(error.contains("section.muz:1:94"), "{error}");

    let path = write(
        dir,
        "kit.muz",
        &song("tracks: [track(\"drums\", drums({kik: \"X...\", snare: \"..X.\"}), kit(\"default\"))]"),
    );
    let error = compile_error(&path);
    assert!(error.contains("grid has no pitch mapping for voice 'kik'"), "{error}");
    assert!(error.contains("did you mean 'kick'?"), "{error}");
    assert!(error.contains("kit.muz:1:31"), "{error}");
}

#[test]
fn automation_targets_name_both_the_declaration_and_the_expected_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "automation.muz",
        concat!(
            "song({\n",
            "    tempo: 120,\n",
            "    tracks: [track(\"drums\", drums({kick: \"X...\"}), kit(\"default\"), {chain: [fx(\"lowpass\", {id: \"tone\"})]})],\n",
            "    automation: [automation(\"drums.tone.cutoff_hz\", curve([[0b, 500], [4b, 16000]]))],\n",
            "    tail: 1\n",
            "});\n",
        ),
    );
    let session = muz::compile::compile(&path).unwrap().session;
    let error = match muz::audio::AudioEngine::new(
        &session,
        muz::audio::AudioConfig {
            sample_rate: 48000.,
            max_frames: 256,
            offline: false,
        },
    ) {
        Ok(_) => panic!("the automation target is not in the expanded graph"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("automation.muz:4:18"), "{error}");
    assert!(error.contains("automation target 'drums.tone.cutoff_hz' is unknown"), "{error}");
    // A kit expands into physical voices; the clue lists what the graph accepts.
    assert!(error.contains("drums.kick.tone.cutoff_hz"), "{error}");
}

#[test]
fn every_listed_function_name_resolves() {
    for name in muz::lang::function_names() {
        let mut evaluator = muz::lang::Evaluator::new();
        if let Err(error) = evaluator.source(&format!("{name}()")) {
            let message = format!("{error:#}");
            assert!(
                !message.contains("unknown function"),
                "function_names() lists '{name}', which the dispatch does not know: {message}"
            );
        }
    }
}

/// Names the `call` dispatch matches on, read straight from its arms.
fn dispatched_names() -> Vec<String> {
    let source = include_str!("../src/lang/builtins.rs");
    let start = source.find("pub fn call(").expect("dispatch entry point");
    let mut names = Vec::new();
    let mut pending: Vec<&str> = Vec::new();
    for line in source[start..].lines() {
        if line == "}" {
            break;
        }
        if line.starts_with("        \"") || !pending.is_empty() {
            pending.push(line);
            if line.contains("=>") {
                let arm = pending.join("\n");
                let head = arm.split("=>").next().unwrap_or_default();
                for name in head.split('"').skip(1).step_by(2) {
                    names.push(name.to_owned());
                }
                pending.clear();
            }
        }
    }
    names
}

#[test]
fn the_function_name_table_lists_every_dispatch_arm() {
    // The table feeds "did you mean" hints; a missing name silently weakens them.
    let listed = muz::lang::function_names();
    let dispatched = dispatched_names();
    assert!(dispatched.len() > 50, "{dispatched:?}");
    let missing: Vec<&String> = dispatched
        .iter()
        .filter(|name| !listed.contains(&name.as_str()))
        .collect();
    assert!(missing.is_empty(), "not offered as suggestions: {missing:?}");
}
