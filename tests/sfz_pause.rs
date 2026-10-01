use muz::{
    audio::{AudioConfig, AudioEngine},
    description::ValidatedSession,
};

#[test]
fn paused_sfz_preserves_held_sample_and_modulation_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = hound::WavWriter::create(
        dir.path().join("sample.wav"),
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for n in 0..2048 {
        writer.write_sample((n as f32 * 0.047).sin() * 0.2).unwrap();
    }
    writer.finalize().unwrap();
    std::fs::write(dir.path().join("program.sfz"), "<region> sample=sample.wav key=60 loop_mode=loop_continuous loop_start=0 loop_end=2047 lfo01_freq=3 lfo01_volume=6 ampeg_attack=0.02").unwrap();
    let path = dir.path().join("main.muz");
    std::fs::write(
        &path,
        "song({tracks:[track(\"x\",note(60,4b),sfz(\"program.sfz\"))]})",
    )
    .unwrap();
    let (value, deps) = muz::lang::load(&path).unwrap();
    let compiled = muz::compile::lower(value, &path, deps).unwrap();
    let checked = ValidatedSession::new(&compiled.session).unwrap();
    let config = AudioConfig {
        sample_rate: 48000.,
        max_frames: 256,
        offline: true,
    };
    let mut paused = AudioEngine::from_validated(&checked, config).unwrap();
    let mut continuous = AudioEngine::from_validated(&checked, config).unwrap();
    paused.set_running(true);
    continuous.set_running(true);
    let mut left = [0.; 512];
    let mut right = [0.; 512];
    for _ in 0..5 {
        paused.render_interleaved(&mut left, 2).unwrap();
        continuous.render_interleaved(&mut right, 2).unwrap();
        assert_eq!(left, right);
    }
    assert!(left.iter().any(|sample| sample.abs() > 0.01));
    let frame = paused.status().transport.project_frame;
    paused.set_running(false);
    for _ in 0..3 {
        paused.render_interleaved(&mut left, 2).unwrap();
    }
    assert_eq!(paused.status().transport.project_frame, frame);
    paused.set_running(true);
    paused.render_interleaved(&mut left, 2).unwrap();
    continuous.render_interleaved(&mut right, 2).unwrap();
    assert_eq!(
        left, right,
        "pause must preserve held sample phase, envelope and LFO history"
    );
}
