#![cfg(feature = "desktop")]
//! Per-note CLAP parameter control, proven against a synthetic probe plugin
//! (tests/fixtures/note_param_plugin.rs) built for this test only.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Entry {
    kind: u16,
    frame: u64,
    note_id: i32,
    key: i16,
    param: u32,
    value: f64,
    known: bool,
}
const NOTE_ON: u16 = 0;
const NOTE_OFF: u16 = 1;
const NOTE_EXPRESSION: u16 = 4;
const PARAM_VALUE: u16 = 5;
const PARAM_MOD: u16 = 6;

struct Probe {
    dir: tempfile::TempDir,
    path: PathBuf,
    library: libloading::Library,
}
impl Probe {
    fn build() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.clap");
        let deps = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_owned();
        let clap = std::fs::read_dir(&deps)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("libclap_sys-")
                    && p.extension().is_some_and(|e| e == "rlib")
            })
            .expect("Cargo test build supplies the clap-sys ABI dependency");
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/note_param_plugin.rs");
        let out = Command::new("rustc")
            .args(["--edition=2024", "--crate-type=cdylib", "-C", "opt-level=1"])
            .arg(&source)
            .arg("--extern")
            .arg(format!("clap_sys={}", clap.display()))
            .arg("-L")
            .arg(format!("dependency={}", deps.display()))
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "probe compile: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // Keep the module loaded so its event log outlives each render's instance.
        let library = unsafe { libloading::Library::new(&path) }.unwrap();
        Self { dir, path, library }
    }
    fn log(&self) -> Vec<Entry> {
        let drain = unsafe {
            self.library
                .get::<unsafe extern "C" fn(*mut Entry, usize) -> usize>(b"drain_log\0")
        }
        .unwrap();
        let mut out = vec![Entry::default(); 65536];
        let n = unsafe { drain(out.as_mut_ptr(), out.len()) };
        out.truncate(n);
        out
    }
    fn song(&self, notes: &str, controls: &str) -> PathBuf {
        self.song_with(notes, &format!("note_controls:{{{controls}}}"))
    }
    fn song_with(&self, notes: &str, options: &str) -> PathBuf {
        let path = serde_json::to_string(&self.path.to_string_lossy()).unwrap();
        let source = format!(
            r#"use "std/instrument" as i;
song({{tempo:120,tracks:[track("p",stack([{notes}]),plugin("probe",{{path:{path},class:"muz.test.note-param",{options}}}))],tail:0}})"#
        );
        let file = self.dir.path().join("song.muz");
        std::fs::write(&file, source).unwrap();
        file
    }
    fn render(&self, song: &Path, block: usize) -> anyhow::Result<Vec<f32>> {
        let wav = self.dir.path().join("out.wav");
        muz::render::render(song, &wav, None, None, &[], 48000, block)?;
        let samples = hound::WavReader::open(&wav)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .step_by(2)
            .collect();
        std::fs::remove_file(&wav).unwrap();
        Ok(samples)
    }
}

const CONTROLS: &str = r#"lift:{param:"p1",mode:"mod"},level:{param:"Level",mode:"value"}"#;

#[test]
fn overlapping_same_pitch_notes_receive_their_own_parameter_events() {
    let probe = Probe::build();
    let song = probe.song(
        r#"phrase("C4:h").express({lift:0.25}),phrase("C4:h").at(1b).express({level:0.1}),phrase("E4:q").at(4b).express({lift:[[0,0],[1,0.4]]})"#,
        CONTROLS,
    );
    probe.log();
    let audio = probe.render(&song, 256).unwrap();
    let log = probe.log();

    // Audio: A alone is Level 0.5 + mod 0.25; B alone is its own value 0.1;
    // together they sum, so neither note's control leaked to the other.
    let (a, both, b) = (audio[12_000], audio[36_000], audio[60_000]);
    assert!(a > 0.0 && b > 0.0, "{a} {b}");
    assert!((a / b - 7.5).abs() < 1e-3, "A {a}, B {b}");
    assert!((both - (a + b)).abs() < 1e-5, "overlap {both} != {a} + {b}");

    let ons: Vec<_> = log.iter().filter(|e| e.kind == NOTE_ON).collect();
    assert_eq!(ons.len(), 3);
    assert_eq!((ons[0].key, ons[1].key), (60, 60));
    assert_ne!(ons[0].note_id, ons[1].note_id);
    let per_note: Vec<_> = log
        .iter()
        .filter(|e| matches!(e.kind, PARAM_VALUE | PARAM_MOD))
        .collect();
    // Every parameter event names one note, after that note's note-on (the probe
    // marks events addressed to a voice it does not have as unknown).
    assert!(
        per_note
            .iter()
            .all(|e| e.note_id >= 0 && e.known && e.param == 1)
    );
    let first = |id: i32| {
        log.iter()
            .position(|e| e.note_id == id && e.kind != NOTE_ON)
            .unwrap()
    };
    for on in &ons[..2] {
        let at = first(on.note_id);
        let on_index = log
            .iter()
            .position(|e| e.kind == NOTE_ON && e.note_id == on.note_id)
            .unwrap();
        assert!(on_index < at);
        assert_eq!(
            log[at].frame, on.frame,
            "first parameter event shares the attack frame"
        );
    }
    let a_events: Vec<_> = per_note
        .iter()
        .filter(|e| e.note_id == ons[0].note_id)
        .collect();
    assert_eq!(a_events.len(), 1, "a constant curve is sent once");
    assert_eq!((a_events[0].kind, a_events[0].value), (PARAM_MOD, 0.25));
    let b_events: Vec<_> = per_note
        .iter()
        .filter(|e| e.note_id == ons[1].note_id)
        .collect();
    assert_eq!(b_events.len(), 1);
    assert_eq!((b_events[0].kind, b_events[0].key), (PARAM_VALUE, 60));
    assert!((b_events[0].value - 0.1).abs() < 1e-6);

    // A changing curve is stepped every 128 frames from the attack and stops
    // before the note-off; nothing is sent at or after it.
    let c = ons[2];
    let off = log
        .iter()
        .find(|e| e.kind == NOTE_OFF && e.note_id == c.note_id)
        .unwrap();
    let steps: Vec<_> = per_note.iter().filter(|e| e.note_id == c.note_id).collect();
    assert!(steps.len() > 100);
    for (i, e) in steps.iter().enumerate() {
        assert_eq!(e.frame, c.frame + 128 * i as u64);
        assert!(e.frame < off.frame);
        assert_eq!(e.kind, PARAM_MOD);
    }
    assert!(steps.windows(2).all(|w| w[1].value > w[0].value));
    assert!(steps.last().unwrap().value <= 0.4 + 1e-6);

    // Block size does not move events or audio.
    let odd = probe.render(&song, 97).unwrap();
    assert_eq!(audio, odd);
    let again: Vec<_> = probe
        .log()
        .into_iter()
        .filter(|e| matches!(e.kind, PARAM_VALUE | PARAM_MOD))
        .map(|e| (e.frame, e.note_id, e.value.to_bits()))
        .collect();
    let before: Vec<_> = per_note
        .iter()
        .map(|e| (e.frame, e.note_id, e.value.to_bits()))
        .collect();
    assert_eq!(before, again);
}

#[test]
fn capability_range_and_destination_diagnostics() {
    let probe = Probe::build();
    let mut host =
        muz::plugins::open_clap(&probe.path, Some("muz.test.note-param"), 48000, 256).unwrap();
    let inspected = serde_json::to_value(host.parameters()).unwrap();
    assert_eq!(
        inspected[3]["scopes"]["value"],
        serde_json::json!(["global"])
    );
    assert_eq!(
        inspected[3]["scopes"]["mod"],
        serde_json::json!(["global", "note_id", "key", "channel", "port"])
    );
    let bind = |host: &mut muz::audio::clap::PreparedClap, param: &str, mode| {
        let controls = [(
            "x".to_owned(),
            muz::model::NoteParameter {
                param: param.into(),
                mode,
            },
        )]
        .into_iter()
        .collect();
        host.bind_note_controls(&controls)
            .map_err(|e| format!("{e:#}"))
    };
    use muz::model::NoteParameterMode::{Mod, Value};
    bind(&mut host, "Pitch", Mod).unwrap();
    bind(&mut host, "1", Value).unwrap();
    let global = bind(&mut host, "Tone", Mod).unwrap_err();
    assert!(
        global.contains("\"Tone\"")
            && global.contains("per note ID")
            && global.contains("mod scopes: global"),
        "{global}"
    );
    let keyed = bind(&mut host, "p3", Value).unwrap_err();
    assert!(
        keyed.contains("value scopes: global, key")
            && keyed.contains("cannot separate two overlapping notes of the same pitch"),
        "{keyed}"
    );
    let pitch = bind(&mut host, "Pitch", Value).unwrap_err();
    assert!(pitch.contains("use mode \"mod\""), "{pitch}");
    assert!(
        bind(&mut host, "p9", Mod)
            .unwrap_err()
            .contains("unknown CLAP parameter 'p9'")
    );
    drop(host);

    // Plain ranges are checked against the plugin before playback.
    for (notes, expected) in [
        (r#"phrase("C4:q").express({lift:1.5})"#, "outside -1..1"),
        (
            r#"phrase("C4:q").express({level:[[0,0.5],[1,1.2]]})"#,
            "outside 0..1",
        ),
    ] {
        let error = format!(
            "{:#}",
            probe.render(&probe.song(notes, CONTROLS), 256).unwrap_err()
        );
        assert!(
            error.contains("note control '") && error.contains(expected),
            "{error}"
        );
    }
    // An invalid binding fails preparation rather than writing globally.
    let song = probe.song(
        r#"phrase("C4:q").express({x:0.1})"#,
        r#"x:{param:"Tone",mode:"mod"}"#,
    );
    assert!(probe.render(&song, 256).is_err());

    // Destinations without per-note parameter addressing reject at compile time.
    let compile = |source: &str| {
        let file = probe.dir.path().join("reject.muz");
        std::fs::write(&file, source).unwrap();
        format!("{:#}", muz::compile::compile(&file).unwrap_err())
    };
    let vst3 = compile(
        r#"song({tracks:[track("p",note(60,1b),plugin("v",{path:"/nowhere/x.vst3",note_controls:{x:{param:"p1",mode:"mod"}}}))]})"#,
    );
    assert!(
        vst3.contains("VST3 parameters are not addressed per note"),
        "{vst3}"
    );
    let path = serde_json::to_string(&probe.path.to_string_lossy()).unwrap();
    let insert = compile(&format!(
        r#"song({{tracks:[track("p",note(60,1b),synth("pad"),{{chain:[plugin("probe",{{path:{path},note_controls:{{x:{{param:"p1",mode:"mod"}}}}}})]}})]}})"#
    ));
    assert!(insert.contains("effects receive no notes"), "{insert}");
    let mode = compile(&format!(
        r#"song({{tracks:[track("p",note(60,1b),plugin("probe",{{path:{path},note_controls:{{x:{{param:"p1"}}}}}}))]}})"#
    ));
    assert!(mode.contains("mode \"value\" or \"mod\""), "{mode}");
    let native =
        compile(r#"song({tracks:[track("p",phrase("C4:q").express({lift:0.2}),synth("pad"))]})"#);
    assert!(
        native.contains("unknown note expression 'lift'"),
        "{native}"
    );
}

#[test]
fn dawproject_reports_per_note_parameter_curves() {
    let probe = Probe::build();
    let song = probe.song(r#"phrase("C4:q").express({lift:0.2})"#, CONTROLS);
    let compiled = muz::compile::compile(&song).unwrap();
    let plan = muz::dawproject::Plan::new(&compiled, "bitwig-linux").unwrap();
    let entry = plan
        .report
        .warnings()
        .find(|e| e.code == "NOTE_PARAMETER_OMITTED")
        .expect("per-note parameter curve is reported");
    assert!(
        entry
            .intended
            .contains("note control 'lift': per-note mod curve on plugin parameter p1")
    );
}

#[test]
fn a_source_envelope_recipe_drives_per_note_modulation() {
    let probe = Probe::build();
    let path = serde_json::to_string(&probe.path.to_string_lossy()).unwrap();
    let file = probe.dir.path().join("recipe.muz");
    std::fs::write(
        &file,
        format!(
            r#"// A per-note envelope written in source: rise to `peak`, then fall back to 0.
fn swell(peak, attack = 40ms, decay = 400ms) {{
    [[0ms, 0], [attack, peak], [attack + decay, 0]]
}};
let synth = plugin("probe", {{path: {path}, class: "muz.test.note-param",
    note_controls: {{detune: {{param: "p1", mode: "mod"}}}}}});
song({{tempo: 120, tracks: [track("lead", phrase("C4:h C4:q").express({{detune: swell(0.25)}}), synth)], tail: 0}})"#
        ),
    )
    .unwrap();
    probe.log();
    probe.render(&file, 256).unwrap();
    let log = probe.log();
    let ons: Vec<_> = log.iter().filter(|e| e.kind == NOTE_ON).collect();
    assert_eq!(ons.len(), 2);
    for on in ons {
        let curve: Vec<_> = log
            .iter()
            .filter(|e| e.kind == PARAM_MOD && e.note_id == on.note_id)
            .map(|e| (e.frame - on.frame, e.value))
            .collect();
        assert_eq!(curve[0], (0, 0.0));
        // 40 ms is frame 1920, exactly step 15; 440 ms is frame 21120, step 165.
        assert_eq!(curve[15].0, 1920);
        assert!((curve[15].1 - 0.25).abs() < 1e-6, "{:?}", curve[15]);
        assert_eq!(curve[165].0, 21120);
        assert!(curve[165].1.abs() < 1e-6, "{:?}", curve[165]);
    }
}

/// A plugin that keeps the first note's ID through a mono-legato transition
/// hears the slurred note's expression only when it addresses that voice.
#[test]
fn legato_expression_addresses_the_voice_that_sounds_a_slurred_note() {
    let probe = Probe::build();
    let notes = r#"i.slur(stack([
        phrase("A4:h").express({volume: [[0, 1], [1, 0.2]]}),
        phrase("C5:h@slur").at(2b).express({tuning: 2})
    ]), "slur")"#;
    let expressions = |options: &str| {
        let song = probe.song_with(notes, options);
        probe.log();
        probe.render(&song, 256).unwrap();
        let log = probe.log();
        let ons: Vec<i32> = log
            .iter()
            .filter(|e| e.kind == NOTE_ON)
            .map(|e| e.note_id)
            .collect();
        assert_eq!(ons.len(), 2);
        let c_on = log
            .iter()
            .position(|e| e.kind == NOTE_ON && e.note_id == ons[1])
            .unwrap();
        let after: Vec<Entry> = log[c_on..]
            .iter()
            .filter(|e| e.kind == NOTE_EXPRESSION)
            .copied()
            .collect();
        (ons, after)
    };
    // Default: the slurred note's tuning addresses its own new note ID.
    let (ons, after) = expressions("render_thread: false");
    let tuning: Vec<_> = after.iter().filter(|e| e.param == 2).collect();
    assert!(tuning.iter().all(|e| e.note_id == ons[1]), "{tuning:?}");

    // Opt-in: volume returns to neutral on the first note's voice, then the
    // slurred note's tuning reaches that voice.
    let (ons, after) = expressions("legato_expression: true");
    assert!(after.iter().all(|e| e.note_id == ons[0]), "{after:?}");
    // The first note's falling volume, still held through the overlap, no
    // longer reaches the voice: only the neutral value does.
    let volume: Vec<_> = after.iter().filter(|e| e.param == 0).collect();
    assert_eq!(volume.len(), 1, "{volume:?}");
    assert_eq!(volume[0].value, 1.0);
    let last_tuning = after.iter().filter(|e| e.param == 2).last().unwrap();
    assert_eq!(last_tuning.value, 2.0);
}

/// An oscillating curve reaches a CLAP instrument as stepped expression values.
#[test]
fn oscillating_tuning_reaches_a_clap_instrument() {
    let probe = Probe::build();
    let song = probe.song_with(
        r#"note("A4", 4b).gate(1).express({tuning: {curve: 0, oscillate: {rate: 5Hz, depth: 0.2}}})"#,
        "render_thread: false",
    );
    probe.log();
    probe.render(&song, 256).unwrap();
    let tuning: Vec<f64> = probe
        .log()
        .into_iter()
        .filter(|e| e.kind == NOTE_EXPRESSION && e.param == 2)
        .map(|e| e.value)
        .collect();
    assert!(tuning.len() > 100, "{}", tuning.len());
    let (lo, hi) = tuning
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    assert!(lo < -0.19 && hi > 0.19 && lo >= -0.2 - 1e-6 && hi <= 0.2 + 1e-6, "{lo} {hi}");
}
