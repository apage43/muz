//! Measured amplitude contracts, isolated from musical pieces and preset timbres.
const RATE: usize = 48_000;

fn bounce(instrument: &str, hold: f64, controls: &str) -> Vec<f32> {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("envelope.muz");
    let audio = dir.path().join("envelope.wav");
    std::fs::write(
        &source,
        format!(
            r#"song({{tempo:120,tracks:[track("probe",
                stack([note("C4",4b,velocity=1).gate({}),{controls}]),
                {instrument})],tail:0.25}})"#,
            hold / 2.0
        ),
    )
    .unwrap();
    muz::render::render(&source, &audio, None, None, &[], RATE as u32, 97).unwrap();
    hound::WavReader::open(audio)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

fn voice(graph: bool, attack: f64, decay: f64, sustain: f64, release: f64) -> String {
    if graph {
        // The envelope itself is a synthetic DC probe, never a listening example.
        format!(
            r#"voice_patch("probe",{{gain_db:0,nodes:[
                {{id:"env",op:"adsr",attack:{attack},decay:{decay},sustain:{sustain},release:{release}}}
            ],output:"env"}})"#
        )
    } else {
        // With filter motion off, a unity-envelope reference has identical noise
        // and filter history. Their sample ratio measures only amplitude response.
        format!(
            r#"synth("init",{{mode:"noise",gain_db:0,filter_env:0,
                attack_ms:{},decay_ms:{},sustain:{sustain},release_ms:{}}})"#,
            attack * 1000.0,
            decay * 1000.0,
            release * 1000.0
        )
    }
}

fn level(audio: &[f32], unity: &[f32], frame: usize) -> f32 {
    // Use the stronger reference channel to avoid a near-zero noise divisor.
    let i = frame * 2 + usize::from(unity[frame * 2 + 1].abs() > unity[frame * 2].abs());
    assert!(unity[i].abs() > 1e-8);
    audio[i] / unity[i]
}

fn close(actual: f32, expected: f32, tolerance: f32) {
    assert!(
        (actual - expected).abs() < tolerance,
        "expected {expected}, measured {actual}"
    );
}

#[test]
fn graph_and_preset_attack_decay_sustain_and_release_are_measurable() {
    for graph in [true, false] {
        let unity = bounce(&voice(graph, 0.0, 0.1, 1.0, 0.1), 2.0, "");
        let shaped = voice(graph, 0.1, 0.1, 0.25, 0.1);
        let held = bounce(&shaped, 0.8, "");
        close(level(&held, &unity, 0), 0.0, 1e-6);
        close(level(&held, &unity, RATE / 20), 0.5, 1e-6);
        close(level(&held, &unity, RATE / 10), 1.0, 1e-6);
        close(
            level(&held, &unity, RATE / 5),
            if graph { 0.25505346 } else { 0.5259096 },
            1e-6,
        );
        close(
            level(&held, &unity, RATE * 3 / 5),
            if graph { 0.25 } else { 0.25505346 },
            1e-6,
        );
        let zero_sustain = bounce(&voice(graph, 0.0, 0.1, 0.0, 0.1), 2.0, "");
        close(level(&zero_sustain, &unity, 0), 1.0, 1e-6);
        close(
            level(&zero_sustain, &unity, RATE / 10),
            if graph { 0.006737947 } else { 0.36787945 },
            1e-6,
        );

        // Release interrupts attack, decay, or a long key hold at the attained
        // level. R is a relative attenuation time, not the retirement deadline.
        for hold in [0.05, 0.2, 0.8] {
            let audio = bounce(&shaped, hold, "");
            let off = (hold * RATE as f64).round() as usize;
            let before = level(&audio, &unity, off - 1);
            let first = level(&audio, &unity, off);
            assert!(first < before && first > before * 0.99);
            let after_r = level(&audio, &unity, off + RATE / 10 - 1);
            close(after_r / before, if graph { 0.0001 } else { 0.001 }, 3e-7);
            assert!(after_r > 0.0, "voice retired at the release parameter time");
            assert!(audio[(off + RATE / 5) * 2..].iter().all(|x| *x == 0.0));
        }
    }
}

#[test]
fn translating_decay_and_release_matches_graph_and_preset_envelopes() {
    let graph_unity = bounce(&voice(true, 0.0, 0.2, 1.0, 0.3), 2.0, "");
    let preset_unity = bounce(&voice(false, 0.0, 0.2, 1.0, 0.3), 2.0, "");
    let graph = bounce(&voice(true, 0.01, 1.0, 0.2, 0.3 * 9.21 / 6.9078), 0.25, "");
    let preset = bounce(&voice(false, 0.01, 0.2, 0.2, 0.3), 0.25, "");
    for frame in (1..RATE * 3 / 4).step_by(31) {
        close(
            level(&graph, &graph_unity, frame),
            level(&preset, &preset_unity, frame),
            2e-6,
        );
    }
}

#[test]
fn percussion_presets_ignore_gate_and_adsr_overrides_but_obey_choke() {
    for mode in ["kick", "snare", "cymbal", "fm_percussion"] {
        let instrument = format!(
            r#"synth("init",{{mode:"{mode}",fm_index:0,decay_ms:100,attack_ms:0,sustain:0,release_ms:1}})"#
        );
        let held = bounce(&instrument, 2.0, "");
        // Disable FM-index motion so changing D changes only amplitude. Doubling
        // the time constant gives these amplitude ratios at 100 and 200 ms.
        let slower = bounce(&instrument.replace("decay_ms:100", "decay_ms:200"), 2.0, "");
        close(level(&held, &slower, RATE / 10), 0.60653066, 1e-6);
        close(level(&held, &slower, RATE / 5), 0.36787945, 1e-6);
        let short = bounce(&instrument, 0.05, "");
        assert_eq!(held, short, "{mode}: note-off interrupted percussion decay");
        let overrides = instrument.replace(
            "attack_ms:0,sustain:0,release_ms:1",
            "attack_ms:500,sustain:1,release_ms:500",
        );
        assert_eq!(held, bounce(&overrides, 2.0, ""), "{mode}: ADSR override");
        assert!(held[RATE / 5..RATE / 2].iter().any(|x| x.abs() > 0.001));
        // A 100 ms time constant crosses the 1e-5 retirement threshold near 1.15s.
        assert!(held[RATE * 6 / 5 * 2..].iter().all(|x| *x == 0.0));
        let choked = bounce(&instrument, 2.0, "cc(120,0,at=1/5b)");
        assert_eq!(&held[..RATE / 10 * 2], &choked[..RATE / 10 * 2]);
        assert!(choked[RATE * 12 / 100 * 2..].iter().all(|x| *x == 0.0));
    }
}
