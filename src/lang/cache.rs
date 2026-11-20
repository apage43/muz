//! Evaluated modules kept across evaluations, for a host that recompiles a
//! piece after each edit. A module is reused only when its source text, the
//! resolution of each import and asset it read, the assets' versions and each
//! imported module's reused evaluation are unchanged, so the result is the one
//! a fresh evaluation would produce.
use super::{SourceFile, Value};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Evaluated modules a host keeps between compilations of one root.
///
/// Pass the same cache to [`super::load_with_cache`] or
/// [`crate::compile::compile_with_cache`] for each revision. It holds `Rc`
/// values, so it stays on the thread that compiles. A different root, loader
/// contrib root, provenance setting or budget clears it.
#[derive(Default)]
pub struct ModuleCache {
    pub(super) state: RefCell<State>,
    pub(super) reused: RefCell<Vec<PathBuf>>,
}
impl ModuleCache {
    pub fn new() -> Self {
        Self::default()
    }
    /// Forget every module.
    pub fn clear(&self) {
        *self.state.borrow_mut() = State::default();
    }
    /// The modules the last evaluation reused, in load order. Standard
    /// modules appear as `<std/name>`.
    pub fn reused(&self) -> Vec<PathBuf> {
        self.reused.borrow().clone()
    }
    /// Modules held.
    pub fn len(&self) -> usize {
        self.state.borrow().records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Default)]
pub(super) struct State {
    pub config: Option<Config>,
    pub records: HashMap<PathBuf, Rc<ModuleRecord>>,
}

/// Everything outside the modules' sources that an evaluation reads.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct Config {
    pub root: PathBuf,
    pub provenance: bool,
    pub expansion: (usize, usize, usize, usize),
    pub evaluation_steps: usize,
    pub contrib: Option<PathBuf>,
}

/// What a module's evaluation read, in order.
pub(super) enum Event {
    /// A module the evaluation loaded (or found loaded), with the generation of
    /// the record its value came from.
    Import {
        requested: PathBuf,
        resolved: PathBuf,
        generation: u64,
    },
    Standard {
        name: String,
        generation: u64,
    },
    /// A file a builtin (`midi`, `clip`) read.
    Asset {
        requested: PathBuf,
        resolved: PathBuf,
        version: (u64, u128),
    },
}

type MemoEntry = ((usize, String), (Vec<Value>, Value));

/// One module's evaluation: its value and what replaying it must restore.
pub(super) struct ModuleRecord {
    pub generation: u64,
    pub text: Arc<str>,
    pub source: Option<Arc<SourceFile>>,
    pub value: Value,
    pub events: Vec<Event>,
    /// Operations of its own top level (imports count for themselves).
    pub steps: usize,
    pub inclusive_steps: usize,
    /// Provenance bytes of its own top level.
    pub bytes: usize,
    pub inclusive_bytes: usize,
    /// The deepest call it reached, relative to the depth it was loaded at.
    pub depth: usize,
    pub interned: Vec<crate::provenance::Interned>,
    pub memo: Vec<MemoEntry>,
}

static GENERATION: AtomicU64 = AtomicU64::new(1);
pub(super) fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// What one module being evaluated has read so far.
pub(super) struct Recording {
    pub events: Vec<Event>,
    pub seen: std::collections::HashSet<PathBuf>,
    pub interned_from: usize,
    pub memo_from: usize,
    pub bytes_from: usize,
    pub nested_bytes: usize,
    pub entry_depth: usize,
    pub outer_max_depth: usize,
    /// Its value depends on more than its inputs (it read `steps()`).
    pub uncacheable: bool,
}

pub(super) fn standard_key(name: &str) -> PathBuf {
    PathBuf::from(format!("<std/{name}>"))
}
pub(super) fn is_standard(key: &Path) -> bool {
    key.as_os_str().to_string_lossy().starts_with("<std/")
}
