//! `format` placeholders: in order, numbered, and named.
fn eval(source: &str) -> anyhow::Result<serde_json::Value> {
    Ok(muz::lang::Evaluator::new()
        .source(source)?
        .get("__result")
        .unwrap()
        .json())
}
fn error(source: &str) -> String {
    format!("{:#}", eval(source).unwrap_err())
}

#[test]
fn numbered_and_named_placeholders_repeat_values() {
    assert_eq!(
        eval(r#"format("{0}:s {1}:s {0}:s {1}:s", ["C3", "G3"])"#).unwrap(),
        "C3:s G3:s C3:s G3:s"
    );
    assert_eq!(
        eval(r#"format("{lo}:e {hi}:e {lo}:q", {lo: "E2", hi: 52})"#).unwrap(),
        "E2:e 52:e E2:q"
    );
    assert_eq!(eval(r#"format("{}-{}", [1, "b"])"#).unwrap(), "1-b");
    // Brace text that is not a placeholder stays literal.
    assert_eq!(eval(r#"format("{x} {} {", [2])"#).unwrap(), "{x} 2 {");
    assert_eq!(eval(r#"format("{0} {first}", {first: 1})"#).unwrap(), "{0} 1");
    // A phrase written once with repeated pitches.
    assert_eq!(
        eval(r#"len(phrase(format("{0}:s {1}:s {0}:s", ["E4", "B4"])).notes)"#).unwrap(),
        3
    );
}

#[test]
fn placeholder_mistakes_name_the_fix() {
    let e = error(r#"format("{} {} {}", [1, 2])"#);
    assert!(e.contains("3 `{}` placeholders for 2 values") && e.contains("`{0}`"), "{e}");
    assert!(error(r#"format("{0} {}", [1, 2])"#).contains("mixes `{}` with numbered"));
    let e = error(r#"format("{0} {2}", [1, 2])"#);
    assert!(e.contains("{2} has no value") && e.contains("2 values"), "{e}");
    assert!(error(r#"format("{0}", [1, 2])"#).contains("value 1 is never used"));
    let e = error(r#"format("{hgih}", {high: 1})"#);
    assert!(e.contains("{hgih} names no field") && e.contains("high"), "{e}");
    assert!(error(r#"format("{}", {a: 1})"#).contains("name each field"));
}
