use muz::{lang, music};
fn eval(source: &str) -> lang::Value {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("test.muz");
    std::fs::write(&p, source).unwrap();
    lang::load(&p).unwrap().0
}
#[test]
fn source_tonal_catalog_can_be_extended_without_kernel_changes() {
    let v = eval(
        "use \"std/catalogs\" as c; let key = scale(60, \"custom\", modes = {custom:[0,3,7]}); [degree(key, 0), degree(key, -2), diatonic_chord(key, 2), scale(\"C\", \"major\")]",
    );
    let a = v.array().unwrap();
    assert_eq!(a[0].number().unwrap(), 55.);
    assert_eq!(a[1].number().unwrap(), 48.);
    assert_eq!(
        a[2].array()
            .unwrap()
            .iter()
            .map(|v| v.number().unwrap())
            .collect::<Vec<_>>(),
        vec![63., 72., 79.]
    );
    assert_eq!(a[3].array().unwrap().len(), 7);
}

#[test]
fn prelude_data_exports_are_ordinary_values() {
    let v = eval(
        r#"
let voices = merge(drum_voices, {clap:39});
let make_drums = drums;
let pattern = make_drums({clap:"X..."}, voices=voices);
[
    voices.clap,
    drum_articulations["X"],
    merge(kit_defaults, {name:"custom"}).name,
    merge(synth_presets, {custom:{mode:"sine"}}).custom.mode,
    scale_modes.major[1],
    piano_preferences.hand_centers[1],
    zero_pitch_costs[0],
    pattern.notes[0].pitch,
    len([1, 2])
]
"#,
    );
    assert_eq!(
        v.json(),
        serde_json::json!([39, 0.95, "custom", "sine", 2, 72, 0, 39, 2])
    );
}
#[test]
fn grid_accepts_source_voice_and_articulation_vocabulary() {
    let v = eval(
        "drums({brush:\"B..B\"}, voices={brush:60}, articulations={\"B\":0.43,\".\":null}, gate=0.25)",
    );
    let p = v.pattern().unwrap();
    assert_eq!(p.span, music::b(4));
    assert_eq!(p.notes.len(), 2);
    assert!(p.notes.iter().all(|n| n.voice == "brush"
        && n.pitch == 60.
        && n.velocity == 0.43
        && n.dur == music::decimal("0.25").unwrap()));
}
#[test]
fn euclidean_source_preserves_span_and_rotation() {
    let v = eval("euclidean(3, 8, rotation=2)");
    let p = v.pattern().unwrap();
    assert_eq!(p.span, music::b(4));
    assert_eq!(p.notes.len(), 3);
    let mut at = p.notes.iter().map(|n| n.at).collect::<Vec<_>>();
    at.sort();
    assert_eq!(
        at,
        vec![music::b(0), music::b(1), music::decimal("2.5").unwrap()]
    );
    assert_eq!(eval("euclidean(0,8)").pattern().unwrap().span, music::b(4));
}
#[test]
fn source_synth_catalog_and_custom_records_compile() {
    let v = eval(
        "song({tracks:[track(\"a\",note(60),synth(\"bell\",{gain_db:-20})),track(\"b\",note(60),{type:\"synth\",name:\"local\",mode:1,cutoff_hz:300})]})",
    );
    let song = muz::compile::lower(v, std::path::Path::new("test.muz"), vec![]);
    assert!(song.is_ok(), "{song:?}");
    let custom = eval("synth(\"reed\",presets={reed:{mode:\"pulse\",cutoff_hz:700}})");
    assert_eq!(
        custom.record().unwrap()["cutoff_hz"].number().unwrap(),
        700.
    );
}
#[test]
fn cosine_and_curve_placement_are_source_recipes() {
    let v = eval("curve_at(lfo(1b,2b),3b)");
    let r = v.record().unwrap();
    let pts = r["points"].array().unwrap();
    assert_eq!(pts.len(), 129);
    assert_eq!(pts[0].array().unwrap()[0].beats().unwrap(), music::b(3));
    assert!((pts[32].array().unwrap()[1].number().unwrap() - 1.).abs() < 1e-10);
    assert_eq!(pts[128].array().unwrap()[0].beats().unwrap(), music::b(5));
}
#[test]
fn tonal_search_preferences_are_editable_source_data() {
    let v = eval(
        "use \"std/tonal\" as t; let harmony=chords(\"C C\"); [voicelead(harmony), t.voicelead(harmony, scoring=merge(t.tonal_scoring,{center:0,candidate_center:0,candidate_spread:0,common_tone:0,parallel:0})), reharmonizations(harmony,phrase(\"C5:q\"),[\"C\",\"Am\"],count=2)]",
    );
    let a = v.array().unwrap();
    let sum = |v: &lang::Value| {
        v.pattern()
            .unwrap()
            .notes
            .iter()
            .map(|n| n.pitch)
            .sum::<f64>()
    };
    assert!(sum(&a[0]) > sum(&a[1]));
    assert_eq!(a[2].array().unwrap().len(), 2);
}
#[test]
fn source_curve_sampling_retains_knots_steps_and_clock_units() {
    let v = eval(
        "let a=curve([[0s,0],[1s,1]],\"step\"); let b=curve([[0s,1],[1s,2]]); [curve_add(a,b,resolution=0.5),curve_mul(b,b,resolution=0.5),curve_map(b,fn(v)=>v*2,resolution=0.5)]",
    );
    let curves = v.array().unwrap();
    let points = |i: usize| curves[i].record().unwrap()["points"].array().unwrap();
    assert_eq!(points(0).len(), 4);
    assert_eq!(points(0)[2].array().unwrap()[0].number().unwrap(), 0.999999);
    assert_eq!(points(0)[3].array().unwrap()[1].number().unwrap(), 3.);
    assert_eq!(points(1)[1].array().unwrap()[1].number().unwrap(), 2.25);
    assert_eq!(points(2)[1].array().unwrap()[1].number().unwrap(), 3.);
    assert!(
        matches!(&points(0)[0].array().unwrap()[0],lang::Value::Num(q) if q.unit==lang::Unit::Seconds)
    );
}
#[test]
fn plugin_aliases_are_user_configuration_and_explicit_options_win() {
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("plugins.json");
    std::fs::write(
        &config,
        r#"{"default":{"path":"instrument.clap","class":"user.class"}}"#,
    )
    .unwrap();
    let source = d.path().join("test.muz");
    std::fs::write(&source,"song({tracks:[track(\"piano\",note(60),piano()),track(\"explicit\",note(60),piano(\"default\",{path:\"other.vst3\",class:\"override\"}))]})").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_muz"))
        .args(["inspect", source.to_str().unwrap(), "--view", "graph"])
        .env("MUZ_PLUGIN_CONFIG", &config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let graph: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let instrument = &graph["tracks"][0]["instrument"];
    assert_eq!(instrument["kind"], "clap");
    assert_eq!(instrument["plugin"]["class_id"], "user.class");
    assert_eq!(
        instrument["plugin"]["bundle_env"],
        d.path().join("instrument.clap").display().to_string()
    );
    let explicit = &graph["tracks"][1]["instrument"];
    assert_eq!(explicit["kind"], "vst3");
    assert_eq!(explicit["plugin"]["class_id"], "override");
}
#[test]
fn unconfigured_plugin_alias_is_named_in_the_error() {
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("plugins.json");
    std::fs::write(&config, "{}").unwrap();
    let source = d.path().join("test.muz");
    std::fs::write(
        &source,
        "song({tracks:[track(\"amp\",note(60),plugin(\"orbitcab\"))]})",
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_muz"))
        .args(["inspect", source.to_str().unwrap(), "--view", "graph"])
        .env("MUZ_PLUGIN_CONFIG", &config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("plugin alias 'orbitcab' is not configured")
            && stderr.contains("plugins.json"),
        "{stderr}"
    );
}
#[test]
fn device_commands_resolve_user_aliases() {
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("plugins.json");
    std::fs::write(&config, r#"{"ghost":{"path":"ghost.clap"}}"#).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_muz"))
        .args(["devices", "inspect", "ghost"])
        .env("MUZ_PLUGIN_CONFIG", &config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&d.path().join("ghost.clap").display().to_string()),
        "an alias argument must resolve to its configured path: {stderr}"
    );
}
#[test]
fn merge_accepts_any_number_of_records_with_shallow_last_wins_updates() {
    let v = eval(
        "let original={x:1,nested:{a:1}}; [merge(),merge(original),merge(original,{x:2,y:2},{x:3,nested:{b:2}},{z:4}),original,merge(overrides={x:2},base={x:1}),merge({x:1},overrides={x:2},{x:3})]",
    );
    assert_eq!(
        v.json(),
        serde_json::json!([
            {},
            {"x":1,"nested":{"a":1}},
            {"x":3,"y":2,"nested":{"b":2},"z":4},
            {"x":1,"nested":{"a":1}},
            {"x":2},
            {"x":3}
        ])
    );
}

#[test]
fn merge_rejects_non_records_and_unrecognized_or_repeated_named_arguments() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("test.muz");
    for source in [
        "merge(1)",
        "merge({},1)",
        "merge({},{},1)",
        "merge({},{},{},1)",
        "merge({},typo={})",
        "merge(base={},base={})",
        "merge({},overrides={},overrides={})",
    ] {
        std::fs::write(&p, source).unwrap();
        assert!(lang::load(&p).is_err(), "accepted {source}");
    }
}

#[test]
fn immutable_source_updates_do_not_change_shared_inputs() {
    let v = eval(
        "let original={values:range(4096),level:1}; let changed=merge(original,{values:map(original.values,fn(x)=>x+1),level:2}); [original,changed]",
    );
    let records = v.array().unwrap();
    let original = records[0].record().unwrap();
    let changed = records[1].record().unwrap();
    assert_eq!(original["level"].number().unwrap(), 1.);
    assert_eq!(changed["level"].number().unwrap(), 2.);
    for (i, (before, after)) in original["values"]
        .array()
        .unwrap()
        .iter()
        .zip(changed["values"].array().unwrap())
        .enumerate()
    {
        assert_eq!(before.number().unwrap(), i as f64);
        assert_eq!(after.number().unwrap(), i as f64 + 1.);
    }
}

#[test]
fn named_synth_modes_validate_overrides_at_the_source_boundary() {
    let inherited = eval("synth(\"bell\")");
    let changed = eval("synth(\"bell\",{mode:\"pulse\"})");
    assert_eq!(inherited.get("mode").unwrap().number().unwrap(), 2.);
    assert_eq!(changed.get("mode").unwrap().number().unwrap(), 1.);
    assert_eq!(
        inherited.get("decay_ms").unwrap().number().unwrap(),
        changed.get("decay_ms").unwrap().number().unwrap()
    );
    let v = eval(
        "song({tracks:[track(\"named\",note(60),synth(\"init\",{mode:\"pulse\",cutoff_hz:700})),track(\"device\",note(60),{type:\"synth\",name:\"direct\",mode:1,cutoff_hz:700})]})",
    );
    let compiled = muz::compile::lower(v, std::path::Path::new("test.muz"), vec![]).unwrap();
    assert_eq!(
        compiled.session.tracks[0].instrument.params,
        compiled.session.tracks[1].instrument.params
    );
    for mode in ["1", "1.0", "\"puls\"", "true"] {
        let mut e = lang::Evaluator::new();
        let error = e
            .source(&format!("synth(\"init\",{{mode:{mode}}})"))
            .unwrap_err();
        assert!(format!("{error:#}").contains("synth mode must be a name"));
    }
}

#[test]
fn long_phrase_rubato_checks_dimensionless_rate_without_exact_time_overflow() {
    let pattern = eval("note(60,48b).rubato(38ms)");
    assert_eq!(pattern.pattern().unwrap().span, music::b(48));
    let mut e = lang::Evaluator::new();
    let error = e.source("note(60,1b).rubato(38ms)").unwrap_err();
    assert!(format!("{error:#}").contains("rubato must preserve forward time"));
}

#[test]
fn percussion_modes_lower_to_source_patches_with_named_decay_controls() {
    for name in ["kick", "snare", "hat", "crash"] {
        let value = eval(&format!("synth(\"{name}\",{{decay_ms:500}})"));
        assert_eq!(value.get("type").unwrap().text().unwrap(), "voice_patch");
    }
}
