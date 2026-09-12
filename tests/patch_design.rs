use std::path::Path;
fn bounce(dir: &Path, instrument: &str, notes: &str) -> Vec<f32> {
    let path = dir.join("case.muz");
    let out = dir.join("case.wav");
    std::fs::write(
        &path,
        format!("song({{tempo:120,tracks:[track(\"p\",{notes},{instrument})],tail:0.2}})"),
    )
    .unwrap();
    muz::render::render(&path, &out, None, None, &[], 48000, 97).unwrap();
    hound::WavReader::open(out)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}
#[test]
fn explicit_completion_preserves_delayed_excitation_and_ignores_other_envelopes() {
    let dir = tempfile::tempdir().unwrap();
    let patch = r#"voice_patch("p",{gain_db:0,nodes:[
      {id:"e",op:"adsr",attack:0,decay:0.01,sustain:0,one_shot:true},
      {id:"d",op:"delay",input:"e",seconds:0.03,max_seconds:0.05},
      {id:"mod",op:"adsr",sustain:1}
    ],output:"d",lifetime:{envelope:"e",tail:0.1}})"#;
    let x = bounce(dir.path(), patch, "note(60,1b,velocity=1)");
    assert!(x[1440 * 2..1800 * 2].iter().any(|x| x.abs() > 0.1));
    assert!(x[7000 * 2..].iter().all(|x| x.abs() < 1e-6));
    let slow = patch.replace("attack:0,decay:0.01", "attack:0.05,decay:0.01");
    let x = bounce(dir.path(), &slow, "note(60,1b,velocity=1)");
    assert!(x[3000 * 2..4000 * 2].iter().any(|x| x.abs() > 0.1));
}
#[test]
fn stereo_output_preserves_center_and_has_documented_balance() {
    let dir = tempfile::tempdir().unwrap();
    let patch = r#"voice_patch("p",{gain_db:0,nodes:[{id:"x",op:"sum",inputs:[0.2]}],output:{left:"x",right:0.4}})"#;
    let center = bounce(dir.path(), patch, "note(60,1/4b,velocity=1)");
    assert!((center[2000] - 0.2).abs() < 1e-6);
    assert!((center[2001] - 0.4).abs() < 1e-6);
    let left = bounce(
        dir.path(),
        patch,
        "note(60,1/4b,velocity=1).express({pan:0})",
    );
    assert!((left[2000] - 0.2 * 2f32.sqrt()).abs() < 1e-6);
    assert_eq!(left[2001], 0.);
}
#[test]
fn stereo_sample_readers_preserve_recorded_channels() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = hound::WavWriter::create(
        dir.path().join("stereo.wav"),
        hound::WavSpec {
            channels: 2,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for _ in 0..10000 {
        w.write_sample(0.2f32).unwrap();
        w.write_sample(-0.3f32).unwrap();
    }
    w.finalize().unwrap();
    let patch = r#"voice_patch("p",{gain_db:0,nodes:[{id:"l",op:"sample",path:"stereo.wav",channel:"left"},{id:"r",op:"sample",path:"stereo.wav",channel:"right"}],output:{left:"l",right:"r"}})"#;
    let x = bounce(dir.path(), patch, "note(60,1/4b,velocity=1)");
    assert!((x[2000] - 0.2).abs() < 1e-6 && (x[2001] + 0.3).abs() < 1e-6);
}

#[test]
fn nested_graphs_share_by_binding_not_by_equal_contents_and_check_units() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.muz");
    let source = r#"use "std/signal" as s;
      let e=s.adsr(); let a=s.sine();
      let b=s.sine();
      song({tracks:[track("p",note(60,1b),voice_patch("p",{output:s.stereo(s.mul([a,e]),s.mul([b,e])),lifetime:{envelope:e,tail:100ms}}))]})"#;
    std::fs::write(&path, source).unwrap();
    let session = muz::compile::compile(&path).unwrap().session;
    let graph = session.tracks[0].instrument.patch.as_ref().unwrap();
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 5);
    let bad = source.replace("s.adsr()", "s.adsr(400Hz)");
    std::fs::write(&path, bad).unwrap();
    let error = format!("{:#}", muz::compile::compile(&path).unwrap_err());
    assert!(error.contains("incompatible units"), "{error}");
}

#[test]
fn segment_envelope_releases_from_current_level_and_drives_completion() {
    let dir = tempfile::tempdir().unwrap();
    let patch = r#"use "std/signal" as s;"#;
    let _ = patch;
    let instrument = r#"voice_patch("p",{gain_db:0,nodes:[{id:"e",op:"mseg",attack:[{time:0.1,to:1}],sustain:0,release:[{time:0.05,to:0}]}],output:{left:"e",right:"e"},lifetime:{envelope:"e",tail:0}})"#;
    let x = bounce(dir.path(), instrument, "note(60,1/10b,velocity=1).gate(1)");
    assert!((x[2399 * 2] - 0.5).abs() < 0.002);
    assert!((x[3600 * 2] - 0.25).abs() < 0.002);
    assert!(x[4900 * 2..].iter().all(|x| x.abs() < 1e-6));
}
#[test]
fn mappings_and_hold_have_explicit_behavior() {
    let dir = tempfile::tempdir().unwrap();
    let instrument = r#"voice_patch("p",{gain_db:0,nodes:[{id:"exp",op:"map",kind:"exp2",input:3},{id:"r",op:"map",kind:"reciprocal",input:"exp"},{id:"e",op:"adsr",attack:0.1,sustain:1},{id:"h",op:"hold",input:"e",rate_hz:100}],output:{left:"r",right:"h"}})"#;
    let x = bounce(dir.path(), instrument, "note(60,1b,velocity=1)");
    assert!((x[600 * 2] - 0.125).abs() < 1e-6);
    assert!((x[600 * 2 + 1] - 0.1).abs() < 0.001);
    assert_eq!(x[600 * 2 + 1], x[900 * 2 + 1]);
}

#[test]
fn legato_transfers_note_ownership_preserves_attack_and_glides() {
    let dir = tempfile::tempdir().unwrap();
    let patch = r#"voice_patch("p",{gain_db:0,voice_mode:"legato",glide_ms:50,nodes:[{id:"e",op:"adsr",attack:0.1,sustain:1},{id:"f",op:"frequency"},{id:"pitch",op:"mul",inputs:["f",0.001]}],output:{left:"e",right:"pitch"}})"#;
    let notes =
        r#"stack([note(69,1/10b,velocity=1).gate(1),note(81,3/10b,at=1/20b,velocity=1).gate(1)])"#;
    let x = bounce(dir.path(), patch, notes);
    assert!(
        (x[3600 * 2] - 0.75).abs() < 0.002,
        "earlier note-off or new attack reset envelope"
    );
    assert!(
        (x[3600 * 2 + 1] - 0.88).abs() < 0.002,
        "glide missed target"
    );
    let y = bounce(dir.path(), &patch.replace("legato", "retrigger"), notes);
    assert!(
        (y[3600 * 2] - 0.5).abs() < 0.002,
        "explicit retrigger did not restart attack"
    );
}
