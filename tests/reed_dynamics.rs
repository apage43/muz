//! The reed correction prototype in examples/woodwind-prototypes.
use std::path::Path;

fn render(dir: &Path, reed: f32, voicing: f32) -> Vec<f32> {
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/woodwind-prototypes/reed_dynamics.muz"
        ),
        dir.join("reed_dynamics.muz"),
    )
    .unwrap();
    let source = dir.join("reed.muz");
    let output = dir.join("reed.wav");
    std::fs::write(
        &source,
        format!(
            r#"use "reed_dynamics.muz" as r;
use "std/physical" as p;
use "std/signal" as s;
let breath = s.adsr(10ms, 20ms, 1, 80ms);
let pipe = p.pipe(s.mul([s.period(s.frequency()), 0.5]), s.frequency(), {{
    tone: 2600Hz, poles: 2, loss: 0.96
}});
let reed = r.single(s.mul([breath, 0.58]), pipe.back, {{reed: {reed}, voicing: {voicing}}});
let patch = voice_patch("reed", {{
    oversample: 2, output: pipe.radiated,
    feedback: [[pipe.entry, s.add([pipe.back, reed.flow])]] + pipe.feedback + reed.feedback,
    lifetime: {{envelope: breath, tail: 120ms}}
}});
song({{tempo: 120, tracks: [track("reed", note("E4", 1b), patch)]}})"#
        ),
    )
    .unwrap();
    muz::render::render(&source, &output, None, None, &[], 48_000, 128).unwrap();
    hound::WavReader::open(output)
        .unwrap()
        .samples()
        .step_by(2)
        .map(Result::unwrap)
        .collect()
}

#[test]
fn reed_and_player_voicing_change_the_feedback_sound_independently() {
    let dir = tempfile::tempdir().unwrap();
    let baseline = render(dir.path(), 0., 0.);
    for (reed, voicing) in [(1., 0.), (0., 1.), (1., 1.)] {
        let signal = render(dir.path(), reed, voicing);
        assert!(signal.iter().all(|s| s.is_finite() && s.abs() < 1.));
        let steady = &signal[9_600..17_000];
        assert!(steady.iter().any(|s| s.abs() > 0.001));
        let difference = steady
            .iter()
            .zip(&baseline[9_600..])
            .map(|(s, b)| (s - b).abs())
            .fold(0., f32::max);
        assert!(
            difference > 0.001,
            "reed {reed}, voicing {voicing}: {difference}"
        );
    }
}
