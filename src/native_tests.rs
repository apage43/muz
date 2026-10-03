use super::*;
use std::time::{Duration, Instant};

fn session() -> Session {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.muz");
    std::fs::write(&path, r#"song({title:"accepted",tracks:[track("a",phrase("C4:w"),synth("pad")),track("b",phrase("E4:w"),synth("pad"))]})"#).unwrap();
    crate::compile::compile(&path).unwrap().session
}
fn runtime(session: &Session) -> NativeRuntime {
    NativeRuntime::start_backend(session, &HostContext::default(), false, true).unwrap()
}
fn event(runtime: &mut NativeRuntime) -> NativeEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut slots = [None];
        if runtime.poll(&mut slots).written == 1 { return slots[0].take().unwrap(); }
        assert!(Instant::now() < deadline, "native headless outcome deadline");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn applied_control(runtime: &mut NativeRuntime, ticket: ControlTicket) -> NativeTransportStatus {
    match event(runtime) {
        NativeEvent::ControlApplied { ticket: actual, transport, .. } => { assert_eq!(actual, ticket); transport }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[test]
fn frozen_inherent_signatures_are_consumable() {
    let _: fn(&Session, &HostContext, bool) -> Result<NativeRuntime, NativeError> = NativeRuntime::start;
    let _: fn(&NativeRuntime) -> AudioConfig = NativeRuntime::audio_config;
    let _: fn(&NativeRuntime) -> NativeStatus = NativeRuntime::status;
    let _: fn(&NativeRuntime) -> PluginStamp = NativeRuntime::plugin_stamp;
    let _: fn(&PluginStamp, &mut Session) = PluginStamp::apply;
    let _: fn(&PreparedNativeRevision) -> RevisionTicket = PreparedNativeRevision::ticket;
    let _: fn(&mut NativeRuntime, &Session, &Session, u64, u64, &HostContext) -> Result<PreparedNativeRevision, NativeError> = NativeRuntime::prepare_revision;
    let _: fn(&mut NativeRuntime, PreparedNativeRevision) -> Result<RevisionTicket, SubmitRevisionError> = NativeRuntime::submit_revision;
    let _: fn(&mut NativeRuntime, NativeOperationId) -> Result<CancelDisposition, NativeError> = NativeRuntime::cancel_operation;
    let _: fn(&mut NativeRuntime, &mut [Option<NativeEvent>]) -> DrainCount = NativeRuntime::poll;
    let _: fn(&mut NativeRuntime, &Session, u64) -> Result<(), NativeError> = NativeRuntime::service_plugins;
    let _: fn(&mut NativeRuntime, &Session, u64, u64, &PluginRestartRequest, &HostContext) -> Result<PreparedNativeRevision, NativeError> = NativeRuntime::prepare_plugin_restart;
    let _: fn(&mut NativeRuntime, u64, bool) -> Result<ControlTicket, NativeError> = NativeRuntime::set_running;
    let _: fn(&mut NativeRuntime, &Session, u64, &HostContext) -> Result<ControlTicket, NativeError> = NativeRuntime::restart;
    let _: fn(&mut NativeRuntime, u64) -> Result<ControlTicket, NativeError> = NativeRuntime::panic;
    let _: fn(&mut NativeRuntime, &Session, u64, u64, &HostContext) -> Result<ControlTicket, NativeError> = NativeRuntime::seek_ticks;
    let _: fn(&mut NativeRuntime, &Session, u64, Option<(u64, u64)>, &HostContext) -> Result<ControlTicket, NativeError> = NativeRuntime::set_loop;
    let _: fn(&mut NativeRuntime, u64, &[String]) -> Result<ControlTicket, NativeError> = NativeRuntime::set_audibility;
    let _: fn(&mut NativeRuntime, u64, &ObservationSelection) -> Result<ControlTicket, NativeError> = NativeRuntime::set_observation;
    let _: fn(&mut NativeRuntime, &mut [Option<ObservationFrame>]) -> DrainCount = NativeRuntime::drain_observations;
    let _: fn(NativeRuntime) -> ShutdownReport = NativeRuntime::shutdown;
}

#[test]
fn accepted_session_is_borrowed_and_only_receipts_advance_revisions() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let context = HostContext::default();
    let mut candidate = accepted.clone();
    candidate.extras.title = "owned by host".into();
    let prepared = runtime.prepare_revision(&accepted, &candidate, 0, 41, &context).unwrap();
    assert_eq!(runtime.status().accepted_revision, 0);
    let ticket = runtime.submit_revision(prepared).unwrap();
    assert_eq!(ticket.source_generation, 41);
    assert_eq!(accepted.extras.title, "accepted");
    match event(&mut runtime) {
        NativeEvent::RevisionApplied { ticket: actual, plugin_stamp, .. } => { assert_eq!(actual, ticket); plugin_stamp.apply(&mut candidate); }
        other => panic!("unexpected outcome: {other:?}"),
    }
    assert_eq!(runtime.status().accepted_revision, 1);
    assert_eq!(runtime.status().source_generation, 41);
    assert_eq!(runtime.service_plugins(&accepted, 1).unwrap_err().code, NativeErrorCode::SessionMismatch);
    runtime.service_plugins(&candidate, 1).unwrap();
    assert_eq!(runtime.cancel_operation(ticket.operation).unwrap(), CancelDisposition::AlreadyResolved);
    assert!(runtime.shutdown().error.is_none());
}

#[test]
fn supersession_cancellation_and_cross_runtime_refusal_return_prepared_ownership() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let mut other = NativeRuntime::start_backend(&accepted, &HostContext::default(), false, true).unwrap();
    let context = HostContext::default();
    let first = runtime.prepare_revision(&accepted, &accepted, 0, 1, &context).unwrap();
    let first_id = first.ticket().operation;
    let second = runtime.prepare_revision(&accepted, &accepted, 0, 2, &context).unwrap();
    let refused = runtime.submit_revision(first).unwrap_err();
    assert_eq!(refused.error.code, NativeErrorCode::Superseded);
    assert_eq!(refused.prepared.ticket().operation, first_id);
    let refused = other.submit_revision(second).unwrap_err();
    assert_eq!(refused.error.code, NativeErrorCode::InvalidRequest);
    let second = refused.prepared;
    assert_eq!(runtime.cancel_operation(second.ticket().operation).unwrap(), CancelDisposition::Prevented);
    let refused = runtime.submit_revision(second).unwrap_err();
    assert_eq!(refused.error.code, NativeErrorCode::Cancelled);
    assert_eq!(runtime.status().accepted_revision, 0);
    assert!(!context.is_cancelled(), "runtime cancellation must not poison accepted/export context");
    assert_eq!(runtime.cancel_operation(NativeOperationId(u64::MAX)).unwrap_err().code, NativeErrorCode::UnknownOperation);
}

#[test]
fn failed_prepare_and_scoped_cancellation_preserve_baseline() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let context = HostContext::default();
    context.cancelled.store(true, Ordering::Release);
    assert_eq!(runtime.prepare_revision(&accepted, &accepted, 0, 1, &context).unwrap_err().code, NativeErrorCode::Cancelled);
    let fresh = HostContext::default();
    let prepared = runtime.prepare_revision(&accepted, &accepted, 0, 1, &fresh).unwrap();
    drop(prepared);
    let mut mismatch = accepted.clone();
    mismatch.tracks[0].name = "not accepted".into();
    assert_eq!(runtime.prepare_revision(&mismatch, &mismatch, 0, 1, &fresh).unwrap_err().code, NativeErrorCode::SessionMismatch);
    assert_eq!(runtime.prepare_revision(&accepted, &accepted, 1, 1, &fresh).unwrap_err().code, NativeErrorCode::StaleRevision);
    assert_eq!(runtime.status().accepted_revision, 0);
}

#[test]
fn prepare_revision_retains_structured_automation_diagnostic() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("candidate.muz");
    std::fs::write(&path, concat!(
        "song({title:\"Ω𝄞\", tracks:[track(\"a\",phrase(\"C4:w\"),synth(\"pad\")),track(\"b\",phrase(\"E4:w\"),synth(\"pad\"))],\n",
        " automation:[automation(\"a.missing\",curve([[0b,-6]]))]})"
    )).unwrap();
    let mut candidate = crate::compile::compile(&path).unwrap().session;
    runtime.plugin_stamp().apply(&mut candidate);
    let error = runtime.prepare_revision(
        &accepted, &candidate, 0, 1, &HostContext::default(),
    ).unwrap_err();
    assert_eq!(error.code, NativeErrorCode::Prepare);
    let display = error.to_string();
    assert_eq!(display, error.message);
    let error = anyhow::Error::new(error).context("native preparation");
    let diagnostic = error.chain()
        .find_map(|cause| cause.downcast_ref::<crate::diagnostic::Diagnostic>())
        .expect("native preparation retains the engine diagnostic source");
    assert_eq!(display, diagnostic.to_string());
    let structured = diagnostic.to_json();
    assert_eq!(structured["message"], "automation target 'a.missing' is unknown");
    assert_eq!(structured["location"]["path"], path.to_string_lossy().as_ref());
    assert_eq!(structured["location"]["line"], 2);
    assert_eq!(structured["location"]["column"], 14);
    assert!(structured["help"].as_array().unwrap().iter().any(|help| {
        help.as_str() == Some("targets name a route (`<track>.out` or `<track>.send.<bus>`) or a device parameter (`<device>.<parameter>`)")
    }), "{structured}");
    assert_eq!(runtime.status().accepted_revision, 0);
    assert_eq!(runtime.status().source_generation, 0);
    assert!(runtime.shutdown().error.is_none());
}

#[test]
fn bounded_reliable_consumers_do_not_lose_or_overwrite_control_outcomes() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let tickets: Vec<_> = (0..64).map(|_| runtime.set_running(0, false).unwrap()).collect();
    assert_eq!(runtime.set_running(0, true).unwrap_err().code, NativeErrorCode::QueueFull);
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.controls.len() != 0 {
        assert_eq!(runtime.poll(&mut []).written, 0);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(runtime.set_running(0, true).unwrap_err().code, NativeErrorCode::QueueFull);
    let sentinel = NativeEvent::OutputFault(NativeError::new(NativeErrorCode::OutputFault, "sentinel"));
    let mut full = [Some(sentinel)];
    assert_eq!(runtime.poll(&mut full), DrainCount { written: 0, remaining: true });
    assert!(matches!(&full[0], Some(NativeEvent::OutputFault(e)) if e.message == "sentinel"));
    for ticket in tickets { applied_control(&mut runtime, ticket); }
    assert_eq!(runtime.poll(&mut []).remaining, false);
    let ticket = runtime.set_running(0, false).unwrap();
    applied_control(&mut runtime, ticket);
    assert_eq!(runtime.cancel_operation(NativeOperationId(1)).unwrap_err().code, NativeErrorCode::UnknownOperation);
}

#[test]
fn policy_commands_validate_exact_ids_and_invalidate_old_preparation() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let context = HostContext::default();
    let prepared = runtime.prepare_revision(&accepted, &accepted, 0, 7, &context).unwrap();
    let id = accepted.tracks[0].id.to_string();
    assert_eq!(runtime.set_audibility(0, &["missing".into()]).unwrap_err().code, NativeErrorCode::UnknownTrack);
    assert_eq!(runtime.set_audibility(0, &[id.clone(), id.clone()]).unwrap_err().code, NativeErrorCode::InvalidRequest);
    let ticket = runtime.set_audibility(0, &[]).unwrap();
    assert_eq!(runtime.submit_revision(prepared).unwrap_err().error.code, NativeErrorCode::Superseded);
    applied_control(&mut runtime, ticket);
    assert_eq!(runtime.audibility, Some(Vec::new()));
    let too_fast = ObservationSelection { meter_tracks: vec![id], master_meter: true, analysis: Some(ObservationTarget::Master), meter_hz: 21, analysis_hz: 10 };
    assert_eq!(runtime.set_observation(0, &too_fast).unwrap_err().code, NativeErrorCode::InvalidRequest);
    assert_eq!(runtime.set_audibility(1, &[]).unwrap_err().code, NativeErrorCode::StaleRevision);
}

#[test]
fn shutdown_resolves_every_admitted_operation_once() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let controls: Vec<_> = (0..10).map(|_| runtime.set_running(0, false).unwrap().operation).collect();
    let report = runtime.shutdown();
    let mut actual = Vec::new();
    for event in report.events {
        match event {
            NativeEvent::ControlApplied { ticket, .. } | NativeEvent::ControlRejected { ticket, .. } => actual.push(ticket.operation),
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
    assert_eq!(actual, controls);
    assert!(report.error.is_none());
}

#[test]
fn queue_full_returns_the_same_expensive_preparation_for_retry() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let context = HostContext::default();
    let prepared = runtime.prepare_revision(&accepted, &accepted, 0, 9, &context).unwrap();
    let ticket = prepared.ticket();
    // Occupy the low-level revision slot with a deterministically cancelled
    // operation. This exercises ownership return independently of worker timing.
    let plan = crate::plan_reconciliation(0, &accepted, &accepted).unwrap();
    let transaction = PreparedTransaction::prepare(&accepted, &accepted, &plan, 0, Instant::now(), runtime.audio_config()).unwrap();
    let guard = OperationGuard::new();
    assert_eq!(guard.cancel(), CancelDisposition::Prevented);
    let occupied = OutputOperation {
        guard,
        payload: OutputPayload::Revision {
            ticket: RevisionTicket { operation: NativeOperationId(999), base_revision: 0, revision: 1, source_generation: 0 },
            transaction: Box::new(transaction),
        },
        observation: None,
    };
    assert!(runtime.output.submit_operation(occupied).is_ok());
    let refusal = runtime.submit_revision(prepared).unwrap_err();
    assert_eq!(refusal.error.code, NativeErrorCode::QueueFull);
    assert_eq!(refusal.prepared.ticket(), ticket);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(receipt) = runtime.output.poll_operation_receipt() {
            assert_eq!(receipt.result, Err(NativeErrorCode::Cancelled));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(runtime.submit_revision(refusal.prepared).unwrap(), ticket);
    assert!(matches!(event(&mut runtime), NativeEvent::RevisionApplied { ticket: actual, .. } if actual == ticket));
}

#[test]
fn plugin_stamp_and_overflow_refusals_do_not_publish_candidates() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let context = HostContext::default();
    let mut candidate = accepted.clone();
    candidate.tracks[0].instrument.kind = DeviceKind::Clap;
    candidate.tracks[0].instrument.generation = 1;
    assert_eq!(runtime.prepare_revision(&accepted, &candidate, 0, 0, &context).unwrap_err().code, NativeErrorCode::SessionMismatch);
    let request = PluginRestartRequest { expected_revision: 0, stamp: PluginStamp { generation: 1 }, instance_tokens: vec![] };
    assert_eq!(runtime.prepare_plugin_restart(&accepted, 0, 0, &request, &context).unwrap_err().code, NativeErrorCode::InvalidRequest);
    runtime.next_operation = u64::MAX;
    assert_eq!(runtime.set_running(0, true).unwrap_err().code, NativeErrorCode::RevisionOverflow);
    assert_eq!(runtime.status().accepted_revision, 0);
    assert!(!runtime.status().transport.snapshot.running);
}

#[test]
fn paused_observation_has_no_fabricated_waveform_and_does_not_seek() {
    let accepted = session();
    let mut runtime = runtime(&accepted);
    let selection = ObservationSelection {
        meter_tracks: accepted.tracks.iter().map(|t| t.id.to_string()).collect(),
        master_meter: true, analysis: Some(ObservationTarget::Master), meter_hz: 20, analysis_hz: 10,
    };
    let before = runtime.status().transport;
    let ticket = runtime.set_observation(0, &selection).unwrap();
    let after = applied_control(&mut runtime, ticket);
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.snapshot.current_tick, before.snapshot.current_tick);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut slots = [None];
        if runtime.drain_observations(&mut slots).written != 0 {
            let frame = slots[0].take().unwrap();
            assert!(!frame.running);
            assert!(frame.mono_samples.is_empty());
            assert!(frame.tracks.iter().all(|track| track.peak == 0.0 && track.rms == 0.0));
            assert_eq!(frame.revision, 0);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn sfz_transport_replay_applies_mask_before_routed_delay_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut wave = hound::WavWriter::create(dir.path().join("held.wav"), hound::WavSpec {
        channels: 1, sample_rate: 48_000, bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    }).unwrap();
    for frame in 0..48_000 {
        wave.write_sample(0.25 * (frame as f32 * 0.043).sin()).unwrap();
    }
    wave.finalize().unwrap();
    std::fs::write(dir.path().join("held.sfz"),
        "<region> sample=held.wav key=60 ampeg_attack=0 ampeg_release=0.1").unwrap();
    let path = dir.path().join("native.muz");
    std::fs::write(&path, r#"song({tempo:120,tracks:[
        track("held",note(60,2b),sfz("held.sfz"),{sends:{echo:0}})
    ],buses:[bus("echo",[fx("delay",{time_ms:100,mix:1,feedback:0.5})])]})"#).unwrap();
    let accepted = crate::compile::compile(&path).unwrap().session;
    let mut runtime = runtime(&accepted);
    let context = HostContext::default();
    let ticket = runtime.set_audibility(0, &[]).unwrap();
    applied_control(&mut runtime, ticket);
    for range in [None, Some((720_000, 960_000))] {
        let mut replayed = context.run(|| runtime.prepare_sfz_transport(
            &accepted, 840_000, range, &context,
        )).unwrap();
        let mut pcm = [0.0; 512];
        for _ in 0..40 {
            replayed.render_interleaved(&mut pcm, 2).unwrap();
            assert_eq!(pcm, [0.0; 512]);
        }
    }
    let ticket = runtime.seek_ticks(&accepted, 0, 840_000, &context).unwrap();
    applied_control(&mut runtime, ticket);
    let ticket = runtime.set_loop(&accepted, 0, Some((720_000, 960_000)), &context).unwrap();
    applied_control(&mut runtime, ticket);
    let ticket = runtime.restart(&accepted, 0, &context).unwrap();
    applied_control(&mut runtime, ticket);
    assert_eq!(runtime.status().accepted_revision, 0);
    assert!(runtime.shutdown().error.is_none());
}

fn deterministic_runtime(session: &Session) -> (NativeRuntime, crate::audio::pipewire::CallbackProcessor) {
    let mut runtime = runtime(session);
    let (output, callback) = crate::audio::pipewire::tests::harness(session);
    runtime.output = output;
    (runtime, callback)
}

fn assert_all_controls_busy(runtime: &mut NativeRuntime, accepted: &Session, revision: u64) {
    let context = HostContext::default();
    let selection = ObservationSelection {
        meter_tracks: Vec::new(), master_meter: true, analysis: Some(ObservationTarget::Master),
        meter_hz: 20, analysis_hz: MAX_ANALYSIS_HZ,
    };
    let policy_generation = runtime.policy_generation;
    let operation = runtime.next_operation;
    for result in [
        runtime.set_running(revision, false), runtime.panic(revision),
        runtime.restart(accepted, revision, &context),
        runtime.seek_ticks(accepted, revision, 0, &context),
        runtime.set_loop(accepted, revision, None, &context),
        runtime.set_audibility(revision, &[]),
        runtime.set_observation(revision, &selection),
    ] {
        assert_eq!(result.unwrap_err().code, NativeErrorCode::Busy);
    }
    let restart = PluginRestartRequest {
        expected_revision: revision, stamp: PluginStamp { generation: 1 }, instance_tokens: vec![1],
    };
    assert_eq!(runtime.prepare_plugin_restart(accepted, revision, 0, &restart, &context).unwrap_err().code,
        NativeErrorCode::Busy);
    assert_eq!(runtime.next_operation, operation);
    assert_eq!(runtime.policy_generation, policy_generation);
    assert_eq!(runtime.control_reservations, 0);
}

#[test]
fn every_control_refuses_through_submission_fade_and_undelivered_revision_receipt() {
    use crate::audio::observation::tests::allocation_activity;
    for structural in [false, true] {
        let mut accepted = session();
        accepted.tracks[0].instrument.kind = crate::model::DeviceKind::PolySynth;
        accepted.tracks[0].instrument.params.clear();
        accepted.tracks[0].instrument.params.insert("gain_db".into(), -12.0);
        let (mut runtime, mut callback) = deterministic_runtime(&accepted);
        let stream_starts = runtime.status().stream_start_count;
        let mut candidate = accepted.clone();
        if structural { candidate.tracks.pop(); } else {
            candidate.tracks[0].instrument.params.insert("gain_db".into(), -18.0);
        }
        let prepared = runtime.prepare_revision(&accepted, &candidate, 0, 7, &HostContext::default()).unwrap();
        let ticket = runtime.submit_revision(prepared).unwrap();
        assert_all_controls_busy(&mut runtime, &accepted, 0);
        let (_, allocations, frees) = allocation_activity(|| callback.process(&mut [0.0; 512], 2));
        assert_eq!((allocations, frees), (0, 0));
        if structural {
            assert!(runtime.status().audio.structural_transition_active);
            assert_all_controls_busy(&mut runtime, &accepted, 0);
            assert_eq!(runtime.cancel_operation(ticket.operation).unwrap(), CancelDisposition::PendingOutcome);
            let (_, allocations, frees) = allocation_activity(|| callback.process(&mut [0.0; 512], 2));
            assert_eq!((allocations, frees), (0, 0));
        }
        // Collection retires ownership and advances native metadata, but cannot
        // release admission while the caller has not received its terminal event.
        assert_eq!(runtime.poll(&mut []).written, 0);
        assert_all_controls_busy(&mut runtime, &accepted, 0);
        assert!(runtime.pending_revision.is_none());
        assert_eq!(runtime.status().accepted_revision, 1);
        assert_all_controls_busy(&mut runtime, &candidate, 1);
        let mut full = [Some(NativeEvent::OutputFault(NativeError::new(NativeErrorCode::OutputFault, "sentinel")))];
        assert_eq!(runtime.poll(&mut full).written, 0);
        assert_all_controls_busy(&mut runtime, &candidate, 1);
        assert_eq!(runtime.cancel_operation(ticket.operation).unwrap(), CancelDisposition::AlreadyResolved);
        match event(&mut runtime) {
            NativeEvent::RevisionApplied { ticket: actual, transport, callback_count,
                structural: actual_structural, faded, plugin_stamp, .. } => {
                assert_eq!(actual, ticket);
                assert_eq!(actual_structural, structural);
                assert_eq!(faded, structural);
                assert_eq!(plugin_stamp.generation, 0);
                assert_eq!(callback_count, if structural { 2 } else { 1 });
                let current = runtime.status().transport;
                assert_eq!(transport.generation, current.generation);
                if structural {
                    assert_eq!(transport.snapshot.sample_position, current.snapshot.sample_position);
                } else {
                    // Value receipts describe the apply boundary before this
                    // callback renders its following hardware block.
                    assert_eq!(transport.snapshot.sample_position + 256, current.snapshot.sample_position);
                }
                assert_eq!(transport.loop_range, current.loop_range);
                assert_eq!(transport.end_tick, current.end_tick);
            }
            other => panic!("unexpected revision outcome: {other:?}"),
        }
        assert!(!runtime.poll(&mut []).remaining);
        let control = runtime.set_running(1, false).unwrap();
        callback.process(&mut [0.0; 512], 2);
        applied_control(&mut runtime, control);
        assert_eq!(runtime.status().stream_start_count, stream_starts);
    }
}

#[test]
fn cancelled_revision_retains_boundary_until_its_single_rejection_is_delivered() {
    let accepted = session();
    let (mut runtime, mut callback) = deterministic_runtime(&accepted);
    let prepared = runtime.prepare_revision(&accepted, &accepted, 0, 9, &HostContext::default()).unwrap();
    let ticket = runtime.submit_revision(prepared).unwrap();
    assert_eq!(runtime.cancel_operation(ticket.operation).unwrap(), CancelDisposition::Prevented);
    assert_eq!(runtime.cancel_operation(ticket.operation).unwrap(), CancelDisposition::Prevented);
    callback.process(&mut [0.0; 512], 2);
    runtime.poll(&mut []);
    assert_all_controls_busy(&mut runtime, &accepted, 0);
    match event(&mut runtime) {
        NativeEvent::RevisionRejected { ticket: actual, error } => {
            assert_eq!(actual, ticket);
            assert_eq!(error.code, NativeErrorCode::Cancelled);
            assert_eq!((error.expected_revision, error.observed_revision), (Some(0), Some(0)));
        }
        other => panic!("{other:?}"),
    }
    assert!(!runtime.poll(&mut []).remaining);
    assert_eq!(runtime.status().source_generation, 0);
    assert!(runtime.set_running(0, false).is_ok());
}

#[test]
fn collected_control_outcomes_still_exclude_prepared_transport_and_revision() {
    let accepted = session();
    let (mut runtime, mut callback) = deterministic_runtime(&accepted);
    let ticket = runtime.set_running(0, false).unwrap();
    callback.process(&mut [0.0; 512], 2);
    runtime.poll(&mut []);
    assert!(runtime.controls.is_empty());
    assert_eq!(runtime.restart(&accepted, 0, &HostContext::default()).unwrap_err().code, NativeErrorCode::Busy);
    assert_eq!(runtime.prepare_revision(&accepted, &accepted, 0, 1, &HostContext::default()).unwrap_err().code,
        NativeErrorCode::Busy);
    applied_control(&mut runtime, ticket);
    assert!(runtime.prepare_revision(&accepted, &accepted, 0, 1, &HostContext::default()).is_ok());
}

#[test]
fn public_observations_publish_full_contiguous_windows_at_negotiated_rate() {
    let accepted = session();
    let (mut runtime, mut callback) = deterministic_runtime(&accepted);
    let selection = ObservationSelection {
        meter_tracks: Vec::new(), master_meter: false, analysis: Some(ObservationTarget::Master),
        meter_hz: 0, analysis_hz: MAX_ANALYSIS_HZ,
    };
    let ticket = runtime.set_observation(0, &selection).unwrap();
    callback.process(&mut [0.0; 512], 2);
    applied_control(&mut runtime, ticket);
    let mut positions = Vec::new();
    let mut epoch = None;
    let mut sequence = 0;
    for _ in 0..200 {
        callback.process(&mut [0.0; 512], 2);
        let mut slots = [None];
        runtime.drain_observations(&mut slots);
        if let Some(frame) = slots[0].take() {
            assert_eq!(frame.sample_count, 2048);
            assert_eq!(frame.mono_samples.len(), 2048);
            assert_eq!(frame.sample_rate, 48_000.0);
            assert!(!frame.meters_present);
            assert!(frame.meter_sample_position.is_none());
            assert_eq!(*epoch.get_or_insert(frame.capture_epoch), frame.capture_epoch);
            assert!(frame.sequence > sequence);
            sequence = frame.sequence;
            positions.push(frame.sample_position);
        }
    }
    assert!(positions.len() > 20, "30 Hz candidate must not retain the former 10 Hz ceiling");
    assert!(positions.windows(2).all(|pair| (1536..=1792).contains(&(pair[1] - pair[0]))));
}

#[test]
fn loop_wrap_advances_generation_and_cannot_publish_a_cross_boundary_window() {
    let accepted = session();
    let (mut runtime, mut callback) = deterministic_runtime(&accepted);
    let selection = ObservationSelection {
        meter_tracks: Vec::new(), master_meter: false, analysis: Some(ObservationTarget::Master),
        meter_hz: 0, analysis_hz: MAX_ANALYSIS_HZ,
    };
    let capture = runtime.set_observation(0, &selection).unwrap();
    callback.process(&mut [0.0; 512], 2);
    applied_control(&mut runtime, capture);
    // At 120 BPM / 48kHz this range is 4,096 physical samples.
    let loop_ticket = runtime.set_loop(&accepted, 0, Some((0, 163_840)), &HostContext::default()).unwrap();
    callback.process(&mut [0.0; 512], 2);
    applied_control(&mut runtime, loop_ticket);
    let mut last_generation = runtime.status().transport.generation;
    let mut last_epoch = 0;
    let mut detailed = 0;
    for _ in 0..80 {
        let (_, allocations, frees) = crate::audio::observation::tests::allocation_activity(||
            callback.process(&mut [0.0; 512], 2));
        assert_eq!((allocations, frees), (0, 0));
        let status = runtime.status();
        let mut frames = [None];
        runtime.drain_observations(&mut frames);
        if status.transport.generation != last_generation {
            assert!(frames[0].is_none(), "wrap retires the preceding complete window");
            last_generation = status.transport.generation;
        }
        if let Some(frame) = frames[0].take() {
            detailed += 1;
            assert_eq!(frame.transport_generation, status.transport.generation);
            assert_eq!(frame.sample_count, 2048);
            assert!(status.transport.snapshot.project_frame >= 2048.0);
            assert!(frame.capture_epoch >= last_epoch);
            last_epoch = frame.capture_epoch;
        }
    }
    assert!(detailed > 0);
    assert!(last_generation > 1);
}

#[test]
fn successful_plugin_restart_keeps_source_generation_and_promotes_receipt_stamp_once() {
    let plugin = crate::native::test_plugin::RestartPlugin::build();
    let mut accepted = plugin.session();
    let (mut runtime, mut callback) = deterministic_runtime(&accepted);
    let source_generation = runtime.status().source_generation;
    let initial_token = runtime.status().devices.iter().find(|device| device.is_plugin).unwrap().instance_token;
    plugin.arm();
    callback.process(&mut [0.0; 512], 2);
    runtime.service_plugins(&accepted, 0).unwrap();
    let request = match event(&mut runtime) {
        NativeEvent::PluginRestartRequested(request) => request,
        other => panic!("{other:?}"),
    };
    assert_eq!(request.instance_tokens, vec![initial_token]);
    assert_eq!(request.expected_revision, 0);
    let prepared = runtime.prepare_plugin_restart(&accepted, 0, source_generation, &request, &HostContext::default()).unwrap();
    let ticket = runtime.submit_revision(prepared).unwrap();
    assert_eq!(runtime.status().accepted_revision, 0);
    assert_eq!(runtime.plugin_stamp().generation, 0);
    for _ in 0..2 {
        let (_, allocations, frees) = crate::audio::observation::tests::allocation_activity(||
            callback.process(&mut [0.0; 512], 2));
        assert_eq!((allocations, frees), (0, 0));
    }
    match event(&mut runtime) {
        NativeEvent::RevisionApplied { ticket: actual, plugin_stamp, structural, faded, transport, callback_count, .. } => {
            assert_eq!(actual, ticket);
            assert_eq!(actual.source_generation, source_generation);
            assert_eq!(actual.revision, 1);
            assert_eq!(plugin_stamp.generation, 1);
            assert!(structural && faded);
            assert_eq!(callback_count, 3);
            assert_eq!(transport.snapshot.sample_position, runtime.status().transport.snapshot.sample_position);
            plugin_stamp.apply(&mut accepted);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(runtime.status().source_generation, source_generation);
    assert_eq!(runtime.status().accepted_revision, 1);
    let current_token = runtime.status().devices.iter().find(|device| device.is_plugin).unwrap().instance_token;
    assert_ne!(current_token, initial_token);
    runtime.service_plugins(&accepted, 1).unwrap();
    assert!(!runtime.poll(&mut []).remaining);
    assert_eq!(runtime.cancel_operation(ticket.operation).unwrap(), CancelDisposition::AlreadyResolved);
    assert!(runtime.shutdown().error.is_none());
}

#[test]
fn an_undelivered_terminal_outcome_survives_preparation_history_churn() {
    let accepted = session();
    let (mut runtime, mut callback) = deterministic_runtime(&accepted);
    let prepared = runtime.prepare_revision(&accepted, &accepted, 0, 1, &HostContext::default()).unwrap();
    let tickets: Vec<_> = (0..64).map(|_| runtime.set_running(0, true).unwrap()).collect();
    callback.process(&mut [0.0; 512], 2);
    runtime.poll(&mut []);
    assert!(runtime.resolved.contains(&tickets[0].operation));
    // A dropped unsubmitted handle is resolved after these 64 collected controls,
    // evicting the oldest control from history without delivering its outcome.
    drop(prepared);
    runtime.poll(&mut []);
    assert!(!runtime.resolved.contains(&tickets[0].operation));
    assert_eq!(runtime.cancel_operation(tickets[0].operation).unwrap(), CancelDisposition::AlreadyResolved);
    assert_eq!(runtime.control_reservations, 64);
    for ticket in tickets { applied_control(&mut runtime, ticket); }
    assert!(!runtime.poll(&mut []).remaining);
}
