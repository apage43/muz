use muz::{
    audio::{AudioConfig, AudioEngine, PreparedTransaction},
    lang::Evaluator,
};
use std::path::Path;

fn compile(source: &str) -> muz::Session {
    let value = Evaluator::new().source(source).unwrap();
    muz::compile::lower(
        value.get("__result").unwrap().clone(),
        Path::new("portable.muz"),
        vec![],
    )
    .unwrap()
    .session
}
fn config() -> AudioConfig {
    AudioConfig {
        sample_rate: 48_000.,
        max_frames: 64,
        offline: true,
    }
}

#[test]
fn no_clock_prepare_applies_and_stale_apply_leaves_engine_unchanged() {
    let old = compile(
        r#"song({tracks:[track("x",note(60,4b),voice_patch("p",{nodes:[{id:"level",op:"param",value:0.2,min:0,max:1}],output:"level"}))],tail:0})"#,
    );
    let mut next = old.clone();
    next.tracks[0].instrument.params.insert("level".into(), 0.4);
    let plan = muz::plan_reconciliation(0, &old, &next).unwrap();
    let mut tx = PreparedTransaction::prepare(&old, &next, &plan, 7, None, config()).unwrap();
    assert_eq!(tx.event_started(), None);
    let mut engine = AudioEngine::new(&old, config()).unwrap();
    tx.apply(&mut engine).unwrap();
    assert_eq!(engine.revision(), 1);
    let mut stale = PreparedTransaction::prepare(&old, &next, &plan, 8, None, config()).unwrap();
    assert!(stale.apply(&mut engine).is_err());
    assert_eq!(engine.revision(), 1);
}

#[test]
fn structural_fades_are_selective_and_consequences_use_retention_decisions() {
    let old = compile(
        r#"song({tracks:[track("keep",note(60,4b),synth("pad")),track("change",note(64,4b),synth("bell"))],tail:0})"#,
    );
    let mut next = old.clone();
    next.tracks[1].instrument.generation += 1;
    let plan = muz::plan_reconciliation(0, &old, &next).unwrap();
    let tx = PreparedTransaction::prepare(&old, &next, &plan, 1, None, config()).unwrap();
    let out = tx.fade_out().unwrap();
    let input = tx.fade_in().unwrap();
    assert_eq!(out.tracks, 0b10);
    assert_eq!(input.tracks, 0b10);
    let rows = muz::audio::transaction::processor_consequences(&old, &next);
    assert!(rows.iter().any(|r| r["effect"] == "retained"));
    assert!(rows.iter().any(|r| r["effect"] == "replaced"));
}
