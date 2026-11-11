use std::{
    path::Path,
    process::{Command, Output},
};
fn source(dir: &Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("recipe.muz");
    std::fs::write(&path, body).unwrap();
    path
}
fn batch(source: &Path, recipe: &str, out: &Path, matched: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_muz"));
    command
        .arg("batch")
        .arg(source)
        .arg(recipe)
        .arg("--output")
        .arg(out);
    if matched {
        command.arg("--match-levels");
    }
    command.output().unwrap()
}
const SONG: &str = r#"song({tracks:[track("tone",phrase("C4:w"),synth("bell"))],tail:0})"#;
#[test]
fn named_recipe_runs_isolated_workers_and_listening_gain_does_not_change_audio() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(
        dir.path(),
        &format!(
            r#"
let original={SONG};
let quiet=merge(original,{{tracks:[track("tone",phrase("C4:w"),synth("bell"),{{gain:-12}})]}});
let comparisons=[{{name:"original",song:original,options:{{seconds:1.2,sample_rate:16000,block_size:128}}}},{{name:"quiet",song:quiet,options:{{seconds:1.2,sample_rate:pow(2,7)*125,block_size:pow(2,7)}}}}];
"#
        ),
    );
    let plain = dir.path().join("plain");
    let matched = dir.path().join("matched");
    for (out, levels) in [(&plain, false), (&matched, true)] {
        let result = batch(&path, "comparisons", out, levels);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("renders.json")).unwrap()).unwrap();
        assert_eq!(report["outputs"].as_array().unwrap().len(), 2);
        assert_eq!(report["outputs"][0]["name"], "original");
        assert_eq!(report["outputs"][1]["name"], "quiet");
        assert!(out.join("listen.html").exists());
        if levels {
            assert!(report["outputs"][0]["listening_gain_db"].as_f64().unwrap() < -8.);
            assert_eq!(
                report["outputs"][1]["listening_gain_db"].as_f64().unwrap(),
                0.
            );
        }
    }
    for name in ["original.wav", "quiet.wav"] {
        assert_eq!(
            std::fs::read(plain.join(name)).unwrap(),
            std::fs::read(matched.join(name)).unwrap()
        );
        let wav = hound::WavReader::open(matched.join(name)).unwrap();
        assert_eq!(wav.spec().sample_rate, 16000);
        assert!(
            wav.into_samples::<f32>()
                .map(Result::unwrap)
                .any(|s| s.abs() > 0.0001)
        );
    }
}
#[test]
fn recipe_rejects_ambiguous_names_and_unknown_options_before_rendering() {
    let dir = tempfile::tempdir().unwrap();
    for (i, rows, expected) in [
        (
            0,
            format!(r#"[{{name:"../escape",song:{SONG}}}]"#),
            "output names",
        ),
        (
            1,
            format!(r#"[{{name:"same",song:{SONG}}},{{name:"same",song:{SONG}}}]"#),
            "duplicate render output",
        ),
        (
            2,
            format!(r#"[{{name:"ok",song:{SONG},options:{{typo:true}}}}]"#),
            "unknown field",
        ),
        (3, "[]".into(), "1..34"),
        (
            4,
            format!(r#"[{{name:"ok",song:{SONG},surprise:1}}]"#),
            "unknown render output field",
        ),
    ] {
        let path = source(dir.path(), &format!("let recipe={rows};"));
        let out = dir.path().join(format!("invalid{i}"));
        let result = batch(&path, "recipe", &out, false);
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !out.exists(),
            "validation should finish before outputs start"
        );
    }
    let path = source(dir.path(), "let unrelated=[];");
    let result = batch(&path, "missing", &dir.path().join("missing"), false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no render recipe"));
}
#[test]
fn failed_output_reports_error_without_discarding_successful_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(
        dir.path(),
        &format!(
            r#"let recipe=[{{name:"good",song:{SONG},options:{{seconds:0.05}}}},{{name:"bad",song:{SONG},options:{{section:"missing"}}}}];"#
        ),
    );
    let out = dir.path().join("results");
    let result = batch(&path, "recipe", &out, false);
    assert!(!result.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("renders.json")).unwrap()).unwrap();
    assert!(report["outputs"][0].get("report").is_some());
    assert!(
        report["outputs"][1]["error"]
            .as_str()
            .unwrap()
            .contains("unknown section")
    );
    assert!(out.join("good.wav").exists());
    assert!(!out.join("bad.wav").exists());
}

#[test]
fn recipe_time_options_reject_musical_units() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(
        dir.path(),
        &format!("let recipe=[{{name:\"bad\",song:{SONG},options:{{tail:2b}}}}];"),
    );
    let error = muz::recipes::prepare(&path, "recipe", dir.path())
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("incompatible units"));
}

#[test]
fn explicit_excerpt_seconds_include_tail_while_unbounded_excerpt_adds_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(
        dir.path(),
        &format!(
            r#"let recipe=[
                {{name:"bounded",song:{SONG},options:{{start:0.25,seconds:0.125,tail:0.5,sample_rate:8000}}}},
                {{name:"to-end",song:{SONG},options:{{start:0.25,tail:0.5,sample_rate:8000}}}}
            ];"#
        ),
    );
    let out = dir.path().join("excerpts");
    let result = batch(&path, "recipe", &out, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for (name, frames) in [("bounded", 1000), ("to-end", 18000)] {
        let wav = hound::WavReader::open(out.join(format!("{name}.wav"))).unwrap();
        assert_eq!(wav.duration(), frames, "{name} excerpt duration");
    }
}

#[test]
fn match_moves_only_the_named_fader_to_the_quietest_candidate() {
    let dir = tempfile::tempdir().unwrap();
    // Two lead candidates 9 dB apart over one backing; each is written in
    // context and soloed.
    let path = source(
        dir.path(),
        r#"
fn version(gain) = song({tracks:[
    track("lead",phrase("C5:w"),synth("bell"),{gain:gain}),
    track("pad",phrase("C3:w"),synth("bell"),{gain:-6})
],tail:0});
let scope={seconds:1.5,sample_rate:16000};
let compare=[
    {name:"loud",song:version(0),options:merge(scope,{match:"lead"})},
    {name:"loud-solo",song:version(0),options:merge(scope,{match:"lead",solo:["lead"]})},
    {name:"quiet",song:version(-9),options:merge(scope,{match:"lead"})},
    {name:"backing",song:version(0),options:merge(scope,{tap:"pad"})}
];
let wrong=[{name:"x",song:version(0),options:{match:"leed"}}];
"#,
    );
    let out = dir.path().join("out");
    let result = batch(&path, "compare", &out, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("renders.json")).unwrap()).unwrap();
    let offset = |i: usize| report["outputs"][i]["match"]["offset_db"].as_f64().unwrap();
    assert!((offset(0) + 9.0).abs() < 0.05, "{report}");
    assert_eq!(offset(0), offset(1));
    assert_eq!(offset(2), 0.0);
    assert!(report["outputs"][3].get("match").is_none());
    // The matched loud candidate now measures as the quiet one does.
    let lufs = |name: &str| {
        muz::analysis::analyze(&out.join(format!("{name}.wav"))).unwrap()["integrated_lufs"]
            .as_f64()
            .unwrap()
    };
    assert!((lufs("loud") - lufs("quiet")).abs() < 0.05);

    let error = muz::recipes::prepare(&path, "wrong", dir.path())
        .err()
        .unwrap();
    let text = format!("{error:?}");
    assert!(
        text.contains("match names no track 'leed'") && text.contains("lead"),
        "{text}"
    );
}
