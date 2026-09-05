fn eval(source: &str) -> anyhow::Result<muz::lang::Value> {
    let d = tempfile::tempdir()?;
    let p = d.path().join("patterns.muz");
    std::fs::write(&p, source)?;
    Ok(muz::lang::load(&p)?.0)
}
#[test]
fn swing_preserves_metadata_events_and_exact_offbeat_time() {
    let value=eval(r#"let p=stack([phrase("C4:e D4:e E4:e F4:e").tag("all","accent"),control(64,100,at=1/2b)]);p.swing(0.6)"#).unwrap();
    let p = value.pattern().unwrap();
    assert_eq!(
        p.notes.iter().map(|n| n.at).collect::<Vec<_>>(),
        vec![
            muz::music::b(0),
            muz::music::decimal("0.6").unwrap(),
            muz::music::b(1),
            muz::music::decimal("1.6").unwrap()
        ]
    );
    assert!(
        p.notes
            .iter()
            .all(|n| n.tags.contains("accent") && n.dur == muz::music::decimal("0.5").unwrap())
    );
    assert_eq!(p.controls[0].at, muz::music::decimal("0.5").unwrap());
    assert!(eval("phrase(\"C4:e\").swing(grid=0b)").is_err());
}
#[test]
fn arpeggio_uses_sorted_chords_integer_octaves_and_retains_data() {
    let value=eval(r#"let p=stack([phrase("[G4 C4 E4]:q [D4 F4]:q").tag("all","harmony").annotate("all",{custom:7}),control(64,127),control(64,0,at=2b)]); arpeggiate(p,[-1,0,3],1/3b)"#).unwrap();
    let p = value.pattern().unwrap();
    assert_eq!(
        p.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![55., 60., 72., 53., 62., 77.]
    );
    assert_eq!(p.span, muz::music::b(2));
    assert_eq!(p.controls.len(), 2);
    assert!(
        p.notes
            .iter()
            .all(|n| n.dur == muz::music::decimal("1/3").unwrap()
                && n.tags.contains("harmony")
                && n.data["custom"].as_f64() == Some(7.))
    );
    let keys = p
        .notes
        .iter()
        .map(|n| &n.key)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(keys.len(), p.notes.len());
    for source in [
        "arpeggiate(phrase(\"C4:q\"),[],1b)",
        "arpeggiate(phrase(\"C4:q\"),[0.5])",
        "arpeggiate(phrase(\"C4:q\"),[0],0b)",
        "arpeggiate(phrase(\"C4:q\"),[0],1b/200001)",
    ] {
        assert!(eval(source).is_err(), "{source}");
    }
}
#[test]
fn arpeggio_clips_last_hit_and_keeps_empty_pattern_span() {
    let v = eval("arpeggiate(phrase(\"C4:q\"),[0],2/3b)").unwrap();
    let p = v.pattern().unwrap();
    assert_eq!(p.notes.len(), 2);
    assert_eq!(p.notes[1].dur, muz::music::decimal("1/3").unwrap());
    let v = eval("arpeggiate(rest(3b))").unwrap();
    assert_eq!(v.pattern().unwrap().span, muz::music::b(3));
}
