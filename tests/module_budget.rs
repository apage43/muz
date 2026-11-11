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
