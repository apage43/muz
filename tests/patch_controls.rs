use muz::reconcile::{ReconcileOperation, plan_reconciliation};
#[test]
fn patch_control_edits_are_updates_including_serialized_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.muz");
    let source = r#"song({tracks:[track("a",note("C4",4b),voice_patch("p",{gain_db:-12,nodes:[{id:"tone",op:"param",value:440,min:20,max:20000},{id:"o",op:"osc",hz:"tone"}],output:"o"}))]})"#;
    std::fs::write(&path, source).unwrap();
    let old = muz::compile::compile(&path).unwrap().session;
    for change in [
        source.replace("value:440", "value:880"),
        source.replace("gain_db:-12", "gain_db:-6"),
        source.replace("gain_db:-12", "gain_db:-12,tone:660"),
    ] {
        std::fs::write(&path, change).unwrap();
        let new = muz::compile::compile(&path).unwrap().session;
        let restored = serde_json::from_value(serde_json::to_value(&new).unwrap()).unwrap();
        let plan = plan_reconciliation(0, &old, &restored).unwrap();
        assert!(
            plan.operations
                .iter()
                .any(|o| matches!(o, ReconcileOperation::SetParameters { .. }))
        );
        assert!(
            !plan
                .operations
                .iter()
                .any(|o| matches!(o, ReconcileOperation::Replace { .. }))
        );
    }
    std::fs::write(&path, source.replace("max:20000", "max:10000")).unwrap();
    let new = muz::compile::compile(&path).unwrap().session;
    assert!(
        plan_reconciliation(0, &old, &new)
            .unwrap()
            .operations
            .iter()
            .any(|o| matches!(o, ReconcileOperation::Replace { .. }))
    );
}
