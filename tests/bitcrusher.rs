use muz::{compile, render};

fn bounce(chain: &str, block: usize) -> Vec<f32> {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("case.muz");
    let wav = dir.path().join("out.wav");
    std::fs::write(&src,format!(
        r#"song({{tracks:[track("p",phrase("C4:e E4:e"),synth("glass-lead"),{{chain:[{chain}]}})],tail:0.2}})"#
    )).unwrap();
    render::render(&src, &wav, None, None, &[], 48000, block).unwrap();
    hound::WavReader::open(wav)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn bitcrusher_render_is_block_invariant_and_dry_bypass_is_exact() {
    let effect = r#"fx("bitcrusher",{bits:7,rate_hz:17321,mix:0.6})"#;
    let a = bounce(effect, 256);
    let b = bounce(effect, 97);
    assert_eq!(a, b, "fractional hold clock depends on callback boundaries");
    let dry = bounce("", 256);
    assert_eq!(
        dry,
        bounce(r#"fx("bitcrusher",{bits:4,rate_hz:8000,mix:0})"#, 97)
    );
    assert_eq!(dry, bounce(r#"fx("bitcrusher",{})"#, 97));
    assert_eq!(dry, bounce(r#"fx("bitcrusher",{rate_hz:96000})"#, 97));
    let mse = a
        .iter()
        .zip(&dry)
        .map(|(a, b)| (a - b).powi(2) as f64)
        .sum::<f64>()
        / a.len() as f64;
    assert!(mse > 1e-7, "crusher did not change rendered sound");
}

#[test]
fn bitcrusher_source_validation_and_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("case.muz");
    for params in ["bits:25", "rate_hz:-1", "mix:2", "mystery:1"] {
        std::fs::write(&src,format!(
            r#"song({{tracks:[track("p",phrase("C4:q"),synth("bell"),{{chain:[fx("bitcrusher",{{{params}}})]}})]}})"#
        )).unwrap();
        assert!(compile::compile(&src).is_err(), "accepted {params}");
    }
    let catalog = muz::plugins::native("bitcrusher").unwrap();
    assert_eq!(catalog["parameters"].as_array().unwrap().len(), 3);
}
