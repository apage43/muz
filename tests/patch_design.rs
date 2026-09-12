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

fn recording(path: &Path, frames: usize, value: impl Fn(usize) -> f32) {
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
    for i in 0..frames {
        w.write_sample(value(i)).unwrap();
    }
    w.finalize().unwrap();
}
#[test]
fn graph_readers_preserve_zone_choices_gain_and_reverse_regions() {
    let dir = tempfile::tempdir().unwrap();
    recording(&dir.path().join("a.wav"), 4800, |_| 0.2);
    recording(&dir.path().join("b.wav"), 4800, |_| 0.6);
    let patch = r#"voice_patch("p",{gain_db:0,nodes:[{id:"r",op:"reader",source:sample([{path:"a.wav",gain_db:-6},{path:"b.wav",gain_db:-6}])}],output:{left:"r",right:"r"},lifetime:{envelope:"r",tail:0}})"#;
    let x = bounce(
        dir.path(),
        patch,
        "note(60,1/4b,velocity=1).annotate(\"all\",{sample_zone:1})",
    );
    assert!((x[1000 * 2] - 0.6 * 10f32.powf(-6. / 20.)).abs() < 1e-6);
    let x = bounce(dir.path(), patch, "note(60,1/4b,velocity=1).repeat(2)");
    assert!((x[1000 * 2] - 0.2 * 10f32.powf(-6. / 20.)).abs() < 1e-6);
    assert!((x[7000 * 2] - 0.6 * 10f32.powf(-6. / 20.)).abs() < 1e-6);
    recording(&dir.path().join("ramp.wav"), 4800, |i| i as f32 / 4800.);
    let reverse = r#"voice_patch("p",{gain_db:0,nodes:[{id:"r",op:"reader",source:sample("ramp.wav"),speed:-1,offset:0.01,end:0.05}],output:{left:"r",right:"r"},lifetime:{envelope:"r",tail:0}})"#;
    let x = bounce(dir.path(), reverse, "note(60,1/4b,velocity=1)");
    assert!((x[0] - (2399. / 4800.)).abs() < 1e-6);
    assert!(x[1800 * 2] < x[1000 * 2]);
    assert!(x[2000 * 2..].iter().all(|x| x.abs() < 1e-6));
}
#[test]
fn graph_reader_coverage_is_checked_before_playback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.muz");
    std::fs::write(&path,r#"song({tracks:[track("p",note(60,1b),voice_patch("p",{nodes:[{id:"r",op:"reader",source:sample([{path:"missing.wav",keys:[70,80]}])}],output:"r"}))]})"#).unwrap();
    recording(&dir.path().join("missing.wav"), 100, |_| 0.);
    let session = muz::compile::compile(&path).unwrap().session;
    let e = session.validate_sample_coverage().unwrap_err();
    assert!(e.contains("no matching zone"), "{e}");
}

#[test]
fn sampled_source_recipe_and_release_lane_compile_with_real_timing() {
    let dir = tempfile::tempdir().unwrap();
    recording(&dir.path().join("tone.wav"), 4800, |_| 0.2);
    let path = dir.path().join("case.muz");
    std::fs::write(
        &path,
        r#"use "std/instrument" as i;
      let p=note(60,1b).gate(0.5);
      song({tempo:120,tracks:[track("a",p,i.sampled(sample("tone.wav",{attack_ms:2ms}))),
      track("r",i.releases(p,50ms,{tempo:120}),sample("tone.wav"))]})"#,
    )
    .unwrap();
    let session = muz::compile::compile(&path).unwrap().session;
    session.validate_sample_coverage().unwrap();
    if let muz::model::TrackSource::Midi(m) = &session.tracks[1].source {
        assert_eq!(m.imported.notes.len(), 1);
        assert!(
            (m.imported.notes[0].start_tick as f64 / m.imported.summary.ppq as f64 - 0.5).abs()
                < 0.001
        );
    }
}
#[test]
fn loop_crossfade_softens_boundary_and_exits_after_release() {
    let dir = tempfile::tempdir().unwrap();
    recording(&dir.path().join("loop.wav"), 4800, |i| {
        if i < 2400 { i as f32 / 2400. } else { 0. }
    });
    let base = r#"voice_patch("p",{gain_db:0,nodes:[{id:"r",op:"reader",source:sample([{path:"loop.wav",loop:[0,0.05]}]),loop_crossfade:FADE}],output:{left:"r",right:"r"},lifetime:{envelope:"r",tail:0}})"#;
    let a = bounce(
        dir.path(),
        &base.replace("FADE", "0"),
        "note(60,1/5b,velocity=1).gate(1)",
    );
    let b = bounce(
        dir.path(),
        &base.replace("FADE", "0.01"),
        "note(60,1/5b,velocity=1).gate(1)",
    );
    assert!((b[2400 * 2] - b[2399 * 2]).abs() < (a[2400 * 2] - a[2399 * 2]).abs() * 0.05);
    assert!(b[10000 * 2..].iter().all(|x| x.abs() < 1e-6));
}

fn bin_level(x: &[f32], start: usize, frames: usize, hz: f64) -> f64 {
    let (mut re, mut im) = (0., 0.);
    for n in 0..frames {
        let phase = std::f64::consts::TAU * hz * n as f64 / 48000.;
        re += x[(start + n) * 2] as f64 * phase.cos();
        im += x[(start + n) * 2] as f64 * phase.sin();
    }
    re.hypot(im) / frames as f64
}
#[test]
fn antialiased_shape_reduces_folded_harmonic_relative_to_fundamental() {
    let dir = tempfile::tempdir().unwrap();
    let patch = r#"voice_patch("p",{gain_db:0,nodes:[{id:"o",op:"osc",hz:7000},{id:"s",op:"shape",input:"o",points:[[-1,-1],[-0.2,-1],[0.2,1],[1,1]],quality:"QUALITY"}],output:"s"})"#;
    let raw = bounce(
        dir.path(),
        &patch.replace("QUALITY", "raw"),
        "note(60,1b,velocity=1)",
    );
    let aa = bounce(
        dir.path(),
        &patch.replace("QUALITY", "adaa"),
        "note(60,1b,velocity=1)",
    );
    let ratio = |x: &[f32]| bin_level(x, 480, 4800, 13000.) / bin_level(x, 480, 4800, 7000.);
    assert!(
        ratio(&aa) < ratio(&raw) * 0.8,
        "raw {} adaa {}",
        ratio(&raw),
        ratio(&aa)
    );
}
#[test]
fn resonator_has_calibrated_decay_and_damping_reduces_feedback_peak() {
    let dir = tempfile::tempdir().unwrap();
    let patch = r#"voice_patch("p",{gain_db:0,nodes:[{id:"e",op:"adsr",attack:0,decay:0.0001,sustain:0,one_shot:true},{id:"r",op:"resonator",input:"e",frequency:1000,decay:0.1}],output:"r",lifetime:{envelope:"e",tail:0.3}})"#;
    let x = bounce(dir.path(), patch, "note(60,1b,velocity=1)");
    let peak = |x: &[f32], start: usize, count: usize| {
        x[start * 2..(start + count) * 2]
            .iter()
            .map(|v| v.abs())
            .fold(0f32, f32::max)
    };
    assert!((peak(&x, 5280, 48) / peak(&x, 480, 48) - 0.001).abs() < 0.00001);
    let delay = patch.replace(
        "op:\"resonator\",input:\"e\",frequency:1000,decay:0.1",
        "op:\"delay\",input:\"e\",seconds:0.01,feedback:0.9",
    );
    let plain = bounce(dir.path(), &delay, "note(60,1b,velocity=1)");
    let damp = bounce(
        dir.path(),
        &delay.replace("feedback:0.9", "feedback:0.9,damping:400"),
        "note(60,1b,velocity=1)",
    );
    assert!((peak(&plain, 480, 50) - peak(&damp, 480, 50)).abs() < 1e-6);
    assert!(peak(&damp, 960, 100) < peak(&plain, 960, 100) * 0.3);
}
#[test]
fn bandpass_rejects_distant_frequency_and_phase_offset_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let patch = r#"voice_patch("p",{gain_db:0,nodes:[{id:"o",op:"osc",hz:HZ},{id:"f",op:"filter",input:"o",cutoff:1000,q:2,mode:"bandpass"}],output:"f"})"#;
    let a = bounce(
        dir.path(),
        &patch.replace("HZ", "1000"),
        "note(60,1b,velocity=1)",
    );
    let b = bounce(
        dir.path(),
        &patch.replace("HZ", "10000"),
        "note(60,1b,velocity=1)",
    );
    assert!(bin_level(&a, 4800, 4800, 1000.) > bin_level(&b, 4800, 4800, 10000.) * 10.);
    let phase = r#"voice_patch("p",{gain_db:0,nodes:[{id:"e",op:"adsr",attack:0,sustain:1},{id:"o",op:"osc",phase:0.25}],output:{left:"o",right:"o"}})"#;
    let x = bounce(dir.path(), phase, "note(60,1b,velocity=1)");
    assert!((x[0] - 1.).abs() < 1e-6);
}

#[test]
fn sampled_filter_expression_remains_independent_for_overlapping_notes() {
    let dir = tempfile::tempdir().unwrap();
    recording(&dir.path().join("tone.wav"), 24000, |i| {
        (std::f32::consts::TAU * 8000. * i as f32 / 48000.).sin() * 0.2
    });
    let patch = r#"voice_patch("p",{gain_db:0,nodes:[{id:"r",op:"reader",source:sample("tone.wav")},{id:"p",op:"expression",kind:"pressure"},{id:"depth",op:"mul",inputs:["p",14000]},{id:"cut",op:"sum",inputs:["depth",200]},{id:"f",op:"filter",input:"r",cutoff:"cut"}],output:"f"})"#;
    let soft = bounce(
        dir.path(),
        patch,
        "note(60,1/2b,velocity=1).express({pressure:0})",
    );
    let bright = bounce(
        dir.path(),
        patch,
        "note(60,1/2b,velocity=1).express({pressure:1})",
    );
    let both = bounce(
        dir.path(),
        patch,
        "stack([note(60,1/2b,velocity=1).express({pressure:0}),note(60,1/2b,velocity=1).express({pressure:1})])",
    );
    assert!(bin_level(&bright, 480, 4800, 8000.) > bin_level(&soft, 480, 4800, 8000.) * 100.);
    assert!(
        both.iter()
            .zip(soft.iter().zip(&bright))
            .all(|(sum, (a, b))| (sum - a - b).abs() < 1e-5)
    );
}

#[test]
fn sampler_initial_offset_can_start_inside_a_loop() {
    let dir = tempfile::tempdir().unwrap();
    recording(&dir.path().join("offset.wav"), 4800, |_| 0.2);
    let patch = r#"voice_patch("p",{gain_db:0,nodes:[{id:"r",op:"reader",source:sample([{path:"offset.wav",offset:0.03,loop:[0.01,0.05]}])}],output:{left:"r",right:"r"},lifetime:{envelope:"r",tail:0}})"#;
    let x = bounce(dir.path(), patch, "note(60,1/2b,velocity=1).gate(1)");
    assert!(x[0..12000 * 2].iter().all(|x| (x - 0.2).abs() < 1e-6));
}
