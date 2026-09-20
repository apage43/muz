use muz::reconcile::{ReconcileOperation, plan_reconciliation};
#[test]
fn patch_control_edits_are_updates_including_serialized_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.muz");
    let source = r#"song({tracks:[track("a",note("C4",4b),voice_patch("p",{gain_db:-12,nodes:[{id:"tone",op:"param",value:440,min:20,max:20000},{id:"o",op:"osc",hz:"tone"}],output:"o"}))]})"#;
    std::fs::write(&path, source).unwrap();
    let old = muz::compile::compile(&path).unwrap().session;
    for change in [
        source.replace("value:440", "value:880"),
        source.replace("gain_db:-12", "gain_db:-6"),
        source.replace("gain_db:-12", "gain_db:-12,tone:660"),
    ] {
        std::fs::write(&path, change).unwrap();
        let new = muz::compile::compile(&path).unwrap().session;
        let mut restored: muz::Session =
            serde_json::from_value(serde_json::to_value(&new).unwrap()).unwrap();
        // This test isolates instrument controls from compiler score provenance.
        restored.tracks[0].source = old.tracks[0].source.clone();
        let plan = plan_reconciliation(0, &old, &restored).unwrap();
        muz::audio::PreparedValueTransaction::prepare(
            &old,
            &restored,
            &plan,
            0,
            std::time::Instant::now(),
        )
        .unwrap();
        assert!(
            plan.operations
                .iter()
                .any(|o| matches!(o, ReconcileOperation::SetParameters { .. }))
        );
        assert!(
            !plan
                .operations
                .iter()
                .any(|o| matches!(o, ReconcileOperation::Replace { .. }))
        );
    }
    std::fs::write(&path, source.replace("max:20000", "max:10000")).unwrap();
    let new = muz::compile::compile(&path).unwrap().session;
    assert!(
        plan_reconciliation(0, &old, &new)
            .unwrap()
            .operations
            .iter()
            .any(|o| matches!(o, ReconcileOperation::Replace { .. }))
    );
}

#[test]
fn changing_inline_default_during_live_playback_preserves_processor_and_updates_sound() {
    use muz::audio::{AudioConfig, AudioEngine, PreparedTransaction};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.muz");
    let source = r#"song({tracks:[track("p",note(60,4b,velocity=1),voice_patch("p",{gain_db:0,nodes:[{id:"level",op:"param",value:0.2,min:0,max:1}],output:{left:"level",right:"level"}}))]})"#;
    std::fs::write(&path, source).unwrap();
    let old = muz::compile::compile(&path).unwrap().session;
    std::fs::write(&path, source.replace("value:0.2", "value:0.4")).unwrap();
    let new = muz::compile::compile(&path).unwrap().session;
    let config = AudioConfig {
        sample_rate: 48000.,
        max_frames: 256,
        offline: false,
    };
    let mut engine = AudioEngine::new(&old, config).unwrap();
    engine.set_running(true);
    let mut output = [0.; 512];
    for _ in 0..4 {
        engine.render_interleaved(&mut output, 2).unwrap();
    }
    let token = engine.device_debug_states()[0].1.instance_token;
    let plan = plan_reconciliation(0, &old, &new).unwrap();
    let mut transaction =
        PreparedTransaction::prepare(&old, &new, &plan, 0, std::time::Instant::now(), config)
            .unwrap();
    engine.apply_transaction(&mut transaction).unwrap();
    for _ in 0..4 {
        engine.render_interleaved(&mut output, 2).unwrap();
    }
    assert_eq!(engine.device_debug_states()[0].1.instance_token, token);
    assert!((output[500] - 0.4).abs() < 1e-5, "{}", output[500]);
}

fn compile_note_controls(source: &str) -> anyhow::Result<muz::Session> {
    let value = muz::lang::Evaluator::new().source(source)?;
    Ok(muz::compile::lower(
        value.get("__result").unwrap().clone(),
        std::path::Path::new("controls.muz"),
        vec![],
    )?
    .session)
}

#[test]
fn named_note_controls_bind_sorted_slots_and_validate_destination() {
    let source = r#"use "std/signal" as s;
    song({tracks:[track("lead",note(60,1b).express({mute:0.7,pick:0.2}),
        voice_patch("pluck",{note_controls:{pick:0.5,mute:0},output:s.expression("mute")}))]})"#;
    let session = compile_note_controls(source).unwrap();
    let muz::model::TrackSource::Midi(m) = &session.tracks[0].source else {
        panic!()
    };
    let program = m.imported.notes[0].performance.unwrap().expression;
    assert_eq!(program.value(7, 0.), Some(0.7));
    assert_eq!(program.value(8, 0.), Some(0.2));
    let reordered =
        compile_note_controls(&source.replace("pick:0.5,mute:0", "mute:0,pick:0.5")).unwrap();
    let a = muz::patch_description::ValidatedPatch::from_json(
        session.tracks[0].instrument.patch.as_ref().unwrap(),
    )
    .unwrap();
    let b = muz::patch_description::ValidatedPatch::from_json(
        reordered.tracks[0].instrument.patch.as_ref().unwrap(),
    )
    .unwrap();
    assert!(a.same_structure(&b));
    for bad in [
        source.replace("pick:0.5,mute:0", "pick:0.5"),
        source.replace("s.expression(\"mute\")", "s.expression(\"vowel\")"),
        source.replace("mute:0.7", "vowel:0.7"),
        source.replace("pick:0.5,mute:0", "pressure:0,mute:0"),
        source.replace("pick:0.5,mute:0", "pick:2,mute:0"),
        source.replace("pick:0.5,mute:0", "pick:1ms,mute:0"),
        source.replace("mute:0.7", "mute:[[0,0],[0,1]]"),
        source.replace("mute:0.7", "mute:1.1"),
    ] {
        assert!(compile_note_controls(&bad).is_err(), "accepted {bad}");
    }
    let undeclared = source.replace("mute:0.7", "vowel:0.7");
    let error = format!("{:#}", compile_note_controls(&undeclared).unwrap_err());
    assert!(
        error.contains("lead") && error.contains("pluck") && error.contains("vowel"),
        "{error}"
    );
    let bypass = source.replace(
        ".express({mute:0.7,pick:0.2})",
        ".map_notes(fn(n)=>{data:{expression:{vowel:0.5}}})",
    );
    assert!(compile_note_controls(&bypass).is_err());
    let standard =
        r#"song({tracks:[track("lead",note(60,1b).express({mute:0.5}),synth("init"))]})"#;
    assert!(compile_note_controls(standard).is_err());
}

#[test]
fn custom_note_schema_changes_replace_instrument() {
    let source = r#"song({tracks:[track("a",note(60,1b),voice_patch("p",{
        note_controls:{mute:0},nodes:[{id:"o",op:"expression",kind:"mute"}],output:"o"}))]})"#;
    let old = compile_note_controls(source).unwrap();
    for changed in [
        source.replace("mute:0", "mute:0.5"),
        source.replace("mute", "open"),
    ] {
        let mut new = compile_note_controls(&changed).unwrap();
        new.tracks[0].source = old.tracks[0].source.clone();
        let plan = plan_reconciliation(0, &old, &new).unwrap();
        assert!(
            plan.operations
                .iter()
                .any(|op| matches!(op, ReconcileOperation::Replace { .. }))
        );
        let config = muz::audio::AudioConfig {
            sample_rate: 48000.,
            max_frames: 256,
            offline: false,
        };
        let mut engine = muz::audio::AudioEngine::new(&old, config).unwrap();
        engine.set_running(true);
        let mut output = [0.; 512];
        engine.render_interleaved(&mut output, 2).unwrap();
        let token = engine.device_debug_states()[0].1.instance_token;
        let mut transaction = muz::audio::PreparedTransaction::prepare(
            &old,
            &new,
            &plan,
            0,
            std::time::Instant::now(),
            config,
        )
        .unwrap();
        engine.apply_transaction(&mut transaction).unwrap();
        engine.render_interleaved(&mut output, 2).unwrap();
        assert_ne!(engine.device_debug_states()[0].1.instance_token, token);
    }
}
