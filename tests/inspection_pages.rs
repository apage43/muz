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
fn graph_metadata_stays_bounded_and_events_keep_their_timing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("instrument.sfz"),
        "<control> set_cc1=64\n<region> sample=*silence key=60",
    )
    .unwrap();
    let path = dir.path().join("inspection.muz");
    std::fs::write(
        &path,
        r#"song({tempo:100,meter:[3,4],tracks:[track("lead",
            stack([note(60,1b),note(62,1b,at=2b)]),
            sfz("instrument.sfz",{max_voices:16}))],tail:0})"#,
    )
    .unwrap();
    let mut c = muz::compile::inspect(&path).unwrap();
    let sfz = c.session.tracks[0].instrument.sfz.as_mut().unwrap();
    sfz.program.regions = vec![sfz.program.regions[0].clone(); 8192];
    assert!(serde_json::to_vec(&c.session).unwrap().len() > muz::inspect::MAX_PAGE_BYTES);

    let first = page_session(
        &c.session,
        7,
        "graph",
        &PageRequest { limit: 1, ..Default::default() },
    )
    .unwrap();
    assert_eq!(first.total, 3);
    assert_eq!(first.next, Some(1));
    assert_eq!(first.rows[0]["kind"], "transport");
    assert_eq!(first.rows[0]["detail"]["mode"], "one_shot");
    assert_eq!(first.rows[0]["detail"]["meter"], serde_json::json!([3, 4]));
    assert_eq!(first.rows[0]["detail"]["meter_source"], "declared");
    let rest = page_session(
        &c.session,
        7,
        "graph",
        &PageRequest { revision: Some(7), offset: first.next.unwrap(), ..Default::default() },
    )
    .unwrap();
    assert_eq!(rest.total, 3);
    assert_eq!(rest.next, None);
    assert_eq!(rest.rows[0]["kind"], "master");
    let track = &rest.rows[1];
    assert_eq!(track["id"], "lead");
    assert_eq!(track["detail"]["source"]["summary"]["ppq"], muz::compile::PPQ);
    assert_eq!(track["detail"]["instrument"]["kind"], "builtin.sfz");
    assert_eq!(track["detail"]["instrument"]["controls"]["cc1"], 64.0);
    assert_eq!(track["detail"]["output"]["to"], "master");
    assert!(track["detail"]["instrument"].get("sfz").is_none());
    let mut rows = first.rows;
    rows.extend(rest.rows);
    let full = muz::inspect::session(&c.session, "graph").unwrap();
    assert_eq!(full, serde_json::json!(rows));
    assert!(serde_json::to_vec(&full).unwrap().len() < muz::inspect::MAX_PAGE_BYTES);

    let first = page_session(
        &c.session,
        7,
        "performance",
        &PageRequest { track: Some("lead".into()), limit: 1, ..Default::default() },
    )
    .unwrap();
    assert_eq!(first.rows[0]["stream"], "notes");
    assert_eq!(first.rows[0]["event"]["start_tick"], 0);
    let rest = page_session(
        &c.session,
        7,
        "performance",
        &PageRequest {
            track: Some("lead".into()), revision: Some(7),
            offset: first.next.unwrap(), ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rest.rows[0]["stream"], "notes");
    assert_eq!(rest.rows[0]["event"]["start_tick"], 2 * muz::compile::PPQ as u64);
    let tempo = rest.rows.iter().find(|row| row["stream"] == "tempos").unwrap();
    assert_eq!(tempo["event"]["tick"], 0);
    assert_eq!(tempo["event"]["micros_per_quarter"], 600_000);
    assert_eq!(rest.next, None);

    // The default CLI path must not switch back to full-session serialization.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_muz"))
        .args(["inspect", path.to_str().unwrap(), "--view", "graph", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let page: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(page["view"], "graph");
    assert_eq!(page["rows"][0]["detail"]["meter"], serde_json::json!([3, 4]));
    assert_eq!(page["rows"][2]["detail"]["source"]["summary"]["ppq"], muz::compile::PPQ);
    assert_eq!(page["total"], 3);
    assert!(page["next"].is_null());
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
