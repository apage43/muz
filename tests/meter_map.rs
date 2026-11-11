//! A song's `meters` list places later meters at bar lines. Diagnostics, MIDI
//! and DAWProject export, plugin transports and `analyze` bars follow it.
use std::io::Read;

/// 4/4 for two bars, one 7/8 bar, then 4/4: bar lines at 0, 4, 8, 11.5, 15.5b.
const FORM: &str = r#"use "std/arrange" as a;
song({
    tempo: 120,
    meters: a.meter_changes([[2, [4, 4]], [1, [7, 8]], [2, [4, 4]]]),
    tracks: [track("t", phrase("C4:w C4:w C4:q. C4:q C4:w"), synth("bell"))],
    tail: 1
})"#;

fn compile(src: &str) -> anyhow::Result<(tempfile::TempDir, muz::compile::Compiled)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("song.muz");
    std::fs::write(&path, src)?;
    let compiled = muz::compile::compile(&path)?;
    Ok((dir, compiled))
}

fn at(beat: f64) -> muz::compile::Diagnostic {
    muz::compile::Diagnostic {
        severity: "warning".into(),
        code: "test".into(),
        track: "t".into(),
        beat,
        message: String::new(),
    }
}

#[test]
fn diagnostics_count_bars_through_meter_changes() {
    let (_dir, c) = compile(FORM).unwrap();
    let transport = &c.session.transport;
    assert_eq!(transport.meter(), [4, 4]);
    assert_eq!(
        transport.meter_points(),
        [(0.0, [4, 4]), (8.0, [7, 8]), (11.5, [4, 4])]
    );
    // 7/8 counts in eighths.
    assert_eq!(at(9.0).position(transport), "bar 3 beat 3 (9b)");
    assert_eq!(at(12.5).position(transport), "bar 4 beat 2 (12.5b)");
    assert_eq!(at(15.5).position(transport), "bar 5 beat 1 (15.5b)");
}

#[test]
fn a_change_at_zero_sets_the_starting_meter_and_repeats_add_no_bar_line() {
    let (_dir, c) = compile(
        r#"song({meter: [4, 4], meters: [[0b, [7, 8]], [7b, [7, 8]], [14b, [3, 4]]],
            tracks: [track("t", phrase("C4:q"), synth("bell"))], tail: 0})"#,
    )
    .unwrap();
    assert_eq!(
        c.session.transport.meter_points(),
        [(0.0, [7, 8]), (14.0, [3, 4])]
    );
}

#[test]
fn a_meter_change_inside_a_bar_is_an_error_naming_the_bar_lines() {
    let error = compile(
        r#"song({meters: [[8b, [7, 8]], [11b, [4, 4]]],
            tracks: [track("t", phrase("C4:q"), synth("bell"))], tail: 0})"#,
    )
    .unwrap_err();
    let text = format!("{error:#}");
    assert!(
        text.contains("meter change at 11b is inside bar 3 (7/8)"),
        "{text}"
    );
    let help = format!("{error:?}");
    assert!(help.contains("8b and 11.5b"), "{help}");
}

#[test]
fn midi_export_writes_a_time_signature_at_each_change() {
    let (_dir, c) = compile(FORM).unwrap();
    let document = muz::smf::export(&c.session).unwrap();
    let mut tick = 0u64;
    let mut signatures = vec![];
    for event in &document.tracks[0] {
        tick += u64::from(event.delta);
        if let muz::smf::Kind::Meta { tag: 0x58, data } = &event.kind {
            signatures.push((tick, data[0], 1u8 << data[1]));
        }
    }
    assert_eq!(
        signatures,
        [(0, 4, 4), (8 * 960, 7, 8), (11 * 960 + 480, 4, 4)]
    );
}

#[test]
fn dawproject_export_writes_time_signature_points() {
    let (dir, c) = compile(FORM).unwrap();
    let plan = muz::dawproject::Plan::new(&c, "bitwig-linux").unwrap();
    assert!(
        plan.report
            .warnings()
            .any(|e| e.code == "METER_MAP_IMPORT_UNVERIFIED")
    );
    let output = dir.path().join("song.dawproject");
    plan.write(&output, false, None).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
    let mut xml = String::new();
    archive
        .by_name("project.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(
        xml.contains(
            "<TimeSignatureAutomation id=\"meter-automation\" timeUnit=\"beats\"><Target parameter=\"meter-param\"/>\
             <TimeSignaturePoint time=\"0\" numerator=\"4\" denominator=\"4\"/>\
             <TimeSignaturePoint time=\"8\" numerator=\"7\" denominator=\"8\"/>\
             <TimeSignaturePoint time=\"11.5\" numerator=\"4\" denominator=\"4\"/>\
             </TimeSignatureAutomation></Arrangement>"
        ),
        "{xml}"
    );
}

#[test]
fn plugin_transport_reports_the_bar_and_meter_in_force() {
    let (_dir, c) = compile(FORM).unwrap();
    let mut transport = muz::audio::RuntimeTransport::from_session(48_000.0, &c.session).unwrap();
    // 9.5 quarter beats at 120 BPM.
    transport.seek_project_frame(9 * 24_000 + 12_000);
    let snapshot = transport.snapshot();
    assert_eq!(snapshot.meter, [7, 8]);
    assert_eq!(snapshot.bar_start, 8.0);
    assert_eq!(snapshot.bar_index, 2);
}

#[test]
fn analyze_song_bars_follow_the_meter_map() {
    let (dir, c) = compile(FORM).unwrap();
    let wav = dir.path().join("song.wav");
    muz::render::render(
        dir.path().join("song.muz").as_path(),
        &wav,
        None,
        None,
        &[],
        48000,
        512,
    )
    .unwrap();
    let TrackSourceTempos(tempos) = TrackSourceTempos::of(&c);
    let report = muz::analysis::analyze_with(
        &wav,
        Some(muz::analysis::PeakWindows::SongBars {
            transport: c.session.transport.clone(),
            tempos,
        }),
    )
    .unwrap();
    let starts: Vec<f64> = report["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["start_seconds"].as_f64().unwrap())
        .collect();
    assert_eq!(starts, [0.0, 2.0, 4.0, 5.75, 7.75]);
    assert_eq!(report["windows"][3]["bar"], 4);
}

struct TrackSourceTempos(Vec<muz::midi::MidiTempo>);
impl TrackSourceTempos {
    fn of(c: &muz::compile::Compiled) -> Self {
        let muz::model::TrackSource::Midi(m) = &c.session.tracks[0].source;
        Self(m.imported.tempos.clone())
    }
}
