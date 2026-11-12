//! std/physical parts and std/woodwinds instruments: each speaks, at pitch, across
//! its range and dynamics, with its family's partials, its breath shaped as
//! breath, and its playing rules turned into curves.
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
fn hz(pitch: f64) -> f64 {
    440. * 2f64.powf((pitch - 69.) / 12.)
}
fn db(x: f64) -> f64 {
    20. * x.log10()
}
fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}
/// Cents from `hz` to the strongest component within 30 cents, in 1-cent steps.
fn fine_offset(x: &[f32], hz: f64) -> f64 {
    (-30..=30)
        .map(|c| (c as f64, level(x, hz * 2f64.powf(c as f64 / 1200.))))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
        .0
}
/// The pitch of a short window in cents from `hz`: the autocorrelation's peak
/// near the period, interpolated.
fn period_cents(x: &[f32], hz: f64) -> f64 {
    let corr = |lag: usize| -> f64 {
        x.iter()
            .zip(&x[lag..])
            .map(|(a, b)| (*a as f64) * (*b as f64))
            .sum()
    };
    let period = 48000. / hz;
    let (lo, hi) = ((period * 0.94) as usize, (period * 1.06) as usize + 1);
    let best = (lo..=hi)
        .max_by(|a, b| corr(*a).total_cmp(&corr(*b)))
        .unwrap();
    let (a, b, c) = (corr(best - 1), corr(best), corr(best + 1));
    let lag = best as f64 + 0.5 * (a - c) / (a - 2. * b + c);
    1200. * (period / lag).log2()
}
/// The strongest level within 12 cents of `hz`, in 1-cent steps.
fn peak(x: &[f32], hz: f64) -> f64 {
    (-12..=12)
        .map(|c| level(x, hz * 2f64.powf(c as f64 / 1200.)))
        .fold(0., f64::max)
}
/// The steady middle of each of `notes`, played two seconds apart (held 1.5 s)
/// on `instrument`, a std/woodwinds expression (`w` is the module).
fn steadies(dir: &Path, instrument: &str, notes: &[String]) -> Vec<Vec<f32>> {
    let seq = notes
        .iter()
        .map(|n| format!("{n}.gate(0.75)"))
        .collect::<Vec<_>>()
        .join(",");
    let x = bounce(
        dir,
        &format!(
            r#"use "std/woodwinds" as w;
            song({{tempo:60,tracks:[track("w",seq([{seq}]),{instrument})],tail:0.5}})"#
        ),
    );
    (0..notes.len())
        .map(|k| x[k * 96000 + 36000..k * 96000 + 68400].to_vec())
        .collect()
}
fn plain(pitch: f64, velocity: f64) -> String {
    format!("note({pitch},2b,velocity={velocity})")
}

#[test]
fn each_woodwind_speaks_at_pitch_across_its_range_and_dynamics() {
    let dir = tempfile::tempdir().unwrap();
    for (name, low, high) in [
        ("flute", 60., 96.),
        ("oboe", 58., 91.),
        ("clarinet", 50., 91.),
        ("bass_clarinet", 34., 79.),
        ("bassoon", 34., 75.),
    ] {
        let pitches: [f64; 5] = [
            low,
            low + 7.,
            ((low + high) / 2f64).round(),
            high - 5.,
            high,
        ];
        let notes: Vec<String> = [0.25, 0.9]
            .iter()
            .flat_map(|v| pitches.iter().map(move |p| plain(*p, *v)))
            .collect();
        let got = steadies(dir.path(), &format!("w.{name}()"), &notes);
        for (k, x) in got.iter().enumerate() {
            let pitch = pitches[k % pitches.len()];
            let cents = fine_offset(x, hz(pitch));
            assert!(
                cents.abs() <= 12. && peak(x, hz(pitch)) > 3e-4,
                "{name} {pitch} ({}): {cents} cents, level {}",
                notes[k],
                peak(x, hz(pitch))
            );
        }
        // Harder blowing is louder and brighter at the bottom of the range.
        let (soft, loud) = (&got[0], &got[pitches.len()]);
        assert!(rms(loud) > 1.6 * rms(soft), "{name}: ff not louder than pp");
    }
}
#[test]
fn a_note_stops_when_its_breath_does() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["flute", "oboe", "clarinet", "bassoon"] {
        let x = bounce(
            dir.path(),
            &format!(
                r#"use "std/woodwinds" as w;
                song({{tempo:60,tracks:[track("w",note(62,1b,velocity=0.8),w.{name}())],tail:0.6}})"#
            ),
        );
        assert!(
            x[x.len() - 4800..].iter().all(|v| v.abs() < 1e-3),
            "{name} rings on"
        );
    }
}
#[test]
fn bores_have_their_families_partials_and_breath_brightens_a_reed() {
    let dir = tempfile::tempdir().unwrap();
    let got = steadies(
        dir.path(),
        "w.clarinet()",
        &[plain(57., 0.9), plain(57., 0.3)],
    );
    let oboe = &steadies(dir.path(), "w.oboe()", &[plain(60., 0.9)])[0];
    let (loud, soft) = (&got[0], &got[1]);
    let f = hz(57.);
    // A cylinder closed by a reed favours odd partials; a cone has them all.
    assert!(peak(loud, 3. * f) > 4. * peak(loud, 2. * f));
    let g = hz(60.);
    assert!(peak(oboe, 2. * g) > 0.3 * peak(oboe, g));
    // Harder blowing beats the reed for longer: the upper partials grow faster
    // than the fundamental.
    let ratio = |x: &[f32]| peak(x, 5. * f) / peak(x, f);
    assert!(ratio(loud) > 2. * ratio(soft));
}
#[test]
fn the_flute_is_rich_at_the_bottom_pure_at_the_top_and_louder_up_high() {
    let dir = tempfile::tempdir().unwrap();
    let pitches = [60., 72., 84., 93.];
    let notes: Vec<String> = pitches.iter().map(|p| plain(*p, 0.5)).collect();
    let got = steadies(dir.path(), "w.flute()", &notes);
    let h2 = |k: usize| {
        let f = hz(pitches[k]);
        db(peak(&got[k], 2. * f) / peak(&got[k], f))
    };
    // The jet's deflection grows as the note falls, so it saturates against the
    // edge: the bottom has its second partial from inside the loop (recorded
    // flutes: -5 to +6 dB at C4), the top almost none.
    assert!(h2(0) > -10., "C4 H2 {:.1} dB", h2(0));
    assert!(h2(3) < -15., "A6 H2 {:.1} dB", h2(3));
    // A flute is quieter at the bottom of its range than at the top.
    let lift = db(rms(&got[3]) / rms(&got[0]));
    assert!(lift > 6., "C4 to A6 only {lift:.1} dB");
    // Breath brightens the low register as well as raising it.
    let low = steadies(
        dir.path(),
        "w.flute()",
        &[plain(60., 0.25), plain(60., 0.9)],
    );
    let f = hz(60.);
    let upper = |x: &[f32]| (2..=5).map(|k| peak(x, k as f64 * f)).sum::<f64>() / peak(x, f);
    assert!(
        upper(&low[1]) > 1.25 * upper(&low[0]),
        "{} {}",
        upper(&low[1]),
        upper(&low[0])
    );
}
#[test]
fn a_jet_aimed_off_the_edge_speaks_at_pitch_up_to_its_documented_range() {
    let dir = tempfile::tempdir().unwrap();
    // `offset` is clamped to 0.6, the furthest a jet speaks at pitch at every
    // breath; 0.8 (once a note an octave and a fifth low) now plays as 0.6.
    for offset in ["0", "0.6", "0.8", "-0.6"] {
        let pitches = [60., 66., 72., 78.];
        let notes: Vec<String> = [0.3, 0.9]
            .iter()
            .flat_map(|v| pitches.iter().map(move |p| plain(*p, *v)))
            .collect();
        let got = steadies(dir.path(), &format!("w.flute({{offset:{offset}}})"), &notes);
        for (k, x) in got.iter().enumerate() {
            let pitch = pitches[k % pitches.len()];
            let cents = fine_offset(x, hz(pitch));
            assert!(
                cents.abs() <= 15.,
                "offset {offset}, {}: {cents} cents",
                notes[k]
            );
        }
    }
}
#[test]
fn a_shorter_jet_overblows_the_flute_and_vibrato_moves_its_pitch() {
    let dir = tempfile::tempdir().unwrap();
    let got = steadies(
        dir.path(),
        "w.flute()",
        &[
            plain(67., 0.8),
            format!("{}.express({{lip:0.25}})", plain(67., 0.8)),
            format!("{}.express({{vibrato:1}})", plain(67., 0.8)),
        ],
    );
    let (low, high) = (hz(67.), hz(79.));
    assert!(peak(&got[0], low) > 2. * peak(&got[0], high));
    assert!(peak(&got[1], high) > 5. * peak(&got[1], low));
    let cents = fine_offset(&got[1], high);
    assert!(cents.abs() <= 12., "overblown octave {cents} cents");
    // Vibrato swings the pitch: in 60 ms windows it wanders far more than the
    // plain note's drift.
    let swing = |x: &[f32]| {
        let cents: Vec<f64> = x.chunks(1440).map(|w| period_cents(w, low)).collect();
        let mean = cents.iter().sum::<f64>() / cents.len() as f64;
        (cents.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / cents.len() as f64).sqrt()
    };
    assert!(
        swing(&got[2]) > 2. * swing(&got[0]),
        "vibrato {} plain {}",
        swing(&got[2]),
        swing(&got[0])
    );
}
#[test]
fn a_register_vent_chooses_the_twelfth_and_the_octave() {
    let dir = tempfile::tempdir().unwrap();
    // A low clarinet fingering with its vent open sounds the twelfth, from the
    // long tube's third mode; the fundamental is gone.
    let pitches = [50., 55., 60.];
    let vented: Vec<String> = pitches
        .iter()
        .map(|p| format!("{}.express({{vent:1}})", plain(*p, 0.6)))
        .collect();
    for (pitch, x) in pitches
        .iter()
        .zip(steadies(dir.path(), "w.clarinet()", &vented))
    {
        let f = hz(*pitch);
        let cents = fine_offset(&x, 3. * f);
        assert!(cents.abs() <= 12., "clarinet {pitch} vented: {cents} cents");
        assert!(
            peak(&x, f) < 0.03 * peak(&x, 3. * f),
            "clarinet {pitch} keeps its fundamental"
        );
    }
    // The same for an oboe's octave vent.
    let pitches = [60., 65., 70.];
    let vented: Vec<String> = pitches
        .iter()
        .map(|p| format!("{}.express({{vent:1}})", plain(*p, 0.6)))
        .collect();
    for (pitch, x) in pitches
        .iter()
        .zip(steadies(dir.path(), "w.oboe()", &vented))
    {
        let f = hz(*pitch);
        let cents = fine_offset(&x, 2. * f);
        assert!(cents.abs() <= 12., "oboe {pitch} vented: {cents} cents");
        assert!(
            peak(&x, f) < 0.05 * peak(&x, 2. * f),
            "oboe {pitch} keeps its fundamental"
        );
    }
    // Above each break the instrument fingers the lower register and vents it:
    // the written note sounds in tune, with no residue of the fingered tube.
    for (name, pitches) in [
        ("clarinet", vec![68., 69., 70., 79., 88.]),
        ("bass_clarinet", vec![56., 57., 58., 70.]),
        ("oboe", vec![75., 76., 77., 84.]),
    ] {
        let notes: Vec<String> = pitches.iter().map(|p| plain(*p, 0.6)).collect();
        for (pitch, x) in pitches
            .iter()
            .zip(steadies(dir.path(), &format!("w.{name}()"), &notes))
        {
            let f = hz(*pitch);
            let cents = fine_offset(&x, f);
            assert!(cents.abs() <= 12., "{name} {pitch}: {cents} cents");
            for sub in [1. / 3., 0.5] {
                assert!(
                    peak(&x, f * sub) < 0.05 * peak(&x, f),
                    "{name} {pitch} has a subharmonic {sub}"
                );
            }
        }
    }
}
#[test]
fn a_vent_opened_during_a_note_slurs_to_the_twelfth() {
    let dir = tempfile::tempdir().unwrap();
    let x = bounce(
        dir.path(),
        r#"use "std/woodwinds" as w;
        song({tempo:60,tracks:[track("w",note(50,3b,velocity=0.6).express({vent:[[0,0],[0.5,0],[0.52,1],[1,1]]}),w.clarinet())],tail:0.3})"#,
    );
    let f = hz(50.);
    let (before, after) = (&x[24000..62400], &x[100800..139200]);
    assert!(peak(before, f) > peak(before, 3. * f));
    assert!(peak(after, 3. * f) > 30. * peak(after, f));
}
#[test]
fn breath_turbulence_falls_away_above_a_corner_that_rises_with_the_breath() {
    let dir = tempfile::tempdir().unwrap();
    // The turbulence alone, at a soft and a loud breath.
    let x = bounce(
        dir.path(),
        r#"use "std/signal" as s;
        use "std/physical" as p;
        let e = s.adsr(1ms, 10ms, 1, 10ms);
        let air = voice_patch("air", {velocity_track: 0,
            output: s.mul([p.turbulence(s.velocity(), 440Hz), e]), lifetime: {envelope: e, tail: 0s}});
        song({tempo:60,tracks:[track("a",seq([note(69,2b,velocity=0.2),note(69,2b,velocity=0.9)]),air)],tail:0.3})"#,
    );
    let band = |x: &[f32], lo: f64, hi: f64| {
        let steps = 24;
        (0..steps)
            .map(|k| level(x, lo * (hi / lo).powf(k as f64 / (steps - 1) as f64)).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    let (soft, loud) = (&x[24000..33600], &x[120000..129600]);
    for seg in [soft, loud] {
        // Not white (which is flat): 8-16 kHz sits under 1-2 kHz, as the floor
        // between a recorded clarinet's partials does (10 to 15 dB).
        let tilt = db(band(seg, 8000., 16000.) / band(seg, 1000., 2000.));
        assert!(tilt < -8., "turbulence tilt {tilt:.1} dB");
    }
    // A harder breath moves the corner up: the top gains on the middle.
    let shape = |x: &[f32]| band(x, 6000., 12000.) / band(x, 500., 1000.);
    assert!(shape(loud) > 1.5 * shape(soft));
}
#[test]
fn the_exact_reed_closes_and_its_table_is_its_own_inverse() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("reed.muz");
    std::fs::write(
        &p,
        r#"use "std/physical" as p;
        let zeta = 0.4;
        // u = zeta (1 - x) sqrt(x): shut from x = 1, the most flow at x = 1/3,
        // and flow backwards into the mouth for x < 0.
        assert(p.bernoulli_flow(1, zeta) == 0 && p.bernoulli_flow(1.5, zeta) == 0, "a shut reed passes nothing");
        assert(abs(p.bernoulli_flow(1 / 3, zeta) - zeta * 2 / 3 * pow(1 / 3, 0.5)) < 0.000000001, "peak flow");
        assert(p.bernoulli_flow(-0.5, zeta) < 0, "reverse flow");
        // Each point is [x + u(x), u(x)]: the table maps the reed's input h to u.
        let pts = p.bernoulli_points(zeta);
        assert(len(filter(range(len(pts) - 1), fn(k) => pts[k + 1][0] <= pts[k][0])) == 0, "h rises along the table");
        assert(len(filter(pts, fn(q) => abs(q[1] - p.bernoulli_flow(q[0] - q[1], zeta)) > 0.000000001)) == 0, "u(h - u) = u");
        1"#,
    )
    .unwrap();
    muz::lang::load(&p).unwrap();
}
#[test]
fn playing_rules_turn_marks_into_breath_tongue_and_overlaps() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("rules.muz");
    std::fs::write(
        &p,
        r#"use "std/woodwinds" as w;
        let line = phrase("D4:q E4:q@slur F4:h@niente r:q G4:e@staccato A4:e@accent B4:w");
        let played = w.perform(line, w.specs.oboe, {timing: {tempo: 60}, humanize: 0ms});
        let ns = sort_by(played.notes, fn(n) => n.at);
        fn x(n) = n.data.expression;
        // The first note slurs on: held, overlapping the next attack, tongued.
        assert(ns[0].release_offset > 0ms && x(ns[0]).tongue > 0, "slur start");
        // The slurred note is not tongued and keeps the breath.
        assert(x(ns[1]).tongue == 0, "slurred note tongued");
        // Al niente: the level falls to nothing; the breath stays above threshold.
        let vol = x(ns[2]).volume;
        assert(vol[len(vol) - 1][1] < 0.05 && x(ns[2]).pressure[len(x(ns[2]).pressure) - 1][1] >= 0.2, "niente");
        assert(ns[3].gate == 0.5, "staccato is half length");
        assert(x(ns[4]).pressure[1][1] > x(ns[4]).pressure[0][1] * 1.2, "accent kick");
        // A long note on an instrument with vibrato gets one; the clarinet's does not.
        assert(merge({vibrato: null}, x(ns[5])).vibrato != null, "oboe vibrato");
        let straight = w.perform(line, w.specs.clarinet, {timing: {tempo: 60}, humanize: 0ms});
        assert(merge({vibrato: null}, sort_by(straight.notes, fn(n) => n.at)[5].data.expression).vibrato == null, "clarinet vibrato");
        // Every performed note takes its breath from `pressure`, not velocity.
        assert(len(filter(ns, fn(n) => x(n).velocity_breath != 0)) == 0, "velocity_breath");
        // A piece can rename the marks and replace the rule.
        let renamed = w.perform(phrase("C4:q D4:q@s"), w.specs.flute, {tags: {slur: "s"}, humanize: 0ms});
        assert(sort_by(renamed.notes, fn(n) => n.at)[1].data.expression.tongue == 0, "renamed slur");
        let mine = w.perform(phrase("C4:q"), w.specs.flute, {rule: fn(n, c) => {gate: 0.25}, humanize: 0ms});
        assert(mine.notes[0].gate == 0.25, "own rule");
        song({tempo: 60, tracks: [track("o", played, w.oboe())]})"#,
    )
    .unwrap();
    muz::lang::load(&p).unwrap();
    // And the performance plays: every note of it speaks.
    let out = d.path().join("rules.wav");
    muz::render::render(&p, &out, None, None, &[], 48000, 97).unwrap();
}
#[test]
fn a_derived_instrument_is_a_changed_spec() {
    let dir = tempfile::tempdir().unwrap();
    // An English horn: an oboe a fifth lower, with its bulb's lower formant. The
    // oboe's calibration still holds roughly a fifth down (recalibrate with
    // tools/woodwinds.py for a finished instrument).
    let got = steadies(
        dir.path(),
        r#"w.instrument(w.derive(w.specs.oboe, {name: "english_horn", range: [52, 84], octave_key: 69,
            formants: [[1000Hz, 1.6, 3.5], [2400Hz, 3, 1]]}))"#,
        &[plain(52., 0.6), plain(64., 0.6), plain(76., 0.6)],
    );
    for (pitch, x) in [52., 64., 76.].iter().zip(&got) {
        let cents = fine_offset(x, hz(*pitch));
        assert!(cents.abs() <= 15., "english horn {pitch}: {cents} cents");
    }
}
#[test]
fn two_players_of_one_instrument_breathe_independently_and_repeat_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let render = || {
        let path = dir.path().join("twins.muz");
        let out = dir.path().join("twins.wav");
        std::fs::write(
            &path,
            r#"use "std/woodwinds" as w;
            let line = phrase("D4:h F4:h");
            song({tempo:60,tracks:[track("a",line,w.flute(),{pan:-1}),track("b",line,w.flute(),{pan:1})],tail:0.3})"#,
        )
        .unwrap();
        muz::render::render(&path, &out, None, None, &[], 48000, 97).unwrap();
        hound::WavReader::open(out)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect::<Vec<f32>>()
    };
    let x = render();
    let (left, right): (Vec<f32>, Vec<f32>) = x.chunks(2).map(|f| (f[0], f[1])).unzip();
    // Each track's noise (its breath, its drift) is its own: unison is not one
    // louder flute.
    let diff = left
        .iter()
        .zip(&right)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    assert!(diff > 0.01, "the two flutes are identical");
    assert!(rms(&left) > 0.01 && rms(&right) > 0.01);
    // And a render repeats exactly.
    assert_eq!(x, render());
}
