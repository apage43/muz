//! Stateful seeks replay history outside the callback instead of restarting a held sample.
use muz::{
    audio::{AudioConfig, AudioEngine},
    description::ValidatedSession,
    lang::Evaluator,
};

fn fixture() -> (tempfile::TempDir, muz::Session) {
    let dir = tempfile::tempdir().unwrap();
    for (name, level) in [("a.wav", 0.25), ("b.wav", 0.5)] {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(dir.path().join(name), spec).unwrap();
        for frame in 0..48000 {
            writer
                .write_sample(level * (frame as f32 * 0.043).sin())
                .unwrap();
        }
        writer.finalize().unwrap();
    }
    let path = dir.path().join("history.sfz");
    std::fs::write(&path,
        "<global> ampeg_attack=0 ampeg_release=0.01\n<group> seq_length=2\n<region> key=60 seq_position=1 sample=a.wav\n<region> key=60 seq_position=2 sample=b.wav\n").unwrap();
    let source = format!(
        r#"song({{tempo:120,tracks:[track("history",stack([note(60,0.125b),note(60,1b,at=0.5b)]),sfz({}))]}})"#,
        serde_json::to_string(&path.to_string_lossy()).unwrap()
    );
    let value = Evaluator::new().source(&source).unwrap();
    let session = muz::compile::lower(
        value.get("__result").unwrap().clone(),
        &dir.path().join("song.muz"),
        vec![],
    )
    .unwrap()
    .session;
    (dir, session)
}
fn config() -> AudioConfig {
    AudioConfig {
        sample_rate: 48000.0,
        max_frames: 256,
        offline: true,
    }
}

#[test]
fn prepared_seek_matches_continuous_sequence_and_held_voice() {
    let (_assets, session) = fixture();
    let checked = ValidatedSession::new(&session).unwrap();
    let mut continuous = AudioEngine::from_validated(&checked, config()).unwrap();
    continuous.set_running(true);
    let target = 18000;
    let mut scratch = vec![0.0; 512];
    let mut rendered = 0;
    while rendered < target {
        let frames = (target - rendered).min(256);
        continuous
            .render_interleaved(&mut scratch[..frames * 2], 2)
            .unwrap();
        rendered += frames;
    }
    let mut replayed =
        AudioEngine::prepare_seek(&checked, config(), 720_000, target as u64).unwrap();
    let mut expected = vec![0.0; 512];
    let mut actual = vec![0.0; 512];
    continuous.render_interleaved(&mut expected, 2).unwrap();
    replayed.render_interleaved(&mut actual, 2).unwrap();
    assert!(expected.iter().any(|v| v.abs() > 0.01));
    assert_eq!(actual, expected);
    assert_eq!(
        replayed.status().transport.current_tick,
        continuous.status().transport.current_tick
    );
}

#[test]
fn replay_budget_rejects_before_preparation() {
    let (_assets, session) = fixture();
    let checked = ValidatedSession::new(&session).unwrap();
    let before = muz::audio::device::processor_preparations();
    let error = match AudioEngine::prepare_seek(&checked, config(), 720_000, 100) {
        Ok(_) => panic!("seek budget must reject"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("budget"));
    assert_eq!(muz::audio::device::processor_preparations(), before);
}

#[test]
fn prepared_loop_restores_held_voice_and_sequence_history_at_each_wrap() {
    let (_assets, session) = fixture();
    let checked = ValidatedSession::new(&session).unwrap();
    // The second note is held at start; preserve its selected second sample and age.
    let mut engine =
        AudioEngine::prepare_loop(&checked, config(), 720_000, 960_000, 18000).unwrap();
    let mut cycles = vec![0.0; 6000 * 2 * 3];
    for chunk in cycles.chunks_mut(512) {
        engine.render_interleaved(chunk, 2).unwrap();
    }
    assert!(cycles[..12000].iter().any(|v| v.abs() > 0.01));
    assert_eq!(&cycles[..12000], &cycles[12000..24000]);
    assert_eq!(&cycles[..12000], &cycles[24000..]);
}
