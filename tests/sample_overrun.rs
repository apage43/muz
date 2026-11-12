//! `muz check` names a held sampler note that outlasts its unlooped recording.
use std::path::Path;

fn tone(path: &Path, seconds: f64) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 8000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..(seconds * 8000.) as usize {
        w.write_sample(((i as f32 * 0.05).sin() * 8000.) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
}

fn warnings(dir: &Path, notes: &str, zone: &str) -> Vec<muz::compile::Diagnostic> {
    let path = dir.join("song.muz");
    std::fs::write(
        &path,
        format!(r#"song({{tempo:60,tracks:[track("flute",{notes},sample([{{path:"tone.wav",root:60{zone}}}]))]}})"#),
    )
    .unwrap();
    muz::compile::compile(&path)
        .unwrap()
        .diagnostics
        .into_iter()
        .filter(|d| d.code == "sample.overrun")
        .collect()
}

#[test]
fn held_notes_past_an_unlooped_recording_are_warned_once_per_track() {
    let dir = tempfile::tempdir().unwrap();
    tone(&dir.path().join("tone.wav"), 1.0);
    // 60 bpm: a beat is a second. The recording lasts 1 s at its root and 0.5 s
    // an octave up, from a 0.25 s offset 0.75 s.
    assert!(warnings(dir.path(), r#"phrase("C4:q")"#, "").is_empty());
    let found = warnings(dir.path(), r#"phrase("C4:q C4:h C5:q").gate(1)"#, "");
    assert_eq!(found.len(), 1);
    let w = &found[0];
    assert_eq!(
        (w.track.as_str(), w.severity.as_str()),
        ("flute", "warning")
    );
    assert!(
        w.message.starts_with("2 held notes outlast"),
        "{}",
        w.message
    );
    assert!(w.message.contains("C4"), "{}", w.message);
    assert!(
        w.message.contains("held 2.00 s, but tone.wav lasts 1.00 s"),
        "{}",
        w.message
    );
    assert_eq!(w.beat, 1.0);
    let found = warnings(dir.path(), r#"phrase("C4:q").gate(1)"#, ", offset:0.25s");
    assert!(
        found[0]
            .message
            .contains("held 1.00 s, but tone.wav lasts 0.75 s"),
        "{}",
        found[0].message
    );
    // Looped and one-shot zones sustain or play out by design.
    assert!(warnings(dir.path(), r#"phrase("C4:w")"#, ", loop:[0.2s, 0.8s]").is_empty());
    assert!(warnings(dir.path(), r#"phrase("C4:w")"#, ", one_shot:true").is_empty());
}
