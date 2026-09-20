use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {
    static WATCH_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
}
struct CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if WATCH_ALLOCATIONS.try_with(Cell::get).unwrap_or(false) {
            ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if WATCH_ALLOCATIONS.try_with(Cell::get).unwrap_or(false) {
            ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
        }
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

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
fn legacy(count: usize, at: u64, duration: u64, cycle: u64) -> muz::Session {
    use muz::model::*;
    let mut s = session(1, 0);
    s.transport = Transport::Loop {
        bpm: 120.,
        meter: [4, 4],
        loop_ticks: cycle,
    };
    s.tracks[0].source = TrackSource::Pattern(Pattern {
        id: Id::new("loop"),
        notes: (0..count)
            .map(|i| Note {
                id: Id::new(format!("n{i}")),
                start_ticks: at,
                duration_ticks: duration,
                key: 60,
                velocity: 1.,
            })
            .collect(),
    });
    s
}
#[test]
fn legacy_preflight_counts_multicycle_releases_and_short_loops() {
    for s in [
        legacy(130, 0, 960, 960),
        legacy(1, 0, 300 * 960, 960),
        legacy(30, 0, 1, 1),
    ] {
        assert!(AudioEngine::new(&s, config()).is_err());
    }
    let mut engine = AudioEngine::new(&legacy(8, 0, 3 * 960, 960), config()).unwrap();
    engine.set_running(true);
    let mut pcm = [0.; 512];
    for _ in 0..500 {
        engine.render_interleaved(&mut pcm, 2).unwrap();
    }
}
#[test]
fn legacy_edits_preflight_accumulated_obligations() {
    let old = legacy(80, 0, 100 * 960, 100 * 960);
    let next = legacy(80, 960, 100 * 960, 100 * 960);
    let third = legacy(80, 4 * 960, 100 * 960, 100 * 960);
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

fn custom_session(notes: usize, lanes: usize) -> muz::Session {
    let declarations = (0..lanes)
        .map(|i| format!("lane_{i:02}:0.25"))
        .collect::<Vec<_>>()
        .join(",");
    let curves = (0..lanes)
        .map(|i| format!("lane_{i:02}:[[0,0.2],[1,0.8]]"))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!(
        r#"song({{tempo:120,tracks:[track("custom",stack(map(range({notes}),fn(i)=>note(60,1b,velocity=1).gate(1).express({{{curves}}}))),voice_patch("p",{{gain_db:0,note_controls:{{{declarations}}},nodes:[{{id:"x",op:"expression",kind:"lane_00"}}],output:{{left:"x",right:"x"}}}}))],tail:0}})"#
    );
    let value = Evaluator::new().source(&source).unwrap();
    muz::compile::lower(
        value.get("__result").unwrap().clone(),
        std::path::Path::new("custom-capacity.muz"),
        vec![],
    )
    .unwrap()
    .session
}

#[test]
fn authored_custom_lanes_count_toward_callback_capacity() {
    // Each note has sixteen changing lanes: 48 updates plus start/release
    // reservations at a 256-frame block size. Six notes exceed 256 events.
    let full = custom_session(6, 16);
    assert!(
        AudioEngine::new(&full, config())
            .unwrap_err_message()
            .contains("callback events")
    );
    let sparse = custom_session(6, 1);
    let mut engine = AudioEngine::new(&sparse, config()).unwrap();
    engine.set_running(true);
    let mut pcm = [0.; 512];
    for _ in 0..100 {
        engine.render_interleaved(&mut pcm, 2).unwrap();
    }
}

#[test]
fn custom_curve_delivery_is_independent_of_audio_block_size() {
    let s = custom_session(1, 1);
    let render = |frames| {
        let mut engine = AudioEngine::new(
            &s,
            AudioConfig {
                max_frames: frames,
                ..config()
            },
        )
        .unwrap();
        engine.set_running(true);
        let mut pcm = vec![0.; 24000 * 2];
        for chunk in pcm.chunks_mut(frames * 2) {
            engine.render_interleaved(chunk, 2).unwrap();
        }
        pcm
    };
    let a = render(256);
    let b = render(97);
    assert_eq!(a, b);
    assert!(a[20000 * 2] > a[1000 * 2] + 0.4);
}

#[test]
fn custom_note_scheduling_and_rendering_do_not_allocate_in_callback() {
    let s = custom_session(2, 8);
    let mut engine = AudioEngine::new(&s, config()).unwrap();
    engine.set_running(true);
    let mut pcm = [0.; 512];
    ALLOCATION_COUNT.with(|count| count.set(0));
    WATCH_ALLOCATIONS.with(|watch| watch.set(true));
    for _ in 0..100 {
        engine.render_interleaved(&mut pcm, 2).unwrap();
    }
    WATCH_ALLOCATIONS.with(|watch| watch.set(false));
    assert_eq!(ALLOCATION_COUNT.with(Cell::get), 0);
}
