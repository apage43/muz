mod automation;
pub mod clap;
pub mod device;
pub mod engine;
pub mod pipewire;
pub mod transaction;
pub mod transport;
pub mod vst3;

pub use device::{
    AudioConfig, DeviceDebugState, DeviceError, DeviceEvent, DeviceEventKind, DeviceProcessor,
    ProcessContext, create_processor,
};
pub use engine::{AudioEngine, EngineError, EngineStatus, StructuralTransactionApplyError};
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

mod vst3_state;
