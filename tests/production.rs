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
