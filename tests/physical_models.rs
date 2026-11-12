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
    assert!(clarinet(5.) / clarinet(1.) > 1.5 * soft(5.) / soft(1.));
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
    // The fundamental leads its octave (a flute's second partial is 5 to 12 dB
    // under it); overblown, the octave is the note.
    assert!(level(&plain[48000..86400], low) > 2. * level(&plain[48000..86400], high));
    assert!(level(&overblown[48000..86400], high) > 5. * level(&overblown[48000..86400], low));
    let (cents, _) = offset(&overblown[48000..86400], high);
    assert!(cents.abs() <= 10., "overblown octave {cents} cents");
    // Vibrato spreads the fundamental's energy away from the centre frequency.
    let vibrato = play(".express({vibrato:1})");
    assert!(level(&vibrato[48000..86400], low) < 0.8 * level(&plain[48000..86400], low));
}

fn db(x: f64) -> f64 {
    20. * x.log10()
}
/// Cents from `hz` to the strongest component within 30 cents, in 1-cent steps.
fn fine_offset(x: &[f32], hz: f64) -> f64 {
    (-30..=30)
        .map(|c| (c as f64, level(x, hz * 2f64.powf(c as f64 / 1200.))))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
        .0
}
/// The strongest level within 12 cents of `hz`, in 1-cent steps.
fn peak(x: &[f32], hz: f64) -> f64 {
    (-12..=12)
        .map(|c| level(x, hz * 2f64.powf(c as f64 / 1200.)))
        .fold(0., f64::max)
}
fn steady(dir: &Path, recipe: &str, note: &str) -> Vec<f32> {
    let x = bounce(
        dir,
        &format!(
            r#"use "std/physical" as p;
            song({{tempo:60,tracks:[track("w",{note},p.{recipe})],tail:0.3}})"#
        ),
    );
    x[48000..86400].to_vec()
}
#[test]
fn a_lopsided_jet_gives_the_flute_its_second_partial_from_breath() {
    let dir = tempfile::tempdir().unwrap();
    for pitch in [60., 67., 72., 79., 84., 91., 96.] {
        let f = hz(pitch);
        let x = steady(
            dir.path(),
            "flute()",
            &format!("note({pitch},2b,velocity=0.6)"),
        );
        let cents = fine_offset(&x, f);
        let h2 = db(peak(&x, 2. * f) / peak(&x, f));
        assert!(cents.abs() <= 9., "flute {pitch}: {cents} cents");
        // The partials come from inside the loop: the jet's offset, not a stage
        // after the pipe. Mezzo-forte puts the octave 5 to 13 dB down.
        assert!((-13.5..=-5.).contains(&h2), "flute {pitch}: H2 {h2:.1} dB");
        // A jet aimed at the edge is symmetric and has almost none.
        let centred = steady(
            dir.path(),
            "flute({offset:0})",
            &format!("note({pitch},2b,velocity=0.6)"),
        );
        assert!(
            db(peak(&centred, 2. * f) / peak(&centred, f)) < -35.,
            "centred flute {pitch}"
        );
    }
    // Breath strengthens the second partial: it is a property of the jet.
    let f = hz(67.);
    let soft = steady(dir.path(), "flute()", "note(67,2b,velocity=0.3)");
    let loud = steady(dir.path(), "flute()", "note(67,2b,velocity=0.9)");
    let ratio = |x: &[f32]| peak(x, 2. * f) / peak(x, f);
    assert!(
        ratio(&loud) > 1.5 * ratio(&soft),
        "{} {}",
        ratio(&loud),
        ratio(&soft)
    );
}
#[test]
fn a_register_vent_chooses_the_twelfth_and_the_octave() {
    let dir = tempfile::tempdir().unwrap();
    // A low clarinet fingering with its vent open sounds the twelfth, from the
    // long tube's third mode; the fundamental is gone.
    for pitch in [50., 55., 60.] {
        let f = hz(pitch);
        let x = steady(
            dir.path(),
            "clarinet()",
            &format!("note({pitch},2b,velocity=0.6).express({{vent:1}})"),
        );
        let cents = fine_offset(&x, 3. * f);
        assert!(cents.abs() <= 9., "clarinet {pitch} vented: {cents} cents");
        assert!(
            peak(&x, f) < 0.03 * peak(&x, 3. * f),
            "clarinet {pitch} vented keeps its fundamental"
        );
    }
    // The same for an oboe's octave vent.
    for pitch in [58., 65., 72.] {
        let f = hz(pitch);
        let x = steady(
            dir.path(),
            "oboe()",
            &format!("note({pitch},2b,velocity=0.6).express({{vent:1}})"),
        );
        let cents = fine_offset(&x, 2. * f);
        assert!(cents.abs() <= 9., "oboe {pitch} vented: {cents} cents");
        assert!(
            peak(&x, f) < 0.03 * peak(&x, 2. * f),
            "oboe {pitch} vented keeps its fundamental"
        );
    }
    // Above the break the recipes finger the lower register and vent it: the
    // written note sounds, in tune, with a closed pipe's odd partials leading.
    for (recipe, pitch) in [
        ("clarinet()", 69.),
        ("clarinet()", 79.),
        ("clarinet()", 88.),
        ("oboe()", 76.),
        ("oboe()", 84.),
    ] {
        let f = hz(pitch);
        let x = steady(
            dir.path(),
            recipe,
            &format!("note({pitch},2b,velocity=0.6)"),
        );
        let cents = fine_offset(&x, f);
        assert!(cents.abs() <= 9., "{recipe} {pitch}: {cents} cents");
        for sub in [1. / 3., 0.5] {
            assert!(
                peak(&x, f * sub) < 0.05 * peak(&x, f),
                "{recipe} {pitch} has a subharmonic {sub}"
            );
        }
    }
}
#[test]
fn a_vent_opened_during_a_note_slurs_to_the_twelfth() {
    let dir = tempfile::tempdir().unwrap();
    let x = bounce(
        dir.path(),
        r#"use "std/physical" as p;
        song({tempo:60,tracks:[track("w",note(50,3b,velocity=0.6).express({vent:[[0,0],[0.5,0],[0.52,1],[1,1]]}),p.clarinet())],tail:0.3})"#,
    );
    let f = hz(50.);
    let (before, after) = (&x[24000..62400], &x[100800..139200]);
    // Closed, the fundamental leads; open, it is gone and the twelfth sounds.
    assert!(peak(before, f) > peak(before, 3. * f));
    assert!(peak(after, 3. * f) > 30. * peak(after, f));
}
