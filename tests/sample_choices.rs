//! Recording identity through lane edits, using generated constant samples.
fn setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, level) in [("low", 0.1f32), ("high", 0.4)] {
        let mut wav = hound::WavWriter::create(
            dir.path().join(format!("{name}.wav")),
            hound::WavSpec {
                channels: 1,
                sample_rate: 8000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..8000 {
            wav.write_sample(level).unwrap();
        }
        wav.finalize().unwrap();
    }
    dir
}
const HEADER: &str = r#"
use "std/sampler" as sampler;
let zones = [{path:"low.wav",root:60,keys:[60,60]},
    {path:"low.wav",root:40,keys:[40,40]},
    {path:"high.wav",root:40,keys:[40,40]}];
let instrument = sample(zones,{attack_ms:0,release_ms:0});
"#;
fn render(dir: &std::path::Path, body: &str, block: usize) -> Vec<f32> {
    let source = dir.join("case.muz");
    std::fs::write(&source, format!("{HEADER}\n{body}")).unwrap();
    let output = dir.join("case.wav");
    muz::render::render(&source, &output, None, None, &[], 8000, block).unwrap();
    hound::WavReader::open(output)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}
#[test]
fn pinned_recordings_survive_removal_and_same_group_lane_splits() {
    let dir = setup();
    for opening in ["C4", "E2"] {
        let original = format!(
            r#"let p = phrase("{opening}:q E2:q");
            song({{tempo:120,tracks:[track("one",p,instrument)],tail:0}})"#
        );
        let reference = render(dir.path(), &original, 97);
        let pinned = format!(
            r#"let p = sampler.pin_recordings(phrase("{opening}:q E2:q"),zones);
            song({{tempo:120,tracks:[track("one",p,instrument)],tail:0}})"#
        );
        assert_eq!(reference, render(dir.path(), &pinned, 256));
        let split = format!(
            r#"let p = sampler.pin_recordings(phrase("{opening}:q E2:q"),zones);
            song({{tempo:120,tracks:[track("one",p.reject("first"),instrument),
                track("two",p.select("first"),instrument)],tail:0}})"#
        );
        assert_eq!(reference, render(dir.path(), &split, 97));
        let removed = format!(
            r#"let p = sampler.pin_recordings(phrase("{opening}:q E2:q"),zones);
            song({{tempo:120,tracks:[track("one",p.reject("first"),instrument)],tail:0}})"#
        );
        assert_eq!(
            &reference[8000..],
            &render(dir.path(), &removed, 256)[8000..]
        );
        let unpinned = render(
            dir.path(),
            r#"song({tempo:120,tracks:[track("one",phrase("r:q E2:q"),instrument)],tail:0})"#,
            97,
        );
        assert_ne!(&reference[8000..], &unpinned[8000..]);
    }
}
#[test]
fn explicit_selection_is_validated_and_serialized() {
    let dir = setup();
    let source = dir.path().join("case.muz");
    for choice in ["-1", "3", "1.5", "0", "\"high\"", "null"] {
        std::fs::write(&source, format!(r#"{HEADER}
            song({{tracks:[track("one",phrase("E2:q").annotate("all",{{sample_zone:{choice}}}),instrument)]}})"#)).unwrap();
        let error = muz::compile::compile(&source).unwrap_err().to_string();
        assert!(error.contains("sample_zone"), "{error}");
    }
    std::fs::write(&source, format!(r#"{HEADER}
        song({{tracks:[track("one",phrase("E2:q").annotate("all",{{sample_zone:2}}),instrument)]}})"#)).unwrap();
    let session = muz::compile::compile(&source).unwrap().session;
    let mut restored = session.clone();
    if let muz::model::TrackSource::Midi(ref mut source) = restored.tracks[0].source {
        let json = serde_json::to_string(&source.imported).unwrap();
        source.imported = serde_json::from_str(&json).unwrap();
        assert_eq!(
            source.imported.notes[0].annotations["sample_zone"],
            serde_json::json!(2)
        );
        source.imported.notes[0]
            .annotations
            .insert("sample_zone".into(), serde_json::json!(99));
    } else {
        panic!("expected MIDI source");
    }
    assert!(
        restored
            .validate_sample_coverage()
            .unwrap_err()
            .contains("sample_zone")
    );
}

#[test]
fn overriding_an_attack_preserves_legacy_counter_for_later_notes() {
    let dir = setup();
    let reference = render(
        dir.path(),
        r#"song({tempo:120,tracks:[track("one",phrase("E2:q E2:q"),instrument)],tail:0})"#,
        97,
    );
    let changed = render(
        dir.path(),
        r#"song({tempo:120,tracks:[track("one",phrase("E2:q E2:q").annotate("first",{sample_zone:2}),instrument)],tail:0})"#,
        256,
    );
    assert_ne!(&reference[..8000], &changed[..8000]);
    assert_eq!(&reference[8000..], &changed[8000..]);
}

#[test]
fn explicit_zone_rejects_other_instruments_and_wrong_velocity_layers() {
    let dir = setup();
    let source = dir.path().join("case.muz");
    for instrument in [
        r#"synth("bell")"#,
        r#"sample([{path:"low.wav",velocity:[0,0.5]},{path:"high.wav",velocity:[0.5,1]}])"#,
    ] {
        std::fs::write(&source, format!(r#"song({{tracks:[track("one",phrase("E2:q").annotate("all",{{sample_zone:0}}),{instrument})]}})"#)).unwrap();
        let error = muz::compile::compile(&source).unwrap_err().to_string();
        assert!(error.contains("sample_zone"), "{error}");
    }
}
