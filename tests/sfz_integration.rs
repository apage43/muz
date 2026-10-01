use muz::{compile, lang};
use std::path::Path;

#[test]
fn native_constructor_is_module_relative_and_rejects_numeric_zone_selection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("instrument.sfz"),
        "<control> set_cc1=64\n<region> sample=*silence key=60",
    )
    .unwrap();
    let main = dir.path().join("main.muz");
    std::fs::write(
        &main,
        "song({tracks:[track(\"x\",note(60,1b),sfz(\"instrument.sfz\",{max_voices:16,seed:7}))]})",
    )
    .unwrap();
    let (value, dependencies) = lang::load(&main).unwrap();
    let result = compile::lower(value, &main, dependencies).unwrap();
    let device = &result.session.tracks[0].instrument;
    assert_eq!(device.kind, muz::model::DeviceKind::Sfz);
    let config = device.sfz.as_ref().unwrap();
    assert_eq!(config.max_voices, 16);
    assert_eq!(config.seed, 7);
    assert_eq!(config.program.regions.len(), 1);
    assert_eq!(device.control_values()["cc1"], 64.);
    assert!(Path::new(&config.path).is_absolute());
    assert_eq!(
        config.max_sample_frames,
        muz::model::DEFAULT_SFZ_SAMPLE_FRAMES
    );
    let mut large = result.session.clone();
    let large_config = large.tracks[0].instrument.sfz.as_mut().unwrap();
    let mut region = large_config.program.regions[0].clone();
    region.opcodes.insert("volume_oncc1".into(), "6".into());
    large_config.program.regions = vec![region; 6798];
    let resources = large.graph_resources();
    assert_eq!(resources.sfz_regions, 6798);
    assert_eq!(resources.sfz_modulation_terms, 6798);
    assert_eq!(resources.sfz_voice_slots, 16);
    assert!(
        large
            .validate_graph_budget()
            .unwrap_err()
            .contains("6798 SFZ regions")
    );
    muz::host::HostContext {
        graph_units: 16000,
        ..Default::default()
    }
    .run(|| large.validate_graph_budget())
    .unwrap();

    let source = format!(
        "song({{tracks:[track(\"x\",note(60,1b).annotate(\"all\",{{sample_zone:0}}),sfz({}))]}})",
        serde_json::to_string(&config.path).unwrap()
    );
    let values = lang::Evaluator::new().source(&source).unwrap();
    let error = compile::lower(values.get("__result").unwrap().clone(), &main, vec![]).unwrap_err();
    assert!(error.to_string().contains("sample_zone"), "{error}");
}

#[test]
fn sfz_case_compatibility_is_exact_first_and_rejects_ambiguous_fallback() {
    use muz::assets::{AssetResolver, FileAssets, MemoryAssets};
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("Samples")).unwrap();
    std::fs::write(dir.path().join("Samples/Horn.wav"), []).unwrap();
    let requested = dir.path().join("samples/horn.wav");
    assert!(FileAssets.resolve(&requested).is_err());
    assert_eq!(
        FileAssets.resolve_case_insensitive(&requested).unwrap(),
        dir.path().join("Samples/Horn.wav")
    );
    std::fs::write(dir.path().join("Samples/HORN.wav"), []).unwrap();
    assert!(FileAssets.resolve_case_insensitive(&requested).is_err());
    let mut memory = MemoryAssets::default();
    memory.insert("/Samples/Horn.wav".into(), Arc::from([]), 1);
    assert_eq!(
        memory
            .resolve_case_insensitive(Path::new("/samples/horn.wav"))
            .unwrap(),
        Path::new("/Samples/Horn.wav")
    );
    memory.insert("/Samples/HORN.wav".into(), Arc::from([]), 1);
    assert!(
        memory
            .resolve_case_insensitive(Path::new("/samples/horn.wav"))
            .is_err()
    );
    assert!(
        memory
            .resolve_case_insensitive(Path::new("/Samples/Horn.wav"))
            .is_ok()
    );
}

#[test]
fn constructor_carries_pinned_overlay_into_normalized_and_saved_state() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let bytes = b"<region> sample=*silence key=60 volume-1\n";
    std::fs::write(dir.path().join("program.sfz"), bytes).unwrap();
    let hash = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let source = format!(
        "let main = sfz({}, {{source_overlays:[{{path:\"program.sfz\",sha256:{},removals:[\"volume-1\"],reason:\"verified malformed token ignored by reference\"}}]}});",
        serde_json::to_string(&dir.path().join("program.sfz").display().to_string()).unwrap(),
        serde_json::to_string(&hash).unwrap()
    );
    let device = muz::device_state::reconstruct(&source, "sfz").unwrap();
    assert_eq!(device.sfz.as_ref().unwrap().source_overlays.len(), 1);
    assert!(
        !device.sfz.as_ref().unwrap().program.regions[0]
            .opcodes
            .contains_key("volume")
    );
    let state = muz::device_state::DeviceState::from_device(
        muz::device_state::DeviceRole::Instrument,
        device,
    )
    .unwrap();
    let restored = muz::device_state::DeviceState::decode(&state.encode().unwrap()).unwrap();
    assert_eq!(
        restored.device.sfz.as_ref().unwrap().source_overlays[0].sha256,
        hash
    );
    restored.host_context().unwrap();
}
