use muz::{assets::MemoryAssets, host::HostContext, lang::Evaluator, snapshot::*};
use std::{path::PathBuf, sync::Arc};
#[test]
fn transfer_rejects_unknown_event_payloads_and_accounts_metadata_bytes() {
    let s = session();
    let snapshot = PlayableSnapshotV1::capture(&s).unwrap();
    let mut value = serde_json::to_value(snapshot).unwrap();
    value["performed"][0]["midi"]["notes"][0]["unsupported_payload"] = serde_json::json!([1, 2, 3]);
    assert!(
        PlayableSnapshotV1::decode_checked(
            &serde_json::to_vec(&value).unwrap(),
            MAX_SNAPSHOT_BYTES
        )
        .is_err()
    );
    let mut s = s;
    let muz::model::TrackSource::Midi(m) = &mut s.tracks[0].source else {
        panic!()
    };
    m.imported.notes[0]
        .annotations
        .insert("large".into(), serde_json::json!("x".repeat(10_000)));
    let mut context = muz::host::HostContext::default();
    context.expansion.bytes = 4096;
    assert!(
        context
            .run(|| validate_events(&s))
            .unwrap_err()
            .to_string()
            .contains("byte limit")
    );
}
fn session() -> muz::Session {
    let v=Evaluator::new().source(r#"song({tempo:123,tracks:[track("a",stack([note(60,1b).express({pan:0.7}),cc(11,42,at=1/2b)]),synth("pad")),track("b",note(65,2b),synth("bell"))],tail:0})"#).unwrap();
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
    let summary = serde_json::to_vec(&SessionSummary::new(&s, 12).unwrap()).unwrap();
    assert!(summary.len() < 2048);
}
#[test]
fn summary_preserves_identity_or_rejects_it_explicitly() {
    let mut s = session();
    let exact = "identity-that-must-not-be-abbreviated-".repeat(8);
    s.tracks[0].id = muz::model::Id::new(&exact);
    let summary = SessionSummary::new(&s, 1).unwrap();
    assert_eq!(summary.tracks[0].id, exact);
    s.tracks[0].id = muz::model::Id::new("x".repeat(1025));
    assert!(SessionSummary::new(&s, 1).is_err());
}
#[test]
fn hostile_event_endpoints_order_limits_and_payloads_are_rejected() {
    let base = PlayableSnapshotV1::capture(&session()).unwrap();
    for kind in 0..8 {
        let mut x = base.clone();
        let midi = &mut x.performed[0].midi;
        match kind {
            0 => midi.notes[0].duration_ticks = 0,
            1 => midi.tempos.push(muz::midi::MidiTempo {
                tick: midi.summary.end_tick + 1,
                micros_per_quarter: 500_000,
                source_order: 0,
            }),
            2 => midi.messages.push(muz::midi::ChannelMessage {
                tick: 0,
                bytes: [0xf0, 0, 0],
                len: 3,
                source_order: 0,
            }),
            3 => midi.messages.push(muz::midi::ChannelMessage {
                tick: 0,
                bytes: [0x90, 128, 0],
                len: 3,
                source_order: 0,
            }),
            4 => midi.summary.controllers += 1,
            5 => midi.notes[0].performance.as_mut().unwrap().pitch = f64::NAN,
            6 => {
                midi.controllers = vec![
                    muz::midi::MidiController {
                        tick: 0,
                        channel: 0,
                        controller: 1,
                        value: 1,
                        source_order: 2,
                    },
                    muz::midi::MidiController {
                        tick: 0,
                        channel: 0,
                        controller: 1,
                        value: 2,
                        source_order: 1,
                    },
                ];
                midi.summary.controllers = 2;
            }
            _ => midi.tempos.push(muz::midi::MidiTempo {
                tick: 0,
                micros_per_quarter: 0,
                source_order: 0,
            }),
        }
        assert!(x.restore_description().is_err(), "case {kind}");
    }

    let mut json = serde_json::to_value(base).unwrap();
    json["performed"][0]["unsupported"] = serde_json::json!(true);
    assert!(
        PlayableSnapshotV1::decode_checked(&serde_json::to_vec(&json).unwrap(), MAX_SNAPSHOT_BYTES)
            .is_err()
    );
}

#[test]
fn memory_asset_manifest_is_checked_during_decode_and_restore() {
    let path = PathBuf::from("/virtual/selected.wav");
    let bytes: Arc<[u8]> = Arc::from([0_u8; 8]);
    let mut assets = MemoryAssets::default();
    assets.insert(path.clone(), bytes, 7);
    let context = HostContext {
        assets: Arc::new(assets),
        ..Default::default()
    };
    context.run(|| {
        let mut s = session();
        s.tracks[0].instrument.sample = Some(vec![muz::model::SampleZone {
            path: path.to_string_lossy().into_owned(),
            gain_db: 0.,
            root: 60.,
            keys: [0, 127],
            velocity: [0., 1.],
            offset_seconds: 0.,
            loop_seconds: None,
            one_shot: false,
        }]);
        s.tracks[0].instrument.asset_versions = vec![(8, 7)];
        let snapshot = PlayableSnapshotV1::capture(&s).unwrap();
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        PlayableSnapshotV1::decode_checked(&encoded, MAX_SNAPSHOT_BYTES)
            .unwrap()
            .restore_checked()
            .unwrap();

        let mut missing = snapshot.clone();
        missing.assets.clear();
        assert!(
            PlayableSnapshotV1::decode_checked(
                &serde_json::to_vec(&missing).unwrap(),
                MAX_SNAPSHOT_BYTES
            )
            .is_err()
        );
        let mut wrong = snapshot;
        wrong.assets[0].modified_nanos = 8;
        assert!(
            PlayableSnapshotV1::decode_checked(
                &serde_json::to_vec(&wrong).unwrap(),
                MAX_SNAPSHOT_BYTES
            )
            .is_err()
        );
    });
}
#[test]
fn actual_worker_renders_the_shared_snapshot() {
    let d = tempfile::tempdir().unwrap();
    let sample = d.path().join("selected.wav");
    {
        let mut writer = hound::WavWriter::create(
            &sample,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..64 {
            writer.write_sample(4096_i16).unwrap();
        }
        writer.finalize().unwrap();
    }
    let source = format!(
        r#"song({{tempo:123,tracks:[track("a",stack([note(60,1b).express({{pan:0.7}}),cc(11,42,at=1/2b)]),sample([{{path:{},keys:[60,60],velocity:[0,1]}}],{{attack_ms:0,release_ms:0}}))],tail:0}})"#,
        serde_json::to_string(&sample.to_string_lossy()).unwrap()
    );
    let value = Evaluator::new().source(&source).unwrap();
    let s = muz::compile::lower(
        value.get("__result").unwrap().clone(),
        &d.path().join("worker.muz"),
        vec![],
    )
    .unwrap()
    .session;
    let muz::model::TrackSource::Midi(midi) = &s.tracks[0].source else {
        panic!()
    };
    assert_eq!(midi.imported.controllers.len(), 1);
    assert_eq!(midi.imported.tempos.len(), 1);
    assert!(
        !midi.imported.notes[0]
            .performance
            .unwrap()
            .expression
            .is_empty()
    );
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
