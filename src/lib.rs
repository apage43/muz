pub mod audio;
pub mod control;
pub mod live;
pub mod midi;
pub mod model;
pub mod reconcile;
pub mod source;
pub mod watch;

pub use model::Session;
pub use reconcile::{ReconcilePlan, plan_reconciliation};
pub use source::{SourceError, parse_project, parse_session_with_root, resolve_project_asset};

pub mod analysis;
pub mod compile;
pub mod lang;
pub mod music;
pub mod performance;
pub mod render;

pub mod plugins;

pub mod inspect;

pub mod worker;

pub mod smf;

pub static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
