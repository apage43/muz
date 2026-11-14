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
    let source = dir.path().join("case.muz");
    let patch = PATCH.replace("OVERSAMPLE", &oversample.to_string());
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

/// Evaluate every node every frame, in graph order.
fn every_node_every_frame(p: &mut VoicePatch) {
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
        let resting = prepared.voices.iter().filter(|v| v.active && v.settled);
        assert_eq!(resting.count(), 1, "the sounding voice's smoothers settle");
    }
}
