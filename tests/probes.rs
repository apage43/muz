//! Probe taps: a voice patch's named signals reach a render without changing
//! what the track sounds like.
use muz::render::{self, RenderOptions};
use std::path::Path;

fn session(dir: &Path, source: &str) -> muz::Session {
    let path = dir.join("song.muz");
    std::fs::write(&path, source).unwrap();
    muz::compile::compile(&path).unwrap().session
}
fn samples(path: &Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}
fn song(patch: &str) -> String {
    format!(
        r#"use "std/signal" as s;
let osc = s.sine();
let p = voice_patch("p", {{output: s.mul([osc, s.noise(), 0.5]), {patch}}});
let kit_voice = voice_patch("k", {{output: s.mul([s.noise(), s.adsr(1ms, 50ms, 0, 5ms)]), {patch}}});
song({{tempo: 120, tail: 0, tracks: [
    track("lead", phrase("C4:h"), p, {{pan: 0.2, gain: -6, chain: [fx("lowpass")], sends: {{room: -3}}}}),
    track("drums", drums({{kick: "x.x.", snare: ".x.x"}}, span = 1b), kit("k", {{kick: kit_voice, snare: kit_voice}}, defaults = {{}}))
], buses: [bus("room", [fx("delay")])]}})"#
    )
}

#[test]
fn a_probe_is_heard_raw_and_leaves_the_mix_alone() {
    let dir = tempfile::tempdir().unwrap();
    let plain = session(dir.path(), &song("lifetime: {envelope: s.adsr(), tail: 0s}"));
    // The probes reference a sounding node, and a node of their own that draws
    // noise: neither changes the track.
    let probed = session(
        dir.path(),
        &song("lifetime: {envelope: s.adsr(), tail: 0s}, probes: {osc: osc, hiss: s.mul([s.noise(), 0.1])}"),
    );
    let mix = |s: &muz::Session, name: &str| {
        let out = dir.path().join(name);
        render::render_with(s.clone(), &out, &RenderOptions::default(), None).unwrap();
        samples(&out)
    };
    assert_eq!(mix(&plain, "plain.wav"), mix(&probed, "probed.wav"));

    let out = dir.path().join("taps");
    let taps = ["lead@osc", "lead@hiss", "drums", "drums.kick@hiss", "master"];
    render::render_taps(
        probed.clone(),
        &out,
        &RenderOptions {
            tap: taps.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        },
    )
    .unwrap();
    let tap = |name: &str| samples(&out.join(format!("{name}.wav")));
    assert_eq!(tap("master"), mix(&plain, "again.wav"), "probes are muted");
    // Raw: no gain, pan, insert or velocity. Both channels carry the probe.
    let osc = tap("lead@osc");
    let peak = osc.iter().fold(0f32, |m, x| m.max(x.abs()));
    assert!((peak - 1.).abs() < 1e-3, "{peak}");
    assert!(osc.chunks(2).all(|f| f[0] == f[1]));
    let hiss = tap("lead@hiss");
    let peak = hiss.iter().fold(0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.05 && peak <= 0.1, "{peak}");
    // A probe copy of a kit voice is not part of the kit's group.
    let mut alone = RenderOptions {
        tap: vec!["drums".into()],
        ..Default::default()
    };
    let group = dir.path().join("group.wav");
    render::render_with(plain.clone(), &group, &alone, None).unwrap();
    assert_eq!(tap("drums"), samples(&group));
    alone.tap = vec!["lead@nope".into()];
    let error = render::render_with(probed.clone(), &group, &alone, None).unwrap_err();
    assert!(format!("{error:#}").contains("track lead has no probe nope"), "{error:#}");
    alone.tap = vec!["lead@osc:post".into()];
    assert!(render::render_with(probed, &group, &alone, None).is_err());
}

#[test]
fn probes_are_declared_in_source_and_listened_to_only_by_a_render() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.muz");
    for (patch, message) in [
        ("listen: \"osc\"", "listen"),
        ("probes: {\"a b\": osc}", "letters, digits and underscores"),
        ("probes: 3", "probes needs a record"),
    ] {
        std::fs::write(
            &path,
            format!(
                r#"use "std/signal" as s; let osc = s.sine();
song({{tracks: [track("t", note(60, 1b), voice_patch("p", {{output: osc, {patch}}}))]}})"#
            ),
        )
        .unwrap();
        let error = format!("{:#}", muz::compile::compile(&path).err().unwrap());
        assert!(error.contains(message), "{patch}: {error}");
    }
}
