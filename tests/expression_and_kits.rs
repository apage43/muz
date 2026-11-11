//! Synthetic signals protect note identity, bounded performance allocation and kit lifecycle.
fn source(root: &std::path::Path, text: &str) -> std::path::PathBuf {
    let p = root.join("case.muz");
    std::fs::write(&p, text).unwrap();
    p
}
fn samples(path: &std::path::Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}
#[test]
fn voice_expression_is_isolated_and_independent_of_render_block_size() {
    let d = tempfile::tempdir().unwrap();
    let patch = r#"voice_patch("test",{nodes:[{id:"env",op:"adsr",attack:0.001,decay:0.1,sustain:0.7,release:0.05},{id:"osc",op:"osc"},{id:"out",op:"mul",inputs:["env","osc"]}],output:"out"})"#;
    let base = format!(
        r#"song({{tracks:[track("test",stack([phrase("C4:w").express({{volume:0}}),phrase("E4:w").express({{tuning:[[0,0],[1,0.7]],pan:0.8}})]),{patch})],tail:0.1}})"#
    );
    let p = source(d.path(), &base);
    let a = d.path().join("a.wav");
    let b = d.path().join("b.wav");
    muz::render::render(&p, &a, None, None, &[], 48000, 256).unwrap();
    muz::render::render(&p, &b, None, None, &[], 48000, 97).unwrap();
    let a = samples(&a);
    let b = samples(&b);
    assert_eq!(a.len(), b.len());
    assert!(a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-6));
    let p = source(
        d.path(),
        &base.replace("phrase(\"C4:w\").express({volume:0}),", ""),
    );
    let c = d.path().join("c.wav");
    muz::render::render(&p, &c, None, None, &[], 48000, 256).unwrap();
    let c = samples(&c);
    assert!(a.iter().any(|v| v.abs() > 0.01));
    assert!(
        a.iter().zip(&c).all(|(a, c)| (a - c).abs() < 1e-6),
        "one note's volume changed the overlapping voice"
    );
}
#[test]
fn held_finger_and_tonal_anchors_survive_search() {
    let d = tempfile::tempdir().unwrap();
    let p = source(
        d.path(),
        r#"song({tracks:[track("p",stack([phrase("E4:w").gate(1).annotate("all",{finger:3}),phrase("[C4 G4]:q [D4 F4]:q").at(1b)]).hand("right"),synth("bell"),{policy:"piano",strict:true})]})"#,
    );
    let c = muz::compile::compile(&p).unwrap();
    let p = &c.score[0].pattern;
    assert!(c.diagnostics.is_empty());
    assert_eq!(p.notes[0].data["finger"], 3);
    assert!(p.notes[1..].iter().all(|n| n.data["finger"] != 3));
    let p = source(
        d.path(),
        r#"chords("C F G C").tag("last","fixed").voicelead(low=48,high=84)"#,
    );
    let (v, _) = muz::lang::load(&p).unwrap();
    let p = v.pattern().unwrap();
    assert!(p.notes.last().unwrap().tags.contains("fixed"));
    assert_eq!(p.notes.last().unwrap().pitch, 55.);
}
#[test]
fn sampled_round_robins_and_choke_release_are_audible() {
    let d = tempfile::tempdir().unwrap();
    for (name, level) in [("a", 0.1f32), ("b", 0.3)] {
        let mut w = hound::WavWriter::create(
            d.path().join(format!("{name}.wav")),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..48000 {
            w.write_sample(level).unwrap();
        }
        w.finalize().unwrap();
    }
    let p = source(
        d.path(),
        r#"song({tempo:120,tracks:[track("hat",stack([phrase("C4:e C4:e").velocity(1),cc(120,0,at=3/4b)]),sample([{path:"a.wav",velocity:[0.5,1]},{path:"b.wav",velocity:[0.5,1]}],{root:60,one_shot:true,attack_ms:0,gain_db:0}))],tail:0.1})"#,
    );
    let out = d.path().join("out.wav");
    muz::render::render(&p, &out, None, None, &[], 48000, 97).unwrap();
    let x = samples(&out);
    assert!((x[4800 * 2] - 0.1).abs() < 1e-5);
    assert!(
        (x[14000 * 2] - 0.4).abs() < 1e-5,
        "round robin did not choose second zone"
    );
    assert!(x[19000 * 2].abs() < 1e-4, "one-shot voice ignored choke");
}

#[test]
fn voice_graph_control_nodes_preserve_hz_and_filter_units() {
    let d = tempfile::tempdir().unwrap();
    // Equivalent literal and signal-connected controls must produce the same audio.
    // Both frequency and a cutoff above 100 Hz previously hit an amplitude clamp.
    let mut outputs = Vec::new();
    for (name, nodes, hz, cutoff) in [
        ("literal", "", "440", "2400"),
        (
            "connected",
            r#"{id:"hz",op:"frequency"},{id:"cutoff",op:"param",value:2400,min:20,max:10000},"#,
            r#""hz""#,
            r#""cutoff""#,
        ),
    ] {
        let text = format!(
            r#"song({{tracks:[track("test",phrase("A4:q").gate(1),voice_patch("test",{{nodes:[{nodes}{{id:"osc",op:"osc",wave:"saw",hz:{hz}}},{{id:"filter",op:"filter",input:"osc",cutoff:{cutoff}}}],output:"filter"}}))],tail:0.05}})"#
        );
        let p = source(d.path(), &text);
        let out = d.path().join(format!("{name}.wav"));
        muz::render::render(&p, &out, None, None, &[], 48000, 97).unwrap();
        outputs.push(samples(&out));
    }
    assert!(outputs[0].iter().any(|x| x.abs() > 0.01));
    assert_eq!(outputs[0].len(), outputs[1].len());
    assert!(
        outputs[0]
            .iter()
            .zip(&outputs[1])
            .all(|(a, b)| (a - b).abs() < 1e-6)
    );
}

fn kit_song(kit: &str, lanes: &str) -> anyhow::Result<muz::compile::Compiled> {
    let d = tempfile::tempdir()?;
    let p = source(
        d.path(),
        &format!(r#"song({{tracks:[track("d",drums({lanes}),{kit})],tail:0}})"#),
    );
    muz::compile::compile(&p)
}

#[test]
fn bare_kit_diagnoses_unmapped_lane_and_default_kit_fills_in() {
    let mapped = r#"kit("band",{kick:synth("kick")},defaults={})"#;
    assert!(kit_song(mapped, r#"{kick:"X..."}"#).is_ok());
    let error = format!(
        "{:#}",
        kit_song(mapped, r#"{kick:"X...",rimshot:"..x."}"#)
            .err()
            .expect("unmapped lane")
    );
    assert!(
        error.contains("kit has no instrument mapping for voice 'rimshot'"),
        "{error}"
    );
    let filled = r#"kit("band",{kick:synth("kick")})"#;
    assert!(kit_song(filled, r#"{kick:"X...",rimshot:"..x.",ride_bell:"x..."}"#).is_ok());
}

#[test]
fn ride_bell_lane_resolves_to_general_midi_pitch() {
    let c = kit_song("kit()", r#"{ride_bell:"X..."}"#).unwrap();
    assert!(c.diagnostics.is_empty());
    let notes: Vec<_> = c.score.iter().flat_map(|t| t.pattern.notes.iter()).collect();
    assert!(notes.iter().any(|n| n.pitch == 53.));
}

/// Tuning curves of each note, in performed order, resolved for a preset synth.
fn tuning_curves(score: &str) -> anyhow::Result<Vec<Vec<(f32, f32)>>> {
    let d = tempfile::tempdir()?;
    let p = source(d.path(), score);
    let c = muz::compile::compile(&p)?;
    let muz::model::TrackSource::Midi(m) = &c.session.tracks[0].source;
    Ok(m.imported
        .notes
        .iter()
        .map(|n| {
            let program = n.performance.unwrap().expression;
            program.points[..program.len as usize]
                .iter()
                .filter(|p| p.kind == 2)
                .map(|p| (p.phase, p.value))
                .collect()
        })
        .collect())
}
fn one_track(tempo: &str, pattern: &str) -> String {
    format!(
        r#"use "std/gestures" as g; song({{{tempo}tracks:[track("t",{pattern},synth("bell"))],tail:0}})"#
    )
}
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn time_positions_keep_their_clock_time_on_any_note_length() {
    let curve = "{tuning:[[0ms,-2],[150ms,0],[1,0]]}";
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        &format!(r#"phrase("C4:q D4:h").gate(1).express({curve})"#),
    ))
    .unwrap();
    // A quarter is 0.5 s and a half 1 s at 120 BPM: the 150 ms point keeps its time.
    assert_eq!(curves[0], vec![(0., -2.), (0.3, 0.), (1., 0.)]);
    assert_eq!(curves[1], vec![(0., -2.), (0.15, 0.), (1., 0.)]);
    // Another tempo changes the note's length, not the gesture's speed.
    let slow = tuning_curves(&one_track(
        "tempo:60,",
        &format!(r#"note(60,1b).gate(1).express({curve})"#),
    ))
    .unwrap();
    assert!(close(slow[0][1].0, 0.15), "{slow:?}");
    // Merging note data in source keeps the time positions as quantities.
    let merged = tuning_curves(&one_track(
        "tempo:120,",
        &format!(
            r#"note(60,1b).gate(1).express({curve}).map_notes(fn(n) => {{
                let check = assert(n.data.expression.tuning[1][0] == 150ms, "time kept");
                {{data: merge(n.data, {{}})}}
            }})"#
        ),
    ))
    .unwrap();
    assert_eq!(merged[0], curves[0]);
}

#[test]
fn short_notes_end_the_curve_at_the_release() {
    // A sixteenth at 120 BPM lasts 125 ms: the 150 ms point is cut at phase 1 with
    // the interpolated value, and the later points are dropped.
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        r#"note(60,1/4b).gate(1).express({tuning:[[0ms,-2],[150ms,0],[0.5,1],[1,0]]})"#,
    ))
    .unwrap();
    assert_eq!(curves[0].len(), 2, "{curves:?}");
    assert_eq!(curves[0][0], (0., -2.));
    assert!(close(curves[0][1].0, 1.) && close(curves[0][1].1, -2. + 2. / 1.2));
    // A curve whose first point is already past the end holds that value.
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        r#"note(60,1/4b).gate(1).express({tuning:[[200ms,3]]})"#,
    ))
    .unwrap();
    assert_eq!(curves[0], vec![(1., 3.)]);
}

#[test]
fn mixed_positions_are_ordered_after_resolution() {
    let curve = "{tuning:[[0,0],[400ms,1],[0.5,0]]}";
    // On a half note 400 ms is phase 0.4, before the phase-0.5 point.
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        &format!("note(60,2b).gate(1).express({curve})"),
    ))
    .unwrap();
    assert_eq!(curves[0], vec![(0., 0.), (0.4, 1.), (0.5, 0.)]);
    // On a quarter it is phase 0.8, so the phase-0.5 point would go backwards.
    let error = tuning_curves(&one_track(
        "tempo:120,",
        &format!("note(60,1b).gate(1).express({curve})"),
    ))
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("note") && message.contains("not after point 2 (400ms)"),
        "{message}"
    );
}

#[test]
fn beat_plus_clock_positions_resolve_through_the_tempo_map() {
    // 90 BPM: a beat is 2/3 s and the 4-beat note lasts 8/3 s. Clock offsets
    // count from the located beat, before it when negative.
    let curves = tuning_curves(&one_track(
        "tempo:90,",
        r#"note(60,4b).gate(1).express({tuning:[[0,0],[[1/2b,-30ms],1],[1/2b,2],[[1/2b,140ms],3],[1b,4]]})"#,
    ))
    .unwrap();
    let gate = 8. / 3.;
    let expected = [0., 1. / 3. - 0.03, 1. / 3., 1. / 3. + 0.14, 2. / 3.];
    assert_eq!(curves[0].len(), expected.len(), "{curves:?}");
    for (point, (at, value)) in curves[0].iter().zip(expected.iter().zip(0..)) {
        assert!(
            close(point.0, (at / gate) as f32) && point.1 == value as f32,
            "{curves:?}"
        );
    }
    // Beat 0..1 at 120 BPM (0.5 s), then 60 BPM: 3/2 beats fall at 1 s, so
    // 100 ms later is 1.1 s of the 1.5 s note.
    let curves = tuning_curves(&one_track(
        "tempo:120,tempos:[[1b,60]],",
        r#"note(60,2b).gate(1).express({tuning:[[0,0],[[3/2b,100ms],1]]})"#,
    ))
    .unwrap();
    assert!(close(curves[0][1].0, 1.1 / 1.5), "{curves:?}");
    // Past the release the point ends the curve, like other time positions:
    // 3/4 beat + 200 ms is 575 ms into a 500 ms note.
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        r#"note(60,1b).gate(1).express({tuning:[[0ms,0],[[3/4b,200ms],1],[1,0]]})"#,
    ))
    .unwrap();
    assert_eq!(curves[0].len(), 2, "{curves:?}");
    assert!(close(curves[0][1].0, 1.) && close(curves[0][1].1, 0.5 / 0.575));
}

#[test]
fn beat_plus_clock_points_are_ordered_by_resolved_time() {
    let curve = "{tuning:[[1/2b,0],[[1/2b,300ms],1],[1b,2]]}";
    // At 60 BPM: 0.5 s, 0.8 s, then 1 s.
    let curves = tuning_curves(&one_track(
        "tempo:60,",
        &format!("note(60,4b).gate(1).express({curve})"),
    ))
    .unwrap();
    assert_eq!(curves[0], vec![(0.125, 0.), (0.2, 1.), (0.25, 2.)]);
    // At 120 BPM beat 1 (0.5 s) comes before 1/2 beat + 300 ms (0.55 s).
    let error = tuning_curves(&one_track(
        "tempo:120,",
        &format!("note(60,4b).gate(1).express({curve})"),
    ))
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("point 3 (1b)") && message.contains("not after point 2 (0.5b + 300ms)"),
        "{message}"
    );
}

#[test]
fn gestures_start_at_a_beat_inside_a_note() {
    // A bend at the attack and another at the note written half a beat in, each
    // keeping its clock length, at 120 BPM on a 1 s note.
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        r#"note(60,2b).gate(1).express({tuning:g.follow(g.bend(-2,150ms),g.at(1/2b,g.bend(-1,140ms)))}).map_notes(fn(n) => {
            let check = assert(n.data.expression.tuning[3][0] == [1/2b, 140ms], "pair kept");
            {data: merge(n.data, {})}
        })"#,
    ))
    .unwrap();
    let expected = [(0., -2.), (0.15, 0.), (0.25, -1.), (0.39, 0.)];
    assert_eq!(curves[0].len(), expected.len(), "{curves:?}");
    for (point, want) in curves[0].iter().zip(expected) {
        assert!(close(point.0, want.0) && point.1 == want.1, "{curves:?}");
    }
    // The documented examples: a scoop around beat 1, and vibrato from beat 1.
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        r#"note(64,2b).gate(1).express({tuning:[[0,0],[[1b,-30ms],0],[[1b,0ms],-1],[[1b,140ms],0]]})"#,
    ))
    .unwrap();
    assert!(close(curves[0][1].0, 0.47) && close(curves[0][3].0, 0.64), "{curves:?}");
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        r#"note(64,2b).gate(1).express({tuning:g.follow(g.bend(-2,150ms),g.at(1b,g.vibrato(5.5Hz,0.3)))})"#,
    ))
    .unwrap();
    assert!(close(curves[0][2].0, 0.5) && curves[0][2].1 == 0., "{curves:?}");
    // Beat points shift by the offset; phases can't be moved.
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        r#"note(60,2b).gate(1).express({tuning:g.at(1/2b,g.bend(-1,1/2b))})"#,
    ))
    .unwrap();
    assert_eq!(curves[0], vec![(0.25, -1.), (0.5, 0.)]);
    let error = tuning_curves(&one_track(
        "",
        r#"note(60,2b).express({tuning:g.at(1/2b,[[0.5,1]])})"#,
    ))
    .unwrap_err();
    assert!(format!("{error:#}").contains("phases stretch"), "{error:#}");
}

#[test]
fn beat_positions_follow_the_tempo_map_from_the_attack() {
    // Beat 0..1 at 120 BPM (0.5 s), then 60 BPM: a 2-beat note lasts 1.5 s and
    // 1.5 beats from the attack fall at 1 s.
    let curves = tuning_curves(&one_track(
        "tempo:120,tempos:[[1b,60]],",
        r#"note(60,2b).gate(1).express({tuning:[[0,0],[3/2b,1]]})"#,
    ))
    .unwrap();
    assert!(close(curves[0][1].0, 2. / 3.), "{curves:?}");
}

#[test]
fn invalid_time_positions_are_diagnosed() {
    for (curve, expected) in [
        ("[[-10ms,0]]", "must not be negative"),
        ("[[100ms,0],[50ms,1]]", "times must increase"),
        ("[[1bars,0]]", "must be a phase 0..1, a time"),
        ("[[1.5,0]]", "phases must be 0..1"),
        ("[[[140ms,1/4b],0]]", "must be [beats, clock time]"),
        ("[[[-1b,0ms],0]]", "must not be negative"),
        ("[[[0b,-10ms],0]]", "does not resolve within the note"),
    ] {
        let error = tuning_curves(&one_track(
            "",
            &format!("note(60,1b).express({{tuning:{curve}}})"),
        ))
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains(expected), "{curve}: {message}");
    }
    // The point budget counts resolved points: 50 ms steps fit a quarter note
    // but not a whole note.
    let steps = "map(range(40), fn(k) => [k * 50ms, 0])";
    tuning_curves(&one_track(
        "tempo:120,",
        &format!("note(60,1b).gate(1).express({{tuning:{steps}}})"),
    ))
    .unwrap();
    let error = tuning_curves(&one_track(
        "tempo:120,",
        &format!("note(60,4b).gate(1).express({{tuning:{steps}}})"),
    ))
    .unwrap_err();
    assert!(format!("{error:#}").contains("at most 32"), "{error:#}");
}

#[test]
fn gesture_recipes_stay_within_the_point_budget() {
    // A bend then vibrato on an 8 s note: 2 + 30 points, sharing one.
    let curves = tuning_curves(&one_track(
        "tempo:30,",
        r#"note(60,4b).gate(1).express({tuning:g.follow(g.bend(-2,150ms),g.vibrato(5.5Hz,0.3,delay=150ms))})"#,
    ))
    .unwrap();
    let curve = &curves[0];
    assert_eq!(curve.len(), 31, "{curve:?}");
    assert_eq!(curve[0], (0., -2.));
    // First peak a quarter cycle after the bend arrives; last point back at center.
    let peak = (0.15 + 0.25 / 5.5) / 8.;
    assert!(
        close(curve[2].0, peak as f32) && close(curve[2].1, 0.3),
        "{curve:?}"
    );
    let end = curve[30];
    assert!(
        close(end.0, ((0.15 + 14. / 5.5) / 8.) as f32) && close(end.1, 0.),
        "{end:?}"
    );
    // The same gesture on a quarter note is cut at its release.
    let curves = tuning_curves(&one_track(
        "tempo:120,",
        r#"note(60,1b).gate(1).express({tuning:g.follow(g.bend(-2,150ms),g.vibrato(delay=150ms))})"#,
    ))
    .unwrap();
    assert!(curves[0].len() < 31 && curves[0].last().unwrap().0 == 1.);
}

/// A clock-evaluated vibrato costs a fixed few points whatever the note length,
/// leaving the budget to other curves, and swings at its rate in performed time.
#[test]
fn oscillating_expression_costs_points_per_shape_not_per_cycle() {
    let d = tempfile::tempdir().unwrap();
    // 3.2 s held note at 60 BPM, vibrato from 0 s with a 1 s fade, plus two
    // 14-point curves: 28 + base + 2 depth + rate = 32 points.
    let curve = (0..14)
        .map(|i| format!("[{}, {}]", i as f64 / 13.0, 0.5 + (i % 2) as f64 * 0.1))
        .collect::<Vec<_>>()
        .join(",");
    let p = source(
        d.path(),
        &format!(
            r#"use "std/gestures" as g;
song({{tempo: 60, tracks: [track("lead", note("A4", 16/5b).gate(1).express({{
    tuning: g.vibrato_lfo(5.2Hz, 0.4, fade = 1s),
    volume: [{curve}],
    pan: [{curve}]
}}), synth("bell"))], tail: 0}})"#
        ),
    );
    let compiled = muz::compile::compile(&p).unwrap();
    let muz::model::TrackSource::Midi(m) = &compiled.session.tracks[0].source;
    let expression = m.imported.notes[0].performance.unwrap().expression;
    assert_eq!(expression.len, 32);
    let cycles = expression.cycles(2).unwrap();
    assert!((cycles - 5.2 * 3.2).abs() < 0.01, "{cycles}");
    // A quarter cycle after the fade the swing peaks at the full depth.
    let quarter = (16.0 + 0.25) / cycles;
    assert!((expression.value(2, quarter).unwrap() - 0.4).abs() < 1e-3);
    assert!(expression.value(2, quarter + 0.5 / cycles).unwrap() < -0.39);
    // Saved sessions keep the oscillation.
    let saved: muz::expression::Program =
        serde_json::from_value(serde_json::to_value(expression).unwrap()).unwrap();
    assert_eq!(saved.value(2, quarter), expression.value(2, quarter));
    // Source callbacks read the record back with its units.
    let roundtrip = d.path().join("roundtrip.muz");
    std::fs::write(
        &roundtrip,
        r#"use "std/gestures" as g;
let main = note("A4", 4b).express({tuning: g.vibrato_lfo(5.2Hz, 0.4)}).map_notes(fn(n) => {data: n.data}).notes[0].data.expression.tuning.oscillate.rate;"#,
    )
    .unwrap();
    let rate = muz::lang::load(&roundtrip).unwrap().0;
    assert_eq!(format!("{}", rate.display_json()), "\"5.2Hz\"");
}
