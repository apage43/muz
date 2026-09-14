use muz::{audio::*, lang::Evaluator, model::TrackSource};
fn session(notes: usize, at: u32) -> muz::Session {
    let source = format!(
        r#"song({{tracks:[track("x",stack(map(range({notes}),fn(i)=>note(60,100b,at={at}b))),voice_patch("p",{{nodes:[{{id:"a",op:"param",value:0.1}}],output:"a"}}))]}})"#
    );
    let v = Evaluator::new().source(&source).unwrap();
    muz::compile::lower(
        v.get("__result").unwrap().clone(),
        std::path::Path::new("capacity.muz"),
        vec![],
    )
    .unwrap()
    .session
}
fn config() -> AudioConfig {
    AudioConfig {
        sample_rate: 48000.,
        max_frames: 256,
        offline: true,
    }
}
#[test]
fn overfull_schedules_reject_before_playback() {
    let s = session(130, 0);
    assert!(
        AudioEngine::new(&s, config())
            .unwrap_err_message()
            .contains("callback events")
    );
}
trait ErrorMessage {
    fn unwrap_err_message(self) -> String;
}
impl ErrorMessage for Result<AudioEngine, EngineError> {
    fn unwrap_err_message(self) -> String {
        match self {
            Err(e) => e.to_string(),
            Ok(_) => panic!("schedule accepted"),
        }
    }
}
#[test]
fn cutover_checks_accumulated_release_obligations_before_mutation() {
    let old = session(80, 0);
    let next = session(80, 1);
    let third = session(80, 4);
    let mut engine = AudioEngine::new(&old, config()).unwrap();
    engine.set_running(true);
    let mut pcm = [0.; 512];
    engine.render_interleaved(&mut pcm, 2).unwrap();
    let plan = muz::plan_reconciliation(0, &old, &next).unwrap();
    let mut tx =
        PreparedTransaction::prepare(&old, &next, &plan, 1, std::time::Instant::now(), config())
            .unwrap();
    engine.apply_transaction(&mut tx).unwrap();
    for _ in 0..150 {
        engine.render_interleaved(&mut pcm, 2).unwrap();
    }
    let plan = muz::plan_reconciliation(1, &next, &third).unwrap();
    let mut tx =
        PreparedTransaction::prepare(&next, &third, &plan, 2, std::time::Instant::now(), config())
            .unwrap();
    assert!(engine.apply_transaction(&mut tx).is_err());
    assert_eq!(engine.revision(), 1);
    engine.render_interleaved(&mut pcm, 2).unwrap();
}
#[test]
fn seek_restores_latest_state_from_long_histories() {
    let mut s = session(1, 0);
    let TrackSource::Midi(m) = &mut s.tracks[0].source else {
        panic!()
    };
    m.imported.controllers = (0..10000)
        .map(|i| muz::midi::MidiController {
            tick: i * 960000,
            channel: 0,
            controller: 7,
            value: (i % 128) as u8,
            source_order: i as u32,
        })
        .collect();
    m.imported.summary.controllers = 10000;
    m.imported.summary.end_tick = 10001 * 960000;
    m.summary = m.imported.summary.clone();
    let mut engine = AudioEngine::new(&s, config()).unwrap();
    engine.set_running(true);
    engine.seek_ticks(9999 * 960000 + 480000);
    let mut pcm = [0.; 512];
    engine.render_interleaved(&mut pcm, 2).unwrap();
    assert_eq!(engine.status().delivered_events.controllers, 1);
}
