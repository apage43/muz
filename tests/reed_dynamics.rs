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

/// The mouth pressure (kPa) at which a reed under `force` newtons of lip
/// shuts, from a slow ramp of the mouth pressure with no bore.
fn closing_kpa(dir: &Path, force: f64) -> f64 {
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/woodwind-prototypes/reed_dynamics.muz"
        ),
        dir.join("reed_dynamics.muz"),
    )
    .unwrap();
    let source = dir.join("closing.muz");
    // The mouth rises to 3 units (15 kPa) over a second, far slower than the
    // reed's resonance, so its displacement follows the pressure statically.
    std::fs::write(
        &source,
        format!(
            r#"use "reed_dynamics.muz" as r;
use "std/signal" as s;
let mouth = s.shape(s.note_age(), [[0, 0], [1s, 3]], "raw");
let reed = r.dynamic(mouth, 0, merge(r.lip_reed({force}), {{tight: 0, loose: 1}}));
let patch = voice_patch("reed", {{
    output: s.mul([reed.flow, 0]), feedback: reed.feedback, probes: {{flow: reed.flow}}
}});
song({{tempo: 60, tracks: [track("reed", note(60, 1.2b).gate(1), patch)]}})"#
        ),
    )
    .unwrap();
    let output = dir.join("flow.wav");
    muz::render::render_with(
        muz::compile::compile(&source).unwrap().session,
        &output,
        &muz::render::RenderOptions {
            tap: vec!["reed@flow".to_string()],
            seconds: Some(1.1),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let flow: Vec<f32> = hound::WavReader::open(output)
        .unwrap()
        .samples::<f32>()
        .step_by(2)
        .map(Result::unwrap)
        .collect();
    let peak = flow.iter().cloned().fold(0., f32::max);
    let top = flow.iter().position(|&u| u == peak).unwrap();
    let shut = top + flow[top..].iter().position(|&u| u < peak * 1e-3).unwrap();
    // Units of 5 kPa, 3 units per second.
    shut as f64 / 48_000. * 3. * 5.
}

// Almeida, George, Smith & Wolfe (2013) measured the clarinet's extinction
// line on the (blowing pressure, lip force) plane falling with an effective
// area of about 3 cm^2: 3.3 kPa per newton. The reed under the lip closes
// along a line of that slope (its static closing pressure sits about 15%
// under the played extinction, which reproduces the measured line).
#[test]
fn reed_closing_pressure_falls_with_lip_force_along_the_extinction_line() {
    let dir = tempfile::tempdir().unwrap();
    let forces = [0.5, 1.0, 1.5, 2.0, 2.5];
    let closing: Vec<f64> = forces.iter().map(|&f| closing_kpa(dir.path(), f)).collect();
    let n = forces.len() as f64;
    let (mf, mp) = (
        forces.iter().sum::<f64>() / n,
        closing.iter().sum::<f64>() / n,
    );
    let slope = forces
        .iter()
        .zip(&closing)
        .map(|(f, p)| (f - mf) * (p - mp))
        .sum::<f64>()
        / forces.iter().map(|f| (f - mf).powi(2)).sum::<f64>();
    assert!(
        (-4.0..=-2.7).contains(&slope),
        "closing pressure {closing:?} kPa falls {slope:.2} kPa per newton"
    );
    // A straight line: each point within 0.15 kPa of the fit.
    for (f, p) in forces.iter().zip(&closing) {
        let fit = mp + slope * (f - mf);
        assert!((p - fit).abs() < 0.15, "{f} N: {p:.2} kPa against {fit:.2}");
    }
    // At the chalumeau's 1.4 N the reed closes near the measured G4
    // extinction (about 6 kPa) less the played margin.
    let at = mp + slope * (1.4 - mf);
    assert!((4.5..=6.0).contains(&at), "closing at 1.4 N: {at:.2} kPa");
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
