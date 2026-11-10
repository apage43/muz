//! Instruments that use one SFZ program share its preparation and its budget.
//! Everything here is generated: a tiny program and two short recordings.
use muz::{
    audio::{AudioConfig, AudioEngine},
    host::HostContext,
    lang::Evaluator,
};
use std::path::Path;

fn tone(path: &Path, frames: usize, step: f32) {
    let mut w = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for i in 0..frames {
        w.write_sample((i as f32 * step).sin() * 0.2).unwrap();
    }
    w.finalize().unwrap();
}

const PROGRAM: &str = "<control> set_cc1=0
<global> ampeg_attack=0.002 ampeg_release=0.03 amp_random=3 pitch_random=20 volume_oncc1=-6 volume=$GAIN
<group> seq_length=2 lokey=55 hikey=65 pitch_keycenter=60
<region> sample=a.wav seq_position=1
<region> sample=b.wav seq_position=2
<group> sw_lokey=24 sw_hikey=25 sw_default=24 lokey=66 hikey=72 pitch_keycenter=69
<region> sample=a.wav sw_last=24
<region> sample=b.wav sw_last=25
";

/// Two tracks whose instruments differ only in per-instrument options unless
/// `second` overrides the second track's instrument.
fn session(dir: &Path, second: &str) -> muz::Session {
    if !dir.join("a.wav").exists() {
        tone(&dir.join("a.wav"), 4800, 0.057);
        tone(&dir.join("b.wav"), 3600, 0.091);
        std::fs::write(dir.join("instrument.sfz"), PROGRAM).unwrap();
    }
    let path = serde_json::to_string(&dir.join("instrument.sfz").to_string_lossy()).unwrap();
    let source = format!(
        "song({{tempo:120,tail:0.2,tracks:[
            track(\"a\",stack([note(25,0.05b),note(69,0.2b,at=0.125b),note(60,0.2b,at=0.25b),note(62,0.2b,at=0.5b),note(60,0.2b,at=0.75b)]),
                sfz({path},{{defines:{{GAIN:\"-3\"}},seed:3}})),
            track(\"b\",stack([note(69,0.2b),note(60,0.2b,at=0.3b),note(60,0.2b,at=0.6b),note(71,0.2b,at=0.8b)]),
                sfz({path},{second}))]}})"
    );
    let v = Evaluator::new().source(&source).unwrap();
    muz::compile::lower(
        v.get("__result").unwrap().clone(),
        &dir.join("song.muz"),
        vec![],
    )
    .unwrap()
    .session
}
const SAME: &str = "{defines:{GAIN:\"-3\"},seed:9,max_voices:8,cc1:64,sample_budget_frames:100000}";

fn engine(context: &HostContext, session: &muz::Session, rate: f32) -> AudioEngine {
    context.run(|| {
        AudioEngine::new(
            session,
            AudioConfig {
                sample_rate: rate,
                max_frames: 256,
                offline: true,
            },
        )
        .unwrap()
    })
}
fn render(mut e: AudioEngine, threads: usize) -> Vec<u32> {
    e.set_render_threads(threads).unwrap();
    e.set_running(true);
    let mut pcm = vec![0f32; 2 * 48000];
    for chunk in pcm.chunks_mut(2 * 256) {
        e.render_interleaved(chunk, 2).unwrap();
    }
    assert!(pcm.iter().any(|x| x.abs() > 0.001));
    pcm.iter().map(|x| x.to_bits()).collect()
}
fn instances(e: &AudioEngine) -> Vec<usize> {
    e.device_sfz_statistics()
        .iter()
        .map(|(_, s)| s.program_instances)
        .collect()
}

#[test]
fn identical_programs_share_one_prepared_program_and_render_bit_identically() {
    let dir = tempfile::tempdir().unwrap();
    let shared_session = session(dir.path(), SAME);
    // A label changes nothing audible, but makes the second program distinct,
    // which forces today's per-track independent preparation.
    let mut independent_session = shared_session.clone();
    independent_session.tracks[1]
        .instrument
        .sfz
        .as_mut()
        .unwrap()
        .program
        .labels
        .insert(900, "independent".into());

    let context = HostContext::default();
    let shared = engine(&context, &shared_session, 48000.);
    assert_eq!(context.sfz_programs.live_programs(), 1);
    assert_eq!(instances(&shared), [2, 2]);
    let stats = shared.device_sfz_statistics();
    assert_eq!(stats[0].1.max_voices, 256);
    assert_eq!(stats[1].1.max_voices, 8);
    assert_eq!(stats[1].1.max_sample_frames, 100000);
    assert_eq!(stats[0].1.decoded_frames, 8400);

    let other = HostContext::default();
    let independent = engine(&other, &independent_session, 48000.);
    assert_eq!(other.sfz_programs.live_programs(), 2);
    assert_eq!(instances(&independent), [1, 1]);

    let expected = render(independent, 1);
    assert_eq!(render(shared, 1), expected);
    // Parallel waves read the shared program from several threads.
    assert_eq!(
        render(engine(&context, &shared_session, 48000.), 2),
        expected
    );
    assert_eq!(render(engine(&other, &independent_session, 48000.), 2), expected);
    // Nothing owns the programs once their instruments are gone.
    assert_eq!(context.sfz_programs.live_programs(), 0);
    assert_eq!(other.sfz_programs.live_programs(), 0);
}

#[test]
fn load_time_inputs_select_distinct_programs_and_changed_recordings_are_reprepared() {
    let dir = tempfile::tempdir().unwrap();
    let context = HostContext::default();
    let same = session(dir.path(), SAME);
    let first = engine(&context, &same, 48000.);
    // A second preparation of the unchanged session reuses the live program.
    let again = engine(&context, &same, 48000.);
    assert_eq!(context.sfz_programs.live_programs(), 1);
    assert_eq!(instances(&again), [4, 4]);
    drop(again);
    // The sample rate is baked into prepared regions.
    let resampled = engine(&context, &same, 44100.);
    assert_eq!(context.sfz_programs.live_programs(), 2);
    assert_eq!(instances(&resampled), [2, 2]);
    drop(resampled);
    // A define changes the parsed program.
    let defined = engine(
        &context,
        &session(dir.path(), "{defines:{GAIN:\"-9\"}}"),
        48000.,
    );
    assert_eq!(instances(&defined), [3, 1]);
    assert_eq!(context.sfz_programs.live_programs(), 2);
    drop(defined);
    // A too-small sample budget is still each instrument's own error.
    assert!(
        context
            .run(|| AudioEngine::new(
                &session(
                    dir.path(),
                    "{defines:{GAIN:\"-3\"},sample_budget_frames:8399}"
                ),
                AudioConfig {
                    sample_rate: 48000.,
                    max_frames: 256,
                    offline: true,
                },
            ))
            .is_err()
    );

    // Rewriting a recording while the old program is still in use must not
    // hand the old audio to a new preparation.
    tone(&dir.path().join("a.wav"), 5000, 0.031);
    let changed = engine(&context, &session(dir.path(), SAME), 48000.);
    assert_eq!(instances(&first), [2, 2]);
    assert_eq!(instances(&changed), [2, 2]);
    assert_eq!(context.sfz_programs.live_programs(), 2);
    assert_eq!(changed.device_sfz_statistics()[0].1.decoded_frames, 8600);
    assert_ne!(render(changed, 1), render(first, 1));
    assert_eq!(context.sfz_programs.live_programs(), 0);
}

#[test]
fn graph_budget_charges_an_identical_program_once_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut two = session(dir.path(), SAME);
    // The reported program's scale: 12,427 regions over 2,278 recordings with
    // 14 modulation declarations each.
    let config = two.tracks[0].instrument.sfz.as_mut().unwrap();
    let mut region = config.program.regions[0].clone();
    for cc in 0..14 {
        region.opcodes.insert(format!("volume_oncc{cc}"), "1".into());
    }
    config.program.regions = (0..12427)
        .map(|i| {
            let mut region = region.clone();
            region.sample = Some(format!("take{}.wav", i % 2278).into());
            region
        })
        .collect();
    let program = config.program.clone();
    let program_units = 12427 + 2278 + (12427usize * 14).div_ceil(32);
    let mut one = two.clone();
    one.tracks.truncate(1);
    let single = one.graph_resources();
    // Master bus, instrument, output route, the program, 256 voice slots.
    assert_eq!(single.units, 3 + program_units + 8);
    assert_eq!(
        (single.sfz_programs, single.sfz_instruments),
        (1, 1),
        "{single:?}"
    );

    // Five tracks, each with its own seed, voice count and controller value.
    two.tracks[1].instrument.sfz.as_mut().unwrap().program = program.clone();
    let mut five = two.clone();
    for i in 2..5 {
        let mut track = two.tracks[1].clone();
        track.id = muz::model::Id::new(format!("extra{i}"));
        track.instrument.sfz.as_mut().unwrap().seed = i as u64;
        track.instrument.params.insert("cc1".into(), i as f32);
        five.tracks.push(track);
    }
    let shared = five.graph_resources();
    let per_instance = 12427usize.div_ceil(32);
    assert_eq!(
        shared.units,
        single.units + 4 * (2 + per_instance + 1),
        "one program, then a device, a route, region slots and 8 voice slots per track"
    );
    assert_eq!((shared.sfz_programs, shared.sfz_instruments), (1, 5));
    assert_eq!(shared.sfz_regions, 12427);
    assert_eq!(shared.sfz_samples, 2278);
    assert_eq!(shared.sfz_modulation_terms, 12427 * 14);
    assert_eq!(shared.sfz_voice_slots, 256 + 4 * 8);
    assert_eq!(shared.sfz_program_costs[0].units, program_units);
    assert_eq!(
        shared.sfz_program_costs[0].units_per_additional_instrument,
        per_instance
    );
    assert!(shared.units < 24000, "{}", shared.units);

    // A program that differs is prepared, and charged, on its own.
    let mut distinct = five.clone();
    distinct.tracks[4]
        .instrument
        .sfz
        .as_mut()
        .unwrap()
        .program
        .regions[0]
        .opcodes
        .insert("tune".into(), "5".into());
    let split = distinct.graph_resources();
    assert_eq!((split.sfz_programs, split.sfz_instruments), (2, 5));
    assert_eq!(split.units, shared.units + program_units - per_instance);

    let error = HostContext {
        graph_units: 20000,
        ..Default::default()
    }
    .run(|| five.validate_graph_budget())
    .unwrap_err();
    for expected in [
        format!(
            "requires {} resource units; allowed 20000 (MUZ_GRAPH_BUDGET, maximum 65536)",
            shared.units
        ),
        "5 SFZ instruments use 1 distinct programs".into(),
        "instruments with an identical program share one prepared copy, counted once".into(),
        format!(
            "instrument.sfz, used by 5, costs {program_units} units once (12427 regions, 2278 samples, 173978 modulation terms) plus {per_instance} units for each instrument after the first"
        ),
    ] {
        assert!(error.contains(&expected), "{expected}\n{error}");
    }
    HostContext {
        graph_units: 24000,
        ..Default::default()
    }
    .run(|| five.validate_graph_budget())
    .unwrap();
    assert!(
        HostContext {
            graph_units: 65537,
            ..Default::default()
        }
        .run(|| five.validate_graph_budget())
        .unwrap_err()
        .contains("1..=65536")
    );
}
