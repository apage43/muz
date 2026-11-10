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
    assert!(
        lang::load(&p).is_err(),
        "beats and seconds must not mix without an explicit time coordinate"
    );
    let (_d, p) = source("let main = phrase(\"C4:0\");");
    assert!(lang::load(&p).is_err());
}

#[test]
fn pitch_text_accepts_double_accidentals_in_phrases_and_pitch_values() {
    let (_d, p) = source(r#"phrase("C##5:q Ebb5:q B##4:q Cbb5:q")"#);
    let value = lang::load(&p).unwrap().0;
    assert_eq!(
        value
            .pattern()
            .unwrap()
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect::<Vec<_>>(),
        vec![74., 74., 73., 70.]
    );
    let (_d, p) =
        source(r#"[pitch("C##5"), pitch("Ebb5"), pitch("C##",octave=5), pitch("Ebb",octave=5)]"#);
    let value = lang::load(&p).unwrap().0;
    assert!(
        value
            .array()
            .unwrap()
            .iter()
            .all(|v| v.number().unwrap() == 74.)
    );
    for invalid in ["C###5", "Ebbb5", "C#b5", "Cb#5", "Cbb-1", "G##9"] {
        let (_d, p) = source(&format!("phrase(\"{invalid}:q\")"));
        assert!(lang::load(&p).is_err(), "accepted {invalid}");
    }
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

#[test]
fn list_sorting_is_stable_and_respects_units() {
    let (_d, p) = source(
        r#"
        let items = [{at:1bars,name:"a"},{at:1b,name:"early"},{at:4b,name:"b"}];
        map(sort_by(items,fn(item)=>item.at),fn(item)=>item.name)
    "#,
    );
    assert_eq!(
        lang::load(&p).unwrap().0.json(),
        serde_json::json!(["early", "a", "b"])
    );
    let (_d, p) = source(r#"sort_by(["z","a"],fn(s)=>s)"#);
    assert_eq!(
        lang::load(&p).unwrap().0.json(),
        serde_json::json!(["a", "z"])
    );
    for input in ["[1b,1s]", "[1,1b]", "[true,false]", "[1,\"a\"]"] {
        let (_d, p) = source(&format!("sort_by({input},fn(x)=>x)"));
        assert!(lang::load(&p).is_err(), "invalid sort keys: {input}");
    }
}

#[test]
fn chord_symbols_parse_completely_into_ascending_pitches() {
    use muz::music::chord;
    // Intervals from the root in octave 3; a slash bass sits below at a negative offset.
    let cases: &[(&str, &[i32])] = &[
        ("C", &[0, 4, 7]),
        ("Cm7b5", &[0, 3, 6, 10]),
        ("C13", &[0, 4, 7, 10, 14, 21]),
        ("C7sus4", &[0, 5, 7, 10]),
        ("C9sus4", &[0, 5, 7, 10, 14]),
        ("C7#9", &[0, 4, 7, 10, 15]),
        ("C7b9", &[0, 4, 7, 10, 13]),
        ("C7b13", &[0, 4, 7, 10, 20]),
        ("C7#11", &[0, 4, 7, 10, 18]),
        ("C13#11", &[0, 4, 7, 10, 14, 18, 21]),
        ("Cmaj7(#11)", &[0, 4, 7, 11, 18]),
        ("C7(b9,#11)", &[0, 4, 7, 10, 13, 18]),
        ("C69", &[0, 4, 7, 9, 14]),
        ("C6/9", &[0, 4, 7, 9, 14]),
        ("Cm6/9", &[0, 3, 7, 9, 14]),
        ("Cadd9", &[0, 4, 7, 14]),
        ("CmM7", &[0, 3, 7, 11]),
        ("C°7", &[0, 3, 6, 9]),
        ("Cø", &[0, 3, 6, 10]),
        ("C/E", &[-8, 0, 4, 7]),
        ("C6/9/E", &[-8, 0, 4, 7, 9, 14]),
        ("Am7/G", &[-2, 0, 3, 7, 10]),
    ];
    for (symbol, intervals) in cases {
        let root = muz::music::pitch(&format!("{}3", &symbol[..1])).unwrap();
        let expected: Vec<f64> = intervals.iter().map(|i| root + f64::from(*i)).collect();
        let pitches = chord(symbol, 3).unwrap();
        assert_eq!(pitches, expected, "{symbol}");
        assert!(
            pitches.windows(2).all(|w| w[0] < w[1]),
            "{symbol} is not ascending"
        );
    }
    assert_eq!(chord("B7sus4", 3).unwrap(), vec![59., 64., 66., 69.]);
    assert_ne!(chord("B7sus4", 3).unwrap(), chord("B7", 3).unwrap());
    assert_eq!(chord("E7#9", 3).unwrap(), vec![52., 56., 59., 62., 67.]);
    for (symbol, unread) in [
        ("C7alt", "'alt'"),
        ("Cmaj7#12", "'#12'"),
        ("Cmsus4", "'sus4'"),
        ("C7/G7", "'7'"),
        ("H7", "'H7'"),
        ("C7(b9", "unclosed '('"),
    ] {
        let error = format!("{:#}", chord(symbol, 3).unwrap_err());
        assert!(
            error.contains(symbol) && error.contains(unread),
            "{symbol}: {error}"
        );
    }
    let (_d, p) = source(r#"chords("C B7sus4 Em7alt")"#);
    let error = format!("{:#}", lang::load(&p).unwrap_err());
    assert!(error.contains("cannot read 'alt'"), "{error}");
}
