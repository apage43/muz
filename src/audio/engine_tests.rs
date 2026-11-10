//! Synthetic checks that run-segmented automation matches the one-frame path.
use super::*;
use crate::description::{ParameterEffect, parameter_specs};
use crate::model::DeviceKind;

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;
const BLOCKS: usize = 80;

fn config() -> AudioConfig {
    AudioConfig {
        sample_rate: RATE,
        max_frames: BLOCK,
        offline: true,
    }
}

fn context(block: usize) -> ProcessContext {
    let start = (block * BLOCK) as u64;
    ProcessContext {
        frames: BLOCK,
        block_start_sample: start,
        transport: TransportSnapshot {
            sample_rate: RATE as f64,
            running: true,
            sample_position: start,
            beat_position: start as f64 * 2.0 / RATE as f64,
            current_tick: 0.0,
            project_frame: start as f64,
            bpm: 120.0,
            meter: [4, 4],
            ended: false,
        },
    }
}

/// Step lanes that change at staggered, block-unaligned times, sometimes
/// several lanes on one frame and sometimes to an unchanged value.
fn lanes(device: &str, parameters: &[(String, f32, f32)]) -> crate::compile::Extras {
    let mut extras = crate::compile::Extras::default();
    for (k, (name, a, b)) in parameters.iter().enumerate() {
        let points = (0..10)
            .map(|j| crate::compile::CurvePoint {
                seconds: if j == 0 {
                    0.0
                } else {
                    0.0071 * (k % 3) as f64 + 0.0389 * j as f64 + 1.3e-5
                },
                value: if j % 3 == 1 { *b } else { *a },
            })
            .collect();
        extras.automation.push(crate::compile::Automation {
            target: format!("{device}.{name}"),
            points,
            shape: "step".into(),
            origin: None,
        });
    }
    extras
}

fn spec_parameters(kind: DeviceKind) -> Vec<(String, f32, f32)> {
    parameter_specs(kind)
        .iter()
        .filter(|s| s.effect == ParameterEffect::Control)
        .map(|s| {
            let span = s.max - s.min;
            let (a, b) = (s.min + span * 0.3, s.min + span * 0.7);
            // Integral scalar ranges are modes and counts.
            if s.name == "mode" {
                // Studio synth percussion modes moved to voice patches.
                (s.name.to_owned(), 0.0, 2.0)
            } else if s.unit == crate::description::ParameterUnit::Scalar
                && s.min.fract() == 0.0
                && s.max.fract() == 0.0
                && span >= 2.0
            {
                (s.name.to_owned(), a.floor(), b.ceil())
            } else {
                (s.name.to_owned(), a, b)
            }
        })
        .collect()
}

fn signal(block: usize, frame: usize, channel: usize) -> f32 {
    let n = (block * BLOCK + frame) as f32;
    let burst = if (block / 6).is_multiple_of(2) {
        0.9
    } else {
        0.02
    };
    burst * (n * 0.0571 + channel as f32).sin() + 0.1 * (n * 0.31).sin() * (n * 0.0013).cos()
}

fn events(block: usize) -> ScheduledEvents {
    let mut out = ScheduledEvents::new();
    if block % 5 == 1 {
        out.push(super::super::DeviceEvent {
            offset: 37 + (block % 7) as u32 * 23,
            kind: super::super::DeviceEventKind::NoteOn {
                sample_zone: None,
                pitch: 55.0 + (block % 12) as f32,
                elapsed_frames: 0,
                note_id: block as u64,
                channel: 0,
                key: 55 + (block % 12) as u8,
                velocity: 0.4 + (block % 4) as f32 * 0.15,
            },
        });
    }
    if block % 5 == 4 {
        out.push(super::super::DeviceEvent {
            offset: 101,
            kind: super::super::DeviceEventKind::NoteOff {
                note_id: block as u64 - 3,
                channel: 0,
                key: 55 + ((block - 3) % 12) as u8,
                velocity: 0.5,
            },
        });
    }
    out
}

/// Which path processes automated frames.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Path {
    OneFrame,
    Segmented,
    Engine,
    /// No automation lanes, for checking that the lanes change the output.
    Static,
}

struct Output {
    bits: Vec<u32>,
    invariant: bool,
    calls: u64,
}

fn run(
    device: &model::Device,
    parameters: &[(String, f32, f32)],
    instrument: bool,
    sidechain: bool,
    path: Path,
) -> Output {
    let parameters = if path == Path::Static {
        &[][..]
    } else {
        parameters
    };
    let extras = lanes(device.id.as_str(), parameters);
    let mut runtime = DeviceRuntime::new(device, config(), &extras).unwrap();
    assert_eq!(runtime.automation.len(), parameters.len());
    if sidechain {
        runtime.sidechain = Some(0);
    }
    let invariant = runtime.processor.block_invariant();
    let mut out = Vec::new();
    for block in 0..BLOCKS {
        let ctx = context(block);
        let mut left = [0.0f32; BLOCK];
        let mut right = [0.0f32; BLOCK];
        let events = if instrument {
            events(block)
        } else {
            for i in 0..BLOCK {
                left[i] = signal(block, i, 0);
                right[i] = signal(block, i, 1);
            }
            ScheduledEvents::new()
        };
        if sidechain {
            let detector: Vec<f32> = (0..BLOCK)
                .map(|i| {
                    if (block * BLOCK + i) % 9000 < 1500 {
                        0.8
                    } else {
                        0.0
                    }
                })
                .collect();
            runtime.feed_detector(ctx, &detector, &detector);
        }
        match path {
            Path::Engine | Path::Static => runtime.process(ctx, &events, &mut left, &mut right),
            path => runtime.process_segments(
                ctx,
                &events,
                &mut left,
                &mut right,
                path == Path::Segmented,
            ),
        }
        .unwrap();
        out.extend(left.iter().chain(&right).map(|x| x.to_bits()));
    }
    Output {
        bits: out,
        invariant,
        calls: runtime.processor.debug_state().process_count,
    }
}

struct Case {
    name: String,
    device: model::Device,
    parameters: Vec<(String, f32, f32)>,
    instrument: bool,
    sidechain: bool,
}

fn native(kind: DeviceKind, instrument: bool) -> Case {
    let device: model::Device = serde_json::from_value(serde_json::json!({
        "id": "dev",
        "kind": kind,
        "params": {},
    }))
    .unwrap();
    Case {
        name: format!("{kind:?}"),
        parameters: spec_parameters(kind),
        device,
        instrument,
        sidechain: false,
    }
}

/// A voice patch whose parameters feed per-sample coefficient math.
const PROBE: &str = r#"track("probe", phrase("C4:q"), voice_patch("probe", {
    gain_db: -12,
    nodes: [
        {id: "cut", op: "param", value: 1800, min: 80, max: 10000},
        {id: "det", op: "param", value: 7, min: 0, max: 50},
        {id: "rel", op: "param", value: 0.2, min: 0.01, max: 2},
        {id: "res", op: "param", value: 400, min: 50, max: 4000},
        {id: "osc", op: "osc", wave: "saw", detune: "det"},
        {id: "sq", op: "osc", wave: "pulse", ratio: 0.5},
        {id: "nz", op: "noise"},
        {id: "mix", op: "sum", inputs: ["osc", "sq"]},
        {id: "filter", op: "filter", input: "mix", cutoff: "cut", q: 0.9},
        {id: "ring", op: "resonator", input: "nz", frequency: "res", decay: 0.3},
        {id: "glide", op: "slew", input: "cut", rise: 0.01, fall: 0.05},
        {id: "sh", op: "hold", input: "nz", rate_hz: 30},
        {id: "echo", op: "delay", input: "filter", seconds: 0.013, feedback: 0.4, max_seconds: 0.05, damping: "cut"},
        {id: "drv", op: "drive", input: "echo", amount: 2},
        {id: "env", op: "adsr", attack: 0.005, decay: 0.1, sustain: 0.5, release: "rel"},
        {id: "body", op: "sum", inputs: ["drv", "ring", "sh"]},
        {id: "tone", op: "filter", input: "body", cutoff: "glide", q: 0.7},
        {id: "out", op: "mul", inputs: ["tone", "env"]}
    ],
    output: "out"
}))"#;

/// Voice patches, racks and samplers come from compiled source. The returned
/// directory holds the sample asset and must outlive device preparation.
fn compiled_cases() -> (tempfile::TempDir, Vec<Case>) {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("tone.wav");
    let mut writer = hound::WavWriter::create(
        &wav,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for i in 0..12_000 {
        writer
            .write_sample((i as f32 * 0.0577).sin() * 0.5)
            .unwrap();
    }
    writer.finalize().unwrap();
    let presets = [
        "glass-lead",
        "pulse-bass",
        "pad",
        "choir",
        "bell",
        "fifths",
        "kick",
        "snare",
        "hat",
        "tom",
        "noise",
    ];
    let mut tracks: Vec<String> = presets
        .iter()
        .map(|p| format!(r#"track("{p}", phrase("C4:q"), synth("{p}"))"#))
        .collect();
    tracks.push(r#"track("smp", phrase("C4:q"), sample("tone.wav", {root: 60}))"#.into());
    tracks.push(PROBE.into());
    tracks.push(
        r#"track("rk", phrase("C4:q"), synth("bell"), {chain: [
            rack([[fx("gain")], [fx("lowpass", {cutoff_hz: 900}), fx("drive")]], {id: "lfo", mix: 0.6,
                modulate: [{target: "0.0.gain_db", base: -3, depth: 3, rate_hz: 2.5, min: -24, max: 0}]}),
            rack([[fx("lowpass")]], {id: "follow", expose: {tone: "0.0.cutoff_hz"}, tone: 1200,
                modulate: [{target: "0.0.resonance", base: 0.2, follower: 2, attack_ms: 5, release_ms: 150, min: 0.1, max: 0.9}]})
        ]})"#
            .into(),
    );
    let source = dir.path().join("cases.muz");
    std::fs::write(
        &source,
        format!("song({{tracks: [{}]}})", tracks.join(",\n")),
    )
    .unwrap();
    let session = crate::compile::compile(&source).unwrap().session;
    let mut cases = Vec::new();
    for track in &session.tracks {
        let mut device = track.instrument.clone();
        let parameters = match device.kind {
            DeviceKind::VoicePatch => {
                let mut p = vec![
                    ("gain_db".to_owned(), -9.0, -2.5),
                    ("velocity_track".to_owned(), 1.0, 0.4),
                    ("glide_ms".to_owned(), 0.0, 30.0),
                ];
                if track.id.as_str() == "probe" {
                    p.extend([
                        ("cut".to_owned(), 1800.0, 600.0),
                        ("det".to_owned(), 7.0, 20.0),
                        ("rel".to_owned(), 0.2, 0.6),
                        ("res".to_owned(), 400.0, 900.0),
                    ]);
                }
                p
            }
            _ => spec_parameters(device.kind),
        };
        device.id = model::Id::new("dev");
        cases.push(Case {
            name: format!("{:?} {}", device.kind, track.id.as_str()),
            device,
            parameters,
            instrument: true,
            sidechain: false,
        });
        for insert in &track.inserts {
            let mut device = insert.clone();
            let mut parameters = vec![
                ("mix".to_owned(), 0.6, 0.25),
                ("gain_db".to_owned(), 0.0, -4.0),
            ];
            if device.id.as_str().ends_with("follow") {
                parameters.push(("tone".to_owned(), 1200.0, 3000.0));
            }
            device.id = model::Id::new("dev");
            for sidechain in [false, true] {
                cases.push(Case {
                    name: format!("Rack {} sidechain={sidechain}", insert.id.as_str()),
                    device: device.clone(),
                    parameters: parameters.clone(),
                    instrument: false,
                    sidechain,
                });
            }
        }
    }
    (dir, cases)
}

fn cases() -> (tempfile::TempDir, Vec<Case>) {
    let (dir, mut cases) = compiled_cases();
    cases.push(native(DeviceKind::StudioSynth, true));
    for kind in [
        DeviceKind::Eq,
        DeviceKind::Bitcrusher,
        DeviceKind::Chorus,
        DeviceKind::Gate,
        DeviceKind::Reverb,
        DeviceKind::Stereo,
        DeviceKind::Lowpass,
        DeviceKind::Highpass,
        DeviceKind::Drive,
        DeviceKind::Gain,
        DeviceKind::Delay,
        DeviceKind::Compressor,
        DeviceKind::Limiter,
    ] {
        cases.push(native(kind, false));
    }
    let mut keyed = native(DeviceKind::Compressor, false);
    keyed.name.push_str(" sidechain");
    keyed.sidechain = true;
    cases.push(keyed);
    (dir, cases)
}

/// For every native device without parameter offsets, the engine's automated
/// path is bit-identical to one-frame processing. Devices that claim block
/// invariance must also match when forced to segment; the claims are listed
/// explicitly so a device that loses or gains the property is noticed.
#[test]
fn segmented_step_automation_is_bit_identical_to_one_frame_processing() {
    let mut claimed = std::collections::BTreeSet::new();
    let (_assets, cases) = cases();
    for case in cases {
        assert!(!case.parameters.is_empty(), "{}", case.name);
        let one = |path| {
            run(
                &case.device,
                &case.parameters,
                case.instrument,
                case.sidechain,
                path,
            )
        };
        let reference = one(Path::OneFrame);
        assert!(
            reference.bits != one(Path::Static).bits,
            "{}: automation lanes did not change the output",
            case.name
        );
        assert!(
            reference.bits.iter().any(|&x| f32::from_bits(x) != 0.0),
            "{} produced silence",
            case.name
        );
        let engine = one(Path::Engine);
        assert!(
            engine.bits == reference.bits,
            "{}: engine automation differs from one-frame processing",
            case.name
        );
        if reference.invariant {
            assert!(
                engine.calls * 8 < reference.calls,
                "{}: engine did not segment ({} vs {} calls)",
                case.name,
                engine.calls,
                reference.calls
            );
            let segmented = one(Path::Segmented);
            assert!(
                segmented.bits == reference.bits,
                "{}: claims block invariance but segmented output differs",
                case.name
            );
            claimed.insert(case.device.kind);
        }
    }
    assert_eq!(
        claimed,
        std::collections::BTreeSet::from(BLOCK_INVARIANT),
        "update BLOCK_INVARIANT with the verified device set"
    );
}

/// Native devices verified block-invariant by the test above.
const BLOCK_INVARIANT: [DeviceKind; 17] = [
    DeviceKind::VoicePatch,
    DeviceKind::Rack,
    DeviceKind::Sampler,
    DeviceKind::Eq,
    DeviceKind::Bitcrusher,
    DeviceKind::Chorus,
    DeviceKind::Gate,
    DeviceKind::StudioSynth,
    DeviceKind::Reverb,
    DeviceKind::Stereo,
    DeviceKind::Lowpass,
    DeviceKind::Highpass,
    DeviceKind::Drive,
    DeviceKind::Gain,
    DeviceKind::Delay,
    DeviceKind::Compressor,
    DeviceKind::Limiter,
];

/// Discovery aid: `cargo test segmentation_report -- --ignored --nocapture`
/// forces segmentation on every device and lists which ones match.
#[test]
#[ignore]
fn segmentation_report() {
    let (_assets, cases) = cases();
    for case in cases {
        let one = |path| {
            run(
                &case.device,
                &case.parameters,
                case.instrument,
                case.sidechain,
                path,
            )
            .bits
        };
        let reference = one(Path::OneFrame);
        let segmented = one(Path::Segmented);
        let first = reference.iter().zip(&segmented).position(|(a, b)| a != b);
        println!(
            "{:<40} {:?}",
            case.name,
            first.map_or("match".into(), |i| format!("differs at {i}"))
        );
    }
}

/// Render a session offline and return the master output, every track tap,
/// and the threads that ran each track's chain.
fn render_threads(
    session: &model::Session,
    threads: usize,
    blocks: usize,
) -> (Vec<u32>, Vec<Vec<std::thread::ThreadId>>) {
    let mut engine = AudioEngine::new(session, config()).unwrap();
    engine.set_render_threads(threads).unwrap();
    engine.set_running(true);
    let mut bits = Vec::new();
    let mut buffer = vec![0.0f32; BLOCK * 2];
    for _ in 0..blocks {
        engine.render_interleaved(&mut buffer, 2).unwrap();
        bits.extend(buffer.iter().map(|x| x.to_bits()));
        for track in 0..engine.track_count() {
            let (_, left, right, _) = engine.track_audio(track);
            bits.extend(
                left[..BLOCK]
                    .iter()
                    .chain(&right[..BLOCK])
                    .map(|x| x.to_bits()),
            );
        }
    }
    let threads = engine
        .tracks
        .iter()
        .map(|t| t.processed_on.clone())
        .collect();
    (bits, threads)
}

fn compile(source: &str) -> (tempfile::TempDir, model::Session) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.muz");
    std::fs::write(&path, source).unwrap();
    let session = crate::compile::compile(&path).unwrap().session;
    (dir, session)
}

/// Native tracks across three sidechain waves, sends, buses and automation
/// render bit-identically on one and four threads.
#[test]
fn parallel_offline_render_is_bit_identical_to_serial() {
    let (_dir, session) = compile(
        r#"song({
    tempo: 132,
    tracks: [
        track("kick", phrase("C2:q").repeat(8), synth("kick"), {chain: [fx("gain", {id: "key", gain_db: 0})]}),
        track("bass", phrase("C2:e G2:e").repeat(8), synth("pulse-bass"), {
            chain: [fx("compressor", {id: "duck", sidechain: "kick", threshold_db: -30, ratio: 6, attack_ms: 2, release_ms: 120})],
            sends: {room: -12}
        }),
        track("pad", phrase("C3:w G3:w"), synth("pad"), {
            chain: [fx("compressor", {id: "pump", sidechain: "bass", threshold_db: -28, ratio: 4, attack_ms: 5, release_ms: 200}), fx("chorus", {mix: 0.4})],
            sends: {room: {gain: -8, pre: true}}
        }),
        track("lead", phrase("C4:q E4:q G4:h A4:q G4:q E4:h"), synth("glass-lead"), {chain: [fx("lowpass", {id: "lp", cutoff_hz: 2500}), fx("delay", {time_ms: 90, feedback: 0.3, mix: 0.2})]}),
        track("hat", phrase("C4:e").repeat(16), synth("hat"), {sends: {room: -15}}),
        track("snare", phrase("r:q C4:q").repeat(4), synth("snare"), {chain: [fx("bitcrusher", {bits: 10, rate_hz: 16000, mix: 0.3})]}),
        track("bell", phrase("G5:h E5:h"), synth("bell"), {chain: [fx("eq", {frequency_hz: 2000, gain_db: 3, q: 0.8})]})
    ],
    buses: [bus("room", [fx("reverb", {mix: 1})])],
    automation: [
        automation("lead.out", curve([[0b, -9], [2b, -3], [5b, -6]], "step")),
        automation("lead.lp.cutoff_hz", curve([[0b, 600], [3b, 3000], [6b, 1200]], "step")),
        automation("pad.out", curve([[0b, -12], [8b, -3]], "linear")),
        automation("hat.send.room", curve([[0b, -24], [8b, -6]], "smooth"))
    ],
    tail: 0.5
})"#,
    );
    let blocks = 200;
    let (serial, serial_threads) = render_threads(&session, 1, blocks);
    assert!(serial_threads.iter().all(Vec::is_empty));
    assert!(serial.iter().any(|&x| f32::from_bits(x) != 0.0));
    let (parallel, parallel_threads) = render_threads(&session, 4, blocks);
    assert!(parallel == serial, "parallel render differs from serial");
    let main = std::thread::current().id();
    assert!(
        parallel_threads.iter().flatten().any(|&t| t != main),
        "no track ran on a worker thread"
    );
}

#[test]
fn parallel_render_threads_are_offline_only() {
    let (_dir, session) = compile(r#"song({tracks: [track("a", phrase("C4:q"), synth("bell"))]})"#);
    let mut live = AudioEngine::new(
        &session,
        AudioConfig {
            offline: false,
            ..config()
        },
    )
    .unwrap();
    assert!(live.set_render_threads(4).is_err());
    assert!(live.set_render_threads(1).is_ok());
    let mut offline = AudioEngine::new(&session, config()).unwrap();
    assert!(offline.set_render_threads(0).is_err());
    assert!(offline.set_render_threads(4).is_ok());
}

/// Render like `muz render`: main-thread callbacks are serviced between blocks.
#[cfg(feature = "desktop")]
fn render_plugins(
    session: &model::Session,
    threads: usize,
    blocks: usize,
    serial_plugins: bool,
) -> (Vec<u32>, Vec<Vec<std::thread::ThreadId>>) {
    let mut engine = AudioEngine::new(session, config()).unwrap();
    engine.set_render_threads(threads).unwrap();
    engine.set_plugins_on_render_thread(serial_plugins);
    engine.set_running(true);
    let mut bits = Vec::new();
    let mut buffer = vec![0.0f32; BLOCK * 2];
    for _ in 0..blocks {
        crate::audio::clap::service_main_thread();
        engine.render_interleaved(&mut buffer, 2).unwrap();
        bits.extend(buffer.iter().map(|x| x.to_bits()));
        for track in 0..engine.track_count() {
            let (_, left, right, _) = engine.track_audio(track);
            bits.extend(
                left[..BLOCK]
                    .iter()
                    .chain(&right[..BLOCK])
                    .map(|x| x.to_bits()),
            );
        }
    }
    let threads = engine
        .tracks
        .iter()
        .map(|t| t.processed_on.clone())
        .collect();
    (bits, threads)
}

/// Plugin tracks over three sidechain waves: several instances of one class,
/// plugin inserts, a plugin in a rack, and tones whose level follows the
/// main-thread callbacks they request while processing.
#[cfg(feature = "desktop")]
fn plugin_tracks(plugin: &crate::native::test_plugin::ThreadPlugin, pinned: &str) -> String {
    let tone = plugin.device("tone", "");
    let pinned = plugin.device("tone", pinned);
    let echo = plugin.device("echo", "");
    format!(
        r#"song({{
    tempo: 140,
    tracks: [
        track("a", phrase("C3:e r:e").repeat(8), {tone}, {{chain: [{echo}]}}),
        track("b", phrase("E3:q r:e G3:e").repeat(4), {tone}),
        track("c", phrase("C4:q E4:q").repeat(4), synth("glass-lead"), {{
            chain: [{pinned}, fx("compressor", {{sidechain: "a", threshold_db: -40, ratio: 6, attack_ms: 1, release_ms: 60}})]
        }}),
        track("d", phrase("G2:h").repeat(4), {tone}, {{
            chain: [fx("compressor", {{sidechain: "c", threshold_db: -40, ratio: 4, attack_ms: 2, release_ms: 90}})],
            sends: {{room: -9}}
        }}),
        track("e", phrase("C5:e").repeat(16), synth("bell")),
        track("f", phrase("A3:q").repeat(8), {tone}, {{
            chain: [fx("compressor", {{sidechain: "b", threshold_db: -40, ratio: 3, attack_ms: 1, release_ms: 40}})]
        }}),
        track("g", phrase("D3:q r:q").repeat(4), synth("pad"), {{chain: [RACK]}})
    ],
    buses: [bus("room", [fx("reverb", {{mix: 1}})])],
    tail: 0.5
}})"#
    )
    .replace("RACK", &format!("rack([[{echo}], [fx(\"gain\", {{gain_db: -6}})]])"))
}

/// CLAP tracks process on worker threads, with truthful thread roles and
/// main-thread callbacks serviced between blocks, bit-identically to a serial
/// render for every thread count.
#[cfg(feature = "desktop")]
#[test]
fn parallel_render_processes_clap_tracks_on_workers() {
    let plugin = crate::native::test_plugin::ThreadPlugin::build();
    let session = plugin.session(&plugin_tracks(&plugin, ""));
    let blocks = 200;
    let (serial, _) = render_plugins(&session, 1, blocks, false);
    assert!(serial.iter().any(|&x| f32::from_bits(x) != 0.0));
    let callbacks = plugin.main_thread_calls();
    assert!(callbacks > 8, "tones request a callback at every note");
    let main = std::thread::current().id();
    for threads in [2, 3, 8, 32] {
        let (parallel, ran) = render_plugins(&session, threads, blocks, false);
        assert!(parallel == serial, "{threads} threads differ from serial");
        for (track, ran) in session.tracks.iter().zip(&ran) {
            assert_eq!(ran.len(), blocks);
            // "d" is alone in the last wave and "e" is native.
            if !matches!(track.id.as_str(), "d" | "e") {
                assert!(
                    ran.iter().all(|&t| t != main),
                    "plugin track {} ran on the rendering thread",
                    track.id
                );
            }
        }
    }
    assert_eq!(plugin.main_thread_calls(), callbacks * 5);
    assert_eq!(plugin.violations(), 0);
}

/// `render_thread: true` keeps one plugin's track on the rendering thread and
/// `set_plugins_on_render_thread` keeps them all there, without changing output.
#[cfg(feature = "desktop")]
#[test]
fn plugin_tracks_can_stay_on_the_rendering_thread() {
    let plugin = crate::native::test_plugin::ThreadPlugin::build();
    let free = plugin.session(&plugin_tracks(&plugin, ""));
    let session = plugin.session(&plugin_tracks(&plugin, ", render_thread: true"));
    let blocks = 60;
    let (serial, _) = render_plugins(&free, 1, blocks, false);
    let main = std::thread::current().id();
    let (declared, ran) = render_plugins(&session, 4, blocks, false);
    assert!(declared == serial);
    for (track, ran) in session.tracks.iter().zip(&ran) {
        match track.id.as_str() {
            "c" => assert!(ran.iter().all(|&t| t == main)),
            "a" | "b" | "f" | "g" => assert!(ran.iter().all(|&t| t != main)),
            _ => {}
        }
    }
    let (all, ran) = render_plugins(&free, 4, blocks, true);
    assert!(all == serial);
    for (track, ran) in free.tracks.iter().zip(&ran) {
        if track.id.as_str() != "e" {
            assert!(
                ran.iter().all(|&t| t == main),
                "track {} left the rendering thread",
                track.id
            );
        }
    }
    assert_eq!(plugin.violations(), 0);
}

/// A plugin that waits in `process` for its main thread stalls the wave; the
/// rendering thread services it instead of deadlocking.
#[cfg(feature = "desktop")]
#[test]
fn a_wave_stalled_on_the_main_thread_is_serviced() {
    let plugin = crate::native::test_plugin::ThreadPlugin::build();
    let block = plugin.device("block", "");
    let session = plugin.session(&format!(
        r#"song({{tracks: [track("a", phrase("C3:w"), {block}), track("b", phrase("E3:w"), {block})]}})"#
    ));
    let mut engine = AudioEngine::new(&session, config()).unwrap();
    engine.set_render_threads(3).unwrap();
    engine.set_running(true);
    plugin.arm_block();
    let mut buffer = vec![0.0f32; BLOCK * 2];
    let started = std::time::Instant::now();
    engine.render_interleaved(&mut buffer, 2).unwrap();
    assert!(started.elapsed() >= WAVE_STALL);
    assert!(buffer.iter().any(|&x| x != 0.0));
    assert_eq!(plugin.violations(), 0);
}

/// Per-block bits of the master and every track tap, and how often each
/// device's processor ran, in track order (instrument, then inserts).
#[cfg(feature = "desktop")]
fn render_blocks(
    session: &model::Session,
    threads: usize,
    blocks: usize,
) -> (Vec<Vec<u32>>, Vec<u64>) {
    let mut engine = AudioEngine::new(session, config()).unwrap();
    engine.set_render_threads(threads).unwrap();
    engine.set_running(true);
    let mut out = Vec::new();
    let mut buffer = vec![0.0f32; BLOCK * 2];
    for _ in 0..blocks {
        crate::audio::clap::service_main_thread();
        engine.render_interleaved(&mut buffer, 2).unwrap();
        let mut bits: Vec<u32> = buffer.iter().map(|x| x.to_bits()).collect();
        for track in 0..engine.track_count() {
            let (_, left, right, _) = engine.track_audio(track);
            bits.extend(
                left[..BLOCK]
                    .iter()
                    .chain(&right[..BLOCK])
                    .map(|x| x.to_bits()),
            );
        }
        out.push(bits);
    }
    let counts = engine
        .tracks
        .iter()
        .flat_map(|t| std::iter::once(&t.instrument).chain(&t.inserts))
        .map(|d| d.processor.debug_state().process_count)
        .collect();
    (out, counts)
}

/// A plugin that reports `SLEEP`, a quiet `CONTINUE_IF_NOT_QUIET` block, or an
/// expired `TAIL` is not processed again until a note or input wakes it, and
/// the result is bit-identical to processing it throughout.
#[cfg(feature = "desktop")]
#[test]
fn clap_sleep_and_tail_statuses_skip_processing_without_changing_output() {
    let plugin = crate::native::test_plugin::ThreadPlugin::build();
    let song = |instrument: &str, insert: &str| {
        let (instrument, insert) = (plugin.device(instrument, ""), plugin.device(insert, ""));
        plugin.session(&format!(
            r#"song({{tempo: 120, tracks: [
                track("a", phrase("C3:q r:w E3:q r:w"), {instrument}),
                track("b", phrase("r:h G3:q r:w r:q"), {instrument}, {{chain: [{insert}]}})
            ], tail: 0}})"#
        ))
    };
    // 5.5 beats at 120 bpm.
    let blocks = 540;
    let (awake, awake_counts) = render_blocks(&song("tone", "echo"), 1, blocks);
    assert!(awake_counts.iter().all(|&count| count == blocks as u64 + 1));
    assert!(awake.iter().flatten().any(|&x| f32::from_bits(x) != 0.0));
    for (instrument, threads) in [("sleep", 1), ("quiet", 1), ("sleep", 4)] {
        let (asleep, counts) = render_blocks(&song(instrument, "tail"), threads, blocks);
        assert!(asleep == awake, "{instrument} on {threads} threads differs");
        // Notes sound for a quarter of the song; the echo tail adds 1500 frames.
        for (device, &count) in counts.iter().enumerate() {
            assert!(
                (50..300).contains(&count),
                "device {device} of {instrument} ran {count} blocks"
            );
        }
        assert!(counts[2] > counts[1], "the tail keeps the echo awake longer");
    }
    assert_eq!(plugin.violations(), 0);
}
