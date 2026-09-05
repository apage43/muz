fn compile(body: &str) -> anyhow::Result<muz::compile::Compiled> {
    let d = tempfile::tempdir()?;
    let path = d.path().join("preferences.muz");
    std::fs::write(&path, format!("use \"std/performance\" as perf;\n{body}"))?;
    muz::compile::compile(&path)
}
fn one_note(options: &str) -> anyhow::Result<muz::compile::Compiled> {
    compile(&format!(
        r#"song({{tracks:[track("p",note("C4",1b),synth("bell"),{options})],tail:0}})"#
    ))
}
#[test]
fn source_hand_preferences_change_advice_without_overriding_anchors() {
    for (centers, hand) in [("[60,100]", "left"), ("[20,60]", "right")] {
        let c=one_note(&format!(r#"{{policy:"piano",fingering:false,playing:merge(perf.piano_preferences,{{hand_centers:{centers}}})}}"#)).unwrap();
        assert_eq!(c.score[0].pattern.notes[0].hand.as_deref(), Some(hand));
        let c=compile(&format!(r#"song({{tracks:[track("p",note("C4",1b).hand("left"),synth("bell"),{{policy:"piano",fingering:false,playing:merge(perf.piano_preferences,{{hand_centers:{centers}}})}})]}})"#)).unwrap();
        assert_eq!(c.score[0].pattern.notes[0].hand.as_deref(), Some("left"));
    }
    let c=compile(r#"song({tracks:[track("p",note("C4",1b).hands(preferences=merge(perf.piano_preferences,{hand_centers:[60,100]})),synth("bell"))]})"#).unwrap();
    assert_eq!(c.score[0].pattern.notes[0].hand.as_deref(), Some("left"));
}
#[test]
fn source_pitch_class_costs_cover_every_black_key_and_keep_finger_anchors() {
    for pitch in [60, 61, 63, 66, 68, 70] {
        let song = |anchor: &str| {
            format!(
                r#"let preferences=merge(perf.piano_preferences,{{finger_positions:[[{pitch},{pitch},{pitch},{pitch},{pitch}],[{pitch},{pitch},{pitch},{pitch},{pitch}]]}});
 song({{tracks:[track("p",note({pitch},1b).hand("right"){anchor},synth("bell"),{{policy:"piano",playing:preferences}})],tail:0}})"#
            )
        };
        let c = compile(&song("")).unwrap();
        let finger = c.score[0].pattern.notes[0].data["finger"].as_u64().unwrap();
        if pitch == 60 {
            assert_eq!(finger, 1);
        } else {
            assert_ne!(
                finger, 1,
                "pitch {pitch} should share the source black-key preference"
            );
        }
        let c = compile(&song(r#".annotate("all",{finger:1})"#)).unwrap();
        assert_eq!(c.score[0].pattern.notes[0].data["finger"].as_u64(), Some(1));
    }
}
#[test]
fn invalid_solver_preferences_are_rejected_for_piano_only() {
    for changes in [
        "{finger_motion_cost:-1}",
        "{finger_motion_cost:1b}",
        "{hand_centers:[60]}",
        "{finger_pitch_costs:[]}",
        "{unknown_cost:1}",
        "{hand_centers:[-1,72]}",
    ] {
        let error = one_note(&format!(
            r#"{{policy:"piano",playing:merge(perf.piano_preferences,{changes})}}"#
        ))
        .unwrap_err();
        assert!(format!("{error:#}").contains("piano"));
    }
    assert!(one_note(r#"{policy:"piano",playing:{}}"#).is_err());
    assert!(one_note(r#"{policy:"free",playing:"unused"}"#).is_ok());
}

#[test]
fn source_track_keeps_explicit_arguments_over_reserved_options() {
    let c=compile(r#"song({tracks:[track("p",note("C4",1b),synth("bell"),{id:"other",pattern:note("D4",1b),instrument:synth("kick")})]})"#).unwrap();
    assert_eq!(c.score[0].id, "p");
    assert_eq!(c.score[0].pattern.notes[0].pitch, 60.);
    assert_eq!(c.session.tracks[0].instrument.params["mode"], 2.);
}
