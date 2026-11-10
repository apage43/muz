//! Synthetic checks for the 4x true-peak estimate and the limiter's true-peak mode.
use super::*;
use crate::audio::{AudioConfig, DeviceProcessor, ProcessContext, TransportSnapshot};

const RATE: f32 = 48_000.0;

fn db(x: f64) -> f64 {
    20.0 * x.max(1.0e-20).log10()
}

/// Margin kept from the ends of a signal, where an estimate sees the step to
/// silence rather than the signal.
const EDGE: usize = 64;

/// The 4x estimate over the interior of `samples`.
fn estimate(samples: &[f32]) -> f64 {
    let mut peak = 0.0f32;
    let interior = EDGE..samples.len() - EDGE;
    for_each_true_peak(samples.iter().copied(), |i, p| {
        if interior.contains(&i) {
            peak = peak.max(p);
        }
    });
    peak as f64
}

/// Reference "continuous" peak over the same interior: a long Kaiser-windowed
/// sinc evaluated on a 32x grid.
fn dense_peak(samples: &[f32]) -> f64 {
    const HALF: i64 = EDGE as i64;
    let kaiser = |t: f64| {
        let r = t / HALF as f64;
        bessel_i0(10.0 * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(10.0)
    };
    let n = samples.len() as i64;
    let mut peak = 0.0f64;
    for a in HALF..n - HALF {
        peak = peak.max((samples[a as usize] as f64).abs());
        for step in 1..32 {
            let fraction = step as f64 / 32.0;
            let mut value = 0.0;
            for i in -HALF + 1..=HALF {
                let t = fraction - i as f64;
                value += samples[(a + i) as usize] as f64 * sinc(t) * kaiser(t);
            }
            peak = peak.max(value.abs());
        }
    }
    peak
}

fn sine(frequency: f64, phase: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            (amplitude
                * (2.0 * std::f64::consts::PI * frequency * n as f64 / RATE as f64 + phase).sin())
                as f32
        })
        .collect()
}

#[test]
fn quarter_rate_sine_at_45_degrees_reads_its_true_peak() {
    let x = sine(RATE as f64 / 4.0, std::f64::consts::FRAC_PI_4, 1.0, 4_800);
    let sample_peak = db(x.iter().fold(0.0f32, |m, v| m.max(v.abs())) as f64);
    let true_peak = db(estimate(&x));
    assert!((sample_peak + 3.0103).abs() < 0.001, "{sample_peak}");
    assert!(
        true_peak.abs() < 0.02,
        "4x estimate {true_peak} dBTP, true peak 0 dBTP"
    );
}

/// Worst-case under-read across the audio band against the dense reference,
/// and no meaningful over-read. The bounds are the documented margins.
#[test]
fn estimate_tracks_the_dense_reference_across_the_band() {
    let mut worst = Vec::new();
    for frequency in [
        1_000.0, 5_000.0, 10_000.0, 12_000.0, 15_000.0, 18_000.0, 20_000.0, 21_000.0, 22_000.0,
    ] {
        let mut low = 0.0f64;
        let mut high = f64::NEG_INFINITY;
        for k in 0..8 {
            let x = sine(frequency, k as f64 * std::f64::consts::PI / 8.0, 0.5, 400);
            let error = db(estimate(&x)) - db(dense_peak(&x));
            low = low.min(error);
            high = high.max(error);
        }
        worst.push((frequency, low, high));
    }
    println!("frequency, worst under-read dB, worst over-read dB: {worst:?}");
    for (frequency, low, high) in worst {
        let bound = if frequency <= 12_000.0 {
            -0.25
        } else if frequency <= 20_000.0 {
            -0.6
        } else {
            -1.0
        };
        assert!(low > bound, "{frequency} Hz under-reads {low} dB");
        assert!(high < 0.1, "{frequency} Hz over-reads {high} dB");
    }
}

#[test]
fn short_signals_report_every_sample() {
    let mut seen = Vec::new();
    for_each_true_peak([0.5f32, -0.25, 0.0], |i, p| seen.push((i, p)));
    assert_eq!(seen.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2]);
    assert!(seen[0].1 >= 0.5 && seen[1].1 >= 0.25);
}

fn context(start: usize, frames: usize) -> ProcessContext {
    ProcessContext {
        frames,
        block_start_sample: start as u64,
        transport: TransportSnapshot {
            sample_rate: RATE as f64,
            running: true,
            sample_position: start as u64,
            beat_position: 0.0,
            current_tick: 0.0,
            project_frame: start as f64,
            bpm: 120.0,
            meter: [4, 4],
            ended: false,
        },
    }
}

fn limiter(params: serde_json::Value) -> Box<dyn DeviceProcessor> {
    let device = serde_json::from_value(serde_json::json!({
        "id": "lim", "kind": "builtin.limiter", "params": params
    }))
    .unwrap();
    crate::audio::create_processor(
        &device,
        AudioConfig {
            sample_rate: RATE,
            max_frames: 512,
            offline: true,
        },
    )
    .unwrap()
}

/// Process `left`/`right` in blocks of `block` frames.
fn run(
    processor: &mut dyn DeviceProcessor,
    left: &[f32],
    right: &[f32],
    block: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (left.to_vec(), right.to_vec());
    let mut start = 0;
    while start < l.len() {
        let frames = block.min(l.len() - start);
        processor
            .process(
                context(start, frames),
                &[],
                &mut l[start..start + frames],
                &mut r[start..start + frames],
            )
            .unwrap();
        start += frames;
    }
    (l, r)
}

/// Loud bursts of high-frequency tones (11–22 kHz) over a low tone, driven well
/// past the ceiling, with phases that put many peaks between samples.
fn adversarial(frames: usize, channel: usize) -> Vec<f32> {
    let tau = 2.0 * std::f64::consts::PI;
    (0..frames)
        .map(|n| {
            let t = n as f64 / RATE as f64;
            let burst = 0.5 + ((n / 700) % 3) as f64 * 0.5;
            let low = 0.4 * (tau * 110.0 * t).sin();
            let high = 0.7 * (tau * 12_000.0 * t + 0.785 + channel as f64).sin()
                + 0.6 * (tau * 17_300.0 * t + 1.3).sin()
                + 0.6 * (tau * 19_900.0 * t + 0.4 * channel as f64).sin()
                + 0.5 * (tau * 21_700.0 * t).sin();
            (low + burst * high) as f32
        })
        .collect()
}

fn hash(left: &[f32], right: &[f32]) -> u64 {
    left.iter()
        .chain(right)
        .fold(0xcbf2_9ce4_8422_2325u64, |h, x| {
            (h ^ u64::from(x.to_bits())).wrapping_mul(0x0100_0000_01b3)
        })
}

/// Default (sample-peak) limiter output is pinned bit for bit, so the true-peak
/// option cannot change existing renders.
#[test]
fn default_limiter_output_is_unchanged() {
    let (left, right) = (adversarial(4_800, 0), adversarial(4_800, 1));
    let cases = [
        (
            serde_json::json!({"ceiling_db": -3.0}),
            0x14dc_2d73_4858_a69du64,
        ),
        (
            serde_json::json!({"ceiling_db": -3.0, "lookahead_ms": 5.0, "release_ms": 50.0}),
            0x1348_cb2a_8712_cd94u64,
        ),
    ];
    for (params, expected) in cases {
        let (l, r) = run(limiter(params.clone()).as_mut(), &left, &right, 256);
        let got = hash(&l, &r);
        println!("{params} -> {got:#x}");
        assert_eq!(got, expected, "{params}");
    }
}

#[test]
fn true_peak_limiter_holds_the_estimate_under_the_ceiling() {
    let frames = 2_400;
    let (left, right) = (adversarial(frames, 0), adversarial(frames, 1));
    let ceiling = -3.0;
    let measure = |l: &[f32], r: &[f32], skip: usize| {
        let (l, r) = (&l[skip..], &r[skip..]);
        let estimate = db(estimate(l).max(estimate(r)));
        let dense = db(dense_peak(l).max(dense_peak(r)));
        let sample = db(l.iter().chain(r).fold(0.0f32, |m, v| m.max(v.abs())) as f64);
        (sample, estimate, dense)
    };

    let (l, r) = run(
        limiter(serde_json::json!({"ceiling_db": ceiling, "lookahead_ms": 5.0})).as_mut(),
        &left,
        &right,
        256,
    );
    let (sample, estimate_db, dense) = measure(&l, &r, 240);
    println!("default: sample {sample:.3} estimate {estimate_db:.3} dense {dense:.3}");
    assert!(sample <= ceiling + 1.0e-4);
    assert!(estimate_db > ceiling + 0.3, "default mode should overshoot");

    let mut processor =
        limiter(serde_json::json!({"ceiling_db": ceiling, "lookahead_ms": 5.0, "true_peak": 1.0}));
    let latency = processor.debug_state().latency_samples as usize;
    assert_eq!(latency, 240 + TRUE_PEAK_DELAY);
    let (l, r) = run(processor.as_mut(), &left, &right, 256);
    let (sample, estimate_db, dense) = measure(&l, &r, latency);
    println!("true peak: sample {sample:.3} estimate {estimate_db:.3} dense {dense:.3}");
    assert!(sample <= ceiling + 1.0e-4);
    assert!(estimate_db <= ceiling + 0.02, "4x estimate {estimate_db}");
    assert!(dense <= ceiling + 0.6, "dense reference {dense}");

    // Block size does not change the result.
    let (l2, r2) = run(
        limiter(serde_json::json!({"ceiling_db": ceiling, "lookahead_ms": 5.0, "true_peak": 1.0}))
            .as_mut(),
        &left,
        &right,
        37,
    );
    assert_eq!(hash(&l, &r), hash(&l2, &r2));
}

/// Material under the ceiling passes unchanged, delayed by the reported latency,
/// including with the minimum lookahead the true-peak mode imposes.
#[test]
fn true_peak_limiter_is_transparent_below_the_ceiling() {
    let x = sine(1_000.0, 0.3, 0.5, 4_800);
    let mut processor = limiter(serde_json::json!({"ceiling_db": -1.0, "true_peak": 1.0}));
    let latency = processor.debug_state().latency_samples as usize;
    assert_eq!(latency, 48 + TRUE_PEAK_DELAY);
    let (l, _) = run(processor.as_mut(), &x, &x, 128);
    for n in latency..x.len() {
        assert!((l[n] - x[n - latency]).abs() < 1.0e-6, "frame {n}");
    }
}

#[test]
fn true_peak_flag_rejects_other_values() {
    let device = serde_json::from_value(serde_json::json!({
        "id": "lim", "kind": "builtin.limiter", "params": {"true_peak": 0.5}
    }))
    .unwrap();
    let config = AudioConfig {
        sample_rate: RATE,
        max_frames: 512,
        offline: true,
    };
    assert!(crate::audio::create_processor(&device, config).is_err());
}

/// Source writes the switch as a boolean; it reaches the device as 1 or 0.
#[test]
fn source_accepts_a_boolean_true_peak_switch() {
    for (text, value) in [("true", 1.0), ("false", 0.0)] {
        let source =
            format!(r#"let main = {{type:"fx",name:"limiter",ceiling_db:-2,true_peak:{text}}};"#);
        let device = crate::device_state::reconstruct(&source, "lim").unwrap();
        assert_eq!(device.params.get("true_peak"), Some(&value));
    }
}

/// Detuned saws with decaying noise hits, driven `drive` times over full scale.
fn dense(frames: usize, channel: usize, drive: f64) -> Vec<f32> {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64 ^ channel as u64;
    let mut noise = move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
    };
    let saw = |f: f64, n: usize| {
        let p = f * n as f64 / RATE as f64;
        2.0 * (p - p.floor()) - 1.0
    };
    (0..frames)
        .map(|n| {
            let pad = [110.0, 110.7, 164.8, 220.9, 329.2, 441.3]
                .iter()
                .map(|&f| saw(f * (1.0 + 0.002 * channel as f64), n))
                .sum::<f64>()
                / 6.0;
            let beat = n % 12_000;
            let hit = noise() * (-(beat as f64) / 1_500.0).exp()
                + (2.0 * std::f64::consts::PI * 60.0 * beat as f64 / RATE as f64).sin()
                    * (-(beat as f64) / 4_000.0).exp();
            (drive * (0.3 * pad + 0.7 * hit)) as f32
        })
        .collect()
}

/// Low tones and kick-like thumps only, so peaks between samples barely exceed
/// the sample peaks, driven over a −3 dB ceiling.
fn tame(frames: usize, channel: usize) -> Vec<f32> {
    let tau = 2.0 * std::f64::consts::PI;
    (0..frames)
        .map(|n| {
            let t = n as f64 / RATE as f64;
            let beat = (n % 12_000) as f64;
            let tones = 0.3 * (tau * 55.0 * t).sin()
                + 0.25 * (tau * 82.4 * t + channel as f64).sin()
                + 0.2 * (tau * 110.0 * t + 0.5).sin();
            let thump = (tau * 60.0 * beat / RATE as f64).sin() * (-beat / 3_000.0).exp();
            (tones + 0.8 * thump) as f32
        })
        .collect()
}

/// How the true-peak limiter's gain compares with the default limiter's on
/// the same input, aligned for their latencies.
struct Comparison {
    /// Largest amount the true-peak gain is above the default gain, in dB
    /// (negative: it always reduces more).
    worst_louder_db: f64,
    /// Largest gain difference either way, in dB.
    worst_difference_db: f64,
    /// RMS level of the true-peak output minus the default output, in dB.
    rms_difference_db: f64,
    /// `(input sample, true-peak gain minus default gain in dB)`.
    differences: Vec<(usize, f64)>,
}

fn compare(
    left: &[f32],
    right: &[f32],
    default_params: serde_json::Value,
    true_peak_params: serde_json::Value,
) -> Comparison {
    let mut outputs = Vec::new();
    for params in [default_params, true_peak_params] {
        let mut processor = limiter(params);
        let latency = processor.debug_state().latency_samples as usize;
        let (l, r) = run(processor.as_mut(), left, right, 256);
        outputs.push((latency, l, r));
    }
    let last = left.len() - outputs[0].0.max(outputs[1].0);
    let rms = |(latency, l, r): &(usize, Vec<f32>, Vec<f32>)| {
        rms_db(&l[*latency..latency + last], &r[*latency..latency + last])
    };
    let mut comparison = Comparison {
        worst_louder_db: f64::NEG_INFINITY,
        worst_difference_db: 0.0,
        rms_difference_db: rms(&outputs[1]) - rms(&outputs[0]),
        differences: Vec::new(),
    };
    for k in 0..last {
        // The gain is linked across channels; read it from the louder one.
        let (x, channel) = if left[k].abs() >= right[k].abs() {
            (left[k], 0)
        } else {
            (right[k], 1)
        };
        if x.abs() < 1.0e-2 {
            continue;
        }
        let gain = |(latency, l, r): &(usize, Vec<f32>, Vec<f32>)| {
            db(f64::from([l, r][channel][k + latency] / x))
        };
        let difference = gain(&outputs[1]) - gain(&outputs[0]);
        comparison.worst_louder_db = comparison.worst_louder_db.max(difference);
        comparison.worst_difference_db = comparison.worst_difference_db.max(difference.abs());
        comparison.differences.push((k, difference));
    }
    comparison
}

fn params(lookahead: f64, release: f64, true_peak: bool) -> serde_json::Value {
    serde_json::json!({
        "ceiling_db": -3.0, "lookahead_ms": lookahead,
        "release_ms": release, "true_peak": if true_peak { 1.0 } else { 0.0 }
    })
}

fn rms_db(l: &[f32], r: &[f32]) -> f64 {
    let sum: f64 = l
        .iter()
        .chain(r)
        .map(|&x| f64::from(x) * f64::from(x))
        .sum();
    10.0 * (sum / (l.len() + r.len()) as f64).log10()
}

/// A stricter detector at the same ceiling never reduces less: on dense,
/// bright material the true-peak gain is at or below the default gain at every
/// sample. `lookahead_ms:0` in true-peak mode runs as 1 ms, so it is compared
/// with the default mode at 1 ms.
#[test]
fn true_peak_mode_reduces_at_least_as_much_as_the_default_mode() {
    let frames = 96_000;
    let (left, right) = (dense(frames, 0, 2.0), dense(frames, 1, 2.0));
    for (lookahead, true_peak_lookahead, release) in [
        (5.0, 5.0, 90.0),
        (5.0, 5.0, 50.0),
        (2.0, 2.0, 200.0),
        (1.0, 0.0, 20.0),
    ] {
        let c = compare(
            &left,
            &right,
            params(lookahead, release, false),
            params(true_peak_lookahead, release, true),
        );
        println!(
            "lookahead {lookahead} release {release}: worst louder {:.5} dB, rms {:+.3} dB",
            c.worst_louder_db, c.rms_difference_db
        );
        assert!(c.worst_louder_db <= 1.0e-4, "{}", c.worst_louder_db);
        assert!(c.rms_difference_db <= 0.0);
    }
}

/// After one smooth 0.5 ms bump, both modes recover along the same curve. (The
/// first true-peak mode released from a single pole and recovered faster:
/// 3.4 dB less reduction than the default mode 60 ms after a peak.) On the
/// bump's rising edge the 4x detector reads the points just after each
/// sample, so true-peak mode attacks a fraction of a sample earlier; each
/// sample also covers the interval before it, so it holds one sample longer.
#[test]
fn true_peak_mode_recovers_like_the_default_mode() {
    let mut x = sine(1_000.0, 0.0, 0.1, 24_000);
    for n in 0..24 {
        x[1_000 + n] += 1.9 * (std::f32::consts::PI * n as f32 / 24.0).sin().powi(2);
    }
    let c = compare(&x, &x, params(5.0, 90.0, false), params(5.0, 90.0, true));
    assert!(c.worst_louder_db <= 1.0e-4, "{}", c.worst_louder_db);
    let recovery = c
        .differences
        .iter()
        .filter(|(k, _)| *k >= 1_012)
        .fold(0.0f64, |m, (_, d)| m.max(d.abs()));
    println!(
        "attack {:.5} dB, recovery {recovery:.5} dB",
        c.worst_difference_db
    );
    assert!(recovery < 0.02, "{recovery}");
}

/// Where 4x peaks barely exceed sample peaks, true-peak mode is the default
/// mode delayed by the interpolator, never louder. The small per-sample
/// differences come from the sliding hold (about 0.01 dB here) and from each
/// sample also covering the interval before it.
#[test]
fn true_peak_mode_matches_the_default_mode_without_inter_sample_peaks() {
    let frames = 96_000;
    let (left, right) = (tame(frames, 0), tame(frames, 1));
    for (lookahead, release) in [(5.0, 90.0), (2.0, 200.0)] {
        let c = compare(
            &left,
            &right,
            params(lookahead, release, false),
            params(lookahead, release, true),
        );
        println!(
            "lookahead {lookahead} release {release}: worst difference {:.5} dB, rms {:+.5} dB",
            c.worst_difference_db, c.rms_difference_db
        );
        assert!(c.worst_louder_db <= 1.0e-4, "{}", c.worst_louder_db);
        assert!(c.worst_difference_db < 0.05, "{}", c.worst_difference_db);
        assert!(c.rms_difference_db.abs() < 0.005, "{}", c.rms_difference_db);
    }
}
