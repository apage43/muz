//! A note's `articulation` selects SFZ regions as a keyswitch pressed just
//! before that note would, without a switch note in the score. Each generated
//! recording is a distinct constant level, so a window's level names the
//! regions that played.
use muz::{
    audio::{AudioConfig, AudioEngine},
    description::ValidatedSession,
};

const OPEN: f32 = 0.05;
const MUTE: f32 = 0.1;
const HARMONIC: f32 = 0.2;
const SLIDE: f32 = 0.4;
const BEAT: usize = 24000;

fn program() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, level) in [
        ("open.wav", OPEN),
        ("mute.wav", MUTE),
        ("harmonic.wav", HARMONIC),
        ("slide.wav", SLIDE),
    ] {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(dir.path().join(name), spec).unwrap();
        for _ in 0..48000 {
            writer.write_sample(level).unwrap();
        }
        writer.finalize().unwrap();
    }
    std::fs::write(
        dir.path().join("guitar.sfz"),
        "<control> set_cc7=127 set_cc11=127\n\
         <global> ampeg_attack=0 ampeg_release=0.001 amp_veltrack=0 lokey=40 hikey=84 \
         sw_lokey=24 sw_hikey=26 sw_default=24\n\
         <region> sw_last=24 sample=open.wav\n\
         <region> sw_last=25 sample=mute.wav\n\
         <region> sw_last=26 locc20=0 hicc20=63 sample=harmonic.wav\n\
         <region> sw_last=26 locc20=64 hicc20=127 sample=slide.wav\n",
    )
    .unwrap();
    dir
}

const ARTICULATIONS: &str =
    "articulations: {open: 24, mute: 25, harmonic: 26, slide: {key: 26, cc20: 100}}";

/// Technique tags are mapped to articulation names in source.
fn compile(dir: &std::path::Path, options: &str, pattern: &str) -> anyhow::Result<muz::compile::Compiled> {
    let path = dir.join("song.muz");
    std::fs::write(
        &path,
        format!(
            r#"use "std/checks" as checks;
let techniques = {{pm: "mute", harm: "harmonic", sl: "slide"}};
fn articulate(p) = p.map_notes(fn(n) => {{
    let named = filter(n.tags, fn(tag) => contains(keys(techniques), tag));
    if len(named) > 0 {{ {{data: merge(n.data, {{articulation: techniques[named[0]]}})}} }} else {{ {{}} }}
}});
let guitar = sfz("guitar.sfz", {{{options}}});
song({{tempo: 120, tail: 0, tracks: [
    track("lead", articulate({pattern}), guitar, {{checks: [checks.in_range(40, 84)]}})]}})"#
        ),
    )?;
    muz::compile::compile(&path)
}

/// Mean left-channel level in the middle of each half-beat note.
fn levels(session: &muz::Session, beats: usize) -> Vec<f32> {
    let checked = ValidatedSession::new(session).unwrap();
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
    let mut left = Vec::with_capacity(beats * BEAT);
    let mut block = vec![0.0; 512];
    while left.len() < beats * BEAT {
        engine.render_interleaved(&mut block, 2).unwrap();
        left.extend(block.chunks(2).map(|frame| frame[0]));
    }
    (0..beats)
        .map(|beat| {
            let window = &left[beat * BEAT + 5000..beat * BEAT + 7000];
            window.iter().sum::<f32>() / window.len() as f32
        })
        .collect()
}

fn assert_levels(actual: &[f32], expected: &[f32]) {
    // One note of the default articulation calibrates the instrument's gain.
    let gain = actual[1] / expected[1];
    assert!(gain > 0.1, "default articulation is silent: {actual:?}");
    for (beat, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (a / gain - e).abs() < 0.004,
            "beat {beat}: level {} is not {e}; all {actual:?}",
            a / gain
        );
    }
}

const SCORE: &str = r#"stack([
    phrase("C4:e@pm r:e C4:e r:q. C4:e@sl r:e C4:e@harm r:e"),
    phrase("C4:e@harm").at(2b),
    phrase("E4:e@pm").at(2b),
    note("C4", 0.5b).annotate("all", {articulation: 25}).at(5b)])"#;

#[test]
fn each_note_plays_its_own_articulation_without_switch_notes() {
    let dir = program();
    let compiled = compile(dir.path(), ARTICULATIONS, SCORE).unwrap();
    // Checks, the score and MIDI export see only the seven musical notes.
    assert!(compiled.diagnostics.is_empty(), "{:?}", compiled.diagnostics);
    let muz::model::TrackSource::Midi(source) = &compiled.session.tracks[0].source;
    assert_eq!(source.imported.notes.len(), 7);
    assert!(source.imported.notes.iter().all(|n| n.key >= 40));
    assert_eq!(compiled.score[0].pattern.notes.len(), 7);
    let exported = muz::smf::export(&compiled.session).unwrap();
    assert_eq!(muz::smf::pattern(&exported, None).unwrap().notes.len(), 7);

    // Instrument state regenerates the options from the model.
    let instrument = &compiled.session.tracks[0].instrument;
    let state = muz::device_state::DeviceState::from_device(
        muz::device_state::DeviceRole::Instrument,
        instrument.clone(),
    )
    .unwrap();
    assert_eq!(state.device.sfz, instrument.sfz);
    assert_eq!(instrument.sfz.as_ref().unwrap().articulations.len(), 4);

    assert_levels(
        &levels(&compiled.session, 6),
        &[
            // The song's first note, at beat 0, with no lead-in.
            MUTE,
            // No choice: the program's sw_default, not the previous note's mute.
            OPEN,
            // Simultaneous notes each keep their own articulation.
            HARMONIC + MUTE,
            // The articulation's controller gates the region for this note...
            SLIDE,
            // ...and does not stay on the channel afterwards.
            HARMONIC,
            // A raw switch key.
            MUTE,
        ],
    );
}

#[test]
fn a_declared_default_articulation_applies_to_notes_naming_none() {
    let dir = program();
    let options = format!("{ARTICULATIONS}, default_articulation: \"mute\"");
    let compiled = compile(dir.path(), &options, r#"phrase("C4:e@harm r:e C4:e r:e")"#).unwrap();
    let actual = levels(&compiled.session, 2);
    assert!(
        (actual[0] / actual[1] - HARMONIC / MUTE).abs() < 0.05,
        "{actual:?}"
    );
}

#[test]
fn written_keyswitch_notes_still_latch_for_notes_naming_no_articulation() {
    let dir = program();
    let compiled = compile(
        dir.path(),
        ARTICULATIONS,
        r#"stack([note(25, 0.1b), phrase("r:q C4:e r:e C4:e@harm r:e C4:e r:e")])"#,
    )
    .unwrap();
    let actual = levels(&compiled.session, 4);
    // The written switch (key 25) holds across the per-note harmonic.
    assert!(actual[1] > 0.01, "{actual:?}");
    assert!((actual[2] / actual[1] - HARMONIC / MUTE).abs() < 0.05, "{actual:?}");
    assert!((actual[3] / actual[1] - 1.).abs() < 0.05, "{actual:?}");
}

#[test]
fn articulation_mistakes_are_compile_errors() {
    let dir = program();
    let error = |options: &str, pattern: &str| {
        format!("{:#}", compile(dir.path(), options, pattern).unwrap_err())
    };
    let unknown = error(
        ARTICULATIONS,
        r#"phrase("C4:q").annotate("all", {articulation: "mut"})"#,
    );
    assert!(unknown.contains("unknown articulation 'mut'"), "{unknown}");
    assert!(unknown.contains("did you mean 'mute'"), "{unknown}");
    assert!(unknown.contains("harmonic, mute, open, slide"), "{unknown}");

    let raw = error(
        ARTICULATIONS,
        r#"phrase("C4:q").annotate("all", {articulation: 30})"#,
    );
    assert!(raw.contains("key 30 is not a keyswitch"), "{raw}");
    assert!(raw.contains("24, 25, 26"), "{raw}");

    let declared = error("articulations: {mute: 30}", r#"phrase("C4:q")"#);
    assert!(declared.contains("'mute': key 30 is not a keyswitch"), "{declared}");
    assert!(declared.contains("24, 25, 26"), "{declared}");

    let default = error(
        "articulations: {mute: 25}, default_articulation: \"open\"",
        r#"phrase("C4:q")"#,
    );
    assert!(default.contains("unknown default articulation 'open'"), "{default}");

    let path = dir.path().join("synth.muz");
    std::fs::write(
        &path,
        r#"song({tracks: [track("x", note(60, 1b).annotate("all", {articulation: "mute"}), synth("bell"))]})"#,
    )
    .unwrap();
    let other = format!("{:#}", muz::compile::compile(&path).unwrap_err());
    assert!(other.contains("articulation requires an sfz() instrument"), "{other}");
}

#[test]
fn dawproject_export_reports_the_omitted_articulation() {
    let dir = program();
    let options = format!("{ARTICULATIONS}, embed_assets: true");
    let compiled = compile(dir.path(), &options, r#"phrase("C4:q@pm")"#).unwrap();
    let plan = muz::dawproject::Plan::new(&compiled, "bitwig-linux").unwrap();
    let entry = plan
        .report
        .warnings()
        .find(|e| e.code == "ARTICULATION_OMITTED")
        .expect("per-note articulation is reported");
    assert!(entry.intended.contains("\"mute\""), "{}", entry.intended);
    assert!(
        !plan
            .report
            .warnings()
            .any(|e| e.code == "NOTE_ANNOTATIONS_OMITTED")
    );
}
