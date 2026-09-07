//! Synthetic sample tuning checks; soundtrack notes are not regression fixtures.
use std::path::Path;

fn sine_sample(path: &Path, rate: u32, period: usize) {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 8,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for i in 0..period * 128 {
        let value = (100. * (std::f64::consts::TAU * i as f64 / period as f64).sin()).round();
        writer.write_sample(value as i8).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn fractional_parent_and_zone_roots_survive_compilation_and_serialization() {
    let dir = tempfile::tempdir().unwrap();
    sine_sample(&dir.path().join("tone.wav"), 11025, 64);
    let source = dir.path().join("case.muz");
    std::fs::write(
        &source,
        r#"
song({tracks:[track("tone",phrase("C4:q"),sample([
    {path:"tone.wav",root:59.75,keys:[0,59]},
    {path:"tone.wav",keys:[60,127]}
],{root:60.25}))]})
"#,
    )
    .unwrap();
    let compiled = muz::compile::compile(&source).unwrap();
    let json = serde_json::to_string(&compiled.session).unwrap();
    let restored: muz::Session = serde_json::from_str(&json).unwrap();
    for session in [&compiled.session, &restored] {
        let zones = session.tracks[0].instrument.sample.as_ref().unwrap();
        for (zone, expected) in zones.iter().zip([59.75, 60.25]) {
            assert!(
                (zone.root - expected).abs() < 1e-12,
                "fractional sample root was changed: {} instead of {expected}",
                zone.root
            );
        }
    }
    // Older serialized projects use JSON integers for root; those remain valid.
    let mut value = serde_json::to_value(&compiled.session).unwrap();
    value["tracks"][0]["instrument"]["sample"][0]["root"] = serde_json::json!(60);
    let restored: muz::Session = serde_json::from_value(value).unwrap();
    assert_eq!(
        restored.tracks[0].instrument.sample.as_ref().unwrap()[0].root,
        60.
    );
}

fn measured_frequency(samples: &[f32], rate: u32, start: f64, end: f64) -> f64 {
    let first = (start * rate as f64) as usize;
    let last = (end * rate as f64) as usize;
    let mut crossings = Vec::new();
    for i in first + 1..last {
        let before = samples[(i - 1) * 2] as f64;
        let after = samples[i * 2] as f64;
        if before <= 0. && after > 0. {
            crossings.push((i - 1) as f64 - before / (after - before));
        }
    }
    assert!(crossings.len() > 10, "missing rendered sine wave");
    rate as f64 * (crossings.len() - 1) as f64
        / (crossings.last().unwrap() - crossings.first().unwrap())
}

#[test]
fn fractional_root_renders_in_tune_across_octaves_rates_and_loop_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let sample_rate = 11025;
    let period = 64;
    sine_sample(&dir.path().join("tone.wav"), sample_rate, period);
    let root = 69. + 12. * (sample_rate as f64 / period as f64 / 440.).log2();
    let source = dir.path().join("case.muz");
    std::fs::write(&source, format!(r#"
song({{tempo:120,tracks:[track("tone",
    stack([note(48,2b,at=0b),note(60,2b,at=2.5b),note(72,2b,at=5b)]).gate(1),
    sample("tone.wav",{{root:{root},loop:[{start}s,{end}s],attack_ms:1,release_ms:15}}))],tail:0.1}})
"#, start=64./sample_rate as f64, end=128./sample_rate as f64)).unwrap();
    for (rate, block) in [(22050, 97), (48000, 256)] {
        let output = dir.path().join(format!("{rate}.wav"));
        muz::render::render(&source, &output, None, None, &[], rate, block).unwrap();
        let samples: Vec<f32> = hound::WavReader::open(output)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect();
        for (index, pitch) in [48., 60., 72.].into_iter().enumerate() {
            let at = index as f64 * 1.25;
            let measured = measured_frequency(&samples, rate, at + 0.12, at + 0.85);
            let expected = 440. * 2f64.powf((pitch - 69.) / 12.);
            let cents = 1200. * (measured / expected).log2();
            assert!(
                cents.abs() < 0.1,
                "MIDI {pitch}, {rate} Hz output: {measured:.6} Hz, error {cents:.4} cents"
            );
        }
    }
}

#[test]
fn preparation_reports_performed_sampler_gaps_and_layer_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    sine_sample(&dir.path().join("tone.wav"), 11025, 64);
    let source = dir.path().join("coverage.muz");
    std::fs::write(
        &source,
        r#"
song({tracks:[track("gap",stack([
    note(60.25,1b,velocity=0.5),note(62,1b,velocity=0.7)
]),sample([{path:"tone.wav",root:60,keys:[60,60],velocity:[0,0.5]}]))]})
"#,
    )
    .unwrap();
    let mut session = muz::compile::compile(&source).unwrap().session;
    let config = muz::audio::AudioConfig {
        sample_rate: 48000.,
        max_frames: 256,
    };
    let error = match muz::audio::AudioEngine::new(&session, config) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("unmapped notes must fail preparation"),
    };
    assert!(error.contains("track 'gap': 2"), "{error}");
    assert!(
        error.contains("pitch 60.25")
            && error.contains("velocity 0.5")
            && error.contains("source key"),
        "{error}"
    );
    let zones = session.tracks[0].instrument.sample.as_mut().unwrap();
    let mut upper = zones[0].clone();
    upper.velocity = [0.5, 1.];
    upper.keys = [60, 62];
    assert!(upper.matches(60, 0.5));
    assert!(upper.matches(62, 1.));
    assert!(!upper.matches(63, 1.));
    zones.push(upper);
    muz::audio::AudioEngine::new(&session, config).unwrap();
}

#[test]
fn value_only_reload_cannot_introduce_an_unmapped_sampler_note() {
    let dir = tempfile::tempdir().unwrap();
    sine_sample(&dir.path().join("tone.wav"), 11025, 64);
    let path = dir.path().join("reload.muz");
    std::fs::write(
        &path,
        r#"song({tracks:[track("sampler",note(60),sample([{path:"tone.wav",keys:[60,60]}]))]})"#,
    )
    .unwrap();
    let mut current = muz::compile::compile(&path).unwrap().session;
    current.tracks[0].source = muz::model::TrackSource::Pattern(muz::model::Pattern {
        id: muz::model::Id::new("loop"),
        notes: vec![muz::model::Note {
            id: muz::model::Id::new("probe"),
            start_ticks: 0,
            duration_ticks: 240,
            key: 60,
            velocity: 0.5,
        }],
    });
    let mut candidate = current.clone();
    candidate.tracks[0].source.pattern_mut().unwrap().notes[0].key = 62;
    let plan = muz::plan_reconciliation(0, &current, &candidate).unwrap();
    let error = muz::audio::PreparedValueTransaction::prepare(
        &current,
        &candidate,
        &plan,
        0,
        std::time::Instant::now(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("source key 'probe'"), "{error}");
}
