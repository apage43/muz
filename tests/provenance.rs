use muz::{
    compile::Compiled,
    host::HostContext,
    lang::{Evaluator, FileSourceLoader},
};
use std::{path::Path, rc::Rc};
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
