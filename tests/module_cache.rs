//! A host that recompiles after each edit reuses unchanged evaluated modules
//! and normalized SFZ programs; the result must be the fresh compilation's.
use muz::{
    assets::MemoryAssets,
    compile::{Compiled, compile_with_cache, compile_with_context},
    host::HostContext,
    lang::{ModuleCache, SourceLoader},
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

#[derive(Clone, Default)]
struct Sources(Rc<RefCell<BTreeMap<PathBuf, String>>>);
impl Sources {
    fn set(&self, path: &str, text: &str) {
        self.0.borrow_mut().insert(path.into(), text.into());
    }
    fn edit(&self, path: &str, from: &str, to: &str) {
        let mut files = self.0.borrow_mut();
        let text = files.get_mut(Path::new(path)).unwrap();
        assert!(text.contains(from), "{from} in {path}");
        *text = text.replacen(from, to, 1);
    }
}
impl SourceLoader for Sources {
    fn resolve(&self, path: &Path) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(self.0.borrow().contains_key(path), "missing {}", path.display());
        Ok(path.to_owned())
    }
    fn read(&self, path: &Path) -> anyhow::Result<String> {
        Ok(self.0.borrow()[path].clone())
    }
    fn contrib_root(&self) -> anyhow::Result<PathBuf> {
        Ok("/contrib".into())
    }
}

/// Everything a compilation exposes, provenance explanations included.
fn fingerprint(c: &Compiled) -> String {
    let mut text = serde_json::to_string(c).unwrap();
    for track in &c.score {
        let mut offset = 0;
        loop {
            let page = c.explain_notes(&track.id, offset, 1000).unwrap();
            text.push_str(&serde_json::to_string(&page.rows).unwrap());
            match page.next {
                Some(next) => offset = next,
                None => break,
            }
        }
    }
    text
}

fn midi(pitches: &[u8]) -> Vec<u8> {
    use muz::smf::{Document, Event, Kind};
    let mut events = vec![];
    for &pitch in pitches {
        events.push(Event { delta: 0, kind: Kind::Midi { bytes: vec![0x90, pitch, 100] } });
        events.push(Event { delta: 240, kind: Kind::Midi { bytes: vec![0x80, pitch, 0] } });
    }
    events.push(Event { delta: 0, kind: Kind::Meta { tag: 47, data: vec![] } });
    muz::smf::encode(&Document { format: 0, division: 480, tracks: vec![events] }).unwrap()
}

fn context(riff: &[u8], version: u128, provenance: bool) -> HostContext {
    let mut assets = MemoryAssets::default();
    assets.insert(PathBuf::from("/p/riff.mid"), riff.to_vec().into(), version);
    HostContext {
        provenance,
        assets: Arc::new(assets),
        ..Default::default()
    }
}

/// Compile with `cache`, check the result against a fresh compilation and
/// return the modules the cached compilation reused, sorted.
fn recompile(sources: &Sources, cache: &Rc<ModuleCache>, context: &HostContext) -> Vec<String> {
    let root = Path::new("/p/song.muz");
    let cached = compile_with_cache(root, Rc::new(sources.clone()), context, cache).unwrap();
    let fresh_context = HostContext { assets: context.assets.clone(), ..context.clone() };
    let fresh = compile_with_context(root, Rc::new(sources.clone()), &fresh_context).unwrap();
    assert_eq!(fingerprint(&cached), fingerprint(&fresh));
    cache
        .reused()
        .into_iter()
        .map(|p| p.display().to_string())
        .filter(|p| !p.starts_with("<std"))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[test]
fn a_recompile_after_an_edit_reuses_unchanged_modules_and_matches_a_fresh_one() {
    let sources = Sources::default();
    sources.set(
        "/p/song.muz",
        r#"use "parts.muz" as parts;
use "mix.muz" as mix;
song({tracks: [
  track("lead", parts.lead, synth("bell"), {gain: mix.lead_gain}),
  track("bass", parts.bass, synth("bell"), {gain: mix.bass_gain}),
]})"#,
    );
    sources.set(
        "/p/parts.muz",
        r#"use "motifs.muz" as m;
let lead = seq([m.phrase(0), m.phrase(5), midi("riff.mid")]);
let bass = transpose(m.phrase(-24), 0);"#,
    );
    sources.set(
        "/p/motifs.muz",
        r#"let motif = seq([note(60, 1b), note(64, 1b), note(67, 2b)]);
let lifted = memo(fn(p, by) => transpose(p, by));
fn phrase(by) = lifted(motif, by);"#,
    );
    sources.set("/p/mix.muz", "let lead_gain = -4;\nlet bass_gain = -6;");
    let cache = Rc::new(ModuleCache::new());
    let riff = midi(&[62, 65]);
    let traced = context(&riff, 1, true);

    // Nothing kept yet.
    assert!(recompile(&sources, &cache, &traced).is_empty());
    // Unchanged: every module, the root included, is reused.
    assert_eq!(
        recompile(&sources, &cache, &traced),
        ["/p/mix.muz", "/p/motifs.muz", "/p/parts.muz", "/p/song.muz"]
    );
    // An edited module and the root that imports it are evaluated again.
    sources.edit("/p/mix.muz", "-4", "-5");
    assert_eq!(recompile(&sources, &cache, &traced), ["/p/motifs.muz", "/p/parts.muz"]);
    // An edit deep in the graph re-evaluates every module above it.
    sources.edit("/p/motifs.muz", "note(64, 1b)", "note(63, 1b)");
    assert_eq!(recompile(&sources, &cache, &traced), ["/p/mix.muz"]);
    // A changed asset version re-evaluates the module that read it.
    let traced = context(&midi(&[62, 66, 69]), 2, true);
    assert_eq!(recompile(&sources, &cache, &traced), ["/p/mix.muz", "/p/motifs.muz"]);
    // Different settings start again.
    let plain = context(&midi(&[62, 66, 69]), 2, false);
    assert!(recompile(&sources, &cache, &plain).is_empty());
    assert_eq!(recompile(&sources, &cache, &plain).len(), 4);
    // A module that reads the evaluation's own state is never kept.
    sources.edit("/p/mix.muz", "let bass_gain = -6;", "let bass_gain = -6 + 0 * steps();");
    recompile(&sources, &cache, &plain);
    assert_eq!(recompile(&sources, &cache, &plain), ["/p/motifs.muz", "/p/parts.muz"]);
}

#[test]
fn a_kept_sfz_program_is_shared_until_a_dependency_changes() {
    let dir = tempfile::tempdir().unwrap();
    let sfz = dir.path().join("instrument.sfz");
    std::fs::write(&sfz, "<region> sample=*silence key=60").unwrap();
    let main = dir.path().join("main.muz");
    std::fs::write(
        &main,
        r#"song({tracks:[track("x",note(60,1b),sfz("instrument.sfz")),track("y",note(62,1b),sfz("instrument.sfz"))]})"#,
    )
    .unwrap();
    let context = HostContext::default();
    let program = |c: &Compiled, track: usize| c.session.tracks[track].instrument.sfz.clone().unwrap().program;
    let compile = || compile_with_context(&main, Rc::new(muz::lang::FileSourceLoader), &context).unwrap();
    let first = compile();
    assert!(Arc::ptr_eq(&program(&first, 0), &program(&first, 1)));
    let second = compile();
    assert!(Arc::ptr_eq(&program(&first, 0), &program(&second, 0)));
    assert_eq!(context.sfz_sources.len(), 1);
    // A changed source (and so a changed version) loads the program again.
    std::fs::write(&sfz, "<region> sample=*silence key=60 volume=-3").unwrap();
    let third = compile();
    assert!(!Arc::ptr_eq(&program(&second, 0), &program(&third, 0)));
    assert_eq!(program(&third, 0).regions[0].opcodes["volume"], "-3");
    let fresh = compile_with_context(&main, Rc::new(muz::lang::FileSourceLoader), &HostContext::default()).unwrap();
    assert_eq!(fingerprint(&third), fingerprint(&fresh));
}
