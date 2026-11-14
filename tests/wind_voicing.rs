//! The wind player's vocal tract and the tongue's shape (std/physical.wind_player
//! and std/woodwinds): synthetic notes show that each control reaches the model
//! and moves the attack the way a player's does.
use std::path::Path;

const RATE: f64 = 48000.;

/// Renders `notes` (a muz list of notes, two seconds apart) on `instrument`,
/// with the breath noise off, and returns the mix or the probe `tap`.
fn render(dir: &Path, instrument: &str, notes: &str, tap: Option<&str>) -> Vec<f32> {
    let path = dir.join("case.muz");
    let out = dir.join(format!("case-{}.wav", tap.unwrap_or("mix")));
    std::fs::write(
        &path,
        format!(
            r#"use "std/woodwinds" as w;
            song({{tempo: 60, tracks: [track("w", stack({notes}), {instrument})], tail: 0.5}})"#
        ),
    )
    .unwrap();
    muz::render::render_with(
        muz::compile::compile(&path).unwrap().session,
        &out,
        &muz::render::RenderOptions {
            tap: tap.map(|t| vec![format!("w@{t}")]).unwrap_or_default(),
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
}

fn quiet(spec: &str) -> String {
    format!("w.instrument(w.specs.{spec}, {{noise: 0, hiss: 0, finger: 0, drift_cents: 0}})")
}

/// Note `k` of a list: `pitch` held 0.6 s at dynamic `dynamic`, from 2k seconds.
fn note(k: usize, pitch: u8, dynamic: f64, express: &str) -> String {
    format!(
        "note({pitch}, 1b, velocity={dynamic}).gate(0.6).at({}b).express({express})",
        2 * k
    )
}

/// The level of the first four partials of `f0`, each heterodyned and averaged
/// over whole periods (at least 8 ms), in dB every millisecond from `from` s.
fn tone(x: &[f32], f0: f64, from: f64, seconds: f64) -> Vec<f64> {
    let w = (RATE / f0 * (0.008 * f0).ceil().max(1.)).round() as isize;
    let a = (from * RATE) as isize;
    let n = (seconds * RATE) as isize;
    let (lo, hi) = (a - w / 2, a + n + w);
    let sample = |i: isize| {
        if i < 0 {
            0.
        } else {
            *x.get(i as usize).unwrap_or(&0.) as f64
        }
    };
    let mut power = vec![0.; n as usize];
    for h in 1..=4 {
        let f = f0 * h as f64;
        if f > 6000. {
            break;
        }
        // Prefix sums of the heterodyned signal, from `lo`.
        let mut sums = vec![(0., 0.)];
        for i in lo..hi {
            let p = std::f64::consts::TAU * f * i as f64 / RATE;
            let (re, im) = sums[sums.len() - 1];
            sums.push((re + sample(i) * p.cos(), im - sample(i) * p.sin()));
        }
        for (k, p) in power.iter_mut().enumerate() {
            let start = (a + k as isize - w / 2 - lo) as usize;
            let (re, im) = (
                sums[start + w as usize].0 - sums[start].0,
                sums[start + w as usize].1 - sums[start].1,
            );
            *p += (re / w as f64).powi(2) + (im / w as f64).powi(2);
        }
    }
    power
        .iter()
        .step_by(48)
        .map(|p| 10. * (p + 1e-24).log10())
        .collect()
}

/// Milliseconds from note `k`'s attack until its tone first comes within 12 dB
/// of its peak in the next 300 ms.
fn onset(x: &[f32], k: usize, pitch: u8) -> f64 {
    let f0 = 440. * 2f64.powf((pitch as f64 - 69.) / 12.);
    let env = tone(x, f0, 2. * k as f64, 0.3);
    let peak = env.iter().cloned().fold(f64::MIN, f64::max);
    env.iter().position(|v| *v >= peak - 12.).unwrap() as f64
}

/// The tone's level `ms` after note `k`'s attack, dB re its level 400 ms in.
fn early(x: &[f32], k: usize, pitch: u8, ms: usize) -> f64 {
    let f0 = 440. * 2f64.powf((pitch as f64 - 69.) / 12.);
    let env = tone(x, f0, 2. * k as f64, 0.45);
    env[ms] - env[400]
}

#[test]
fn a_tract_tuned_to_the_note_speeds_the_altissimo_and_a_mistuned_one_slows_it() {
    let dir = tempfile::tempdir().unwrap();
    // Clarinet D6 (1175 Hz), softly: the resting tract (1200 Hz, 0.5 of the
    // bore's impedance), then the tongue raised eleven times as strong and
    // tuned a little above the note (`tract` 0.51: 1243 Hz), then as strong
    // but an octave low.
    let notes = format!(
        "[{}, {}, {}]",
        note(0, 86, 0.45, "{tongue: 0.4}"),
        note(1, 86, 0.45, "{tongue: 0.4, voicing: 0.9375, tract: 0.51}"),
        note(2, 86, 0.45, "{tongue: 0.4, voicing: 0.9375, tract: 0.26}")
    );
    let x = render(dir.path(), &quiet("clarinet"), &notes, None);
    let (rest, tuned, mistuned) = (onset(&x, 0, 86), onset(&x, 1, 86), onset(&x, 2, 86));
    assert!(tuned < rest - 5., "tuned {tuned} ms, resting {rest} ms");
    assert!(
        mistuned > tuned + 5.,
        "mistuned {mistuned} ms, tuned {tuned} ms"
    );
}

#[test]
fn a_model_without_a_tract_ignores_the_voicing_controls() {
    let dir = tempfile::tempdir().unwrap();
    // The std flute builds no tract (its `voicing` is 0).
    let plain = render(
        dir.path(),
        &quiet("flute"),
        &format!("[{}]", note(0, 79, 0.5, "{}")),
        None,
    );
    let voiced = render(
        dir.path(),
        &quiet("flute"),
        &format!("[{}]", note(0, 79, 0.5, "{voicing: 1, tract: 0.2}")),
        None,
    );
    assert_eq!(plain, voiced);
}

#[test]
fn a_ku_speaks_later_and_softer_than_a_ta() {
    let dir = tempfile::tempdir().unwrap();
    // The player's own syllables (std/woodwinds `syllables`), the ku a little
    // weaker as the player tongues it.
    let notes = r#"[note(62, 1b, velocity=0.55).gate(0.6).express(merge({tongue: 0.4}, w.syllable(w.syllables.ta))),
        note(62, 1b, velocity=0.55).gate(0.6).at(2b).express(merge({tongue: 0.3}, w.syllable(w.syllables.ku)))]"#;
    let x = render(dir.path(), &quiet("clarinet"), notes, None);
    let (ta, ku) = (onset(&x, 0, 62), onset(&x, 1, 62));
    assert!(ku > ta + 3., "ku {ku} ms, ta {ta} ms");
    let (ta20, ku20) = (early(&x, 0, 62, 20), early(&x, 1, 62, 20));
    assert!(
        ku20 < ta20 - 3.,
        "at 20 ms: ku {ku20:.1} dB, ta {ta20:.1} dB"
    );
}

#[test]
fn the_tongue_lets_the_reed_go_over_its_release_and_its_burst_lasts_its_length() {
    let dir = tempfile::tempdir().unwrap();
    // The same tongue let go as a firm "ta" (3 ms) and as a "da" (12 ms): the
    // flow through the reed opens slower behind the "da".
    let notes = r#"[note(62, 1b, velocity=0.55).gate(0.6).express(merge({tongue: 0.4}, w.syllable(w.syllables.firm))),
        note(62, 1b, velocity=0.55).gate(0.6).at(2b).express(merge({tongue: 0.4}, w.syllable(w.syllables.da)))]"#;
    let flow = render(dir.path(), &quiet("clarinet"), notes, Some("flow"));
    let mean = |from: usize, ms: usize| {
        flow[from..from + ms * 48]
            .iter()
            .map(|v| *v as f64)
            .sum::<f64>()
            / (ms * 48) as f64
    };
    let (firm, da) = (mean(0, 3), mean(96000, 3));
    assert!(
        firm > 0.01 && da < firm * 0.5,
        "flow in the first 3 ms: firm {firm:.3}, da {da:.3}"
    );
    // An accent's burst outlasts a light tongue's at the same strength: the
    // blowing dynamic is above its steady value for longer.
    let notes = r#"[note(62, 1b, velocity=0.55).gate(0.6).express(merge({tongue: 0.8}, w.syllable(w.syllables.firm))),
        note(62, 1b, velocity=0.55).gate(0.6).at(2b).express(merge({tongue: 0.8}, w.syllable(w.syllables.light)))]"#;
    let dynamic = render(dir.path(), &quiet("clarinet"), notes, Some("dynamic"));
    let lasts = |from: usize| {
        let steady = dynamic[from + 400 * 48] as f64;
        let peak = dynamic[from..from + 200 * 48]
            .iter()
            .cloned()
            .fold(f32::MIN, f32::max) as f64;
        dynamic[from..from + 400 * 48]
            .iter()
            .rposition(|v| *v as f64 > steady + (peak - steady) * 0.1)
            .unwrap() as f64
            / 48.
    };
    let (accent, light) = (lasts(0), lasts(96000));
    assert!(
        accent > light * 2.5,
        "burst: accent {accent:.1} ms, light {light:.1} ms"
    );
}

#[test]
fn the_player_voices_slurs_and_the_altissimo_and_shapes_its_tongue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("player.muz");
    std::fs::write(
        &path,
        r#"use "std/woodwinds" as w;
        fn x(p) = map(sort_by(p.notes, fn(n) => n.at), fn(n) => n.data.expression);
        let still = {timing: {tempo: 100}, accept: true};
        // A slur up across the clarinet's break, a slur down across it, a
        // tongued altissimo note, and an accent.
        let cl = x(w.play(phrase("G4:q D5:q@slur G4:q@slur r:q D6:q r:q C5:q@accent"), w.specs.clarinet, still));
        assert(cl[1].voicing[0][1] > 0.5 && cl[1].tract[0][1] < 0.5 && cl[1].voicing[1][1] == 0.5, "up: " + str(cl[1]));
        assert(cl[2].voicing[0][1] < 0.5 && !contains(keys(cl[2]), "tract"), "down: " + str(cl[2]));
        assert(cl[3].voicing > 0.9 && abs(cl[3].tract - 0.51) < 0.01, "altissimo: " + str(cl[3]));
        assert(cl[4].tongue_burst > w.syllable(w.syllables.ta).tongue_burst && cl[4].tongue_release < w.syllable(w.syllables.ta).tongue_release, "accent");
        // Fast double tonguing on the flute: light tu, back-tongue ku with the
        // tract shaped by the back of the tongue; a legato tongue is a da.
        let fl = x(w.play(phrase("A5:s A5:s A5:s A5:s A5:s A5:s r:q B5:q"), w.specs.flute, merge(still, {timing: {tempo: 120}, soft: fn(n) => n.pitch == 83})));
        assert(fl[0].tongue_back == 0 && fl[1].tongue_back == 1 && fl[1].tongue < fl[0].tongue, "tu-ku");
        assert(fl[0].tongue_burst < w.syllable(w.syllables.ta).tongue_burst, "light: " + str(fl[0]));
        assert(kind(fl[1].voicing) == "list" && fl[1].tract[0][1] < 0.5, "ku tract");
        assert(fl[6].tongue_release == w.syllable(w.syllables.da).tongue_release, "da: " + str(fl[6]));
        // Brightness stays the embouchure: the player writes none.
        assert(len(filter(cl + fl, fn(e) => contains(keys(e), "brightness"))) == 0, "brightness");
        1"#,
    )
    .unwrap();
    muz::lang::load(&path).unwrap();
}
