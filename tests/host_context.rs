use muz::{
    assets::MemoryAssets,
    host::HostContext,
    lang::{Evaluator, FileSourceLoader},
};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, atomic::Ordering},
};
fn wav() -> Arc<[u8]> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(
            &mut out,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..480 {
            w.write_sample(4096i16).unwrap();
        }
        w.finalize().unwrap();
    }
    out.into_inner().into()
}
#[test]
fn contexts_have_independent_limits_and_cancellation() {
    let a = HostContext::default();
    let b = HostContext {
        evaluation_steps: 1,
        graph_units: 7,
        ..HostContext::default()
    };
    a.cancelled.store(true, Ordering::Relaxed);
    assert!(
        Evaluator::with_context(Rc::new(FileSourceLoader), a.clone())
            .source("1")
            .is_err()
    );
    assert!(
        Evaluator::with_context(Rc::new(FileSourceLoader), b.clone())
            .source("1+2")
            .is_err()
    );
    assert!(
        Evaluator::with_context(Rc::new(FileSourceLoader), HostContext::default())
            .source("1+2")
            .is_ok()
    );
    a.run(|| {
        assert_eq!(muz::model::graph_budget().unwrap(), 4096);
        b.run(|| assert_eq!(muz::model::graph_budget().unwrap(), 7));
        assert_eq!(muz::model::graph_budget().unwrap(), 4096);
    });
}

#[test]
fn immutable_assets_share_decoding_but_isolate_resolvers_and_limits() {
    let path = Path::new("/shared.wav");
    let mut assets = MemoryAssets::default();
    assets.insert(path.into(), wav(), 1);
    let context = HostContext {
        assets: Arc::new(assets),
        ..Default::default()
    };
    let first = context.run(|| muz::audio_file::load_shared(path, 480).unwrap().1);
    let second = context.run(|| muz::audio_file::load_shared(path, 480).unwrap().1);
    assert!(Arc::ptr_eq(&first, &second));
    context.run(|| assert!(muz::audio_file::load_shared(path, 1).is_err()));
    let mut assets = MemoryAssets::default();
    assets.insert(path.into(), wav(), 1);
    let other = HostContext {
        assets: Arc::new(assets),
        ..context.clone()
    };
    let third = other.run(|| muz::audio_file::load_shared(path, 480).unwrap().1);
    assert!(!Arc::ptr_eq(&first, &third));
    context.cancelled.store(true, Ordering::Relaxed);
    context.run(|| {
        assert!(
            muz::audio_file::load_shared(path, 480)
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        )
    });
}

#[test]
fn snapshots_reject_mid_read_changes_and_observe_read_cancellation() {
    use muz::assets::{AssetReader, AssetResolver};
    struct Changing(std::sync::atomic::AtomicU64);
    impl AssetResolver for Changing {
        fn resolve(&self, p: &Path) -> anyhow::Result<PathBuf> {
            Ok(p.into())
        }
        fn version(&self, _: &Path) -> anyhow::Result<(u64, u128)> {
            Ok((4, self.0.fetch_add(1, Ordering::Relaxed) as u128))
        }
        fn open(&self, _: &Path) -> anyhow::Result<Box<dyn AssetReader>> {
            Ok(Box::new(std::io::Cursor::new(vec![0; 4])))
        }
    }
    let context = HostContext {
        assets: Arc::new(Changing(Default::default())),
        ..Default::default()
    };
    context.run(|| {
        let error = muz::assets::read_bounded(Path::new("changing"), 4).unwrap_err();
        assert!(format!("{error:#}").contains("stale asset"));
    });
    struct Cancelling {
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    }
    struct Reader {
        cancelled: Arc<std::sync::atomic::AtomicBool>,
        inner: std::io::Cursor<Vec<u8>>,
    }
    impl std::io::Read for Reader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            self.cancelled.store(true, Ordering::Relaxed);
            std::io::Read::read(&mut self.inner, out)
        }
    }
    impl std::io::Seek for Reader {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            std::io::Seek::seek(&mut self.inner, pos)
        }
    }
    impl AssetResolver for Cancelling {
        fn resolve(&self, p: &Path) -> anyhow::Result<PathBuf> {
            Ok(p.into())
        }
        fn version(&self, _: &Path) -> anyhow::Result<(u64, u128)> {
            Ok((100000, 1))
        }
        fn open(&self, _: &Path) -> anyhow::Result<Box<dyn AssetReader>> {
            Ok(Box::new(Reader {
                cancelled: self.cancelled.clone(),
                inner: std::io::Cursor::new(vec![0; 100000]),
            }))
        }
    }
    let mut context = HostContext::default();
    context.assets = Arc::new(Cancelling {
        cancelled: context.cancelled.clone(),
    });
    context.run(|| {
        assert!(
            format!(
                "{:#}",
                muz::assets::read_bounded(Path::new("cancelled"), 100000).unwrap_err()
            )
            .contains("cancelled")
        )
    });
}
#[test]
fn memory_assets_prepare_clip_and_decode_wav_flac_midi_without_files() {
    let mut assets = MemoryAssets::default();
    assets.insert(PathBuf::from("/virtual/tone.wav"), wav(), 1);
    let hex = "664c614300000022100010000000000000000bb800f0000000200000000000000000000000000000000084000028200000007265666572656e6365206c6962464c414320312e352e3020323032353032313100000000fff86a08001f7a000000f914";
    let flac: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    assets.insert(PathBuf::from("/virtual/tone.flac"), flac.into(), 2);
    let midi = muz::smf::encode(&muz::smf::Document {
        format: 0,
        division: 960,
        tracks: vec![vec![muz::smf::Event {
            delta: 0,
            kind: muz::smf::Kind::Meta {
                tag: 47,
                data: vec![],
            },
        }]],
    })
    .unwrap();
    assets.insert(PathBuf::from("/virtual/empty.mid"), midi.into(), 1);
    let context = HostContext {
        assets: Arc::new(assets),
        ..Default::default()
    };
    context.run(|| {
        assert_eq!(
            muz::audio_file::load(Path::new("/virtual/tone.wav"), 480)
                .unwrap()
                .1
                .len(),
            480
        );
        assert_eq!(
            muz::audio_file::load(Path::new("/virtual/tone.flac"), 480)
                .unwrap()
                .1
                .len(),
            32
        );
        assert!(muz::audio_file::load(Path::new("/virtual/tone.wav"), 10).is_err());
        assert!(
            muz::midi::import_midi_file("/virtual/empty.mid")
                .unwrap()
                .notes
                .is_empty()
        );
        let mut e = Evaluator::with_context(Rc::new(FileSourceLoader), context.clone());
        e.path = PathBuf::from("/virtual/song.muz");
        let v = e
            .source(r#"song({tracks:[clip("audio","tone.wav",{fade_in:0,fade_out:0})]})"#)
            .unwrap();
        let s = muz::compile::lower(
            v.get("__result").unwrap().clone(),
            Path::new("/virtual/song.muz"),
            vec![],
        )
        .unwrap()
        .session;
        let snap = muz::snapshot::PlayableSnapshotV1::capture(&s).unwrap();
        let mut changed = MemoryAssets::default();
        changed.insert(PathBuf::from("/virtual/tone.wav"), wav(), 99);
        let changed_context = HostContext {
            assets: Arc::new(changed),
            ..Default::default()
        };
        changed_context.run(|| assert!(snap.clone().restore_checked().is_err()));
        let restored = snap.restore_checked().unwrap();
        let mut engine = muz::audio::AudioEngine::new(
            &restored,
            muz::audio::AudioConfig {
                sample_rate: 48000.,
                max_frames: 256,
                offline: true,
            },
        )
        .unwrap();
        engine.set_running(true);
        let mut pcm = [0.; 512];
        engine.render_interleaved(&mut pcm, 2).unwrap();
        assert!(pcm.iter().any(|x| *x != 0.));
    });
}
