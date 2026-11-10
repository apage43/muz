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
