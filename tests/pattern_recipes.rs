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

#[test]
fn note_runs_snapshot_context_and_native_storage_order() {
    let value = eval(
        r#"
let p=overlay([
 note(64).at(2b,key="last"),
 note(60).at(0b,key="first"),
 note(62).at(1b,key="middle")
]);
map_note_runs(p,fn(c)=>{
 at:c.note.at+10b, voice:"edited", key:c.note.key+"/new",
 data:{index:c.index,count:c.count,run:c.run,
       previous:if c.previous == null {null} else {c.previous.pitch},
       next:if c.next == null {null} else {c.next.pitch}}
})
"#,
    )
    .unwrap();
    let p = value.pattern().unwrap();
    assert_eq!(
        p.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![64., 60., 62.]
    );
    assert_eq!(
        p.notes.iter().map(|n| n.at).collect::<Vec<_>>(),
        vec![muz::music::b(12), muz::music::b(10), muz::music::b(11)]
    );
    for (note, index, previous, next) in [
        (
            &p.notes[0],
            2.,
            serde_json::json!(62.),
            serde_json::Value::Null,
        ),
        (
            &p.notes[1],
            0.,
            serde_json::Value::Null,
            serde_json::json!(62.),
        ),
        (
            &p.notes[2],
            1.,
            serde_json::json!(60.),
            serde_json::json!(64.),
        ),
    ] {
        assert_eq!(note.data["index"].as_f64(), Some(index));
        assert_eq!(note.data["count"].as_f64(), Some(3.));
        assert_eq!(note.data["run"], "");
        assert_eq!(note.data["previous"].as_f64(), previous.as_f64());
        assert_eq!(note.data["next"].as_f64(), next.as_f64());
        assert!(note.key.ends_with("/new"));
        assert_eq!(note.voice, "edited");
    }
}

#[test]
fn note_runs_voices_stable_ties_and_custom_grouping() {
    let source = r#"
let p=stack([
 phrase("[E4 C4]:q D4:q").voice("a"),
 phrase("G4:q A4:q").voice("b")
]);
map_note_runs(p,fn(c)=>{data:{index:c.index,count:c.count,
 previous:if c.previous==null {null} else {c.previous.pitch},
 next:if c.next==null {null} else {c.next.pitch}}})
"#;
    let value = eval(source).unwrap();
    let p = value.pattern().unwrap();
    assert_eq!(p.notes[0].data["next"].as_f64(), Some(60.));
    assert_eq!(p.notes[1].data["previous"].as_f64(), Some(64.));
    assert_eq!(p.notes[2].data["next"], serde_json::Value::Null);
    assert_eq!(p.notes[3].data["previous"], serde_json::Value::Null);
    assert_eq!(p.notes[3].data["count"].as_f64(), Some(2.));
    for (run, expected) in [
        ("fn(n)=>[n.voice,n.at]", vec![2., 2., 1., 1., 1.]),
        ("fn(n)=>0", vec![5.; 5]),
    ] {
        let source = format!(
            r#"
let p=stack([phrase("[E4 C4]:q D4:q").voice("a"),phrase("G4:q A4:q").voice("b")]);
map_note_runs(p,fn(c)=>{{data:{{count:c.count}}}},run={run})
"#
        );
        let value = eval(&source).unwrap();
        assert_eq!(
            value
                .pattern()
                .unwrap()
                .notes
                .iter()
                .map(|n| n.data["count"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
    }
    let value = eval(
        r#"map_note_runs(phrase("[E4 C4]:q D4:q"),fn(c)=>{data:{count:c.count,index:c.index}})"#,
    )
    .unwrap();
    assert!(
        value
            .pattern()
            .unwrap()
            .notes
            .iter()
            .all(|n| n.data["count"].as_f64() == Some(3.))
    );
}

#[test]
fn note_runs_drop_uses_original_neighbors_and_preserves_other_material() {
    let source = r#"stack([phrase("C4:q D4:q E4:q r:q").annotate("all",{custom:7}),control(64,80,at=1b),cc(11,42)])"#;
    let original = eval(source).unwrap();
    let changed_source = format!("let p={source};")
        + r#"
map_note_runs(p,fn(c)=>if c.index==1 {null} else {
 {pitch:if c.previous==null {c.note.pitch} else {c.previous.pitch}}
})"#;
    let changed = eval(&changed_source).unwrap();
    let a = original.pattern().unwrap();
    let b = changed.pattern().unwrap();
    assert_eq!(b.notes.len(), 2);
    assert_eq!(b.notes[1].pitch, 62.);
    assert_eq!(a.notes[0], b.notes[0]);
    assert_eq!(b.notes[1].data["custom"].as_f64(), Some(7.));
    assert_eq!(a.controls, b.controls);
    assert_eq!(a.raw, b.raw);
    assert_eq!(a.span, b.span);
    let empty =
        eval(r#"map_note_runs(rest(4b),fn(c)=>assert(false),run=fn(n)=>assert(false))"#).unwrap();
    assert_eq!(empty.pattern().unwrap().span, muz::music::b(4));
    for source in [
        r#"map_note_runs(phrase("C4:q D4:q"),fn(c)=>{key:"duplicate"})"#,
        r#"map_note_runs(note(60),fn(c)=>{velocity:2})"#,
    ] {
        assert!(eval(source).is_err());
    }
}

#[test]
fn note_runs_many_singletons_and_clock_payloads() {
    let many = eval(
        r#"
let p=phrase("C4:q").repeat(1000);
map_note_runs(p,fn(c)=>{
 assert(c.previous==null && c.next==null && c.index==0 && c.count==1);
 {velocity:0.5}
},run=fn(n)=>n.key)
"#,
    )
    .unwrap();
    assert_eq!(many.pattern().unwrap().notes.len(), 1000);
    let dir = tempfile::tempdir().unwrap();
    let mut writer = hound::WavWriter::create(
        dir.path().join("clip.wav"),
        hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for _ in 0..800 {
        writer.write_sample(0.0f32).unwrap();
    }
    writer.finalize().unwrap();
    let path = dir.path().join("clip.muz");
    std::fs::write(
        &path,
        r#"
let original=clip("audio","clip.wav",{fade_in:0ms,fade_out:0ms}).pattern;
let main=[original,map_note_runs(original,fn(c)=>{velocity:0.5})];
"#,
    )
    .unwrap();
    let value = muz::lang::load(&path).unwrap().0;
    let values = value.array().unwrap();
    let mut expected = values[0].pattern().unwrap().clone();
    expected.notes[0].velocity = 0.5;
    assert_eq!(&expected, values[1].pattern().unwrap());
    std::fs::write(
        &path,
        r#"
let audio=clip("audio","clip.wav",{fade_in:0ms,fade_out:0ms}).pattern;
let main=map_note_runs(audio,fn(c)=>{duration:1b});
"#,
    )
    .unwrap();
    let error = muz::lang::load(&path).unwrap_err();
    assert!(format!("{error:#}").contains("clock clip duration"));
}
#[test]
fn controls_over_lets_a_layer_replace_equal_time_controls_only() {
    let d = tempfile::tempdir().unwrap();
    let defs = r#"let ramp = stack([control(1, 40, at=0b), control(1, 80, at=4b), control(2, 7, at=4b)]);
let step = control(1, 96, at=4b);"#;
    let build = |name: &str, combine: &str| {
        let p = d.path().join(name);
        let src = format!(
            "{defs}\nlet dyn = {combine};\nsong({{tracks:[track(\"a\", stack([note(60, 8b), dyn]), synth(\"bell\"))]}});"
        );
        std::fs::write(&p, src).unwrap();
        p
    };
    let p = build("over.muz", "controls_over(ramp, step)");
    muz::compile::compile(&p).unwrap();
    let e = eval(&format!("{defs}\ncontrols_over(ramp, step)")).unwrap();
    let c = &e.pattern().unwrap().controls;
    let cc1: Vec<_> = c
        .iter()
        .filter(|c| c.cc == 1 && c.at == muz::music::b(4))
        .collect();
    assert_eq!(cc1.len(), 1);
    assert_eq!(cc1[0].value, 96);
    assert_eq!(c.len(), 3);
    let p = build("plain.muz", "stack([ramp, step])");
    let err = format!("{:#}", muz::compile::compile(&p).unwrap_err());
    assert!(err.contains("conflicting CC"), "{err}");
}

fn offsets(source: &str) -> Vec<(f64, f64)> {
    let v = eval(&format!("use \"std/performance\" as perf;\n{source}")).unwrap();
    v.pattern().unwrap().notes.iter().map(|n| (n.pitch, n.offset_ms)).collect()
}
#[test]
fn spread_offsets_chord_notes_by_pitch_rank_in_both_directions() {
    let up = offsets(r#"phrase("[G4 C4 E4]:q").spread(10ms)"#);
    assert_eq!(up, vec![(67., 20.), (60., 0.), (64., 10.)]);
    let down = offsets(r#"perf.spread(phrase("[G4 C4 E4]:q"), 10ms, "down")"#);
    assert_eq!(down, vec![(67., 0.), (60., 20.), (64., 10.)]);
    let added = offsets(r#"phrase("[C4 E4]:q").map_notes(fn(n)=>{offset_ms:3}).spread(10ms)"#);
    assert!(added[0] == (60., 3.) && (added[1].1 - 13.).abs() < 1e-9);
}
#[test]
fn spread_rolls_hands_independently_and_leaves_other_notes_alone() {
    let hands = offsets(
        r#"stack([phrase("[C2 G2]:q").hand("left"), phrase("[C5 E5]:q").hand("right")]).spread(10ms)"#,
    );
    assert_eq!(hands, vec![(36., 0.), (43., 10.), (72., 0.), (76., 10.)]);
    let mixed = offsets(r#"phrase("C4:q [C4 E4]:q").spread(10ms)"#);
    assert_eq!(mixed, vec![(60., 0.), (60., 0.), (64., 10.)]);
    let selected = offsets(
        r#"stack([phrase("[C4 E4]:q").tag("all","a"), phrase("[C3 E3]:q").tag("all","b")]).spread(10ms, "up", "a")"#,
    );
    assert_eq!(selected, vec![(60., 0.), (64., 10.), (48., 0.), (52., 0.)]);
}
#[test]
fn voice_top_raises_only_the_highest_note_with_clamping() {
    let v = eval(r#"phrase("[C4 G4 E4]:q").velocity(0.95).voice_top(0.2)"#).unwrap();
    let vel: Vec<f64> = v.pattern().unwrap().notes.iter().map(|n| n.velocity).collect();
    assert_eq!(vel, vec![0.95, 1.0, 0.95]);
}
