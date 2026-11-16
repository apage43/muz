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

#[test]
fn freezes_store_compactly_stay_exact_and_old_wav_ones_compact_in_place() {
    use muz::freeze::FreezeState::*;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let session = compile(dir.path(), SONG);
    let original = dir.path().join("original.wav");
    render(session.clone(), &original, None);
    let mixed = |name: &str| {
        let mut applied = session.clone();
        muz::freeze::apply(&mut applied, &cache, 48_000).unwrap();
        let out = dir.path().join(name);
        render(applied, &out, None);
        samples(&out)
    };
    let states = |s: &muz::Session| {
        muz::freeze::status(s, &cache, 48_000).unwrap().into_iter().map(|t| (t.state, t.audio)).collect::<Vec<_>>()
    };
    muz::freeze::freeze(&session, &cache, &["lead".into()], &RenderOptions::default()).unwrap();
    let ext = muz::audio_file::COMPACT_EXTENSION;
    let written = cache.join(format!("lead.{ext}"));
    assert_eq!(states(&session)[0], (Fresh, written.clone()));
    assert!(mixed("compact.wav") == samples(&original), "a compact freeze plays the mix exactly");
    if ext == "wv" {
        // A freeze as builds without WavPack write it: a WAV tap, same record.
        let wav = cache.join("lead.wav");
        let tap = render(session.clone(), &wav, Some("lead"));
        std::fs::remove_file(&written).unwrap();
        assert_eq!(tap.latency_samples, 240);
        assert_eq!(states(&session)[0], (Fresh, wav.clone()), "a WAV freeze stays fresh");
        assert!(mixed("legacy.wav") == samples(&original));
        let done = muz::freeze::compact(&cache).unwrap();
        assert_eq!(done.len(), 1);
        assert!(done[0].after_bytes < done[0].before_bytes);
        assert!(!wav.exists());
        assert_eq!(states(&session)[0], (Fresh, written.clone()), "compacting keeps it fresh");
        assert!(mixed("compacted.wav") == samples(&original));
        assert!(muz::freeze::compact(&cache).unwrap().is_empty(), "nothing left to compact");
    }
    assert!(muz::freeze::remove(&cache, "lead").unwrap());
    assert_eq!(states(&session)[0].0, Missing);
    assert!(!muz::freeze::remove(&cache, "lead").unwrap());
    assert!(std::fs::read_dir(&cache).unwrap().all(|e| {
        let name = e.unwrap().file_name();
        name == "cpu.json"
    }));
}

#[test]
fn renders_write_wavpack_by_extension() {
    if muz::audio_file::COMPACT_EXTENSION != "wv" {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let session = compile(dir.path(), SONG);
    let wav = dir.path().join("mix.wav");
    let wv = dir.path().join("mix.wv");
    render(session.clone(), &wav, None);
    render(session, &wv, None);
    assert!(std::fs::metadata(&wv).unwrap().len() < std::fs::metadata(&wav).unwrap().len());
    let (_, decoded) = muz::audio_file::load(&wv, usize::MAX).unwrap();
    let flat: Vec<f32> = decoded.into_iter().flatten().collect();
    assert!(flat == samples(&wav));
    let copy = dir.path().join("copy.wav");
    muz::audio_file::reencode(&wv, &copy).unwrap();
    assert!(samples(&copy) == samples(&wav), "WavPack re-encodes back to the same WAV samples");
}

/// A graph where the frozen tracks read a sidechain (itself through a rack
/// branch), send into latency-bearing buses and share automation with the
/// rest of the mix, which the freeze pass must not need.
const WIDE: &str = r#"song({tempo: 120, tracks: [
    track("kick", phrase("C2:q r:q C2:q r:q").repeat(4), synth("init"), {chain: [fx("gain", {id: "kgain"})]}),
    track("bass", phrase("C2:h G1:h").repeat(4), synth("init"), {
        chain: [fx("compressor", {sidechain: "kick", ratio: 4, threshold_db: -30}), fx("limiter", {lookahead_ms: 3})],
        sends: {verb: -6}
    }),
    track("lead", phrase("C4:e D4:e E4:q G4:h").repeat(4), synth("bell"), {
        chain: [fx("gain", {id: "trim"})], sends: {verb: {gain: -3, pre: false}}
    }),
    track("pad", note("C3", 16b).gate(1), synth("pad"), {chain: [fx("compressor", {sidechain: "lead"})]}),
    track("hat", phrase("C5:e").repeat(32), synth("bell"))
], buses: [bus("verb", [fx("reverb", {}), fx("limiter", {lookahead_ms: 5})])],
automation: [
    automation("kick.kgain.gain_db", curve([[0b, 0], [8b, -12]], "linear")),
    automation("lead.trim.gain_db", curve([[0b, -3], [8b, -9]], "linear")),
    automation("lead.out", curve([[0b, 0], [6b, -6]], "step")),
    automation("verb.fx0.mix", curve([[0b, 0.2], [8b, 0.5]], "linear"))
],
tail: 1})"#;

#[test]
fn freezing_renders_only_what_the_taps_read_with_identical_output() {
    let dir = tempfile::tempdir().unwrap();
    let session = compile(dir.path(), WIDE);
    let taps: Vec<String> = vec!["bass".into(), "pad".into()];
    let deps = muz::fingerprint::dependencies(&session, &taps).unwrap();
    assert_eq!(deps.tracks.iter().map(String::as_str).collect::<Vec<_>>(), ["bass", "kick", "lead", "pad"]);
    assert!(deps.buses.is_empty(), "a track tap reads no bus");
    let bus = muz::fingerprint::dependencies(&session, &["verb".into()]).unwrap();
    assert_eq!(bus.tracks.iter().map(String::as_str).collect::<Vec<_>>(), ["bass", "kick", "lead"], "the reverb hears bass and lead, and bass hears kick");
    assert!(bus.buses.contains("verb"));

    // The whole graph, one tap at a time, is the reference.
    let reference: Vec<Vec<f32>> = taps
        .iter()
        .map(|t| {
            let out = dir.path().join(format!("whole-{t}.wav"));
            render(session.clone(), &out, Some(t));
            samples(&out)
        })
        .collect();
    let mut pruned = session.clone();
    muz::render::keep_tap_dependencies(&mut pruned, &taps).unwrap();
    let ids: Vec<_> = pruned.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["kick", "bass", "lead", "pad"]);
    assert!(pruned.buses[0].inserts.is_empty() && pruned.master.inserts.is_empty());
    assert!(pruned.extras.automation.iter().all(|l| !l.target.starts_with("verb.fx")));
    for (tap, expected) in taps.iter().zip(&reference) {
        let out = dir.path().join(format!("pruned-{tap}.wav"));
        render(pruned.clone(), &out, Some(tap));
        assert!(samples(&out) == *expected, "{tap} renders identically from its dependencies");
    }

    let cache = dir.path().join("cache");
    let report = muz::freeze::freeze(&session, &cache, &taps, &RenderOptions::default()).unwrap();
    let ran: Vec<_> = report.track_seconds.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(ran, ["kick", "bass", "lead", "pad"], "the hat is not rendered");
    for (tap, expected) in taps.iter().zip(&reference) {
        let (_, frames) = muz::audio_file::load(&muz::freeze::audio_path(&cache, tap), usize::MAX).unwrap();
        let got: Vec<f32> = frames.into_iter().flatten().collect();
        assert!(got == *expected, "the {tap} freeze equals its whole-graph tap");
    }
    // Streaming the freezes still reproduces the full mix.
    let original = dir.path().join("mix.wav");
    render(session.clone(), &original, None);
    let mut applied = session.clone();
    muz::freeze::apply(&mut applied, &cache, 48_000).unwrap();
    let frozen = dir.path().join("frozen.wav");
    render(applied, &frozen, None);
    assert!(samples(&frozen) == samples(&original));
}

#[test]
fn independent_taps_render_in_parallel_passes_identically() {
    let dir = tempfile::tempdir().unwrap();
    let session = compile(dir.path(), WIDE);
    let taps: Vec<String> = vec!["bass".into(), "hat".into(), "pad".into(), "kick".into()];
    let groups = muz::render::independent_tap_groups(&session, &taps).unwrap();
    assert_eq!(groups, [vec![0, 3], vec![1], vec![2]], "bass reads kick; hat and pad stand alone");
    let reference: Vec<Vec<f32>> = taps
        .iter()
        .map(|t| {
            let out = dir.path().join(format!("whole-{t}.wav"));
            render(session.clone(), &out, Some(t));
            samples(&out)
        })
        .collect();
    for threads in [1, 2, 3, 8] {
        let paths: Vec<_> = taps.iter().map(|t| dir.path().join(format!("{threads}-{t}.wv"))).collect();
        let options = RenderOptions { tap: taps.clone(), threads, ..Default::default() };
        let reports = muz::render::render_taps_parallel(&session, &paths, &options, None).unwrap();
        for ((tap, report), expected) in taps.iter().zip(&reports).zip(&reference) {
            assert_eq!(report.scope.tap, [tap.clone()]);
            let (_, frames) = muz::audio_file::load(std::path::Path::new(&report.output), usize::MAX).unwrap();
            let got: Vec<f32> = frames.into_iter().flatten().collect();
            assert!(got == *expected, "{tap} on {threads} threads");
        }
        // Each pass measured only its own tracks.
        let ran = |i: usize| reports[i].track_seconds.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>();
        assert_eq!(ran(1), ["hat"]);
        assert_eq!(ran(0), ["kick", "bass"]);
    }
    let cache = dir.path().join("cache");
    let options = RenderOptions { threads: 4, ..Default::default() };
    let progress = muz::control::RenderProgress {
        file: None,
        processed: Default::default(),
        total: Default::default(),
        cancel: Default::default(),
    };
    let frozen = muz::freeze::freeze_with(&session, &cache, &taps, &options, Some(&progress)).unwrap();
    use std::sync::atomic::Ordering::Relaxed;
    assert!(progress.total.load(Relaxed) > 3 * 48_000 * 8, "three passes over the song");
    assert_eq!(progress.processed.load(Relaxed), progress.total.load(Relaxed));
    assert!(frozen.tracks.iter().all(|t| !t.partial && t.rendered[0] == 0.0));
    let report = frozen.report;
    let mut ran: Vec<_> = report.track_seconds.iter().map(|(t, _)| t.as_str()).collect();
    ran.sort();
    assert_eq!(ran, ["bass", "hat", "kick", "lead", "pad"]);
    assert!(muz::freeze::status(&session, &cache, 48_000).unwrap().iter().filter(|s| s.state == muz::freeze::FreezeState::Fresh).count() == 4);
}

/// A 32-second line whose bar `at + 1` is `EDIT`, on a sampled instrument and
/// native effects, which a render from mid-song reproduces once the effects'
/// state from the notes before has rung out.
fn partial_song(edit: &str, delay: &str) -> String {
    partial_song_at(12, edit, delay)
}

fn partial_song_at(at: usize, edit: &str, delay: &str) -> String {
    let after = 15 - at;
    format!(
        r#"song({{tempo: 120, tracks: [
    track("lead", seq([phrase("C4:e D4:e E4:q G4:h").repeat({at}), phrase("{edit}"), phrase("C4:e D4:e E4:q G4:h").repeat({after})]), sfz("tone.sfz"), {{
        chain: [fx("compressor", {{}}), fx("limiter", {{lookahead_ms: 5}}), fx("delay", {{time_ms: 200, feedback: {delay}}})]
    }}),
    track("pad", note("C3", 64b).gate(1), synth("pad"))
], tail: 1}})"#
    )
}

fn tone_sfz(dir: &Path) {
    let mut w = hound::WavWriter::create(
        dir.join("tone.wav"),
        hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float },
    )
    .unwrap();
    for n in 0..30000 {
        w.write_sample((n as f32 * 0.05).sin() * 0.4 * (1.0 - n as f32 / 30000.0)).unwrap();
    }
    w.finalize().unwrap();
    std::fs::write(dir.join("tone.sfz"), "<region> sample=tone.wav lokey=0 hikey=127 pitch_keycenter=60 ampeg_release=0.1\n").unwrap();
}

#[test]
fn an_edit_refreezes_only_around_its_change_and_matches_a_full_render() {
    use muz::freeze::FreezeState::*;
    let dir = tempfile::tempdir().unwrap();
    tone_sfz(dir.path());
    let cache = dir.path().join("cache");
    let options = RenderOptions::default();
    let freeze = |source: &str| {
        let session = compile(dir.path(), source);
        let frozen = muz::freeze::freeze_with(&session, &cache, &["lead".into()], &options, None).unwrap();
        assert_eq!(muz::freeze::status(&session, &cache, 48_000).unwrap()[0].state, Fresh);
        (session, frozen.tracks.into_iter().next().unwrap())
    };
    let check = |session: &muz::Session, name: &str| {
        let full = dir.path().join(format!("full-{name}.wav"));
        render(session.clone(), &full, Some("lead"));
        let (_, frames) = muz::audio_file::load(&muz::freeze::audio_path(&cache, "lead"), usize::MAX).unwrap();
        let frozen: Vec<f32> = frames.into_iter().flatten().collect();
        assert!(frozen == samples(&full), "{name}: the spliced freeze equals a full render");
    };
    let (_, first) = freeze(&partial_song("C4:e D4:e E4:q G4:h", "0.3"));
    assert!(!first.partial);

    // A changed note in bar 13 (24 s): a pre-roll from 4 s with bounded
    // history, which matches the old freeze, and the render stops once the
    // delay has rung out and the output matches again.
    let (edited, outcome) = freeze(&partial_song("C4:e D4:e E4:q A4:h", "0.3"));
    assert!(outcome.partial, "{outcome:?}");
    assert!(outcome.rendered[0] > 3.0 && outcome.rendered[0] < 5.0, "{outcome:?}");
    assert!(outcome.rendered[1] < 31.0, "{outcome:?}");
    check(&edited, "note");
    // Unchanged input is fresh, and a second edit splices into the first.
    let (edited, outcome) = freeze(&partial_song("C4:e D4:e E4:q B4:h", "0.3"));
    assert!(outcome.partial);
    check(&edited, "again");

    // A change in the first seconds renders from the song start, exactly,
    // and stops once the output matches again.
    freeze(&partial_song("C4:e D4:e E4:q G4:h", "0.3"));
    let early = partial_song("C4:e D4:e E4:q G4:h", "0.3")
        .replacen("C4:e D4:e E4:q G4:h\").repeat(12)", "D4:e D4:e E4:q G4:h\").repeat(12)", 1);
    let (edited, outcome) = freeze(&early);
    assert!(outcome.partial && outcome.rendered[0] == 0.0 && outcome.rendered[1] < 30.0, "{outcome:?}");
    check(&edited, "start");

    // A voice patch's noise runs on through the whole song, so a render
    // from mid-song never matches; one from the song start does, and still
    // stops once the changed note has rung out.
    let bell = |edit: &str| partial_song(edit, "0.3").replace(r#"sfz("tone.sfz")"#, r#"synth("bell")"#);
    freeze(&bell("C4:e D4:e E4:q G4:h"));
    let (edited, outcome) = freeze(&bell("C4:e D4:e E4:q A4:h"));
    assert!(outcome.partial && outcome.rendered[0] == 0.0 && outcome.rendered[1] < 32.0, "{outcome:?}");
    check(&edited, "bell");

    // A device change re-renders everything.
    let (_, outcome) = freeze(&partial_song("C4:e D4:e E4:q G4:h", "0.9"));
    assert!(!outcome.partial, "{outcome:?}");

    // A delay that rings to the end never matches again: a full render.
    let (edited, outcome) = freeze(&partial_song("C4:e D4:e E4:q A4:h", "0.9"));
    assert!(!outcome.partial, "{outcome:?}");
    check(&edited, "ringing");
}

#[test]
fn route_lanes_on_latent_chains_land_on_the_music_they_were_written_against() {
    let dir = tempfile::tempdir().unwrap();
    // A held tone through a limiter's 5 ms lookahead (240 frames), and a bus
    // with the same latency. Each route's lane cuts it at exactly 1 s.
    let song = |route: &str| {
        format!(
            r#"song({{tempo: 120, tracks: [
    track("tone", note("C4", 6b).gate(1), synth("pad"), {{
        chain: [fx("limiter", {{lookahead_ms: 5}})], output: "grp"
    }})
], buses: [bus("grp", [fx("limiter", {{lookahead_ms: 5}})])],
automation: [automation("{route}", curve([[0b, 0], [2b, -120]], "step"))],
tail: 0}})"#
        )
    };
    for route in ["tone.out", "grp.out"] {
        let session = compile(dir.path(), &song(route));
        let out = dir.path().join("cut.wav");
        render(session, &out, None);
        let frames: Vec<f32> = samples(&out).chunks(2).map(|f| f[0].abs().max(f[1].abs())).collect();
        let cut = frames.iter().rposition(|&x| x > 1e-3).unwrap() + 1;
        assert_eq!(cut, 48_000, "{route}: the step lands at 1 s, not early by the chain's latency");
    }
}
