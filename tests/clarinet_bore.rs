//! The tone-hole lattice clarinet prototype in examples/woodwind-prototypes.
use std::path::Path;

const RATE: usize = 48_000;
const PROTOTYPES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/woodwind-prototypes");

fn copy_modules(dir: &Path) {
    for module in [
        "lattice-clarinet.muz",
        "clarinet_bore.muz",
        "clarinet_geometry.muz",
        "clarinet_tuning.muz",
        "reed_dynamics.muz",
    ] {
        let to = dir.join(module);
        if !to.exists() {
            std::fs::copy(Path::new(PROTOTYPES).join(module), to).unwrap();
        }
    }
}

fn render(dir: &Path, name: &str, source: &str) -> Vec<f32> {
    copy_modules(dir);
    let file = dir.join(format!("{name}.muz"));
    let output = dir.join(format!("{name}.wav"));
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

fn hz(key: f64) -> f64 {
    440. * 2_f64.powf((key - 69.) / 12.)
}

/// The cents offset from `pitch` of the strongest partial within `span` cents.
fn cents(signal: &[f32], pitch: f64, span: f64) -> (f64, f64) {
    let mut best = (0.0, 0.0);
    let mut c = -span;
    while c <= span {
        let a = amplitude(signal, pitch * 2_f64.powf(c / 1200.));
        if a > best.1 {
            best = (c, a);
        }
        c += 0.25;
    }
    best
}

const NOTE: f64 = 0.6;
const GAP: f64 = 0.15;

fn scale_source(pressure: f64) -> String {
    format!(
        r#"use "lattice-clarinet.muz" as lc;
let line = stack(map(range(50, 88), fn(k) =>
    note(k, {NOTE}b, at = (k - 50) * {step}b).gate(1).express({{pressure: {pressure}, tongue: 0}})
));
song({{tempo: 60, tracks: [track("clarinet", line, lc.clarinet({{velocity_breath: 0}}))]}})"#,
        step = NOTE + GAP
    )
}

#[test]
fn lattice_clarinet_is_in_tune_across_its_range_at_three_dynamics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_owned();
    // Copy the modules once, then render the three dynamics side by side.
    copy_modules(&path);
    let renders: Vec<_> = [0.3, 0.45, 0.6]
        .into_iter()
        .map(|pressure| {
            let path = path.clone();
            std::thread::spawn(move || {
                (
                    pressure,
                    render(&path, &format!("scale-{pressure}"), &scale_source(pressure)),
                )
            })
        })
        .collect();
    for handle in renders {
        let (pressure, audio) = handle.join().unwrap();
        assert!(audio.iter().all(|x| x.is_finite() && x.abs() < 1.0));
        for key in 50..88 {
            let start = (key - 50) as f64 * (NOTE + GAP);
            let signal = window(&audio, start + 0.3, start + 0.55);
            let pitch = hz(key as f64);
            let (offset, fundamental) = cents(signal, pitch, 40.);
            assert!(
                offset.abs() <= 5.,
                "key {key} at pressure {pressure}: {offset:+.2} cents"
            );
            assert!(fundamental > 0.001, "key {key} fundamental: {fundamental}");
            for unwanted in [pitch / 3.0, pitch * 3.0 / 2.0] {
                assert!(
                    amplitude(signal, unwanted) < fundamental * 0.3,
                    "key {key} at pressure {pressure}: unwanted resonance {unwanted} Hz"
                );
            }
        }
    }
}

// Written-note slurs: across the break both ways and within each register.
const SLURS: [(&str, i32, i32); 9] = [
    ("A4-B4", 69, 71),
    ("B4-A4", 71, 69),
    ("G4-D5", 67, 74),
    ("D5-G4", 74, 67),
    ("B4-G4", 71, 67),
    ("C4-E4", 60, 64),
    ("E4-C4", 64, 60),
    ("D5-F5", 74, 77),
    ("F5-D5", 77, 74),
];
const SLUR_GAP: f64 = 1.2;

#[test]
fn lattice_clarinet_slurs_settle_without_stray_modes() {
    let dir = tempfile::tempdir().unwrap();
    let mut notes = Vec::new();
    let mut grid = Vec::new();
    for pressure in [0.3, 0.55] {
        for key_time in [0.0, 0.4, 1.0] {
            for (name, a, b) in SLURS {
                let t = grid.len() as f64 * SLUR_GAP;
                let e = format!("{{pressure: {pressure}, tongue: 0, key_time: {key_time}}}");
                notes.push(format!(
                    "note({}, 0.53b, at = {t:.2}b).gate(1).express({e})",
                    a - 2
                ));
                notes.push(format!(
                    "note({}, 0.5b, at = {:.2}b).gate(1).express({e})",
                    b - 2,
                    t + 0.5
                ));
                grid.push((name, pressure, key_time, b));
            }
        }
    }
    let source = format!(
        r#"use "lattice-clarinet.muz" as lc;
let line = stack([{}]);
song({{tempo: 60, tracks: [track("clarinet", line, lc.clarinet({{velocity_breath: 0}}))]}})"#,
        notes.join(",\n")
    );
    let audio = render(dir.path(), "slurs", &source);
    assert!(audio.iter().all(|x| x.is_finite() && x.abs() < 1.0));
    let rms = |x: &[f32]| (x.iter().map(|v| (v * v) as f64).sum::<f64>() / x.len() as f64).sqrt();
    let db = |x: f64| 20. * x.log10();
    for (k, (name, pressure, key_time, b)) in grid.into_iter().enumerate() {
        let change = k as f64 * SLUR_GAP + 0.5;
        let to = hz((b - 2) as f64);
        let steady = window(&audio, change + 0.25, change + 0.45);
        let (offset, level) = cents(steady, to, 30.);
        let third = amplitude(steady, to * 3.0);
        let floor = rms(window(&audio, change - 0.2, change - 0.02)).min(rms(steady));
        let label = format!("{name} at pressure {pressure}, key_time {key_time}");
        assert!(
            offset.abs() <= 5.,
            "{label}: settles {offset:+.1} cents off"
        );
        // Settled: the new fundamental within 2 dB of its steady level from
        // then on, in 15 ms windows. Stray modes: the twelfth below against
        // the new fundamental, and the third partial against its own steady
        // level (a third-mode burst).
        let (mut settle, mut low, mut burst) = (None, 0.0_f64, 0.0_f64);
        let mut t = -0.01;
        while t < 0.2 {
            let w = window(&audio, change + t, change + t + 0.015);
            if amplitude(w, to) >= 0.8 * level {
                settle.get_or_insert(t.max(0.0));
            } else {
                settle = None;
            }
            if t >= 0.0 {
                low = low.max(amplitude(w, to / 3.0) / level);
                burst = burst.max(amplitude(w, to * 3.0) / third);
            }
            t += 0.0025;
        }
        let settle = settle.unwrap_or_else(|| panic!("{label}: never settles"));
        let mut dip: f64 = 0.0;
        let mut t = 0.0;
        while t <= settle.max(0.01) {
            dip = dip.min(db(
                rms(window(&audio, change + t, change + t + 0.015)) / floor
            ));
            t += 0.0025;
        }
        // Within a register and descending, slurs settle in 2-25 ms with no
        // dip. An ascent across the break is a release and a new attack of
        // the clarion: 25-40 ms, dipping up to 7 dB at soft and slow keys.
        assert!(
            settle < 0.045,
            "{label}: settles after {:.1} ms",
            settle * 1000.
        );
        assert!(dip > -7.0, "{label}: level dips {dip:.1} dB");
        assert!(
            db(burst) < 7.0,
            "{label}: third-mode burst {:.1} dB",
            db(burst)
        );
        assert!(
            db(low) < -12.0,
            "{label}: twelfth below at {:.1} dB",
            db(low)
        );
    }
}

// Quarter tones: half-shaded holes within a register, and across the break
// and into the altissimo the upper fingering bent down by the player.
#[test]
fn lattice_clarinet_quarter_tones_sit_between_their_semitones() {
    let dir = tempfile::tempdir().unwrap();
    let written = [53, 56, 58, 60, 62, 64, 66, 68, 70, 72, 75, 79, 84];
    let notes: Vec<String> = written
        .iter()
        .enumerate()
        .map(|(i, w)| {
            format!(
                "note({}.5, {NOTE}b, at = {:.2}b).gate(1).express({{pressure: 0.45, tongue: 0}})",
                w - 2,
                i as f64 * (NOTE + GAP)
            )
        })
        .collect();
    let source = format!(
        r#"use "lattice-clarinet.muz" as lc;
let line = stack([{}]);
song({{tempo: 60, tracks: [track("clarinet", line, lc.clarinet({{velocity_breath: 0}}))]}})"#,
        notes.join(",\n")
    );
    let audio = render(dir.path(), "quarters", &source);
    for (i, w) in written.iter().enumerate() {
        let start = i as f64 * (NOTE + GAP);
        let signal = window(&audio, start + 0.3, start + 0.55);
        let pitch = hz(*w as f64 - 1.5);
        let (offset, fundamental) = cents(signal, pitch, 40.);
        // Shaded holes land within a few cents; across the break and into
        // the altissimo the bent upper fingering is looser.
        let tolerance = if *w == 70 || *w == 84 { 12. } else { 8. };
        assert!(
            offset.abs() <= tolerance,
            "written {w} and a quarter: {offset:+.2} cents"
        );
        assert!(
            amplitude(signal, pitch / 3.0) < fundamental * 0.3,
            "written {w} and a quarter: twelfth below"
        );
    }
}
