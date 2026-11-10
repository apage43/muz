//! Partial measurements: several taps from one pass, selected stems, and bounded
//! section history. Tiny native-synth songs keep each case fast.
use muz::render::{self, RenderOptions};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

const SONG: &str = r#"song({tempo:120,
    sections:[section("intro",2b),section("verse",2b),section("end",2b)],
    tracks:[
        track("lead",phrase("C4:q E4:q G4:q C5:q D5:q E5:q"),synth("bell"),{sends:{hall:-6}}),
        track("pad",phrase("C3:h G3:h E3:h"),synth("init",{sustain:1,release_ms:300}),{gain:-6})
    ],
    buses:[bus("hall",[fx("delay",{time_ms:120,feedback:0.4,mix:1})])],
    tail:0.25})"#;

fn write(dir: &Path, source: &str) -> PathBuf {
    let path = dir.join("song.muz");
    std::fs::write(&path, source).unwrap();
    path
}
fn session(path: &Path) -> muz::Session {
    muz::compile::compile(path).unwrap().session
}
fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}
fn muz<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_muz"))
        .args(args)
        .output()
        .unwrap()
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn several_taps_from_one_pass_equal_single_tap_renders() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(&write(dir.path(), SONG));
    for (format, threads) in [("float32", 1), ("pcm24", 3)] {
        let options = RenderOptions {
            section: Some("verse".into()),
            block_size: 97,
            format: format.into(),
            threads,
            tap: vec!["lead".into(), "hall".into(), "master".into()],
            ..Default::default()
        };
        let out = dir.path().join(format!("taps-{format}"));
        let reports = render::render_taps(s.clone(), &out, &options).unwrap();
        assert_eq!(reports.len(), 3);
        for (report, id) in reports.iter().zip(["lead", "hall", "master"]) {
            assert_eq!(report.scope.tap, vec![id.to_owned()]);
            let single = dir.path().join(format!("{format}-{id}.wav"));
            let expected = render::render_with(
                s.clone(),
                &single,
                &RenderOptions {
                    tap: vec![id.into()],
                    ..options.clone()
                },
                None,
            )
            .unwrap();
            assert_eq!(report.frames, expected.frames);
            assert_eq!(report.sample_peak_dbfs, expected.sample_peak_dbfs);
            assert_eq!(
                bytes(&out.join(format!("{id}.wav"))),
                bytes(&single),
                "{format}/{id}"
            );
        }
    }
}

#[test]
fn a_single_tap_keeps_its_file_output_and_report_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), SONG);
    let out = dir.path().join("lead.wav");
    let result = muz(&[
        "render".as_ref(),
        path.as_os_str(),
        "--tap".as_ref(),
        "lead".as_ref(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(out.is_file());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["scope"]["tap"], "lead");

    let taps = dir.path().join("taps");
    let result = muz(&[
        "render".as_ref(),
        path.as_os_str(),
        "--tap".as_ref(),
        "lead".as_ref(),
        "--tap".as_ref(),
        "hall".as_ref(),
        "-o".as_ref(),
        taps.as_os_str(),
    ]);
    assert!(result.status.success(), "{}", stderr(&result));
    assert_eq!(bytes(&taps.join("lead.wav")), bytes(&out));
    assert!(taps.join("hall.wav").is_file());
    let reports: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(reports[1]["scope"]["tap"], "hall");
}

#[test]
fn tap_arguments_are_checked_before_rendering() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), SONG);
    let out = dir.path().join("out");
    let run = |taps: &[&str], out: &Path| {
        let mut args = vec!["render".to_owned(), path.display().to_string()];
        for tap in taps {
            args.extend(["--tap".to_owned(), (*tap).to_owned()]);
        }
        args.extend(["-o".to_owned(), out.display().to_string()]);
        muz(&args)
    };
    let duplicate = run(&["lead", "lead"], &out);
    assert!(!duplicate.status.success());
    assert!(stderr(&duplicate).contains("tap lead is listed more than once"));
    let unknown = run(&["lead", "hal"], &out);
    assert!(!unknown.status.success());
    assert!(stderr(&unknown).contains("unknown tap hal"));
    assert!(stderr(&unknown).contains("hall"), "{}", stderr(&unknown));
    let single_unknown = run(&["leed"], &dir.path().join("x.wav"));
    assert!(stderr(&single_unknown).contains("unknown tap leed"));
    let file = run(&["lead", "hall"], &dir.path().join("taps.wav"));
    assert!(!file.status.success());
    assert!(stderr(&file).contains("pass a directory to -o"));
    assert!(!out.exists() && !dir.path().join("taps.wav").exists());

    // Queued and batch renders produce one file per job.
    let queued = muz(&[
        "render".as_ref(),
        "--socket".as_ref(),
        dir.path().join("none.sock").as_os_str(),
        "--tap".as_ref(),
        "lead".as_ref(),
        "--tap".as_ref(),
        "hall".as_ref(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);
    assert!(stderr(&queued).contains("a queued render takes one --tap"));
    let error = render::render_with(
        session(&path),
        &dir.path().join("y.wav"),
        &RenderOptions {
            tap: vec!["lead".into(), "hall".into()],
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("several taps need a directory output"));
}

#[test]
fn recipe_options_accept_one_tap_string_or_a_list() {
    let one: RenderOptions = serde_json::from_str(r#"{"tap":"lead"}"#).unwrap();
    assert_eq!(one.tap, vec!["lead".to_owned()]);
    let many: RenderOptions = serde_json::from_str(r#"{"tap":["lead","hall"]}"#).unwrap();
    assert_eq!(many.tap.len(), 2);
    let none: RenderOptions = serde_json::from_str(r#"{"tap":null}"#).unwrap();
    assert!(none.tap.is_empty());
    assert_eq!(serde_json::to_value(&one).unwrap()["tap"], "lead");
    assert!(serde_json::to_value(RenderOptions::default()).unwrap()["tap"].is_null());
}

const STEM_SONG: &str = r#"song({tempo:120,
    sections:[section("a",2b),section("b",2b)],
    tracks:[
        track("kick",phrase("C2:q C2:q C2:q C2:q"),synth("init",{release_ms:40})),
        track("pad",phrase("C3:w"),synth("init",{sustain:1,release_ms:300}),{sends:{hall:0}}),
        track("bass",phrase("C2:h G2:h"),synth("init",{sustain:1,release_ms:80}),{chain:[
            fx("compressor",{sidechain:"kick",threshold_db:-30,ratio:6,knee_db:0,attack_ms:1,release_ms:40})
        ]}),
        track("lead",phrase("E4:e G4:e C5:q E5:h"),synth("bell"),{sends:{hall:-6}})
    ],
    buses:[bus("hall",[fx("delay",{time_ms:90,feedback:0.5,mix:1})])],
    master:[fx("limiter",{lookahead_ms:5,ceiling_db:-1})],
    automation:[
        automation("pad.out",curve([[0b,-12],[2b,0]],"linear")),
        automation("lead.out",curve([[0b,0],[2b,-6]],"linear")),
        automation("hall.fx0.mix",curve([[0b,1],[2b,0.5]],"linear"))
    ],
    tail:0.3})"#;

#[test]
fn selected_stems_equal_a_full_stem_export() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(&write(dir.path(), STEM_SONG));
    for (section, threads) in [(None, 1), (Some("b"), 4)] {
        let options = RenderOptions {
            section: section.map(str::to_owned),
            threads,
            block_size: 113,
            ..Default::default()
        };
        let full = dir.path().join(format!("full-{threads}"));
        render::stems(s.clone(), &full, &options).unwrap();
        let some = dir.path().join(format!("some-{threads}"));
        let reports =
            render::stems_of(s.clone(), &some, &options, &["lead".into(), "bass".into()]).unwrap();
        // Reports and files follow session order; the sidechain source is
        // processed for the bass compressor but not written.
        let written: Vec<_> = reports.iter().map(|r| r.scope.tap[0].clone()).collect();
        assert_eq!(written, ["bass", "lead"]);
        for id in ["bass", "lead"] {
            assert_eq!(
                bytes(&some.join(format!("{id}.wav"))),
                bytes(&full.join(format!("{id}.wav"))),
                "{id} with {threads} threads"
            );
        }
        assert!(!some.join("kick.wav").exists() && !some.join("pad.wav").exists());
        // Without the bass, the first track is not needed either; the timeline
        // and the lead's automation stay those of the whole song.
        let lead = dir.path().join(format!("lead-{threads}"));
        render::stems_of(s.clone(), &lead, &options, &["lead".into()]).unwrap();
        assert_eq!(bytes(&lead.join("lead.wav")), bytes(&full.join("lead.wav")));
    }
    let error = render::stems_of(
        s,
        &dir.path().join("x"),
        &Default::default(),
        &["leed".into()],
    )
    .unwrap_err();
    let text = format!("{error:#}");
    assert!(
        text.contains("unknown stem track leed") && text.contains("lead"),
        "{text}"
    );
}

#[test]
fn stems_cli_takes_repeatable_track_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), STEM_SONG);
    for wet in [false, true] {
        let out = dir.path().join(format!("stems-{wet}"));
        let mut args = vec![
            "stems".to_owned(),
            path.display().to_string(),
            "--track".into(),
            "pad".into(),
            "--track".into(),
            "kick".into(),
            "-o".into(),
            out.display().to_string(),
        ];
        if wet {
            args.push("--wet".into());
        }
        let result = muz(&args);
        assert!(result.status.success(), "{}", stderr(&result));
        let reports: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(reports.as_array().unwrap().len(), 2);
        assert!(out.join("kick.wav").is_file() && out.join("pad.wav").is_file());
        assert!(!out.join("lead.wav").exists());
    }
}
