use std::path::Path;

fn write_source(dir: &Path, source: &str) -> std::path::PathBuf {
    let path = dir.join("staple.muz");
    std::fs::write(&path, source).unwrap();
    path
}

#[test]
fn staple_recovery_shifts_forward_flow_with_monotonic_abscissae() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        r#"use "std/physical" as p;
        fn check(zeta) {
            let flat = p.bernoulli_points(zeta);
            let recovered = p.bernoulli_points(zeta, 1);
            assert(len(flat) == len(recovered), "same sampling");
            map(range(len(flat)), fn(i) => {
                let a = flat[i];
                let b = recovered[i];
                assert(a[1] == b[1], "same flow at the constriction");
                assert(abs(b[0] - (a[0] - pow(max(0, a[1]), 2))) < 0.000000000001, "forward recovery");
                if i > 0 { assert(recovered[i - 1][0] < b[0], "invertible") } else { true }
            })
        }
        map([0.01, 0.32, 0.55, 0.99], check)"#,
    );
    muz::lang::load(&path).unwrap();
}

fn render(dir: &Path, instrument: &str, pitch: u8, recovery: &str) -> Vec<f32> {
    let source = write_source(
        dir,
        &format!(
            r#"use "std/woodwinds" as w;
            let model = w.{instrument}({{velocity_breath: 0, noise: 0, hiss: 0, drift_cents: 0{recovery}}});
            let line = note({pitch}, 1b).gate(1).express({{pressure: 0.6}});
            song({{tempo: 120, tracks: [track("wind", line, model)], tail: 0.25}})"#
        ),
    );
    let output = dir.join("staple.wav");
    muz::render::render(&source, &output, None, None, &[], 48_000, 128).unwrap();
    hound::WavReader::open(output)
        .unwrap()
        .samples()
        .step_by(2)
        .map(Result::unwrap)
        .collect()
}

#[test]
fn oboe_and_bassoon_can_opt_in_to_bounded_staple_recovery() {
    let dir = tempfile::tempdir().unwrap();
    for (instrument, pitch) in [("oboe", 67), ("bassoon", 55)] {
        let baseline = render(dir.path(), instrument, pitch, "");
        let disabled = render(dir.path(), instrument, pitch, ", staple_recovery: 0");
        assert_eq!(disabled, baseline);
        let enabled = render(dir.path(), instrument, pitch, ", staple_recovery: 1");
        assert!(enabled.iter().all(|x| x.is_finite() && x.abs() < 1.));
        let steady = &enabled[9_600..19_200];
        assert!(
            steady.iter().any(|x| x.abs() > 0.001),
            "{instrument} is silent"
        );
        let delta = steady
            .iter()
            .zip(&baseline[9_600..])
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(delta > 0.001, "{instrument}: staple difference {delta}");
    }
}
