//! A slur down out of a conical reed's octave register speaks in the lower
//! register: std's oboe, played by std's player, over seeds, phases,
//! dynamics and tempi. Until the octave hole kept its place as the key
//! closed, every short slur from E5 to D5 sounded D6 for 50-140 ms and a
//! slurred sixteenth could lock onto it.
use std::path::Path;

const RATE: f64 = 48000.;

/// The second partial over the first, in dB, every 10 ms from `from` to `to`
/// seconds, each over six periods of `f0`.
fn balance(x: &[f32], f0: f64, from: f64, to: f64) -> Vec<f64> {
    let w = (RATE / f0 * 6.).round() as usize;
    let mut out = vec![];
    let mut a = (from * RATE) as usize;
    while a + w <= ((to * RATE) as usize).min(x.len()) {
        let level = |h: f64| {
            let (mut re, mut im) = (0., 0.);
            for (i, v) in x[a..a + w].iter().enumerate() {
                let p = std::f64::consts::TAU * h * f0 * i as f64 / RATE;
                re += *v as f64 * p.cos();
                im -= *v as f64 * p.sin();
            }
            (re * re + im * im).sqrt() + 1e-12
        };
        out.push(20. * (level(2.) / level(1.)).log10());
        a += (RATE * 0.01) as usize;
    }
    out
}

/// Renders every case at `tempo` on one oboe track, each phrase after a rest
/// at a phase of its own, and returns the mix with each slur down out of the
/// octave register (from E5, the octave key's first note) as [attack s,
/// length s, pitch].
fn render(
    dir: &Path,
    tempo: u32,
    phrases: &[&str],
    dynamics: &[f64],
    seeds: u32,
) -> (Vec<f32>, Vec<[f64; 3]>) {
    let mut cases = vec![];
    for p in phrases {
        for d in dynamics {
            for s in 0..seeds {
                cases.push(format!("[\"{p}\", {d}, {s}]"));
            }
        }
    }
    let path = dir.join(format!("descents{tempo}.muz"));
    std::fs::write(
        &path,
        format!(
            r#"use "std/woodwinds" as w;
            let timing = {{tempo: {tempo}}};
            let spec = w.derive(w.specs.oboe, {{staple_recovery: 0.5}});
            let cases = [{}];
            // Each phrase after three beats' rest, at a phase of its own.
            let parts = map(range(len(cases)), fn(k) => w.play(phrase(cases[k][0]).at((k * 3 + (k * 0.137) % 1) * 1b), spec,
                {{timing: timing, seed: cases[k][2] + 10 * k, dynamics: [[0b, cases[k][1]]], accept: true, strict: false}}));
            fn slurs() = fold(parts, [], fn(out, p) => {{
                let ns = sort_by(p.notes, fn(n) => n.at);
                out + map(filter(range(1, len(ns)), fn(i) => contains(ns[i].tags, "slur") && ns[i - 1].pitch >= 76 && ns[i].pitch < 76),
                    fn(i) => [seconds_at(ns[i].at, timing) / 1s + ns[i].offset / 1s, seconds_at(ns[i].duration, timing) / 1s, ns[i].pitch])
            }});
            song({{tempo: {tempo}, tracks: [track("o", stack(parts), w.instrument(spec))], tail: 0.5}})"#,
            cases.join(", ")
        ),
    )
    .unwrap();
    let slurs: Vec<[f64; 3]> = serde_json::from_value(
        muz::lang::evaluate(&path, None, Some("slurs()"))
            .unwrap()
            .display_json(),
    )
    .unwrap();
    let out = dir.join(format!("descents{tempo}.wav"));
    muz::render::render(&path, &out, None, None, &[], 48_000, 128).unwrap();
    let x = hound::WavReader::open(out)
        .unwrap()
        .samples::<f32>()
        .step_by(2)
        .map(Result::unwrap)
        .collect();
    (x, slurs)
}

#[test]
fn a_slur_down_out_of_the_oboes_octave_register_speaks_below_it() {
    let dir = tempfile::tempdir().unwrap();
    let phrases = [
        "F5:s E5:s@slur D5:s@slur C#5:s@slur",
        "A4:q D5:e F5:e E5:e@slur D5:e@slur",
        "F5:e D5:e@slur",
        "E5:s Eb5:s@slur",
        "G5:e C5:e@slur",
        "D6:e D5:e@slur",
    ];
    let mut slurs = 0;
    let mut cracks = vec![];
    for tempo in [108, 160] {
        let (x, found) = render(dir.path(), tempo, &phrases, &[0.3, 0.6, 0.9], 4);
        for [at, length, pitch] in found {
            let f0 = 440. * 2f64.powf((pitch - 69.) / 12.);
            // The octave above: its fundamental 20 dB under its second
            // partial (an oboe's own D5 has it about 9 dB under).
            let frames = balance(&x, f0, at, at + length);
            let longest = frames
                .iter()
                .fold((0, 0), |(run, most), b| {
                    let run = if *b > 20. { run + 10 } else { 0 };
                    (run, most.max(run))
                })
                .1;
            slurs += 1;
            // A lock: the octave above for 60 ms or more in a row, where a
            // slur's own crossing takes up to about 40.
            if longest >= 60 {
                cracks.push((tempo, pitch, (at * 1000.).round() / 1000., longest));
            }
        }
    }
    assert!(slurs >= 140, "{slurs} slurs");
    assert!(
        cracks.is_empty(),
        "{} of {slurs} slurs sound the octave above: {cracks:?}",
        cracks.len()
    );
}
