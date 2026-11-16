//! An instrument's declared bend range turns the channel pitch-bend messages a
//! track carries into semitones for embedders. It is descriptive: what plays
//! is unchanged. muz source sends raw bends only to plugins (that case, with
//! the events the plugin receives, is in clap_note_params.rs); native
//! instruments get them from imported MIDI, so these tests add the messages
//! to the compiled track as an import would.
use muz::midi::ChannelMessage;
use muz::model::{BendRange, TrackSource};

/// Compile one track playing C4 for a whole note, then add bends as an
/// imported file would carry them: at beat 1 full up (and a quarter up on
/// channel 3), at beat 2 half down, at beat 3 back to center.
fn compile(dir: &std::path::Path, instrument: &str) -> anyhow::Result<muz::compile::Compiled> {
    let path = dir.join("song.muz");
    std::fs::write(
        &path,
        format!(
            r#"song({{tempo: 120, tail: 0, tracks: [track("lead", phrase("C4:w"), {instrument})]}})"#
        ),
    )?;
    let mut compiled = muz::compile::compile(&path)?;
    let TrackSource::Midi(source) = &mut compiled.session.tracks[0].source;
    let beat = source.imported.summary.ppq as u64;
    let messages = &mut source.imported.messages;
    for (i, (tick, bytes)) in [
        (beat, [0xe0, 127, 127]),
        (beat, [0xe3, 0, 80]),
        (2 * beat, [0xe0, 0, 32]),
        (3 * beat, [0xe0, 0, 64]),
    ]
    .into_iter()
    .enumerate()
    {
        messages.push(ChannelMessage {
            tick,
            bytes,
            len: 3,
            source_order: 1000 + i as u32,
        });
    }
    messages.sort_by_key(|m| (m.tick, m.source_order));
    Ok(compiled)
}

fn program() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(dir.path().join("saw.wav"), spec).unwrap();
    for i in 0..96000 {
        writer.write_sample((i % 100) as f32 / 500.).unwrap();
    }
    writer.finalize().unwrap();
    std::fs::write(
        dir.path().join("lead.sfz"),
        "<global> ampeg_release=0.001 bend_up=1200 bend_down=-100\n\
         <region> sample=saw.wav pitch_keycenter=60\n",
    )
    .unwrap();
    dir
}

fn render(session: &muz::Session) -> Vec<u32> {
    let checked = muz::description::ValidatedSession::new(session).unwrap();
    let mut engine = muz::audio::AudioEngine::from_validated(
        &checked,
        muz::audio::AudioConfig {
            sample_rate: 48000.0,
            max_frames: 256,
            offline: true,
        },
    )
    .unwrap();
    engine.set_running(true);
    let mut out = Vec::new();
    let mut block = vec![0.0; 512];
    while out.len() < 2 * 48000 * 2 {
        engine.render_interleaved(&mut block, 2).unwrap();
        out.extend(block.iter().map(|s| s.to_bits()));
    }
    out
}

/// Every bend message as (tick, channel, semitones).
fn bends(compiled: &muz::compile::Compiled) -> Vec<(u64, u8, f64)> {
    let track = &compiled.session.tracks[0];
    let TrackSource::Midi(source) = &track.source;
    source
        .imported
        .messages
        .iter()
        .filter_map(|m| track.bend(m).map(|(c, s)| (m.tick, c, s)))
        .collect()
}

#[test]
fn a_declared_range_reads_bends_as_semitones() {
    let dir = tempfile::tempdir().unwrap();
    let compiled = compile(
        dir.path(),
        r#"synth("bell", {bend_range: {up: 2, down: 12}})"#,
    )
    .unwrap();
    let track = &compiled.session.tracks[0];
    assert_eq!(
        track.instrument.bend_range,
        Some(BendRange { up: 2., down: 12. })
    );
    let found = bends(&compiled);
    let beat = found[0].0;
    // bend(1) is exactly `up`, bend(-0.5) half of `down`.
    assert_eq!(
        found,
        [
            (beat, 0, 2.),
            (beat, 3, 2048. / 8191. * 2.),
            (2 * beat, 0, -6.),
            (3 * beat, 0, 0.),
        ]
    );
    assert_eq!(track.bend_at(0, 0), Some(0.));
    assert_eq!(track.bend_at(0, beat - 1), Some(0.));
    assert_eq!(track.bend_at(0, beat), Some(2.));
    assert_eq!(track.bend_at(0, 2 * beat + 7), Some(-6.));
    assert_eq!(track.bend_at(0, 10 * beat), Some(0.));
    assert!((track.bend_at(3, 2 * beat).unwrap() - 0.5).abs() < 1e-3);
    assert_eq!(track.bend_at(5, 2 * beat), Some(0.));

    // A plain number is the same range both ways.
    let both = compile(dir.path(), r#"synth("bell", {bend_range: 1.5})"#).unwrap();
    assert_eq!(
        both.session.tracks[0].instrument.bend_range,
        Some(BendRange { up: 1.5, down: 1.5 })
    );

    // Without a declaration there is nothing to read the messages against.
    let bare = compile(dir.path(), r#"synth("bell")"#).unwrap();
    assert_eq!(bare.session.tracks[0].instrument.bend_range, None);
    assert!(bends(&bare).is_empty());
    assert_eq!(bare.session.tracks[0].bend_at(0, beat), None);

    // Purely descriptive: same fingerprint, same session apart from the field.
    assert_eq!(
        muz::fingerprint::tap(&bare.session, "lead", 48000).unwrap(),
        muz::fingerprint::tap(&compiled.session, "lead", 48000).unwrap()
    );
    let json = serde_json::to_value(&compiled.session).unwrap();
    assert_eq!(
        json["tracks"][0]["instrument"]["bend_range"],
        serde_json::json!({"up": 2.0, "down": 12.0})
    );
    assert!(
        serde_json::to_value(&bare.session).unwrap()["tracks"][0]["instrument"]
            .get("bend_range")
            .is_none()
    );

    // Inspection: the graph lists the range, message rows carry semitones.
    let graph =
        muz::inspect::page_session(&compiled.session, 0, "graph", &Default::default()).unwrap();
    let row = graph.rows.iter().find(|r| r["kind"] == "track").unwrap();
    assert_eq!(
        row["detail"]["instrument"]["bend_range"],
        serde_json::json!({"up": 2.0, "down": 12.0})
    );
    let page = muz::inspect::page_session(&compiled.session, 0, "performance", &Default::default())
        .unwrap();
    let semitones: Vec<f64> = page
        .rows
        .iter()
        .filter_map(|r| r.get("bend_semitones").and_then(|v| v.as_f64()))
        .collect();
    assert_eq!(semitones.len(), 4);
    assert!(semitones.contains(&-6.));

    // Instrument state writes the declaration back to source.
    let state = muz::device_state::DeviceState::from_device(
        muz::device_state::DeviceRole::Instrument,
        compiled.session.tracks[0].instrument.clone(),
    )
    .unwrap();
    assert_eq!(
        state.device.bend_range,
        Some(BendRange { up: 2., down: 12. })
    );
}

#[test]
fn an_sfz_program_exports_its_bend_range_and_a_declaration_replaces_it_without_changing_audio() {
    let dir = program();
    let exported = compile(dir.path(), r#"sfz("lead.sfz")"#).unwrap();
    assert_eq!(
        exported.session.tracks[0].instrument.bend_range,
        Some(BendRange { up: 12., down: 1. })
    );
    assert_eq!(bends(&exported)[2].2, -0.5);

    let declared = compile(dir.path(), r#"sfz("lead.sfz", {bend_range: 2})"#).unwrap();
    assert_eq!(
        declared.session.tracks[0].instrument.bend_range,
        Some(BendRange { up: 2., down: 2. })
    );

    // The program still bends by its own opcodes: bit-identical audio and
    // equal fingerprints with and without the declaration.
    let audio = render(&exported.session);
    assert!(audio.iter().any(|s| *s != 0));
    assert_eq!(audio, render(&declared.session));
    // The bends do reach the program: without them it sounds different.
    let mut unbent = exported.session.clone();
    let TrackSource::Midi(source) = &mut unbent.tracks[0].source;
    source.imported.messages.clear();
    assert_ne!(audio, render(&unbent));
    assert_eq!(
        muz::fingerprint::tap(&exported.session, "lead", 48000).unwrap(),
        muz::fingerprint::tap(&declared.session, "lead", 48000).unwrap()
    );

    // Regions that disagree export nothing.
    std::fs::write(
        dir.path().join("mixed.sfz"),
        "<region> sample=saw.wav key=60 bend_up=200\n<region> sample=saw.wav key=62 bend_up=1200\n",
    )
    .unwrap();
    let mixed = compile(dir.path(), r#"sfz("mixed.sfz")"#).unwrap();
    assert_eq!(mixed.session.tracks[0].instrument.bend_range, None);
}

#[test]
fn bend_range_mistakes_are_compile_errors() {
    let dir = tempfile::tempdir().unwrap();
    let error = |instrument: &str| format!("{:#}", compile(dir.path(), instrument).unwrap_err());
    let negative = error(r#"synth("bell", {bend_range: -2})"#);
    assert!(
        negative.contains("bend_range must be a number of semitones 0..127"),
        "{negative}"
    );
    let up = error(r#"synth("bell", {bend_range: {up: 200, down: 2}})"#);
    assert!(
        up.contains("bend_range up must be a number of semitones 0..127"),
        "{up}"
    );
    let missing = error(r#"synth("bell", {bend_range: {up: 2}})"#);
    assert!(
        missing.contains("bend_range needs both up and down"),
        "{missing}"
    );
    let field = error(r#"synth("bell", {bend_range: {up: 2, down: 2, cents: 1}})"#);
    assert!(
        field.contains("bend_range has unknown field 'cents'"),
        "{field}"
    );

    let path = dir.path().join("fx.muz");
    std::fs::write(
        &path,
        r#"song({tracks: [track("x", note(60, 1b), synth("bell"), {chain: [fx("gain", {bend_range: 2})]})]})"#,
    )
    .unwrap();
    let effect = format!("{:#}", muz::compile::compile(&path).unwrap_err());
    assert!(
        effect.contains("bend_range belongs on an instrument"),
        "{effect}"
    );
}
