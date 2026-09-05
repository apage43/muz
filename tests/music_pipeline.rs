use muz::{compile, lang};
fn source(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("test.muz");
    std::fs::write(&p, text).unwrap();
    (d, p)
}
#[test]
fn functions_and_reuse_preserve_exact_time_and_annotations() {
    let (_d, p) = source(
        "fn motif(p) = phrase(p).tag(\"last\", \"echo\"); let main = motif(\"C4:e D4:q\").repeat(3).transpose(7);",
    );
    let (v, _) = lang::load(&p).unwrap();
    let p = v.pattern().unwrap();
    assert_eq!(p.span, muz::music::decimal("9/2").unwrap());
    let tagged: Vec<_> = p.notes.iter().filter(|n| n.tags.contains("echo")).collect();
    assert_eq!(tagged.len(), 3);
    assert!(tagged.iter().all(|n| n.pitch == 69.0));
    assert_ne!(tagged[0].key, tagged[1].key);
}
#[test]
fn dimensional_mistakes_and_invalid_music_fail_at_source() {
    let (_d, p) = source("let main = 1b + 2ms;");
    assert!(lang::load(&p).unwrap_err().to_string().contains("byte"));
    let (_d, p) = source("let main = phrase(\"C4:0\");");
    assert!(lang::load(&p).is_err());
}
#[test]
fn native_render_has_the_same_timing_at_different_block_sizes() {
    let (d, p) = source(
        "song({tempo:123,tracks:[track(\"lead\",phrase(\"C4:e E4:s G4:s B4:q\"),synth(\"bell\"))],tail:0.2})",
    );
    let a = d.path().join("a.wav");
    let b = d.path().join("b.wav");
    muz::render::render(&p, &a, None, None, &[], 48000, 256).unwrap();
    muz::render::render(&p, &b, None, None, &[], 48000, 97).unwrap();
    let a: Vec<f32> = hound::WavReader::open(a)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect();
    let b: Vec<f32> = hound::WavReader::open(b)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect();
    assert_eq!(a.len(), b.len());
    let err = a
        .iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(err < 1e-5, "render depends on block size: {err}");
    assert!(a.iter().any(|x| x.abs() > 0.01));
}
#[test]
fn piano_checks_simultaneous_material_across_voices() {
    let (_d, p) = source(
        "song({tracks:[track(\"p\",stack([phrase(\"[C4 D4 E4]:q\"),phrase(\"[F4 G4 A4]:q\")]).hand(\"right\"),synth(\"bell\"),{policy:\"piano\"})]})",
    );
    let c = compile::compile(&p).unwrap();
    assert!(c.diagnostics.iter().any(|d| d.code == "piano.capacity"));
}

#[test]
fn gate_scaling_preserves_authored_articulation_and_other_material() {
    let (_d, p) = source(
        r#"
        let motif = seq([phrase("C4:q").gate(0.4), phrase("E4:q").gate(0.8)]).tag("last","echo");
        let main = stack([motif, cc(64,100,at=1b)]).scale_gate(factor=0.5);
    "#,
    );
    let (v, _) = lang::load(&p).unwrap();
    let scaled = v.pattern().unwrap();
    assert_eq!(
        scaled.notes.iter().map(|n| n.gate).collect::<Vec<_>>(),
        vec![0.2, 0.4]
    );
    assert_eq!(scaled.span, muz::music::b(2));
    assert_eq!(scaled.raw.len(), 1);
    assert!(scaled.notes[1].tags.contains("echo"));
    let (_d, p) = source(r#"phrase("C4:q").gate(0.4).gate(value=0.7)"#);
    assert_eq!(
        lang::load(&p).unwrap().0.pattern().unwrap().notes[0].gate,
        0.7
    );
    for op in ["gate", "scale_gate"] {
        let (_d, p) = source(&format!("phrase(\"C4:q\").{op}(0)"));
        assert!(lang::load(&p).is_err());
    }
}
