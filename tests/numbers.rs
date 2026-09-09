use muz::lang::{Number, Unit, Value};
fn eval(source: &str) -> anyhow::Result<Value> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("numbers.muz");
    std::fs::write(&path, source)?;
    Ok(muz::lang::load(&path)?.0)
}
#[test]
fn finite_control_arithmetic_composes() {
    let value = eval("0.5-0.5*cos(6.28318*33/64)").unwrap();
    assert!(
        (value.number().unwrap() - (0.5 - 0.5 * (6.28318_f64 * 33. / 64.).cos())).abs() < 1e-15
    );
    let envelope = eval("map(range(129),fn(i)=>[i*1b/64,0.5-0.5*cos(6.28318*i/64)])").unwrap();
    assert_eq!(envelope.array().unwrap().len(), 129);
    assert!(
        matches!(eval("10000000000*10000000000").unwrap(),Value::Num(q) if matches!(q.value,Number::Inexact(_)))
    );
    for source in [
        "pow(10,10000)",
        "pow(-1,0.5)",
        "1/0",
        "cos(1)/0",
        "pow(10,308)*10",
    ] {
        assert!(eval(source).is_err(), "{source}");
    }
}
#[test]
fn musical_rationals_stay_exact() {
    let value = eval("1b/3+1b/3+1b/3").unwrap();
    assert_eq!(value.beats().unwrap(), muz::music::b(1));
    assert!(matches!(value,Value::Num(q) if matches!(q.value,Number::Exact(_))));
    assert!(eval("10000000000b*10000000000").is_err());
    assert_eq!(eval("(1/3)*3b").unwrap().beats().unwrap(), muz::music::b(1));
    assert_eq!(
        eval("cos(0)*1b").unwrap().beats().unwrap(),
        muz::music::b(1)
    );
}

#[test]
fn sequencing_inexact_raw_offsets_preserves_onsets_or_reports_overflow() {
    let source = "seq([rest(32b), note_on(57, 0.5, at = 0.025b + 2 * (0.013b + 0.001b * sin(14)), channel = 2)])";
    match eval(source) {
        Ok(value) => {
            let onset = muz::music::real(value.pattern().unwrap().raw[0].at);
            let expected = 32. + 0.025 + 2. * (0.013 + 0.001 * 14f64.sin());
            assert!(
                (onset - expected).abs() < 1e-12,
                "onset {onset}, expected {expected}"
            );
        }
        Err(error) => assert!(error.to_string().contains("overflow"), "{error:#}"),
    }
}

#[test]
fn score_transforms_report_overflow_instead_of_wrapping() {
    for source in [
        "seq([rest(9223372036854775807b), rest(1b)])",
        "rest(9223372036854775807b).repeat(2)",
        "rest(4611686018427387904b).fit(9223372036854775807b)",
        "rest(4611686018427387904b).stretch(2)",
        "note(60,1b,at=9223372036854775807b)",
        "note(60).map_notes(fn(n)=>{at:9223372036854775807b}).at(1b)",
        "control(64,0).map_controls(fn(c)=>{at:9223372036854775807b}).at(1b)",
        "cc(64,0).map_raw(fn(r)=>{at:9223372036854775807b}).at(1b)",
        "rest(1b).slice(-9223372036854775807b,1b)",
        "note(60).map_notes(fn(n)=>{at:-9223372036854775807b}).reverse()",
        "phrase(\"C4:9223372036854775807 D4:q\")",
        "phrase(\"C4:9223372036854775807.\")",
        "chords(\"C G\",each=4611686018427387904b)",
        "drums({kick:\"x.x\"},span=1b/9223372036854775807)",
    ] {
        let error = eval(source).unwrap_err().to_string();
        assert!(error.contains("overflow"), "{source}: {error}");
        assert!(
            error.contains("numbers.muz:1:"),
            "missing source location: {error}"
        );
        assert!(error.contains('^'), "missing source span: {error}");
    }
    // Ordinary exact and representable inexact placements still work for every event kind.
    for offset in ["1b/3", "cos(0)*1b/3"] {
        let value = eval(&format!(
            "stack([note(60),control(64,0),cc(11,42)]).at({offset}).repeat(3)"
        ))
        .unwrap();
        let pattern = value.pattern().unwrap();
        for i in 0..3 {
            let expected = muz::music::decimal(&format!("{}/3", 1 + i * 4)).unwrap();
            assert_eq!(pattern.notes[i].at, expected);
            assert_eq!(pattern.controls[i].at, expected);
            assert_eq!(pattern.raw[i].at, expected);
        }
    }
}
#[test]
fn numeric_helpers_preserve_dimensions_and_exact_values() {
    for (source, unit, expected) in [
        ("max(1b,2b)+1b", Unit::Beat, 3.),
        ("abs(-10ms)+1ms", Unit::Seconds, 0.011),
        ("floor(1.8b)+round(1.2b)", Unit::Beat, 2.),
        ("min(1bar,5b)", Unit::Beat, 4.),
        ("max(1b/3,1b/4)*3", Unit::Beat, 1.),
        (
            "floor(-9223372036854775807b/2)",
            Unit::Beat,
            -4611686018427387904.,
        ),
    ] {
        let Value::Num(q) = eval(source).unwrap() else {
            panic!("{source}")
        };
        assert_eq!(q.unit, unit);
        assert!((q.number() - expected).abs() < 1e-15);
        assert!(matches!(q.value, Number::Exact(_)));
    }
    for source in [
        "min(1b,1s)",
        "max(1,1b)",
        "sin(1b)",
        "cos(1Hz)",
        "pow(2b,2)",
        "pow(2,2b)",
        "1b*2b",
        "1b%2s",
    ] {
        assert!(eval(source).is_err(), "{source}");
    }
    let sorted = eval("sort_by([cos(0),0,2],fn(x)=>x)").unwrap();
    assert_eq!(
        sorted
            .array()
            .unwrap()
            .iter()
            .map(|v| v.number().unwrap())
            .collect::<Vec<_>>(),
        vec![0., 1., 2.]
    );
}

#[test]
fn ranges_keep_exact_indices_and_unit_steps() {
    let values = eval("range(1b,start=0b,step=1b/3)").unwrap();
    let values = values.array().unwrap();
    assert_eq!(values.len(), 3);
    assert_eq!(
        values[2].beats().unwrap(),
        muz::music::decimal("2/3").unwrap()
    );
    assert!(
        values
            .iter()
            .all(|v| matches!(v,Value::Num(q) if matches!(q.value,Number::Exact(_))))
    );
    let value = eval("map(range(4),fn(i)=>i*1b/3)").unwrap();
    assert_eq!(value.array().unwrap()[3].beats().unwrap(), muz::music::b(1));
    assert!(eval("range(1b,start=0s,step=1b/3)").is_err());
    assert!(
        eval("map([1],fn(x)=>unknown_nested_function(x))")
            .unwrap_err()
            .to_string()
            .contains("unknown_nested_function")
    );
}

#[test]
fn range_normalizes_bars_without_changing_its_default_stride() {
    let mut e = muz::lang::Evaluator::new();
    for (source, count, last) in [("range(2bars)", 2, 4), ("range(2bars,step=1b)", 8, 7)] {
        let module = e.source(source).unwrap();
        let values = module.get("__result").unwrap().array().unwrap();
        assert_eq!(values.len(), count);
        assert_eq!(values.last().unwrap().beats().unwrap(), muz::music::b(last));
    }
}

#[test]
fn performed_seconds_keep_floating_precision_through_tempo_maps() {
    let value = eval(
        r#"
        use "std/mix" as mix;
        let timing = {tempo:73,tempos:[[3b,89],[7b,97],[11b,113],[17b,79],[23b,67]]};
        let n = note("C4",1b/3,at=111b).gate(0.83)
            .refine("all",{offset_ms:17.123456789,release_offset_ms:31.987654321}).notes[0];
        [seconds_at(n.at,timing), n.offset, mix.note_end(n,timing)+120ms, seconds_at(1s,timing)]
    "#,
    )
    .unwrap();
    let values = value.array().unwrap();
    for value in &values[..3] {
        assert!(
            matches!(value, Value::Num(q) if q.unit == Unit::Seconds && matches!(q.value,Number::Inexact(_)))
        );
    }
    assert!(matches!(&values[3], Value::Num(q) if matches!(q.value,Number::Exact(_))));
    let expected = 3. * 0.821918
        + 4. * 0.674157
        + 4. * 0.618557
        + 6. * 0.530973
        + 6. * 0.759494
        + (111. + 0.83 / 3. - 23.) * 0.895522
        + 0.017123456789
        + 0.031987654321
        + 0.12;
    assert!((values[2].number().unwrap() - expected).abs() < 1e-12);
}
