//! Slurs on a mono `sample()` instrument, against a generated recording: a loud
//! pick burst followed by a quiet looped tone at 1 kHz (root C5).
use std::path::Path;

const RATE: u32 = 48000;
const PERIOD: usize = 48;

fn plucked_sample(path: &Path) {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for i in 0..RATE as usize {
        let level = if i < 240 { 0.9 } else { 0.2 };
        let value = level * (std::f64::consts::TAU * i as f64 / PERIOD as f64).sin();
        writer
            .write_sample((value * 32767.).round() as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
}

/// Left channel of a render of `C5:q D5:q@h` at 120 BPM through `instrument`,
/// after `shape` is applied to the phrase.
fn render(dir: &Path, name: &str, shape: &str, instrument: &str) -> Vec<f32> {
    let source = dir.join(format!("{name}.muz"));
    std::fs::write(
        &source,
        format!(
            r#"use "std/instrument" as i;
let guitar = sample("pluck.wav", {{root: 72, loop: [0.5s, 0.9s], attack_ms: 0, release_ms: 20}});
let line = phrase("C5:q D5:q@h").map_notes(fn(n) => {{velocity: 0.3}}, "tag:h");
song({{tempo: 120, tracks: [track("guitar", {shape}, {instrument})], tail: 0.1}})
"#
        ),
    )
    .unwrap();
    let notes = {
        let session = muz::compile::compile(&source).unwrap().session;
        let muz::model::TrackSource::Midi(midi) = &session.tracks[0].source;
        midi.imported
            .notes
            .iter()
            .map(|n| n.key)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        notes,
        [72, 74],
        "{name}: both written notes stay in the score"
    );
    let output = dir.join(format!("{name}.wav"));
    muz::render::render(&source, &output, None, None, &[], RATE, 256).unwrap();
    hound::WavReader::open(output)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .step_by(2)
        .collect()
}

fn window(samples: &[f32], start: f64, end: f64) -> &[f32] {
    &samples[(start * RATE as f64) as usize..(end * RATE as f64) as usize]
}
fn peak(samples: &[f32], start: f64, end: f64) -> f32 {
    window(samples, start, end)
        .iter()
        .fold(0., |m, x| x.abs().max(m))
}
fn rms(samples: &[f32], start: f64, end: f64) -> f32 {
    let w = window(samples, start, end);
    (w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32).sqrt()
}
/// Mean frequency from rising zero crossings, in cents above the 1 kHz root.
fn cents(samples: &[f32], start: f64, end: f64) -> f64 {
    let w = window(samples, start, end);
    let crossings: Vec<f64> = (1..w.len())
        .filter(|&i| w[i - 1] <= 0. && w[i] > 0.)
        .map(|i| (i - 1) as f64 - w[i - 1] as f64 / (w[i] - w[i - 1]) as f64)
        .collect();
    assert!(crossings.len() > 10, "missing rendered tone");
    let hz = RATE as f64 * (crossings.len() - 1) as f64
        / (crossings.last().unwrap() - crossings.first().unwrap());
    1200. * (hz / 1000.).log2()
}

#[test]
fn a_slurred_sampler_note_continues_the_voice_at_the_new_pitch() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    plucked_sample(&dir.join("pluck.wav"));
    // The second note starts at 0.5 s. A new attack replays the pick burst there.
    let onset = |s: &[f32]| peak(s, 0.49, 0.56) / peak(s, 0.3, 0.45);

    let slurred = render(dir, "slurred", r#"i.slur(line, "h")"#, "i.mono(guitar)");
    assert!(
        onset(&slurred) < 1.1,
        "slur re-attacked: {}",
        onset(&slurred)
    );
    let pitch = cents(&slurred, 0.51, 0.95);
    assert!((pitch - 200.).abs() < 2., "slurred pitch {pitch} cents");
    assert!(cents(&slurred, 0.1, 0.49).abs() < 2.);
    // The picked note's level carries on; the slurred note's velocity is unused.
    let level = rms(&slurred, 0.6, 0.9) / rms(&slurred, 0.2, 0.45);
    assert!((level - 1.).abs() < 0.03, "level changed by {level}");
    // Release follows the slurred note's end, not the picked note's.
    assert!(peak(&slurred, 1.05, 1.1) < 1e-4);

    // The slur is the overlap: abutting notes on the same instrument re-attack,
    // and so does a polyphonic sampler given the same overlapping notes.
    let picked = render(dir, "picked", "line.gate(1)", "i.mono(guitar)");
    assert!(onset(&picked) > 2.5, "missing attack: {}", onset(&picked));
    let poly = render(dir, "poly", r#"i.slur(line, "h")"#, "guitar");
    assert!(onset(&poly) > 2.5, "missing attack: {}", onset(&poly));
    let restruck = render(
        dir,
        "retrigger",
        r#"i.slur(line, "h")"#,
        "i.mono(guitar, retrigger = true)",
    );
    assert!(
        onset(&restruck) > 2.5,
        "missing attack: {}",
        onset(&restruck)
    );
    assert!((cents(&restruck, 0.6, 0.95) - 200.).abs() < 2.);

    // A slide is the slurred note's own tuning curve, relative to its pitch:
    // it starts at the earlier note and arrives after 300 ms.
    let slid = render(
        dir,
        "slid",
        r#"i.slur(line, "h", slide = 300ms)"#,
        "i.mono(guitar)",
    );
    assert!(onset(&slid) < 1.1);
    let (early, late) = (cents(&slid, 0.5, 0.6), cents(&slid, 0.82, 0.95));
    assert!(early > 10. && early < 80., "slide starts at {early} cents");
    assert!((late - 200.).abs() < 2., "slide ends at {late} cents");

    // The instrument's `glide_ms` moves every slur the same way.
    let glided = render(
        dir,
        "glided",
        r#"i.slur(line, "h")"#,
        "i.mono(guitar, 300ms)",
    );
    assert!(onset(&glided) < 1.1);
    let (early, late) = (cents(&glided, 0.5, 0.6), cents(&glided, 0.82, 0.95));
    assert!(early > 10. && early < 80., "glide starts at {early} cents");
    assert!((late - 200.).abs() < 2., "glide ends at {late} cents");
}

#[test]
fn sampler_voice_mode_names_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    plucked_sample(&dir.path().join("pluck.wav"));
    let source = dir.path().join("mode.muz");
    std::fs::write(
        &source,
        r#"song({tracks:[track("a",note(72),sample("pluck.wav",{voice_mode:"solo"}))]})"#,
    )
    .unwrap();
    let error = muz::compile::compile(&source).unwrap_err().to_string();
    assert!(error.contains("poly, legato or retrigger"), "{error}");
}
