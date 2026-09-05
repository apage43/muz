use muz::render;
fn bounce(src: &str, block: usize) -> Vec<f32> {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("case.muz");
    let w = dir.path().join("out.wav");
    std::fs::write(&p, src).unwrap();
    render::render(&p, &w, None, None, &[], 48000, block).unwrap();
    hound::WavReader::open(w)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}
#[test]
fn parallel_latency_is_aligned_and_trimmed() {
    let single = bounce(
        "song({tracks:[track(\"a\",phrase(\"C4:e E4:e\"),synth(\"bell\"))],tail:0.4})",
        256,
    );
    let parallel = bounce(
        "song({tracks:[track(\"a\",phrase(\"C4:e E4:e\"),synth(\"bell\"),{gain:-6.020599913}),track(\"b\",phrase(\"C4:e E4:e\"),synth(\"bell\"),{gain:-6.020599913,chain:[fx(\"limiter\",{lookahead_ms:7,ceiling_db:0})]})],tail:0.4})",
        97,
    );
    assert_eq!(single.len(), parallel.len());
    let error = single
        .iter()
        .zip(&parallel)
        .map(|(a, b)| (a - b).abs())
        .fold(0., f32::max);
    assert!(
        error < 0.0001,
        "parallel route comb filtering or untrimmed latency: {error}"
    );
    let rack = bounce(
        r#"song({tracks:[track("a",phrase("C4:e E4:e"),synth("bell"),{chain:[rack([[fx("gain",{gain_db:-6.020599913})],[fx("gain",{gain_db:-6.020599913}),fx("limiter",{lookahead_ms:7,ceiling_db:0})]])]})],tail:0.4})"#,
        97,
    );
    let error = single
        .iter()
        .zip(rack)
        .map(|(a, b)| (a - b).abs())
        .fold(0., f32::max);
    assert!(
        error < 0.0001,
        "rack branches were not compensated: {error}"
    );
}

#[test]
fn simultaneous_stems_match_individual_taps_with_latency() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("case.muz");
    std::fs::write(&p,r#"song({tracks:[track("a",phrase("C4:e E4:e"),synth("bell"),{chain:[fx("limiter",{lookahead_ms:7})]}),track("b",phrase("E4:q"),synth("bell"))],tail:0.2})"#).unwrap();
    let s = muz::compile::compile(&p).unwrap().session;
    let options = render::RenderOptions::default();
    let reports = render::stems(s.clone(), &d.path().join("stems"), &options).unwrap();
    for id in ["a", "b"] {
        let out = d.path().join(format!("{id}.wav"));
        render::render_with(
            s.clone(),
            &out,
            &render::RenderOptions {
                tap: Some(id.into()),
                ..options.clone()
            },
            None,
        )
        .unwrap();
        let samples = |p: &std::path::Path| {
            hound::WavReader::open(p)
                .unwrap()
                .samples::<f32>()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            samples(&out),
            samples(&d.path().join(format!("stems/{id}.wav")))
        );
    }
    assert_eq!(reports.len(), 2);
}
#[test]
fn sidechain_and_automation_do_not_depend_on_buffer_boundaries() {
    let src = r#"song({tempo:120, tracks:[track("detector",phrase("C2:s r:e C2:s r:q"),synth("kick"),{gain:-120}),track("pad",phrase("[C4 E4 G4]:h"),synth("pad"),{chain:[fx("compressor",{id:"duck",sidechain:"detector",threshold_db:-35,ratio:8,attack_ms:2,release_ms:70}),fx("eq",{id:"tone",frequency_hz:1000,gain_db:3})],sends:{echo:-120}})], buses:[bus("echo",[fx("delay",{time_beats:0.25,mix:1})])],automation:[automation("pad.tone.gain_db",curve([[0b,-4],[1/2b,6],[2b,-2]])),automation("pad.send.echo",curve([[0b,-120],[1/2b,-6],[1b,-120]],"step"))],tail:0.5})"#;
    let a = bounce(src, 256);
    let b = bounce(src, 97);
    assert_eq!(a.len(), b.len());
    let error = a
        .iter()
        .zip(&b)
        .map(|(a, b)| (a - b).abs())
        .fold(0., f32::max);
    assert!(error < 0.0001, "buffer dependent dynamics: {error}");
}

#[test]
fn isolated_note_sends_keep_overlap_out_of_returns_and_preserve_dry_audio() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("send.muz");
    let source = r#"
        let selected = phrase("E5:q").tag("all","echo").at(1b);
        let material = stack([phrase("C3:w"),selected,cc(64,100),cc(64,0,at=3b)]);
        song({tracks:[track("p",material,synth("bell"),{
            policy:"piano",strict:true,gain:-6,chain:[fx("lowpass",{cutoff_hz:900})],
            note_sends:{echo:{tag:"echo",gain:-12}}
        })],buses:[bus("echo",[fx("delay",{time_beats:0.25,mix:1})])],tail:0.5})
    "#;
    std::fs::write(&p, source).unwrap();
    let c = muz::compile::compile(&p).unwrap();
    assert_eq!(c.score.len(), 1);
    assert_eq!(c.score[0].pattern.notes.len(), 2);
    assert!(c.diagnostics.is_empty());
    let samples = |s: muz::Session, tap: &str| {
        let w = d.path().join("tap.wav");
        render::render_with(
            s,
            &w,
            &render::RenderOptions {
                tap: Some(tap.into()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        hound::WavReader::open(w)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
    };
    let dry = samples(c.session.clone(), "p");
    let wet = samples(c.session.clone(), "echo");
    // The unchanged dry performance and its common insert still sound exactly once.
    let mut dry_only = c.session;
    dry_only.tracks.retain(|t| t.id.as_str() == "p");
    assert_eq!(dry, samples(dry_only, "p"));
    // Independent reference: just the selected note and pedal into the wet bus.
    std::fs::write(&p, r#"song({tracks:[track("reference",stack([phrase("E5:q").at(1b),cc(64,100),cc(64,0,at=3b),rest(4b)]),synth("bell"),{output:"echo",gain:-12,pan:0})],buses:[bus("echo",[fx("delay",{time_beats:0.25,mix:1})])],tail:0.5})"#).unwrap();
    let reference = samples(muz::compile::compile(&p).unwrap().session, "echo");
    assert_eq!(wet.len(), reference.len());
    assert!(wet.iter().any(|v| v.abs() > 1e-4));
    assert!(wet.iter().zip(reference).all(|(a, b)| (a - b).abs() < 1e-5));
    // Splitting the echo must never allow an impossible combined hand through.
    let impossible = source
        .replace(
            "phrase(\"C3:w\")",
            "phrase(\"[C4 D4 F4 G4 A4]:w\").hand(\"right\")",
        )
        .replace("phrase(\"E5:q\")", "phrase(\"E4:q\").hand(\"right\")");
    std::fs::write(&p, impossible).unwrap();
    assert!(muz::compile::compile(&p).is_err());
    std::fs::write(&p, source.replace("tag:\"echo\"", "tag:\"typo\"")).unwrap();
    assert!(muz::compile::compile(&p).is_err());
}
