#![cfg(feature = "desktop")]
use muz::live::LiveSession;
use std::time::{Duration, Instant};

#[test]
fn live_sfz_seek_uses_prepared_history_cutover_without_restarting_stream() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("program.sfz"),
        "<region> sample=*silence key=60",
    )
    .unwrap();
    let source = dir.path().join("main.muz");
    std::fs::write(
        &source,
        "song({tracks:[track(\"x\",note(60,4b),sfz(\"program.sfz\"))]})",
    )
    .unwrap();
    let mut live = LiveSession::start_backend(&source, false, Duration::ZERO, true).unwrap();
    let before = live.status();
    live.seek_ticks(960000).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        live.poll(Duration::from_millis(2)).unwrap();
        let status = live.status();
        if (status.transport.current_tick - 960000.).abs() < 0.01 {
            assert!(!status.transport.running);
            assert_eq!(status.stream_generation, before.stream_generation);
            assert_eq!(status.stream_start_count, before.stream_start_count);
            assert_eq!(status.render_faults, 0);
            break;
        }
        assert!(Instant::now() < deadline, "prepared seek was not accepted");
    }
}

#[test]
fn live_sfz_nonzero_loop_reuses_prepared_history_without_stream_restart() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("program.sfz"),
        "<region> sample=*silence key=60",
    )
    .unwrap();
    let source = dir.path().join("main.muz");
    std::fs::write(
        &source,
        "song({tracks:[track(\"x\",note(60,4b),sfz(\"program.sfz\"))]})",
    )
    .unwrap();
    let session = muz::source::parse_project(&source).unwrap();
    let checked = muz::description::ValidatedSession::new(&session).unwrap();
    let mut offline = muz::audio::AudioEngine::prepare_loop(
        &checked,
        muz::audio::AudioConfig {
            sample_rate: 48000.,
            max_frames: 256,
            offline: true,
        },
        96000,
        192000,
        48000,
    )
    .unwrap();
    offline.set_running(false);
    offline.set_running(true);
    let mut audio = [0.; 512];
    for block in 0..30 {
        offline
            .render_interleaved(&mut audio, 2)
            .unwrap_or_else(|error| panic!("prepared loop block {block}: {error}"));
    }
    let mut live = LiveSession::start_backend(&source, false, Duration::ZERO, true).unwrap();
    let before = live.status();
    live.set_loop(Some((96000, 192000))).unwrap();
    live.set_running(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_tick = 0.;
    let mut wraps = 0;
    while wraps < 3 {
        live.poll(Duration::from_millis(2)).unwrap();
        let status = live.status();
        let tick = status.transport.current_tick;
        if tick < last_tick {
            wraps += 1;
        }
        last_tick = tick;
        assert_eq!(status.stream_generation, before.stream_generation);
        assert_eq!(status.stream_start_count, before.stream_start_count);
        assert_eq!(status.render_faults, 0);
        assert!(Instant::now() < deadline, "loop did not wrap repeatedly");
    }
    // Accepted source revisions invalidate and rebuild the loop checkpoint.
    std::fs::write(
        &source,
        "song({tracks:[track(\"x\",note(60,4b,velocity=0.5),sfz(\"program.sfz\"))]})",
    )
    .unwrap();
    let edit_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        live.poll(Duration::from_millis(2)).unwrap();
        let status = live.status();
        assert_eq!(status.render_faults, 0);
        if status.applied_revision > 0 && status.pending_revision.is_none() {
            break;
        }
        assert!(
            Instant::now() < edit_deadline,
            "source revision was not accepted"
        );
    }
    live.set_running(false).unwrap();
    live.seek_ticks(144000).unwrap();
    loop {
        live.poll(Duration::from_millis(2)).unwrap();
        let status = live.status();
        if !status.transport.running && (status.transport.current_tick - 144000.).abs() < 0.01 {
            break;
        }
        assert!(
            Instant::now() < edit_deadline,
            "seek inside audition loop was not restored: {:?}",
            status
        );
    }
    std::fs::write(
        &source,
        "song({tracks:[track(\"x\",note(60,4b,velocity=0.25),sfz(\"program.sfz\"))]})",
    )
    .unwrap();
    let revision_deadline = Instant::now() + Duration::from_secs(2);
    while live.status().applied_revision < 2 {
        live.poll(Duration::from_millis(2)).unwrap();
        assert_eq!(live.status().render_faults, 0);
        assert!(
            Instant::now() < revision_deadline,
            "source edit after prepared cutover failed: {:?}",
            live.status().last_error
        );
    }
    live.set_loop(None).unwrap();
}
