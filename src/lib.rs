#[cfg(feature = "desktop")]
pub mod analysis;
pub mod assets;
pub mod audio;
pub mod audio_file;
pub mod compile;
#[cfg(feature = "desktop")]
pub mod control;
pub mod description;
pub mod expression;
pub mod inspect;
pub mod lang;
pub mod limits;
#[cfg(feature = "desktop")]
pub mod live;
pub mod midi;
pub mod model;
pub mod music;
mod patch_source;
pub mod performance;
#[cfg(feature = "desktop")]
pub mod plugins;
#[cfg(feature = "desktop")]
pub mod recipes;
pub mod reconcile;
#[cfg(feature = "desktop")]
pub mod render;
pub mod smf;
pub mod source;
pub mod tonal;
#[cfg(feature = "desktop")]
pub mod watch;
#[cfg(feature = "desktop")]
pub mod worker;

pub use model::Session;
pub use reconcile::{ReconcilePlan, plan_reconciliation};
pub use source::{SourceError, parse_project, parse_session_with_root, resolve_project_asset};
pub static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
