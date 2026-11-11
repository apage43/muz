//! A lane may name a device inside a rack branch by its id, as it names a
//! device in a track's own chain. Compile lowers it onto the rack's `expose`.
fn render(source: &str) -> anyhow::Result<Vec<f32>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("rack.muz");
    let wav = dir.path().join("rack.wav");
    std::fs::write(&path, source)?;
    muz::render::render(&path, &wav, None, None, &[], 48000, 256)?;
    Ok(hound::WavReader::open(wav)?
        .samples::<f32>()
        .map(Result::unwrap)
        .collect())
}

fn song(rack_options: &str, target: &str) -> String {
    format!(
        r#"song({{
        tracks: [track("bass", phrase("C2:h E2:h"), synth("bell"), {{chain: [
            fx("gain", {{gain_db: -2}}),
            rack([[fx("lowpass", {{cutoff_hz: 300}})],
                  [fx("highpass", {{cutoff_hz: 300}}), fx("gain", {{id: "mid_level", gain_db: 0}})]]{rack_options})
        ]}})],
        automation: [automation("{target}", curve([[0b, 0], [2b, -30], [4b, -6]]))],
        tail: 0.2
    }})"#
    )
}

#[test]
fn branch_device_ids_automate_like_an_explicit_exposure() {
    let explicit = render(&song(r#", {expose: {mid: "1.1.gain_db"}}"#, "bass.fx1.mid")).unwrap();
    let short = render(&song("", "bass.mid_level.gain_db")).unwrap();
    let full = render(&song("", "bass.fx1.1.mid_level.gain_db")).unwrap();
    assert_eq!(explicit, short);
    assert_eq!(explicit, full);
    let unautomated = render(&song("", "bass.out").replace("-30", "0").replace("-6", "0")).unwrap();
    assert_ne!(explicit, unautomated);
}

#[test]
fn branch_lanes_report_unknown_controls_and_are_listed_as_targets() {
    let error = render(&song("", "bass.mid_level.gain")).unwrap_err();
    let text = format!("{error:?}");
    assert!(text.contains("has no automatable control 'gain'"), "{text}");
    assert!(text.contains("gain_db"), "{text}");

    let error = render(&song("", "bass.mid_levl.gain_db")).unwrap_err();
    let text = format!("{error:?}");
    assert!(text.contains("bass.mid_level.gain_db"), "{text}");
}

#[test]
fn a_modulated_branch_control_cannot_also_take_a_lane() {
    let options = r#", {modulate: [{target: "1.1.gain_db", base: -6, min: -12, max: 0}]}"#;
    let error = render(&song(options, "bass.mid_level.gain_db")).unwrap_err();
    assert!(format!("{error:?}").contains("driven by a rack modulator"));
}
