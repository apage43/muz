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

/// One slur of a grid: its name, written notes, pressure, key_time, and the
/// phase of the old note's oscillation at the change (the old note starts
/// that fraction of its period early).
type Slur = (&'static str, i32, i32, f64, f64, f64);

fn slur_source(grid: &[Slur], track: &str) -> String {
    let mut notes = Vec::new();
    for (k, &(_, a, b, pressure, key_time, phase)) in grid.iter().enumerate() {
        let t = k as f64 * SLUR_GAP;
        let early = phase / hz((a - 2) as f64);
        let e = format!("{{pressure: {pressure}, tongue: 0, key_time: {key_time}}}");
        // An ascent across the break leads with the register key as far as
        // the geometry has the player lift it (lattice-clarinet.muz's
        // `vented` rule; the change is 500 ms into the first note's slot).
        let first = if a <= 70 && b > 70 {
            format!(
                "{{pressure: {pressure}, tongue: 0, key_time: {key_time}, vent: lc.vent_curve({:.4})}}",
                500. + early * 1000.
            )
        } else {
            e.clone()
        };
        notes.push(format!(
            "note({}, {:.6}b, at = {:.6}b).gate(1).express({first})",
            a - 2,
            0.53 + early,
            t - early
        ));
        notes.push(format!(
            "note({}, 0.5b, at = {:.2}b).gate(1).express({e})",
            b - 2,
            t + 0.5
        ));
    }
    format!(
        r#"use "lattice-clarinet.muz" as lc;
let line = stack([{}]);
song({{tempo: 60, tracks: [track("{track}", line, lc.clarinet({{velocity_breath: 0}}))]}})"#,
        notes.join(",\n")
    )
}

/// Each slur's settling time (s) and dip (dB), checking its pitch and stray
/// modes; ascents across the break are returned for their own bounds.
fn check_slurs(audio: &[f32], grid: &[Slur], seed: &str) -> Vec<(String, f64, f64)> {
    assert!(audio.iter().all(|x| x.is_finite() && x.abs() < 1.0));
    let rms = |x: &[f32]| (x.iter().map(|v| (v * v) as f64).sum::<f64>() / x.len() as f64).sqrt();
    let db = |x: f64| 20. * x.log10();
    let mut ascents = Vec::new();
    for (k, &(name, a, b, pressure, key_time, phase)) in grid.iter().enumerate() {
        let change = k as f64 * SLUR_GAP + 0.5;
        let to = hz((b - 2) as f64);
        let steady = window(audio, change + 0.25, change + 0.45);
        let (offset, level) = cents(steady, to, 30.);
        let before = window(audio, change - 0.2, change - 0.02);
        // The third partial's reference: its steady level, or the old note's
        // level at that frequency where it is higher (D5's second partial is
        // open G's third), so a burst is a third mode above both.
        let third = amplitude(steady, to * 3.0).max(amplitude(before, to * 3.0));
        let floor = rms(before).min(rms(steady));
        let label =
            format!("{name} at pressure {pressure}, key_time {key_time}, phase {phase} ({seed})");
        // Tuned within 0.3 cents with the player's pitch still, the note
        // settles within the player's drift (2.5 cents) of it.
        assert!(
            offset.abs() <= 4.,
            "{label}: settles {offset:+.1} cents off"
        );
        // Settled: the new fundamental within 2 dB of its steady level from
        // then on, in 15 ms windows. Stray modes: the twelfth below against
        // the new fundamental, and the third partial against its own steady
        // level (a third-mode burst).
        let (mut settle, mut low, mut burst) = (None, 0.0_f64, 0.0_f64);
        let mut t = -0.01;
        while t < 0.2 {
            let w = window(audio, change + t, change + t + 0.015);
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
                rms(window(audio, change + t, change + t + 0.015)) / floor
            ));
            t += 0.0025;
        }
        if a <= 70 && b > 70 {
            ascents.push((label.clone(), settle, dip));
        } else {
            // Within a register and descending, slurs settle within 10 ms
            // with hardly a dip.
            assert!(
                settle * 1000. < 15.0,
                "{label}: settles after {:.1} ms",
                settle * 1000.
            );
            assert!(dip > -3.0, "{label}: level dips {dip:.1} dB");
        }
        // Landing on open G from B4, the little finger stays down (open G
        // leaves it free), so its keys do not open under the landing.
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
    ascents
}

#[test]
fn lattice_clarinet_slurs_settle_without_stray_modes() {
    let mut grid = Vec::new();
    for pressure in [0.3, 0.55] {
        for key_time in [0.0, 0.4, 1.0] {
            for (name, a, b) in SLURS {
                grid.push((name, a, b, pressure, key_time, 0.0));
            }
        }
    }
    // How soon a register change lands, and how deep it dips, depends on the
    // phase of the old note when the keys move, which a player does not
    // choose: the ascents across the break again, on another breath, at
    // four phases a quarter of the old note's period apart.
    let mut ascents = Vec::new();
    for pressure in [0.3, 0.55] {
        for key_time in [0.0, 0.4, 1.0] {
            for (name, a, b) in SLURS.into_iter().filter(|s| s.1 <= 70 && s.2 > 70) {
                for phase in [0.0, 0.25, 0.5, 0.75] {
                    ascents.push((name, a, b, pressure, key_time, phase));
                }
            }
        }
    }
    let renders: Vec<_> = [("clarinet", grid), ("ascents", ascents)]
        .into_iter()
        .map(|(seed, grid)| {
            std::thread::spawn(move || {
                let dir = tempfile::tempdir().unwrap();
                let audio = render(dir.path(), seed, &slur_source(&grid, seed));
                check_slurs(&audio, &grid, seed)
            })
        })
        .collect();
    let ascents: Vec<_> = renders
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    // An ascent across the break is a short new attack: the throat note's
    // regime gives way and the clarion's grows anew, as a legato between
    // registers sounds in players' mouthpieces (Pamies-Vila, Hofmann &
    // Chatziioannou 2019). Bounded by what players do (README): professional
    // flutists' changes of several fingers spend 13 +/- 9 to 21 +/- 11 ms in
    // passing fingerings (Almeida, Chow, Smith & Wolfe 2009), over which a
    // regime unsupported decays at about 0.4 dB per ms (Li, Almeida, Smith &
    // Wolfe 2016). Every ascent settles within 45 ms and dips less than
    // 6 dB, and they settle within 30 ms on average.
    for (label, settle, dip) in &ascents {
        assert!(
            settle * 1000. < 45.0,
            "{label}: settles after {:.1} ms",
            settle * 1000.
        );
        assert!(*dip > -6.0, "{label}: level dips {dip:.1} dB");
    }
    let mean = ascents.iter().map(|a| a.1).sum::<f64>() / ascents.len() as f64;
    assert!(
        mean * 1000. <= 30.0,
        "ascents settle after {:.1} ms on average",
        mean * 1000.
    );
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

// A chain of cylinders joined by the bore's area steps (`scatter`), fed a
// pulse and ending anechoically, reflects as transmission-line theory says.
fn chain_source(lengths: &[usize], radii: &[f64]) -> String {
    let refl: Vec<String> = radii
        .windows(2)
        .map(|r| {
            let (a, b) = (r[0] * r[0], r[1] * r[1]);
            format!("{}", (a - b) / (a + b))
        })
        .collect();
    let lengths: Vec<String> = lengths.iter().map(|n| n.to_string()).collect();
    format!(
        r#"use "clarinet_bore.muz" as cb;
use "std/signal" as s;
let lengths = [{lengths}];
let reflections = [{refl}];
let pulse = s.shape(s.note_age(), [[0, 0.1], [0.021ms, 0], [1s, 0]], "raw");
fn line(n) = s.line(n / {RATE} * 1s, {{max_seconds: (n + 4) / {RATE} * 1s}});
let down = map(lengths, line);
let up = map(lengths, line);
let m = len(lengths);
let joins = map(range(m - 1), fn(k) => cb.scatter(down[k], up[k + 1], reflections[k]));
let patch = voice_patch("chain", {{
    output: up[0],
    probes: {{incident: pulse, reflected: up[0]}},
    feedback: [[down[0], pulse], [up[m - 1], s.mul([down[m - 1], 0])]] +
        fold(range(m - 1), [], fn(l, k) => l + [[up[k], joins[k].up], [down[k + 1], joins[k].down]])
}});
song({{tempo: 60, tracks: [track("chain", note(60, 1b).gate(1), patch)]}})"#,
        lengths = lengths.join(", "),
        refl = refl.join(", "),
    )
}

/// `sum x[n] e^{-i w n}` as (re, im).
fn dft(x: &[f32], frequency: f64) -> (f64, f64) {
    let w = std::f64::consts::TAU * frequency / RATE as f64;
    x.iter().enumerate().fold((0., 0.), |(re, im), (n, &v)| {
        let (s, c) = (w * n as f64).sin_cos();
        (re + v as f64 * c, im - v as f64 * s)
    })
}

fn cdiv(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let d = b.0 * b.0 + b.1 * b.1;
    ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
}

/// Pressure reflection at the input of cylinders `lengths` samples long with
/// `radii`, the last ending anechoically: the input impedance carried up the
/// chain as a transmission line, Z = Zc (Z_L + j Zc tan kL) / (Zc + j Z_L tan kL).
fn chain_reflection(lengths: &[usize], radii: &[f64], frequency: f64) -> (f64, f64) {
    let zc: Vec<f64> = radii.iter().map(|r| 1. / (r * r)).collect();
    let mut z = (zc[zc.len() - 1], 0.);
    for k in (0..lengths.len()).rev() {
        let t = (std::f64::consts::TAU * frequency * lengths[k] as f64 / RATE as f64).tan();
        let ratio = cdiv((z.0, z.1 + zc[k] * t), (zc[k] - z.1 * t, z.0 * t));
        z = (ratio.0 * zc[k], ratio.1 * zc[k]);
    }
    cdiv((z.0 - zc[0], z.1), (z.0 + zc[0], z.1))
}

#[test]
fn bore_steps_scatter_as_transmission_lines() {
    let dir = tempfile::tempdir().unwrap();
    // One step, then a short flare of cylinders widening to a bell.
    for (lengths, radii) in [
        (vec![24, 30], vec![7.3, 9.0]),
        (vec![24, 9, 10, 11, 9], vec![7.3, 8.6, 10.4, 16.0, 24.0]),
    ] {
        copy_modules(dir.path());
        let file = dir.path().join("chain.muz");
        std::fs::write(&file, chain_source(&lengths, &radii)).unwrap();
        // The probes are raw: no gain or smoothing between the bore and the file.
        let tap = |name: &str| -> Vec<f32> {
            let out = dir.path().join(format!("{name}.wav"));
            muz::render::render_with(
                muz::compile::compile(&file).unwrap().session,
                &out,
                &muz::render::RenderOptions {
                    tap: vec![format!("chain@{name}")],
                    seconds: Some(0.3),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
            hound::WavReader::open(out)
                .unwrap()
                .samples::<f32>()
                .step_by(2)
                .map(Result::unwrap)
                .collect()
        };
        let (reflected, pulse) = (tap("reflected"), tap("incident"));
        // The first echo, from the first step, is that step's reflection.
        let (a, b) = (radii[0] * radii[0], radii[1] * radii[1]);
        let first = reflected[..2 * lengths[0] + 4]
            .iter()
            .fold(0_f32, |m, v| if v.abs() > m.abs() { *v } else { m });
        let peak = pulse.iter().fold(0_f32, |m, v| m.max(*v));
        assert!(
            ((first / peak) as f64 - (a - b) / (a + b)).abs() < 0.002,
            "first echo {} against {}",
            first / peak,
            (a - b) / (a + b)
        );
        for frequency in [150., 400., 1000., 2500., 6000.] {
            let got = cdiv(dft(&reflected, frequency), dft(&pulse, frequency));
            let want = chain_reflection(&lengths, &radii, frequency);
            let error = ((got.0 - want.0).powi(2) + (got.1 - want.1).powi(2)).sqrt();
            assert!(
                error < 0.01,
                "{} steps at {frequency} Hz: {got:?} against {want:?}",
                radii.len() - 1
            );
        }
    }
}
