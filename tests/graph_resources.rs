//! Resource preparation is tested with synthetic lanes, never a composition.
fn compiled(tracks: usize) -> muz::Session {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph.muz");
    let lanes = (0..tracks).map(|i| format!(r#"track("lane{i}",note(60,1b),synth("init"),{{chain:[fx("gain"),fx("gain"),fx("gain")]}})"#)).collect::<Vec<_>>().join(",");
    std::fs::write(&path, format!("song({{tracks:[{lanes}]}})")).unwrap();
    muz::compile::compile(&path).unwrap().session
}

#[test]
fn expanded_graph_above_old_caps_prepares_and_processes_every_device() {
    let session = compiled(42);
    let r = session.graph_resources();
    assert_eq!(r.tracks, 42);
    assert_eq!(r.devices, 168);
    assert_eq!(r.routes, 42);
    assert_eq!(r.units, 211);
    let mut engine = muz::audio::AudioEngine::new(
        &session,
        muz::audio::AudioConfig {
            sample_rate: 48000.,
            max_frames: 256,
        },
    )
    .unwrap();
    engine.set_running(true);
    let mut audio = [0.; 512];
    engine.render_interleaved(&mut audio, 2).unwrap();
    let states = engine.device_debug_states();
    assert_eq!(states.len(), 168);
    assert!(states.iter().all(|(_, s)| s.process_count > 0));
}

#[test]
fn resource_failure_reports_total_budget_and_contributors_before_device_preparation() {
    let mut session = compiled(1);
    let template = session.tracks[0].clone();
    session.tracks = (0..900)
        .map(|i| {
            let mut track = template.clone();
            track.id = muz::model::Id::new(format!("expanded{i}"));
            track
        })
        .collect();
    let error = session.validate_graph_budget().unwrap_err();
    assert!(
        error.contains("4501 resource units; allowed 4096"),
        "{error}"
    );
    assert!(error.contains("900 tracks, 3600 devices"), "{error}");
    assert!(error.contains("track expanded0: 5"), "{error}");
}

#[test]
fn check_honors_explicit_process_budget_and_rejects_invalid_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("budget.muz");
    std::fs::write(
        &path,
        "song({tracks:[track(\"one\",note(60),synth(\"init\"))]})",
    )
    .unwrap();
    for (budget, succeeds, message) in [
        ("3", true, ""),
        ("2", false, "requires 3 resource units; allowed 2"),
        ("nonsense", false, "must be an integer"),
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_muz"))
            .env("MUZ_GRAPH_BUDGET", budget)
            .arg("check")
            .arg(&path)
            .arg("--json")
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if succeeds {
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["graph"]["units"], 3);
            assert_eq!(value["graph_budget"], 3);
        } else {
            assert!(String::from_utf8_lossy(&output.stderr).contains(message));
        }
    }
}
