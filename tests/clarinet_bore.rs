//! The fixed-bore clarinet prototype in examples/woodwind-prototypes.
use std::path::Path;

const RATE: usize = 48_000;
const PROTOTYPES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/woodwind-prototypes");

fn render(dir: &Path, source: &str) -> Vec<f32> {
    for module in [
        "lattice-clarinet.muz",
        "clarinet_bore.muz",
        "reed_dynamics.muz",
    ] {
        std::fs::copy(Path::new(PROTOTYPES).join(module), dir.join(module)).unwrap();
    }
    let file = dir.join("lattice.muz");
    let output = dir.join("lattice.wav");
    std::fs::write(&file, source).unwrap();
    muz::render::render(&file, &output, None, None, &[], RATE as u32, 128).unwrap();
    hound::WavReader::open(output)
        .unwrap()
        .samples::<f32>()
        .step_by(2)
        .map(Result::unwrap)
        .collect()
}

fn window(audio: &[f32], start: f64, end: f64) -> &[f32] {
    &audio[(start * RATE as f64).round() as usize..(end * RATE as f64).round() as usize]
}

fn amplitude(audio: &[f32], frequency: f64) -> f64 {
    let omega = std::f64::consts::TAU * frequency / RATE as f64;
    let (mut sine, mut cosine) = (0.0, 0.0);
    for (i, &sample) in audio.iter().enumerate() {
        let time = i as f64;
        let weight = 0.5 - 0.5 * (std::f64::consts::TAU * time / audio.len() as f64).cos();
        let (s, c) = (omega * time).sin_cos();
        sine += s * weight * sample as f64;
        cosine += c * weight * sample as f64;
    }
    sine.hypot(cosine) / audio.len() as f64
}

fn hz(key: u8) -> f64 {
    440. * 2_f64.powf((key as f64 - 69.) / 12.)
}

#[test]
fn fixed_clarinet_speaks_at_the_target_in_both_registers() {
    let dir = tempfile::tempdir().unwrap();
    let audio = render(
        dir.path(),
        r#"use "lattice-clarinet.muz" as lc;
let line = seq(map(range(50, 88), fn(k) =>
    note(k, 0.5b).gate(0.85).express({pressure: 0.39, tongue: 0})
));
song({tempo: 60, tracks: [track("clarinet", line, lc.clarinet({velocity_breath: 0}))]})"#,
    );
    assert!(audio.iter().all(|x| x.is_finite() && x.abs() < 1.0));
    for key in 50..88 {
        let start = (key - 50) as f64 * 0.5;
        let signal = window(&audio, start + 0.20, start + 0.40);
        let pitch = hz(key);
        // A prototype threshold: the lattice tunes through its loss filters.
        // A calibrated model should hold 5 cents at several dynamics.
        let (offset, fundamental) = (-15_i32..=15)
            .map(|cents| {
                (
                    cents,
                    amplitude(signal, pitch * 2_f64.powf(cents as f64 / 1200.)),
                )
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!(
            offset.abs() < 15,
            "key {key} at edge of 15-cent tuning window"
        );
        assert!(fundamental > 0.001, "key {key} fundamental: {fundamental}");
        for unwanted in [pitch * 0.5, pitch / 3.0, pitch * 5.0 / 3.0] {
            assert!(
                amplitude(signal, unwanted) < fundamental * 0.5,
                "key {key}: unwanted resonance {unwanted} Hz"
            );
        }
    }
}

#[test]
fn fixed_clarinet_changes_register_without_a_third_mode_burst() {
    let dir = tempfile::tempdir().unwrap();
    let audio = render(
        dir.path(),
        r#"use "lattice-clarinet.muz" as lc;
let notes = stack([
    note(69, 0.48b).gate(1).express({pressure: 0.4, tongue: 0}),
    note(65, 0.48b, at = 0.45b).gate(1).express({pressure: 0.4, tongue: 0}),
    note(69, 0.5b, at = 0.90b).gate(1).express({pressure: 0.4, tongue: 0})
]);
song({tempo: 60, tracks: [track("clarinet", notes, lc.clarinet({velocity_breath: 0}))]})"#,
    );
    let early = window(&audio, 0.455, 0.480);
    let steady = window(&audio, 0.6, 0.7);
    let pitch = hz(65);
    let fundamental = amplitude(early, pitch);
    assert!(fundamental > amplitude(steady, pitch) * 0.6, "audible dip");
    assert!(
        amplitude(early, pitch * 3.0) < fundamental * 0.5,
        "third-mode burst"
    );
    let early = window(&audio, 0.92, 0.945);
    assert!(amplitude(early, hz(69)) > amplitude(early, hz(69) / 3.0) * 2.0);
}
