mod automation;
#[cfg(feature = "desktop")]
pub mod clap;
pub mod device;
pub mod engine;
#[cfg(feature = "desktop")]
pub mod pipewire;
pub mod transaction;
pub mod transport;
#[cfg(feature = "desktop")]
pub mod vst3;

pub use device::{
    AudioConfig, DeviceDebugState, DeviceError, DeviceEvent, DeviceEventKind, DeviceProcessor,
    ProcessContext, create_processor,
};
pub use engine::{
    AudioEngine, EngineError, EngineFade, EngineStatus, FadeDirection,
    StructuralTransactionApplyError,
};
#[cfg(feature = "desktop")]
pub use pipewire::{
    PipeWireError, PipeWireOutput, PipeWireStatus, RuntimeTelemetrySnapshot, TransactionQueueFull,
};
pub use transaction::{
    DeviceSlot, PreparedStructuralTransaction, PreparedTransaction, PreparedValueOperation,
    PreparedValueTransaction, StructuralTransactionPrepareError, TransactionApplyError,
    TransactionPrepareError, TransactionReceipt, ValueTransactionApplyError,
    ValueTransactionPrepareError, ValueTransactionReceipt,
};
pub use transport::{
    DeliveredEvents, PatternScheduler, RuntimeTransport, ScheduleError, ScheduledEvents,
    TransportBlock, TransportSnapshot,
};

pub const MAX_AUDIO_FRAMES: usize = 1_024;
pub const MAX_EVENTS_PER_BLOCK: usize = 256;
pub const MAX_ACTIVE_NOTES: usize = 256;
pub const MAX_SYNTH_VOICES: usize = 16;

#[cfg(feature = "desktop")]
mod vst3_state;
