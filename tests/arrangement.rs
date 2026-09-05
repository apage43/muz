use muz::{
    compile::{Compiled, PPQ},
    model::TrackSource,
};
fn compile(source: &str) -> anyhow::Result<Compiled> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("arrangement.muz");
    std::fs::write(&path, source)?;
    muz::compile::compile(&path)
}
const SETTINGS: &str =
    r#"{tempo:120,meter:[3,4],tracks:[track("p",rest(0b),synth("bell"))],tail:0}"#;
fn arranged(prefix: f64, tempo: u32, nested: bool) -> Compiled {
    let grouping = if nested {
        "a.group(a.sequence([a.occurrence(\"inner\", bridge)]))"
    } else {
        "bridge"
    };
    compile(&format!(r#"
use "std/arrange" as a;
let bridge = a.passage(3b,[a.part("p",phrase("C4:q D4:q"))],
 [a.local("p.gain_db",[[0b,-6],[1b,0]]),a.clock("p.pan",[[0ms,0],[20ms,1]])],[[0b,{tempo}]]);
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
        let local = curves.iter().find(|a| a.target == "p.gain_db").unwrap();
        let clock = curves.iter().find(|a| a.target == "p.pan").unwrap();
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
let shared=a.passage(3b,[a.part("p",phrase("C4:q"))],[a.local("p.gain_db",[[0b,-6],[1b,0],[3b,-6]])]);
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
 [automation("p.gain_db",curve([[n.at,-6],[n.at+n.duration,0]]))]
}}]);
let nested=a.group(a.sequence([a.occurrence("inner",child)]));
a.build(a.sequence([a.occurrence("lead",a.passage(3b)),a.occurrence("outer",nested)]),{SETTINGS})"#
    ));
    let c = c.unwrap();
    assert_eq!(c.session.extras.automation[0].points[0].seconds, 1.5);
    assert_eq!(c.score[0].pattern.notes[0].at, muz::music::b(3));
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
}
