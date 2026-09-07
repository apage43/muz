use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {static WATCH:Cell<bool>=const{Cell::new(false)};static ALLOCS:Cell<usize>=const{Cell::new(0)};}
struct Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if WATCH.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.with(|n| n.set(n.get() + 1));
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if WATCH.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.with(|n| n.set(n.get() + 1));
        }
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
fn compile(src: &str) -> (tempfile::TempDir, std::path::PathBuf, muz::Session) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("case.muz");
    std::fs::write(&p, src).unwrap();
    let c = muz::compile::compile(&p).unwrap();
    (d, p, c.session)
}
#[test]
fn native_callback_does_not_allocate_or_retire_objects() {
    let (_d, _p, s) = compile(
        r#"song({tracks:[track("voice",phrase("C4:h E4:h").express({brightness:[[0,0.2],[1,0.8]],tuning:[[0,0],[1,0.1]]}),voice_patch("test",{nodes:[{id:"a",op:"osc"},{id:"d",op:"delay",input:"a",seconds:0.01,max_seconds:0.02},{id:"e",op:"adsr"},{id:"out",op:"mul",inputs:["d","e"]}],output:"out"})),track("a",phrase("C4:e E4:e G4:e B4:e").repeat(8).express({volume:[[0,0.5],[1,1]],pan:0.6,tuning:[[0,0],[1,0.1]]}),synth("pad"),{chain:[fx("chorus"),rack([[fx("reverb"),fx("gain")]],{modulate:[{target:"0.1.gain_db",base:-6,depth:3,rate_hz:1,follower:-2,min:-12,max:0}]})]})],automation:[automation("a.instrument.cutoff_hz",curve([[0b,300],[8b,4000]]))],master:[fx("limiter")]})"#,
    );
    let mut e = muz::audio::AudioEngine::new(
        &s,
        muz::audio::AudioConfig {
            sample_rate: 48000.,
            max_frames: 256,
        },
    )
    .unwrap();
    let mut b = [0.; 512];
    e.set_running(true);
    ALLOCS.with(|n| n.set(0));
    WATCH.with(|w| w.set(true));
    let mut error = None;
    for _ in 0..100 {
        if let Err(e) = e.render_interleaved(&mut b, 2) {
            error = Some(e);
            break;
        }
    }
    WATCH.with(|w| w.set(false));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        ALLOCS.with(Cell::get),
        0,
        "allocation or object retirement on audio callback"
    );
}
#[test]
fn section_tail_cannot_start_material_from_the_next_section() {
    let (d, p, _) = compile(
        r#"song({sections:[section("quiet",1bars),section("next",1bars)],tracks:[track("a",phrase("C4:w").at(1bars),synth("bell"))],tail:1})"#,
    );
    let path = d.path().join("quiet.wav");
    muz::render::render(&p, &path, None, Some("quiet"), &[], 48000, 97).unwrap();
    let audio = hound::WavReader::open(path)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert!(audio.iter().all(|v| v.abs() < 1e-12));
}
#[test]
fn smf_preserves_independent_sequences_and_opaque_payloads() {
    use muz::smf::*;
    let doc = Document {
        format: 2,
        division: 0xe728,
        tracks: vec![
            vec![
                Event {
                    delta: 0,
                    kind: Kind::Meta {
                        tag: 127,
                        data: vec![0, 1, 255],
                    },
                },
                Event {
                    delta: 300,
                    kind: Kind::SysEx {
                        data: vec![125, 3, 247],
                    },
                },
                Event {
                    delta: 0,
                    kind: Kind::Meta {
                        tag: 47,
                        data: vec![],
                    },
                },
            ],
            vec![
                Event {
                    delta: 0,
                    kind: Kind::Midi {
                        bytes: vec![145, 60, 100],
                    },
                },
                Event {
                    delta: 100,
                    kind: Kind::Midi {
                        bytes: vec![145, 60, 0],
                    },
                },
                Event {
                    delta: 0,
                    kind: Kind::Meta {
                        tag: 47,
                        data: vec![],
                    },
                },
            ],
        ],
    };
    let roundtrip = decode(&encode(&doc).unwrap()).unwrap();
    assert_eq!(doc, roundtrip);
    assert!(pattern(&doc, None).is_err());
    let mut musical = doc;
    musical.division = 480;
    assert!(pattern(&musical, None).is_err());
    assert_eq!(pattern(&musical, Some(1)).unwrap().notes.len(), 1);
}

#[test]
fn removing_a_sounding_note_keeps_its_original_release_obligation() {
    use muz::audio::{AudioConfig, AudioEngine, PreparedTransaction};
    let (d, p, s) = compile(
        r#"song({sections:[section("a",2bars)],tracks:[track("a",phrase("C4:w"),synth("init",{sustain:1,release_ms:30}))]})"#,
    );
    let config = AudioConfig {
        sample_rate: 48000.,
        max_frames: 256,
    };
    let mut e = AudioEngine::new(&s, config).unwrap();
    e.set_running(true);
    let mut buf = [0.; 512];
    for _ in 0..40 {
        e.render_interleaved(&mut buf, 2).unwrap();
    }
    let token = e.device_debug_states()[0].1.instance_token;
    std::fs::write(&p,r#"song({sections:[section("a",2bars)],tracks:[track("a",rest(2bars),synth("init",{sustain:1,release_ms:30}))]})"#).unwrap();
    let next = muz::compile::compile(&p).unwrap().session;
    let plan = muz::plan_reconciliation(0, &s, &next).unwrap();
    let mut transaction =
        PreparedTransaction::prepare(&s, &next, &plan, 1, std::time::Instant::now(), config)
            .unwrap();
    ALLOCS.with(|n| n.set(0));
    WATCH.with(|w| w.set(true));
    let result = e.apply_transaction(&mut transaction);
    WATCH.with(|w| w.set(false));
    result.unwrap();
    assert_eq!(ALLOCS.with(Cell::get), 0);
    assert_eq!(e.device_debug_states()[0].1.instance_token, token);
    e.render_interleaved(&mut buf, 2).unwrap();
    assert!(
        buf.iter().any(|v| v.abs() > 0.001),
        "source replacement killed the held voice"
    );
    for _ in 0..420 {
        e.render_interleaved(&mut buf, 2).unwrap();
    }
    assert_eq!(e.status().delivered_events.note_ons, 1);
    assert_eq!(e.status().delivered_events.note_offs, 1);
    assert!(
        buf.iter().all(|v| v.abs() < 0.0001),
        "deleted note remained stuck"
    );
    drop(d);
}
