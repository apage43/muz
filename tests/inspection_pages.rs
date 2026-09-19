use muz::inspect::{PageRequest, page_compiled, page_session};

fn compiled(source: &str) -> muz::compile::Compiled {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("inspection.muz");
    std::fs::write(&path, source).unwrap();
    muz::compile::inspect(&path).unwrap()
}

const SOURCE: &str = r#"song({
 tracks:[track("lead",stack([note(60,1b),note(62,1b,at=2b)]),voice_patch("tone",{nodes:[{id:"a",op:"param",value:0.2},{id:"b",op:"sum",inputs:["a"]}],output:"b"}))],
 automation:[automation("lead.level.gain_db",curve([[0b,-6],[2b,0]],"step"))],
 sections:[section("first",1b)],tail:0
})"#;

#[test]
fn pages_have_revision_and_continuation() {
    let c = compiled(SOURCE);
    let first = page_session(
        &c.session,
        7,
        "graph",
        &PageRequest {
            limit: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(first.revision, 7);
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.next, Some(1));
    let second = page_session(
        &c.session,
        7,
        "graph",
        &PageRequest {
            offset: 1,
            limit: 100,
            revision: Some(7),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(second.next.is_none());
    assert_eq!(first.total, second.total);
}

#[test]
fn zero_limit_and_stale_revision_are_explicit_errors() {
    let c = compiled(SOURCE);
    assert!(
        page_session(
            &c.session,
            3,
            "sections",
            &PageRequest {
                limit: 0,
                ..Default::default()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("greater than zero")
    );
    assert!(
        page_session(
            &c.session,
            3,
            "sections",
            &PageRequest {
                revision: Some(2),
                ..Default::default()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("stale inspection revision")
    );
}

#[test]
fn nested_patch_and_automation_details_are_separate_pages() {
    let c = compiled(SOURCE);
    let patches = page_session(&c.session, 0, "patches", &Default::default()).unwrap();
    assert_eq!(patches.rows[0]["nodes"], 2);
    assert!(patches.rows[0].get("patch").is_none());
    let nodes = page_session(
        &c.session,
        0,
        "patch_nodes",
        &PageRequest {
            limit: 1,
            track: Some("lead".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(nodes.total, 2);
    assert_eq!(nodes.next, Some(1));
    let lanes = page_session(&c.session, 0, "automation", &Default::default()).unwrap();
    assert_eq!(lanes.rows[0]["points"], 2);
    assert!(lanes.rows[0].get("point").is_none());
    let points = page_session(
        &c.session,
        0,
        "automation_points",
        &PageRequest {
            track: Some("lead.level.gain_db".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(points.total, 2);
}

#[test]
fn performance_filters_before_paging_and_locations_keep_exact_keys() {
    let c = compiled(SOURCE);
    let performance = page_session(
        &c.session,
        0,
        "performance",
        &PageRequest {
            track: Some("lead".into()),
            start_tick: Some(0),
            end_tick: Some(muz::compile::tick(1.5)),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        performance
            .rows
            .iter()
            .filter(|row| row["stream"] == "notes")
            .count(),
        1
    );
    let locations = page_compiled(
        &c,
        0,
        "locations",
        &PageRequest {
            limit: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(locations.rows.len(), 1);
    assert_eq!(
        locations.rows[0]["key"],
        c.locations.keys().next().unwrap().as_str()
    );
}

#[test]
fn one_oversized_nested_row_is_rejected() {
    let mut c = compiled(SOURCE);
    c.session.tracks[0].instrument.patch.as_mut().unwrap()["nodes"] =
        serde_json::json!([{"id":"huge","op":"param","label":"x".repeat(1024*1024)}]);
    let error = page_session(&c.session, 0, "patch_nodes", &Default::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("one inspection row exceeds 1 MiB")
    );
}

#[test]
fn byte_continuation_stops_at_first_omitted_row() {
    let mut c = compiled(SOURCE);
    c.session.tracks[0].instrument.patch.as_mut().unwrap()["nodes"] = serde_json::json!([
        {"id":"a","op":"param","label":"x".repeat(600_000)},
        {"id":"b","op":"param","label":"y".repeat(600_000)},
        {"id":"c","op":"param"}
    ]);
    let first = page_session(
        &c.session,
        0,
        "patch_nodes",
        &PageRequest {
            limit: 100,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.next, Some(1));
    let second = page_session(
        &c.session,
        0,
        "patch_nodes",
        &PageRequest {
            offset: 1,
            limit: 100,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(second.rows.len(), 2);
    assert!(second.next.is_none());
}

#[test]
fn overview_is_bounded_and_time_filtered() {
    let c = compiled(SOURCE);
    let page = page_session(
        &c.session,
        0,
        "performance_overview",
        &PageRequest {
            track: Some("lead".into()),
            start_tick: Some(0),
            end_tick: Some(muz::compile::tick(4.0)),
            limit: 3,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.rows.len(), 3);
    assert_eq!(page.next, Some(3));
    assert!(page.total <= 128);
    assert_eq!(page.rows[0]["pitches"], serde_json::json!([60]));
    let all = page_session(
        &c.session,
        0,
        "performance_overview",
        &PageRequest {
            track: Some("lead".into()),
            start_tick: Some(0),
            end_tick: Some(muz::compile::tick(4.0)),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(all.rows[32]["pitches"], serde_json::json!([]));
    assert_eq!(all.rows[64]["pitches"], serde_json::json!([62]));
    let muz::model::TrackSource::Midi(midi) = &c.session.tracks[0].source else {
        panic!("MIDI source")
    };
    let release = midi.imported.notes[0].duration_ticks;
    assert!(all.rows[..32].iter().all(|row| {
        row["pitches"]
            == if row["start_tick"].as_u64().unwrap() < release {
                serde_json::json!([60])
            } else {
                serde_json::json!([])
            }
    }));
    let error = page_session(
        &c.session,
        0,
        "performance_overview",
        &PageRequest {
            track: Some("lead".into()),
            start_tick: Some(u64::MAX),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("overflows u64"));
}
