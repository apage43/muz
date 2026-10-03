#![cfg(feature = "desktop")]

use muz::{host::HostContext, native::{ControlTicket, NativeEvent, NativeRuntime, ObservationFrame, ObservationSelection, ObservationTarget}};
use std::time::{Duration, Instant};

fn acknowledge(runtime: &mut NativeRuntime, ticket: ControlTicket) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut events = [None];
        runtime.poll(&mut events);
        if let Some(event) = events[0].take() {
            match event {
                NativeEvent::ControlApplied { ticket: actual, .. } => { assert_eq!(actual, ticket); return; }
                other => panic!("native qualification outcome: {other:?}"),
            }
        }
        assert!(Instant::now() < deadline, "actual native callback receipt timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn capture(runtime: &mut NativeRuntime, duration: Duration) -> Vec<(ObservationFrame, Instant)> {
    let deadline = Instant::now() + duration;
    let mut captured = Vec::new();
    while Instant::now() < deadline {
        let mut events = [None];
        runtime.poll(&mut events);
        assert!(events[0].is_none(), "unexpected native event: {:?}", events[0]);
        let mut frames = [None];
        runtime.drain_observations(&mut frames);
        if let Some(frame) = frames[0].take() { captured.push((frame, Instant::now())); }
        std::thread::sleep(Duration::from_millis(1));
    }
    captured
}

#[test]
#[ignore = "requires actual native output; explicitly run with muted physical output"]
fn actual_native_muted_output_qualifies_contiguous_window_boundaries() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native-observation.muz");
    std::fs::write(&path, "song({tempo:120,tracks:[track(\"tone\",note(69,32b).gate(1),synth(\"pad\"))],tail:0})").unwrap();
    let context = HostContext::default();
    let compiled = muz::compile::compile_with_context(&path, std::rc::Rc::new(muz::lang::FileSourceLoader), &context).unwrap();
    let mut runtime = NativeRuntime::start(&compiled.session, &context, false).unwrap();
    let silence = runtime.set_audibility(0, &[]).unwrap();
    acknowledge(&mut runtime, silence);
    let selection = ObservationSelection {
        meter_tracks: vec!["tone".into()], master_meter: true,
        analysis: Some(ObservationTarget::Track("tone".into())), meter_hz: 20, analysis_hz: 30,
    };
    let observe = runtime.set_observation(0, &selection).unwrap();
    acknowledge(&mut runtime, observe);
    let play = runtime.set_running(0, true).unwrap();
    acknowledge(&mut runtime, play);
    let initial = runtime.status();
    println!("native-environment audio={:?} config={:?} stream_starts={} generation={}", initial.audio, runtime.audio_config(), initial.stream_start_count, initial.transport.generation);
    let frames: Vec<_> = capture(&mut runtime, Duration::from_secs(3)).into_iter()
        .filter(|(frame, _)| frame.sample_count != 0).collect();
    assert!(frames.len() > 45, "30Hz candidate delivered only {} frames", frames.len());
    let epoch = frames[0].0.capture_epoch;
    let generation = frames[0].0.transport_generation;
    let mut arrivals = Vec::new();
    let mut sample_hops = Vec::new();
    for (index, (frame, arrived)) in frames.iter().enumerate() {
        assert_eq!(frame.revision, 0);
        assert_eq!(frame.transport_generation, generation);
        assert_eq!(frame.capture_epoch, epoch);
        assert_eq!(frame.sample_count, 2048);
        assert_eq!(frame.mono_samples.len(), 2048);
        assert_eq!(frame.sample_rate, runtime.audio_config().sample_rate);
        assert!(frame.sample_position >= 2048);
        assert_eq!(frame.target, Some(ObservationTarget::Track("tone".into())));
        assert!(frame.mono_samples.iter().all(|sample| sample.is_finite()));
        assert!(frame.mono_samples.iter().any(|sample| sample.abs() > 0.0001));
        if let Some(master) = frame.master { assert_eq!(master, [0.0, 0.0]); }
        if index != 0 {
            assert!(frame.sequence > frames[index - 1].0.sequence);
            assert!(frame.sample_position > frames[index - 1].0.sample_position);
            arrivals.push(arrived.duration_since(frames[index - 1].1).as_secs_f64() * 1000.0);
            sample_hops.push(frame.sample_position - frames[index - 1].0.sample_position);
        }
    }
    arrivals.sort_by(f64::total_cmp);
    sample_hops.sort_unstable();
    let paused = runtime.set_running(0, false).unwrap();
    acknowledge(&mut runtime, paused);
    let paused_frames = capture(&mut runtime, Duration::from_millis(150));
    assert!(paused_frames.iter().all(|(frame, _)| frame.sample_count == 0 && frame.target.is_none()));
    let seek = runtime.seek_ticks(&compiled.session, 0, 0, &context).unwrap();
    acknowledge(&mut runtime, seek);
    let resumed = runtime.set_running(0, true).unwrap();
    acknowledge(&mut runtime, resumed);
    let restored = capture(&mut runtime, Duration::from_millis(300));
    let restored_detail = restored.iter().find(|(frame, _)| frame.sample_count == 2048).expect("resume never warmed detail");
    assert!(restored_detail.0.capture_epoch > epoch);
    assert!(restored_detail.0.transport_generation > generation);
    let final_status = runtime.status();
    assert_eq!(final_status.stream_start_count, initial.stream_start_count);
    assert_eq!(final_status.audio.render_faults, initial.audio.render_faults);
    assert_eq!(final_status.audio.stream_errors, initial.audio.stream_errors);
    assert_eq!(final_status.audio.transaction_faults, initial.audio.transaction_faults);
    println!("native-capture frames={} sample_count=2048 sample_rate={} window_ms={:.6} arrival_p50_ms={:.6} arrival_p95_ms={:.6} hop_p50_samples={} hop_p95_samples={} dropped={} callback_delta={} output_peak={} faults_render={} faults_stream={} faults_transaction={} stream_starts={}",
        frames.len(), frames[0].0.sample_rate, 2048.0 / f64::from(frames[0].0.sample_rate) * 1000.0,
        arrivals[arrivals.len() / 2], arrivals[(arrivals.len() * 95 / 100).min(arrivals.len() - 1)],
        sample_hops[sample_hops.len() / 2], sample_hops[(sample_hops.len() * 95 / 100).min(sample_hops.len() - 1)],
        final_status.observation_dropped, final_status.audio.callback_count - initial.audio.callback_count,
        final_status.last_peak, final_status.audio.render_faults, final_status.audio.stream_errors,
        final_status.audio.transaction_faults, final_status.stream_start_count);
    assert!(runtime.shutdown().error.is_none());
}
