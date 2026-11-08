#![cfg(feature = "desktop")]
use muz::live::{LiveEvent, LiveSession};
use std::time::{Duration, Instant};

fn poll_clean(live: &mut LiveSession) {
    for event in live.poll(Duration::from_millis(2)).unwrap() {
        if let LiveEvent::Rejected { diagnostic } = event {
            panic!("live operation was rejected: {diagnostic:?}");
        }
    }
}

/// Runs a control call, polling until admission stops refusing with `Busy`.
/// Observable status (running, applied revision) can lead the polled outcome
/// that releases the previous control's reservation.
fn admit<T>(
    live: &mut LiveSession,
    mut call: impl FnMut(&mut LiveSession) -> Result<T, muz::live::LiveSessionError>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match call(live) {
            Ok(value) => return value,
            Err(muz::live::LiveSessionError::Native(error))
                if error.code == muz::native::NativeErrorCode::Busy
                    && Instant::now() < deadline =>
            {
                poll_clean(live);
            }
            Err(error) => panic!("control was not admitted: {error:?}"),
        }
    }
}

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
    assert_eq!(before.applied_revision, 0);
    assert_eq!(before.runtime_revision, 0);
    assert_eq!(before.audio.host, "headless");
    admit(&mut live, |live| live.seek_ticks(960000));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        poll_clean(&mut live);
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
    admit(&mut live, |live| live.restart());
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        poll_clean(&mut live);
        let status = live.status();
        if status.transport.current_tick.abs() < 0.01 {
            assert!(!status.transport.running);
            assert_eq!(status.applied_revision, 0);
            assert_eq!(status.runtime_revision, 0);
            assert_eq!(status.stream_start_count, before.stream_start_count);
            break;
        }
        assert!(Instant::now() < deadline, "prepared restart was not accepted");
    }
    assert!(live.shutdown().error.is_none());
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
    admit(&mut live, |live| live.set_loop(Some((96000, 192000))));
    admit(&mut live, |live| live.set_running(true));
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_tick = 0.;
    let mut wraps = 0;
    while wraps < 3 {
        poll_clean(&mut live);
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
        poll_clean(&mut live);
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
    admit(&mut live, |live| live.set_running(false));
    let pause_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        poll_clean(&mut live);
        if !live.status().transport.running {
            break;
        }
        assert!(Instant::now() < pause_deadline, "pause was not acknowledged");
    }
    admit(&mut live, |live| live.seek_ticks(144000));
    loop {
        poll_clean(&mut live);
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
        poll_clean(&mut live);
        assert_eq!(live.status().render_faults, 0);
        assert!(
            Instant::now() < revision_deadline,
            "source edit after prepared cutover failed: {:?}",
            live.status().last_error
        );
    }
    admit(&mut live, |live| live.set_loop(None));
    assert!(live.shutdown().error.is_none());
}
