//! An instrument's named keyswitches tell embedders which notes are technique
//! presses. They are descriptive: what plays is unchanged. The plugin case,
//! with the events a device receives, is in clap_note_params.rs.
use muz::model::{Keyswitch, TrackSource};

fn program() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, level) in [("open.wav", 0.05f32), ("mute.wav", 0.1)] {
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
        "<global> ampeg_release=0.001 lokey=40 hikey=84 sw_lokey=24 sw_hikey=26 sw_default=24\n\
         <group> sw_last=24 sw_label=Open String\n<region> sample=open.wav\n\
         <group> sw_last=25 sw_label=Palm Mute\n<region> sample=mute.wav\n\
         <group> sw_last=26\n<region> sample=mute.wav\n",
    )
    .unwrap();
    dir
}

fn compile(dir: &std::path::Path, instrument: &str) -> anyhow::Result<muz::compile::Compiled> {
    let path = dir.join("song.muz");
    std::fs::write(
        &path,
        format!(
            r#"song({{tempo: 120, tail: 0, tracks: [track("lead",
    stack([note(25, 0.25b), phrase("r:e C4:q E4:q"), note(26, 0.25b, at = 1b), note(30, 0.25b, at = 1b)]),
    {instrument})]}})"#
        ),
    )?;
    muz::compile::compile(&path)
}

fn names(compiled: &muz::compile::Compiled) -> Vec<(u8, Option<&str>)> {
    let track = &compiled.session.tracks[0];
    let TrackSource::Midi(source) = &track.source;
    let mut names: Vec<_> = source
        .imported
        .notes
        .iter()
        .map(|n| (n.key, track.keyswitch(n)))
        .collect();
    names.sort();
    names
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

#[test]
fn an_sfz_program_names_its_labelled_switch_keys_and_declarations_add_to_them() {
    let dir = program();
    let labelled = compile(dir.path(), r#"sfz("guitar.sfz")"#).unwrap();
    assert_eq!(
        names(&labelled),
        [
            (25, Some("Palm Mute")),
            (26, None),
            (30, None),
            (60, None),
            (64, None)
        ]
    );
    let declared = compile(
        dir.path(),
        r#"sfz("guitar.sfz", {keyswitches: {mute: 25, harmonic: 26, bend: {key: 30, channel: 0}}})"#,
    )
    .unwrap();
    // A declared name replaces the program's label for the same key.
    let table = &declared.session.tracks[0].instrument.keyswitches;
    assert_eq!(
        table.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Open String", "bend", "harmonic", "mute"]
    );
    assert_eq!(
        table["bend"],
        Keyswitch {
            key: 30,
            channel: Some(0)
        }
    );
    assert_eq!(
        names(&declared),
        [
            (25, Some("mute")),
            (26, Some("harmonic")),
            (30, Some("bend")),
            (60, None),
            (64, None)
        ]
    );

    // Purely descriptive: the same audio, bit for bit, and the same fingerprint.
    let audio = render(&labelled.session);
    assert!(audio.iter().any(|s| *s != 0));
    assert_eq!(audio, render(&declared.session));
    assert_eq!(
        muz::fingerprint::tap(&labelled.session, "lead", 48000).unwrap(),
        muz::fingerprint::tap(&declared.session, "lead", 48000).unwrap()
    );

    // Instrument state regenerates the declared names; labels come back
    // from the program.
    let instrument = &declared.session.tracks[0].instrument;
    let state = muz::device_state::DeviceState::from_device(
        muz::device_state::DeviceRole::Instrument,
        instrument.clone(),
    )
    .unwrap();
    assert_eq!(&state.device.keyswitches, table);

    // Inspection names presses for agents.
    let page = muz::inspect::page_session(&declared.session, 0, "performance", &Default::default())
        .unwrap();
    let pressed: Vec<_> = page
        .rows
        .iter()
        .filter_map(|r| r.get("keyswitch").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(pressed, ["mute", "harmonic", "bend"]);
    let graph =
        muz::inspect::page_session(&declared.session, 0, "graph", &Default::default()).unwrap();
    let track = graph.rows.iter().find(|r| r["kind"] == "track").unwrap();
    assert_eq!(
        track["detail"]["instrument"]["keyswitches"]["bend"],
        serde_json::json!({"key": 30, "channel": 0})
    );
}

#[test]
fn any_instrument_declares_keyswitches() {
    let dir = tempfile::tempdir().unwrap();
    let compiled = compile(dir.path(), r#"synth("bell", {keyswitches: {stop: 26}})"#).unwrap();
    assert_eq!(names(&compiled)[1], (26, Some("stop")));
}

#[test]
fn keyswitch_mistakes_are_compile_errors() {
    let dir = tempfile::tempdir().unwrap();
    let error = |instrument: &str| format!("{:#}", compile(dir.path(), instrument).unwrap_err());
    let key = error(r#"synth("bell", {keyswitches: {stop: 130}})"#);
    assert!(
        key.contains("keyswitch 'stop': key must be an integer MIDI key 0..127"),
        "{key}"
    );
    let channel = error(r#"synth("bell", {keyswitches: {stop: {key: 20, channel: 16}}})"#);
    assert!(
        channel.contains("channel must be an integer MIDI channel 0..15"),
        "{channel}"
    );
    let field = error(r#"synth("bell", {keyswitches: {stop: {key: 20, chan: 1}}})"#);
    assert!(
        field.contains("keyswitch 'stop' has unknown field 'chan'"),
        "{field}"
    );
    let missing = error(r#"synth("bell", {keyswitches: {stop: {channel: 1}}})"#);
    assert!(
        missing.contains("keyswitch 'stop' needs a key"),
        "{missing}"
    );
    let twice = error(r#"synth("bell", {keyswitches: {a: 20, b: 20}})"#);
    assert!(
        twice.contains("'a' and 'b' name the same key and channel"),
        "{twice}"
    );

    let path = dir.path().join("fx.muz");
    std::fs::write(
        &path,
        r#"song({tracks: [track("x", note(60, 1b), synth("bell"), {chain: [fx("gain", {keyswitches: {a: 1}})]})]})"#,
    )
    .unwrap();
    let effect = format!("{:#}", muz::compile::compile(&path).unwrap_err());
    assert!(
        effect.contains("keyswitches belong on an instrument"),
        "{effect}"
    );
}
