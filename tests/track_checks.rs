//! Source checks named by a track's `checks` option report diagnostics on the
//! track's performed notes. Small synthetic sources only.
fn compile(body: &str) -> anyhow::Result<muz::compile::Compiled> {
    let d = tempfile::tempdir()?;
    let path = d.path().join("checks.muz");
    std::fs::write(&path, format!("use \"std/checks\" as checks;\n{body}"))?;
    muz::compile::compile(&path)
}
fn one_track(pattern: &str, checks: &str) -> anyhow::Result<muz::compile::Compiled> {
    compile(&format!(
        r#"song({{tracks:[track("t",{pattern},synth("bell"),{{checks:{checks}}})],tail:0}})"#
    ))
}
fn codes(c: &muz::compile::Compiled) -> Vec<(String, String, f64)> {
    c.diagnostics
        .iter()
        .map(|d| (d.code.clone(), d.track.clone(), d.beat))
        .collect()
}

#[test]
fn monophonic_reports_overlaps_and_accepts_lines_and_gated_notes() {
    let c = one_track(
        r#"stack([note("C3",2b), note("E3",1b).at(1b)])"#,
        "[checks.monophonic()]",
    )
    .unwrap();
    assert_eq!(
        codes(&c),
        [("check.monophonic.overlap".into(), "t".into(), 1.0)]
    );
    assert_eq!(c.diagnostics[0].severity, "warning");
    for line in [
        r#"phrase("C3:q D3:q E3:h")"#,
        r#"stack([note("C3",2b).gate(0.5), note("E3",1b).at(1b)])"#,
    ] {
        let c = one_track(line, "[checks.monophonic()]").unwrap();
        assert!(c.diagnostics.is_empty(), "{line}: {:?}", codes(&c));
    }
}

#[test]
fn monophonic_names_pitches_overlap_length_and_bar_position() {
    let c = one_track(
        r#"stack([note("C3",2b), note("E3",1b).at(1b)])"#,
        "[checks.monophonic()]",
    )
    .unwrap();
    let d = &c.diagnostics[0];
    assert_eq!(
        d.message,
        "E3 starts while C3 from 1b earlier still sounds; overlap 0.8b"
    );
    assert_eq!(d.position(c.session.transport.meter()), "bar 1 beat 2 (1b)");
    // An eighth at gate 0.54 reaches 0.02b into the next sixteenth; a
    // tolerance accepts it as legato.
    let slip = r#"stack([note("C3",1/2b).gate(0.54).at(4b), note("D3",1/4b).at(17/4b)])"#;
    let c = one_track(slip, "[checks.monophonic()]").unwrap();
    assert_eq!(
        c.diagnostics[0].message,
        "D3 starts while C3 from 0.25b earlier still sounds; overlap 0.02b"
    );
    assert_eq!(
        c.diagnostics[0].position(c.session.transport.meter()),
        "bar 2 beat 1.25 (4.25b)"
    );
    assert!(
        one_track(slip, "[checks.monophonic(tolerance = 1/32b)]")
            .unwrap()
            .diagnostics
            .is_empty()
    );
    // A held note is compared with every later onset, not only the next one.
    let c = one_track(
        r#"stack([note("C3",4b), note("D3",1/2b).at(1b), note("E3",1/2b).at(2b), note("G3",1b).at(4b)])"#,
        "[checks.monophonic()]",
    )
    .unwrap();
    assert_eq!(
        c.diagnostics.iter().map(|d| d.beat).collect::<Vec<_>>(),
        [1.0, 2.0]
    );
    assert!(
        c.diagnostics[1]
            .message
            .starts_with("E3 starts while C3 from 2b")
    );
    let c = one_track(
        r#"stack([note("G4",1b), note("D5",1b)])"#,
        "[checks.monophonic()]",
    )
    .unwrap();
    assert_eq!(
        c.diagnostics[0].message,
        "G4 and D5 start together; overlap 0.9b"
    );
}

#[test]
fn diagnostic_positions_count_bars_and_beats_in_the_song_meter() {
    let c = compile(
        r#"song({meter:[7,8], tracks:[track("t",note("C4",1b),synth("bell"),{checks:[fn(notes) => [{code:"x", message:"m", at:4b}]]})],tail:0})"#,
    )
    .unwrap();
    assert_eq!(
        c.diagnostics[0].position(c.session.transport.meter()),
        "bar 2 beat 2 (4b)"
    );
    let error = one_track(
        r#"note("C4",1b)"#,
        r#"[fn(notes) => [{code:"brief", message:"unplayable", severity:"error", at:5b}]]"#,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("t bar 2 beat 2 (5b): unplayable"),
        "{error:#}"
    );
}

#[test]
fn in_range_reports_only_pitches_outside_the_bounds() {
    let c = one_track(r#"phrase("E1:q C2:q G4:q")"#, "[checks.in_range(28, 60)]").unwrap();
    assert_eq!(
        codes(&c),
        [("check.in_range.outside".into(), "t".into(), 2.0)]
    );
    assert_eq!(c.diagnostics[0].message, "G4 is outside E1..C4 (28..60)");
    let c = one_track(r#"phrase("E1:q G3:q")"#, "[checks.in_range(28, 60)]").unwrap();
    assert!(c.diagnostics.is_empty());
}

#[test]
fn kit_checks_see_every_voice_of_the_logical_track() {
    let kit = r#"let hits = stack([
        note(36,1/4b).voice("kick"), note(38,1/4b).voice("snare"), note(42,1/4b).voice("hat"),
        note(36,1/4b).voice("kick").at(1b), note(42,1/4b).voice("hat").at(1b)
    ]);
    let k = kit("band", {kick: synth("kick"), snare: synth("kick"), hat: synth("kick")}, defaults = {});"#;
    let song = |checks: &str| {
        compile(&format!(
            r#"{kit}
song({{tracks:[track("drums",hits,k,{{checks:{checks}}})],tail:0}})"#
        ))
    };
    let c = song("[checks.max_simultaneous(2, 1/4b)]").unwrap();
    assert_eq!(
        codes(&c),
        [(
            "check.max_simultaneous.exceeded".into(),
            "drums".into(),
            0.0
        )]
    );
    assert!(
        c.diagnostics[0].message.contains("kick snare hat"),
        "{}",
        c.diagnostics[0].message
    );
    assert!(
        song("[checks.max_simultaneous(3, 1/4b)]")
            .unwrap()
            .diagnostics
            .is_empty()
    );
    let c = song(
        r#"[fn(notes) => [{code:"voices", message:str(len(group_by(notes, fn(n) => n.voice)))}]]"#,
    )
    .unwrap();
    assert_eq!(c.diagnostics[0].message, "3");
}

#[test]
fn custom_check_diagnostics_reach_compile_with_code_track_and_time() {
    let c = compile(
        r#"fn quiet(notes) = map(filter(notes, fn(n) => n.velocity < 0.2), fn(n) => {
    code: "dynamics.inaudible", message: "too quiet for this player", at: n.at
});
song({tracks:[
    track("a",phrase("C4:q D4:q").velocity(0.1),synth("bell"),{checks:[quiet]}),
    track("b",phrase("C4:q"),synth("bell"),{checks:[fn(notes) => [{code:"x", message:"bar two", at:1bar}]]})
],tail:0})"#,
    )
    .unwrap();
    assert_eq!(
        codes(&c),
        [
            ("check.dynamics.inaudible".into(), "a".into(), 0.0),
            ("check.dynamics.inaudible".into(), "a".into(), 1.0),
            ("check.x".into(), "b".into(), 4.0),
        ]
    );
    // The documented articulation example: one diagnostic per mixed onset.
    let c = compile(
        r#"fn no_mixed_bows(notes) = map(
    filter(group_by(notes, fn(n) => n.at), fn(onset) =>
        len(group_by(onset, fn(n) => n.voice)) > 1),
    fn(onset) => {code: "violin.bows", message: "legato and spiccato at once", at: onset[0].at}
);
let line = stack([note("G4",1b).voice("legato"), note("D5",1b).voice("spiccato"), note("A4",1b).at(1b).voice("legato")]);
song({tracks:[track("vn",line,synth("bell"),{checks:[checks.monophonic(), checks.in_range(55, 103), no_mixed_bows]})],tail:0})"#,
    )
    .unwrap();
    assert_eq!(
        codes(&c),
        [
            ("check.monophonic.overlap".into(), "vn".into(), 0.0),
            ("check.violin.bows".into(), "vn".into(), 0.0),
        ]
    );
    // Checks see the notes as performed, after piano hand allocation.
    let c = one_track(
        r#"phrase("C2:q C6:q")"#,
        r#"[fn(notes) => map(notes, fn(n) => {code:"hand", message:n.hand})], policy:"piano""#,
    )
    .unwrap();
    let hands: Vec<_> = c
        .diagnostics
        .iter()
        .filter(|d| d.code == "check.hand")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(hands, ["left", "right"]);
    // A check may opt into failing compilation.
    let error = one_track(
        r#"note("C4",1b)"#,
        r#"[fn(notes) => [{code:"brief", message:"unplayable", severity:"error"}]]"#,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("unplayable"), "{error:#}");
}

#[test]
fn malformed_check_returns_and_values_are_clear_errors() {
    for (check, expected) in [
        (
            "fn(notes) => 3",
            "check checks[0] on track 't' returned number, not a list",
        ),
        ("fn(notes) => [1]", "number at [0], not a record"),
        (r#"fn(notes) => [{message:"m"}]"#, "no valid 'code' at [0]"),
        (r#"fn(notes) => [{code:"c"}]"#, "no text 'message' at [0]"),
        (
            r#"fn(notes) => [{code:"c", message:"m", at:2}]"#,
            "not a musical time",
        ),
        (
            r#"fn(notes) => [{code:"c", message:"m", severity:"info"}]"#,
            "invalid 'severity'",
        ),
        (
            r#"fn(notes) => [{code:"c", message:"m", pitch:60}]"#,
            "unknown field 'pitch'",
        ),
        ("3", "checks[0] must be a function"),
    ] {
        let error = format!(
            "{:#}",
            one_track(r#"note("C4",1b)"#, &format!("[{check}]")).unwrap_err()
        );
        assert!(error.contains(expected), "{check}: {error}");
    }
}

#[test]
fn an_endless_check_names_itself_and_its_track_when_over_budget() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("budget.muz");
    std::fs::write(
        &path,
        r#"fn spin(notes) = map(range(100000), fn(i) => map(range(10), fn(j) => j));
song({tracks:[track("lead",note("C4",1b),synth("bell"),{checks:[spin]})],tail:0})"#,
    )
    .unwrap();
    let context = muz::host::HostContext {
        evaluation_steps: 300_000,
        ..Default::default()
    };
    let error = format!(
        "{:#}",
        muz::compile::compile_with_context(
            &path,
            std::rc::Rc::new(muz::lang::FileSourceLoader),
            &context
        )
        .unwrap_err()
    );
    assert!(
        error.contains("check 'spin' (checks[0]) on track 'lead'"),
        "{error}"
    );
    assert!(
        error.contains("evaluation budget exceeded (300000 operations)"),
        "{error}"
    );
}

#[test]
fn song_checks_see_named_tracks_performed_notes_merged_in_time_order() {
    let song = |checks: &str| {
        compile(&format!(
            r#"song({{tracks:[
    track("violin",stack([note("G4",2b), note("A4",1b).at(3b)]),synth("bell")),
    track("violin_spiccato",note("D5",1/2b).at(1b),synth("bell")),
    track("cello",note("C3",4b),synth("bell"))
], checks:{checks}, tail:0}})"#
        ))
    };
    let c =
        song(r#"[{tracks: ["violin", "violin_spiccato"], check: checks.monophonic()}]"#).unwrap();
    assert_eq!(
        codes(&c),
        [(
            "check.monophonic.overlap".into(),
            "violin+violin_spiccato".into(),
            1.0
        )]
    );
    assert_eq!(
        c.diagnostics[0].message,
        "D5 (violin_spiccato) starts while G4 (violin) from 1b earlier still sounds; overlap 0.45b"
    );
    // Each note carries its track; the list is in onset order across tracks.
    let c = song(
        r#"[{tracks: ["violin", "violin_spiccato"], check: fn(notes) => [{code: "order",
            message: fold(notes, "", fn(s, n) => s + n.track + "@" + str(n.at / 1b) + " ")}]}]"#,
    )
    .unwrap();
    assert_eq!(
        c.diagnostics[0].message,
        "violin@0 violin_spiccato@1 violin@3 "
    );
    // Track checks see `track` too.
    let c = one_track(
        r#"note("C4",1b)"#,
        r#"[fn(notes) => [{code: "t", message: notes[0].track}]]"#,
    )
    .unwrap();
    assert_eq!(c.diagnostics[0].message, "t");
    for (checks, expected) in [
        (
            r#"[{tracks: ["violin", "violin_spicato"], check: checks.monophonic()}]"#,
            "song checks[0] names unknown track 'violin_spicato'",
        ),
        (
            r#"[{tracks: [], check: checks.monophonic()}]"#,
            "needs at least one track",
        ),
        (
            r#"[{tracks: ["violin"], check: 3}]"#,
            "song checks[0] 'check' must be a function",
        ),
        (
            r#"[{tracks: ["violin"], check: checks.monophonic(), name: "x"}]"#,
            "unknown song check field 'name'",
        ),
        (
            r#"[{tracks: ["violin"], check: fn(notes) => 3}]"#,
            "song check checks[0] on tracks 'violin' returned number, not a list",
        ),
    ] {
        let error = format!("{:#}", song(checks).unwrap_err());
        assert!(error.contains(expected), "{checks}: {error}");
    }
}
