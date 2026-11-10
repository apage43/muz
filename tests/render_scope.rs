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
