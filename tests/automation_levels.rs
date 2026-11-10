//! An `.out` lane replaces an explicit output `gain`; the compiler says so.
//! Small synthetic sources only.
fn compile(body: &str) -> anyhow::Result<muz::compile::Compiled> {
    let d = tempfile::tempdir()?;
    let path = d.path().join("levels.muz");
    std::fs::write(&path, body)?;
    muz::compile::compile(&path)
}
fn song(options: &str, lanes: &str) -> String {
    format!(
        r#"song({{tracks:[track("guitar",phrase("C4:q"),synth("bell"),{options})],
        automation:[{lanes}],tail:0}})"#
    )
}
fn warnings(c: &muz::compile::Compiled) -> Vec<(String, String)> {
    c.diagnostics
        .iter()
        .filter(|d| d.code == "automation.replaces_gain")
        .map(|d| (d.track.clone(), d.message.clone()))
        .collect()
}
const LANE: &str = r#"automation("guitar.out",curve([[1b,-9],[2b,-3]]))"#;

#[test]
fn explicit_gain_under_an_out_lane_warns_with_the_remedy() {
    let c = compile(&song("{gain:-6.5}", LANE)).unwrap();
    assert_eq!(
        warnings(&c),
        [(
            "guitar".into(),
            "`gain: -6.5` on guitar has no effect: the automation lane on guitar.out \
             replaces it for the whole song; put the whole fader level in the lane \
             (add -6.5 dB to each point) or drop `gain`"
                .into()
        )]
    );
    assert_eq!(c.diagnostics[0].severity, "warning");
    // An explicit 0 dB is still a written value the lane discards.
    assert_eq!(
        warnings(&compile(&song("{gain:0}", LANE)).unwrap()).len(),
        1
    );
}

#[test]
fn gain_or_lane_alone_does_not_warn() {
    for (options, lanes) in [
        ("{gain:-6.5}", ""),
        ("{}", LANE),
        // A lane on another target leaves the fader's gain live.
        (
            "{gain:-6.5,chain:[fx(\"gain\",{id:\"trim\",gain_db:0})]}",
            r#"automation("guitar.trim.gain_db",curve([[0b,-3]]))"#,
        ),
    ] {
        let c = compile(&song(options, lanes)).unwrap();
        assert!(c.diagnostics.is_empty(), "{options} {lanes}");
    }
}

#[test]
fn bus_and_kit_voice_faders_follow_the_same_rule() {
    let source = |bus_gain: &str, kit_gain: &str, voice_gain: &str| {
        format!(
            r#"song({{tracks:[track("drums",drums({{kick:"X..."}}),
                kit("default",{{kick:{{instrument:synth("kick"){voice_gain}}}}}),
                {{output:"room"{kit_gain}}})],
            buses:[bus("room",[],{{{bus_gain}}})],
            automation:[automation("drums.kick.out",curve([[0b,-4]])),
                automation("room.out",curve([[0b,-2]]))],tail:0}})"#
        )
    };
    let c = compile(&source("gain:-3", ",gain:-1", ",gain:-2")).unwrap();
    let found = warnings(&c);
    assert_eq!(
        found.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        ["drums.kick", "room"]
    );
    // The kit voice's dead value is the kit gain plus the voice gain.
    assert!(
        found[0].1.starts_with("`gain: -3` on drums.kick"),
        "{found:?}"
    );
    assert!(found[1].1.starts_with("`gain: -3` on room"), "{found:?}");
    for (kit_gain, voice_gain) in [(",gain:-1", ""), ("", ",gain:-2")] {
        let c = compile(&source("", kit_gain, voice_gain)).unwrap();
        assert_eq!(warnings(&c).len(), 1, "{kit_gain} {voice_gain}");
    }
    assert!(warnings(&compile(&source("", "", "")).unwrap()).is_empty());
}

#[test]
fn send_levels_under_a_lane_stay_quiet() {
    // Declaring a send needs a level; parking it under a throw lane is idiomatic.
    let c = compile(
        r#"song({tracks:[track("lead",phrase("C4:q"),synth("bell"),{sends:{echo:-120}})],
        buses:[bus("echo",[])],
        automation:[automation("lead.send.echo",curve([[0b,-120],[1b,-12]],"step"))],tail:0})"#,
    )
    .unwrap();
    assert!(c.diagnostics.is_empty());
}

#[test]
fn trim_plus_ride_is_written_into_the_lane() {
    let c = compile(
        r#"let trim = -6.5;
        let ride = [[0b, 0], [4b, -3], [8b, 2]];
        song({tracks:[track("guitar",phrase("C4:q"),synth("bell"))],
        automation:[automation("guitar.out",
            curve(ride.map(fn(p) => [p[0], p[1] + trim]), "step"))],tail:0})"#,
    )
    .unwrap();
    assert!(c.diagnostics.is_empty());
    let lane = &c.session.extras.automation[0];
    assert_eq!(lane.shape, "step");
    assert_eq!(
        lane.points.iter().map(|p| p.value).collect::<Vec<_>>(),
        [-6.5, -9.5, -4.5]
    );
}
