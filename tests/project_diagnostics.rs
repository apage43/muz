//! Small JSON5 sessions exercise field identity independently of value text.
const PROJECT: &str = r#"{
  schema: 1,
  transport: {mode:'loop', bpm:120, meter:[4,4], loop_beats:4},
  master: {id:'master'},
  tracks: [{
    id:'lead',
    name:'é, // not a comment, } [ :',
    source:{kind:'pattern', id:'phrase', notes:[
      {id:'n', at:0, duration:1, key:60, velocity:0.8},
    ]},
    instrument:{id:'voice', kind:'builtin.poly_synth', params:{gain_db:-12}},
    inserts:[{id:'tone', kind:'builtin.lowpass', params:{cutoff_hz:1000}}],
    output:{id:'out', to:'master', gain_db:0},
  }],
}"#;

fn error(source: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json5");
    std::fs::write(&path, source).unwrap();
    muz::parse_project(&path).unwrap_err().to_string()
}

fn assert_field(source: &str, field: &str, value: &str) -> String {
    let error = error(source);
    assert!(error.contains(&format!("field: {field}")), "{error}");
    let at = source.rfind(value).unwrap();
    let line = source[..at].bytes().filter(|c| *c == b'\n').count() + 1;
    let start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let col = source[start..at].chars().count() + 1;
    assert!(
        error.contains(&format!("project.json5:{line}:{col}:")),
        "{error}"
    );
    error
}

#[test]
fn reported_semantic_failures_name_the_exact_field_and_location() {
    muz::parse_session_with_root(PROJECT, ".").unwrap();
    for (from, to, field, value, message) in [
        (
            "schema: 1",
            "schema: 2",
            "schema",
            "2,",
            "unsupported schema",
        ),
        (
            "gain_db:-12",
            "cutof_hz:123",
            "tracks[0].instrument.params.cutof_hz",
            "123",
            "unknown parameter",
        ),
        (
            "id:'tone'",
            "id:'voice'",
            "tracks[0].inserts[0].id",
            "'voice'",
            "duplicate global id",
        ),
        (
            "kind:'builtin.poly_synth', params:{gain_db:-12}",
            "kind:'builtin.lowpass', params:{}",
            "tracks[0].instrument.kind",
            "'builtin.lowpass'",
            "must be an instrument",
        ),
        (
            "to:'master'",
            "to:'missing'",
            "tracks[0].output.to",
            "'missing'",
            "undeclared bus",
        ),
        ("bpm:120", "bpm:0", "transport.bpm", "0, meter", "bpm must"),
        (
            "duration:1",
            "duration:-1",
            "tracks[0].source.notes[0].duration",
            "-1,",
            "note duration",
        ),
        (
            "velocity:0.8",
            "velocity:8",
            "tracks[0].source.notes[0].velocity",
            "8}",
            "velocity must",
        ),
        (
            "gain_db:0",
            "gain_db:25",
            "tracks[0].output.gain_db",
            "25",
            "gain_db must",
        ),
    ] {
        let source = PROJECT.replace(from, to);
        // The instrument-kind case intentionally repeats the same value in an
        // insert later in the document. Remove that unrelated insert here;
        // the dedicated test below checks repeated values at different paths.
        let source = if field == "tracks[0].instrument.kind" {
            source.replace(
                "inserts:[{id:'tone', kind:'builtin.lowpass', params:{cutoff_hz:1000}}]",
                "inserts:[]",
            )
        } else {
            source
        };
        let error = assert_field(&source, field, value);
        assert!(error.contains(message), "{error}");
        if message == "duplicate global id" {
            assert!(
                error.contains("first declared at tracks[0].instrument.id"),
                "{error}"
            );
        }
    }
}

#[test]
fn repeated_values_and_escaped_keys_are_located_by_path() {
    let source = PROJECT
        .replace("params:{gain_db:-12}", "params:{'cut\\u006ff_hz':-12}")
        .replace("cutoff_hz:1000", "cutoff_hz:-12");
    let error = error(&source);
    assert!(
        error.contains("field: tracks[0].instrument.params.cutof_hz"),
        "{error}"
    );
    assert!(error.contains("project.json5:11:80:"), "{error}");
    let source = PROJECT.replace("params:{gain_db:-12}", "params:{'bad.key':1}");
    assert_field(&source, r#"tracks[0].instrument.params["bad.key"]"#, "1}}");
    // JSON5 map values use the final occurrence of a repeated parameter key.
    let source = PROJECT.replace("cutoff_hz:1000", "cutoff_hz:1000, cutoff_hz:-10");
    assert_field(&source, "tracks[0].inserts[0].params.cutoff_hz", "-10");
}

#[test]
fn json5_trivia_line_endings_and_inline_sources_keep_locations() {
    for newline in ["\n", "\r\n", "\r", "\u{2028}", "\u{2029}"] {
        let source = format!(
            "{{{newline} /* schema: 2, [ ] {{ */ schema: 2,{newline} transport:{{mode:'loop',bpm:120,meter:[4,4],loop_beats:4}},{newline} master:{{id:'master'}},{newline}}}"
        );
        let error = error(&source);
        assert!(error.contains("project.json5:2:33:"), "{error}");
        assert!(error.contains("field: schema"), "{error}");
    }
    let source = PROJECT
        .replace(
            "name:'é, // not a comment, } [ :'",
            "name:'é\\\ncontinued' /* : } ] */",
        )
        .replace("schema: 1", "schema: 2");
    assert_field(&source, "schema", "2,");
    let error = muz::parse_session_with_root(&source, ".")
        .unwrap_err()
        .to_string();
    assert!(error.contains("<project>:2:11:"), "{error}");
    let syntax_error = error_syntax("{\r\n schema: @}");
    assert!(
        syntax_error.contains("project.json5:2:10:"),
        "{syntax_error}"
    );
    assert!(syntax_error.contains("invalid JSON5"), "{syntax_error}");
}

fn error_syntax(source: &str) -> String {
    error(source)
}

#[test]
fn missing_fields_point_at_the_containing_record_and_routes_retain_paths() {
    let source = PROJECT.replace(
        "master: {id:'master'},",
        "master: {id:'master'},\n  buses:[{id:'room'}],",
    );
    assert_field(&source, "buses[0].output", "{id:'room'}");
    let source = PROJECT.replace(
        "master: {id:'master'},",
        "master: {id:'master'},\n  buses:[{id:'room',output:{id:'room.out',to:'gone',gain_db:0}}],",
    );
    assert_field(&source, "buses[0].output.to", "'gone'");
}

#[test]
fn later_array_entries_and_unicode_prefixes_keep_field_identity() {
    let mut project: serde_json::Value = json5::from_str(PROJECT).unwrap();
    let track = project["tracks"][0].clone();
    project["tracks"] = serde_json::Value::Array(
        (0..3)
            .map(|i| {
                let mut track = track.clone();
                track["id"] = format!("lead{i}").into();
                track["source"]["id"] = format!("phrase{i}").into();
                track["source"]["notes"][0]["id"] = format!("note{i}").into();
                track["instrument"]["id"] = format!("voice{i}").into();
                track["inserts"][0]["id"] = format!("tone{i}").into();
                track["output"]["id"] = format!("out{i}").into();
                track
            })
            .collect(),
    );
    project["tracks"][2]["instrument"]["params"] = serde_json::json!({"cutof_hz":123});
    assert_field(
        &serde_json::to_string_pretty(&project).unwrap(),
        "tracks[2].instrument.params.cutof_hz",
        "123",
    );
    let source = PROJECT
        .replace(
            "schema: 1",
            "master: {id:'master',name:'é'}, sch\\u0065ma: 2",
        )
        .replace("  master: {id:'master'},\n", "");
    assert_field(&source, "schema", "2,");
}
