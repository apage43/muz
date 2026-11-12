//! fx("convolve"): the output is the input convolved with the impulse response,
//! sample for sample and without latency, for responses short enough to run in
//! direct form and long enough to need partitioned FFTs.
use std::path::Path;

fn render(dir: &Path, chain: &str) -> Vec<[f32; 2]> {
    let path = dir.join("case.muz");
    let out = dir.join("case.wav");
    std::fs::write(
        &path,
        format!(
            r#"let source = voice_patch("src", {{gain_db: 0, velocity_track: 0, nodes: [
                {{id: "o", op: "osc", wave: "saw"}}, {{id: "e", op: "adsr", attack: 0.002, decay: 0.05, sustain: 0.3, release: 0.02}},
                {{id: "m", op: "mul", inputs: ["o", "e"]}}], output: "m"}});
            song({{tempo: 120, tracks: [track("t", phrase("C4:e E4:e G5:e r:q"), source, {{pan: 0.3, chain: [{chain}]}})], tail: 0.6}})"#
        ),
    )
    .unwrap();
    muz::render::render(&path, &out, None, None, &[], 48000, 97).unwrap();
    let x: Vec<f32> = hound::WavReader::open(out)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect();
    x.chunks(2).map(|f| [f[0], f[1]]).collect()
}

/// The largest difference between `wet` and `dry` convolved with `ir`, over the
/// first `frames`, relative to the wet signal's peak.
fn error(dry: &[[f32; 2]], wet: &[[f32; 2]], ir: &[[f64; 2]], frames: usize) -> f64 {
    let mut worst = 0f64;
    let mut peak = 0f64;
    for n in 0..frames {
        for c in 0..2 {
            let expect: f64 = ir
                .iter()
                .enumerate()
                .take(n + 1)
                .map(|(k, h)| h[c] * dry[n - k][c] as f64)
                .sum();
            worst = worst.max((expect - wet[n][c] as f64).abs());
            peak = peak.max((wet[n][c] as f64).abs());
        }
    }
    worst / peak
}

fn decaying(taps: usize) -> Vec<[f64; 2]> {
    (0..taps)
        .map(|k| {
            let x = (k as f64 * 1.7).sin() * 0.9995f64.powi(k as i32) * 0.05;
            [x, x]
        })
        .collect()
}

#[test]
fn convolution_matches_the_direct_sum_short_and_long() {
    let dir = tempfile::tempdir().unwrap();
    let dry = render(dir.path(), "");
    // Three taps: all direct form.
    let wet = render(dir.path(), r#"fx("convolve", {ir: [1, -0.5, 0.25]})"#);
    let ir = [[1., 1.], [-0.5, -0.5], [0.25, 0.25]];
    assert!(error(&dry, &wet, &ir, 30000) < 1e-6);
    // A body's length (3000 taps: 64-tap blocks) and a room's (20000: 256).
    for taps in [3000, 20000] {
        let wet = render(
            dir.path(),
            &format!(
                r#"fx("convolve", {{ir: map(range({taps}), fn(k) => sin(k * 1.7) * pow(0.9995, k) * 0.05)}})"#
            ),
        );
        let e = error(&dry, &wet, &decaying(taps), 30000);
        assert!(e < 1e-4, "{taps} taps: relative error {e}");
    }
    // Half wet, half dry, and the wet gain.
    let mixed = render(dir.path(), r#"fx("convolve", {ir: [0.5], mix: 0.5, gain_db: 6.0206})"#);
    for n in 0..30000 {
        assert!((mixed[n][0] - dry[n][0]).abs() < 1e-5);
    }
}

#[test]
fn an_impulse_response_file_is_stereo_and_resampled_to_the_render() {
    let dir = tempfile::tempdir().unwrap();
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 24000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(dir.path().join("ir.wav"), spec).unwrap();
    // Left passes the signal; right delays it by one 24 kHz sample (two at 48 kHz).
    for frame in [[0.5f32, 0.], [0., 0.5], [0., 0.], [0., 0.]] {
        w.write_sample(frame[0]).unwrap();
        w.write_sample(frame[1]).unwrap();
    }
    w.finalize().unwrap();
    let dry = render(dir.path(), "");
    let wet = render(dir.path(), r#"fx("convolve", {ir: "ir.wav"})"#);
    // Resampling keeps the gain: each tap scales by 24/48, and the linear
    // interpolation spreads it to its new neighbours (the taps sum to 0.5).
    let ir = [[0.25, 0.], [0.125, 0.125], [0., 0.25], [0., 0.125]];
    assert!(error(&dry, &wet, &ir, 30000) < 1e-5);
    // An inline response at another rate: the same.
    let inline = render(dir.path(), r#"fx("convolve", {ir: [[0.5, 0], [0, 0.5], [0, 0], [0, 0]], ir_rate: 24kHz})"#);
    assert!(error(&dry, &inline, &ir, 30000) < 1e-5);
}

#[test]
fn a_response_from_a_magnitude_curve_and_a_woodwind_body() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ir.muz");
    std::fs::write(
        &p,
        r#"use "std/physical" as ph;
        use "std/woodwinds" as w;
        // Flat: an impulse at the middle tap. -6 dB everywhere: half of it.
        let flat = ph.response_ir([[100, 0], [1000, 0]], 63);
        assert(abs(flat[31] - 1) < 0.01 && abs(flat[30]) < 0.01, "flat");
        let down = ph.response_ir([[100, -6.0206], [1000, -6.0206]], 63);
        assert(abs(fold(down, 0, fn(a, x) => a + x) - 0.5) < 0.001, "dc gain");
        // A peak at 6 kHz: a 6 kHz cosine through it is twice as strong as a 1 kHz one.
        let peak = ph.response_ir([[1000, 0], [4000, 0], [6000, 6.0206], [9000, 0]], 255);
        fn part(hz, f) = fold(range(255), 0, fn(a, n) => a + peak[n] * f(2 * 3.141592653589793 * hz * n / 48000));
        fn at(hz) = pow(pow(part(hz, cos), 2) + pow(part(hz, sin), 2), 0.5);
        assert(abs(at(6000) / at(1000) - 2) < 0.1, str(at(6000) / at(1000)));
        assert(len(w.body(w.specs.bassoon)) == 1 && len(w.body(w.specs.oboe)) == 0, "body chains");
        song({tempo: 60, tracks: [track("b", phrase("D3:h"), w.bassoon(), {chain: w.body(w.specs.bassoon)})]})"#,
    )
    .unwrap();
    muz::lang::load(&p).unwrap();
}
