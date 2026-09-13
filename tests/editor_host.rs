use muz::lang::SourceLoader;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
};

struct Memory(BTreeMap<String, String>);
impl SourceLoader for Memory {
    fn resolve(&self, path: &Path) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(
            self.0.contains_key(path.to_str().unwrap()),
            "missing document"
        );
        Ok(path.to_owned())
    }
    fn read(&self, path: &Path) -> anyhow::Result<String> {
        Ok(self.0[path.to_str().unwrap()].clone())
    }
    fn contrib_root(&self) -> anyhow::Result<PathBuf> {
        Ok("library".into())
    }
}
fn documents(files: &[(&str, &str)]) -> Rc<dyn SourceLoader> {
    Rc::new(Memory(
        files
            .iter()
            .map(|(p, s)| (p.to_string(), s.to_string()))
            .collect(),
    ))
}

#[test]
fn editor_compiles_unsaved_imports_and_retains_declaration_locations() {
    let host = documents(&[
        (
            "song.muz",
            "use \"part.muz\" as p;\nsong({tracks: [p.line]})",
        ),
        (
            "part.muz",
            "use \"std/synthesis\" as s;\nlet line = track(\"lead\", phrase(\"C4:q E4:q\"), s.pluck());",
        ),
    ]);
    let compiled = muz::compile::compile_with_loader(Path::new("song.muz"), host).unwrap();
    assert_eq!(compiled.session.tracks.len(), 1);
    let location = serde_json::to_value(&compiled.locations["track.lead"]).unwrap();
    assert_eq!(location["path"], "part.muz");
    assert_eq!(location["line"], 2);
    let mut engine = muz::audio::AudioEngine::new(
        &compiled.session,
        muz::audio::AudioConfig {
            sample_rate: 48000.,
            max_frames: 1024,
            offline: true,
        },
    )
    .unwrap();
    engine.set_running(true);
    let mut audio = vec![0.; 2048];
    engine.render_interleaved(&mut audio, 2).unwrap();
    assert!(audio.iter().all(|v| v.is_finite()));
    assert!(audio.iter().any(|v| v.abs() > 0.0001));
}

#[test]
fn host_contrib_and_cycles_follow_the_same_loader() {
    let host = documents(&[
        ("song.muz", "use \"contrib/kit/part\" as p; p.answer"),
        ("library/kit/part.muz", "let answer = 42;"),
    ]);
    let (value, deps) = muz::lang::load_with_loader(Path::new("song.muz"), host).unwrap();
    assert_eq!(value.number().unwrap(), 42.);
    assert!(deps.contains(&PathBuf::from("library/kit/part.muz")));
    let cycle = documents(&[
        ("a.muz", "use \"b.muz\" as b; b"),
        ("b.muz", "use \"a.muz\" as a; a"),
    ]);
    let error = muz::lang::load_with_loader(Path::new("a.muz"), cycle)
        .err()
        .unwrap();
    assert!(error.to_string().contains("circular import"));
}

#[test]
fn structured_diagnostic_names_the_imported_document() {
    let host = documents(&[
        ("song.muz", "use \"bad.muz\" as b; b"),
        ("bad.muz", "// café\nlet x = not_a_function(1);"),
    ]);
    let error = muz::lang::load_with_loader(Path::new("song.muz"), host)
        .err()
        .unwrap();
    let diagnostic = error
        .downcast_ref::<muz::lang::Diagnostic>()
        .unwrap()
        .to_json();
    assert_eq!(diagnostic["location"]["path"], "bad.muz");
    assert_eq!(diagnostic["location"]["line"], 2);
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("not_a_function")
    );
}
