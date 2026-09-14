use muz::{description::*, patch_description::ValidatedPatch};
#[test]
fn checked_session_prepares_source_and_wire_equivalently() {
    let values = muz::lang::Evaluator::new()
        .source(r#"song({tracks:[track("x",note(60,1b),voice_patch("p",{nodes:[{id:"a",op:"param",value:0.1}],output:"a"}))]})"#)
        .unwrap();
    let session = muz::compile::lower(
        values.get("__result").unwrap().clone(),
        std::path::Path::new("checked.muz"),
        vec![],
    )
    .unwrap()
    .session;
    let restored = muz::snapshot::PlayableSnapshotV1::capture(&session).unwrap();
    let wire = serde_json::to_vec(&restored).unwrap();
    let restored = muz::snapshot::PlayableSnapshotV1::decode_checked(&wire, wire.len())
        .unwrap()
        .restore_description()
        .unwrap();
    let config = muz::audio::AudioConfig {
        sample_rate: 48000.,
        max_frames: 256,
        offline: true,
    };
    let checked = ValidatedSession::new(&session).unwrap();
    let mut a = muz::audio::AudioEngine::from_validated(&checked, config).unwrap();
    let mut b = muz::audio::AudioEngine::new(&restored, config).unwrap();
    a.set_running(true);
    b.set_running(true);
    let mut left = [0.; 512];
    let mut right = [0.; 512];
    a.render_interleaved(&mut left, 2).unwrap();
    b.render_interleaved(&mut right, 2).unwrap();
    assert_eq!(left, right);
    let mut invalid = session.clone();
    invalid.tracks[0].instrument.sample = Some(vec![]);
    assert!(ValidatedSession::new(&invalid).is_err());
    let context = muz::host::HostContext {
        graph_units: 128,
        ..Default::default()
    };
    let checked = context.run(|| ValidatedSession::new(&session).unwrap());
    let other = muz::host::HostContext {
        graph_units: 1,
        ..Default::default()
    };
    let engine = other.run(|| muz::audio::AudioEngine::from_validated(&checked, config).unwrap());
    assert_eq!(engine.graph_budget(), 128);
    context
        .cancelled
        .store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(muz::audio::AudioEngine::from_validated(&checked, config).is_err());
}
#[test]
fn typed_patch_rejects_edges_types_and_ranges_before_preparation() {
    let good = serde_json::json!({"nodes":[{"id":"a","op":"noise"},{"id":"b","op":"sum","inputs":["a","a"]}],"output":"b"});
    let p = ValidatedPatch::from_json(&good).unwrap();
    assert_eq!(p.nodes().len(), 2);
    for graph in [
        serde_json::json!({"nodes":[{"id":"a","op":"sum","inputs":["future"]}],"output":"a"}),
        serde_json::json!({"nodes":[{"id":"a","op":"noise","typo":3}],"output":"a"}),
        serde_json::json!({"nodes":[{"id":"a","op":"param","value":2,"min":0,"max":1}],"output":"a"}),
        serde_json::json!({"nodes":[{"id":"a","op":"osc","hz":true}],"output":"a"}),
    ] {
        assert!(ValidatedPatch::from_json(&graph).is_err());
    }
}

#[test]
fn checked_patch_resolves_edges_and_separates_controls() {
    use muz::patch_description::{Operation, Signal};
    let graph = serde_json::json!({"nodes":[{"id":"a","op":"param","value":0.1},
        {"id":"b","op":"sum","inputs":["a","a"]}],"output":{"left":"b","right":"a"}});
    let a = ValidatedPatch::from_json(&graph).unwrap();
    let Operation::Sum { inputs } = &a.nodes()[1].operation else {
        panic!()
    };
    assert_eq!(inputs, &[Signal::Resolved(0), Signal::Resolved(0)]);
    let mut changed = graph.clone();
    changed["nodes"][0]["value"] = serde_json::json!(0.8);
    changed["gain_db"] = serde_json::json!(-6.);
    let b = ValidatedPatch::from_json(&changed).unwrap();
    assert!(a.same_structure(&b));
    assert_ne!(a.controls(), b.controls());
    changed["nodes"][1]["inputs"][1] = serde_json::json!(0.);
    assert!(!a.same_structure(&ValidatedPatch::from_json(&changed).unwrap()));
    for bad in [
        serde_json::json!({"nodes":[{"id":"a","op":"noise"}],"output":{"left":"a","right":"a","extra":0}}),
        serde_json::json!({"nodes":[{"id":"a","op":"adsr"}],"output":"a","lifetime":{"envelope":"a","tail":"oops"}}),
        serde_json::json!({"nodes":[{"id":"a","op":"shape","points":[[0,0],[1e-14,1]]}],"output":"a"}),
        serde_json::json!({"nodes":[{"id":"a","op":"noise"}],"output":"a","voice_mode":false}),
    ] {
        assert!(ValidatedPatch::from_json(&bad).is_err(), "{bad}");
    }
}
#[test]
fn tagged_adapter_rejects_incompatible_device_payloads() {
    let mut d = muz::model::Device {
        id: muz::model::Id::new("d"),
        kind: muz::model::DeviceKind::Gain,
        params: Default::default(),
        patch: None,
        rack: None,
        sample: None,
        vst3: None,
        generation: 0,
        sidechain: None,
        asset_versions: vec![],
    };
    assert!(validate_device(&d).is_ok());
    d.patch = Some(serde_json::json!({"nodes":[]}));
    assert!(validate_device(&d).is_err());
}
#[test]
fn static_descriptors_match_native_setter_boundaries() {
    use muz::audio::*;
    for kind in [
        muz::model::DeviceKind::PolySynth,
        muz::model::DeviceKind::Gain,
    ] {
        let d = muz::model::Device {
            id: muz::model::Id::new("d"),
            kind,
            params: Default::default(),
            patch: None,
            rack: None,
            sample: None,
            vst3: None,
            generation: 0,
            sidechain: None,
            asset_versions: vec![],
        };
        let mut p = create_processor(
            &d,
            AudioConfig {
                sample_rate: 48000.,
                max_frames: 256,
                offline: true,
            },
        )
        .unwrap();
        for s in parameter_specs(kind) {
            for value in [s.min, s.default, s.max] {
                assert!(validate_control(&d, s.name, value).is_ok());
                assert!(p.set_parameter(s.name, value).is_ok());
            }
            assert!(validate_control(&d, s.name, f32::NAN).is_err());
        }
    }
}
