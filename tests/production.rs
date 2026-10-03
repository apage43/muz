use muz::render;
fn bounce(src: &str, block: usize) -> Vec<f32> {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("case.muz");
    let w = dir.path().join("out.wav");
    std::fs::write(&p, src).unwrap();
    render::render(&p, &w, None, None, &[], 48000, block).unwrap();
    hound::WavReader::open(w)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}
#[test]
fn parallel_latency_is_aligned_and_trimmed() {
    let single = bounce(
        "song({tracks:[track(\"a\",phrase(\"C4:e E4:e\"),synth(\"bell\"))],tail:0.4})",
        256,
    );
    let parallel = bounce(
        "song({tracks:[track(\"a\",phrase(\"C4:e E4:e\"),synth(\"bell\"),{gain:-6.020599913}),track(\"b\",phrase(\"C4:e E4:e\"),synth(\"bell\"),{gain:-6.020599913,chain:[fx(\"limiter\",{lookahead_ms:7,ceiling_db:0})]})],tail:0.4})",
        97,
    );
    assert_eq!(single.len(), parallel.len());
    let error = single
        .iter()
        .zip(&parallel)
        .map(|(a, b)| (a - b).abs())
        .fold(0., f32::max);
    assert!(
        error < 0.0001,
        "parallel route comb filtering or untrimmed latency: {error}"
    );
    let rack = bounce(
        r#"song({tracks:[track("a",phrase("C4:e E4:e"),synth("bell"),{chain:[rack([[fx("gain",{gain_db:-6.020599913})],[fx("gain",{gain_db:-6.020599913}),fx("limiter",{lookahead_ms:7,ceiling_db:0})]])]})],tail:0.4})"#,
        97,
    );
    let error = single
        .iter()
        .zip(rack)
        .map(|(a, b)| (a - b).abs())
        .fold(0., f32::max);
    assert!(
        error < 0.0001,
        "rack branches were not compensated: {error}"
    );
}

#[test]
fn simultaneous_stems_match_individual_taps_with_latency() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("case.muz");
    std::fs::write(&p,r#"song({tracks:[track("a",phrase("C4:e E4:e"),synth("bell"),{chain:[fx("limiter",{lookahead_ms:7})]}),track("b",phrase("E4:q"),synth("bell"))],tail:0.2})"#).unwrap();
    let s = muz::compile::compile(&p).unwrap().session;
    let options = render::RenderOptions::default();
    let reports = render::stems(s.clone(), &d.path().join("stems"), &options).unwrap();
    for id in ["a", "b"] {
        let out = d.path().join(format!("{id}.wav"));
        render::render_with(
            s.clone(),
            &out,
            &render::RenderOptions {
                tap: Some(id.into()),
                ..options.clone()
            },
            None,
        )
        .unwrap();
        let samples = |p: &std::path::Path| {
            hound::WavReader::open(p)
                .unwrap()
                .samples::<f32>()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            samples(&out),
            samples(&d.path().join(format!("stems/{id}.wav")))
        );
    }
    assert_eq!(reports.len(), 2);
}

#[test]
fn tempo_mapped_sections_and_dry_stems_preserve_timing_and_history() {
    fn samples(path: &std::path::Path) -> Vec<f32> {
        hound::WavReader::open(path)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect()
    }
    fn assert_audio(actual: &[f32], expected: &[f32], context: &str) {
        assert_eq!(actual.len(), expected.len(), "{context}: output length");
        let error = actual
            .iter()
            .zip(expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0., f32::max);
        assert!(
            error < 1e-5,
            "{context}: audio/timing/history error {error}"
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sections.muz");
    std::fs::write(
        &path,
        r#"
        let held = stack([
            note("C3",7/4b),
            note("E3",3/2b).at(9/4b),
            note("G3",3/2b).at(17/4b)
        ]).gate(1);
        let echoes = stack([
            note("C5",1/4b),
            note("E5",1/4b).at(5/2b),
            note("G5",1/4b).at(9/2b)
        ]).gate(1);
        song({
            tempo:120,tempos:[[1b,60],[2b,120],[3b,240],[4b,60],[5b,120],[6b,90]],
            sections:[section("opening",2b),section("middle",2b),section("terminal",2b)],
            tracks:[
                track("held",held,synth("init",{sustain:1,release_ms:800}),{
                    gain:-6,chain:[fx("limiter",{lookahead_ms:7,ceiling_db:0})]
                }),
                track("echoes",echoes,synth("init",{sustain:1,release_ms:800}),{
                    chain:[fx("delay",{time_beats:1,feedback:0.75,mix:1})]
                })
            ],tail:0.125
        })
        "#,
    )
    .unwrap();
    let session = muz::compile::compile(&path).unwrap().session;
    // Independent integration of the conductor map: 0..2 = 1.5s,
    // 2..4 = 0.75s, 4..6 = 1.5s. Each boundary also changes tempo.
    for (name, end_beat, start, musical_seconds) in [
        ("opening", 2., 0., 1.5),
        ("middle", 4., 1.5, 0.75),
        ("terminal", 6., 2.25, 1.5),
    ] {
        let duration = musical_seconds + 0.125;
        let frames = (duration * 48000.) as u64;
        let options = render::RenderOptions {
            section: Some(name.into()),
            block_size: 97,
            ..Default::default()
        };
        let out = dir.path().join(format!("{name}.wav"));
        let report = render::render_with(session.clone(), &out, &options, None).unwrap();
        assert_eq!(report.frames, frames, "{name}: tempo-integrated duration");
        assert_eq!(report.seconds, duration);
        assert_eq!(report.track_meters.len(), 2);
        let actual = samples(&out);
        assert_eq!(actual.len(), frames as usize * 2);

        // A clock-time reference keeps the complete, valid conductor timeline,
        // but removes notes belonging to subsequent sections. No notes cross
        // the cut, and the tail ends before the next tempo event. This avoids
        // the section preparation path while retaining all preceding history.
        let cut = muz::compile::tick(end_beat);
        let mut reference = session.clone();
        for track in &mut reference.tracks {
            let muz::model::TrackSource::Midi(midi) = &mut track.source else {
                panic!("expected MIDI performance")
            };
            assert!(midi.imported.controllers.is_empty());
            assert!(midi.imported.messages.is_empty());
            midi.imported.notes.retain(|n| n.start_tick < cut);
            assert!(
                midi.imported
                    .notes
                    .iter()
                    .all(|n| n.start_tick + n.duration_ticks <= cut)
            );
            midi.imported.summary.notes = midi.imported.notes.len() as u32;
            midi.imported.summary.events =
                midi.imported.summary.notes * 2 + midi.imported.summary.tempos;
            midi.summary = midi.imported.summary.clone();
        }
        let clock_options = render::RenderOptions {
            start: Some(start),
            seconds: Some(duration),
            section: None,
            ..options.clone()
        };
        let expected_path = dir.path().join(format!("{name}-clock.wav"));
        render::render_with(reference.clone(), &expected_path, &clock_options, None).unwrap();
        assert_audio(&actual, &samples(&expected_path), name);

        let stem_dir = dir.path().join(format!("{name}-stems"));
        let reports = render::stems(session.clone(), &stem_dir, &options).unwrap();
        assert_eq!(reports.len(), 2);
        for (index, id) in ["held", "echoes"].iter().enumerate() {
            assert_eq!(reports[index].frames, frames);
            assert_eq!(reports[index].seconds, duration);
            let stem = samples(&stem_dir.join(format!("{id}.wav")));
            let tap_path = dir.path().join(format!("{name}-{id}-clock.wav"));
            let tap_options = render::RenderOptions {
                tap: Some((*id).into()),
                ..clock_options.clone()
            };
            render::render_with(reference.clone(), &tap_path, &tap_options, None).unwrap();
            assert_audio(&stem, &samples(&tap_path), &format!("{name}/{id}"));
            if name != "opening" {
                // New notes start at least 0.125 beats into these sections.
                // The first 50ms therefore contains preceding voices/echoes,
                // not newly started material from a cold or rebased engine.
                assert!(
                    stem[..4800].iter().any(|x| x.abs() > 1e-4),
                    "{name}/{id}: preceding instrument/effect history lost"
                );
            }
            if *id == "echoes" {
                // A tempo exactly at the cut is musically meaningful during
                // release: removing it changes the beat-synchronized delay.
                let mut missing_boundary = reference.clone();
                for track in &mut missing_boundary.tracks {
                    let muz::model::TrackSource::Midi(midi) = &mut track.source else {
                        unreachable!()
                    };
                    midi.imported.tempos.retain(|t| t.tick != cut);
                    midi.imported.summary.tempos -= 1;
                    midi.imported.summary.events -= 1;
                    midi.summary = midi.imported.summary.clone();
                }
                let wrong_path = dir.path().join(format!("{name}-missing-boundary.wav"));
                render::render_with(missing_boundary, &wrong_path, &tap_options, None).unwrap();
                let wrong = samples(&wrong_path);
                let tail_start = (musical_seconds * 48000.) as usize * 2;
                assert!(
                    stem[tail_start..]
                        .iter()
                        .zip(&wrong[tail_start..])
                        .any(|(a, b)| (a - b).abs() > 1e-4),
                    "{name}: end-boundary tempo did not affect release audio"
                );
            }
        }
    }
}
#[test]
fn sidechain_and_automation_do_not_depend_on_buffer_boundaries() {
    let src = r#"song({tempo:120, tracks:[track("detector",phrase("C2:s r:e C2:s r:q"),synth("kick"),{gain:-120}),track("pad",phrase("[C4 E4 G4]:h"),synth("pad"),{chain:[fx("compressor",{id:"duck",sidechain:"detector",threshold_db:-35,ratio:8,attack_ms:2,release_ms:70}),fx("eq",{id:"tone",frequency_hz:1000,gain_db:3})],sends:{echo:-120}})], buses:[bus("echo",[fx("delay",{time_beats:0.25,mix:1})])],automation:[automation("pad.tone.gain_db",curve([[0b,-4],[1/2b,6],[2b,-2]])),automation("pad.send.echo",curve([[0b,-120],[1/2b,-6],[1b,-120]],"step"))],tail:0.5})"#;
    let a = bounce(src, 256);
    let b = bounce(src, 97);
    assert_eq!(a.len(), b.len());
    let error = a
        .iter()
        .zip(&b)
        .map(|(a, b)| (a - b).abs())
        .fold(0., f32::max);
    assert!(error < 0.0001, "buffer dependent dynamics: {error}");
}

#[test]
fn tagged_notes_automate_existing_send_gain_at_performed_times() {
    let src = r#"
        use "std/mix" as mix;
        let trigger = phrase("E5:q").gate(0.5).tag("all","echo")
            .refine("all",{offset_ms:25});
        let material = stack([phrase("C3:w"),trigger.at(1b),trigger.at(3/2b),trigger.at(3b)]);
        song({tempo:120,tracks:[track("p",material,synth("bell"),{
            policy:"piano",strict:true,sends:{echo:-120}
        })],buses:[bus("echo",[fx("delay",{time_beats:0.25,mix:1})])],
        automation:[mix.throws(material,"echo","p.send.echo",-12,tail=180ms)],tail:0.5})
    "#;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("tagged.muz");
    std::fs::write(&p, src).unwrap();
    let c = muz::compile::compile(&p).unwrap();
    // Tags drive automation without creating extra instrument performances.
    assert_eq!(c.session.tracks.len(), 1);
    let muz::model::TrackSource::Midi(midi) = &c.session.tracks[0].source else {
        panic!("expected MIDI performance")
    };
    assert_eq!(midi.imported.notes.len(), c.score[0].pattern.notes.len());
    let lane = &c.session.extras.automation[0];
    assert_eq!(lane.target, "p.send.echo");
    for (at, gain) in [
        (0.52, -120.),
        (0.53, -12.),
        (0.96, -12.),
        (1.21, -120.),
        (1.53, -12.),
        (1.96, -120.),
    ] {
        assert_eq!(lane.value_at(at), gain, "send gain at {at}s");
    }
    // Explicit mix automation is the audio reference, including the overlapping
    // untagged bass. Adjacent note windows merge through the first two triggers.
    let reference = src.replace(
        "automation:[mix.throws(material,\"echo\",\"p.send.echo\",-12,tail=180ms)]",
        "automation:[automation(\"p.send.echo\",curve([[0s,-120],[0.525s,-12],[1.205s,-120],[1.525s,-12],[1.955s,-120]],\"step\"))]"
    );
    let actual = bounce(src, 97);
    let expected = bounce(&reference, 256);
    assert_eq!(actual.len(), expected.len());
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(a, b)| (a - b).abs() < 1e-5)
    );
    let closed = bounce(
        &src.replace(
            "automation:[mix.throws(material,\"echo\",\"p.send.echo\",-12,tail=180ms)]",
            "automation:[]",
        ),
        256,
    );
    assert!(
        actual.iter().zip(closed).any(|(a, b)| (a - b).abs() > 1e-3),
        "send never opened"
    );
}

#[test]
fn composer_functions_derive_arbitrary_automation_across_tempo_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("custom.muz");
    std::fs::write(
        &path,
        r#"
        let timing = {tempo:120,tempos:[[2b,60]]};
        let material = phrase("C4:q E4:q").at(3/2b).gate(0.8).velocity(0.5)
            .refine("all",{offset_ms:25}).tag("all","shape").annotate("all",{peak:4000});
        fn gesture(notes, target) {
            let points = fold(sort_by(notes,fn(n)=>n.at),[[0s,300]],fn(ps,n)=>ps+[
                [seconds_at(n.at,timing)+n.offset,300],
                [seconds_at(n.at+n.duration*n.gate/2,timing)+n.offset,n.data.peak*n.velocity],
                [seconds_at(n.at+n.duration*n.gate,timing)+n.offset+n.release_offset,300]
            ]);
            automation(target,curve(points,"smooth"))
        }
        song(merge(timing, {tracks:[track("lead",material,synth("bell"),{
            chain:[fx("lowpass",{id:"tone",cutoff_hz:300})]
        })],automation:[gesture(material.select("shape").notes,"lead.tone.cutoff_hz")],tail:0.2}))
    "#,
    )
    .unwrap();
    let c = muz::compile::compile(&path).unwrap();
    let lane = &c.session.extras.automation[0];
    for (at, value) in [
        (0.775, 300.),
        (0.975, 2000.),
        (1.325, 300.),
        (1.525, 300.),
        (1.925, 2000.),
        (2.325, 300.),
    ] {
        assert!((lane.value_at(at) - value).abs() < 1e-3, "gesture at {at}s");
    }
    let muz::model::TrackSource::Midi(midi) = &c.session.tracks[0].source else {
        panic!("expected notes")
    };
    for (note, points) in midi
        .imported
        .notes
        .iter()
        .zip(lane.points[1..].as_chunks::<3>().0.iter())
    {
        let attack = muz::compile::seconds_at(
            note.start_tick as f64 / muz::compile::PPQ as f64,
            &midi.imported.tempos,
        );
        let release = muz::compile::seconds_at(
            (note.start_tick + note.duration_ticks) as f64 / muz::compile::PPQ as f64,
            &midi.imported.tempos,
        );
        assert!((attack - points[0].seconds).abs() < 1e-6);
        assert!((release - points[2].seconds).abs() < 1e-6);
    }
    render::render_with(
        c.session,
        &dir.path().join("gesture.wav"),
        &Default::default(),
        None,
    )
    .unwrap();
}
