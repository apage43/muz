use muz::{
    audio::{AudioConfig, AudioEngine},
    lang::Evaluator,
};
use std::path::Path;

fn engine() -> AudioEngine {
    let source = r#"song({tracks:[track("lead",note(60,16b).gate(1),synth("pad"),{sends:{pre:{gain:0,pre:true},post:{gain:0,pre:false}}})],buses:[bus("pre",[]),bus("post",[])],tail:0})"#;
    let value = Evaluator::new().source(source).unwrap();
    let session = muz::compile::lower(
        value.get("__result").unwrap().clone(),
        Path::new("audition.muz"),
        vec![],
    )
    .unwrap()
    .session;
    let mut engine = AudioEngine::new(
        &session,
        AudioConfig {
            sample_rate: 48000.,
            max_frames: 1024,
            offline: true,
        },
    )
    .unwrap();
    engine.set_running(true);
    engine
}

#[test]
fn mask_silences_output_and_all_sends_without_stopping_voices_or_transport() {
    let mut muted = engine();
    let mut baseline = engine();
    let mut a = [0.; 2048];
    let mut b = [0.; 2048];
    for _ in 0..3 {
        muted.render_interleaved(&mut a, 2).unwrap();
        baseline.render_interleaved(&mut b, 2).unwrap();
    }
    assert!(a.iter().any(|v| v.abs() > 0.0001));
    let revision = muted.revision();
    let tick = muted.status().current_tick;
    muted.set_track_audibility(&[], true).unwrap();
    muted.render_interleaved(&mut a, 2).unwrap();
    baseline.render_interleaved(&mut b, 2).unwrap();
    assert!(
        a[..100].iter().any(|v| v.abs() > 0.0001),
        "short ramp, not an abrupt cut"
    );
    assert!(
        a[500..].iter().all(|v| *v == 0.),
        "no pre/post-send leakage after ramp"
    );
    assert!(muted.status().current_tick > tick);
    assert_eq!(muted.revision(), revision);
    assert!(
        muted
            .set_track_audibility(&["unknown".into()], false)
            .is_err()
    );
    muted.render_interleaved(&mut a, 2).unwrap();
    baseline.render_interleaved(&mut b, 2).unwrap();
    assert!(a.iter().all(|v| *v == 0.));
    muted.set_track_audibility(&["lead".into()], true).unwrap();
    muted.render_interleaved(&mut a, 2).unwrap();
    baseline.render_interleaved(&mut b, 2).unwrap();
    assert_eq!(
        &a[500..],
        &b[500..],
        "held voice advanced identically while inaudible"
    );
}
