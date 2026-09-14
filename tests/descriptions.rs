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
