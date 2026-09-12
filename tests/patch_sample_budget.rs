use std::path::Path;

fn wav(path: &Path, frames: usize) {
    let mut w = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for _ in 0..frames {
        w.write_sample(0.25f32).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn budget_is_structural_validated_and_not_an_automatable_control() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("case.muz");
    let source = r#"song({tracks:[track("p",note(60),voice_patch("p",{
        sample_budget_frames:16,nodes:[{id:"x",op:"sum",inputs:[0]}],output:"x"}))]})"#;
    std::fs::write(&p, source).unwrap();
    let old = muz::compile::compile(&p).unwrap().session;
    assert!(
        !old.tracks[0]
            .instrument
            .control_values()
            .contains_key("sample_budget_frames")
    );
    std::fs::write(&p, source.replace("frames:16", "frames:32")).unwrap();
    let new = muz::compile::compile(&p).unwrap().session;
    assert!(
        !old.tracks[0]
            .instrument
            .same_structural_identity(&new.tracks[0].instrument)
    );
    let restored: muz::Session =
        serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
    assert_eq!(
        restored.tracks[0].instrument.patch.as_ref().unwrap()["sample_budget_frames"],
        32.0
    );
    for bad in ["0", "-1", "1.5", "134217729", "16ms", "\"16\""] {
        std::fs::write(&p, source.replace("frames:16", &format!("frames:{bad}"))).unwrap();
        let error = format!("{:#}", muz::compile::compile(&p).unwrap_err());
        assert!(error.contains("sample_budget_frames"), "{error}");
    }
    assert!(
        muz::model::patch_sample_budget(&serde_json::json!({"sample_budget_frames":0})).is_err()
    );
    assert_eq!(
        muz::model::patch_sample_budget(&serde_json::json!({})).unwrap(),
        8388608
    );
}

#[test]
fn aggregate_budget_counts_shared_legacy_and_zone_assets_once() {
    let dir = tempfile::tempdir().unwrap();
    wav(&dir.path().join("a.wav"), 16);
    wav(&dir.path().join("b.wav"), 16);
    let path = dir.path().join("case.muz");
    let source = r#"use "std/signal" as s;
      song({tracks:[track("p",note(60,1/100b),voice_patch("p",{
        sample_budget_frames:32,
        output:s.add([{op:"sample",path:"a.wav",root:60},
            s.reader(sample("./a.wav"),"left"),s.reader(sample("b.wav"),"right")])
      }))],tail:0})"#;
    std::fs::write(&path, source).unwrap();
    muz::render::render(
        &path,
        &dir.path().join("ok.wav"),
        None,
        None,
        &[],
        48000,
        64,
    )
    .unwrap();
    std::fs::write(&path, source.replace("frames:32", "frames:31")).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_muz"))
        .args([
            "render",
            path.to_str().unwrap(),
            "-o",
            dir.path().join("bad.wav").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains("require 32 decoded frames (256 bytes)"),
        "{message}"
    );
    assert!(
        message.contains("allows 31 frames (248 bytes)"),
        "{message}"
    );
    assert!(message.contains("sample_budget_frames"), "{message}");
}
