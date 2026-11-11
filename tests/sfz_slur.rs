//! `std/instrument.slur` marks slurs with a score tag and writes the overlap
//! only in performed time: an SFZ program's `trigger=legato` regions play the
//! transition, and `checks.monophonic()` reads the line as one voice.
use muz::{
    audio::{AudioConfig, AudioEngine},
    description::ValidatedSession,
};

const ATTACK: f32 = 0.1;
const LEGATO: f32 = 0.3;
const BEAT: usize = 24000;

#[test]
fn tagged_slurs_play_legato_regions_and_pass_monophony_checks() {
    let dir = tempfile::tempdir().unwrap();
    for (name, level) in [("attack.wav", ATTACK), ("legato.wav", LEGATO)] {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(dir.path().join(name), spec).unwrap();
        for _ in 0..96000 {
            writer.write_sample(level).unwrap();
        }
        writer.finalize().unwrap();
    }
    std::fs::write(
        dir.path().join("cello.sfz"),
        "<control> set_cc7=127 set_cc11=127\n\
         <global> ampeg_attack=0 ampeg_release=0.001 amp_veltrack=0 pitch_keytrack=0\n\
         <region> trigger=first sample=attack.wav\n\
         <region> trigger=legato sample=legato.wav\n",
    )
    .unwrap();
    let path = dir.path().join("song.muz");
    std::fs::write(
        &path,
        r#"use "std/checks" as checks;
use "std/instrument" as i;
// Bow C, slur to D, then a new bow on E.
let line = i.slur(phrase("C3:q D3:q@slur E3:q"), "slur");
song({tempo: 120, tail: 0, tracks: [track("cello", line, sfz("cello.sfz"),
    {checks: [checks.monophonic()]})]})"#,
    )
    .unwrap();
    let compiled = muz::compile::compile(&path).unwrap();
    assert!(
        compiled.diagnostics.is_empty(),
        "{:?}",
        compiled.diagnostics
    );
    // The tag stays in the score and the export; the overlap is performed only.
    assert!(compiled.score[0].pattern.notes[1].tags.contains("slur"));
    let muz::model::TrackSource::Midi(source) = &compiled.session.tracks[0].source;
    let [c, d, _] = &source.imported.notes[..] else {
        panic!("three notes")
    };
    assert!(c.start_tick + c.duration_ticks > d.start_tick);

    let checked = ValidatedSession::new(&compiled.session).unwrap();
    let mut engine = AudioEngine::from_validated(
        &checked,
        AudioConfig {
            sample_rate: 48000.0,
            max_frames: 256,
            offline: true,
        },
    )
    .unwrap();
    engine.set_running(true);
    let mut left = Vec::new();
    let mut block = vec![0.0; 512];
    while left.len() < 3 * BEAT {
        engine.render_interleaved(&mut block, 2).unwrap();
        left.extend(block.chunks(2).map(|frame| frame[0]));
    }
    let level = |beat: usize| {
        let window = &left[beat * BEAT + 5000..beat * BEAT + 7000];
        window.iter().sum::<f32>() / window.len() as f32
    };
    let gain = level(0) / ATTACK;
    assert!(gain > 0.1, "silent");
    assert!(
        (level(1) / gain - LEGATO).abs() < 0.01,
        "slur: {}",
        level(1) / gain
    );
    assert!(
        (level(2) / gain - ATTACK).abs() < 0.01,
        "new bow: {}",
        level(2) / gain
    );
}
