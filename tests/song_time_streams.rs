//! Song-time sample zones, tap fingerprints and freezing composed from them.
use muz::audio::{AudioConfig, AudioEngine};
use muz::render::{RenderOptions, render_with};
use std::path::Path;

// The lead's limiter lookahead gives its chain latency the stream must not
// repeat; the pad keeps the master's compensation nonzero.
const SONG: &str = r#"song({tempo: 120, tracks: [
    track("lead", phrase("C4:e D4:e E4:q G4:h").repeat(4), synth("bell"), {
        chain: [fx("limiter", {lookahead_ms: 5}), fx("gain", {id: "trim", gain_db: -3})],
        sends: {verb: {gain: -6, pre: false}}
    }),
    track("pad", note("C3", 16b).gate(1), synth("pad"))
], buses: [bus("verb", [fx("reverb", {})])],
automation: [
    automation("lead.trim.gain_db", curve([[0b, -3], [8b, -9]], "linear")),
    automation("lead.out", curve([[0b, 0], [6b, -6]], "step"))
],
tail: 1})"#;

fn compile(dir: &Path, source: &str) -> muz::Session {
    let path = dir.join("song.muz");
    std::fs::write(&path, source).unwrap();
    muz::compile::compile(&path).unwrap().session
}

fn samples(path: &Path) -> Vec<f32> {
    hound::WavReader::open(path).unwrap().samples::<f32>().map(Result::unwrap).collect()
}

fn render(session: muz::Session, out: &Path, tap: Option<&str>) -> muz::render::RenderReport {
    let options = RenderOptions { tap: tap.map(str::to_owned).into_iter().collect(), ..Default::default() };
    render_with(session, out, &options, None).unwrap()
}

#[test]
fn a_streamed_tap_reproduces_the_track_in_the_mix() {
    let dir = tempfile::tempdir().unwrap();
    let session = compile(dir.path(), SONG);
    let original = dir.path().join("original.wav");
    render(session.clone(), &original, None);
    let tap = dir.path().join("lead.wav");
    let latency = render(session.clone(), &tap, Some("lead")).latency_samples;
    assert_eq!(latency, 240, "the limiter's 5 ms lookahead");
    let mut streamed = session.clone();
    muz::freeze::stream_track(&mut streamed, "lead", &tap, 0.0, latency).unwrap();
    assert!(streamed.tracks[0].inserts.is_empty());
    let frozen = dir.path().join("frozen.wav");
    let report = render(streamed.clone(), &frozen, None);
    let (a, b) = (samples(&original), samples(&frozen));
    assert_eq!(a.len(), b.len());
    assert!(a == b, "a stream declaring its chain's latency reproduces the mix exactly");
    // The stream's own tap is the file it plays.
    let again = dir.path().join("again.wav");
    render(streamed, &again, Some("lead"));
    assert_eq!(samples(&again), samples(&tap));
    // Per-track processing time is reported for every physical track.
    let names: Vec<_> = report.track_seconds.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, ["lead", "pad"]);
    assert!(report.track_seconds.iter().all(|(_, s)| *s > 0.0));
    let tracks: f64 = report.track_seconds.iter().map(|(_, s)| s).sum();
    assert!(report.cpu_seconds >= tracks);
}

#[test]
fn song_time_zones_stay_aligned_through_seek_loop_and_pause() {
    let dir = tempfile::tempdir().unwrap();
    let session = compile(dir.path(), SONG);
    let tap = dir.path().join("lead.wav");
    render(session.clone(), &tap, Some("lead"));
    let reference = samples(&tap);
    let mut streamed = session;
    muz::freeze::stream_track(&mut streamed, "lead", &tap, 0.0, 0).unwrap();
    let mut engine = AudioEngine::new(
        &streamed,
        AudioConfig { sample_rate: 48_000.0, max_frames: 256, offline: true },
    )
    .unwrap();
    engine.set_tap(Some("lead")).unwrap();
    assert_eq!(engine.latency_samples(), 0);
    let ppq = muz::compile::PPQ as u64;
    // At 120 bpm a beat is 24,000 frames.
    let beat = 24_000usize;
    engine.seek_ticks(3 * ppq);
    engine.set_loop(Some((2 * ppq, 5 * ppq)));
    engine.set_running(true);
    let mut out = vec![0.0; 256 * 2];
    let mut frame = 3 * beat; // the seek lies inside the loop
    let mut checked = 0;
    for block in 0..1000 {
        if block == 300 {
            engine.set_running(false);
            engine.render_interleaved(&mut out, 2).unwrap();
            assert!(out.iter().all(|x| *x == 0.0), "a paused stream is silent");
            engine.set_running(true);
        }
        engine.render_interleaved(&mut out, 2).unwrap();
        for pair in out.chunks(2) {
            assert_eq!(pair, &reference[frame * 2..frame * 2 + 2], "frame {frame}");
            frame += 1;
            if frame == 5 * beat {
                frame = 2 * beat;
            }
            checked += 1;
        }
    }
    assert!(checked > 3 * 3 * beat, "covered several loop passes");
}

#[test]
fn song_time_zones_offset_and_resample() {
    let dir = tempfile::tempdir().unwrap();
    // A 24 kHz click file placed one second into the song.
    let click = dir.path().join("click.wav");
    let mut writer = hound::WavWriter::create(
        &click,
        hound::WavSpec { channels: 2, sample_rate: 24_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float },
    )
    .unwrap();
    for i in 0..2_400 {
        let x = if i == 0 { 1.0 } else { 0.0 };
        writer.write_sample(x).unwrap();
        writer.write_sample(x).unwrap();
    }
    writer.finalize().unwrap();
    let session = compile(
        dir.path(),
        r#"song({tempo: 120, tracks: [track("ref", rest(4b), sample("click.wav", {at: 1s}))], tail: 0})"#,
    );
    assert!(session.tracks[0].instrument.streams_song_time());
    let out = dir.path().join("out.wav");
    render(session, &out, Some("ref"));
    let audio = samples(&out);
    let peak = audio.chunks(2).enumerate().max_by(|a, b| a.1[0].total_cmp(&b.1[0])).unwrap();
    assert_eq!(peak.0, 48_000);
    assert!(audio[..48_000 * 2 - 8].iter().all(|x| x.abs() < 1e-6));
}

#[test]
fn tap_fingerprints_follow_only_what_reaches_the_tap() {
    let dir = tempfile::tempdir().unwrap();
    let base = compile(dir.path(), SONG);
    let print = |s: &muz::Session, tap: &str| muz::fingerprint::tap(s, tap, 48_000).unwrap();
    let lead = print(&base, "lead");
    assert_eq!(lead, print(&base, "lead"));
    assert_ne!(lead, muz::fingerprint::tap(&base, "lead", 44_100).unwrap());
    // Another track's notes, the lead's fader and send, and the bus leave it.
    let other = compile(dir.path(), &SONG.replace("note(\"C3\", 16b)", "note(\"D3\", 16b)"));
    assert_eq!(print(&other, "lead"), lead);
    assert_ne!(print(&other, "pad"), print(&base, "pad"));
    assert_ne!(print(&other, "master"), print(&base, "master"));
    let routed = compile(dir.path(), &SONG.replace("gain: -6", "gain: -9"));
    assert_eq!(print(&routed, "lead"), lead);
    assert_ne!(print(&routed, "verb"), print(&base, "verb"));
    // Its notes, devices and their lanes change it.
    for edit in [
        SONG.replace("G4:h", "A4:h"),
        SONG.replace("lookahead_ms: 5", "lookahead_ms: 4"),
        SONG.replace("[8b, -9]", "[8b, -10]"),
    ] {
        assert_ne!(print(&compile(dir.path(), &edit), "lead"), lead);
    }
    assert!(muz::fingerprint::tap(&base, "nothing", 48_000).is_err());
}

#[test]
fn freeze_cache_uses_fresh_renders_and_reports_stale_ones() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let session = compile(dir.path(), SONG);
    let options = RenderOptions::default();
    muz::freeze::freeze(&session, &cache, &["lead".into(), "pad".into()], &options).unwrap();
    let state = |s: &muz::Session| {
        muz::freeze::status(s, &cache, 48_000)
            .unwrap()
            .into_iter()
            .map(|t| (t.track, t.state, t.cpu.is_some()))
            .collect::<Vec<_>>()
    };
    use muz::freeze::FreezeState::*;
    assert_eq!(state(&session), [("lead".into(), Fresh, true), ("pad".into(), Fresh, true)]);
    let edited = compile(dir.path(), &SONG.replace("G4:h", "A4:h"));
    assert_eq!(state(&edited), [("lead".into(), Stale, false), ("pad".into(), Fresh, true)]);
    let mut applied = edited.clone();
    let states = muz::freeze::apply(&mut applied, &cache, 48_000).unwrap();
    assert_eq!(muz::freeze::stale_warnings(&states).len(), 1);
    assert_eq!(applied.tracks[0], edited.tracks[0], "a stale freeze is not played");
    assert!(applied.tracks[1].instrument.streams_song_time());
}

#[test]
fn offline_budgets_exceed_the_live_ceiling() {
    use muz::model::{MAX_LIVE_GRAPH_BUDGET, MAX_OFFLINE_GRAPH_BUDGET, check_live_graph_budget};
    let context = muz::host::HostContext { graph_units: MAX_OFFLINE_GRAPH_BUDGET, ..Default::default() };
    let dir = tempfile::tempdir().unwrap();
    let session = compile(dir.path(), SONG);
    let engine = context.run(|| AudioEngine::new(&session, AudioConfig { sample_rate: 48_000.0, max_frames: 256, offline: true }));
    assert_eq!(engine.unwrap().graph_budget(), MAX_OFFLINE_GRAPH_BUDGET);
    let over = muz::host::HostContext { graph_units: MAX_OFFLINE_GRAPH_BUDGET + 1, ..Default::default() };
    assert!(over.run(|| AudioEngine::new(&session, AudioConfig { sample_rate: 48_000.0, max_frames: 256, offline: true })).is_err());
    assert!(check_live_graph_budget(MAX_LIVE_GRAPH_BUDGET).is_ok());
    assert!(check_live_graph_budget(MAX_LIVE_GRAPH_BUDGET + 1).is_err());
}
