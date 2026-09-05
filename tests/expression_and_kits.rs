//! Synthetic signals protect note identity, bounded performance allocation and kit lifecycle.
fn source(root: &std::path::Path, text: &str) -> std::path::PathBuf {
    let p = root.join("case.muz");
    std::fs::write(&p, text).unwrap();
    p
}
fn samples(path: &std::path::Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}
#[test]
fn voice_expression_is_isolated_and_independent_of_render_block_size() {
    let d = tempfile::tempdir().unwrap();
    let patch = r#"voice_patch("test",{nodes:[{id:"env",op:"adsr",attack:0.001,decay:0.1,sustain:0.7,release:0.05},{id:"osc",op:"osc"},{id:"out",op:"mul",inputs:["env","osc"]}],output:"out"})"#;
    let base = format!(
        r#"song({{tracks:[track("test",stack([phrase("C4:w").express({{volume:0}}),phrase("E4:w").express({{tuning:[[0,0],[1,0.7]],pan:0.8}})]),{patch})],tail:0.1}})"#
    );
    let p = source(d.path(), &base);
    let a = d.path().join("a.wav");
    let b = d.path().join("b.wav");
    muz::render::render(&p, &a, None, None, &[], 48000, 256).unwrap();
    muz::render::render(&p, &b, None, None, &[], 48000, 97).unwrap();
    let a = samples(&a);
    let b = samples(&b);
    assert_eq!(a.len(), b.len());
    assert!(a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-6));
    let p = source(
        d.path(),
        &base.replace("phrase(\"C4:w\").express({volume:0}),", ""),
    );
    let c = d.path().join("c.wav");
    muz::render::render(&p, &c, None, None, &[], 48000, 256).unwrap();
    let c = samples(&c);
    assert!(a.iter().any(|v| v.abs() > 0.01));
    assert!(
        a.iter().zip(&c).all(|(a, c)| (a - c).abs() < 1e-6),
        "one note's volume changed the overlapping voice"
    );
}
#[test]
fn held_finger_and_tonal_anchors_survive_search() {
    let d = tempfile::tempdir().unwrap();
    let p = source(
        d.path(),
        r#"song({tracks:[track("p",stack([phrase("E4:w").gate(1).annotate("all",{finger:3}),phrase("[C4 G4]:q [D4 F4]:q").at(1b)]).hand("right"),synth("bell"),{policy:"piano",strict:true})]})"#,
    );
    let c = muz::compile::compile(&p).unwrap();
    let p = &c.score[0].pattern;
    assert!(c.diagnostics.is_empty());
    assert_eq!(p.notes[0].data["finger"], 3);
    assert!(p.notes[1..].iter().all(|n| n.data["finger"] != 3));
    let p = source(
        d.path(),
        r#"chords("C F G C").tag("last","fixed").voicelead(low=48,high=84)"#,
    );
    let (v, _) = muz::lang::load(&p).unwrap();
    let p = v.pattern().unwrap();
    assert!(p.notes.last().unwrap().tags.contains("fixed"));
    assert_eq!(p.notes.last().unwrap().pitch, 55.);
}
#[test]
fn sampled_round_robins_and_choke_release_are_audible() {
    let d = tempfile::tempdir().unwrap();
    for (name, level) in [("a", 0.1f32), ("b", 0.3)] {
        let mut w = hound::WavWriter::create(
            d.path().join(format!("{name}.wav")),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..48000 {
            w.write_sample(level).unwrap();
        }
        w.finalize().unwrap();
    }
    let p = source(
        d.path(),
        r#"song({tempo:120,tracks:[track("hat",stack([phrase("C4:e C4:e").velocity(1),cc(120,0,at=3/4b)]),sample([{path:"a.wav",velocity:[0.5,1]},{path:"b.wav",velocity:[0.5,1]}],{root:60,one_shot:true,attack_ms:0,gain_db:0}))],tail:0.1})"#,
    );
    let out = d.path().join("out.wav");
    muz::render::render(&p, &out, None, None, &[], 48000, 97).unwrap();
    let x = samples(&out);
    assert!((x[4800 * 2] - 0.1).abs() < 1e-5);
    assert!(
        (x[14000 * 2] - 0.4).abs() < 1e-5,
        "round robin did not choose second zone"
    );
    assert!(x[19000 * 2].abs() < 1e-4, "one-shot voice ignored choke");
}
