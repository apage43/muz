//! `attempt` turns a failure into a value so a self-test can list every failure.
use muz::lang::Value;

fn eval(source: &str) -> anyhow::Result<Value> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("part.muz");
    std::fs::write(&path, source)?;
    Ok(muz::lang::load(&path)?.0)
}

#[test]
fn attempt_returns_the_value_or_the_failure_and_its_place() {
    let v = eval("[attempt(fn() => 1 + 2),\n attempt(fn() => assert(1 == 2, \"one is not two\"))]")
        .unwrap()
        .json();
    assert_eq!(v[0], serde_json::json!({"ok": true, "value": 3}));
    assert_eq!(v[1]["ok"], false);
    assert_eq!(v[1]["error"], "one is not two");
    assert!(
        v[1]["at"].as_str().unwrap().ends_with("part.muz:2:18"),
        "{}",
        v[1]["at"]
    );
}

#[test]
fn std_checks_all_reports_every_failed_self_test() {
    let source = r#"use "std/checks" as c;
let tests = {
    fits: fn() => assert(true, "fits"),
    range: fn() => assert(false, "the bass leaves its range"),
    voices: fn() => assert(false, "three voices at bar 4")
};
[c.failures(tests), c.all({fits: tests.fits})]"#;
    let v = eval(source).unwrap().json();
    let failures = v[0].as_array().unwrap();
    assert_eq!(failures.len(), 2);
    assert!(
        failures[0]
            .as_str()
            .unwrap()
            .starts_with("range: the bass leaves its range (")
    );
    assert!(
        failures[1]
            .as_str()
            .unwrap()
            .starts_with("voices: three voices at bar 4 (")
    );
    assert_eq!(v[1], true);
    let error = format!(
        "{:#}",
        eval(&source.replace("{fits: tests.fits}", "tests")).unwrap_err()
    );
    assert!(error.contains("2 of 3 self-tests failed"), "{error}");
    assert!(
        error.contains("range: the bass leaves its range"),
        "{error}"
    );
    assert!(error.contains("voices: three voices at bar 4"), "{error}");
}

#[test]
fn attempt_does_not_catch_the_evaluation_budget_or_call_depth() {
    let error = eval(
        "fn spin() = map(range(200000), fn(i) => map(range(30), fn(j) => i + j));\nattempt(spin)",
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("evaluation budget exceeded"),
        "{error:#}"
    );
}
