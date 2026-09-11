//! Independent envelopes and sample calibration must survive overlapping voices.
use std::path::Path;
fn render(dir: &Path, source: &str, block: usize) -> Vec<f32> {
    let path = dir.join("case.muz");
    let out = dir.join("out.wav");
    std::fs::write(&path, source).unwrap();
    muz::render::render(&path, &out, None, None, &[], 48000, block).unwrap();
    hound::WavReader::open(out)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

fn native_instruments(dir: &Path) -> [&'static str; 2] {
    let mut wav = hound::WavWriter::create(
        dir.join("tone.wav"),
        hound::WavSpec {
            channels: 2,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for i in 0..96000 {
        let x = 0.2 * (std::f32::consts::TAU * i as f32 / 100.).sin();
        wav.write_sample(x).unwrap();
        wav.write_sample(x * 0.5).unwrap();
    }
    wav.finalize().unwrap();
    [
        r#"synth("init",{mode:"fm",fm_index:0,sub:0})"#,
        r#"sample([{path:"tone.wav",root:69}],{attack_ms:0,release_ms:300})"#,
    ]
}

#[test]
fn native_volume_curves_are_per_voice_and_block_independent() {
    let dir = tempfile::tempdir().unwrap();
    for instrument in native_instruments(dir.path()) {
        let source = |a: &str, b: &str| {
            format!(
                r#"song({{tempo:120,tracks:[track("pad",stack([
        note(60,1b).express({{volume:{a}}}),
        note(60,1b,at=0.5b).express({{volume:{b},pan:0.8,expression:0.6,tuning:12}})
    ]),{instrument})],tail:0.3}})"#
            )
        };
        let a = render(dir.path(), &source("[[0,0.58],[0.38,1],[1,0.55]]", "0"), 97);
        let b = render(dir.path(), &source("0", "[[0,1],[1,0.3]]"), 97);
        let both = source("[[0,0.58],[0.38,1],[1,0.55]]", "[[0,1],[1,0.3]]");
        let mixed = render(dir.path(), &both, 97);
        let blocked = render(dir.path(), &both, 256);
        assert!(mixed.iter().any(|x| x.abs() > 0.001));
        assert_eq!(mixed.len(), a.len());
        assert_eq!(mixed.len(), b.len());
        assert_eq!(mixed.len(), blocked.len());
        for (((m, a), b), other) in mixed.iter().zip(a).zip(b).zip(blocked) {
            assert!((m - a - b).abs() < 1e-6, "volume leaked into another voice");
            assert!((m - other).abs() < 1e-6, "block-dependent expression");
        }
    }
}
#[test]
fn zone_gain_calibrates_velocity_layers_without_changing_releasing_voices() {
    let dir = tempfile::tempdir().unwrap();
    for (name, level) in [("soft", 0.1f32), ("loud", 0.4)] {
        let mut w = hound::WavWriter::create(
            dir.path().join(format!("{name}.wav")),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..48000 {
            w.write_sample(level).unwrap();
        }
        w.finalize().unwrap();
    }
    let source = r#"
    fn calibrated(zones, measured_db, target_db) =
        map(range(len(zones)), fn(i) => merge(zones[i], {gain_db:target_db-measured_db[i]}));
    song({tempo:120,tracks:[track("sample",stack([
        note(60,0.25b,velocity=0.25).gate(1),note(60,0.25b,at=0.5b,velocity=0.75).gate(1)
    ]),sample(calibrated([
        {path:"soft.wav",velocity:[0,0.5]},
        {path:"loud.wav",velocity:[0.5,1]}
    ],[-20,-7.958800173440752],-20),{root:60,velocity_track:0,attack_ms:0,release_ms:500,gain_db:0}))],tail:0.5})"#;
    let x = render(dir.path(), source, 97);
    assert!((x[4800 * 2] - 0.1).abs() < 1e-6);
    // At 250 ms, the first voice has released for 125 ms. The new layer is 0.1.
    assert!(
        (x[12000 * 2] - (0.1 * (1.0 - 6001.0 / 24000.0) + 0.1)).abs() < 1e-4,
        "new layer was not calibrated independently: {}",
        x[12000 * 2]
    );
    let y = render(dir.path(), source, 256);
    assert_eq!(x, y);
}

#[test]
fn native_controls_preserve_unexpressed_sound_and_apply_exact_gains() {
    let dir = tempfile::tempdir().unwrap();
    for instrument in native_instruments(dir.path()) {
        let source = |expression: &str| {
            format!(
                r#"song({{tracks:[track("tone",
        note(69,1b).gate(1){expression},{instrument})],tail:0.3}})"#
            )
        };
        let plain = render(dir.path(), &source(""), 97);
        let unity = render(
            dir.path(),
            &source(".express({volume:1,expression:1,pan:0.5,tuning:0})"),
            97,
        );
        assert_eq!(plain, unity);
        let quiet = render(
            dir.path(),
            &source(".express({volume:0.5,expression:0.5})"),
            97,
        );
        for (a, b) in plain.iter().zip(quiet) {
            assert!((a * 0.25 - b).abs() < 1e-7);
        }
        for (pan, silent) in [(0, 1), (1, 0)] {
            let x = render(dir.path(), &source(&format!(".express({{pan:{pan}}})")), 97);
            assert!(x.chunks_exact(2).all(|frame| frame[silent] == 0.));
            assert!(x.iter().any(|v| v.abs() > 0.01));
        }
        let tuned = render(dir.path(), &source(".express({tuning:12})"), 97);
        let crossings = |x: &[f32]| {
            x.chunks_exact(2).map(|p| p[0]).collect::<Vec<_>>()[4800..19200]
                .windows(2)
                .filter(|p| p[0] <= 0. && p[1] > 0.)
                .count()
        };
        assert!((crossings(&tuned) as isize - 2 * crossings(&plain) as isize).abs() <= 2);
        // Repeated curve updates must set tuning relative to the root, not compound it.
        let tuning_curve = render(
            dir.path(),
            &source(".express({tuning:[[0,0],[0.1,12],[1,12]]})"),
            97,
        );
        assert!((crossings(&tuning_curve) as isize - crossings(&tuned) as isize).abs() <= 1);
        let curved = render(dir.path(), &source(".express({volume:[[0,0],[1,1]]})"), 97);
        for (frame, (a, b)) in plain
            .chunks_exact(2)
            .zip(curved.chunks_exact(2))
            .enumerate()
        {
            // Expression holds its last scheduled value through the release.
            let gain = (frame.min(23999) / 128 * 128) as f32 / 24000.;
            for ch in 0..2 {
                assert!((a[ch] * gain - b[ch]).abs() < 1e-6, "curve gain at {frame}");
            }
        }
    }
}

#[test]
fn zone_gain_roundtrips_defaults_and_rejects_invalid_values() {
    let dir = tempfile::tempdir().unwrap();
    let mut wav = hound::WavWriter::create(
        dir.path().join("unused.wav"),
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    wav.write_sample(0.1f32).unwrap();
    wav.finalize().unwrap();
    let path = dir.path().join("case.muz");
    let source = |gain: f64| {
        format!(
            r#"song({{tracks:[track("s",note(60),sample([
        {{path:"unused.wav",gain_db:{gain}}},"unused.wav"
    ],{{gain_db:-9}}))]}})"#
        )
    };
    std::fs::write(&path, source(6.)).unwrap();
    let session = muz::compile::compile(&path).unwrap().session;
    let mut json = serde_json::to_value(&session).unwrap();
    let decoded: muz::Session = serde_json::from_value(json.clone()).unwrap();
    let zones = decoded.tracks[0].instrument.sample.as_ref().unwrap();
    assert_eq!(zones[0].gain_db, 6.);
    assert_eq!(
        zones[1].gain_db, 0.,
        "instrument gain was inherited as zone gain"
    );
    json["tracks"][0]["instrument"]["sample"][0]
        .as_object_mut()
        .unwrap()
        .remove("gain_db");
    let legacy: muz::Session = serde_json::from_value(json).unwrap();
    assert_eq!(
        legacy.tracks[0].instrument.sample.as_ref().unwrap()[0].gain_db,
        0.
    );
    for gain in [-121., 121.] {
        std::fs::write(&path, source(gain)).unwrap();
        let error = muz::compile::compile(&path).unwrap_err().to_string();
        assert!(error.contains("sample zone gain_db"), "{error}");
    }
}

#[test]
fn presets_reject_expression_without_a_defined_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.muz");
    std::fs::write(
        &path,
        r#"song({tracks:[track("s",note(60).express({pressure:0.5}),synth("pad"))]})"#,
    )
    .unwrap();
    let error = muz::compile::compile(&path).unwrap_err().to_string();
    assert!(error.contains("voice_patch or CLAP"), "{error}");
    // Serialized sessions must be checked at preparation too.
    std::fs::write(
        &path,
        r#"song({tracks:[track("s",note(60).express({volume:0.5}),synth("pad"))]})"#,
    )
    .unwrap();
    let mut session = muz::compile::compile(&path).unwrap().session;
    if let muz::model::TrackSource::Midi(m) = &mut session.tracks[0].source {
        m.imported.notes[0]
            .performance
            .as_mut()
            .unwrap()
            .expression
            .points[0]
            .kind = 6;
    } else {
        panic!("expected performed notes");
    }
    assert!(
        muz::audio::AudioEngine::new(
            &session,
            muz::audio::AudioConfig {
                sample_rate: 48000.,
                max_frames: 97,
                offline: false,
            }
        )
        .is_err()
    );
}
