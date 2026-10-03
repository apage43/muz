//! Stateful seeks replay history outside the callback instead of restarting a held sample.
use muz::{
    audio::{AudioConfig, AudioEngine},
    description::ValidatedSession,
    lang::Evaluator,
};

fn fixture() -> (tempfile::TempDir, muz::Session) {
    fixture_with_effects(false)
}

fn fixture_with_effects(effects: bool) -> (tempfile::TempDir, muz::Session) {
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
    let routes = if effects { ",{sends:{echo:0}}" } else { "" };
    let buses = if effects {
        r#",buses:[bus("echo",[fx("delay",{time_ms:100,mix:1,feedback:0.5})])]"#
    } else { "" };
    let source = format!(
        r#"song({{tempo:120,tracks:[track("history",stack([note(60,0.125b),note(60,1b,at=0.5b)]),sfz({}){routes})]{buses}}})"#,
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

#[test]
fn runtime_revision_replays_sfz_loop_and_preserves_running_policy() {
    use muz::audio::PreparedTransaction;

    let (_assets, current) = fixture();
    let range = Some((720_000, 960_000));
    for structural in [false, true] {
        for running in [false, true] {
            let mut candidate = current.clone();
            if structural {
                candidate.tracks[0].instrument.generation += 1;
            } else {
                candidate.extras.title = "presentation revision".into();
            }
            let plan = muz::plan_reconciliation(0, &current, &candidate).unwrap();
            let mut transaction =
                PreparedTransaction::prepare(&current, &candidate, &plan, 7, None, config())
                    .unwrap();
            assert_eq!(transaction.is_structural(), structural);
            transaction
                .prepare_runtime_policy(&current, &candidate, range, 18_000, None)
                .unwrap();
            assert!(transaction.is_structural());
            assert!(transaction.needs_fade());
            assert!(transaction.fade_out().unwrap().master);
            assert!(transaction.fade_in().unwrap().master);
            let checked = ValidatedSession::new(&current).unwrap();
            let mut engine =
                AudioEngine::prepare_loop(&checked, config(), 720_000, 960_000, 18_000)
                    .unwrap();
            let mut scratch = [0.0; 512];
            engine.render_interleaved(&mut scratch, 2).unwrap();
            engine.set_running(running);
            transaction.apply(&mut engine).unwrap();
            assert_eq!(engine.revision(), 1);
            assert_eq!(engine.status().transport.running, running);
            assert!((engine.status().transport.current_tick - 720_000.0).abs() < 0.01);
            let checked = ValidatedSession::new(&candidate).unwrap();
            let mut expected =
                AudioEngine::prepare_loop(&checked, config(), 720_000, 960_000, 18_000)
                    .unwrap();
            expected.set_running(running);
            let mut expected_pcm = [0.0; 512];
            // Cross multiple boundaries: losing the freshly prepared checkpoint
            // on acceptance would diverge from exact held-voice/sequence history.
            for _ in 0..80 {
                engine.render_interleaved(&mut scratch, 2).unwrap();
                expected.render_interleaved(&mut expected_pcm, 2).unwrap();
                assert_eq!(scratch, expected_pcm);
                assert_eq!(
                    engine.status().transport.current_tick,
                    expected.status().transport.current_tick
                );
            }
        }
    }
}

#[test]
fn runtime_loop_budget_failure_keeps_transaction_retryable_and_audio_accepted() {
    use muz::audio::PreparedTransaction;

    let (_assets, current) = fixture();
    let mut candidate = current.clone();
    candidate.extras.title = "new title".into();
    let plan = muz::plan_reconciliation(0, &current, &candidate).unwrap();
    let mut transaction =
        PreparedTransaction::prepare(&current, &candidate, &plan, 7, None, config()).unwrap();
    let checked = ValidatedSession::new(&current).unwrap();
    let mut engine =
        AudioEngine::prepare_loop(&checked, config(), 720_000, 960_000, 18_000).unwrap();
    let before = engine.status();
    let error = transaction
        .prepare_runtime_policy(&current, &candidate, Some((720_000, 960_000)), 0, None)
        .unwrap_err();
    assert!(error.to_string().contains("budget"));
    assert!(!transaction.is_structural());
    assert_eq!(engine.status(), before);
    transaction
        .prepare_runtime_policy(
            &current,
            &candidate,
            Some((720_000, 960_000)),
            18_000,
            None,
        )
        .unwrap();
    transaction.apply(&mut engine).unwrap();
    assert_eq!(engine.revision(), 1);
}

#[test]
fn runtime_sfz_loop_replacement_reapplies_empty_listening_mask() {
    use muz::audio::PreparedTransaction;

    let (_assets, current) = fixture_with_effects(true);
    // Positive control: masking only after replay retains audible bus delay history.
    let checked = ValidatedSession::new(&current).unwrap();
    let mut unmasked_replay =
        AudioEngine::prepare_loop(&checked, config(), 720_000, 960_000, 18_000).unwrap();
    unmasked_replay.set_track_audibility(&[], false).unwrap();
    let mut leaked_history = [0.0; 512];
    unmasked_replay.render_interleaved(&mut leaked_history, 2).unwrap();
    assert!(leaked_history.iter().any(|sample| sample.abs() > 0.0001));
    for structural in [false, true] {
        let mut candidate = current.clone();
        if structural {
            candidate.tracks[0].instrument.generation += 1;
        } else {
            candidate.extras.title = "masked presentation revision".into();
        }
        let plan = muz::plan_reconciliation(0, &current, &candidate).unwrap();
        let mut transaction =
            PreparedTransaction::prepare(&current, &candidate, &plan, 7, None, config()).unwrap();
        transaction
            .prepare_runtime_policy(
                &current,
                &candidate,
                Some((720_000, 960_000)),
                18_000,
                Some(&[]),
            )
            .unwrap();
        let mut engine =
            AudioEngine::prepare_loop(&checked, config(), 720_000, 960_000, 18_000).unwrap();
        transaction.apply(&mut engine).unwrap();
        let mut pcm = [0.0; 512];
        for _ in 0..80 {
            engine.render_interleaved(&mut pcm, 2).unwrap();
            assert_eq!(pcm, [0.0; 512]);
        }
    }
}
