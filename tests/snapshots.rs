use muz::{lang::Evaluator, snapshot::*};
fn session() -> muz::Session {
    let v=Evaluator::new().source(r#"song({tracks:[track("a",note(60,1b).express({pan:0.7}),synth("pad")),track("b",note(65,2b),synth("bell"))],tail:0})"#).unwrap();
    muz::compile::lower(
        v.get("__result").unwrap().clone(),
        std::path::Path::new("memory.muz"),
        vec![],
    )
    .unwrap()
    .session
}
#[test]
fn transfer_associates_by_id_and_rejects_malformed_bundles() {
    let s = session();
    let mut snapshot = PlayableSnapshotV1::capture(&s).unwrap();
    snapshot.description.tracks.reverse();
    let bytes = serde_json::to_vec(&snapshot).unwrap();
    let restored = PlayableSnapshotV1::decode_checked(&bytes, MAX_SNAPSHOT_BYTES)
        .unwrap()
        .restore_checked()
        .unwrap();
    assert_eq!(restored.tracks[0], s.tracks[1]);
    assert!(PlayableSnapshotV1::decode_checked(&bytes, 2).is_err());
    for kind in 0..7 {
        let mut x = snapshot.clone();
        match kind {
            0 => x.version = 2,
            1 => {
                x.performed.pop();
            }
            2 => x.performed.push(x.performed[0].clone()),
            3 => x.performed[0].track_id = muz::model::Id::new("unknown"),
            4 => x.performed[0].source_id = muz::model::Id::new("wrong"),
            5 => x.performed[0].midi.summary.ppq = 0,
            _ => {
                x.performed[0].midi.notes[0].start_tick = 1;
                x.performed[0].midi.notes[0].duration_ticks = u64::MAX;
            }
        }
        assert!(x.restore_checked().is_err(), "case {kind}");
    }
}
#[test]
fn direct_inputs_cannot_panic_fixed_buffer_consumers() {
    let mut s = session();
    let muz::model::TrackSource::Midi(m) = &mut s.tracks[0].source else {
        panic!()
    };
    m.imported.notes[0]
        .performance
        .as_mut()
        .unwrap()
        .expression
        .len = 255;
    assert!(validate_events(&s).is_err());
    assert!(
        muz::audio::AudioEngine::new(
            &s,
            muz::audio::AudioConfig {
                sample_rate: 48000.,
                max_frames: 256,
                offline: true
            }
        )
        .is_err()
    );
}
#[test]
fn summary_is_bounded_and_does_not_copy_event_metadata() {
    let mut s = session();
    s.extras.title = "x".repeat(2_000_000);
    let muz::model::TrackSource::Midi(m) = &mut s.tracks[0].source else {
        panic!()
    };
    m.imported.notes[0]
        .annotations
        .insert("large".into(), serde_json::json!("z".repeat(2_000_000)));
    let summary = serde_json::to_vec(&SessionSummary::new(&s, 12)).unwrap();
    assert!(summary.len() < 2048);
}
#[test]
fn actual_worker_renders_the_shared_snapshot() {
    let d = tempfile::tempdir().unwrap();
    let s = session();
    let out = d.path().join("worker.wav");
    let input = d.path().join("input.json");
    let options = muz::render::RenderOptions::default();
    std::fs::write(&input,serde_json::to_vec(&serde_json::json!({"snapshot":PlayableSnapshotV1::capture(&s).unwrap(),"options":options,"output":out})).unwrap()).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_muz"))
        .arg("render-worker")
        .arg(&input)
        .arg(d.path().join("report.json"))
        .arg(d.path().join("progress.json"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let direct = d.path().join("direct.wav");
    muz::render::render_with(s, &direct, &options, None).unwrap();
    assert_eq!(std::fs::read(out).unwrap(), std::fs::read(direct).unwrap());
}
