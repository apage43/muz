/// Evaluates through the same library path as `muz eval` and prints its JSON.
fn eval(source: &str) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.muz");
    std::fs::write(&path, source).unwrap();
    muz::lang::load(&path).unwrap().0.display_json()
}

#[test]
fn eval_prints_quantities_in_source_spelling() {
    let value = eval(
        "let n = phrase(\"C4:q D4:e.\").notes[1];\n\
         {at: n.at, dur: n.duration, bar: 2bars, sec: 250ms, db: -6dB, hz: 1kHz,\n\
          bpm: 90bpm, third: 1b / 3, scalar: 3, frac: 0.5, list: [1b, 2]}",
    );
    assert_eq!(value["at"], "1b");
    assert_eq!(value["dur"], "0.75b");
    assert_eq!(value["bar"], "2bar");
    assert_eq!(value["sec"], "0.25s");
    assert_eq!(value["db"], "-6dB");
    assert_eq!(value["hz"], "1000Hz");
    assert_eq!(value["bpm"], "90bpm");
    assert_eq!(value["third"], "1b/3");
    assert_eq!(value["scalar"], 3);
    assert_eq!(value["frac"], 0.5);
    assert_eq!(value["list"], serde_json::json!(["1b", 2]));
}

#[test]
fn printed_quantities_parse_back_to_the_same_values() {
    let value = eval("[0.75b, 2bar, 0.25s, -6dB, 1b / 3, 90bpm]");
    let pasted: Vec<_> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let again = eval(&format!("[{}]", pasted.join(", ")));
    assert_eq!(again, value);
}

#[test]
fn pattern_dump_matches_accessors_and_keeps_every_serde_field() {
    let source = "stack([phrase(\"C4:q D4:e.\"), control(controller=7, value=100, at=1b, offset=20ms), cc(7, 100, at=2b)])";
    let value = eval(source);
    let fields: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    assert_eq!(fields, ["controls", "notes", "raw", "span"]);
    assert_eq!(value["span"], "2b");
    assert_eq!(value["notes"][1]["at"], "1b");
    assert_eq!(value["notes"][1]["duration"], "0.75b");
    assert_eq!(value["notes"][1]["offset"], "0s");
    assert_eq!(value["controls"][0]["at"], "1b");
    assert_eq!(value["controls"][0]["offset"], "0.02s");
    assert_eq!(value["controls"][0]["controller"], 7);
    assert_eq!(value["raw"][0]["at"], "2b");
    assert_eq!(value["raw"][0]["bytes"], serde_json::json!([176, 7, 100]));
    assert_eq!(value["notes"], eval(&format!("let p = {source}; p.notes")));
}

#[test]
fn patterns_nested_in_lists_and_records_print_with_units() {
    let value = eval("[phrase(\"C4:q\"), {line: phrase(\"D4:e\")}]");
    assert_eq!(value[0]["span"], "1b");
    assert_eq!(value[0]["notes"][0]["duration"], "1b");
    assert_eq!(value[1]["line"]["notes"][0]["duration"], "0.5b");
}

#[test]
fn pattern_dump_shows_clock_clip_timing_in_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 8000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(dir.path().join("clip.wav"), spec).unwrap();
    for _ in 0..800 {
        writer.write_sample(0.0f32).unwrap();
    }
    writer.finalize().unwrap();
    let path = dir.path().join("t.muz");
    std::fs::write(
        &path,
        "clip(\"audio\", \"clip.wav\", {fade_in: 0ms, fade_out: 0ms}).pattern",
    )
    .unwrap();
    let value = muz::lang::load(&path).unwrap().0.display_json();
    let clock = &value["notes"][0]["clock"];
    assert_eq!(clock["duration"], "0.1s");
    assert_eq!(clock["offset"], "0s");
    assert!(clock["span"].as_str().unwrap().ends_with('s'));
}

#[test]
fn kind_names_every_kind_of_value() {
    let value = eval(
        "fn f(x) = x;\n\
         map([1, 2b, true, \"s\", [1], {a: 1}, phrase(\"C4:q\"), f, fn(x) => x, len, null], kind)",
    );
    assert_eq!(
        value,
        serde_json::json!([
            "number", "number", "bool", "text", "list", "record", "pattern", "function",
            "function", "function", "null"
        ])
    );
}
