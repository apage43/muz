use muz::{
    lang,
    music::{self, Pattern},
};
fn evaluate(source: &str) -> anyhow::Result<lang::Value> {
    let d = tempfile::tempdir()?;
    let p = d.path().join("test.muz");
    std::fs::write(&p, source)?;
    lang::load(&p).map(|v| v.0)
}
fn pattern(source: &str) -> Pattern {
    evaluate(source).unwrap().pattern().unwrap().clone()
}
#[test]
fn sparse_transforms_preserve_every_untouched_field_and_event() {
    let original = pattern(
        r#"let main = stack([phrase("C4:q@one E4:q").hand("left").voice("lead").annotate("all",{custom:[1,2]}),control(64,80,at=1b,offset=12ms),cc(11,42)]);"#,
    );
    let changed = pattern(
        r#"let main = stack([phrase("C4:q@one E4:q").hand("left").voice("lead").annotate("all",{custom:[1,2]}),control(64,80,at=1b,offset=12ms),cc(11,42)]).map_notes(fn(n)=>{duration:n.duration*2,release_offset:7ms});"#,
    );
    assert_eq!(original.controls, changed.controls);
    assert_eq!(original.raw, changed.raw);
    assert_eq!(original.span, changed.span);
    for (mut expected, actual) in original.notes.into_iter().zip(changed.notes) {
        expected.dur *= music::b(2);
        expected.release_offset_ms = 7.;
        assert_eq!(expected, actual);
    }
}
#[test]
fn callbacks_filter_expand_refine_and_validate() {
    let p = pattern(
        r#"let main=phrase("C4:q D4:q E4:q").filter_notes(fn(n)=>n.pitch != 62).flat_map_notes(fn(n)=>[{}, {pitch:n.pitch+12}]).refine("all",fn(n)=>{duration:1/2b, release_offset:3ms});"#,
    );
    assert_eq!(p.notes.len(), 4);
    assert!(
        p.notes
            .iter()
            .all(|n| n.dur == music::decimal("1/2").unwrap())
    );
    assert_ne!(p.notes[0].key, p.notes[1].key);
    for src in [
        r#"let main=note(60).map_notes(fn(n)=>{velocity:2});"#,
        r#"let main=control(64,1).map_controls(fn(c)=>{value:128});"#,
        r#"let main=note(60).map_notes(fn(n)=>{duration:0b});"#,
    ] {
        assert!(evaluate(src).is_err(), "{src}");
    }
}
#[test]
fn coherent_displacement_covers_release_cc_and_raw() {
    let p = pattern(
        r#"let main=stack([note(60,2b).gate(0.5),control(64,0,at=1b),cc(11,12,at=1b)]).displace(fn(at)=>at/1b*10ms);"#,
    );
    assert_eq!(p.notes[0].offset_ms, 0.);
    assert!((p.notes[0].release_offset_ms - 10.).abs() < 1e-8);
    assert!((p.controls[0].offset_ms - 10.).abs() < 1e-8);
    assert!((p.raw[0].offset_ms - 10.).abs() < 1e-8);
}
#[test]
fn source_policy_recipes_and_keyed_variation() {
    let p = pattern(
        r#"let main=phrase("C4:q D4:q").scale_gate(0.5).dynamics(0.3,0.8).humanize(2ms).rubato(4ms);"#,
    );
    assert_eq!(p.notes[0].gate, 0.45);
    let p = pattern(
        r#"let main=note(38,1b).voice("snare").groove({snare:4ms}).drum_feel().flam().roll(step=1/4b);"#,
    );
    assert_eq!(p.notes.len(), 8);
    let p = pattern(r#"let main=pedal(phrase("[C3 E3 G3]:q [D3 F3 A3]:q"));"#);
    assert_eq!(p.controls.len(), 5);
    let a = evaluate(r#"let main=keyed_noise(["voice",1b],4);"#)
        .unwrap()
        .number()
        .unwrap();
    let b = evaluate(r#"let main=keyed_noise(["voice",1b],4);"#)
        .unwrap()
        .number()
        .unwrap();
    assert_eq!(a, b);
    assert!((-1.0..=1.0).contains(&a));
}
#[test]
fn local_pickups_and_identity_preserving_overlay() {
    let p = pattern(
        r#"let main=overlay([note(60).at(-1b,key="pickup").map_notes(fn(n)=>{velocity:0.5}).at(2b,key="a"),note(64).at(2b,key="b")]);"#,
    );
    assert_eq!(p.notes[0].at, music::b(1));
    assert!(p.notes[0].key.starts_with("a/pickup/"));
    assert!(evaluate(r#"let main=overlay([note(60),note(64)]);"#).is_err());
}
#[test]
fn control_and_raw_expansion_filtering_preserve_other_events() {
    let p = pattern(
        r#"let main=stack([note(60),control(64,1),cc(11,12)])
        .flat_map_controls(fn(c)=>[{}, {at:1b,value:0}])
        .flat_map_raw(fn(r)=>[{}, {at:1b,bytes:[176,11,0]}])
        .map_controls(fn(c)=>if c.value == 1 {null} else {{}})
        .map_raw(fn(r)=>if r.at == 0b {null} else {{}});"#,
    );
    assert_eq!(p.notes.len(), 1);
    assert_eq!(p.controls.len(), 1);
    assert_eq!(p.controls[0].at, music::b(1));
    assert_eq!(p.raw.len(), 1);
    assert_eq!(p.raw[0].bytes, [176, 11, 0]);
}
#[test]
fn keyed_humanization_survives_unrelated_occurrence_insertion() {
    let a = pattern(r#"let main=overlay([note(60).at(1b,key="a")]).humanize(seed=8);"#);
    let b = pattern(
        r#"let main=overlay([note(70).at(0b,key="intro"),note(60).at(1b,key="a")]).humanize(seed=8);"#,
    );
    assert_eq!(a.notes[0], b.notes[1]);
    let fixed = pattern(
        r#"let main=note(60).tag("all","fixed").humanize(50ms).groove({"":40ms}).drum_feel(20ms);"#,
    );
    assert_eq!(fixed.notes[0].offset_ms, 0.);
}

#[test]
fn grouping_preserves_dimensions_and_note_transforms_reject_duplicate_keys() {
    let mut e = muz::lang::Evaluator::new();
    let v = e.source("group_by([1b,1s,4b,1bars],fn(v)=>v)").unwrap();
    let groups = v.get("__result").unwrap().array().unwrap();
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[2].array().unwrap().len(), 2);
    let distinct = e
        .source("group_by([1/3b,0.3333333333333333b],fn(v)=>v)")
        .unwrap();
    assert_eq!(distinct.get("__result").unwrap().array().unwrap().len(), 2);
    let error = e
        .source("phrase(\"C4:q D4:q\").map_notes(fn(n)=>{key:\"same\"})")
        .unwrap_err();
    assert!(format!("{error:#}").contains("duplicate identity"));
}
