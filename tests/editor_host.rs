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

#[test]
fn expanded_kit_groups_preserve_authored_membership_in_snapshots() {
    let host = documents(&[(
        "song.muz",
        r#"
        song({tracks: [
            track("rack.a", phrase("C4:q").map_notes(fn(n) => {voice: "metal.high"}),
                kit("custom", {"metal.high": synth("init")})),
            track("rack.a.lead", phrase("E4:q"), synth("init")),
            track("second", drums({kick:"X...",hat:"x.x."}), kit()),
            track("silent", drums({kick:"...."}), kit())
        ], tail: 0})
    "#,
    )]);
    let compiled = muz::compile::compile_with_loader(Path::new("song.muz"), host).unwrap();
    let groups = muz::inspect::session(&compiled.session, "track_groups").unwrap();
    assert_eq!(
        groups,
        serde_json::json!([
            {"id":"rack.a", "kind":"kit", "members":[{"track":"rack.a.metal.high","label":"metal.high"}]},
            {"id":"second", "kind":"kit", "members":[{"track":"second.hat","label":"hat"},{"track":"second.kick","label":"kick"}]},
            {"id":"silent", "kind":"kit", "members":[]}
        ])
    );
    assert!(compiled.locations.contains_key("track.rack.a"));
    assert!(compiled.locations.contains_key("track.rack.a.metal.high"));
    let snapshot = serde_json::to_value(&compiled.session).unwrap();
    let restored: muz::Session = serde_json::from_value(snapshot.clone()).unwrap();
    assert_eq!(
        restored.extras.track_groups,
        compiled.session.extras.track_groups
    );
    let mut old = snapshot;
    old["extras"]
        .as_object_mut()
        .unwrap()
        .remove("track_groups");
    assert!(
        serde_json::from_value::<muz::Session>(old)
            .unwrap()
            .extras
            .track_groups
            .is_empty()
    );
}

#[test]
fn evaluated_root_classification_matches_loader_semantics_without_readiness() {
    use muz::lang::RootClassification;
    let context = muz::host::HostContext::default();
    for source in [
        "song({tracks:[]})",
        "let main = song({tracks:[]}); null",
        "let make = fn() => song({tracks:[]}); make()",
        "use \"part.muz\" as p; p.make()",
        "use \"contrib/kit/part\" as p; p.make()",
        // A root type is a fact even when lowering must reject its contents.
        "{type:\"song\",tracks:false}",
        "song({tracks:[track(\"a\",note(60),sample(\"missing.wav\"))]})",
        "song({tracks:[track(\"a\",note(60),sfz(\"missing.sfz\"))]})",
        "song({tracks:[track(\"a\",note(60),synth(\"pad\"))],automation:[automation(\"absent.gain\",curve([[0b,0]]))]})",
    ] {
        let loader = documents(&[
            ("root.muz", source), ("part.muz", "let make = fn() => song({tracks:[]});"),
            ("library/kit/part.muz", "let make = fn() => song({tracks:[]});"),
        ]);
        let loaded = context.run(|| muz::lang::load_with_loader(Path::new("root.muz"), loader.clone())).unwrap();
        assert_eq!(loaded.0.get("type").unwrap().text().unwrap(), "song");
        match muz::lang::classify_root_with_context(Path::new("root.muz"), loader, &context).unwrap() {
            RootClassification::Song { dependencies } => assert_eq!(dependencies, loaded.1),
            other => panic!("{source}: {other:?}"),
        }
    }
    for source in ["null", "note(60)", "{answer:42}", "{type:\"pattern\"}", "{type:42}",
        "\"song({tracks:[]})\"", "// song({tracks:[]})\n42", "let main = null; song({tracks:[]})"] {
        match muz::lang::classify_root_with_context(Path::new("root.muz"), documents(&[("root.muz", source)]), &context).unwrap() {
            RootClassification::NonSong { description, dependencies } => {
                assert!(!description.is_empty());
                assert_eq!(dependencies, vec![PathBuf::from("root.muz")]);
            }
            other => panic!("{source}: {other:?}"),
        }
    }
}

#[test]
fn unresolved_roots_keep_diagnostics_but_cancellation_and_limits_abort() {
    use muz::lang::RootClassification;
    let context = muz::host::HostContext {
        assets: std::sync::Arc::new(muz::assets::MemoryAssets::default()), ..Default::default()
    };
    for files in [
        vec![("root.muz", "song({")],
        vec![("root.muz", "use \"missing.muz\" as p; p")],
        vec![("root.muz", "use \"other.muz\" as p; p"), ("other.muz", "use \"root.muz\" as p; p")],
        vec![("root.muz", "use \"other.muz\" as p; p.make()"), ("other.muz", "let make = fn() => invalid_name(1);")],
        vec![("root.muz", "midi(\"missing.mid\")")],
    ] {
        let loader = documents(&files);
        let original = context.run(|| muz::lang::load_with_loader(Path::new("root.muz"), loader.clone())).unwrap_err();
        match muz::lang::classify_root_with_context(Path::new("root.muz"), loader, &context).unwrap() {
            RootClassification::Unresolved { error } => {
                assert_eq!(error.to_string(), original.to_string());
                assert_eq!(error.downcast_ref::<muz::lang::Diagnostic>().unwrap().to_json(),
                    original.downcast_ref::<muz::lang::Diagnostic>().unwrap().to_json());
            }
            other => panic!("{other:?}"),
        }
    }
    let tiny = muz::host::HostContext { evaluation_steps: 0, ..Default::default() };
    let error = muz::lang::classify_root_with_context(Path::new("root.muz"),
        documents(&[("root.muz", "song({tracks:[]})")]), &tiny).unwrap_err();
    assert!(error.downcast_ref::<muz::lang::Diagnostic>().unwrap().is_limit());
    let tiny = muz::host::HostContext {
        expansion: muz::limits::ExpansionLimits { notes: 0, ..Default::default() }, ..Default::default()
    };
    assert!(muz::lang::classify_root_with_context(Path::new("root.muz"),
        documents(&[("root.muz", "note(60).repeat(2)")]), &tiny).is_err());
    context.cancelled.store(true, std::sync::atomic::Ordering::Release);
    assert!(muz::lang::classify_root_with_context(Path::new("root.muz"),
        documents(&[("root.muz", "song({tracks:[]})")]), &context).is_err());
}

#[test]
fn classification_never_reads_lowering_assets_and_deleted_overlays_stay_unresolved() {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    struct Deny(AtomicUsize);
    impl muz::assets::AssetResolver for Deny {
        fn resolve(&self, _: &Path) -> anyhow::Result<PathBuf> {
            self.0.fetch_add(1, Ordering::Relaxed); anyhow::bail!("asset access denied")
        }
        fn version(&self, _: &Path) -> anyhow::Result<(u64, u128)> {
            self.0.fetch_add(1, Ordering::Relaxed); anyhow::bail!("asset access denied")
        }
        fn open(&self, _: &Path) -> anyhow::Result<Box<dyn muz::assets::AssetReader>> {
            self.0.fetch_add(1, Ordering::Relaxed); anyhow::bail!("asset access denied")
        }
    }
    let assets = Arc::new(Deny(AtomicUsize::new(0)));
    let context = muz::host::HostContext { assets: assets.clone(), ..Default::default() };
    for source in [
        "song({tracks:[track(\"a\",note(60),sample(\"missing.wav\"))]})",
        "song({tracks:[track(\"a\",note(60),sfz(\"missing.sfz\"))]})",
        "song({tracks:[track(\"a\",note(60),plugin(\"unconfigured\"))]})",
        "song({tracks:[track(\"a\",note(60),plugin(\"missing.clap\"))]})",
    ] {
        let loader = documents(&[("root.muz", source)]);
        assert!(matches!(muz::lang::classify_root_with_context(Path::new("root.muz"), loader.clone(), &context).unwrap(),
            muz::lang::RootClassification::Song { .. }));
        assert_eq!(assets.0.load(Ordering::Relaxed), 0);
        assert!(muz::compile::compile_with_context(Path::new("root.muz"), loader, &context).is_err());
        assets.0.store(0, Ordering::Relaxed);
    }
    let source = "use \"new.muz\" as p; p.main";
    assert!(matches!(muz::lang::classify_root_with_context(Path::new("root.muz"),
        documents(&[("root.muz", source), ("new.muz", "let main = song({tracks:[]});")]), &context).unwrap(),
        muz::lang::RootClassification::Song { .. }));
    assert!(matches!(muz::lang::classify_root_with_context(Path::new("root.muz"),
        documents(&[("root.muz", source)]), &context).unwrap(),
        muz::lang::RootClassification::Unresolved { .. }));
}

#[test]
fn classifier_preserves_evaluation_cancellation_and_checks_completed_empty_roots() {
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    struct Interrupt { source: &'static str, cancelled: Arc<AtomicBool> }
    impl SourceLoader for Interrupt {
        fn resolve(&self, path: &Path) -> anyhow::Result<PathBuf> { Ok(path.to_owned()) }
        fn read(&self, _: &Path) -> anyhow::Result<String> {
            self.cancelled.store(true, Ordering::Release);
            Ok(self.source.to_owned())
        }
        fn contrib_root(&self) -> anyhow::Result<PathBuf> { Ok("library".into()) }
    }
    for source in ["", "song({tracks:[]})"] {
        let context = muz::host::HostContext::default();
        let error = muz::lang::classify_root_with_context(Path::new("root.muz"),
            Rc::new(Interrupt { source, cancelled: context.cancelled.clone() }), &context).unwrap_err();
        assert!(context.is_cancelled());
        if !source.is_empty() {
            let diagnostic = error.downcast_ref::<muz::lang::Diagnostic>().unwrap().to_json();
            assert_eq!(diagnostic["location"]["path"], "root.muz");
            assert_eq!(diagnostic["message"], "evaluation cancelled");
        }
    }
}

#[test]
fn evaluation_resource_budget_failure_aborts_instead_of_classifying_unresolved() {
    struct Oversized;
    impl muz::assets::AssetResolver for Oversized {
        fn resolve(&self, path: &Path) -> anyhow::Result<PathBuf> { Ok(path.to_owned()) }
        fn version(&self, _: &Path) -> anyhow::Result<(u64, u128)> { Ok((u64::MAX, 1)) }
        fn open(&self, _: &Path) -> anyhow::Result<Box<dyn muz::assets::AssetReader>> {
            panic!("oversized asset must reject before opening")
        }
    }
    let context = muz::host::HostContext { assets: std::sync::Arc::new(Oversized), ..Default::default() };
    let error = muz::lang::classify_root_with_context(Path::new("root.muz"),
        documents(&[("root.muz", "midi(\"oversized.mid\")")]), &context).unwrap_err();
    let diagnostic = error.downcast_ref::<muz::lang::Diagnostic>().unwrap();
    assert!(diagnostic.is_limit());
    assert_eq!(diagnostic.to_json()["location"]["path"], "root.muz");
}

#[test]
fn classifier_marks_builtin_phrase_and_provenance_budget_producers() {
    let classify = |source: &str, context: &muz::host::HostContext| {
        muz::lang::classify_root_with_context(Path::new("root.muz"),
            documents(&[("root.muz", source)]), context)
    };
    let context = muz::host::HostContext::default();
    for source in ["range(200001)", "range(200001,start=0)", "range(200000.1)"] {
        let error = classify(source, &context).unwrap_err();
        assert!(error.downcast_ref::<muz::lang::Diagnostic>().unwrap().is_limit());
    }
    assert!(matches!(classify("range(-1)", &context).unwrap(), muz::lang::RootClassification::Unresolved { .. }));
    let phrase = format!("phrase({})", serde_json::to_string(&"C4:q ".repeat(200_001)).unwrap());
    let error = classify(&phrase, &context).unwrap_err();
    assert!(error.downcast_ref::<muz::lang::Diagnostic>().unwrap().is_limit());
    let (note, _) = context.run(|| muz::lang::load_with_loader(Path::new("root.muz"),
        documents(&[("root.muz", "note(60)")]))).unwrap();
    let bytes = muz::limits::ExpansionCost::of(note.pattern().unwrap()).unwrap().bytes;
    let provenance = muz::host::HostContext {
        provenance: true, expansion: muz::limits::ExpansionLimits { bytes, ..Default::default() },
        ..Default::default()
    };
    let error = classify("note(60)", &provenance).unwrap_err();
    let diagnostic = error.downcast_ref::<muz::lang::Diagnostic>().unwrap();
    assert!(diagnostic.is_limit());
    assert!(diagnostic.to_string().contains("provenance arena"));
    let mut source = "let p0 = note(60);\n".to_owned();
    for index in 1..=129 { source.push_str(&format!("let p{index} = p{}.at(0b);\n", index - 1)); }
    source.push_str("p129");
    let error = classify(&source, &muz::host::HostContext { provenance: true, ..Default::default() }).unwrap_err();
    let diagnostic = error.downcast_ref::<muz::lang::Diagnostic>().unwrap();
    assert!(diagnostic.is_limit());
    assert!(diagnostic.to_string().contains("provenance occurrence depth"));
}
