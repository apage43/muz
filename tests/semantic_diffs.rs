use muz::lang::Evaluator;
use std::path::Path;

fn compile(source: &str) -> muz::compile::Compiled {
    let value = Evaluator::new().source(source).unwrap();
    muz::compile::lower(
        value.get("__result").unwrap().clone(),
        Path::new("diff.muz"),
        vec![],
    )
    .unwrap()
}

#[test]
fn semantic_categories_report_sound_changes_and_page_independently() {
    let old = compile(
        r#"song({tempo:120,tracks:[track("x",stack([note(60,1b).annotate("all",{v:1}),cc(11,2)]),synth("pad"))],tail:0})"#,
    );
    let new = compile(
        r#"song({tempo:90,tracks:[track("x",stack([note(62,1b).annotate("all",{v:2}),cc(11,3)]),synth("bell"))],tail:0})"#,
    );
    for category in [
        "notes",
        "metadata",
        "occurrences",
        "controllers",
        "tempos",
        "performed",
        "consequences",
    ] {
        let page = muz::provenance::diff_page(&old, &new, category, 0, 1).unwrap();
        if [
            "notes",
            "metadata",
            "controllers",
            "tempos",
            "performed",
            "consequences",
        ]
        .contains(&category)
        {
            assert!(page.total > 0, "{category}");
        }
        assert!(page.rows.len() <= 1, "{category}");
        if page.total > 1 {
            let next = muz::provenance::diff_page(&old, &new, category, 1, 1).unwrap();
            assert_eq!(next.rows.len(), 1, "{category}");
        }
    }
    assert!(muz::provenance::diff_page(&old, &new, "notes", 0, 0).is_err());
    assert!(muz::provenance::diff_page(&old, &new, "unsupported", 0, 1).is_err());
}

#[test]
fn diff_pages_do_not_construct_skipped_oversized_rows_and_stop_at_byte_boundary() {
    let source = r#"song({tracks:[track("x",stack([note(60,1b,at=0b),note(62,1b,at=2b),note(64,1b,at=4b)]),synth("pad"))],tail:0})"#;
    let old = compile(source);
    let mut new = compile(source);
    {
        let muz::model::TrackSource::Midi(m) = &mut new.session.tracks[0].source else {
            panic!("performed source")
        };
        m.imported.notes[0].annotations.insert(
            "large".into(),
            serde_json::Value::String("x".repeat(1_100_000)),
        );
        m.imported.notes[1].annotations.insert(
            "large".into(),
            serde_json::Value::String("y".repeat(600_000)),
        );
        m.imported.notes[2].attack_velocity = m.imported.notes[2].attack_velocity.saturating_sub(1);
    }

    let error = muz::provenance::diff_page(&old, &new, "performed", 0, 100).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("one inspection row exceeds 1 MiB")
    );

    // The first row is outside this page, so it must neither be constructed nor
    // rejected. The second and small third row fit together.
    let page = muz::provenance::diff_page(&old, &new, "performed", 1, 100).unwrap();
    assert_eq!(page.rows.len(), 2);
    assert!(page.next.is_none());

    let muz::model::TrackSource::Midi(m) = &mut new.session.tracks[0].source else {
        panic!("performed source")
    };
    m.imported.notes[0].annotations.insert(
        "large".into(),
        serde_json::Value::String("x".repeat(600_000)),
    );
    let first = muz::provenance::diff_page(&old, &new, "performed", 0, 100).unwrap();
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.next, Some(1));
}

#[test]
fn metadata_extras_name_each_changed_field() {
    let old = compile(r#"song({title:"before",tracks:[track("x",note(60),synth("pad"))],tail:0})"#);
    let new = compile(r#"song({title:"after",tracks:[track("x",note(60),synth("pad"))],tail:1})"#);
    let page = muz::provenance::diff_page(&old, &new, "metadata", 0, 100).unwrap();
    let fields = page
        .rows
        .iter()
        .filter_map(|row| row["field"].as_str())
        .collect::<Vec<_>>();
    assert!(fields.contains(&"title"));
    assert!(fields.contains(&"tail"));
    assert!(
        muz::provenance::diff_page(&old, &new, "metadata", 0, 1001)
            .unwrap_err()
            .to_string()
            .contains("1000")
    );
}
