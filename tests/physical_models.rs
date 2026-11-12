//! std/physical recipes: each family speaks, at pitch, at both ends of its dynamics.
use std::path::Path;
fn bounce(dir: &Path, source: &str) -> Vec<f32> {
    let path = dir.join("case.muz");
    let out = dir.join("case.wav");
    std::fs::write(&path, source).unwrap();
    muz::render::render(&path, &out, None, None, &[], 48000, 97).unwrap();
    hound::WavReader::open(out)
        .unwrap()
        .samples::<f32>()
        .step_by(2)
        .map(Result::unwrap)
        .collect()
}
fn level(x: &[f32], hz: f64) -> f64 {
    let (mut re, mut im) = (0., 0.);
    for (n, v) in x.iter().enumerate() {
        let phase = std::f64::consts::TAU * hz * n as f64 / 48000.;
        re += *v as f64 * phase.cos();
        im += *v as f64 * phase.sin();
    }
    re.hypot(im) / x.len() as f64
}
/// Cents from `hz` to the strongest component within a quarter tone of it.
fn offset(x: &[f32], hz: f64) -> (f64, f64) {
    (-10..=10)
        .map(|step| {
            let cents = step as f64 * 5.;
            (cents, level(x, hz * 2f64.powf(cents / 1200.)))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
}
fn hz(pitch: f64) -> f64 {
    440. * 2f64.powf((pitch - 69.) / 12.)
}
#[test]
fn each_family_speaks_at_pitch_across_its_range_and_dynamics() {
    let dir = tempfile::tempdir().unwrap();
    for (recipe, pitches) in [
        ("clarinet", [50., 67., 88.]),
        ("flute", [60., 76., 93.]),
        ("oboe", [58., 72., 88.]),
    ] {
        for velocity in [0.1, 0.9] {
            for pitch in pitches {
                let x = bounce(
                    dir.path(),
                    &format!(
                        r#"use "std/physical" as p;
                    song({{tempo:60,tracks:[track("w",note({pitch},2b,velocity={velocity}),p.{recipe}())],tail:0.3}})"#
                    ),
                );
                let steady = &x[48000..86400];
                let (cents, peak) = offset(steady, hz(pitch));
                assert!(
                    cents.abs() <= 15. && peak > 1e-3,
                    "{recipe} {pitch} at {velocity}: {cents} cents, level {peak}"
                );
                // The note stops when the breath does.
                assert!(x[x.len() - 2400..].iter().all(|v| v.abs() < 1e-3));
            }
        }
    }
}
#[test]
fn bores_have_their_families_harmonics_and_breath_brightens_a_reed() {
    let dir = tempfile::tempdir().unwrap();
    let play = |recipe: &str, velocity: f64| {
        let x = bounce(
            dir.path(),
            &format!(
                r#"use "std/physical" as p;
            song({{tempo:60,tracks:[track("w",note(57,2b,velocity={velocity}),p.{recipe})],tail:0.3}})"#
            ),
        );
        let steady = x[48000..86400].to_vec();
        move |harmonic: f64| level(&steady, 220. * harmonic)
    };
    // A cylinder closed by a reed has no even harmonics; a cone has them all.
    let clarinet = play("clarinet()", 0.9);
    assert!(clarinet(3.) > 10. * clarinet(2.));
    let oboe = play("oboe()", 0.9);
    assert!(oboe(2.) > 10. * clarinet(2.) && oboe(2.) > 0.1 * oboe(1.));
    // Harder blowing closes the reed for longer: the upper partials grow faster
    // than the fundamental.
    let soft = play("clarinet()", 0.1);
    assert!(clarinet(5.) / clarinet(1.) > 2. * soft(5.) / soft(1.));
}
#[test]
fn a_shorter_jet_overblows_the_flute_and_vibrato_moves_its_pitch() {
    let dir = tempfile::tempdir().unwrap();
    let play = |express: &str| {
        bounce(
            dir.path(),
            &format!(
                r#"use "std/physical" as p;
            song({{tempo:60,tracks:[track("w",note(67,2b,velocity=0.8){express},p.flute())],tail:0.3}})"#
            ),
        )
    };
    let plain = play("");
    let overblown = play(".express({jet:0.25})");
    let (low, high) = (hz(67.), hz(79.));
    assert!(level(&plain[48000..86400], low) > 5. * level(&plain[48000..86400], high));
    assert!(level(&overblown[48000..86400], high) > 5. * level(&overblown[48000..86400], low));
    // Vibrato spreads the fundamental's energy away from the centre frequency.
    let vibrato = play(".express({vibrato:1})");
    assert!(level(&vibrato[48000..86400], low) < 0.8 * level(&plain[48000..86400], low));
}
