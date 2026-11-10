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
            quoted.clone(),
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

#[test]
fn diatonic_transpose_accepts_downward_and_zero_steps() {
    let pitches = |steps: &str| {
        let source = format!(
            r#"let key = scale("C", "major", octave = 4);
            map(note("E4").diatonic_transpose(key, {steps}).notes, fn(n) => n.pitch)"#
        );
        muz::lang::Evaluator::new()
            .source(&source)
            .unwrap_or_else(|e| panic!("{source}: {e:#}"))
            .get("__result")
            .unwrap()
            .json()
    };
    assert_eq!(pitches("-2"), serde_json::json!([60]));
    assert_eq!(pitches("-7"), serde_json::json!([52]));
    assert_eq!(pitches("0"), serde_json::json!([64]));
    let error = muz::lang::Evaluator::new()
        .source(r#"note("E4").diatonic_transpose(scale("C", "major"), 1.5)"#)
        .err()
        .expect("fractional steps are rejected");
    assert!(format!("{error:#}").contains("argument 'steps'"), "{error:#}");
}

fn eval_json(source: &str) -> serde_json::Value {
    muz::lang::Evaluator::new()
        .source(source)
        .unwrap_or_else(|e| panic!("{source}: {e:#}"))
        .get("__result")
        .unwrap()
        .json()
}

fn eval_error(source: &str) -> String {
    let error = muz::lang::Evaluator::new()
        .source(source)
        .err()
        .unwrap_or_else(|| panic!("{source} should fail"));
    format!("{error:#}")
}

#[test]
fn range_reads_start_then_end_and_names_the_old_order() {
    assert_eq!(eval_json("range(3)"), serde_json::json!([0, 1, 2]));
    assert_eq!(eval_json("range(4, 8)"), serde_json::json!([4, 5, 6, 7]));
    assert_eq!(eval_json("range(0, 10, 4)"), serde_json::json!([0, 4, 8]));
    assert_eq!(eval_json("range(2, 4)"), eval_json("range(end = 4, start = 2)"));
    assert_eq!(eval_json("range(step = 2, end = 5)"), serde_json::json!([0, 2, 4]));
    assert_eq!(eval_json("range(1, end = 3)"), serde_json::json!([1, 2]));
    assert_eq!(eval_json("range(4, step = 2)"), serde_json::json!([0, 2]));
    assert_eq!(eval_json("range(4, 4)"), serde_json::json!([]));
    let error = eval_error("range(8, 4)");
    assert!(
        error.contains("range(start, end): start 8 is after end 4; write range(4, 8)"),
        "{error}"
    );
}

#[test]
fn chars_splits_text_into_unicode_scalar_strings() {
    assert_eq!(
        eval_json(r#"chars("c.X.")"#),
        serde_json::json!(["c", ".", "X", "."])
    );
    assert_eq!(eval_json(r#"chars("")"#), serde_json::json!([]));
    assert_eq!(
        eval_json(r#"chars("a♯é")"#),
        serde_json::json!(["a", "♯", "é"])
    );
    assert_eq!(eval_json(r#"chars("c.X.")[2]"#), serde_json::json!("X"));
    assert!(eval_error("chars(3)").contains("text"));
    assert!(eval_error(r#"chars(["a"])"#).contains("text"));
}

#[test]
fn join_concatenates_text_and_inverts_chars() {
    assert_eq!(eval_json(r#"join(["a", "b", "c"])"#), serde_json::json!("abc"));
    assert_eq!(
        eval_json(r#"join(["a", "b", "c"], "-")"#),
        serde_json::json!("a-b-c")
    );
    assert_eq!(eval_json("join([])"), serde_json::json!(""));
    assert_eq!(
        eval_json(r#"join(chars("c.X♯"))"#),
        serde_json::json!("c.X♯")
    );
    let error = eval_error(r#"join(["a", 3])"#);
    assert!(error.contains("join item 1 must be text"), "{error}");
    assert!(eval_error(r#"join("abc")"#).contains("list"));
}

#[test]
fn drum_grid_hits_keep_their_strike_symbol() {
    let setup = r#"let g = drum_grid({a: "Xk.x", b: ["k", "."]}, {a: 40, b: 41}, {X: 0.9, k: 0.5, x: 0.5, ".": null}, 0.5, 4b);"#;
    // From a callback: same velocity, different symbols.
    assert_eq!(
        eval_json(&format!(
            r#"{setup} map(select(g, "voice:a").notes, fn(n) => n.data.symbol)"#
        )),
        serde_json::json!(["X", "k", "x"])
    );
    // From a selector, on tag and data.
    assert_eq!(
        eval_json(&format!(
            r#"{setup} map(select(g, "tag:sym:k").notes, fn(n) => n.pitch)"#
        )),
        serde_json::json!([40, 41])
    );
    assert_eq!(
        eval_json(&format!(
            r#"{setup} len(filter_notes(g, fn(n) => n.data.symbol == "x").notes)"#
        )),
        serde_json::json!(1)
    );
    assert!(eval_error(r#"drum_grid({a: ["kk"]}, {a: 40}, {k: 0.5})"#).contains("one-character"));
}

#[test]
fn list_slice_is_half_open_and_diagnoses_bad_bounds() {
    assert_eq!(
        eval_json("slice([1,2,3,4], 1, 3)"),
        serde_json::json!([2, 3])
    );
    assert_eq!(
        eval_json("slice([1,2,3], 0, 3)"),
        serde_json::json!([1, 2, 3])
    );
    assert_eq!(eval_json("slice([1,2,3], 2, 2)"), serde_json::json!([]));
    assert_eq!(eval_json("slice([], 0, 0)"), serde_json::json!([]));
    assert_eq!(
        eval_json("slice(list = [1,2,3], from = 1, to = 2)"),
        serde_json::json!([2])
    );
    assert!(eval_error("slice([1,2,3], 0, 4)").contains("out of range"));
    assert!(eval_error("slice([1,2,3], 3, 1)").contains("increasing"));
    assert!(eval_error("slice([1,2,3], -1, 2)").contains("slice from"));
    assert!(eval_error("slice([1,2,3], 0.5, 2)").contains("slice from"));
    // Patterns keep their time-window meaning, as builtin and as method.
    let kept = eval_json(
        r#"let p = seq([note("C4", 1b), note("D4", 1b)]);
        [map(slice(p, 1b, 2b).notes, fn(n) => n.pitch), map(p.slice(0b, 1b).notes, fn(n) => n.pitch)]"#,
    );
    assert_eq!(kept, serde_json::json!([[62], [60]]));
}

#[test]
fn record_builds_tables_from_computed_keys() {
    assert_eq!(
        eval_json(r#"record(map(["a","b"], fn(k) => [k + "2", len(k)]))"#),
        serde_json::json!({"a2": 1, "b2": 1})
    );
    assert_eq!(eval_json("record([])"), serde_json::json!({}));
    assert_eq!(
        eval_json(r#"record([["k", [1, 2]]])["k"]"#),
        serde_json::json!([1, 2])
    );
    assert!(eval_error(r#"record([["a", 1], ["a", 2]])"#).contains("duplicate key 'a'"));
    assert!(eval_error("record([[1, 2]])").contains("text key"));
    assert!(eval_error(r#"record([["a"]])"#).contains("pair 0"));
    assert!(eval_error(r#"record([["a", 1, 2]])"#).contains("pair 0"));
    assert!(eval_error(r#"record(["ab"])"#).contains("pair 0"));
    assert!(eval_error("record({a: 1})").contains("list"));
}

#[test]
fn budget_diagnostic_names_grouping_and_indexing() {
    let message = eval_error(
        "map(range(3000), fn(x) => len(filter(range(3000), fn(y) => y == x)))",
    );
    assert!(message.contains("evaluation budget exceeded"), "{message}");
    assert!(
        message.contains("group_by") && message.contains("index_by"),
        "{message}"
    );
}
