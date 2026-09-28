//! Narrow, allocation-free-at-process-time VST3 instrument hosting.
//!
//! Loading, initialization, state changes, and destruction belong on the coordinator thread.
//! [`PreparedVst3::process`] is the only audio-thread operation; all COM callback objects and their
//! storage are allocated while preparing the instance.

use std::{
    cell::UnsafeCell,
    ffi::{c_char, c_void},
    fmt,
    mem::MaybeUninit,
    path::{Path, PathBuf},
    ptr,
    str::FromStr,
};

use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};
use serde::Serialize;
use thiserror::Error;
use vst3::{
    Class, ComPtr, ComWrapper, Interface,
    Steinberg::Vst::{
        AudioBusBuffers, AudioBusBuffers__type0,
        BusDirections_::*,
        BusInfo, Event,
        Event_::EventTypes_::{kNoteOffEvent, kNoteOnEvent, kPolyPressureEvent},
        Event__type0, IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentHandler,
        IComponentHandlerTrait, IComponentTrait, IConnectionPoint, IConnectionPointTrait,
        IEditController, IEditControllerTrait, IEventList, IEventListTrait, IHostApplication,
        IHostApplicationTrait, IMidiMapping, IMidiMappingTrait, IParamValueQueue,
        IParamValueQueueTrait, IParameterChanges, IParameterChangesTrait,
        IoModes_::kSimple,
        MediaTypes_::{kAudio, kEvent},
        NoteOffEvent, NoteOnEvent, ParamID, ParameterInfo, PolyPressureEvent, ProcessContext,
        ProcessContext_::StatesAndFlags_::{
            kBarPositionValid, kContTimeValid, kPlaying, kProjectTimeMusicValid, kTempoValid,
            kTimeSigValid,
        },
        ProcessData,
        ProcessModes_::{kOffline, kRealtime},
        ProcessSetup, SpeakerArr,
        SymbolicSampleSizes_::kSample32,
    },
    Steinberg::{
        self, FUnknown, IPluginBaseTrait, IPluginFactory, IPluginFactory2, IPluginFactory2Trait,
        IPluginFactory3, IPluginFactory3Trait, IPluginFactoryTrait, PClassInfo, PClassInfo2,
        PFactoryInfo, TUID, kInvalidArgument, kNoInterface, kNotImplemented, kResultFalse,
        kResultOk,
    },
};

pub const VST3_SAMPLE_RATE: f64 = 48_000.0;
pub const VST3_MAX_FRAMES: usize = 8192;
pub const VST3_EVENT_CAPACITY: usize = 512;
pub const VST3_PARAMETER_QUEUE_CAPACITY: usize = 2048;
pub const MAX_PROBE_REPEATS: usize = 64;
const PEDAL_CONTROLLERS: [u8; 3] = [64, 66, 67];
const AUDIO_MODULE_CATEGORY: &str = "Audio Module Class";

type ModuleEntry = unsafe extern "system" fn(*mut c_void) -> bool;
type ModuleExit = unsafe extern "system" fn() -> bool;
type GetPluginFactory = unsafe extern "system" fn() -> *mut IPluginFactory;

/// An exact VST3 16-byte class identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Vst3ClassId(pub [u8; 16]);

impl Vst3ClassId {
    fn as_tuid(self) -> TUID {
        self.0.map(|byte| byte as c_char)
    }

    fn from_tuid(value: TUID) -> Self {
        Self(value.map(|byte| byte as u8))
    }
}

impl fmt::Display for Vst3ClassId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl Serialize for Vst3ClassId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("class ID must contain exactly 32 hexadecimal digits")]
pub struct Vst3ClassIdParseError;

impl FromStr for Vst3ClassId {
    type Err = Vst3ClassIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let digits = value.as_bytes();
        if digits.len() != 32 || !digits.iter().all(u8::is_ascii_hexdigit) {
            return Err(Vst3ClassIdParseError);
        }

        let mut id = [0_u8; 16];
        for (index, target) in id.iter_mut().enumerate() {
            let high = hex_nibble(digits[index * 2]).ok_or(Vst3ClassIdParseError)?;
            let low = hex_nibble(digits[index * 2 + 1]).ok_or(Vst3ClassIdParseError)?;
            *target = (high << 4) | low;
        }
        Ok(Self(id))
    }
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Vst3Metadata {
    pub module: PathBuf,
    pub class_id: Vst3ClassId,
    pub class_name: String,
    pub category: String,
    pub subcategories: String,
    pub vendor: String,
    pub version: String,
    pub sdk_version: String,
    pub factory_vendor: String,
    pub latency_samples: u32,
    pub tail_samples: u32,
    pub event_input_channels: i32,
    pub output_channels: i32,
    pub pedal_parameter_ids: [u32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Vst3Event {
    Midi {
        sample_offset: usize,
        bytes: [u8; 3],
        len: u8,
    },
    NoteOn {
        tuning: f32,
        sample_offset: usize,
        channel: u8,
        pitch: u8,
        velocity: f32,
        note_id: i32,
    },
    NoteOff {
        sample_offset: usize,
        channel: u8,
        pitch: u8,
        velocity: f32,
        note_id: i32,
    },
    Pedal {
        sample_offset: usize,
        controller: u8,
        value: u8,
    },
}

impl Vst3Event {
    fn sample_offset(self) -> usize {
        match self {
            Self::Midi { sample_offset, .. }
            | Self::NoteOn { sample_offset, .. }
            | Self::NoteOff { sample_offset, .. }
            | Self::Pedal { sample_offset, .. } => sample_offset,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Vst3TimeContext {
    pub continuous_time_samples: i64,
    pub project_time_samples: i64,
    pub project_time_music: f64,
    pub bar_position_music: f64,
    pub tempo: f64,
    pub time_signature_numerator: i32,
    pub time_signature_denominator: i32,
    pub playing: bool,
}

impl Default for Vst3TimeContext {
    fn default() -> Self {
        Self {
            continuous_time_samples: 0,
            project_time_samples: 0,
            project_time_music: 0.0,
            bar_position_music: 0.0,
            tempo: 120.0,
            time_signature_numerator: 4,
            time_signature_denominator: 4,
            playing: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Vst3ProcessReport {
    pub process_count: u64,
    pub output_silence_flags: u64,
}

#[derive(Debug, Error)]
pub enum Vst3Error {
    #[error("unknown, read-only or invalid plugin parameter (values are normalized 0..1)")]
    UnknownParameter,
    #[error("VST3 bundle or module does not exist: {0}")]
    BundleNotFound(PathBuf),
    #[error("VST3 bundle has no file name: {0}")]
    InvalidBundle(PathBuf),
    #[error("VST3 Linux module does not exist: {0}")]
    ModuleNotFound(PathBuf),
    #[error("failed to load VST3 module {path}: {source}")]
    LoadModule {
        path: PathBuf,
        #[source]
        source: libloading::Error,
    },
    #[error("VST3 module {path} does not export {symbol}: {source}")]
    MissingSymbol {
        path: PathBuf,
        symbol: &'static str,
        #[source]
        source: libloading::Error,
    },
    #[error("VST3 ModuleEntry returned false")]
    ModuleEntryFailed,
    #[error("VST3 GetPluginFactory returned null")]
    NullFactory,
    #[error("VST3 call {operation} failed with tresult {result}")]
    CallFailed {
        operation: &'static str,
        result: Steinberg::tresult,
    },
    #[error("VST3 audio class {0} was not found")]
    ClassNotFound(Vst3ClassId),
    #[error("VST3 class {class_id} has category {category:?}, not {AUDIO_MODULE_CATEGORY:?}")]
    NotAudioModule {
        class_id: Vst3ClassId,
        category: String,
    },
    #[error("VST3 class metadata requires IPluginFactory2")]
    MissingFactory2,
    #[error("VST3 version mismatch: expected {expected:?}, found {actual:?}")]
    VersionMismatch { expected: String, actual: String },
    #[error("VST3 component does not implement {0}")]
    MissingInterface(&'static str),
    #[error("VST3 component has no event input bus")]
    MissingEventInput,
    #[error("VST3 component has no audio output bus")]
    MissingAudioOutput,
    #[error("VST3 output bus has {0} channels; stereo is required")]
    NonStereoOutput(i32),
    #[error("VST3 event input reports invalid channel count {0}")]
    InvalidEventChannels(i32),
    #[error("VST3 controller has no MIDI mapping for CC{0}")]
    MissingMidiMapping(u8),
    #[error("VST3 controller maps multiple required pedals to parameter {0}")]
    DuplicateMidiMapping(u32),
    #[error("audio block has {frames} frames; expected 1..={VST3_MAX_FRAMES}")]
    InvalidBlockSize { frames: usize },
    #[error("left and right output buffers must have equal lengths")]
    MismatchedOutputBuffers,
    #[error("block contains {count} events; capacity is {VST3_EVENT_CAPACITY}")]
    EventCapacityExceeded { count: usize },
    #[error("events are not ordered by nondecreasing sample offset")]
    EventsOutOfOrder,
    #[error("event sample offset {offset} is outside the {frames}-frame block")]
    EventOutsideBlock { offset: usize, frames: usize },
    #[error("invalid MIDI channel {0}; expected 0..=15")]
    InvalidMidiChannel(u8),
    #[error("invalid note velocity {0}; expected a finite value in 0..=1")]
    InvalidVelocity(f32),
    #[error("unsupported pedal controller CC{0}; expected CC64, CC66, or CC67")]
    UnsupportedPedal(u8),
    #[error("parameter queue for CC{controller} exceeds capacity {VST3_PARAMETER_QUEUE_CAPACITY}")]
    ParameterCapacityExceeded { controller: u8 },
    #[error("invalid process context")]
    InvalidProcessContext,
    #[error("probe repeat count must be in 1..={MAX_PROBE_REPEATS}, got {0}")]
    InvalidRepeatCount(usize),
    #[error("probe repeat {0} produced a non-finite sample")]
    NonFiniteProbeOutput(usize),
    #[error("probe repeat {0} produced only zero samples")]
    SilentProbeOutput(usize),
}

#[derive(Clone, Debug)]
pub struct Vst3ProbeOptions {
    pub bundle: PathBuf,
    pub class_id: Vst3ClassId,
    pub expected_version: Option<String>,
    pub repeats: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Vst3ProbeRun {
    pub repeat: usize,
    pub metadata: Vst3Metadata,
    pub process_count: u64,
    pub finite_output: bool,
    pub nonzero_output: bool,
    pub peak: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Vst3ProbeReport {
    pub repeats: usize,
    pub runs: Vec<Vst3ProbeRun>,
}

#[derive(Clone, Copy)]
struct ParameterPoint {
    sample_offset: i32,
    value: f64,
}

struct EventListState {
    events: [MaybeUninit<Event>; VST3_EVENT_CAPACITY],
    len: usize,
}

struct FixedEventList {
    state: UnsafeCell<EventListState>,
}

// SAFETY: the object is movable between threads, but PreparedVst3's exclusive `&mut self`
// process contract permits access on only one processing thread at a time. A plugin may call this
// interface only synchronously while its ProcessData is valid.
unsafe impl Send for FixedEventList {}
unsafe impl Sync for FixedEventList {}

impl FixedEventList {
    fn new() -> Self {
        Self {
            state: UnsafeCell::new(EventListState {
                events: [MaybeUninit::uninit(); VST3_EVENT_CAPACITY],
                len: 0,
            }),
        }
    }

    fn reset(&self) {
        // SAFETY: caller holds exclusive access to PreparedVst3 and no plugin call is in progress.
        unsafe { (*self.state.get()).len = 0 };
    }

    fn push(&self, event: Event) -> Result<(), ()> {
        // SAFETY: all accesses are serialized by PreparedVst3::process.
        let state = unsafe { &mut *self.state.get() };
        if state.len == state.events.len() {
            return Err(());
        }
        state.events[state.len].write(event);
        state.len += 1;
        Ok(())
    }
}

impl Class for FixedEventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for FixedEventList {
    unsafe fn getEventCount(&self) -> i32 {
        unsafe { (*self.state.get()).len as i32 }
    }

    unsafe fn getEvent(&self, index: i32, event: *mut Event) -> Steinberg::tresult {
        if event.is_null() || index < 0 {
            return kInvalidArgument;
        }
        let state = unsafe { &*self.state.get() };
        let Some(stored) = state
            .events
            .get(index as usize)
            .filter(|_| (index as usize) < state.len)
        else {
            return kInvalidArgument;
        };
        unsafe { event.write(stored.assume_init()) };
        kResultOk
    }

    unsafe fn addEvent(&self, event: *mut Event) -> Steinberg::tresult {
        if event.is_null() {
            return kInvalidArgument;
        }
        match self.push(unsafe { *event }) {
            Ok(()) => kResultOk,
            Err(()) => kResultFalse,
        }
    }
}

struct ParameterQueueState {
    points: [ParameterPoint; VST3_PARAMETER_QUEUE_CAPACITY],
    len: usize,
}

struct FixedParameterQueue {
    parameter_id: ParamID,
    state: UnsafeCell<ParameterQueueState>,
}

// SAFETY: see FixedEventList. Queue access is confined to the synchronous process call.
unsafe impl Send for FixedParameterQueue {}
unsafe impl Sync for FixedParameterQueue {}

impl FixedParameterQueue {
    fn new(parameter_id: ParamID) -> Self {
        Self {
            parameter_id,
            state: UnsafeCell::new(ParameterQueueState {
                points: [ParameterPoint {
                    sample_offset: 0,
                    value: 0.0,
                }; VST3_PARAMETER_QUEUE_CAPACITY],
                len: 0,
            }),
        }
    }

    fn reset(&self) {
        // SAFETY: caller holds exclusive access and no plugin call is in progress.
        unsafe { (*self.state.get()).len = 0 };
    }

    fn push(&self, sample_offset: i32, value: f64) -> Result<i32, ()> {
        // SAFETY: all accesses are serialized by PreparedVst3::process.
        let state = unsafe { &mut *self.state.get() };
        if state.len == state.points.len() {
            return Err(());
        }
        let index = state.len;
        state.points[index] = ParameterPoint {
            sample_offset,
            value,
        };
        state.len += 1;
        Ok(index as i32)
    }
}

impl Class for FixedParameterQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for FixedParameterQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.parameter_id
    }

    unsafe fn getPointCount(&self) -> i32 {
        unsafe { (*self.state.get()).len as i32 }
    }

    unsafe fn getPoint(
        &self,
        index: i32,
        sample_offset: *mut i32,
        value: *mut f64,
    ) -> Steinberg::tresult {
        if index < 0 || sample_offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        let state = unsafe { &*self.state.get() };
        let Some(point) = state
            .points
            .get(index as usize)
            .filter(|_| (index as usize) < state.len)
        else {
            return kInvalidArgument;
        };
        unsafe {
            sample_offset.write(point.sample_offset);
            value.write(point.value);
        }
        kResultOk
    }

    unsafe fn addPoint(
        &self,
        sample_offset: i32,
        value: f64,
        index: *mut i32,
    ) -> Steinberg::tresult {
        if index.is_null() || sample_offset < 0 || !value.is_finite() {
            return kInvalidArgument;
        }
        match self.push(sample_offset, value) {
            Ok(point_index) => {
                unsafe { index.write(point_index) };
                kResultOk
            }
            Err(()) => kResultFalse,
        }
    }
}

struct FixedParameterChanges {
    ids: Vec<ParamID>,
    queues: Vec<*mut IParamValueQueue>,
    objects: Vec<*const FixedParameterQueue>,
}

// SAFETY: queue pointers remain valid for this object's entire lifetime, and all calls occur in the
// single synchronous process invocation guarded by PreparedVst3's exclusive borrow.
unsafe impl Send for FixedParameterChanges {}
unsafe impl Sync for FixedParameterChanges {}

impl Class for FixedParameterChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for FixedParameterChanges {
    unsafe fn getParameterCount(&self) -> i32 {
        self.objects
            .iter()
            .filter(|q| unsafe { (*(**q)).state.get().as_ref().unwrap().len > 0 })
            .count() as i32
    }

    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        if index < 0 {
            return ptr::null_mut();
        }
        self.objects
            .iter()
            .enumerate()
            .filter(|(_, q)| unsafe { (*(**q)).state.get().as_ref().unwrap().len > 0 })
            .nth(index as usize)
            .map(|(i, _)| self.queues[i])
            .unwrap_or(ptr::null_mut())
    }

    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut i32,
    ) -> *mut IParamValueQueue {
        if id.is_null() || index.is_null() {
            return ptr::null_mut();
        }
        let requested = unsafe { *id };
        let Some(queue_index) = self
            .ids
            .iter()
            .position(|candidate| *candidate == requested)
        else {
            return ptr::null_mut();
        };
        unsafe { index.write(queue_index as i32) };
        self.queues[queue_index]
    }
}

#[derive(Default)]
struct HostApplication {
    restart: std::sync::atomic::AtomicU32,
}

impl Class for HostApplication {
    type Interfaces = (IHostApplication, IComponentHandler);
}
impl IComponentHandlerTrait for HostApplication {
    unsafe fn beginEdit(&self, _: ParamID) -> Steinberg::tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, _: ParamID, _: f64) -> Steinberg::tresult {
        kNotImplemented
    }
    unsafe fn endEdit(&self, _: ParamID) -> Steinberg::tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, flags: i32) -> Steinberg::tresult {
        self.restart
            .fetch_or(flags as u32, std::sync::atomic::Ordering::Relaxed);
        kResultOk
    }
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut [u16; 128]) -> Steinberg::tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        let name = unsafe { &mut *name };
        name.fill(0);
        for (source, target) in "Muz".encode_utf16().zip(name.iter_mut()) {
            *target = source;
        }
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        _iid: *mut TUID,
        object: *mut *mut c_void,
    ) -> Steinberg::tresult {
        if object.is_null() {
            return kInvalidArgument;
        }
        unsafe { object.write(ptr::null_mut()) };
        kNoInterface
    }
}

/// A completely prepared instrument instance. It is intentionally not Clone.
///
/// Move it between threads only while it is quiescent. Processing requires `&mut self`, performs no
/// host allocation or locking, and does not make state/lifecycle calls.
pub struct PreparedVst3 {
    library: Option<Library>,
    module_exit: Option<ModuleExit>,
    module_entered: bool,
    factory: Option<ComPtr<IPluginFactory>>,
    host: Option<ComWrapper<HostApplication>>,
    component: Option<ComPtr<IComponent>>,
    processor: Option<ComPtr<IAudioProcessor>>,
    controller: Option<ComPtr<IEditController>>,
    midi_mapping: Option<ComPtr<IMidiMapping>>,
    component_connection: Option<ComPtr<IConnectionPoint>>,
    controller_connection: Option<ComPtr<IConnectionPoint>>,
    component_connected: bool,
    controller_connected: bool,
    component_initialized: bool,
    controller_initialized: bool,
    event_bus_active: bool,
    output_bus_active: bool,
    component_active: bool,
    processing: bool,
    event_list: Option<ComWrapper<FixedEventList>>,
    parameter_queues: Option<Vec<ComWrapper<FixedParameterQueue>>>,
    parameter_changes: Option<ComWrapper<FixedParameterChanges>>,
    pedal_parameter_ids: [ParamID; 3],
    metadata: Option<Vst3Metadata>,
    process_count: u64,
    sample_rate: f64,
    max_frames: usize,
    process_mode: i32,
    input_channels: i32,
    parameters: Vec<PluginParameter>,
    pending_parameters: Vec<Option<f64>>,
    timed_parameters: Vec<(usize, u32, f64)>,
    cc_ids: [[u32; 131]; 16],
}

// SAFETY: a prepared instance is moved only while quiescent. All COM access is serialized through
// `&mut self`, process-time callbacks are synchronous, and destruction runs on the retirement
// owner after the audio graph releases the instance.
unsafe impl Send for PreparedVst3 {}

impl PreparedVst3 {
    pub fn prepare(
        bundle: impl AsRef<Path>,
        class_id: Vst3ClassId,
        expected_version: Option<&str>,
    ) -> Result<Self, Vst3Error> {
        Self::prepare_config(
            bundle,
            class_id,
            expected_version,
            VST3_SAMPLE_RATE,
            256,
            false,
        )
    }
    pub fn prepare_config(
        bundle: impl AsRef<Path>,
        class_id: Vst3ClassId,
        expected_version: Option<&str>,
        sample_rate: f64,
        max_frames: usize,
        offline: bool,
    ) -> Result<Self, Vst3Error> {
        if max_frames == 0 || max_frames > VST3_MAX_FRAMES {
            return Err(Vst3Error::InvalidBlockSize { frames: max_frames });
        }
        let module_path = resolve_linux_vst3_module(bundle.as_ref())?;
        let library = unsafe { Library::open(Some(&module_path), RTLD_NOW | RTLD_LOCAL) }.map_err(
            |source| Vst3Error::LoadModule {
                path: module_path.clone(),
                source,
            },
        )?;

        // SAFETY: symbol names and signatures are the Linux VST3 module ABI. Function pointers are
        // copied while the library is loaded and never called after it is unloaded.
        let (module_entry, module_exit, get_factory) = unsafe {
            let entry = *library
                .get::<ModuleEntry>(b"ModuleEntry\0")
                .map_err(|source| Vst3Error::MissingSymbol {
                    path: module_path.clone(),
                    symbol: "ModuleEntry",
                    source,
                })?;
            let exit = *library
                .get::<ModuleExit>(b"ModuleExit\0")
                .map_err(|source| Vst3Error::MissingSymbol {
                    path: module_path.clone(),
                    symbol: "ModuleExit",
                    source,
                })?;
            let factory = *library
                .get::<GetPluginFactory>(b"GetPluginFactory\0")
                .map_err(|source| Vst3Error::MissingSymbol {
                    path: module_path.clone(),
                    symbol: "GetPluginFactory",
                    source,
                })?;
            (entry, exit, factory)
        };

        // libloading exposes the dlopen handle only by transferring ownership. Reconstituting the
        // Library immediately keeps exactly one dlclose owner while passing the actual handle to
        // ModuleEntry as required by the Linux VST3 ABI.
        let raw_handle = library.into_raw();
        let entered = unsafe { module_entry(raw_handle) };
        let library = unsafe { Library::from_raw(raw_handle) };
        if !entered {
            return Err(Vst3Error::ModuleEntryFailed);
        }

        let mut prepared = Self {
            library: Some(library),
            module_exit: Some(module_exit),
            module_entered: true,
            factory: None,
            host: Some(ComWrapper::new(HostApplication::default())),
            component: None,
            processor: None,
            controller: None,
            midi_mapping: None,
            component_connection: None,
            controller_connection: None,
            component_connected: false,
            controller_connected: false,
            component_initialized: false,
            controller_initialized: false,
            event_bus_active: false,
            output_bus_active: false,
            component_active: false,
            processing: false,
            event_list: None,
            parameter_queues: None,
            parameter_changes: None,
            pedal_parameter_ids: [0; 3],
            metadata: None,
            process_count: 0,
            sample_rate,
            max_frames,
            process_mode: if offline { kOffline } else { kRealtime } as i32,
            input_channels: 0,
            parameters: Vec::new(),
            pending_parameters: Vec::new(),
            timed_parameters: Vec::with_capacity(16384),
            cc_ids: [[u32::MAX; 131]; 16],
        };

        let factory_ptr = unsafe { get_factory() };
        prepared.factory =
            Some(unsafe { ComPtr::from_raw(factory_ptr) }.ok_or(Vst3Error::NullFactory)?);

        let host_context = prepared.host_context();
        if let Some(factory3) = prepared.factory().cast::<IPluginFactory3>() {
            let result = unsafe { factory3.setHostContext(host_context) };
            // A factory may expose IPluginFactory3 without consuming the optional
            // host context. sfizz returns kNotImplemented and can still create
            // and initialize its audio class.
            if result != kNotImplemented {
                exact("IPluginFactory3::setHostContext", result)?;
            }
        }

        let (class_index, class_info, factory_vendor) = prepared.find_class(class_id)?;
        if class_info.category != AUDIO_MODULE_CATEGORY {
            return Err(Vst3Error::NotAudioModule {
                class_id,
                category: class_info.category,
            });
        }
        if let Some(expected) = expected_version
            && class_info.version != expected
        {
            return Err(Vst3Error::VersionMismatch {
                expected: expected.to_owned(),
                actual: class_info.version,
            });
        }

        let class_id = prepared.actual_class_id(class_index)?;
        let component = unsafe {
            create_instance::<IComponent>(prepared.factory(), class_id.as_tuid(), "IComponent")?
        };
        exact("IComponent::initialize", unsafe {
            component.initialize(host_context)
        })?;
        prepared.component = Some(component);
        prepared.component_initialized = true;
        prepared.processor = Some(
            prepared
                .component()
                .cast::<IAudioProcessor>()
                .ok_or(Vst3Error::MissingInterface("IAudioProcessor"))?,
        );

        if let Some(controller) = prepared.component().cast::<IEditController>() {
            prepared.controller = Some(controller);
        } else {
            let mut controller_cid = [0; 16];
            exact("IComponent::getControllerClassId", unsafe {
                prepared
                    .component()
                    .getControllerClassId(&mut controller_cid)
            })?;
            let controller = unsafe {
                create_instance::<IEditController>(
                    prepared.factory(),
                    controller_cid,
                    "IEditController",
                )?
            };
            exact("IEditController::initialize", unsafe {
                controller.initialize(host_context)
            })?;
            prepared.controller_initialized = true;
            prepared.controller = Some(controller);
        }

        exact("setComponentHandler", unsafe {
            prepared.controller().setComponentHandler(
                prepared
                    .host
                    .as_ref()
                    .unwrap()
                    .as_com_ref::<IComponentHandler>()
                    .unwrap()
                    .as_ptr(),
            )
        })?;
        prepared.connect_component_and_controller()?;
        prepared.pedal_parameter_ids = prepared.query_pedal_mappings()?;
        prepared.create_process_objects()?;

        let (event_input_channels, output_channels) = prepared.configure_buses_and_processing()?;
        let latency_samples = unsafe { prepared.processor().getLatencySamples() };
        let tail_samples = unsafe { prepared.processor().getTailSamples() };
        prepared.metadata = Some(Vst3Metadata {
            module: module_path,
            class_id: prepared.actual_class_id(class_index)?,
            class_name: class_info.name,
            category: class_info.category,
            subcategories: class_info.subcategories,
            vendor: class_info.vendor,
            version: class_info.version,
            sdk_version: class_info.sdk_version,
            factory_vendor,
            latency_samples,
            tail_samples,
            event_input_channels,
            output_channels,
            pedal_parameter_ids: prepared.pedal_parameter_ids,
        });

        // Retain class_index as a checked fact: find_class has successfully obtained metadata for
        // this exact factory slot. It is intentionally not part of the runtime state.
        let _ = class_index;
        Ok(prepared)
    }

    pub fn save_state(&self, path: &Path) -> anyhow::Result<()> {
        use super::vst3_state::StateStream;
        let stream = ComWrapper::new(StateStream::new(Vec::new()));
        exact("getState", unsafe {
            self.component()
                .getState(stream.as_com_ref::<Steinberg::IBStream>().unwrap().as_ptr())
        })?;
        std::fs::write(path, stream.0.borrow().get_ref())?;
        Ok(())
    }
    pub fn load_state(&mut self, path: &Path) -> anyhow::Result<()> {
        use super::vst3_state::StateStream;
        let mut bytes = crate::assets::read_bounded(path, 64 * 1024 * 1024)?;
        anyhow::ensure!(
            bytes.len() <= 64 * 1024 * 1024,
            "plugin state exceeds 64 MiB"
        );
        if bytes.starts_with(b"VST3") {
            anyhow::ensure!(bytes.len() >= 48, "truncated VST3 preset");
            let list = u64::from_le_bytes(bytes[40..48].try_into()?) as usize;
            anyhow::ensure!(
                list <= bytes.len().saturating_sub(8) && &bytes[list..list + 4] == b"List",
                "invalid VST3 preset chunk list"
            );
            let count = u32::from_le_bytes(bytes[list + 4..list + 8].try_into()?) as usize;
            let mut component = None;
            for i in 0..count.min(1024) {
                let at = list + 8 + i * 20;
                anyhow::ensure!(at + 20 <= bytes.len(), "truncated preset chunk");
                if &bytes[at..at + 4] == b"Comp" {
                    let off = u64::from_le_bytes(bytes[at + 4..at + 12].try_into()?) as usize;
                    let len = u64::from_le_bytes(bytes[at + 12..at + 20].try_into()?) as usize;
                    component = Some(
                        bytes
                            .get(
                                off..off
                                    .checked_add(len)
                                    .ok_or_else(|| anyhow::anyhow!("invalid preset extent"))?,
                            )
                            .ok_or_else(|| anyhow::anyhow!("invalid preset extent"))?
                            .to_vec(),
                    );
                }
            }
            bytes = component.ok_or_else(|| anyhow::anyhow!("preset has no component state"))?;
        }
        processing_call("setProcessing(false)", unsafe {
            self.processor().setProcessing(0)
        })?;
        self.processing = false;
        exact("setActive(false)", unsafe { self.component().setActive(0) })?;
        self.component_active = false;
        let stream = ComWrapper::new(StateStream::new(bytes));
        let ptr = stream.as_com_ref::<Steinberg::IBStream>().unwrap().as_ptr();
        exact("setState", unsafe { self.component().setState(ptr) })?;
        stream.0.borrow_mut().set_position(0);
        let result = unsafe { self.controller().setComponentState(ptr) };
        if result != kNotImplemented && result != kResultFalse {
            exact("setComponentState", result)?;
        }
        exact("setActive(true)", unsafe { self.component().setActive(1) })?;
        self.component_active = true;
        processing_call("setProcessing(true)", unsafe {
            self.processor().setProcessing(1)
        })?;
        self.processing = true;
        let latency = unsafe { self.processor().getLatencySamples() };
        let tail = unsafe { self.processor().getTailSamples() };
        if let Some(metadata) = &mut self.metadata {
            metadata.latency_samples = latency;
            metadata.tail_samples = tail;
        }
        Ok(())
    }
    pub fn metadata(&self) -> &Vst3Metadata {
        self.metadata
            .as_ref()
            .expect("metadata exists for every successfully prepared VST3")
    }

    pub fn process_count(&self) -> u64 {
        self.process_count
    }

    /// Process one planar stereo block without host allocation or locking.
    pub fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        events: &[Vst3Event],
        context: Vst3TimeContext,
    ) -> Result<Vst3ProcessReport, Vst3Error> {
        if left.len() != right.len() {
            return Err(Vst3Error::MismatchedOutputBuffers);
        }
        let frames = left.len();
        if frames == 0 || frames > self.max_frames {
            return Err(Vst3Error::InvalidBlockSize { frames });
        }
        // Note and parameter capacities are independent fixed storages. Validate each while
        // converting so a flush may expand one engine event into note and pedal releases.
        if !valid_context(context) {
            return Err(Vst3Error::InvalidProcessContext);
        }

        self.event_list().reset();
        for queue in self.parameter_queues().iter() {
            queue.reset();
        }

        for (i, pending) in self.pending_parameters.iter_mut().enumerate() {
            if let Some(value) = pending.take() {
                self.parameter_queues.as_ref().unwrap()[i]
                    .push(0, value)
                    .map_err(|_| Vst3Error::ParameterCapacityExceeded { controller: 0 })?;
            }
        }
        for &(index, offset, value) in &self.timed_parameters {
            if offset as usize >= frames {
                return Err(Vst3Error::EventOutsideBlock {
                    offset: offset as usize,
                    frames,
                });
            }
            self.parameter_queues()[index]
                .push(offset as i32, value)
                .map_err(|_| Vst3Error::ParameterCapacityExceeded { controller: 0 })?;
        }
        self.timed_parameters.clear();
        let mut previous_offset = 0;
        for (index, event) in events.iter().copied().enumerate() {
            let offset = event.sample_offset();
            if offset >= frames {
                return Err(Vst3Error::EventOutsideBlock { offset, frames });
            }
            if index != 0 && offset < previous_offset {
                return Err(Vst3Error::EventsOutOfOrder);
            }
            previous_offset = offset;
            self.queue_event(event)?;
        }

        if self.input_channels == 0 {
            left.fill(0.0);
            right.fill(0.0);
        }
        let mut input_buffers = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut input_bus = AudioBusBuffers {
            numChannels: self.input_channels,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: input_buffers.as_mut_ptr(),
            },
        };
        let mut channel_buffers = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut output_bus = AudioBusBuffers {
            numChannels: 2,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: channel_buffers.as_mut_ptr(),
            },
        };
        let mut process_context = raw_process_context(context);
        process_context.sampleRate = self.sample_rate;
        let mut data = ProcessData {
            processMode: self.process_mode,
            symbolicSampleSize: kSample32 as i32,
            numSamples: frames as i32,
            numInputs: if self.input_channels > 0 { 1 } else { 0 },
            numOutputs: 1,
            inputs: if self.input_channels > 0 {
                &mut input_bus
            } else {
                ptr::null_mut()
            },
            outputs: &mut output_bus,
            inputParameterChanges: self.parameter_changes_ptr(),
            outputParameterChanges: ptr::null_mut(),
            inputEvents: self.event_list_ptr(),
            outputEvents: ptr::null_mut(),
            processContext: &mut process_context,
        };

        exact("IAudioProcessor::process", unsafe {
            self.processor().process(&mut data)
        })?;
        self.process_count = self.process_count.saturating_add(1);
        Ok(Vst3ProcessReport {
            process_count: self.process_count,
            output_silence_flags: output_bus.silenceFlags,
        })
    }

    fn factory(&self) -> &ComPtr<IPluginFactory> {
        self.factory.as_ref().expect("factory is live")
    }

    fn component(&self) -> &ComPtr<IComponent> {
        self.component.as_ref().expect("component is live")
    }

    fn processor(&self) -> &ComPtr<IAudioProcessor> {
        self.processor.as_ref().expect("processor is live")
    }

    fn controller(&self) -> &ComPtr<IEditController> {
        self.controller.as_ref().expect("controller is live")
    }

    fn host_context(&self) -> *mut FUnknown {
        self.host
            .as_ref()
            .expect("host is live")
            .as_com_ref::<IHostApplication>()
            .expect("host implements IHostApplication")
            .upcast::<FUnknown>()
            .as_ptr()
    }

    fn event_list(&self) -> &FixedEventList {
        self.event_list.as_ref().expect("event list is live")
    }

    fn event_list_ptr(&self) -> *mut IEventList {
        self.event_list
            .as_ref()
            .expect("event list is live")
            .as_com_ref::<IEventList>()
            .expect("event list implements IEventList")
            .as_ptr()
    }

    fn parameter_queues(&self) -> &[ComWrapper<FixedParameterQueue>] {
        self.parameter_queues
            .as_ref()
            .expect("parameter queues are live")
    }

    fn parameter_changes_ptr(&self) -> *mut IParameterChanges {
        self.parameter_changes
            .as_ref()
            .expect("parameter changes are live")
            .as_com_ref::<IParameterChanges>()
            .expect("parameter changes implements IParameterChanges")
            .as_ptr()
    }

    fn find_class(&self, class_id: Vst3ClassId) -> Result<(i32, ClassMetadata, String), Vst3Error> {
        let mut factory_info: PFactoryInfo = zeroed_ffi();
        exact("IPluginFactory::getFactoryInfo", unsafe {
            self.factory().getFactoryInfo(&mut factory_info)
        })?;
        let factory_vendor = c_string(&factory_info.vendor);

        let factory2 = self
            .factory()
            .cast::<IPluginFactory2>()
            .ok_or(Vst3Error::MissingFactory2)?;
        let count = unsafe { self.factory().countClasses() };
        if count < 0 {
            return Err(Vst3Error::CallFailed {
                operation: "IPluginFactory::countClasses",
                result: count,
            });
        }
        for index in 0..count {
            let mut base: PClassInfo = zeroed_ffi();
            exact("IPluginFactory::getClassInfo", unsafe {
                self.factory().getClassInfo(index, &mut base)
            })?;
            if (class_id.0 != [0; 16] && Vst3ClassId::from_tuid(base.cid) != class_id)
                || c_string(&base.category) != AUDIO_MODULE_CATEGORY
            {
                continue;
            }
            let mut info: PClassInfo2 = zeroed_ffi();
            exact("IPluginFactory2::getClassInfo2", unsafe {
                factory2.getClassInfo2(index, &mut info)
            })?;
            if class_id.0 != [0; 16] && Vst3ClassId::from_tuid(info.cid) != class_id {
                return Err(Vst3Error::CallFailed {
                    operation: "IPluginFactory2::getClassInfo2 returned a different CID",
                    result: kResultFalse,
                });
            }
            return Ok((
                index,
                ClassMetadata {
                    name: c_string(&info.name),
                    category: c_string(&info.category),
                    subcategories: c_string(&info.subCategories),
                    vendor: c_string(&info.vendor),
                    version: c_string(&info.version),
                    sdk_version: c_string(&info.sdkVersion),
                },
                factory_vendor,
            ));
        }
        Err(Vst3Error::ClassNotFound(class_id))
    }

    fn connect_component_and_controller(&mut self) -> Result<(), Vst3Error> {
        self.component_connection = self.component().cast::<IConnectionPoint>();
        self.controller_connection = self.controller().cast::<IConnectionPoint>();
        let (Some(component), Some(controller)) = (
            self.component_connection.as_ref(),
            self.controller_connection.as_ref(),
        ) else {
            return Ok(());
        };
        exact("component IConnectionPoint::connect", unsafe {
            component.connect(controller.as_ptr())
        })?;
        self.component_connected = true;
        exact("controller IConnectionPoint::connect", unsafe {
            controller.connect(component.as_ptr())
        })?;
        self.controller_connected = true;
        Ok(())
    }

    fn query_pedal_mappings(&mut self) -> Result<[ParamID; 3], Vst3Error> {
        if let Some(mapping) = self.controller().cast::<IMidiMapping>() {
            for channel in 0..16 {
                for cc in 0..131 {
                    let mut id = u32::MAX;
                    if unsafe {
                        mapping.getMidiControllerAssignment(0, channel as i16, cc as i16, &mut id)
                    } == kResultOk
                    {
                        self.cc_ids[channel][cc] = id;
                    }
                }
            }
            self.midi_mapping = Some(mapping);
        }
        Ok(PEDAL_CONTROLLERS.map(|cc| self.cc_ids[0][cc as usize]))
    }
    fn actual_class_id(&self, index: i32) -> Result<Vst3ClassId, Vst3Error> {
        let mut info: PClassInfo = zeroed_ffi();
        exact("getClassInfo", unsafe {
            self.factory().getClassInfo(index, &mut info)
        })?;
        Ok(Vst3ClassId::from_tuid(info.cid))
    }
    pub fn parameters(&self) -> &[PluginParameter] {
        &self.parameters
    }
    /// Effective normalized values after state load and queued overrides have been applied.
    /// Call on the coordinator thread, never from the audio callback.
    pub fn parameter_values(&self) -> anyhow::Result<Vec<(u32, f64)>> {
        self.parameters
            .iter()
            .map(|parameter| {
                let value = unsafe { self.controller().getParamNormalized(parameter.id) };
                anyhow::ensure!(
                    value.is_finite() && (0.0..=1.0).contains(&value),
                    "VST3 parameter {} has invalid normalized value",
                    parameter.id
                );
                Ok((parameter.id, value))
            })
            .collect()
    }
    pub fn plain_to_normalized(&self, name: &str, value: f64) -> Result<f64, Vst3Error> {
        let p = self
            .parameters
            .iter()
            .find(|p| name.parse::<u32>().ok() == Some(p.id) || name == p.name || name == p.key)
            .ok_or(Vst3Error::UnknownParameter)?;
        let value = unsafe { self.controller().plainParamToNormalized(p.id, value) };
        if value.is_finite() && (0.0..=1.).contains(&value) {
            Ok(value)
        } else {
            Err(Vst3Error::UnknownParameter)
        }
    }
    pub fn restart_flags(&self) -> u32 {
        self.host
            .as_ref()
            .unwrap()
            .restart
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn finish_preparation(&mut self) -> Result<(), Vst3Error> {
        // Set both halves while quiescent. Latency-affecting controls settle before PDC is built.
        for (p, v) in self.parameters.iter().zip(&self.pending_parameters) {
            if let Some(v) = v {
                unsafe {
                    self.controller().setParamNormalized(p.id, *v);
                }
            }
        }
        let mut left = vec![0.; self.max_frames];
        let mut right = vec![0.; self.max_frames];
        self.process(&mut left, &mut right, &[], Vst3TimeContext::default())?;
        let latency = unsafe { self.processor().getLatencySamples() };
        let tail = unsafe { self.processor().getTailSamples() };
        if let Some(m) = &mut self.metadata {
            m.latency_samples = latency;
            m.tail_samples = tail;
        }
        self.host
            .as_ref()
            .unwrap()
            .restart
            .store(0, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    pub fn set_parameter(&mut self, name: &str, value: f64) -> Result<(), Vst3Error> {
        let index = self
            .parameters
            .iter()
            .position(|p| name.parse::<u32>().ok() == Some(p.id) || name == p.name || name == p.key)
            .ok_or(Vst3Error::UnknownParameter)?;
        if !value.is_finite()
            || !(0.0..=1.0).contains(&value)
            || self.parameters[index].flags & 2 != 0
        {
            return Err(Vst3Error::UnknownParameter);
        }
        self.pending_parameters[index] = Some(value);
        Ok(())
    }
    pub fn set_parameter_at(
        &mut self,
        name: &str,
        value: f64,
        offset: u32,
    ) -> Result<(), Vst3Error> {
        self.set_parameter(name, value)?;
        let index = self
            .parameters
            .iter()
            .position(|p| name.parse::<u32>().ok() == Some(p.id) || name == p.name || name == p.key)
            .ok_or(Vst3Error::UnknownParameter)?;
        self.pending_parameters[index] = None;
        if self.timed_parameters.len() == self.timed_parameters.capacity() {
            return Err(Vst3Error::ParameterCapacityExceeded { controller: 0 });
        }
        self.timed_parameters.push((index, offset, value));
        Ok(())
    }
    fn create_process_objects(&mut self) -> Result<(), Vst3Error> {
        let count = unsafe { self.controller().getParameterCount() };
        if !(0..=16384).contains(&count) {
            return Err(Vst3Error::UnknownParameter);
        }
        let mut ids = Vec::new();
        for index in 0..count {
            let mut p: ParameterInfo = zeroed_ffi();
            exact("getParameterInfo", unsafe {
                self.controller().getParameterInfo(index, &mut p)
            })?;
            ids.push(p.id);
            let name = utf16(&p.title);
            self.parameters.push(PluginParameter {
                id: p.id,
                key: parameter_key(&name),
                name,
                units: utf16(&p.units),
                default: p.defaultNormalizedValue,
                steps: p.stepCount,
                flags: p.flags,
            });
        }
        // MIDI mappings may reference hidden parameters omitted from enumeration.
        for id in self
            .cc_ids
            .iter()
            .flatten()
            .copied()
            .filter(|id| *id != u32::MAX)
        {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let queues: Vec<_> = ids
            .iter()
            .map(|id| ComWrapper::new(FixedParameterQueue::new(*id)))
            .collect();
        let pointers = queues
            .iter()
            .map(|q| q.as_com_ref::<IParamValueQueue>().unwrap().as_ptr())
            .collect();
        let objects = queues
            .iter()
            .map(|q| &**q as *const FixedParameterQueue)
            .collect();
        self.pending_parameters = vec![None; self.parameters.len()];
        self.parameter_changes = Some(ComWrapper::new(FixedParameterChanges {
            ids,
            queues: pointers,
            objects,
        }));
        self.parameter_queues = Some(queues);
        self.event_list = Some(ComWrapper::new(FixedEventList::new()));
        Ok(())
    }

    fn configure_buses_and_processing(&mut self) -> Result<(i32, i32), Vst3Error> {
        let io_mode_result = unsafe { self.component().setIoMode(kSimple as i32) };
        if io_mode_result != kResultOk && io_mode_result != kNotImplemented {
            return Err(Vst3Error::CallFailed {
                operation: "IComponent::setIoMode",
                result: io_mode_result,
            });
        }

        let event_count = unsafe { self.component().getBusCount(kEvent as i32, kInput as i32) };

        let mut event_info: BusInfo = zeroed_ffi();
        if event_count > 0 {
            exact("IComponent::getBusInfo(event input)", unsafe {
                self.component()
                    .getBusInfo(kEvent as i32, kInput as i32, 0, &mut event_info)
            })?;
            if event_info.channelCount <= 0 {
                return Err(Vst3Error::InvalidEventChannels(event_info.channelCount));
            }
        }

        let output_count = unsafe { self.component().getBusCount(kAudio as i32, kOutput as i32) };
        if output_count <= 0 {
            return Err(Vst3Error::MissingAudioOutput);
        }
        let mut output_info: BusInfo = zeroed_ffi();
        exact("IComponent::getBusInfo(audio output)", unsafe {
            self.component()
                .getBusInfo(kAudio as i32, kOutput as i32, 0, &mut output_info)
        })?;
        if output_info.channelCount != 2 {
            return Err(Vst3Error::NonStereoOutput(output_info.channelCount));
        }

        let input_count = unsafe { self.component().getBusCount(kAudio as i32, kInput as i32) };
        self.input_channels = if input_count > 0 { 2 } else { 0 };
        let mut input_arrangement = SpeakerArr::kStereo;
        let mut output_arrangement = SpeakerArr::kStereo;
        let arrangement_result = unsafe {
            self.processor().setBusArrangements(
                if input_count > 0 {
                    &mut input_arrangement
                } else {
                    ptr::null_mut()
                },
                if input_count > 0 { 1 } else { 0 },
                &mut output_arrangement,
                1,
            )
        };
        if arrangement_result != kResultOk && arrangement_result != kResultFalse {
            return Err(Vst3Error::CallFailed {
                operation: "IAudioProcessor::setBusArrangements",
                result: arrangement_result,
            });
        }
        let mut actual_output_arrangement = 0;
        exact("IAudioProcessor::getBusArrangement", unsafe {
            self.processor()
                .getBusArrangement(kOutput as i32, 0, &mut actual_output_arrangement)
        })?;
        if actual_output_arrangement != SpeakerArr::kStereo {
            return Err(Vst3Error::CallFailed {
                operation: "IAudioProcessor stereo output arrangement",
                result: kResultFalse,
            });
        }
        if event_count > 0 {
            exact("IComponent::activateBus(event input)", unsafe {
                self.component()
                    .activateBus(kEvent as i32, kInput as i32, 0, 1)
            })?;
            self.event_bus_active = true;
        }
        exact("IComponent::activateBus(audio output)", unsafe {
            self.component()
                .activateBus(kAudio as i32, kOutput as i32, 0, 1)
        })?;
        self.output_bus_active = true;
        if input_count > 0 {
            exact("activate audio input", unsafe {
                self.component()
                    .activateBus(kAudio as i32, kInput as i32, 0, 1)
            })?;
        }
        for i in 1..input_count {
            unsafe {
                self.component()
                    .activateBus(kAudio as i32, kInput as i32, i, 0);
            }
        }
        for i in 1..output_count {
            unsafe {
                self.component()
                    .activateBus(kAudio as i32, kOutput as i32, i, 0);
            }
        }

        exact("IAudioProcessor::canProcessSampleSize(f32)", unsafe {
            self.processor().canProcessSampleSize(kSample32 as i32)
        })?;
        let mut setup = ProcessSetup {
            processMode: self.process_mode,
            symbolicSampleSize: kSample32 as i32,
            maxSamplesPerBlock: self.max_frames as i32,
            sampleRate: self.sample_rate,
        };
        exact("IAudioProcessor::setupProcessing", unsafe {
            self.processor().setupProcessing(&mut setup)
        })?;
        exact("IComponent::setActive(true)", unsafe {
            self.component().setActive(1)
        })?;
        self.component_active = true;
        processing_call("IAudioProcessor::setProcessing(true)", unsafe {
            self.processor().setProcessing(1)
        })?;
        self.processing = true;

        Ok((event_info.channelCount, output_info.channelCount))
    }

    fn queue_event(&self, event: Vst3Event) -> Result<(), Vst3Error> {
        match event {
            Vst3Event::Midi {
                sample_offset,
                bytes,
                len: _,
            } => {
                let channel = (bytes[0] & 15) as usize;
                if bytes[0] >> 4 == 10 {
                    return self
                        .event_list()
                        .push(Event {
                            busIndex: 0,
                            sampleOffset: sample_offset as i32,
                            ppqPosition: 0.,
                            flags: 0,
                            r#type: kPolyPressureEvent as u16,
                            __field0: Event__type0 {
                                polyPressure: PolyPressureEvent {
                                    channel: channel as i16,
                                    pitch: bytes[1] as i16,
                                    pressure: bytes[2] as f32 / 127.,
                                    noteId: -1,
                                },
                            },
                        })
                        .map_err(|_| Vst3Error::EventCapacityExceeded {
                            count: VST3_EVENT_CAPACITY + 1,
                        });
                }
                let (cc, value) = match bytes[0] >> 4 {
                    11 => (bytes[1] as usize, bytes[2] as f64 / 127.),
                    12 => (130, bytes[1] as f64 / 127.),
                    13 => (128, bytes[1] as f64 / 127.),
                    14 => (
                        129,
                        ((bytes[2] as u16) << 7 | bytes[1] as u16) as f64 / 16383.,
                    ),
                    _ => return Err(Vst3Error::UnknownParameter),
                };
                let id = self.cc_ids[channel][cc];
                let index = self
                    .parameter_changes
                    .as_ref()
                    .unwrap()
                    .ids
                    .iter()
                    .position(|p| *p == id);
                if let Some(index) = index {
                    self.parameter_queues()[index]
                        .push(sample_offset as i32, value)
                        .map_err(|_| Vst3Error::ParameterCapacityExceeded {
                            controller: cc as u8,
                        })?;
                } else if !matches!(cc, 0 | 32 | 120 | 121 | 123) {
                    return Err(Vst3Error::MissingMidiMapping(cc as u8));
                }
                Ok(())
            }
            Vst3Event::NoteOn {
                tuning,
                sample_offset,
                channel,
                pitch,
                velocity,
                note_id,
            } => {
                validate_note(channel, velocity)?;
                let event = Event {
                    busIndex: 0,
                    sampleOffset: sample_offset as i32,
                    ppqPosition: 0.0,
                    flags: 0,
                    r#type: kNoteOnEvent as u16,
                    __field0: Event__type0 {
                        noteOn: NoteOnEvent {
                            channel: channel as i16,
                            pitch: pitch as i16,
                            tuning,
                            velocity,
                            length: 0,
                            noteId: note_id,
                        },
                    },
                };
                self.event_list()
                    .push(event)
                    .map_err(|_| Vst3Error::EventCapacityExceeded {
                        count: VST3_EVENT_CAPACITY + 1,
                    })
            }
            Vst3Event::NoteOff {
                sample_offset,
                channel,
                pitch,
                velocity,
                note_id,
            } => {
                validate_note(channel, velocity)?;
                let event = Event {
                    busIndex: 0,
                    sampleOffset: sample_offset as i32,
                    ppqPosition: 0.0,
                    flags: 0,
                    r#type: kNoteOffEvent as u16,
                    __field0: Event__type0 {
                        noteOff: NoteOffEvent {
                            channel: channel as i16,
                            pitch: pitch as i16,
                            velocity,
                            noteId: note_id,
                            tuning: 0.0,
                        },
                    },
                };
                self.event_list()
                    .push(event)
                    .map_err(|_| Vst3Error::EventCapacityExceeded {
                        count: VST3_EVENT_CAPACITY + 1,
                    })
            }
            Vst3Event::Pedal {
                sample_offset,
                controller,
                value,
            } => {
                let id = self.cc_ids[0][controller as usize];
                let Some(queue_index) = self
                    .parameter_changes
                    .as_ref()
                    .unwrap()
                    .ids
                    .iter()
                    .position(|p| *p == id)
                else {
                    return Ok(());
                };
                self.parameter_queues()[queue_index]
                    .push(sample_offset as i32, f64::from(value) / 127.0)
                    .map(|_| ())
                    .map_err(|_| Vst3Error::ParameterCapacityExceeded { controller })
            }
        }
    }
}

impl Drop for PreparedVst3 {
    fn drop(&mut self) {
        // No lifecycle method is called from process. Drop is required to run only after the audio
        // graph has returned this instance to its coordinator/retirement owner.
        unsafe {
            if self.processing {
                if let Some(processor) = self.processor.as_ref() {
                    let _ = processor.setProcessing(0);
                }
                self.processing = false;
            }
            if self.component_active {
                if let Some(component) = self.component.as_ref() {
                    let _ = component.setActive(0);
                }
                self.component_active = false;
            }
            if self.output_bus_active {
                if let Some(component) = self.component.as_ref() {
                    let _ = component.activateBus(kAudio as i32, kOutput as i32, 0, 0);
                }
                self.output_bus_active = false;
            }
            if self.event_bus_active {
                if let Some(component) = self.component.as_ref() {
                    let _ = component.activateBus(kEvent as i32, kInput as i32, 0, 0);
                }
                self.event_bus_active = false;
            }
            if self.controller_connected {
                if let (Some(controller), Some(component)) = (
                    self.controller_connection.as_ref(),
                    self.component_connection.as_ref(),
                ) {
                    let _ = controller.disconnect(component.as_ptr());
                }
                self.controller_connected = false;
            }
            if self.component_connected {
                if let (Some(component), Some(controller)) = (
                    self.component_connection.as_ref(),
                    self.controller_connection.as_ref(),
                ) {
                    let _ = component.disconnect(controller.as_ptr());
                }
                self.component_connected = false;
            }
            if self.controller_initialized {
                if let Some(controller) = self.controller.as_ref() {
                    let _ = controller.terminate();
                }
                self.controller_initialized = false;
            }
            if self.component_initialized {
                if let Some(component) = self.component.as_ref() {
                    let _ = component.terminate();
                }
                self.component_initialized = false;
            }
        }

        // Release every module and host COM reference before ModuleExit. ParameterChanges contains
        // borrowed pointers to queues, so it must be dropped before those queues.
        self.parameter_changes.take();
        self.parameter_queues.take();
        self.event_list.take();
        self.midi_mapping.take();
        self.controller_connection.take();
        self.component_connection.take();
        self.processor.take();
        self.controller.take();
        self.component.take();
        self.factory.take();
        self.host.take();

        if self.module_entered {
            if let Some(module_exit) = self.module_exit.take() {
                unsafe {
                    let _ = module_exit();
                }
            }
            self.module_entered = false;
        }
        // Library is last: dropping it invokes dlclose only after ModuleExit has completed.
        self.library.take();
    }
}

struct ClassMetadata {
    name: String,
    category: String,
    subcategories: String,
    vendor: String,
    version: String,
    sdk_version: String,
}

pub fn resolve_linux_vst3_module(bundle: &Path) -> Result<PathBuf, Vst3Error> {
    if bundle.is_file() {
        return Ok(bundle.to_owned());
    }
    if !bundle.is_dir() {
        return Err(Vst3Error::BundleNotFound(bundle.to_owned()));
    }
    let stem = bundle
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Vst3Error::InvalidBundle(bundle.to_owned()))?;
    let module = bundle
        .join("Contents")
        .join("x86_64-linux")
        .join(format!("{stem}.so"));
    if !module.is_file() {
        return Err(Vst3Error::ModuleNotFound(module));
    }
    Ok(module)
}

pub fn probe_vst3(options: &Vst3ProbeOptions) -> Result<Vst3ProbeReport, Vst3Error> {
    if options.repeats == 0 || options.repeats > MAX_PROBE_REPEATS {
        return Err(Vst3Error::InvalidRepeatCount(options.repeats));
    }
    let mut runs = Vec::with_capacity(options.repeats);
    for repeat_index in 0..options.repeats {
        let mut instance = PreparedVst3::prepare(
            &options.bundle,
            options.class_id,
            options.expected_version.as_deref(),
        )?;
        let mut peak = 0.0_f32;
        let mut finite = true;
        let mut nonzero = false;
        for block in 0..8 {
            let mut left = [0.0_f32; 256];
            let mut right = [0.0_f32; 256];
            let note_id = 1;
            let first_events = [
                Vst3Event::NoteOn {
                    tuning: 0.,
                    sample_offset: 0,
                    channel: 0,
                    pitch: 60,
                    velocity: 0.8,
                    note_id,
                },
                Vst3Event::Pedal {
                    sample_offset: 0,
                    controller: 64,
                    value: 96,
                },
                Vst3Event::Pedal {
                    sample_offset: 0,
                    controller: 66,
                    value: 32,
                },
                Vst3Event::Pedal {
                    sample_offset: 0,
                    controller: 67,
                    value: 16,
                },
            ];
            let release_events = [
                Vst3Event::NoteOff {
                    sample_offset: 0,
                    channel: 0,
                    pitch: 60,
                    velocity: 0.4,
                    note_id,
                },
                Vst3Event::Pedal {
                    sample_offset: 1,
                    controller: 64,
                    value: 0,
                },
                Vst3Event::Pedal {
                    sample_offset: 1,
                    controller: 66,
                    value: 0,
                },
                Vst3Event::Pedal {
                    sample_offset: 1,
                    controller: 67,
                    value: 0,
                },
            ];
            let events: &[Vst3Event] = match block {
                0 => &first_events,
                4 => &release_events,
                _ => &[],
            };
            instance.process(
                &mut left,
                &mut right,
                events,
                Vst3TimeContext {
                    project_time_samples: (block * 256) as i64,
                    project_time_music: block as f64 * 256_f64 * 120.0 / (60.0 * VST3_SAMPLE_RATE),
                    bar_position_music: 0.0,
                    ..Vst3TimeContext::default()
                },
            )?;
            for sample in left.into_iter().chain(right) {
                finite &= sample.is_finite();
                nonzero |= sample != 0.0;
                if sample.is_finite() {
                    peak = peak.max(sample.abs());
                }
            }
        }
        let repeat = repeat_index + 1;
        if !finite {
            return Err(Vst3Error::NonFiniteProbeOutput(repeat));
        }
        if !nonzero {
            return Err(Vst3Error::SilentProbeOutput(repeat));
        }
        runs.push(Vst3ProbeRun {
            repeat,
            metadata: instance.metadata().clone(),
            process_count: instance.process_count(),
            finite_output: finite,
            nonzero_output: nonzero,
            peak,
        });
        // instance drops here, proving that every repeat executes complete reverse teardown before
        // the next dlopen/ModuleEntry cycle.
    }
    Ok(Vst3ProbeReport {
        repeats: options.repeats,
        runs,
    })
}

unsafe fn create_instance<I: Interface>(
    factory: &ComPtr<IPluginFactory>,
    cid: TUID,
    interface_name: &'static str,
) -> Result<ComPtr<I>, Vst3Error> {
    let mut object = ptr::null_mut();
    let result = unsafe {
        factory.createInstance(cid.as_ptr(), I::IID.as_ptr() as *const c_char, &mut object)
    };
    if result != kResultOk {
        return Err(Vst3Error::CallFailed {
            operation: interface_name,
            result,
        });
    }
    unsafe { ComPtr::from_raw(object as *mut I) }.ok_or(Vst3Error::CallFailed {
        operation: "IPluginFactory::createInstance returned null",
        result: kResultFalse,
    })
}

fn exact(operation: &'static str, result: Steinberg::tresult) -> Result<(), Vst3Error> {
    if result == kResultOk {
        Ok(())
    } else {
        Err(Vst3Error::CallFailed { operation, result })
    }
}

fn processing_call(operation: &'static str, result: Steinberg::tresult) -> Result<(), Vst3Error> {
    // A processor with no start/stop work may return kNotImplemented.
    if result == kNotImplemented {
        Ok(())
    } else {
        exact(operation, result)
    }
}

#[cfg(test)]
mod processing_result_tests {
    use super::*;

    #[test]
    fn accepts_unimplemented_processing_transition_but_not_failure() {
        assert!(processing_call("setProcessing", kResultOk).is_ok());
        assert!(processing_call("setProcessing", kNotImplemented).is_ok());
        assert!(processing_call("setProcessing", kResultFalse).is_err());
    }
}

fn validate_note(channel: u8, velocity: f32) -> Result<(), Vst3Error> {
    if channel > 15 {
        return Err(Vst3Error::InvalidMidiChannel(channel));
    }
    if !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
        return Err(Vst3Error::InvalidVelocity(velocity));
    }
    Ok(())
}

fn valid_context(context: Vst3TimeContext) -> bool {
    context.project_time_samples >= 0
        && context.continuous_time_samples >= 0
        && context.project_time_music.is_finite()
        && context.bar_position_music.is_finite()
        && context.tempo.is_finite()
        && context.tempo > 0.0
        && context.time_signature_numerator > 0
        && context.time_signature_denominator > 0
}

fn raw_process_context(context: Vst3TimeContext) -> ProcessContext {
    let mut raw: ProcessContext = zeroed_ffi();
    raw.state = kProjectTimeMusicValid
        | kBarPositionValid
        | kTempoValid
        | kTimeSigValid
        | kContTimeValid
        | if context.playing { kPlaying } else { 0 };
    raw.sampleRate = VST3_SAMPLE_RATE;
    raw.projectTimeSamples = context.project_time_samples;
    raw.continousTimeSamples = context.continuous_time_samples;
    raw.projectTimeMusic = context.project_time_music;
    raw.barPositionMusic = context.bar_position_music;
    raw.tempo = context.tempo;
    raw.timeSigNumerator = context.time_signature_numerator;
    raw.timeSigDenominator = context.time_signature_denominator;
    raw
}

fn c_string<const N: usize>(value: &[c_char; N]) -> String {
    let bytes = value.iter().map(|byte| *byte as u8).collect::<Vec<_>>();
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn zeroed_ffi<T>() -> T {
    // SAFETY: used only for VST3 C ABI records made entirely of integers, floats, pointers, and
    // unions of those fields. All-zero is a valid initial representation which the callee fills.
    unsafe { MaybeUninit::<T>::zeroed().assume_init() }
}

#[derive(Clone, Debug, Serialize)]
pub struct PluginParameter {
    pub id: u32,
    pub name: String,
    pub key: String,
    pub units: String,
    pub default: f64,
    pub steps: i32,
    pub flags: i32,
}
fn utf16(v: &[u16]) -> String {
    String::from_utf16_lossy(&v[..v.iter().position(|x| *x == 0).unwrap_or(v.len())])
}
fn parameter_key(v: &str) -> String {
    v.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}
