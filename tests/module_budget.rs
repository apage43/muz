//! Imports share one evaluation budget. A module can keep self-checks that run
//! only when it is the entry point, and a budget error names the spenders.
fn write(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn is_entry_is_true_only_in_the_module_evaluated_directly() {
    let dir = tempfile::tempdir().unwrap();
    let part = write(
        dir.path(),
        "part.muz",
        r#"let mode = if is_entry() { "self-test" } else { "imported" }; let main = mode;"#,
    );
    let song = write(
        dir.path(),
        "song.muz",
        r#"use "part.muz" as p; let main = [p.mode, is_entry()];"#,
    );
    let direct = muz::lang::load(&part).unwrap().0;
    assert_eq!(direct.json(), "self-test");
    let imported = muz::lang::load(&song).unwrap().0;
    assert_eq!(imported.json(), serde_json::json!(["imported", true]));
}

#[test]
fn an_expr_inspects_a_module_without_its_entry_self_check() {
    let dir = tempfile::tempdir().unwrap();
    let part = write(
        dir.path(),
        "contract.muz",
        r#"let roots = [1, 2, 3];
fn self_test() { assert(len(roots) == 4, "four roots") }
let checked = if is_entry() { self_test() } else { true };"#,
    );
    // Evaluated as the program, the self-check runs (and here fails).
    let program = format!("{:?}", muz::lang::evaluate(&part, None, None).unwrap_err());
    assert!(program.contains("four roots"), "{program}");
    // Under --expr the data stays readable, and the check runs once, on request.
    let roots = muz::lang::evaluate(&part, None, Some("roots")).unwrap();
    assert_eq!(roots.display_json(), serde_json::json!([1, 2, 3]));
    let entry = muz::lang::evaluate(&part, None, Some("[checked, is_entry()]")).unwrap();
    assert_eq!(entry.display_json(), serde_json::json!([true, false]));
    let asked = format!("{:?}", muz::lang::evaluate(&part, None, Some("self_test()")).unwrap_err());
    assert!(asked.contains("<expr>:1:1") && !asked.contains("contract.muz:3"), "{asked}");
}

#[test]
fn a_budget_error_lists_operations_per_module() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "heavy.muz",
        "let total = fold(range(3000), 0, fn(sum, i) => sum + i);",
    );
    write(dir.path(), "light.muz", "let x = 1 + 2;");
    let song = write(
        dir.path(),
        "song.muz",
        r#"use "light.muz" as l; use "heavy.muz" as h; let main = l.x;"#,
    );
    let context = muz::host::HostContext {
        evaluation_steps: 5000,
        ..Default::default()
    };
    let error = context.run(|| muz::lang::load(&song)).unwrap_err();
    let text = format!("{error:?}");
    assert!(text.contains("evaluation budget exceeded (5000 operations)"), "{text}");
    assert!(text.contains("operations spent per module"), "{text}");
    let heavy = text.find("heavy.muz ").expect("heavy listed");
    let light = text.find("light.muz ").expect("light listed");
    assert!(heavy < light, "the largest spender comes first: {text}");
}

#[test]
fn an_import_alias_called_as_a_function_is_reported_at_the_use_line() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "keys.muz", "let part = 1;");
    let song = write(
        dir.path(),
        "song.muz",
        r#"use "keys.muz" as keys;
fn names(r) = keys(r);
let main = names({a: keys.part});"#,
    );
    let text = format!("{:?}", muz::lang::load(&song).unwrap_err());
    assert!(
        text.contains("import alias 'keys' is also called as a function"),
        "{text}"
    );
    assert!(text.contains("hides the builtin keys()"), "{text}");
    assert!(text.contains("song.muz:1"), "{text}");

    // A lambda parameter of the same name is its own binding.
    let fine = write(
        dir.path(),
        "fine.muz",
        r#"use "keys.muz" as keys;
let main = map([fn(r) => len(r)], fn(keys) => keys({a: 1}));"#,
    );
    muz::lang::load(&fine).unwrap();
}

#[test]
fn calling_a_let_that_hides_a_builtin_names_the_binding() {
    let dir = tempfile::tempdir().unwrap();
    let song = write(
        dir.path(),
        "song.muz",
        "let keys = {a: 1};\nlet main = keys({b: 2});",
    );
    let text = format!("{:?}", muz::lang::load(&song).unwrap_err());
    assert!(
        text.contains("'keys' is a record here, not the builtin keys()"),
        "{text}"
    );
}
