use muz::{description::*, patch_description::ValidatedPatch};
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
