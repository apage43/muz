use muz::{
    compile::{Compiled, PPQ},
    model::TrackSource,
};
fn compile(source: &str) -> anyhow::Result<Compiled> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("arrangement.muz");
    std::fs::write(&path, source)?;
    let compiled = muz::compile::compile(&path)?;
    // Exercise real automation targets as well as source-level timing.
    let _prepared = muz::audio::AudioEngine::new(
        &compiled.session,
        muz::audio::AudioConfig {
            sample_rate: 48000.,
            max_frames: 256,
            offline: false,
        },
    )?;
    Ok(compiled)
}
const SETTINGS: &str =
    r#"{tempo:120,meter:[3,4],tracks:[track("p",rest(0b),synth("bell"),{pan:0})],tail:0}"#;
fn arranged(prefix: f64, tempo: u32, nested: bool) -> Compiled {
    let grouping = if nested {
        "a.group(a.sequence([a.occurrence(\"inner\", bridge)]))"
    } else {
        "bridge"
    };
    compile(&format!(r#"
use "std/arrange" as a;
let bridge = a.passage(3b,[a.part("p",phrase("C4:q D4:q"))],
 [a.local("p.out",[[0b,-6],[1b,0]]),a.clock("p.pan.pan",[[0ms,0],[20ms,1]])],[[0b,{tempo}]]);
let form = a.sequence([a.occurrence("intro",a.passage({prefix}b)),a.occurrence("bridge",{grouping})]);
a.build(form,{SETTINGS})
"#)).unwrap()
}
#[test]
fn meter_measures_are_explicit_and_bare_bars_have_one_meaning() {
    let c=compile(&format!(r#"use "std/arrange" as a;
let form=a.sequence([a.occurrence("measure",a.passage(a.bars(1,[3,4]))),a.occurrence("literal",a.passage(1bar))]);
a.build(form,{SETTINGS})"#)).unwrap();
    assert_eq!(c.session.extras.sections[0].end, 3.);
    assert_eq!(c.session.extras.sections[1].start, 3.);
    assert_eq!(c.session.extras.sections[1].end, 7.);
}

#[test]
fn measures_alias_checks_meters_and_preserves_bars_compatibility() {
    use muz::lang::{Evaluator, Value};
    let v=Evaluator::new().source(r#"use "std/arrange" as a; a.measures(1)==4b && a.measures(1,[3,4])==3b && a.measures(1,[6,8])==3b && a.measures(1,[7,8])==3.5b && a.measures(2,[7,8])==a.bars(2,[7,8])"#).unwrap();
    assert!(matches!(v.get("__result"), Some(Value::Bool(true))));
    for meter in ["[0,4]", "[3,3]", "[3.5,4]", "[3,4,5]"] {
        assert!(
            Evaluator::new()
                .source(&format!("use \"std/arrange\" as a; a.measures(1,{meter})"))
                .is_err()
        );
    }
}
#[test]
fn passage_motion_keeps_notes_tempo_and_clock_gestures_coherent() {
    let original = arranged(3., 60, false);
    let moved = arranged(6., 90, false);
    for (c, start, tempo) in [(&original, 3., 60.), (&moved, 6., 90.)] {
        assert_eq!(muz::music::real(c.score[0].pattern.notes[0].at), start);
        assert_eq!(muz::music::real(c.score[0].pattern.notes[1].at), start + 1.);
        let TrackSource::Midi(midi) = &c.session.tracks[0].source else {
            panic!()
        };
        assert!(
            midi.imported
                .tempos
                .iter()
                .any(|p| p.tick == (start * PPQ as f64) as u64
                    && (60_000_000. / p.micros_per_quarter as f64 - tempo).abs() < 0.001)
        );
        let curves = &c.session.extras.automation;
        let local = curves.iter().find(|a| a.target == "p.out").unwrap();
        let clock = curves.iter().find(|a| a.target == "p.pan.pan").unwrap();
        assert!((local.points[0].seconds - start * 0.5).abs() < 1e-6);
        assert!((local.points[1].seconds - local.points[0].seconds - 60. / tempo).abs() < 1e-6);
        assert!((clock.points[0].seconds - local.points[0].seconds).abs() < 1e-6);
        assert!((clock.points[1].seconds - clock.points[0].seconds - 0.020).abs() < 1e-9);
    }
    assert_eq!(
        original.score[0]
            .pattern
            .notes
            .iter()
            .map(|n| &n.key)
            .collect::<Vec<_>>(),
        moved.score[0]
            .pattern
            .notes
            .iter()
            .map(|n| &n.key)
            .collect::<Vec<_>>()
    );
}
#[test]
fn nested_passage_motion_preserves_identity_and_gesture_context() {
    let first = arranged(3., 60, true);
    let moved = arranged(6., 60, true);
    assert_eq!(
        first.score[0].pattern.notes[0].key,
        moved.score[0].pattern.notes[0].key
    );
    assert!(
        first.score[0].pattern.notes[0]
            .key
            .starts_with("bridge/inner/")
    );
    assert!((moved.session.extras.automation[0].points[0].seconds - 3.).abs() < 1e-9);
    assert!((moved.session.extras.automation[1].points[1].seconds - 3.020).abs() < 1e-9);
}
#[test]
fn pinned_exceptions_edit_one_occurrence_and_fail_when_selection_disappears() {
    let source = format!(
        r#"use "std/arrange" as a;
let shared=a.passage(3b,[a.part("p",phrase("C4:q@accent D4:q"))]);
let changed=a.edit(shared,"p",fn(p)=>a.require(p,"accent").refine("accent",{{velocity:0.2}}));
a.build(a.sequence([a.occurrence("first",shared),a.occurrence("second",changed)]),{SETTINGS})"#
    );
    let c = compile(&source).unwrap();
    let notes = &c.score[0].pattern.notes;
    assert_eq!(notes.len(), 4);
    assert_ne!(notes[0].velocity, notes[2].velocity);
    assert_eq!(notes[1].velocity, notes[3].velocity);
    assert!((notes[2].velocity - 0.2).abs() < 1e-9);
    let error = compile(&source.replace("q@accent", "q")).unwrap_err();
    assert!(format!("{error:#}").contains("pinned selection"));
}
#[test]
fn repeated_gestures_join_once_and_reject_ambiguous_overlap() {
    let source = format!(
        r#"use "std/arrange" as a;
let shared=a.passage(3b,[a.part("p",phrase("C4:q"))],[a.local("p.out",[[0b,-6],[1b,0],[3b,-6]])]);
a.build(a.sequence([a.occurrence("first",shared),a.occurrence("second",shared)]),{SETTINGS})"#
    );
    let c = compile(&source).unwrap();
    assert_eq!(c.session.extras.automation.len(), 1);
    let points = &c.session.extras.automation[0].points;
    assert_eq!(
        points.iter().map(|p| p.seconds).collect::<Vec<_>>(),
        vec![0., 0.5, 1.5, 2., 3.]
    );
    let error = compile(&source.replace("[3b,-6]", "[4b,-6]")).unwrap_err();
    assert!(format!("{error:#}").contains("overlapping automation"));
}
#[test]
fn negative_pickup_is_valid_after_placement_but_not_before_score_zero() {
    let source = format!(
        r#"use "std/arrange" as a;
let pickup=a.passage(3b,[a.part("p",note("C4",1b,at=-1b))]);
a.build(a.sequence([a.occurrence("lead",a.passage(3b)),a.occurrence("pickup",pickup)]),{SETTINGS})"#
    );
    let c = compile(&source).unwrap();
    assert_eq!(c.score[0].pattern.notes[0].at, muz::music::b(2));
    assert!(compile(&source.replace("a.occurrence(\"lead\",a.passage(3b)),", "")).is_err());
}
#[test]
fn nested_note_derived_gestures_see_the_final_placed_material() {
    let c = compile(&format!(
        r#"use "std/arrange" as a;
let notes=phrase("C4:q");
let child=a.passage(3b,[a.part("p",notes)],[fn(c)=>{{
 let n=a.material(c,"p").notes[0];
 assert(n.key == "outer/inner/" + notes.notes[0].key, "context identity differs from placed score");
 [automation("p.out",curve([[n.at,-6],[n.at+n.duration,0]]))]
}}]);
let nested=a.group(a.sequence([a.occurrence("inner",child)]));
a.build(a.sequence([a.occurrence("lead",a.passage(3b)),a.occurrence("outer",nested)]),{SETTINGS})"#
    ));
    let c = c.unwrap();
    assert_eq!(c.session.extras.automation[0].points[0].seconds, 1.5);
    assert_eq!(c.score[0].pattern.notes[0].at, muz::music::b(3));
}

#[test]
fn grouped_edits_keep_nested_gestures_in_sync_and_siblings_isolated() {
    let c = compile(r#"
use "std/arrange" as a;
use "std/mix" as mix;
let child=a.passage(4b,[a.part("p",note(60,1b).tag("all","keep").at(0b,key="keep")),
    a.part("p",note(64,1b,at=1b).tag("all","remove").at(0b,key="remove"))],[fn(c)=>{
    let p=a.material(c,"p");
    let kept=p.select("keep").notes;
    assert(len(kept)==1,"child context leaked sibling layers");
    let n=kept[0];
    assert(n.key==c.name+"/keep/note","child context lost final score identity");
    if n.velocity == 0.25 {
        assert(len(p.notes)==1,"deleted note survived in child context");
        assert(n.at==c.start+1b && n.duration==2b && n.gate==0.5,
            "child context ignored grouped note edits");
    } else {
        assert(len(p.notes)==2 && n.at==c.start,"shared original changed");
    };
    [automation("p.out",curve([[mix.note_start(n,c.timing),-6],
        [mix.note_end(n,c.timing),-6]]))]
}],[[0b,60]]);
let pair=a.group(a.sequence([a.occurrence("first",child),a.occurrence("second",child)]));
let nested=a.group(a.sequence([a.occurrence("padding",a.passage(2b)),a.occurrence("pair",pair)]));
let edited=a.edit(nested,"p",fn(p)=>p.filter_notes(fn(n)=>!contains(n.tags,"remove"))
    .map_notes(fn(n)=>{at:n.at+1b,duration:2b,gate:0.5,velocity:0.25,offset:10ms,release_offset:30ms}));
a.build(a.sequence([a.occurrence("intro",a.passage(3b)),
    a.occurrence("original",nested),a.occurrence("changed",edited)]),
    {tempo:120,tempos:[[16b,120]],tracks:[track("p",rest(0b),synth("bell"))],tail:0})
"#).unwrap();
    let notes = &c.score[0].pattern.notes;
    assert_eq!(notes.len(), 6);
    assert_eq!(
        notes
            .iter()
            .map(|n| muz::music::real(n.at))
            .collect::<Vec<_>>(),
        vec![5., 6., 9., 10., 16., 20.]
    );
    let points = &c.session.extras.automation[0].points;
    let expected = [2.5, 3.4, 6.5, 7.4, 13.51, 14.04, 16.01, 17.04];
    assert_eq!(points.len(), expected.len());
    for (point, expected) in points.iter().zip(expected) {
        assert!(
            (point.seconds - expected).abs() < 1e-8,
            "{} != {expected}",
            point.seconds
        );
    }
}

#[test]
fn grouped_gestures_handle_empty_children_and_replaced_patterns() {
    let source = r#"
use "std/arrange" as a;
let child=a.passage(2b,[a.part("p",note(60))],[fn(c)=>{
    let p=a.material(c,"p");
    assert(len(p.notes)==0,"deleted child still has notes");
    []
}]);
let empty=a.passage(1b,[],[fn(c)=>{
    assert(len(c.parts)==0,"empty child borrowed sibling material");
    []
}]);
let grouped=a.group(a.sequence([a.occurrence("empty",empty),a.occurrence("child",child)]));
let edited=a.edit(grouped,"p",fn(p)=>rest(2b));
a.build(a.sequence([a.occurrence("outer",edited)]),
    {tracks:[track("p",rest(0b),synth("bell"))],tail:0})
"#;
    let c = compile(source).unwrap();
    assert!(c.score[0].pattern.notes.is_empty());
    assert!(c.session.extras.automation.is_empty());
    let pinned = source.replace(
        "let p=a.material(c,\"p\");",
        "let p=a.require(a.material(c,\"p\"),\"all\");",
    );
    assert!(format!("{:#}", compile(&pinned).unwrap_err()).contains("pinned selection"));
    let replaced = source.replace("rest(2b)", "note(67,1b,at=1b)").replace(
        "len(p.notes)==0",
        "len(p.notes)==1 && p.notes[0].pitch==67 && p.notes[0].at==1b",
    );
    let c = compile(&replaced).unwrap();
    assert_eq!(c.score[0].pattern.notes[0].pitch, 67.);
}

#[test]
fn clock_clip_placement_keeps_local_seconds_and_duration() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("clip.wav");
    let mut writer = hound::WavWriter::create(
        &wav,
        hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for _ in 0..800 {
        writer.write_sample(0.1f32).unwrap();
    }
    writer.finalize().unwrap();
    let source = dir.path().join("song.muz");
    std::fs::write(&source,r#"use "std/arrange" as a;
let audio=clip("audio","clip.wav",{at:100ms,duration:50ms,fade_out:10ms});
a.build(a.sequence([a.occurrence("before",a.passage(4b)),a.occurrence("clip",a.passage(1b,[a.part("audio",audio.pattern)]))]),{tempo:120,tracks:[audio]})"#).unwrap();
    let compiled = muz::compile::compile(&source).unwrap();
    let note = &compiled.score[0].pattern.notes[0];
    assert!((muz::music::real(note.at) - 4.2).abs() < 1e-9);
    assert!((muz::music::real(note.dur) - 0.08).abs() < 1e-9);
    let contracted = std::fs::read_to_string(&source).unwrap().replace(
        "a.passage(1b,[a.part(\"audio\",audio.pattern)])",
        "a.require_contained(a.passage(1b,[a.part(\"audio\",a.require_span(audio.pattern,1b))]))",
    );
    std::fs::write(&source, contracted).unwrap();
    let error = muz::compile::compile(&source).unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("unsupported clock timing") && message.contains("part audio"),
        "{message}"
    );
}

fn contract_source(material: &str) -> String {
    format!(
        r#"use "std/arrange" as a;
a.build(a.sequence([a.occurrence("lead",a.passage(4b)),a.occurrence("checked",{material})]),{SETTINGS})"#
    )
}
fn contract_result(material: &str) -> anyhow::Result<muz::lang::Value> {
    muz::lang::Evaluator::new().source(&contract_source(material))
}
fn contained(pattern: &str, options: &str) -> String {
    format!(r#"a.require_contained(a.passage(4b,[a.part("p",{pattern})]){options})"#)
}

#[test]
fn logical_span_contracts_count_rests_and_remain_opt_in() {
    let mut e = muz::lang::Evaluator::new();
    let v = e
        .source(
            r#"use "std/arrange" as a;
let p=seq([note(60,1b),rest(3b)]);
a.require_span(p,4b)==p && a.require_fit(p,4b)==p && a.require_span(rest(0b),0b).span==0b"#,
        )
        .unwrap();
    assert!(v.get("__result").unwrap().truth());
    for call in [
        "a.require_span(note(60,8b),4b)",
        "a.require_fit(note(60,8b),4b)",
        "a.require_span(rest(0b),-1b)",
        "a.require_fit(rest(0b),1s)",
        "a.require_span(rest(0b),0)",
    ] {
        assert!(
            e.source(&format!(r#"use "std/arrange" as a; {call}"#))
                .is_err(),
            "{call}"
        );
    }
    compile(&contract_source(
        r#"a.passage(4b,[a.part("p",note(60,8b))])"#,
    ))
    .unwrap();
    assert!(contract_result(&contained("note(60,8b)", "")).is_err());
    // A short gate fits in performance even though logical fit fails.
    assert!(contract_result(&contained("note(60,8b).gate(0.5)", "")).is_ok());
}

#[test]
fn containment_has_independent_pickup_tail_and_attack_boundaries() {
    for (pattern, options, succeeds, diagnostic) in [
        ("note(60,4b).gate(1)", "", true, ""),
        ("note(60,1b,at=4b)", ",tails=true", false, "note start"),
        ("note(60,1b,at=-1b)", "", false, "pickup"),
        ("note(60,1b,at=-2b)", ",pickups=true", true, ""),
        ("note(60,5b).gate(1)", "", false, "release"),
        ("note(60,5b).gate(1)", ",tails=true", true, ""),
        ("note(60,5b).gate(1)", ",pickups=true", false, "release"),
        (
            "note(60,1b).map_notes(fn(n)=>{offset:-1ms})",
            "",
            false,
            "pickup",
        ),
        (
            "note(60,1b).map_notes(fn(n)=>{offset:-1ms})",
            ",pickups=true",
            true,
            "",
        ),
        (
            "note(60,1b,at=3b).map_notes(fn(n)=>{offset:500ms})",
            ",tails=true",
            false,
            "note start",
        ),
        (
            "note(60,4b).gate(1).map_notes(fn(n)=>{release_offset:1ms})",
            "",
            false,
            "release",
        ),
        (
            "note(60,4b).gate(1).map_notes(fn(n)=>{release_offset:1ms})",
            ",tails=true",
            true,
            "",
        ),
        ("rest(8b)", "", true, ""),
    ] {
        let result = contract_result(&contained(pattern, options));
        assert_eq!(result.is_ok(), succeeds, "{pattern}{options}: {result:?}");
        if !succeeds {
            let message = format!("{:#}", result.unwrap_err());
            assert!(
                message.contains("checked")
                    && message.contains("part p")
                    && message.contains(diagnostic),
                "{message}"
            );
        }
    }
}

#[test]
fn containment_checks_controls_raw_events_and_each_duplicate_layer() {
    for (pattern, options, succeeds, class) in [
        ("control(64,1,at=4b)", ",tails=true", false, "control"),
        ("control(64,1,at=0b,offset=-1ms)", "", false, "control"),
        ("control(64,1,at=0b,offset=-1ms)", ",pickups=true", true, ""),
        ("cc(11,1,at=4b)", ",tails=true", false, "raw event"),
        (
            "cc(11,1,at=3b).map_raw(fn(r)=>{offset:500ms})",
            ",tails=true",
            false,
            "raw event",
        ),
        (
            "cc(11,1,at=0b).map_raw(fn(r)=>{offset:-1ms})",
            ",pickups=true",
            true,
            "",
        ),
    ] {
        let result = contract_result(&contained(pattern, options));
        assert_eq!(result.is_ok(), succeeds, "{pattern}: {result:?}");
        if !succeeds {
            assert!(format!("{:#}", result.unwrap_err()).contains(class));
        }
    }
    let layers = r#"a.require_contained(a.passage(4b,[a.part("p",rest(8b)),a.part("p",note(60,1b,at=4b))]))"#;
    assert!(contract_result(layers).is_err());
    assert!(contract_result("a.require_contained(a.passage(4b))").is_ok());
}

#[test]
fn containment_uses_complete_tempo_map_and_runs_before_gestures() {
    let source = format!(
        r#"use "std/arrange" as a;
let shared=a.require_contained(a.passage(4b,[a.part("p",note(60,1b,at=3b).gate(0.1)
    .map_notes(fn(n)=>{{offset:300ms}}))], [fn(c)=>{{assert(false,"gesture ran first");[]}}]));
a.build(a.sequence([a.occurrence("slow",a.edit(shared,"p",fn(p)=>rest(4b))),
 a.occurrence("fast",shared)]),merge({SETTINGS},{{tempos:[[4b,240]]}}))"#
    );
    let error = muz::lang::Evaluator::new().source(&source).unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("fast") && message.contains("note start"),
        "{message}"
    );
    let source = source.replace("[fn(c)=>{assert(false,\"gesture ran first\");[]}]", "[]");
    // Identical offset material passes at the slower occurrence and fails at the faster one.
    let source = source.replace("a.edit(shared,\"p\",fn(p)=>rest(4b))", "shared");
    let error = muz::lang::Evaluator::new().source(&source).unwrap_err();
    assert!(format!("{error:#}").contains("fast"));
    assert!(
        muz::lang::Evaluator::new()
            .source(&source.replace("[[4b,240]]", "[[4b,120]]"))
            .is_ok()
    );
}

#[test]
fn nested_containment_reads_final_child_slots_without_siblings() {
    let source = format!(
        r#"use "std/arrange" as a;
let child=a.require_contained(a.passage(2b,[a.part("p",note(60,1b))]));
let pair=a.group(a.sequence([a.occurrence("child",child),
 a.occurrence("sibling",a.passage(2b,[a.part("p",note(64,8b))]))]));
let nested=a.group(a.sequence([a.occurrence("pair",pair)]));
a.build(a.sequence([a.occurrence("outer",nested)]),{SETTINGS})"#
    );
    compile(&source).unwrap();
    let edited = source.replace(
        "a.occurrence(\"outer\",nested)",
        "a.occurrence(\"outer\",a.edit(nested,\"p\",fn(p)=>p.map_notes(fn(n)=>{at:n.at+2b})))",
    );
    let error = muz::lang::Evaluator::new().source(&edited).unwrap_err();
    assert!(format!("{error:#}").contains("outer/pair/child"));
    let emptied = source.replace(
        "a.occurrence(\"outer\",nested)",
        "a.occurrence(\"outer\",a.edit(nested,\"p\",fn(p)=>rest(8b)))",
    );
    compile(&emptied).unwrap();
}
