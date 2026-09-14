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
fn tempo_revision_keeps_held_voice_and_original_release() {
    let old = compile(SOURCE).unwrap();
    let mut next = old.clone();
    let muz::model::TrackSource::Midi(m) = &mut next.tracks[0].source else {
        panic!()
    };
    for tempo in &mut m.imported.tempos {
        tempo.micros_per_quarter = 600_000;
    }
    let mut engine = AudioEngine::new(&old, config()).unwrap();
    let before = sample(&mut engine);
    let plan = muz::plan_reconciliation(0, &old, &next).unwrap();
    let mut tx =
        PreparedTransaction::prepare(&old, &next, &plan, 1, Instant::now(), config()).unwrap();
    engine.apply_transaction(&mut tx).unwrap();
    assert_eq!(sample(&mut engine), before);
    assert_eq!(engine.status().delivered_events.note_ons, 1);
    let mut pcm = [0.; 512];
    for _ in 0..400 {
        engine.render_interleaved(&mut pcm, 2).unwrap();
    }
    assert_eq!(engine.status().delivered_events.note_offs, 1);
    assert!(engine.apply_transaction(&mut tx).is_err());
}

#[test]
fn stale_revision_and_wrong_configuration_leave_engine_unchanged() {
    let old = compile(SOURCE).unwrap();
    let plan = muz::plan_reconciliation(2, &old, &old).unwrap();
    let mut tx =
        PreparedTransaction::prepare(&old, &old, &plan, 3, Instant::now(), config()).unwrap();
    let mut engine = AudioEngine::new(&old, config()).unwrap();
    assert!(engine.apply_transaction(&mut tx).is_err());
    assert_eq!(engine.revision(), 0);
    let plan = muz::plan_reconciliation(0, &old, &old).unwrap();
    let mut other = config();
    other.sample_rate = 44100.;
    let mut tx = PreparedTransaction::prepare(&old, &old, &plan, 1, Instant::now(), other).unwrap();
    assert!(engine.apply_transaction(&mut tx).is_err());
    assert_eq!(engine.revision(), 0);
}

#[test]
fn clock_named_annotations_are_ordinary_metadata() {
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
        let session = compile(&source).unwrap();
        let muz::model::TrackSource::Midi(m) = &session.tracks[0].source else {
            panic!()
        };
        assert_eq!(m.imported.notes[0].start_tick, 0);
        assert!(m.imported.notes[0].annotations.contains_key("clock_start"));
    }
}

#[test]
fn clock_payload_is_validated_and_score_transforms_are_explicit() {
    use muz::music::{ClockPlacement, Note, Pattern, b};
    for (a, d, s) in [
        (0., 0., 1.),
        (-1., 1., 1.),
        (0., 1., 0.5),
        (f64::INFINITY, 1., 1.),
    ] {
        assert!(ClockPlacement::new(a, d, s).is_err());
    }
    let mut n = Note::new(b(0), b(1), 60., "clock".into());
    n.clock = Some(ClockPlacement::new(0.1, 0.04, 0.05).unwrap());
    let p = Pattern {
        notes: vec![n],
        span: b(1),
        ..Default::default()
    };
    assert!(p.require_score_time("stretch").is_err());
    let shifted = p.shifted(b(4), "occurrence").unwrap();
    assert_eq!(shifted.notes[0].clock, p.notes[0].clock);
    let mut raw = serde_json::to_value(&p).unwrap();
    raw["notes"][0]["clock"]["duration_seconds"] = serde_json::json!(-1);
    assert!(
        serde_json::from_value::<Pattern>(raw)
            .unwrap()
            .validate()
            .is_err()
    );
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
