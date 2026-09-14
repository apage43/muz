use muz::{
    audio::{AudioConfig, AudioEngine, PreparedTransaction},
    lang::Evaluator,
    reconcile::ReconcileOperation,
};
use std::{path::Path, time::Instant};

fn compile(source: &str) -> anyhow::Result<muz::Session> {
    let value = Evaluator::new().source(source)?;
    Ok(muz::compile::lower(
        value.get("__result").unwrap().clone(),
        Path::new("boundary.muz"),
        vec![],
    )?
    .session)
}
fn config() -> AudioConfig {
    AudioConfig {
        sample_rate: 48000.,
        max_frames: 256,
        offline: true,
    }
}
fn sample(e: &mut AudioEngine) -> f32 {
    e.set_running(true);
    let mut pcm = [0.; 512];
    for _ in 0..4 {
        e.render_interleaved(&mut pcm, 2).unwrap();
    }
    pcm[500]
}
const SOURCE: &str = r#"song({tracks:[track("x",note(60,4b,velocity=1).gate(1),voice_patch("p",{gain_db:0,nodes:[{id:"level",op:"param",value:0.2,min:0,max:1}],output:"level"}),{gain:-12,sends:{send:{gain:0,pre:false}}})],buses:[bus("send",[]),bus("other",[])],tail:0})"#;

#[test]
fn malformed_clock_annotations_are_diagnostics() {
    for payload in [
        "{clock_start:0}",
        "{clock_start:0,clock_duration:1}",
        "{clock_start:0,clock_duration:null,clock_span:1}",
        "{clock_start:0,clock_duration:1,clock_span:-1}",
        "{clock_start:-1,clock_duration:1,clock_span:1}",
    ] {
        let source = format!(
            "song({{tracks:[track(\"x\",note(60).annotate(\"all\",{payload}),synth(\"init\"))]}})"
        );
        let error = compile(&source).unwrap_err().to_string();
        assert!(error.contains("clock"), "{error}");
        assert!(error.contains("<source>:1:"), "{error}");
    }
}

#[test]
fn every_route_payload_field_reconciles_and_pre_updates_live_output() {
    let old = compile(SOURCE).unwrap();
    for field in ["pre", "to", "gain"] {
        let mut next = old.clone();
        let route = &mut next.tracks[0].sends[0];
        match field {
            "pre" => route.pre = true,
            "to" => route.to = muz::model::Id::new("other"),
            _ => route.gain_db = -3.,
        }
        let route = route.clone();
        let plan = muz::plan_reconciliation(0, &old, &next).unwrap();
        assert!(
            plan.operations
                .iter()
                .any(|op| matches!(op,ReconcileOperation::UpdateRoute{route:r} if r==&route))
        );
        let mut live = AudioEngine::new(&old, config()).unwrap();
        sample(&mut live);
        let token = live.device_debug_states()[0].1.instance_token;
        let mut tx =
            PreparedTransaction::prepare(&old, &next, &plan, 1, Instant::now(), config()).unwrap();
        live.apply_transaction(&mut tx).unwrap();
        assert_eq!(live.device_debug_states()[0].1.instance_token, token);
        assert_eq!(
            sample(&mut live),
            sample(&mut AudioEngine::new(&next, config()).unwrap())
        );
    }
    let mut next = old.clone();
    next.tracks[0].sends[0].id = muz::model::Id::new("new-send");
    let plan = muz::plan_reconciliation(0, &old, &next).unwrap();
    assert!(
        plan.operations
            .iter()
            .any(|op| matches!(op, ReconcileOperation::Remove { .. }))
    );
    assert!(
        plan.operations
            .iter()
            .any(|op| matches!(op, ReconcileOperation::Add { .. }))
    );
}
