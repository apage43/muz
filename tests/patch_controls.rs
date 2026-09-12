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
