//! Callback allocation and event-boundary determinism for layered, stateful SFZ.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {
    static WATCH: Cell<bool> = const {Cell::new(false)};
    static COUNT: Cell<usize> = const {Cell::new(0)};
}
struct Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if WATCH.try_with(Cell::get).unwrap_or(false) {
            COUNT.with(|c| c.set(c.get() + 1));
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if WATCH.try_with(Cell::get).unwrap_or(false) {
            COUNT.with(|c| c.set(c.get() + 1));
        }
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
use muz::{
    audio::{AudioConfig, AudioEngine},
    lang::Evaluator,
};
fn fixture() -> (tempfile::TempDir, muz::Session) {
    let dir = tempfile::tempdir().unwrap();
    let mut w = hound::WavWriter::create(
        dir.path().join("tone.wav"),
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for i in 0..4800 {
        w.write_sample(((i as f32) * 0.057).sin() * 0.2).unwrap();
    }
    w.finalize().unwrap();
    let path = dir.path().join("layered.sfz");
    std::fs::write(&path,"<global> sample=tone.wav key=60 ampeg_attack=0.002 ampeg_release=0.02\n<group> loop_mode=loop_continuous loop_start=100 loop_end=4000 seq_length=2\n<region> seq_position=1 pan=-30\n<region> seq_position=1 pan=30\n<region> seq_position=2 tune=12\n<group> trigger=release_key loop_mode=one_shot\n<region> volume=-12\n").unwrap();
    let source = format!(
        "song({{tempo:120,tail:0.2,tracks:[track(\"x\",stack([note(60,0.125b),note(60,0.125b,at=0.25b),note(60,0.125b,at=0.5b)]),sfz({}))]}})",
        serde_json::to_string(&path.to_string_lossy()).unwrap()
    );
    let v = Evaluator::new().source(&source).unwrap();
    let session = muz::compile::lower(
        v.get("__result").unwrap().clone(),
        &dir.path().join("song.muz"),
        vec![],
    )
    .unwrap()
    .session;
    (dir, session)
}
fn render(session: &muz::Session, block: usize, watch: bool) -> (Vec<f32>, usize) {
    let mut e = AudioEngine::new(
        session,
        AudioConfig {
            sample_rate: 48000.,
            max_frames: 256,
            offline: true,
        },
    )
    .unwrap();
    e.set_running(true);
    let mut pcm = vec![0.; 48000];
    COUNT.with(|c| c.set(0));
    WATCH.with(|c| c.set(watch));
    for chunk in pcm.chunks_mut(block * 2) {
        e.render_interleaved(chunk, 2).unwrap();
    }
    WATCH.with(|c| c.set(false));
    let count = COUNT.with(Cell::get);
    let stats = e.device_sfz_statistics();
    assert_eq!(stats.len(), 1);
    assert!(stats[0].1.started_voices >= 7);
    (pcm, count)
}
#[test]
fn layered_attack_release_loop_callbacks_allocate_nothing_and_ignore_block_partition() {
    let (_assets, session) = fixture();
    let (a, allocations) = render(&session, 256, true);
    assert_eq!(allocations, 0, "allocations or deallocations in callback");
    let (b, _) = render(&session, 63, false);
    assert!(a.iter().any(|x| x.abs() > 0.001));
    assert_eq!(
        a, b,
        "event and DSP state must depend on samples, not callback partition"
    );
}

#[test]
fn reusable_loop_checkpoint_restores_without_callback_allocation() {
    let (_assets, session) = fixture();
    let checked = muz::description::ValidatedSession::new(&session).unwrap();
    let mut e = AudioEngine::prepare_loop(
        &checked,
        AudioConfig {
            sample_rate: 48000.,
            max_frames: 256,
            offline: true,
        },
        540_000,
        600_000,
        13500,
    )
    .unwrap();
    let mut pcm = vec![0.; 1500 * 2 * 3];
    COUNT.with(|c| c.set(0));
    WATCH.with(|c| c.set(true));
    for chunk in pcm.chunks_mut(512) {
        e.render_interleaved(chunk, 2).unwrap();
    }
    WATCH.with(|c| c.set(false));
    assert_eq!(COUNT.with(Cell::get), 0);
    assert_eq!(&pcm[..3000], &pcm[3000..6000]);
    assert_eq!(&pcm[..3000], &pcm[6000..]);
}
