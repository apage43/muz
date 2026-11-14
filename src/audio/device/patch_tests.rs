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
    render_with(p, |_, _| {})
}

/// `render`, calling `between` with each block's number before it.
fn render_with(p: &mut VoicePatch, mut between: impl FnMut(&mut VoicePatch, usize)) -> Vec<u32> {
    let brightness = p.note_controls.resolve("brightness").unwrap();
    let lip = p.note_controls.resolve("lip").unwrap();
    let tuning = p.note_controls.resolve("tuning").unwrap();
    let mut out = Vec::new();
    for block in 0..120 {
        between(p, block);
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
            // A choke cuts off note 2 where it still sounds on its own.
            30 => events.push(DeviceEvent {
                offset: 11,
                kind: DeviceEventKind::NoteChoke {
                    note_id: 2,
                    channel: 0,
                    key: 67,
                },
            }),
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
        let active =
            (p.voices.iter().enumerate()).fold(0, |m, (i, v)| m | u32::from(v.active) << i);
        assert_eq!(p.sounding, active, "block {block}: the active voices");
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
/// oscillators of each wave (with phase offsets that wrap below zero and
/// land on whole cycles), envelopes (whose decays outlast `exp`, and segment
/// envelopes of every curve that sustain, release or play once), shapes (one
/// with a point for each entry of its table, and a long antialiased one),
/// resonators, sample and zone readers, maps, smoothers, filters of every
/// mode with constant, event-rate and audio-rate cutoffs, and delays (linear
/// with the shortest time, cubic with fixed, event-rate and audio-rate
/// times, damped) closing feedback loops.
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
        {id: "wrap", op: "osc", hz: "f", phase: -1},
        {id: "behind", op: "osc", wave: "pulse", hz: 300, phase: -0.25, width: 0.3},
        {id: "whole", op: "osc", wave: "triangle", hz: 400, phase: 2},
        {id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03},
        {id: "hit", op: "adsr", attack: 0, decay: 0.02, sustain: 0, release: 0.01, one_shot: true},
        {id: "seg", op: "mseg", attack: [{time: 0.01, to: 1}, {time: 0.05, to: 0.4, curve: "exp"}], release: [{time: 0.02, to: 0}], sustain: 1},
        {id: "seg2", op: "mseg", attack: [{time: 0.002, to: 1, curve: "smooth"}, {time: 0.004, to: 0.5}, {time: 0.003, to: 0.7, curve: "exp"}], release: [{time: 0.004, to: 0.2, curve: "smooth"}, {time: 0.01, to: 0, curve: "exp"}], sustain: 2},
        {id: "once", op: "mseg", attack: [{time: 0.003, to: 1}, {time: 0.01, to: 0, curve: "smooth"}], release: [{time: 0.01, to: 0}], one_shot: true},
        {id: "quick", op: "adsr", attack: 0.001, decay: 0.0001, sustain: 0.3, release: "tone"},
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
        {id: "long", op: "shape", input: "mix3", points: map(range(2049), fn(k) => [k / 1024 - 1, sin(k / 9)]), quality: "raw"},
        {id: "longa", op: "shape", input: "sine", points: map(range(300), fn(k) => [k / 150 - 1, cos(k / 5)])},
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
        {id: "lin0", op: "delay", input: "mix", seconds: 0, feedback: "lfo", max_seconds: 0.001},
        {id: "lin3", op: "delay", input: "into", seconds: "wob", feedback: 0.5, damping: "acut", max_seconds: 0.01},
        {id: "lin2", op: "delay", input: "into", seconds: 0.003, feedback: "tone", damping: "acut"},
        {id: "fixed", op: "delay", input: "into", seconds: 0.002, interpolation: "cubic", feedback: -0.5, damping: 3000},
        {id: "var", op: "delay", input: "into", seconds: "half", interpolation: "cubic", damping: "f", max_seconds: 0.05},
        {id: "avar", op: "delay", input: "mix", seconds: "wob", interpolation: "cubic", max_seconds: 0.01, feedback: 0.9, max_feedback: 0.99, damping: "acut"},
        {id: "back", op: "mul", inputs: ["fixed", -0.95]},
        {id: "into", op: "sum", inputs: ["back", "prod", "var", "lin2"]},
        {id: "a", op: "sum", inputs: ["copy", "prod4", "abs", "rec", "e2", "l2", "sq", "cl", "dr", "held", "sl", "sl0", "adaa", "raw", "res", "res2"]},
        {id: "a2", op: "sum", inputs: ["wrap", "behind", "whole", "seg2", "once", "quick", "lin0", "lin3", "long", "longa"]},
        {id: "c", op: "sum", inputs: ["lp", "hp", "bp", "notch", "lp1", "hp1", "ap1", "settle", "lin", "avar", "into", "age", "t", "v"]},
        {id: "ac", op: "sum", inputs: ["a", "c", "a2"]},
        {id: "out", op: "mul", inputs: ["ac", 0.01, "env"]},
        {id: "outr", op: "mul", inputs: ["c", "seg", 0.01]}
    ],
    output: {left: "out", right: "outr"}
})"#;

/// Minima, maxima and clamps of zeros of both signs at audio rate, and a sine
/// at a phase of -1 cycle, captured raw by `hold` so the sign of each result
/// stays in the voice's state.
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
        {id: "o", op: "osc", hz: 0, phase: -1},
        {id: "ho", op: "hold", input: "o", rate_hz: 0},
        {id: "out", op: "sum", inputs: ["env", "ha", "hb", "hc", "hd", "he", "hf", "hg", "hh", "ho"]}
    ],
    output: "out", lifetime: {envelope: "env", tail: 0.004}
})"#;

/// Quiet nodes that come to rest apart (see `jit::Quiet`): settling
/// smoothers of two expressions, an antialiased shape and their readers;
/// audio-rate envelopes, delays of the key, a hold and a segment envelope,
/// the shapes, smoothers and filters that follow them, a reader of several
/// and of a settling node, and audio-rate nodes that read them, filters and
/// a damped delay among them whose coefficients follow a cluster. The legato
/// voice glides, so the key moves while they rest.
#[cfg(feature = "jit")]
const RESTING: &str = r#"voice_patch("p", {
    gain_db: -6, voice_mode: "legato", glide_ms: 2, oversample: OVERSAMPLE,
    note_controls: {lip: 0.5},
    nodes: [
        {id: "tone", op: "param", value: 0.5, min: 0, max: 1},
        {id: "f", op: "frequency"},
        {id: "t", op: "target_frequency"},
        {id: "lip", op: "expression", kind: "lip"},
        {id: "b", op: "expression", kind: "brightness"},
        {id: "key", op: "map", input: "t", kind: "log2"},
        {id: "ls", op: "slew", input: "lip", rise: 0.003, fall: 0.006},
        {id: "bs", op: "slew", input: "b", rise: 0.01, fall: 0.002},
        {id: "lb", op: "mul", inputs: ["ls", "bs"]},
        {id: "lbf", op: "filter", input: "lb", cutoff: 200, mode: "lowpass1"},
        {id: "tq", op: "shape", input: "tone", points: [[0, 0.5], [1, 2]]},
        {id: "ready", op: "adsr", attack: 0.004, decay: 0.001, sustain: 1, release: 0.01},
        {id: "gate", op: "shape", input: "ready", points: [[0.9, 0], [1, 1]], quality: "raw"},
        {id: "stagger", op: "mul", inputs: ["tone", 0.004]},
        {id: "late", op: "delay", input: "key", seconds: 0.003, max_seconds: 0.01},
        {id: "later", op: "delay", input: "key", seconds: "stagger", max_seconds: 0.01, interpolation: "cubic"},
        {id: "back", op: "mul", inputs: ["key", -1]},
        {id: "lag", op: "sum", inputs: ["late", "back"]},
        {id: "lagg", op: "mul", inputs: ["lag", "gate"]},
        {id: "idx", op: "sum", inputs: ["key", "lagg"]},
        {id: "idx2", op: "sum", inputs: ["later", 0.1]},
        {id: "target", op: "shape", input: "idx", points: [[7, 0], [8.2, 1]], quality: "raw"},
        {id: "travel", op: "slew", input: "target", rise: 0.002, fall: 0.003},
        {id: "corner", op: "shape", input: "travel", points: [[0, 300], [1, 3000]], quality: "raw"},
        {id: "depth", op: "shape", input: "travel", points: [[0, 0], [1, 0.8]], quality: "raw"},
        {id: "target2", op: "shape", input: "idx2", points: [[8, 1], [9, 0]], quality: "raw"},
        {id: "travel2", op: "slew", input: "target2", rise: 0.004, fall: 0.001},
        {id: "both", op: "mul", inputs: ["travel", "travel2", "lbf", "idx"]},
        {id: "held", op: "hold", input: "key", rate_hz: 50},
        {id: "seg", op: "mseg", attack: [{time: 0.005, to: 1}], release: [{time: 0.01, to: 0}], sustain: 0},
        {id: "segs", op: "slew", input: "seg", rise: 0.001, fall: 0.001},
        {id: "lpq", op: "filter", input: "gate", cutoff: 500, mode: "lowpass1"},
        {id: "osc", op: "osc", wave: "saw", hz: "f", fm: "both"},
        {id: "air", op: "noise"},
        {id: "lp", op: "filter", input: "osc", cutoff: "corner", q: "tq"},
        {id: "lp1", op: "filter", input: "osc", cutoff: "corner", mode: "lowpass1"},
        {id: "bp", op: "filter", input: "air", cutoff: "corner", q: 3, mode: "bandpass"},
        {id: "echo", op: "delay", input: "osc", seconds: 0.002, damping: "corner", max_seconds: 0.01, interpolation: "cubic"},
        {id: "mix", op: "sum", inputs: ["lp", "air", "lp1", "bp", "echo"]},
        {id: "open", op: "mul", inputs: ["mix", "depth", "held", "segs", "lpq"]},
        {id: "out", op: "sum", inputs: ["open", "both", "lb", "travel2", "segs"]}
    ],
    output: "out"
})"#;

/// Signals past `jit::LIMIT` and past f32's range: a burst of noise a
/// billion times too loud in delay lines (one longer than a mirrored line),
/// filters, a resonator and a hold, so that the bounded copy of the
/// audio-rate steps waits for them to drain; products of it that overflow,
/// for guards that find values not finite; and oscillators whose frequency
/// becomes NaN, so that a phase stays NaN, while the bounded copy runs or
/// not (constants rather than inputs from other tiers keep it running).
/// Also smoothers with event-rate times (one of them zero), raw shapes of a
/// key's slew that overshoots their common domain, and filters at the ends
/// of the q range, with constant, event-rate and audio-rate q.
#[cfg(feature = "jit")]
const EXTREMES: &str = r#"voice_patch("p", {
    gain_db: -6, voice_mode: "legato", glide_ms: 1, oversample: OVERSAMPLE,
    note_controls: {lip: 0.5},
    nodes: [
        {id: "tone", op: "param", value: 0.5, min: 0, max: 1},
        {id: "lip", op: "expression", kind: "lip"},
        {id: "t", op: "target_frequency"},
        {id: "air", op: "noise"},
        {id: "burst", op: "mseg", attack: [{time: 0.002, to: 1}, {time: 0.004, to: 0}], release: [{time: 0.001, to: 0}], sustain: 1},
        {id: "loud", op: "mul", inputs: ["air", "burst", 1000000000]},
        {id: "period", op: "map", input: "t", kind: "reciprocal"},
        {id: "echo", op: "delay", input: "loud", seconds: 0.003, feedback: 0.5, max_seconds: 0.01, interpolation: "cubic"},
        {id: "long", op: "delay", input: "loud", seconds: "period", feedback: 0.2, max_seconds: 0.25},
        {id: "lp1", op: "filter", input: "loud", cutoff: 2000, mode: "lowpass1"},
        {id: "svf", op: "filter", input: "echo", cutoff: 900, q: 3, mode: "bandpass"},
        {id: "ring", op: "resonator", input: "loud", frequency: 700, decay: 0.05},
        {id: "held", op: "hold", input: "loud", rate_hz: 2000},
        {id: "huge", op: "mul", inputs: ["loud", "loud", "loud", "loud", "loud"]},
        {id: "fifth", op: "mul", inputs: ["echo", "echo", "echo", "echo", "echo"]},
        {id: "edge", op: "mul", inputs: ["ring", 1000000000000000000, 1000000000000000000, 100]},
        {id: "faster", op: "mul", inputs: ["loud", 1000000000000000000, 1000]},
        {id: "nan", op: "osc", hz: "faster", ratio: 1000000000000000000, detune: -1000000000},
        {id: "nan3", op: "osc", wave: "triangle", hz: "faster", ratio: 1000000000000000000, detune: -1000000000},
        {id: "fast", op: "mul", inputs: ["air", 1000000000000000000, 1000]},
        {id: "nan4", op: "osc", wave: "triangle", hz: "fast", ratio: 1000000000000000000, detune: -1000000000},
        {id: "after", op: "sum", inputs: ["nan", "nan3", "nan4"]},
        {id: "key", op: "map", input: "t", kind: "log2"},
        {id: "target", op: "shape", input: "key", points: [[8, -0.5], [8.6, 1.5]], quality: "raw"},
        {id: "rise", op: "mul", inputs: ["tone", 0.004]},
        {id: "now", op: "mul", inputs: ["tone", 0]},
        {id: "travel", op: "slew", input: "target", rise: "rise", fall: "lip"},
        {id: "snap", op: "slew", input: "target", rise: "now", fall: "rise"},
        {id: "corner", op: "shape", input: "travel", points: [[0, 300], [1, 3000]], quality: "raw"},
        {id: "depth", op: "shape", input: "travel", points: [[0, 0], [0.5, 0.2], [1, 0.8]], quality: "raw"},
        {id: "body", op: "filter", input: "air", cutoff: "corner", mode: "lowpass1"},
        {id: "damped", op: "filter", input: "loud", cutoff: 900, q: 0.05, mode: "notch"},
        {id: "wide", op: "filter", input: "echo", cutoff: "corner", q: "now", mode: "highpass"},
        {id: "rough", op: "filter", input: "loud", cutoff: 400, q: "air", mode: "notch"},
        {id: "sharp", op: "filter", input: "air", cutoff: 1200, q: 90, mode: "bandpass"},
        {id: "mix", op: "sum", inputs: ["echo", "long", "lp1", "svf", "ring", "held", "huge", "fifth", "edge", "after", "body", "snap", "damped", "wide", "rough", "sharp"]},
        {id: "small", op: "mul", inputs: ["mix", 0.000000001, "depth"]},
        {id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03},
        {id: "out", op: "mul", inputs: ["small", "env"]},
        {id: "clean", op: "mul", inputs: ["body", "snap", "env", 0.1]}
    ],
    output: {left: "out", right: "clean"}
})"#;

/// Filters whose arguments sweep the tangent's domain (see `tan`): a cutoff
/// at audio rate from below the floor to past 0.45 of the rate, for each
/// kind of filter, and one per event, from an expression; and a key's slew
/// read by three shapes that share a lookup, the cutoff, q and depth of a
/// filter whose coefficients are refilled where the key moves, as a tone
/// hole's are.
#[cfg(feature = "jit")]
const SWEEPS: &str = r#"voice_patch("p", {
    oversample: OVERSAMPLE, note_controls: {lip: 0.5},
    nodes: [
        {id: "t", op: "target_frequency"},
        {id: "lip", op: "expression", kind: "lip"},
        {id: "ramp", op: "osc", wave: "saw", hz: 37},
        {id: "octaves", op: "mul", inputs: ["ramp", 7.65]},
        {id: "lifted", op: "sum", inputs: ["octaves", 9.95]},
        {id: "cutoff", op: "map", input: "lifted", kind: "exp2"},
        {id: "tone", op: "param", value: 0.5, min: 0, max: 1},
        {id: "held", op: "mul", inputs: ["lip", "tone", 60000]},
        {id: "air", op: "noise"},
        {id: "lp", op: "filter", input: "air", cutoff: "cutoff", q: 0.7},
        {id: "hp", op: "filter", input: "air", cutoff: "cutoff", q: 0.01, mode: "highpass"},
        {id: "bp", op: "filter", input: "air", cutoff: "cutoff", q: 30, mode: "bandpass"},
        {id: "lp1", op: "filter", input: "air", cutoff: "cutoff", mode: "lowpass1"},
        {id: "ap", op: "filter", input: "air", cutoff: "held", mode: "allpass1"},
        {id: "key", op: "map", input: "t", kind: "log2"},
        {id: "target", op: "shape", input: "key", points: [[7.8, -0.5], [8.4, 1.5]], quality: "raw"},
        {id: "travel", op: "slew", input: "target", rise: 0.003, fall: 0.005},
        {id: "corner", op: "shape", input: "travel", points: [[0, 200], [0.3, 900], [1, 6000]], quality: "raw"},
        {id: "width", op: "shape", input: "travel", points: [[0, 0.02], [1, 0.3]], quality: "raw"},
        {id: "depth", op: "shape", input: "travel", points: [[0, 0], [1, -0.8]], quality: "raw"},
        {id: "hole", op: "filter", input: "air", cutoff: "corner", q: "width"},
        {id: "open", op: "mul", inputs: ["hole", "depth"]},
        {id: "mix", op: "sum", inputs: ["lp", "hp", "bp", "lp1", "ap", "open"]},
        {id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03},
        {id: "out", op: "mul", inputs: ["mix", "env"]}
    ],
    output: "out"
})"#;

/// Oscillators whose frequency overflows while the bounded copy of the
/// audio-rate steps runs: a constant ratio and a product of noise and
/// constants, none of them bounded by `jit::LIMIT`, and none checked, as
/// inputs from other tiers are.
#[cfg(feature = "jit")]
const OVERFLOWS: &str = r#"voice_patch("p", {
    oversample: OVERSAMPLE, note_controls: {lip: 0.5},
    nodes: [
        {id: "tone", op: "param", value: 0.5, min: 0, max: 1},
        {id: "air", op: "noise"},
        {id: "fast", op: "mul", inputs: ["air", 1000000000000000000, 1000]},
        {id: "tri", op: "osc", wave: "triangle", hz: "fast", ratio: 1000000000000000000, detune: -1000000000},
        {id: "saw", op: "osc", wave: "saw", hz: "fast", ratio: 1000000000000000000, detune: -1000000000},
        {id: "line", op: "delay", input: "mix", seconds: 0.001, feedback: 0.5, max_seconds: 0.002},
        {id: "mix", op: "sum", inputs: ["tri", "saw", "air"]},
        {id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03},
        {id: "out", op: "mul", inputs: ["mix", "env", "line"]}
    ],
    output: "out"
})"#;

/// A delay line alone holding samples past `jit::LIMIT`, then draining,
/// and a product of its reads that overflows only past the limit; then an
/// event-rate input past the limit, and a product of it that overflows.
#[cfg(feature = "jit")]
const DRAINS: &str = r#"voice_patch("p", {
    oversample: OVERSAMPLE, note_controls: {lip: 0.5},
    nodes: [
        {id: "tone", op: "param", value: 0.5, min: 0, max: 1},
        {id: "lip", op: "expression", kind: "lip"},
        {id: "lift", op: "sum", inputs: ["lip", -0.5]},
        {id: "big", op: "mul", inputs: ["lift", 1000000000000000000]},
        {id: "air", op: "noise"},
        {id: "cube", op: "mul", inputs: ["air", "big", "big", "big"]},
        {id: "burst", op: "mseg", attack: [{time: 0.002, to: 1}, {time: 0.004, to: 0}], release: [{time: 0.001, to: 0}], sustain: 1},
        {id: "loud", op: "mul", inputs: ["air", "burst", 1000000000]},
        {id: "late", op: "delay", input: "loud", seconds: 0.005, max_seconds: 0.01, interpolation: "cubic"},
        {id: "fifth", op: "mul", inputs: ["late", "late", "late", "late", "late"]},
        {id: "mix", op: "sum", inputs: ["fifth", "air", "cube"]},
        {id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03},
        {id: "out", op: "mul", inputs: ["mix", "env"]}
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

/// Render `patch` compiled and interpreted, require the same bits, and
/// return the compiled patch.
#[cfg(feature = "jit")]
fn assert_compiled_matches(patch: &str, dir: &std::path::Path) -> VoicePatch {
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
    // are caches: compiled code may skip one whose inputs are constants. So
    // are the copies after each line, which only compiled code keeps. Packs
    // keep their lanes' state in rows, which `unpack` copies to the nodes.
    if let Some(c) = &compiled.compiled {
        compiled.voices.iter_mut().for_each(|v| c.unpack(v));
    }
    let state = |p: &VoicePatch| -> Vec<Vec<u64>> {
        p.voices
            .iter()
            .map(|v| {
                let nodes = v
                    .state
                    .iter()
                    .flat_map(|s| smoother_state(s).into_iter().chain([s.imaginary.to_bits()]));
                let lines = (p.delays.iter())
                    .flat_map(|&(.., base, length)| &v.delay[base..base + length])
                    .map(|x| u64::from(x.to_bits()));
                nodes.chain(lines).collect()
            })
            .collect()
    };
    assert!(
        state(&compiled) == state(&interpreted),
        "states differ\n{patch}"
    );
    compiled
}

#[cfg(feature = "jit")]
#[test]
fn compiled_patches_match_the_interpreter_bit_for_bit() {
    let dir = tempfile::tempdir().unwrap();
    write_tone(dir.path());
    for oversample in [1, 2, 4] {
        for patch in [
            EVERY_OPERATION,
            SIGNED_ZEROS,
            PATCH,
            RESTING,
            EXTREMES,
            OVERFLOWS,
            DRAINS,
            SWEEPS,
        ] {
            let patch = patch.replace("OVERSAMPLE", &oversample.to_string());
            assert_compiled_matches(&patch, dir.path());
        }
        // Both copies of the audio-rate steps ran, and guards found values
        // that were not finite.
        let ran = jit::RAN
            .each_ref()
            .map(|n| n.load(std::sync::atomic::Ordering::Relaxed));
        assert!(ran.iter().all(|&n| n > 0), "{oversample}x: {ran:?}");
        // Quiet nodes of the sounding voice came to rest.
        let patch = RESTING.replace("OVERSAMPLE", &oversample.to_string());
        let compiled = assert_compiled_matches(&patch, dir.path());
        let resting = (compiled.voices.iter().filter(|v| v.active))
            .map(|v| v.state.iter().filter(|s| s.resting).count())
            .sum::<usize>();
        assert!(resting >= 4, "{oversample}x: {resting} resting");
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
    random_patch_with(r, count, oversample, false)
}

/// `random_patch`, where `holes` makes some filters tone holes: a key's
/// slew read by shapes that share a lookup, which give the filter's cutoff
/// and q.
#[cfg(feature = "jit")]
fn random_patch_with(r: &mut Random, count: usize, oversample: usize, holes: bool) -> String {
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
        let mut extra = Vec::new();
        // An earlier node or a constant; a delay's input may be any node.
        let signal = |r: &mut Random, any: bool| -> String {
            match r.below(10) {
                0..=2 => r.pick(&CONSTANTS).to_string(),
                _ if any => format!("\"n{}\"", r.below(count)),
                _ => format!("\"n{}\"", r.below(i)),
            }
        };
        let s = |r: &mut Random| signal(r, false);
        let node = match r.below(19) {
            // Now and then past f32's range, now and then not.
            17 | 18 => {
                let x = s(r);
                let scale = r.pick(&["1000000000000000000", "4"]);
                format!(
                    r#"op: "mul", inputs: [{x}, 1000000000000000000, 1000000000000000000, 300, {scale}]"#
                )
            }
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
                let (cutoff, q) = if holes && r.below(2) == 0 {
                    let (x, fall) = (s(r), s(r));
                    let keys = [
                        format!(r#"{{id: "n{i}k", op: "slew", input: {x}, rise: 0.002, fall: {fall}}}"#),
                        format!(r#"{{id: "n{i}c", op: "shape", input: "n{i}k", points: [[0, 150], [0.4, 900], [1, 7000]], quality: "raw"}}"#),
                        format!(r#"{{id: "n{i}q", op: "shape", input: "n{i}k", points: [[0, 0.03], [1, 3]], quality: "raw"}}"#),
                        format!(r#"{{id: "n{i}d", op: "shape", input: "n{i}k", points: [[0, 1], [1, -0.5]], quality: "raw"}}"#),
                    ];
                    extra.extend(keys);
                    (format!(r#""n{i}c""#), format!(r#""n{i}q""#))
                } else {
                    let cutoff = if r.below(2) == 0 { "800".into() } else { s(r) };
                    let q = if r.below(2) == 0 { "0.7".into() } else { s(r) };
                    (cutoff, q)
                };
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
                let phase = match (wave, r.below(3)) {
                    ("sine", 0) => format!(", phase: {}", s(r)),
                    (_, 1) => format!(", phase: {}", r.pick(&["-1", "-0.25", "0.5", "3"])),
                    _ => String::new(),
                };
                format!(
                    r#"op: "osc", wave: "{wave}", fm: {fm}, width: {width}, detune: {detune}{hz}{phase}"#
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
        nodes.extend(extra);
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
fn a_patch_that_cannot_compile_runs_interpreted() {
    jit::fail_next_compile();
    let mut refused = prepare(2);
    assert!(refused.compiled.is_none());
    let mut compiled = prepare(2);
    assert!(compiled.compiled.is_some());
    assert!(render(&mut refused) == render(&mut compiled));
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
    // Guards found values that were not finite, and their cold path fixed
    // them as the interpreter's guards do.
    assert!(jit::RAN[2].load(std::sync::atomic::Ordering::Relaxed) > 0);
    // Tone holes: keys whose shapes share a lookup and refill filters.
    let mut r = Random(0x2545_f491_4f6c_dd1d);
    for case in 0..30 {
        let oversample = [1, 2, 4][case % 3];
        let count = 10 + r.below(40);
        let patch = random_patch_with(&mut r, count, oversample, true);
        assert_compiled_matches(&patch, dir.path());
    }
}

/// A scattering lattice of `sections` sections, as a tone-hole lattice
/// repeats its junctions: each reads a wave arriving from above and one from
/// below (undamped cubic delays of fixed times, some feeding back through
/// an audio-rate signal), scatters them through a lowpass whose cutoff
/// follows a key's slew, a scale and a highpass, and sends the results on.
/// Sections 3 and 5 scale the wave from below first, 6 and 7 take a
/// bandpass and audio-rate feedback instead, and the others add a product
/// that overflows now and then; so packs of every kind are full, take three
/// lanes or two, or come one to a section and stay scalar. An excitation now
/// and then past `jit::LIMIT` runs the guarded copy too. The keys overshoot
/// their shapes' ends both ways and turn back before they settle, so their
/// slews run alone for a while (see `jit::Gen::tail_of`), except where an
/// expression scales a shape, and the output reads one key directly.
#[cfg(feature = "jit")]
fn lattice(sections: usize, oversample: usize) -> String {
    let mut nodes = vec![
        r#"{id: "tone", op: "param", value: 0.5, min: 0, max: 1}"#.to_string(),
        r#"{id: "t", op: "target_frequency"}"#.into(),
        r#"{id: "key", op: "map", input: "t", kind: "log2"}"#.into(),
        r#"{id: "lip", op: "expression", kind: "lip"}"#.into(),
        r#"{id: "air", op: "noise"}"#.into(),
        r#"{id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03}"#
            .into(),
        r#"{id: "burst", op: "mseg", attack: [{time: 0.03, to: 0}, {time: 0.001, to: 1}, {time: 0.002, to: 0}], release: [{time: 0.001, to: 0}], sustain: 2}"#.into(),
        r#"{id: "exc", op: "mul", inputs: ["air", "env", 3]}"#.into(),
        r#"{id: "kick", op: "mul", inputs: ["air", "burst", 100000000]}"#.into(),
        r#"{id: "into", op: "sum", inputs: ["exc", "kick"]}"#.into(),
        r#"{id: "fb", op: "mul", inputs: ["air", 0.5]}"#.into(),
    ];
    let mut flows = Vec::new();
    for k in 0..sections {
        let gain = matches!(k, 3 | 5);
        let band = matches!(k, 6 | 7);
        let f = k as f32;
        let above = if k == 0 {
            "into".to_string()
        } else {
            format!("down{}", k - 1)
        };
        let below = if k + 1 == sections {
            "end".to_string()
        } else {
            format!("up{}", k + 1)
        };
        // Constants past their limit are clamped too.
        let feedback = if band {
            r#""fb", max_feedback: 0.3"#.to_string()
        } else {
            format!("{}, max_feedback: 0.3", 0.2 + 0.05 * f)
        };
        nodes.push(format!(
            r#"{{id: "d{k}", op: "delay", input: "{above}", seconds: {}, interpolation: "cubic", feedback: 0, max_seconds: 0.001}}"#,
            0.0002 + 0.00003 * f
        ));
        nodes.push(format!(
            r#"{{id: "u{k}", op: "delay", input: "{below}", seconds: {}, interpolation: "cubic", feedback: {feedback}, max_seconds: 0.001}}"#,
            0.00017 + 0.00002 * f
        ));
        let b = if gain {
            nodes.push(format!(
                r#"{{id: "g{k}", op: "mul", inputs: ["u{k}", {}]}}"#,
                0.99 - 0.001 * f
            ));
            format!("g{k}")
        } else {
            format!("u{k}")
        };
        nodes.push(format!(
            r#"{{id: "w{k}", op: "sum", inputs: ["d{k}", "{b}", 0]}}"#
        ));
        nodes.push(format!(
            r#"{{id: "target{k}", op: "shape", input: "key", points: [[{}, -0.5], [{}, 1.5]], quality: "raw"}}"#,
            7.9 + 0.05 * f,
            8.4 + 0.05 * f
        ));
        nodes.push(format!(
            r#"{{id: "travel{k}", op: "slew", input: "target{k}", rise: 0.002, fall: {}}}"#,
            0.003 + 0.0005 * f
        ));
        nodes.push(format!(
            r#"{{id: "corner{k}", op: "shape", input: "travel{k}", points: [[0, {}], [1, {}]], quality: "raw"}}"#,
            200. + 50. * f,
            3000. + 100. * f
        ));
        nodes.push(format!(
            r#"{{id: "depth{k}", op: "shape", input: "travel{k}", points: [[0, 0], [1, {}]], quality: "raw"}}"#,
            -0.8 + 0.02 * f
        ));
        nodes.push(format!(
            r#"{{id: "lp{k}", op: "filter", input: "w{k}", cutoff: "corner{k}", mode: "lowpass1"}}"#
        ));
        // In odd sections a step of the key's cluster reads an expression
        // too, so the key's slew moves its shapes even past their ends.
        let depth = if k % 2 == 1 {
            nodes.push(format!(
                r#"{{id: "scaled{k}", op: "mul", inputs: ["depth{k}", "lip", 2]}}"#
            ));
            format!("scaled{k}")
        } else {
            format!("depth{k}")
        };
        nodes.push(format!(
            r#"{{id: "open{k}", op: "mul", inputs: ["lp{k}", "{depth}"]}}"#
        ));
        nodes.push(format!(
            r#"{{id: "r{k}", op: "sum", inputs: ["w{k}", "open{k}"]}}"#
        ));
        nodes.push(if band {
            format!(
                r#"{{id: "hp{k}", op: "filter", input: "r{k}", cutoff: {}, q: {}, mode: "bandpass"}}"#,
                9000. + 100. * f,
                0.7 + 0.1 * f
            )
        } else {
            format!(
                r#"{{id: "hp{k}", op: "filter", input: "r{k}", cutoff: {}, mode: "highpass1"}}"#,
                12000. - 300. * f
            )
        });
        nodes.push(format!(
            r#"{{id: "s{k}", op: "mul", inputs: ["hp{k}", {}]}}"#,
            -0.03 - 0.01 * f
        ));
        nodes.push(format!(
            r#"{{id: "leak{k}", op: "sum", inputs: ["open{k}", "s{k}"]}}"#
        ));
        nodes.push(format!(
            r#"{{id: "up{k}", op: "sum", inputs: ["{b}", 0, "leak{k}"]}}"#
        ));
        if gain || band {
            nodes.push(format!(
                r#"{{id: "down{k}", op: "sum", inputs: ["d{k}", 0, "leak{k}"]}}"#
            ));
        } else {
            // Past f32's range whenever the lowpass is past about 1.1.
            nodes.push(format!(
                r#"{{id: "big{k}", op: "mul", inputs: ["lp{k}", 1000000000000000000, 1000000000000000000, 300]}}"#
            ));
            nodes.push(format!(
                r#"{{id: "tiny{k}", op: "mul", inputs: ["big{k}", 1e-40]}}"#
            ));
            nodes.push(format!(
                r#"{{id: "down{k}", op: "sum", inputs: ["d{k}", 0, "leak{k}", "tiny{k}"]}}"#
            ));
        }
        flows.push(format!("\"open{k}\""));
    }
    nodes.push(format!(
        r#"{{id: "end", op: "mul", inputs: ["down{}", -0.9]}}"#,
        sections - 1
    ));
    nodes.push(format!(
        r#"{{id: "flows", op: "sum", inputs: [{}]}}"#,
        flows.join(", ")
    ));
    format!(
        r#"voice_patch("p", {{
    gain_db: -12, voice_mode: "legato", glide_ms: 2, oversample: {oversample}, note_controls: {{lip: 0.5}},
    nodes: [
        {},
        {{id: "mix", op: "sum", inputs: ["flows", "end", "up0", "travel0"]}},
        {{id: "out", op: "mul", inputs: ["mix", "env", 0.1]}}
    ],
    output: "out"
}})"#,
        nodes.join(",\n        ")
    )
}

/// Lines alone, packed with their writes, holding a burst past `jit::LIMIT`
/// that only they keep: the bounded copy of the audio-rate steps waits for
/// the packed writes to drain, or the fifth power of a line overflows
/// without its guard. The sums read a pack's lanes in its order, and in
/// another order that starts the same.
#[cfg(feature = "jit")]
const PACKED_DRAINS: &str = r#"voice_patch("p", {
    oversample: OVERSAMPLE, note_controls: {lip: 0.5},
    nodes: [
        {id: "tone", op: "param", value: 0.5, min: 0, max: 1},
        {id: "air", op: "noise"},
        {id: "burst", op: "mseg", attack: [{time: 0.002, to: 1}, {time: 0.004, to: 0}], release: [{time: 0.001, to: 0}], sustain: 1},
        {id: "loud", op: "mul", inputs: ["air", "burst", 1000000000]},
        {id: "a0", op: "delay", input: "in0", seconds: 0.0002, interpolation: "cubic", max_seconds: 0.001},
        {id: "a1", op: "delay", input: "in1", seconds: 0.00023, interpolation: "cubic", max_seconds: 0.001},
        {id: "a2", op: "delay", input: "in2", seconds: 0.00031, interpolation: "cubic", max_seconds: 0.001},
        {id: "a3", op: "delay", input: "in3", seconds: 0.00037, interpolation: "cubic", max_seconds: 0.001},
        {id: "h0", op: "mul", inputs: ["a0", 0.5]},
        {id: "h1", op: "mul", inputs: ["a1", 0.6]},
        {id: "h2", op: "mul", inputs: ["a2", 0.7]},
        {id: "h3", op: "mul", inputs: ["a3", 0.4]},
        {id: "g0", op: "mul", inputs: ["a0", 0.25]},
        {id: "g1", op: "mul", inputs: ["a1", 0.2]},
        {id: "g2", op: "mul", inputs: ["a2", 0.15]},
        {id: "g3", op: "mul", inputs: ["a3", 0.1]},
        {id: "x0", op: "sum", inputs: ["h0", "g0"]},
        {id: "x1", op: "sum", inputs: ["h1", "g2"]},
        {id: "x2", op: "sum", inputs: ["h2", "g1"]},
        {id: "x3", op: "sum", inputs: ["h3", "g3"]},
        {id: "in0", op: "sum", inputs: ["loud", "x0"]},
        {id: "in1", op: "sum", inputs: ["loud", "x1"]},
        {id: "in2", op: "sum", inputs: ["loud", "x2"]},
        {id: "in3", op: "sum", inputs: ["loud", "x3"]},
        {id: "fifth", op: "mul", inputs: ["a0", "a0", "a0", "a0", "a0"]},
        {id: "mix", op: "sum", inputs: ["fifth", "a1", "a2", "a3"]},
        {id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03},
        {id: "out", op: "mul", inputs: ["mix", "env", 0.000000001]}
    ],
    output: "out"
})"#;

/// Packs of a lattice's steps and writes compute what the interpreter
/// computes, full and with remainder lanes, beside steps of other shapes and
/// scalar readers of their lanes.
#[cfg(feature = "jit")]
#[test]
fn packed_lattices_match_the_interpreter() {
    let dir = tempfile::tempdir().unwrap();
    for oversample in [1, 2, 4] {
        for sections in [11, 6] {
            let [steps, writes, partial] = jit::packed();
            let patch = lattice(sections, oversample);
            assert_compiled_matches(&patch, dir.path());
            let now = jit::packed();
            assert!(
                now[0] > steps && now[1] > writes,
                "{sections} sections: no packs"
            );
            if sections == 11 {
                assert!(now[2] > partial, "11 sections: no remainder lanes");
            }
            // Compiled without packs, it computes the same.
            jit::pack_nothing(true);
            assert_compiled_matches(&patch, dir.path());
            jit::pack_nothing(false);
            assert_eq!(jit::packed(), now, "packs while packing is off");
        }
        let patch = PACKED_DRAINS.replace("OVERSAMPLE", &oversample.to_string());
        let [_, writes, _] = jit::packed();
        assert_compiled_matches(&patch, dir.path());
        assert!(
            jit::packed()[1] >= writes + 4,
            "the lines' writes are packed"
        );
    }
    // Both copies of the audio-rate steps ran.
    let ran = jit::RAN
        .each_ref()
        .map(|n| n.load(std::sync::atomic::Ordering::Relaxed));
    assert!(ran[0] > 0 && ran[1] > 0, "{ran:?}");
}

/// `lattice` whose holes' filters are two-pole lowpasses, whose packs keep
/// two rows of state and four of coefficients.
#[cfg(feature = "jit")]
fn two_pole_lattice(sections: usize, oversample: usize) -> String {
    let mut patch = lattice(sections, oversample);
    for k in 0..sections {
        let filter = format!(
            r#"{{id: "lp{k}", op: "filter", input: "w{k}", cutoff: "corner{k}", mode: "lowpass1"}}"#
        );
        let two_pole = format!(
            r#"{{id: "lp{k}", op: "filter", input: "w{k}", cutoff: "corner{k}", q: {}}}"#,
            0.6 + 0.05 * k as f32
        );
        assert!(patch.contains(&filter), "section {k}'s filter");
        patch = patch.replace(&filter, &two_pole);
    }
    patch
}

/// Give a compiled patch's voices to the interpreter, as they stand: the
/// rows where packs keep their lanes' state go back to the nodes' state,
/// and memos, some of which compiled code keeps in its own form, empty.
#[cfg(feature = "jit")]
fn to_interpreter(p: &mut VoicePatch) -> jit::Compiled {
    let compiled = p.compiled.take().expect("a compiled patch");
    for v in &mut p.voices {
        compiled.unpack(v);
        v.state.iter_mut().for_each(|s| s.memo.valid = false);
    }
    compiled
}

/// Give an interpreted patch's voices to `compiled`: its packs' rows from
/// the nodes' state, the copies after the lines from the lines, memos
/// empty, every cluster awake and every event-rate and settling step run
/// again (which refills hoisted memos), and the guarded copy of the
/// audio-rate steps until the interpreter's lines are written over.
#[cfg(feature = "jit")]
fn to_compiled(p: &mut VoicePatch, compiled: jit::Compiled) {
    let span = p.delays.iter().map(|d| d.3).max().unwrap_or(0) as u32 + 1;
    for v in &mut p.voices {
        compiled.pack(v);
        // The copies after the lines, which only compiled code keeps.
        for &(.., base, length) in &p.delays {
            v.delay
                .copy_within(base..base + line_tail(length), base + length);
        }
        for s in &mut v.state {
            s.memo.valid = false;
            s.resting = false;
            s.lean = false;
        }
        v.changed = Sources::ALL;
        v.wild = span;
    }
    p.compiled = Some(compiled);
}

/// Voices handed between compiled code and the interpreter mid-note, each
/// way and back and forth, sound as the interpreter's alone: packs keep
/// their lanes' state where `unpack` and `pack` find it.
#[cfg(feature = "jit")]
#[test]
fn voices_handed_between_compiled_code_and_the_interpreter_go_on_alike() {
    let dir = tempfile::tempdir().unwrap();
    let patches = [
        lattice(11, 2),
        lattice(6, 1),
        two_pole_lattice(7, 1),
        gated_lattice(8, 2),
        PACKED_DRAINS.replace("OVERSAMPLE", "1"),
    ];
    for patch in &patches {
        let p = prepare_in(patch, dir.path());
        let rows = p.compiled.as_ref().and_then(jit::Compiled::memory);
        assert!(rows.is_some(), "the patch's packs keep rows:\n{patch}");
        for handoffs in [&[23][..], &[0, 41], &[9, 31, 52, 77, 90]] {
            assert_handoffs_match(patch, dir.path(), handoffs);
        }
    }
}

/// Render `patch` compiled, handing its voices to the interpreter and back
/// at each block of `handoffs`, and require the interpreter's render alone.
#[cfg(feature = "jit")]
fn assert_handoffs_match(patch: &str, dir: &std::path::Path, handoffs: &[usize]) {
    let mut interpreted = prepare_in(patch, dir);
    interpreted.compiled = None;
    let reference = render(&mut interpreted);
    let mut p = prepare_in(patch, dir);
    assert!(p.compiled.is_some(), "the patch compiles:\n{patch}");
    let mut held = None;
    let out = render_with(&mut p, |p, block| {
        if handoffs.contains(&block) {
            match held.take() {
                None => held = Some(to_interpreter(p)),
                Some(compiled) => to_compiled(p, compiled),
            }
        }
    });
    if let Some(i) = out.iter().zip(&reference).position(|(a, b)| a != b) {
        panic!(
            "handed over at {handoffs:?}: block {} sample {} differs\n{patch}",
            i / (2 * BLOCK),
            i % (2 * BLOCK)
        );
    }
}

/// `lattice` whose holes' filters take their input gated by their keys,
/// as a closed hole is: the gate is an exact zero while the key is shut,
/// so the input is a zero of either sign, the filter's state decays (through
/// subnormals) to +0, and packs of such filters skip their steps.
#[cfg(feature = "jit")]
fn gated_lattice(sections: usize, oversample: usize) -> String {
    let mut patch = lattice(sections, oversample);
    for k in 0..sections {
        let filter = format!(
            r#"{{id: "lp{k}", op: "filter", input: "w{k}", cutoff: "corner{k}", mode: "lowpass1"}}"#
        );
        let mode = [
            r#"q: 0.6"#,
            r#"mode: "lowpass1""#,
            r#"q: 0.7, mode: "highpass""#,
        ][k % 3];
        let gated = format!(
            r#"{{id: "gate{k}", op: "shape", input: "travel{k}", points: [[0, 0], [0.5, 0], [1, 1]], quality: "raw"}},
        {{id: "wg{k}", op: "mul", inputs: ["w{k}", "gate{k}"]}},
        {{id: "lp{k}", op: "filter", input: "wg{k}", cutoff: "corner{k}", {mode}}}"#
        );
        assert!(patch.contains(&filter), "section {k}'s filter");
        patch = patch.replace(&filter, &gated);
    }
    patch
}

#[cfg(feature = "jit")]
#[test]
fn gated_filters_rest_at_zero_and_match_the_interpreter() {
    let dir = tempfile::tempdir().unwrap();
    let before = jit::SILENT.load(std::sync::atomic::Ordering::Relaxed);
    for oversample in [1, 2, 4] {
        for sections in [8, 5] {
            assert_compiled_matches(&gated_lattice(sections, oversample), dir.path());
        }
    }
    let after = jit::SILENT.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        after > before,
        "no pack of filters found its state and input zero"
    );
}

/// A random section of `steps` steps, replicated into a lattice of
/// `sections`: each section reads two lines, and its last two steps feed
/// its neighbours'. Constants vary between sections, and now and then a
/// section's step is of another kind, so packs mix full and remainder lanes
/// and sections fall out of them.
#[cfg(feature = "jit")]
fn random_lattice(r: &mut Random, sections: usize, steps: usize, oversample: usize) -> String {
    const CONSTANTS: [&str; 10] = [
        "0",
        "-0",
        "1",
        "-1",
        "0.5",
        "-0.7",
        "0.999",
        "0.002",
        "3000",
        "1000000000000000000",
    ];
    // Each step: its kind (0 sum, 1 product, 2 filter) and its inputs: a
    // line ("a", "b"), an earlier step, a constant, or a shared signal.
    #[derive(Clone)]
    struct Step {
        kind: usize,
        mode: &'static str,
        cutoff: usize,
        inputs: Vec<usize>,
    }
    let shared = ["\"air\"", "\"lip\"", "\"fb\""];
    let mut template = Vec::new();
    for j in 0..steps {
        let kind = r.below(3);
        let count = if kind == 2 { 1 } else { 1 + r.below(3) };
        // Below 2: the lines; below 2 + j: earlier steps; then shared signals
        // and constants.
        let inputs = (0..count)
            .map(|_| match r.below(10) {
                0..=5 => r.below(2 + j),
                6 => 2 + steps + r.below(shared.len()),
                // A step of the section before.
                7 => 2 + steps + shared.len() + CONSTANTS.len() + r.below(steps),
                _ => 2 + steps + shared.len() + r.below(CONSTANTS.len()),
            })
            .collect();
        let modes = [
            "lowpass",
            "highpass",
            "bandpass",
            "notch",
            "lowpass1",
            "highpass1",
            "allpass1",
        ];
        template.push(Step {
            kind,
            mode: modes[r.below(modes.len())],
            // A constant, the key's slew, or an audio-rate signal.
            cutoff: r.below(3),
            inputs,
        });
    }
    let feedback_kind = r.below(2);
    let mut nodes = vec![
        r#"{id: "tone", op: "param", value: 0.5, min: 0, max: 1}"#.to_string(),
        r#"{id: "t", op: "target_frequency"}"#.into(),
        r#"{id: "lip", op: "expression", kind: "lip"}"#.into(),
        r#"{id: "key", op: "map", input: "t", kind: "log2"}"#.into(),
        r#"{id: "air", op: "noise"}"#.into(),
        r#"{id: "fb", op: "mul", inputs: ["air", 0.3]}"#.into(),
        r#"{id: "env", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.6, release: 0.03}"#
            .into(),
        r#"{id: "into", op: "mul", inputs: ["air", "env"]}"#.into(),
        r#"{id: "travel", op: "slew", input: "key", rise: 0.002, fall: 0.003}"#.into(),
        r#"{id: "corner", op: "shape", input: "travel", points: [[7, 300], [9, 5000]], quality: "raw"}"#.into(),
        r#"{id: "wobble", op: "sum", inputs: [2000, "fb"]}"#.into(),
    ];
    let mut outputs = Vec::new();
    for k in 0..sections {
        let mut steps_k = template.clone();
        // Now and then a section's step of another kind.
        if r.below(5) == 0 {
            let j = r.below(steps);
            steps_k[j].kind = (steps_k[j].kind + 1) % 3;
            if steps_k[j].kind != 2 && steps_k[j].inputs.is_empty() {
                steps_k[j].inputs.push(0);
            }
            let most = if steps_k[j].kind == 2 { 1 } else { 3 };
            steps_k[j].inputs.truncate(most);
        }
        let constants: Vec<&str> = (0..CONSTANTS.len())
            .map(|c| {
                if r.below(2) == 0 {
                    CONSTANTS[c]
                } else {
                    r.pick(&CONSTANTS)
                }
            })
            .collect();
        let name = |i: usize| -> String {
            match i {
                0 => format!("\"a{k}\""),
                1 => format!("\"b{k}\""),
                i if i < 2 + steps => format!("\"s{k}_{}\"", i - 2),
                i if i < 2 + steps + shared.len() => shared[i - 2 - steps].to_string(),
                i if i < 2 + steps + shared.len() + CONSTANTS.len() => {
                    constants[i - 2 - steps - shared.len()].to_string()
                }
                // The first section reads its own line instead.
                _ if k == 0 => "\"a0\"".to_string(),
                i => {
                    let j = i - 2 - steps - shared.len() - CONSTANTS.len();
                    format!("\"s{}_{j}\"", k - 1)
                }
            }
        };
        let above = if k == 0 {
            "\"into\"".to_string()
        } else {
            format!("\"s{}_{}\"", k - 1, steps - 1)
        };
        let below = if k + 1 == sections {
            "\"into\"".to_string()
        } else {
            format!("\"s{}_{}\"", k + 1, steps - 2)
        };
        let feedback = match feedback_kind {
            0 => format!("{}", 0.1 + 0.01 * k as f32),
            _ => r#""fb", max_feedback: 0.5"#.to_string(),
        };
        let seconds = |r: &mut Random| 0.00015 + 0.00001 * r.below(30) as f32;
        nodes.push(format!(
            r#"{{id: "a{k}", op: "delay", input: {above}, seconds: {}, interpolation: "cubic", feedback: {feedback}, max_seconds: 0.001}}"#,
            seconds(r)
        ));
        nodes.push(format!(
            r#"{{id: "b{k}", op: "delay", input: {below}, seconds: {}, interpolation: "cubic", feedback: 0, max_seconds: 0.001}}"#,
            seconds(r)
        ));
        for (j, s) in steps_k.iter().enumerate() {
            let inputs: Vec<String> = s.inputs.iter().map(|&i| name(i)).collect();
            let node = match s.kind {
                0 => format!(r#"op: "sum", inputs: [{}]"#, inputs.join(", ")),
                1 => format!(r#"op: "mul", inputs: [{}]"#, inputs.join(", ")),
                _ => {
                    let cutoff = match s.cutoff {
                        0 => format!("{}", 500 + 100 * k),
                        1 => "\"corner\"".into(),
                        _ => "\"wobble\"".into(),
                    };
                    format!(
                        r#"op: "filter", input: {}, cutoff: {cutoff}, q: 0.8, mode: "{}""#,
                        inputs[0], s.mode
                    )
                }
            };
            nodes.push(format!(r#"{{id: "s{k}_{j}", {node}}}"#));
        }
        outputs.push(format!("\"s{k}_{}\"", steps - 1));
    }
    format!(
        r#"voice_patch("p", {{
    gain_db: -24, voice_mode: "legato", glide_ms: 2, oversample: {oversample}, note_controls: {{lip: 0.5}},
    nodes: [
        {},
        {{id: "mix", op: "sum", inputs: [{}]}},
        {{id: "out", op: "mul", inputs: ["mix", "env"]}}
    ],
    output: "out"
}})"#,
        nodes.join(",\n        "),
        outputs.join(", ")
    )
}

#[cfg(feature = "jit")]
#[test]
fn random_packed_lattices_match_the_interpreter() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Random(0x2545_f491_4f6c_dd1d);
    let before = jit::packed();
    for case in 0..36 {
        let oversample = [1, 2, 4][case % 3];
        let sections = 2 + r.below(9);
        let steps = 3 + r.below(8);
        let patch = random_lattice(&mut r, sections, steps, oversample);
        assert_compiled_matches(&patch, dir.path());
        // Handed to the interpreter and back mid-note, now and then.
        if case % 4 == 1 {
            assert_handoffs_match(&patch, dir.path(), &[9, 31, 52, 77, 90]);
        }
    }
    let after = jit::packed();
    assert!(
        (0..3).all(|i| after[i] > before[i]),
        "packs: {before:?} -> {after:?}"
    );
}

/// The halvers' vectorized sums are the scalar sums: each output adds its
/// taps' products in order, newest frame first, from zero.
#[test]
fn halvers_sum_their_taps_in_order() {
    let mut seed = 0x2545_f491u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        // Magnitudes from 1e-6 to 1e3, so rounding depends on the order.
        let x = seed as f32 / u32::MAX as f32 * 2. - 1.;
        x * 10f32.powi((seed % 10) as i32 - 6)
    };
    for mut halver in [Halver::steep(200), Halver::half_band(200)] {
        let taps = halver.taps.clone();
        let count = taps.len();
        let mut past = vec![[0f32; 2]; count - 1];
        for len in [200, 34, 2, 96, 64, 130] {
            let input: Vec<[f32; 2]> = (0..len).map(|_| [noise(), noise()]).collect();
            let mut output = vec![[0f32; 2]; len / 2];
            halver.decimate(&input, &mut output);
            past.extend_from_slice(&input);
            for (j, out) in output.iter().enumerate() {
                let mut sum = [0f32; 2];
                for (t, tap) in taps.iter().enumerate() {
                    let frame = past[2 * j + count - t];
                    sum[0] += frame[0] * tap;
                    sum[1] += frame[1] * tap;
                }
                assert_eq!(out.map(f32::to_bits), sum.map(f32::to_bits), "{len}: {j}");
            }
            past.drain(..past.len() - (count - 1));
        }
    }
}
