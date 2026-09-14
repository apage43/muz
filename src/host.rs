//! Instance-owned preparation services. Scopes are synchronous and unwind-safe;
//! they do not install services on the audio callback or other threads.
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone)]
pub struct HostContext {
    pub provenance: bool,
    pub expansion: crate::limits::ExpansionLimits,
    pub evaluation_steps: usize,
    pub graph_units: usize,
    pub cancelled: Arc<AtomicBool>,
    pub assets: Arc<dyn crate::assets::AssetResolver>,
    pub legacy_interrupt: bool,
}
impl Default for HostContext {
    fn default() -> Self {
        Self {
            provenance: false,
            expansion: Default::default(),
            evaluation_steps: 5_000_000,
            graph_units: 4096,
            cancelled: Arc::new(AtomicBool::new(false)),
            assets: Arc::new(crate::assets::FileAssets),
            legacy_interrupt: false,
        }
    }
}
thread_local! { static CURRENT:RefCell<Option<HostContext>>=const{RefCell::new(None)}; }
impl HostContext {
    pub fn cli() -> Self {
        let mut c = Self::default();
        c.legacy_interrupt = true;
        c.graph_units = crate::model::graph_budget().unwrap_or(0);
        c
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
            || (self.legacy_interrupt && crate::INTERRUPTED.load(Ordering::Relaxed))
    }
    pub fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        struct Restore(Option<HostContext>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|c| *c.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(CURRENT.with(|c| c.replace(Some(self.clone()))));
        f()
    }
}
pub(crate) fn current() -> Option<HostContext> {
    CURRENT.with(|c| c.borrow().clone())
}
