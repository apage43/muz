//! Small source-level reproductions; compositions remain freely editable.
#[test]
fn quoted_syntax_remains_literal_through_evaluation_and_formatting() {
    for literal in [
        "if", "else", "fn", "let", "use", "as", "export", "true", "false", "null", "<eof>", "{",
        "}", "[", "]", "(", ")", ",", ";", ":", "=", ".", "-", "!", "+", "*", "/", "%", "==", "!=",
        "<", ">", "<=", ">=", "&&", "||", "=>",
    ] {
        // Exercise statement/collection ends, defaults, lookahead and binary operators.
        let quoted = serde_json::to_string(literal).unwrap();
        for source in [
            format!("let x = {quoted}; x"),
            format!("{quoted}"),
            format!("fn f(x={quoted}) {{ x }} f()"),
            format!("[{quoted}][0]"),
            format!("{{{quoted}:{quoted}}}[{quoted}]"),
            format!("if true {{ {quoted} }} else {{ \"other\" }}"),
        ] {
            let evaluate = |s: &str| {
                muz::lang::Evaluator::new()
                    .source(s)
                    .unwrap_or_else(|e| panic!("{s}: {e:#}"))
                    .get("__result")
                    .unwrap()
                    .json()
            };
            assert_eq!(evaluate(&source), literal, "{source}");
            let formatted = muz::lang::format::format(&source).unwrap();
            assert_eq!(evaluate(&formatted), literal, "{formatted}");
            assert_eq!(muz::lang::format::format(&formatted).unwrap(), formatted);
        }
    }
    for invalid in [
        r#"let "x" = 1"#,
        r#"let x "=" 1"#,
        r#"fn f "(" x) = x"#,
        r#"if true {1} "else" {2}"#,
        r#"use "m.muz" "as" m"#,
        r#"[1 "," 2]"#,
    ] {
        assert!(
            muz::lang::parse(invalid).is_err(),
            "accepted quoted syntax: {invalid}"
        );
    }
}

#[test]
fn nested_errors_show_the_offending_line_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested.muz");
    std::fs::write(
        &path,
        concat!(
            "fn make_note() = note(\"C4\", 1b, offset=2ms);\n",
            "fn phrase_layer() = stack(map(range(2), fn(i) => make_note()));\n",
            "fn passage_layer() = stack(map(range(2), fn(i) => phrase_layer()));\n",
            "passage_layer()\n",
        ),
    )
    .unwrap();
    let error = muz::lang::load(&path).unwrap_err().to_string();
    assert!(error.contains("nested.muz:1:18"), "{error}");
    assert!(error.contains("note(\"C4\", 1b, offset=2ms)"), "{error}");
    assert!(error.contains("unexpected arguments: offset"), "{error}");
    assert!(!error.contains("at byte"), "{error}");
    assert!(
        error
            .lines()
            .next()
            .unwrap()
            .ends_with("unexpected arguments: offset"),
        "{error}"
    );
    assert_eq!(
        error.matches(&path.display().to_string()).count(),
        1,
        "{error}"
    );
    for line in [2, 3, 4] {
        assert_eq!(
            error.matches(&format!("nested.muz:{line}:")).count(),
            1,
            "{error}"
        );
    }
    assert!(
        error.contains(&"^".repeat("note(\"C4\", 1b, offset=2ms)".len())),
        "{error}"
    );
}

#[test]
fn evaluation_diagnostics_preserve_imports_defaults_and_unicode_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("caller.muz");
    let module = dir.path().join("helper.muz");
    // A Unicode value before the span makes byte offsets differ from columns.
    let helper = "fn bad() { let label = \"é\"; 1b*2b }\n";
    std::fs::write(&module, helper).unwrap();
    std::fs::write(&path, "use \"helper.muz\" as h;\nh.bad()\n").unwrap();
    let error = muz::lang::load(&path).unwrap_err().to_string();
    assert!(
        error.starts_with(&format!("{}:1:29:", module.display())),
        "{error}"
    );
    assert!(error.contains("1b*2b"), "{error}");
    assert!(error.contains("caller.muz:2:1"), "{error}");
    std::fs::write(&module, "fn bad(x=note(60,offset=2ms)) = x;\n").unwrap();
    let error = muz::lang::load(&path).unwrap_err().to_string();
    assert!(
        error.starts_with(&format!("{}:1:10:", module.display())),
        "{error}"
    );
    assert!(error.contains("caller.muz:2:1"), "{error}");
    // Bundled source is available too, and returning from errors restores the caller path.
    let mut evaluator = muz::lang::Evaluator::new();
    let error = evaluator
        .source("use \"std/patterns\" as p; p.swing(note(60),0.1)")
        .unwrap_err()
        .to_string();
    assert!(error.contains("<std/"), "{error}");
    assert!(!error.contains("at byte"), "{error}");
    assert_eq!(evaluator.path, std::path::Path::new("<source>"));

    let mut chain = String::from("fn f0() = 1b*2b;\n");
    for i in 1..13 {
        chain.push_str(&format!("fn f{i}() = f{}();\n", i - 1));
    }
    std::fs::write(&module, chain).unwrap();
    std::fs::write(&path, "use \"helper.muz\" as h;\nh.f12()\n").unwrap();
    let error = muz::lang::load(&path).unwrap_err().to_string();
    assert!(error.contains("caller lines omitted"), "{error}");
    assert!(error.contains("caller.muz:2:1"), "{error}");
    assert!(error.lines().count() < 16, "{error}");
}

#[test]
fn sampler_prepares_two_hundred_zones() {
    let dir = tempfile::tempdir().unwrap();
    let wave = dir.path().join("tiny.wav");
    let mut writer = hound::WavWriter::create(
        wave,
        hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    writer.write_sample(0.25f32).unwrap();
    writer.finalize().unwrap();
    let path = dir.path().join("zones.muz");
    std::fs::write(
        &path,
        r#"use "std/arrange" as arrange;
        let zones = arrange.flatten(map(range(25),fn(i)=>map(range(8),fn(take)=>{
            path:"tiny.wav",root:48+i,keys:[48+i,48+i]
        })));
        song({tracks:[track("p",note(72).annotate("all",{sample_zone:199}),sample(zones))]})"#,
    )
    .unwrap();
    let session = muz::compile::compile(&path).unwrap().session;
    let resources = session.graph_resources();
    assert_eq!(resources.sample_zones, 200);
    assert_eq!(resources.units, 203);
    let mut engine = muz::audio::AudioEngine::new(
        &session,
        muz::audio::AudioConfig {
            sample_rate: 8000.,
            max_frames: 32,
            offline: false,
        },
    )
    .unwrap();
    engine.set_running(true);
    let mut audio = [0.; 64];
    engine.render_interleaved(&mut audio, 2).unwrap();
    assert!(audio.iter().any(|x| *x > 0.0));
    // Budget failure is aggregate and reported before loading any recordings.
    std::fs::remove_file(dir.path().join("tiny.wav")).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_muz"))
        .env("MUZ_GRAPH_BUDGET", "202")
        .arg("check")
        .arg(&path)
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        error.contains("requires 203 resource units; allowed 202"),
        "{error}"
    );
    assert!(error.contains("200 sample zones"), "{error}");
    assert!(error.contains("track p: 202"), "{error}");
}

#[test]
fn native_delay_accepts_short_clock_durations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delay.muz");
    std::fs::write(
        &path,
        r#"song({tempo:132,tracks:[track("p",note(60),synth("init"),
        {chain:[fx("delay",{time_ms:11.7})]})]})"#,
    )
    .unwrap();
    muz::compile::compile(&path).unwrap();
}
