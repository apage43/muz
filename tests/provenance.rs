use muz::{
    assets::MemoryAssets,
    compile::Compiled,
    host::HostContext,
    lang::{Evaluator, FileSourceLoader},
};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
fn compile(source: &str, enabled: bool) -> Compiled {
    let context = HostContext {
        provenance: enabled,
        ..Default::default()
    };
    let mut e = Evaluator::with_context(Rc::new(FileSourceLoader), context);
    let value = e.source(source).unwrap();
    muz::compile::lower(
        value.get("__result").unwrap().clone(),
        Path::new("explain.muz"),
        vec![],
    )
    .unwrap()
}
fn compile_with_context(
    source: &str,
    context: HostContext,
    path: &str,
) -> anyhow::Result<Compiled> {
    let mut e = Evaluator::with_context(Rc::new(FileSourceLoader), context);
    e.path = PathBuf::from(path);
    let value = e.source(source)?;
    muz::compile::lower(
        value.get("__result").unwrap().clone(),
        Path::new(path),
        vec![],
    )
}
fn location_text(location: &muz::diagnostic::Location) -> String {
    serde_json::to_value(location).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned()
}
const SOURCE: &str = r#"let motif=note(60,1b); let phrase=overlay([motif.at(0b,key="first"),motif.at(2b,key="second")]); song({tracks:[track("p",phrase,synth("bell"))]})"#;
#[test]
fn provenance_is_optional_and_does_not_change_music_or_transfer() {
    let plain = compile(SOURCE, false);
    let traced = compile(SOURCE, true);
    assert_eq!(plain.session, traced.session);
    assert_eq!(plain.score[0].pattern, traced.score[0].pattern);
    let page = traced.explain_notes("p", 0, 1).unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.next, Some(1));
    assert!(page.rows[0].definition.is_some());
    assert!(
        plain.explain_notes("p", 0, 1).unwrap().rows[0]
            .definition
            .is_none()
    );
    assert!(
        muz::provenance::diff(&plain, &traced, 0, 100)
            .unwrap()
            .notes
            .rows
            .is_empty()
    );
}
#[test]
fn keyed_changes_and_renames_have_truthful_bounded_diffs() {
    let a = compile(SOURCE, true);
    let b = compile(&SOURCE.replace("note(60,1b)", "note(62,1b)"), true);
    let diff = muz::provenance::diff(&a, &b, 0, 1).unwrap();
    assert_eq!(diff.notes.total, 2);
    assert_eq!(diff.notes.rows[0].change, "changed");
    assert_eq!(diff.notes.next, Some(1));
    let c = compile(&SOURCE.replace("second", "other"), true);
    let diff = muz::provenance::diff(&a, &c, 0, 20).unwrap();
    assert_eq!(diff.notes.total, 2);
    assert!(diff.notes.rows.iter().any(|x| x.change == "removed"));
    assert!(diff.notes.rows.iter().any(|x| x.change == "added"));
}

#[test]
fn returning_wrapper_does_not_replace_the_relevant_primitive_edit() {
    let compiled = compile(
        r#"let raise = fn(p)=>p.transpose(7);
let motif = note(60,1b);
let result = raise(motif);
song({tracks:[track("p",result,synth("bell"))]})"#,
        true,
    );
    let note = &compiled.explain_notes("p", 0, 1).unwrap().rows[0];
    let pitch = note.field_edits.get("pitch").unwrap();
    assert!(location_text(pitch).contains("transpose(7)"));
    assert_eq!(
        location_text(note.latest_edit.as_ref().unwrap()),
        location_text(pitch),
        "the wrapper call must not replace the inner primitive edit",
    );
}

#[test]
fn sparse_transform_records_only_the_changed_note_and_field() {
    let compiled = compile(
        r#"let a = note(60,1b).at(0b,key="a");
let b = note(64,1b).at(1b,key="b");
let changed = a.map_notes(fn(n)=>{pitch:n.pitch+12});
song({tracks:[track("p",overlay([changed,b]),synth("bell"))]})"#,
        true,
    );
    let page = compiled.explain_notes("p", 0, 10).unwrap();
    let changed = page.rows.iter().find(|n| n.key.contains("a")).unwrap();
    let untouched = page.rows.iter().find(|n| n.key.contains("b")).unwrap();
    assert!(changed.field_edits.contains_key("pitch"));
    assert!(!changed.field_edits.contains_key("velocity"));
    assert!(location_text(changed.field_edits.get("pitch").unwrap()).contains("map_notes"));
    assert!(!untouched.field_edits.contains_key("pitch"));
}

#[test]
fn nested_placements_expansion_and_named_insertion_keep_occurrence_context() {
    let nested = compile(
        r#"let motif=note(60,1b);
let expanded=motif.flat_map_notes(fn(n)=>[{}, {pitch:n.pitch+12}]);
let nested=expanded.repeat(2).at(3b,key="section");
song({tracks:[track("p",nested,synth("bell"))]})"#,
        true,
    );
    let page = nested.explain_notes("p", 0, 10).unwrap();
    assert_eq!(page.total, 4);
    for note in &page.rows {
        let operations = note
            .occurrences
            .iter()
            .map(|o| o.operation.as_str())
            .collect::<Vec<_>>();
        assert!(operations.contains(&"at"), "{operations:?}");
        assert!(operations.contains(&"repeat"), "{operations:?}");
    }

    let inserted = compile(
        &SOURCE.replace(
            "motif.at(0b,key=\"first\")",
            "motif.at(4b,key=\"intro\"),motif.at(0b,key=\"first\")",
        ),
        true,
    );
    let diff = muz::provenance::diff(&compile(SOURCE, true), &inserted, 0, 20).unwrap();
    assert_eq!(diff.notes.total, 1);
    assert_eq!(diff.notes.rows[0].change, "added");
    assert!(diff.notes.rows[0].key.contains("intro"));
}

#[test]
fn removals_renames_and_duplicate_keys_are_reported_without_invented_matches() {
    let original = compile(SOURCE, true);
    let removed = compile(&SOURCE.replace(",motif.at(2b,key=\"second\")", ""), true);
    let page = muz::provenance::diff_page(&original, &removed, "occurrences", 0, 20).unwrap();
    assert!(page.rows.iter().any(|row| row["change"] == "removed"));

    let mut duplicates_a = compile(SOURCE, true);
    let mut duplicates_b = compile(&SOURCE.replace("note(60,1b)", "note(62,1b)"), true);
    for compiled in [&mut duplicates_a, &mut duplicates_b] {
        for note in &mut compiled.score[0].pattern.notes {
            note.key = "same".into();
        }
    }
    let diff = muz::provenance::diff(&duplicates_a, &duplicates_b, 0, 20).unwrap();
    assert_eq!(diff.notes.rows[0].change, "ambiguous duplicate key");
}

#[test]
fn imported_memory_midi_names_asset_version_track_and_event_order() {
    use muz::smf::{Document, Event, Kind};
    let midi = muz::smf::encode(&Document {
        format: 0,
        division: 480,
        tracks: vec![vec![
            Event {
                delta: 0,
                kind: Kind::Midi {
                    bytes: vec![0x90, 60, 100],
                },
            },
            Event {
                delta: 240,
                kind: Kind::Midi {
                    bytes: vec![0x80, 60, 0],
                },
            },
            Event {
                delta: 0,
                kind: Kind::Midi {
                    bytes: vec![0x90, 64, 100],
                },
            },
            Event {
                delta: 240,
                kind: Kind::Midi {
                    bytes: vec![0x80, 64, 0],
                },
            },
            Event {
                delta: 0,
                kind: Kind::Meta {
                    tag: 47,
                    data: vec![],
                },
            },
        ]],
    })
    .unwrap();
    let mut assets = MemoryAssets::default();
    assets.insert(PathBuf::from("/virtual/part.mid"), midi.clone().into(), 77);
    let compiled = compile_with_context(
        r#"song({tracks:[track("p",midi("part.mid"),synth("bell"))]})"#,
        HostContext {
            provenance: true,
            assets: Arc::new(assets),
            ..Default::default()
        },
        "/virtual/song.muz",
    )
    .unwrap();
    let page = compiled.explain_notes("p", 0, 10).unwrap();
    assert_eq!(page.total, 2);
    for (note, order) in page.rows.iter().zip([0, 2]) {
        let asset = note.asset.as_ref().unwrap();
        assert_eq!(asset.path, "/virtual/part.mid");
        assert_eq!(asset.version, (midi.len() as u64, 77));
        assert_eq!(asset.track, 0);
        assert_eq!(asset.order, order);
        assert!(
            note.location_quality
                .contains("asset track and event order")
        );
    }
}

#[test]
fn provenance_occurrence_arena_obeys_the_expansion_byte_budget() {
    let mut context = HostContext {
        provenance: true,
        ..Default::default()
    };
    context.expansion.bytes = 1024;
    let error = compile_with_context(
        r#"let a=note(60,1b); let b=note(61,1b); let c=note(62,1b); let d=note(63,1b); let e=note(64,1b); song({tracks:[track("p",a,synth("bell"))]})"#,
        context,
        "budget.muz",
    ).unwrap_err();
    assert!(
        format!("{error:#}").contains("provenance arena byte limit exceeded"),
        "{error:#}"
    );
}

#[test]
fn note_explanations_skip_oversized_rows_without_constructing_them() {
    let mut compiled = compile(SOURCE, true);
    compiled.score[0].pattern.notes[0].data.insert(
        "large".into(),
        serde_json::Value::String("x".repeat(1_100_000)),
    );
    // Explanation rows currently expose locations rather than note data, but a
    // very large skipped note must still be cheap and page by its stable offset.
    let page = compiled.explain_notes("p", 1, 1).unwrap();
    assert_eq!(page.rows.len(), 1);
    assert!(
        compiled
            .explain_notes("p", 0, 1001)
            .unwrap_err()
            .to_string()
            .contains("1000")
    );
}
