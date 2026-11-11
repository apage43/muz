//! An SFZ instrument's random draws (`amp_random`, `lorand`/`hirand`, ...) are
//! keyed by its seed and each note's own identity, not by a running count of
//! events, so an edit elsewhere on the track does not re-roll later notes.
//! Generated constant recordings make a note's level name its draw.
use muz::{
    audio::{AudioConfig, AudioEngine},
    description::ValidatedSession,
    render::{self, RenderOptions},
};
use std::path::Path;

const BEAT: usize = 24000;
const BEATS: usize = 8;
/// Eight C4 eighths, one per beat.
const LEAD: &str =
    r#"phrase("C4:e r:e C4:e r:e C4:e r:e C4:e r:e C4:e r:e C4:e r:e C4:e r:e C4:e r:e")"#;
/// The same lead with an earlier sounding note and two notes that match no
/// region (as keyswitch notes do), all ending well before beat 2.
const WITH_EXTRA: &str = r#"stack([phrase("C4:e r:e C4:e r:e C4:e r:e C4:e r:e C4:e r:e C4:e r:e C4:e r:e C4:e r:e"),
    note(30, 0.1b, at=0.5b), note("E4", 0.25b, at=1b), note(31, 0.1b, at=1.5b)])"#;

fn program() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, level) in [("low.wav", 0.25), ("high.wav", 0.5)] {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(dir.path().join(name), spec).unwrap();
        for _ in 0..48000 {
            writer.write_sample(level).unwrap();
        }
        writer.finalize().unwrap();
    }
    std::fs::write(
        dir.path().join("draws.sfz"),
        "<control> set_cc7=127 set_cc11=127\n\
         <group> lokey=40 hikey=84 ampeg_attack=0 ampeg_release=0.001 amp_veltrack=0 amp_random=6\n\
         <region> lorand=0 hirand=0.5 sample=low.wav\n\
         <region> lorand=0.5 hirand=1 sample=high.wav\n",
    )
    .unwrap();
    dir
}

fn session(dir: &Path, pattern: &str, seed: u64) -> muz::Session {
    let path = dir.join("song.muz");
    std::fs::write(
        &path,
        format!(
            r#"song({{tempo: 120, tail: 0, sections: [section("a", 4b), section("b", 4b)],
    tracks: [track("lead", {pattern}, sfz("draws.sfz", {{seed: {seed}}}))]}})"#
        ),
    )
    .unwrap();
    muz::compile::compile(&path).unwrap().session
}

fn config(max_frames: usize) -> AudioConfig {
    AudioConfig {
        sample_rate: 48000.0,
        max_frames,
        offline: true,
    }
}

/// Left channel of `frames` frames rendered in blocks of `block` frames.
fn pull(engine: &mut AudioEngine, frames: usize, block: usize) -> Vec<f32> {
    let mut left = Vec::with_capacity(frames);
    let mut buffer = vec![0.0; block * 2];
    while left.len() < frames {
        let n = (frames - left.len()).min(block);
        engine.render_interleaved(&mut buffer[..n * 2], 2).unwrap();
        left.extend(buffer[..n * 2].chunks(2).map(|frame| frame[0]));
    }
    left
}

fn left(session: &muz::Session, block: usize) -> Vec<f32> {
    let checked = ValidatedSession::new(session).unwrap();
    let mut engine = AudioEngine::from_validated(&checked, config(block)).unwrap();
    engine.set_running(true);
    pull(&mut engine, BEATS * BEAT, block)
}

/// Mean level in the middle of each beat's note.
fn levels(left: &[f32]) -> Vec<f32> {
    (0..BEATS)
        .map(|beat| {
            let window = &left[beat * BEAT + 2000..beat * BEAT + 10000];
            window.iter().sum::<f32>() / window.len() as f32
        })
        .collect()
}

#[test]
fn removing_earlier_notes_leaves_later_draws_bit_identical() {
    let dir = program();
    let with = left(&session(dir.path(), WITH_EXTRA, 7), 256);
    let without = left(&session(dir.path(), LEAD, 7), 256);
    // Everything from beat 2 on, after the removed notes have fully decayed.
    assert!(with[2 * BEAT..] == without[2 * BEAT..]);
    // The removed E4 did sound, so the comparison is not vacuous.
    assert_ne!(with[BEAT..2 * BEAT], without[BEAT..2 * BEAT]);
}

#[test]
fn repeated_notes_vary_and_seeds_repeat_or_differ() {
    let dir = program();
    let seven = levels(&left(&session(dir.path(), LEAD, 7), 256));
    assert!(seven.iter().all(|level| *level > 0.2), "{seven:?}");
    let mut distinct = seven.clone();
    distinct.sort_by(f32::total_cmp);
    distinct.dedup();
    assert!(
        distinct.len() >= 6,
        "identical notes repeat draws: {seven:?}"
    );
    assert_eq!(levels(&left(&session(dir.path(), LEAD, 7), 256)), seven);
    let eight = levels(&left(&session(dir.path(), LEAD, 8), 256));
    assert!(
        seven.iter().zip(&eight).filter(|(a, b)| a != b).count() >= 6,
        "{seven:?} {eight:?}"
    );
}

#[test]
fn draws_are_block_size_invariant() {
    let dir = program();
    let s = session(dir.path(), WITH_EXTRA, 7);
    let reference = left(&s, 256);
    for block in [1, 97, 128] {
        assert!(left(&s, block) == reference, "block {block}");
    }
}

#[test]
fn seeks_sections_and_bounded_history_match_the_full_render() {
    let dir = program();
    let s = session(dir.path(), WITH_EXTRA, 7);
    let full = left(&s, 256);

    // A prepared seek to beat 4 replays history and continues identically.
    let checked = ValidatedSession::new(&s).unwrap();
    let ppq = muz::compile::PPQ as u64;
    let mut seek =
        AudioEngine::prepare_seek(&checked, config(256), 4 * ppq, 4 * BEAT as u64).unwrap();
    assert!(pull(&mut seek, 4 * BEAT, 256) == full[4 * BEAT..]);

    // Section renders, with full history and with a bounded history that
    // starts after the removed-note region (so its notes are not replayed).
    for history in [None, Some(0.5)] {
        let out = dir.path().join("section.wav");
        render::render_with(
            s.clone(),
            &out,
            &RenderOptions {
                section: Some("b".into()),
                history,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let section: Vec<f32> = hound::WavReader::open(&out)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .step_by(2)
            .collect();
        std::fs::remove_file(&out).unwrap();
        assert!(
            section[..4 * BEAT] == full[4 * BEAT..],
            "section with history {history:?}"
        );
    }
}
