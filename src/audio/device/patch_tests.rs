//! Synthetic checks that the prepared schedule computes what evaluating every
//! node every frame, in graph order, computes.
use super::*;

const RATE: f32 = 48_000.;
const BLOCK: usize = 96;

/// Event-rate chains (constants, parameters, frequencies, velocity and
/// expressions through every pure operation) feeding audio-rate nodes, in a
/// legato voice that glides.
const PATCH: &str = r#"voice_patch("p", {
    gain_db: -6, voice_mode: "legato", glide_ms: 3, oversample: OVERSAMPLE,
    note_controls: {lip: 0.5},
    nodes: [
        {id: "tone", op: "param", value: 0.5, min: 0, max: 1},
        {id: "f", op: "frequency"},
        {id: "t", op: "target_frequency"},
        {id: "v", op: "velocity"},
        {id: "b", op: "expression", kind: "brightness"},
        {id: "lip", op: "expression", kind: "lip"},
        {id: "k", op: "sum", inputs: [0.25, 0.5]},
        {id: "kk", op: "mul", inputs: ["k", 2, 3, 0.5]},
        {id: "cents", op: "mul", inputs: ["b", 0.01, -1]},
        {id: "ratio", op: "map", input: "cents", kind: "exp2"},
        {id: "curve", op: "shape", input: "tone", points: [[0, 0.2], [0.5, 1], [1, 0.4]], quality: "raw"},
        {id: "hz", op: "mul", inputs: ["f", "ratio", "curve"]},
        {id: "cut", op: "sum", inputs: ["t", "v", 200, "kk", "lip"]},
        {id: "lim", op: "map", input: "cut", kind: "clamp", min: 60, max: "hz"},
        {id: "q", op: "sum", inputs: ["curve", 0.3]},
        {id: "lf", op: "map", input: "f", kind: "log2"},
        {id: "rv", op: "map", input: "v", kind: "reciprocal"},
        {id: "rt", op: "map", input: "tone", kind: "sqrt"},
        {id: "ab", op: "map", input: "cents", kind: "abs"},
        {id: "amount", op: "sum", inputs: ["rt", "ab", 1]},
        {id: "osc", op: "osc", wave: "saw", hz: "hz", detune: "lf"},
        {id: "air", op: "noise"},
        {id: "aa", op: "shape", input: "lip", points: [[0, 0], [1, 1]]},
        {id: "mix", op: "sum", inputs: ["osc", "air", "aa"]},
        {id: "filt", op: "filter", input: "mix", cutoff: "lim", q: "q"},
        {id: "smooth", op: "slew", input: "rv", rise: 0.002, fall: 0.004},
        {id: "glow", op: "filter", input: "lip", cutoff: 300, q: 2},
        {id: "settle", op: "mul", inputs: ["smooth", "glow", "aa"]},
        {id: "dr", op: "drive", input: "filt", amount: "amount"},
        {id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03},
        {id: "age", op: "note_age"},
        {id: "out", op: "mul", inputs: ["dr", "env", "settle", "kk"]}
    ],
    output: "out"
})"#;

fn prepare(oversample: usize) -> VoicePatch {
    let dir = tempfile::tempdir().unwrap();
    prepare_in(
        &PATCH.replace("OVERSAMPLE", &oversample.to_string()),
        dir.path(),
    )
}

/// Prepare `patch` from a song in `dir`, where its samples are.
fn prepare_in(patch: &str, dir: &std::path::Path) -> VoicePatch {
    let source = dir.join("case.muz");
    std::fs::write(
        &source,
        format!(r#"song({{tracks: [track("p", phrase("C4:q"), {patch})]}})"#),
    )
    .unwrap();
    let session = crate::compile::compile(&source).unwrap().session;
    let device = session.tracks[0].instrument.clone();
    let crate::description::DevicePayload::VoicePatch(graph) =
        crate::description::validate_device(&device).unwrap()
    else {
        unreachable!()
    };
    let config = AudioConfig {
        sample_rate: RATE,
        max_frames: BLOCK,
        offline: true,
    };
    VoicePatch::new(&device, &graph, config, 1).unwrap()
}

/// Evaluate every node every frame, in graph order, interpreted: compiled
/// code follows the schedule it was compiled for.
fn every_node_every_frame(p: &mut VoicePatch) {
    p.compiled = None;
    p.steps.sort_by_key(|s| s.node);
    p.control = 0;
    p.settling = 0;
    p.readers = (p.steps.iter().enumerate())
        .filter_map(|(i, s)| matches!(s.op, Op::Reader { .. }).then_some(i))
        .collect();
}

fn note_on(offset: u32, note_id: u64, pitch: f32, velocity: f32) -> DeviceEvent {
    DeviceEvent {
        offset,
        kind: DeviceEventKind::NoteOn {
            sample_zone: None,
            articulation: None,
            identity: None,
            pitch,
            elapsed_frames: 0,
            note_id,
            channel: 0,
            key: pitch as u8,
            velocity,
        },
    }
}

fn expression(offset: u32, note_id: u64, expression: u8, value: f64) -> DeviceEvent {
    DeviceEvent {
        offset,
        kind: DeviceEventKind::NoteExpression {
            note_id,
            channel: 0,
            key: 60,
            expression: expression.into(),
            value,
        },
    }
}

/// Overlapping notes (legato transfers that glide), expression changes within
/// blocks, parameter changes between them, and a release.
fn render(p: &mut VoicePatch) -> Vec<u32> {
    let brightness = p.note_controls.resolve("brightness").unwrap();
    let lip = p.note_controls.resolve("lip").unwrap();
    let tuning = p.note_controls.resolve("tuning").unwrap();
    let mut out = Vec::new();
    for block in 0..120 {
        let mut events = Vec::new();
        match block {
            0 => events.push(note_on(5, 1, 60., 0.7)),
            3 => events.push(expression(17, 1, brightness, 0.8)),
            6 => events.push(note_on(40, 2, 67., 0.5)),
            8 => events.push(expression(0, 2, lip, 0.9)),
            9 => events.push(expression(50, 2, tuning, -0.3)),
            12 => events.push(note_on(1, 3, 55., 0.9)),
            20 => events.push(DeviceEvent {
                offset: 33,
                kind: DeviceEventKind::NoteOff {
                    note_id: 3,
                    channel: 0,
                    key: 55,
                    velocity: 0.5,
                },
            }),
            24 => events.push(note_on(70, 4, 62., 0.3)),
            _ => {}
        }
        if block % 7 == 5 {
            p.set_parameter("tone", (block as f32 * 0.13) % 1.).unwrap();
        }
        let ctx = ProcessContext {
            frames: BLOCK,
            block_start_sample: (block * BLOCK) as u64,
            transport: crate::audio::transport::TransportSnapshot {
                sample_rate: RATE as f64,
                running: true,
                sample_position: (block * BLOCK) as u64,
                beat_position: 0.,
                current_tick: 0.,
                project_frame: 0.,
                bpm: 120.,
                meter: [4, 4],
                bar_start: 0.,
                bar_index: 0,
                ended: false,
            },
        };
        let (mut left, mut right) = ([0f32; BLOCK], [0f32; BLOCK]);
        p.process(ctx, &events, &mut left, &mut right).unwrap();
        out.extend(left.iter().chain(&right).map(|x| x.to_bits()));
    }
    out
}

#[test]
fn event_rate_nodes_match_evaluating_every_node_every_frame() {
    for oversample in [1, 2] {
        let mut prepared = prepare(oversample);
        // Constants, parameters, note data and their pure functions run only
        // when an event changes them, and smoothers of them until they settle.
        let settling = ["aa", "smooth", "glow", "settle"];
        let audio = ["osc", "air", "mix", "filt", "dr", "env", "age", "out"];
        assert_eq!(prepared.settling - prepared.control, settling.len());
        assert_eq!(prepared.steps.len() - prepared.settling, audio.len());
        let mut reference = prepare(oversample);
        every_node_every_frame(&mut reference);
        let (a, b) = (render(&mut prepared), render(&mut reference));
        assert!(a.iter().any(|x| f32::from_bits(*x) != 0.));
        assert!(a == b, "{oversample}x: schedules differ");
        let resting = prepared
            .voices
            .iter()
            .filter(|v| v.active && v.unsettled == 0);
        assert_eq!(resting.count(), 1, "the sounding voice's smoothers settle");
    }
}

/// Every operation, at event, settling and audio rate where it can run so:
/// oscillators of each wave, envelopes, shapes, resonators, sample and zone
/// readers, maps, smoothers, filters of every mode with constant, event-rate
/// and audio-rate cutoffs, and delays (linear, cubic with fixed, event-rate
/// and audio-rate times, damped) closing feedback loops.
#[cfg(feature = "jit")]
const EVERY_OPERATION: &str = r#"voice_patch("p", {
    gain_db: -6, oversample: OVERSAMPLE, note_controls: {lip: 0.5},
    nodes: [
        {id: "tone", op: "param", value: 0.5, min: 0, max: 1},
        {id: "f", op: "frequency"},
        {id: "t", op: "target_frequency"},
        {id: "v", op: "velocity"},
        {id: "age", op: "note_age"},
        {id: "b", op: "expression", kind: "brightness"},
        {id: "lip", op: "expression", kind: "lip"},
        {id: "air", op: "noise"},
        {id: "cents", op: "mul", inputs: ["b", 0.01, -1]},
        {id: "ratio", op: "map", input: "cents", kind: "exp2"},
        {id: "lf", op: "map", input: "f", kind: "log2"},
        {id: "period", op: "map", input: "f", kind: "reciprocal"},
        {id: "half", op: "mul", inputs: ["period", 0.5]},
        {id: "sine", op: "osc", hz: "f", phase: "air"},
        {id: "saw", op: "osc", wave: "saw", ratio: "ratio", detune: "lf"},
        {id: "pulse", op: "osc", wave: "pulse", width: "tone", detune: 7, fm: "sine"},
        {id: "tri", op: "osc", wave: "triangle", ratio: 0.5},
        {id: "lfo", op: "osc", hz: 5},
        {id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03},
        {id: "hit", op: "adsr", attack: 0, decay: 0.02, sustain: 0, release: 0.01, one_shot: true},
        {id: "seg", op: "mseg", attack: [{time: 0.01, to: 1}, {time: 0.05, to: 0.4, curve: "exp"}], release: [{time: 0.02, to: 0}], sustain: 1},
        {id: "smp", op: "sample", path: "tone.wav", root: 60, loop: true, channel: "left"},
        {id: "rd", op: "reader", source: sample("tone.wav"), speed: "ratio"},
        {id: "mix", op: "sum", inputs: ["saw", "pulse", "tri", "air", "smp", "rd"]},
        {id: "mix2", op: "sum", inputs: ["sine", "lfo"]},
        {id: "mix3", op: "sum", inputs: ["mix", "mix2", 0.1]},
        {id: "copy", op: "sum", inputs: ["mix3"]},
        {id: "prod", op: "mul", inputs: ["mix", "env"]},
        {id: "prod3", op: "mul", inputs: ["prod", "seg", 2]},
        {id: "prod4", op: "mul", inputs: ["prod3", "env", "hit", 1.5]},
        {id: "abs", op: "map", input: "mix", kind: "abs"},
        {id: "rec", op: "map", input: "sine", kind: "reciprocal"},
        {id: "e2", op: "map", input: "sine", kind: "exp2"},
        {id: "l2", op: "map", input: "abs", kind: "log2"},
        {id: "sq", op: "map", input: "mix", kind: "sqrt"},
        {id: "cl", op: "map", input: "mix", kind: "clamp", min: "lfo", max: 0.5},
        {id: "dr", op: "drive", input: "mix3", amount: "tone"},
        {id: "held", op: "hold", input: "air", rate_hz: 300},
        {id: "sl", op: "slew", input: "sq", rise: 0.001, fall: "tone"},
        {id: "sl0", op: "slew", input: "mix", rise: 0, fall: 0.002},
        {id: "adaa", op: "shape", input: "mix3", points: [[-1, -0.8], [0, 0], [1, 0.9]]},
        {id: "raw", op: "shape", input: "sine", points: [[-1, 0], [1, 1]], quality: "raw"},
        {id: "res", op: "resonator", input: "air", frequency: 800, decay: 0.2},
        {id: "res2", op: "resonator", input: "air", frequency: "f", decay: "tone"},
        {id: "acut", op: "mul", inputs: ["e2", 1000]},
        {id: "lp", op: "filter", input: "mix", cutoff: 2000, q: 2},
        {id: "hp", op: "filter", input: "mix", cutoff: "f", q: "tone", mode: "highpass"},
        {id: "bp", op: "filter", input: "mix", cutoff: "acut", mode: "bandpass"},
        {id: "notch", op: "filter", input: "mix", cutoff: 1200, q: "lip", mode: "notch"},
        {id: "lp1", op: "filter", input: "mix", cutoff: "acut", mode: "lowpass1"},
        {id: "hp1", op: "filter", input: "mix", cutoff: "f", mode: "highpass1"},
        {id: "ap1", op: "filter", input: "mix", cutoff: 500, mode: "allpass1"},
        {id: "glow", op: "filter", input: "lip", cutoff: 300, q: 2},
        {id: "smooth", op: "slew", input: "period", rise: 0.002, fall: 0.004},
        {id: "aa", op: "shape", input: "lip", points: [[0, 0], [1, 1]]},
        {id: "settle", op: "mul", inputs: ["smooth", "glow", "aa", 100]},
        {id: "wobble", op: "mul", inputs: ["lfo", 0.001]},
        {id: "wob", op: "sum", inputs: [0.004, "wobble"]},
        {id: "lin", op: "delay", input: "mix", seconds: "period", feedback: 0.3},
        {id: "lin2", op: "delay", input: "into", seconds: 0.003, feedback: "tone", damping: "acut"},
        {id: "fixed", op: "delay", input: "into", seconds: 0.002, interpolation: "cubic", feedback: -0.5, damping: 3000},
        {id: "var", op: "delay", input: "into", seconds: "half", interpolation: "cubic", damping: "f", max_seconds: 0.05},
        {id: "avar", op: "delay", input: "mix", seconds: "wob", interpolation: "cubic", max_seconds: 0.01, feedback: 0.9, max_feedback: 0.99, damping: "acut"},
        {id: "back", op: "mul", inputs: ["fixed", -0.95]},
        {id: "into", op: "sum", inputs: ["back", "prod", "var", "lin2"]},
        {id: "a", op: "sum", inputs: ["copy", "prod4", "abs", "rec", "e2", "l2", "sq", "cl", "dr", "held", "sl", "sl0", "adaa", "raw", "res", "res2"]},
        {id: "c", op: "sum", inputs: ["lp", "hp", "bp", "notch", "lp1", "hp1", "ap1", "settle", "lin", "avar", "into", "age", "t", "v"]},
        {id: "ac", op: "sum", inputs: ["a", "c"]},
        {id: "out", op: "mul", inputs: ["ac", 0.01, "env"]},
        {id: "outr", op: "mul", inputs: ["c", "seg", 0.01]}
    ],
    output: {left: "out", right: "outr"}
})"#;

/// Minima, maxima and clamps of zeros of both signs at audio rate, captured
/// raw by `hold` so the sign of each result stays in the voice's state.
/// Source has no negative zero; `0 * -1` makes one.
#[cfg(feature = "jit")]
const SIGNED_ZEROS: &str = r#"voice_patch("p", {
    oversample: OVERSAMPLE, note_controls: {lip: 0.5},
    nodes: [
        {id: "tone", op: "param", value: 0.5, min: 0, max: 1},
        {id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03},
        {id: "neg", op: "mul", inputs: [0, -1]},
        {id: "p", op: "mul", inputs: [0, 1]},
        {id: "n", op: "mul", inputs: ["env", "neg"]},
        {id: "one", op: "sum", inputs: ["env", 1]},
        {id: "minus", op: "mul", inputs: ["one", -1]},
        {id: "a", op: "map", input: "one", kind: "clamp", min: "neg", max: "p"},
        {id: "b", op: "map", input: "one", kind: "clamp", min: "p", max: "neg"},
        {id: "c", op: "map", input: "minus", kind: "clamp", min: "neg", max: "p"},
        {id: "d", op: "map", input: "minus", kind: "clamp", min: "p", max: "neg"},
        {id: "e", op: "map", input: "n", kind: "clamp", min: "p", max: 1},
        {id: "f", op: "map", input: "n", kind: "sqrt"},
        {id: "g", op: "map", input: "n", kind: "abs"},
        {id: "h", op: "slew", input: "n", rise: 0, fall: 0},
        {id: "ha", op: "hold", input: "a", rate_hz: 0},
        {id: "hb", op: "hold", input: "b", rate_hz: 0},
        {id: "hc", op: "hold", input: "c", rate_hz: 0},
        {id: "hd", op: "hold", input: "d", rate_hz: 0},
        {id: "he", op: "hold", input: "e", rate_hz: 0},
        {id: "hf", op: "hold", input: "f", rate_hz: 0},
        {id: "hg", op: "hold", input: "g", rate_hz: 0},
        {id: "hh", op: "hold", input: "h", rate_hz: 0},
        {id: "out", op: "sum", inputs: ["env", "ha", "hb", "hc", "hd", "he", "hf", "hg", "hh"]}
    ],
    output: "out"
})"#;

/// A short stereo sample for the sample and zone readers.
#[cfg(feature = "jit")]
fn write_tone(dir: &std::path::Path) {
    let mut w = hound::WavWriter::create(
        dir.join("tone.wav"),
        hound::WavSpec {
            channels: 2,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for i in 0..2000 {
        let x = (i as f32 * 0.05).sin();
        w.write_sample(x).unwrap();
        w.write_sample(x * 0.5 - 0.1).unwrap();
    }
    w.finalize().unwrap();
}

/// Render `patch` compiled and interpreted, and require the same bits.
#[cfg(feature = "jit")]
fn assert_compiled_matches(patch: &str, dir: &std::path::Path) {
    let mut compiled = prepare_in(patch, dir);
    assert!(compiled.compiled.is_some(), "the patch compiles:\n{patch}");
    let mut interpreted = prepare_in(patch, dir);
    interpreted.compiled = None;
    let (a, b) = (render(&mut compiled), render(&mut interpreted));
    if let Some(i) = a.iter().zip(&b).position(|(a, b)| a != b) {
        panic!(
            "block {} sample {}: compiled {} interpreted {}\n{patch}",
            i / (2 * BLOCK),
            i % (2 * BLOCK),
            f32::from_bits(a[i]),
            f32::from_bits(b[i])
        );
    }
    // Delay lines and node state too, where the sign of a zero shows. Memos
    // are caches: compiled code may skip one whose inputs are constants.
    let state = |p: &VoicePatch| -> Vec<Vec<u64>> {
        p.voices
            .iter()
            .map(|v| {
                let nodes = v
                    .state
                    .iter()
                    .flat_map(|s| smoother_state(s).into_iter().chain([s.imaginary.to_bits()]));
                let lines = v.delay.iter().map(|x| u64::from(x.to_bits()));
                nodes.chain(lines).collect()
            })
            .collect()
    };
    assert!(
        state(&compiled) == state(&interpreted),
        "states differ\n{patch}"
    );
}

#[cfg(feature = "jit")]
#[test]
fn compiled_patches_match_the_interpreter_bit_for_bit() {
    let dir = tempfile::tempdir().unwrap();
    write_tone(dir.path());
    for oversample in [1, 2, 4] {
        for patch in [EVERY_OPERATION, SIGNED_ZEROS, PATCH] {
            let patch = patch.replace("OVERSAMPLE", &oversample.to_string());
            assert_compiled_matches(&patch, dir.path());
        }
    }
}

/// A small xorshift generator, so the graphs are the same on every run.
#[cfg(feature = "jit")]
struct Random(u64);
#[cfg(feature = "jit")]
impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// A random graph of `count` nodes: each reads earlier nodes or constants,
/// and a delay may read any node, closing loops. Constants include zeros of
/// both signs and tiny and huge values, whose products overflow.
#[cfg(feature = "jit")]
fn random_patch(r: &mut Random, count: usize, oversample: usize) -> String {
    const CONSTANTS: [&str; 14] = [
        "0",
        "-0",
        "1",
        "-1",
        "0.5",
        "-0.7",
        "2",
        "0.999",
        "440",
        "3000",
        "0.002",
        "0.000000001",
        "1000000000000000000",
        "-9000000000000000000",
    ];
    let mut nodes = vec![
        r#"{id: "tone", op: "param", value: 0.5, min: 0, max: 1}"#.to_string(),
        r#"{id: "n0", op: "sum", inputs: ["tone"]}"#.into(),
        r#"{id: "n1", op: "frequency"}"#.into(),
        r#"{id: "n2", op: "expression", kind: "lip"}"#.into(),
        r#"{id: "n3", op: "noise"}"#.into(),
        r#"{id: "n4", op: "velocity"}"#.into(),
        r#"{id: "n5", op: "adsr", attack: 0.003, decay: 0.05, sustain: 0.5, release: 0.02}"#.into(),
        r#"{id: "n6", op: "expression", kind: "brightness"}"#.into(),
    ];
    for i in nodes.len() - 1..count {
        // An earlier node or a constant; a delay's input may be any node.
        let signal = |r: &mut Random, any: bool| -> String {
            match r.below(10) {
                0..=2 => r.pick(&CONSTANTS).to_string(),
                _ if any => format!("\"n{}\"", r.below(count)),
                _ => format!("\"n{}\"", r.below(i)),
            }
        };
        let s = |r: &mut Random| signal(r, false);
        let node = match r.below(17) {
            0 | 1 => {
                let n = 1 + r.below(6);
                let inputs: Vec<_> = (0..n).map(|_| s(r)).collect();
                let op = if r.below(2) == 0 { "sum" } else { "mul" };
                format!(r#"op: "{op}", inputs: [{}]"#, inputs.join(", "))
            }
            2 => {
                let kind = r.pick(&["clamp", "abs", "reciprocal", "exp2", "log2", "sqrt"]);
                let (x, min, max) = (s(r), s(r), s(r));
                format!(r#"op: "map", input: {x}, kind: "{kind}", min: {min}, max: {max}"#)
            }
            3 => {
                let (x, amount) = (s(r), s(r));
                format!(r#"op: "drive", input: {x}, amount: {amount}"#)
            }
            4 | 5 => {
                let mode = r.pick(&[
                    "lowpass",
                    "highpass",
                    "bandpass",
                    "notch",
                    "lowpass1",
                    "highpass1",
                    "allpass1",
                ]);
                let cutoff = if r.below(2) == 0 { "800".into() } else { s(r) };
                let q = if r.below(2) == 0 { "0.7".into() } else { s(r) };
                let x = s(r);
                format!(r#"op: "filter", input: {x}, cutoff: {cutoff}, q: {q}, mode: "{mode}""#)
            }
            6 | 7 => {
                let seconds = match r.below(3) {
                    0 => "0.0021".to_string(),
                    _ => s(r),
                };
                let interpolation = r.pick(&["linear", "cubic", "cubic"]);
                let damping = match r.below(3) {
                    0 => String::new(),
                    1 => ", damping: 2500".into(),
                    _ => format!(", damping: {}", s(r)),
                };
                let (x, feedback) = (signal(r, true), s(r));
                format!(
                    r#"op: "delay", input: {x}, seconds: {seconds}, feedback: {feedback}, interpolation: "{interpolation}", max_seconds: 0.01, max_feedback: 0.99{damping}"#
                )
            }
            8 => {
                let (x, rise, fall) = (s(r), s(r), s(r));
                format!(r#"op: "slew", input: {x}, rise: {rise}, fall: {fall}"#)
            }
            9 => {
                let (x, hz) = (s(r), s(r));
                format!(r#"op: "hold", input: {x}, rate_hz: {hz}"#)
            }
            10 => {
                let (x, quality) = (s(r), r.pick(&["adaa", "raw"]));
                format!(
                    r#"op: "shape", input: {x}, points: [[-1, 0.3], [0, -0.2], [0.5, 1], [2, 0.1]], quality: "{quality}""#
                )
            }
            11 => {
                let (x, frequency, decay) = (s(r), s(r), s(r));
                format!(r#"op: "resonator", input: {x}, frequency: {frequency}, decay: {decay}"#)
            }
            12 => {
                let wave = r.pick(&["sine", "saw", "pulse", "triangle"]);
                let hz = if r.below(2) == 0 {
                    String::new()
                } else {
                    format!(", hz: {}", s(r))
                };
                let (fm, width, detune) = (s(r), s(r), s(r));
                format!(
                    r#"op: "osc", wave: "{wave}", fm: {fm}, width: {width}, detune: {detune}{hz}"#
                )
            }
            13 => {
                let (a, d, s_, r_) = (s(r), s(r), s(r), s(r));
                format!(r#"op: "adsr", attack: {a}, decay: {d}, sustain: {s_}, release: {r_}"#)
            }
            14 => r#"op: "note_age""#.into(),
            15 => r#"op: "noise""#.into(),
            _ => r#"op: "mseg", attack: [{time: 0.01, to: 1}, {time: 0.03, to: 0.3, curve: "smooth"}], release: [{time: 0.02, to: 0}], sustain: 1"#.into(),
        };
        nodes.push(format!(r#"{{id: "n{i}", {node}}}"#));
    }
    let tail: Vec<_> = (count.saturating_sub(4)..count)
        .map(|i| format!("\"n{i}\""))
        .collect();
    let mode = r.pick(&[
        r#"voice_mode: "legato", glide_ms: 2,"#,
        r#"voice_mode: "legato","#,
        "",
    ]);
    format!(
        r#"voice_patch("p", {{
    gain_db: -12, oversample: {oversample}, {mode} note_controls: {{lip: 0.5}},
    nodes: [
        {},
        {{id: "mix", op: "sum", inputs: [{}]}},
        {{id: "out", op: "mul", inputs: ["mix", "n5"]}}
    ],
    output: "out"
}})"#,
        nodes.join(",\n        "),
        tail.join(", ")
    )
}

#[cfg(feature = "jit")]
#[test]
fn random_compiled_graphs_match_the_interpreter() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Random(0x9e37_79b9_7f4a_7c15);
    for case in 0..60 {
        let oversample = [1, 2, 4][case % 3];
        let count = 10 + r.below(40);
        let patch = random_patch(&mut r, count, oversample);
        assert_compiled_matches(&patch, dir.path());
    }
}
