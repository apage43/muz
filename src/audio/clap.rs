//! CLAP hosting. Native pointers stay alive with the module; only the coordinator services
//! main-thread callbacks. Audio processing uses preallocated ports and event storage.
use super::{
    AudioConfig, DeviceDebugState, DeviceError, DeviceEvent, DeviceEventKind, DeviceProcessor,
    ProcessContext,
};
use crate::model;
use anyhow::{Context, Result, ensure};
use clap_sys::{
    audio_buffer::*,
    entry::*,
    events::*,
    ext::{
        audio_ports::*, latency::*, note_ports::*, params::*, state::*, tail::*, thread_check::*,
    },
    factory::plugin_factory::*,
    host::*,
    plugin::*,
    process::*,
    stream::*,
    version::*,
};
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_char, c_void},
    io::{Read, Write},
    path::Path,
    ptr,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::ThreadId,
};
thread_local! {static AUDIO:Cell<bool>=const{Cell::new(false)};}
struct AudioRole(bool);
impl AudioRole {
    fn enter() -> Self {
        Self(AUDIO.replace(true))
    }
}
impl Drop for AudioRole {
    fn drop(&mut self) {
        AUDIO.set(self.0);
    }
}
struct MainService {
    pointer: Mutex<usize>,
    thread: ThreadId,
    callback: AtomicBool,
    restart: AtomicU32,
}
static SERVICES: OnceLock<Mutex<Vec<Weak<MainService>>>> = OnceLock::new();
pub fn service_main_thread() {
    let Some(registry) = SERVICES.get() else {
        return;
    };
    let mut list = registry.lock().unwrap();
    list.retain(|weak| {
        let Some(s) = weak.upgrade() else {
            return false;
        };
        if s.thread == std::thread::current().id() && s.callback.swap(false, Ordering::Relaxed) {
            let guard = s.pointer.lock().unwrap();
            let p = *guard as *const clap_plugin;
            if !p.is_null() {
                unsafe {
                    if let Some(f) = (*p).on_main_thread {
                        f(p);
                    }
                }
            }
        }
        true
    });
}
struct Host {
    api: clap_host,
    service: Arc<MainService>,
}
unsafe fn host<'a>(p: *const clap_host) -> &'a Host {
    unsafe { &*((*p).host_data as *const Host) }
}
unsafe extern "C" fn restart(h: *const clap_host) {
    unsafe { host(h) }
        .service
        .restart
        .fetch_or(1, Ordering::Relaxed);
}
unsafe extern "C" fn callback(h: *const clap_host) {
    unsafe { host(h) }
        .service
        .callback
        .store(true, Ordering::Relaxed);
}
unsafe extern "C" fn request_process(_: *const clap_host) {}
unsafe extern "C" fn is_main(h: *const clap_host) -> bool {
    !AUDIO.get() && unsafe { host(h) }.service.thread == std::thread::current().id()
}
unsafe extern "C" fn is_audio(_: *const clap_host) -> bool {
    AUDIO.get()
}
unsafe extern "C" fn rescan(h: *const clap_host, flags: u32) {
    if flags & !CLAP_AUDIO_PORTS_RESCAN_NAMES != 0 {
        unsafe { host(h) }
            .service
            .restart
            .fetch_or(2, Ordering::Relaxed);
    }
}
unsafe extern "C" fn note_rescan(h: *const clap_host, flags: u32) {
    if flags & CLAP_NOTE_PORTS_RESCAN_ALL != 0 {
        unsafe { host(h) }
            .service
            .restart
            .fetch_or(4, Ordering::Relaxed);
    }
}
unsafe extern "C" fn latency_changed(h: *const clap_host) {
    unsafe { host(h) }
        .service
        .restart
        .fetch_or(8, Ordering::Relaxed);
}

unsafe extern "C" fn support(_: *const clap_host, _: u32) -> bool {
    true
}
unsafe extern "C" fn dialects(_: *const clap_host) -> u32 {
    CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI
}
unsafe extern "C" fn clear(h: *const clap_host, _: u32, _: u32) {
    unsafe { host(h) }
        .service
        .restart
        .fetch_or(16, Ordering::Relaxed);
}
// INFO changes display names/visibility only. Source controls bind stable IDs during preparation.
unsafe extern "C" fn param_rescan(h: *const clap_host, flags: u32) {
    if flags & CLAP_PARAM_RESCAN_ALL != 0 {
        unsafe { host(h) }
            .service
            .restart
            .fetch_or(32, Ordering::Relaxed);
    }
}
static THREAD: clap_host_thread_check = clap_host_thread_check {
    is_main_thread: Some(is_main),
    is_audio_thread: Some(is_audio),
};
static LATENCY: clap_host_latency = clap_host_latency {
    changed: Some(latency_changed),
};
static TAIL: clap_host_tail = clap_host_tail {
    changed: Some(request_process),
};
static PARAMS: clap_host_params = clap_host_params {
    rescan: Some(param_rescan),
    clear: Some(clear),
    request_flush: Some(callback),
};
static PORTS: clap_host_audio_ports = clap_host_audio_ports {
    is_rescan_flag_supported: Some(support),
    rescan: Some(rescan),
};
static NOTES: clap_host_note_ports = clap_host_note_ports {
    supported_dialects: Some(dialects),
    rescan: Some(note_rescan),
};
unsafe extern "C" fn extension(_: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return ptr::null();
    }
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_THREAD_CHECK {
        &THREAD as *const _ as _
    } else if id == CLAP_EXT_LATENCY {
        &LATENCY as *const _ as _
    } else if id == CLAP_EXT_TAIL {
        &TAIL as *const _ as _
    } else if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as _
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const _ as _
    } else if id == CLAP_EXT_NOTE_PORTS {
        &NOTES as *const _ as _
    } else {
        ptr::null()
    }
}
#[derive(serde::Serialize)]
pub struct Parameter {
    pub id: u32,
    pub key: String,
    pub name: String,
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub flags: u32,
    #[serde(skip)]
    cookie: usize,
}
#[derive(serde::Serialize)]
pub struct Metadata {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub inputs: usize,
    pub outputs: usize,
    pub native_notes: bool,
    pub latency: u32,
    pub tail: u32,
}
enum Event {
    Note(clap_event_note),
    Expression(clap_event_note_expression),
    Parameter(clap_event_param_value),
    Midi(clap_event_midi),
}
impl Event {
    fn header(&self) -> &clap_event_header {
        match self {
            Self::Note(v) => &v.header,
            Self::Expression(v) => &v.header,
            Self::Parameter(v) => &v.header,
            Self::Midi(v) => &v.header,
        }
    }
}
fn header<T>(kind: u16, time: u32) -> clap_event_header {
    clap_event_header {
        size: std::mem::size_of::<T>() as u32,
        time,
        space_id: CLAP_CORE_EVENT_SPACE_ID,
        type_: kind,
        flags: 0,
    }
}
unsafe extern "C" fn size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const Vec<Event>) }.len() as u32
}
unsafe extern "C" fn get(list: *const clap_input_events, i: u32) -> *const clap_event_header {
    unsafe { &*((*list).ctx as *const Vec<Event>) }
        .get(i as usize)
        .map_or(ptr::null(), |e| e.header())
}
unsafe extern "C" fn output(_: *const clap_output_events, _: *const clap_event_header) -> bool {
    true
}
struct Ports {
    audio: Vec<[Vec<f32>; 2]>,
    pointers: Vec<[*mut f32; 2]>,
    raw: Vec<clap_audio_buffer>,
}
impl Ports {
    fn new(channels: Vec<u32>, frames: usize) -> Self {
        let audio = channels
            .iter()
            .map(|_| [vec![0.; frames], vec![0.; frames]])
            .collect();
        let pointers = channels.iter().map(|_| [ptr::null_mut(); 2]).collect();
        let raw = channels
            .into_iter()
            .map(|c| clap_audio_buffer {
                data32: ptr::null_mut(),
                data64: ptr::null_mut(),
                channel_count: c,
                latency: 0,
                constant_mask: 0,
            })
            .collect();
        Self {
            audio,
            pointers,
            raw,
        }
    }
    fn bind(&mut self, frames: usize) {
        for i in 0..self.raw.len() {
            for ch in 0..2 {
                self.audio[i][ch][..frames].fill(0.);
                self.pointers[i][ch] = self.audio[i][ch].as_mut_ptr();
            }
            self.raw[i].data32 = self.pointers[i].as_mut_ptr();
        }
    }
}
pub struct PreparedClap {
    _library: libloading::Library,
    entry: clap_plugin_entry,
    host: Box<Host>,
    plugin: *const clap_plugin,
    active: bool,
    started: bool,
    config: AudioConfig,
    token: u64,
    count: u64,
    metadata: Metadata,
    parameters: Vec<Parameter>,
    pending: Vec<Option<f64>>,
    timed: Vec<(usize, u32, f64)>,
    tunings: arrayvec::ArrayVec<(u64, f64), 256>,
    events: Vec<Event>,
    inputs: Ports,
    outputs: Ports,
    reset_pending: bool,
}
// A prepared instance moves to the audio thread; main-only callbacks use the synchronized
// service pointer. CLAP explicitly allows on_main_thread concurrently with process.
unsafe impl Send for PreparedClap {}
unsafe fn string(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}
impl PreparedClap {
    pub fn open(path: &Path, id: Option<&str>, config: AudioConfig, token: u64) -> Result<Self> {
        let library = unsafe { libloading::Library::new(path) }
            .with_context(|| format!("load {}", path.display()))?;
        let entry = unsafe { **library.get::<*const clap_plugin_entry>(b"clap_entry\0")? };
        ensure!(
            clap_version_is_compatible(entry.clap_version),
            "incompatible CLAP version"
        );
        let path_text = CString::new(path.as_os_str().as_encoded_bytes())?;
        ensure!(
            unsafe { entry.init.context("CLAP entry lacks init")?(path_text.as_ptr()) },
            "CLAP module init failed"
        );
        let service = Arc::new(MainService {
            pointer: Mutex::new(0),
            thread: std::thread::current().id(),
            callback: AtomicBool::new(false),
            restart: AtomicU32::new(0),
        });
        let mut host = Box::new(Host {
            api: clap_host {
                clap_version: CLAP_VERSION,
                host_data: ptr::null_mut(),
                name: c"muz".as_ptr(),
                vendor: c"muz".as_ptr(),
                url: c"https://localhost".as_ptr(),
                version: c"0.1.0".as_ptr(),
                get_extension: Some(extension),
                request_restart: Some(restart),
                request_process: Some(request_process),
                request_callback: Some(callback),
            },
            service,
        });
        host.api.host_data = (&mut *host as *mut Host).cast();
        let empty = || Ports::new(vec![], config.max_frames);
        let mut s = Self {
            _library: library,
            entry,
            host,
            plugin: ptr::null(),
            active: false,
            started: false,
            config,
            token,
            count: 0,
            metadata: Metadata {
                id: String::new(),
                name: String::new(),
                vendor: String::new(),
                version: String::new(),
                inputs: 0,
                outputs: 0,
                native_notes: false,
                latency: 0,
                tail: 0,
            },
            parameters: Vec::new(),
            pending: Vec::new(),
            timed: Vec::with_capacity(16384),
            tunings: arrayvec::ArrayVec::new(),
            events: Vec::with_capacity(18432),
            inputs: empty(),
            outputs: empty(),
            reset_pending: false,
        };
        unsafe {
            let factory = (entry.get_factory.context("CLAP entry lacks factory")?(
                CLAP_PLUGIN_FACTORY_ID.as_ptr(),
            ) as *const clap_plugin_factory)
                .as_ref()
                .context("missing CLAP plugin factory")?;
            let count = factory.get_plugin_count.context("missing plugin count")?(factory);
            let mut selected = ptr::null();
            for i in 0..count.min(1024) {
                let d = factory
                    .get_plugin_descriptor
                    .context("missing descriptor")?(factory, i);
                if !d.is_null() && id.is_none_or(|id| string((*d).id) == id) {
                    selected = d;
                    break;
                }
            }
            ensure!(!selected.is_null(), "CLAP plugin ID not found");
            let d = &*selected;
            s.metadata.id = string(d.id);
            s.metadata.name = string(d.name);
            s.metadata.vendor = string(d.vendor);
            s.metadata.version = string(d.version);
            s.plugin = factory.create_plugin.context("missing plugin creation")?(
                factory,
                &s.host.api,
                d.id,
            );
            ensure!(!s.plugin.is_null(), "CLAP plugin creation failed");
            *s.host.service.pointer.lock().unwrap() = s.plugin as usize;
            SERVICES
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .unwrap()
                .push(Arc::downgrade(&s.host.service));
            ensure!(
                (*s.plugin).init.context("missing plugin init")?(s.plugin),
                "CLAP init failed"
            );
        }
        service_main_thread();
        s.prepare_ports()?;
        s.read_parameters()?;
        s.activate()?;
        Ok(s)
    }
    fn ext<T>(&self, id: &CStr) -> Option<&T> {
        unsafe { ((*self.plugin).get_extension?(self.plugin, id.as_ptr()) as *const T).as_ref() }
    }
    fn prepare_ports(&mut self) -> Result<()> {
        let e = self
            .ext::<clap_plugin_audio_ports>(CLAP_EXT_AUDIO_PORTS)
            .context("CLAP audio ports required")?;
        let mut lists = [Vec::new(), Vec::new()];
        for (i, input) in [true, false].into_iter().enumerate() {
            let count = unsafe { e.count.context("missing port count")?(self.plugin, input) };
            ensure!(count <= 8, "at most eight CLAP audio ports per direction");
            for n in 0..count {
                let mut info = unsafe { std::mem::zeroed() };
                ensure!(
                    unsafe {
                        e.get.context("missing port info")?(self.plugin, n, input, &mut info)
                    },
                    "cannot query CLAP port"
                );
                ensure!(
                    matches!(info.channel_count, 1 | 2),
                    "CLAP ports must be mono/stereo"
                );
                lists[i].push(info.channel_count);
            }
        }
        ensure!(!lists[1].is_empty(), "CLAP plugin has no audio output");
        self.metadata.inputs = lists[0].len();
        self.metadata.outputs = lists[1].len();
        self.inputs = Ports::new(std::mem::take(&mut lists[0]), self.config.max_frames);
        self.outputs = Ports::new(std::mem::take(&mut lists[1]), self.config.max_frames);
        if let Some(e) = self.ext::<clap_plugin_note_ports>(CLAP_EXT_NOTE_PORTS) {
            let mut info = unsafe { std::mem::zeroed() };
            if unsafe { e.count.context("missing note port count")?(self.plugin, true) } > 0
                && unsafe {
                    e.get.context("missing note port info")?(self.plugin, 0, true, &mut info)
                }
            {
                self.metadata.native_notes = info.supported_dialects & CLAP_NOTE_DIALECT_CLAP != 0;
            }
        }
        Ok(())
    }
    fn read_parameters(&mut self) -> Result<()> {
        let mut out = Vec::new();
        if let Some(e) = self.ext::<clap_plugin_params>(CLAP_EXT_PARAMS) {
            let count = unsafe { e.count.context("missing param count")?(self.plugin) };
            ensure!(count <= 16384, "too many CLAP parameters");
            for i in 0..count {
                let mut info = unsafe { std::mem::zeroed() };
                ensure!(
                    unsafe { e.get_info.context("missing param info")?(self.plugin, i, &mut info) },
                    "CLAP parameter query failed"
                );
                out.push(Parameter {
                    id: info.id,
                    key: format!("p{}", info.id),
                    name: unsafe { string(info.name.as_ptr()) },
                    module: unsafe { string(info.module.as_ptr()) },
                    min: info.min_value,
                    max: info.max_value,
                    default: info.default_value,
                    flags: info.flags,
                    cookie: info.cookie as usize,
                });
            }
        }
        self.parameters = out;
        self.pending = vec![None; self.parameters.len()];
        Ok(())
    }
    fn activate(&mut self) -> Result<()> {
        ensure!(
            unsafe {
                (*self.plugin).activate.context("missing activate")?(
                    self.plugin,
                    self.config.sample_rate as f64,
                    1,
                    self.config.max_frames as u32,
                )
            },
            "CLAP activate failed"
        );
        self.active = true;
        {
            let _role = AudioRole::enter();
            ensure!(
                unsafe {
                    (*self.plugin)
                        .start_processing
                        .context("missing start_processing")?(self.plugin)
                },
                "CLAP start failed"
            );
        }
        self.started = true;
        self.refresh_metadata();
        self.host.service.restart.store(0, Ordering::Relaxed);
        Ok(())
    }
    fn deactivate(&mut self) {
        unsafe {
            if self.started {
                let _role = AudioRole::enter();
                if let Some(f) = (*self.plugin).stop_processing {
                    f(self.plugin);
                }
                self.started = false;
            }
            if self.active {
                if let Some(f) = (*self.plugin).deactivate {
                    f(self.plugin);
                }
                self.active = false;
            }
        }
    }
    fn refresh_metadata(&mut self) {
        self.metadata.latency = self
            .ext::<clap_plugin_latency>(CLAP_EXT_LATENCY)
            .and_then(|e| e.get)
            .map_or(0, |f| unsafe { f(self.plugin) });
        self.metadata.tail = self
            .ext::<clap_plugin_tail>(CLAP_EXT_TAIL)
            .and_then(|e| e.get)
            .map_or(0, |f| unsafe { f(self.plugin) });
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub fn parameters(&self) -> &[Parameter] {
        &self.parameters
    }
    pub fn finish_preparation(&mut self) -> Result<()> {
        let mut left = vec![0.; self.config.max_frames];
        let mut right = vec![0.; self.config.max_frames];
        let transport = super::TransportSnapshot {
            sample_rate: self.config.sample_rate as f64,
            running: false,
            sample_position: 0,
            beat_position: 0.,
            bpm: 120.,
            meter: [4, 4],
            loop_ticks: 0,
            current_tick: 0.,
            project_frame: 0.,
            ended: false,
        };
        self.process(
            ProcessContext {
                frames: self.config.max_frames,
                block_start_sample: 0,
                transport,
            },
            &[],
            &mut left,
            &mut right,
        )?;
        service_main_thread();
        self.refresh_metadata();
        self.host.service.restart.store(0, Ordering::Relaxed);
        Ok(())
    }
    pub fn set(&mut self, name: &str, value: f64) -> Result<()> {
        let i = self
            .parameters
            .iter()
            .position(|p| name == p.key || name == p.name || name.parse::<u32>().ok() == Some(p.id))
            .context("unknown CLAP parameter")?;
        let p = &self.parameters[i];
        ensure!(
            value.is_finite()
                && value >= p.min
                && value <= p.max
                && p.flags & CLAP_PARAM_IS_READONLY == 0,
            "invalid/read-only CLAP parameter (values are plain units)"
        );
        self.pending[i] = Some(if p.flags & CLAP_PARAM_IS_STEPPED != 0 {
            value.round()
        } else {
            value
        });
        Ok(())
    }
    pub fn load_state(&mut self, path: &Path) -> Result<()> {
        let bytes = std::fs::read(path)?;
        ensure!(bytes.len() <= 64 * 1024 * 1024, "CLAP state exceeds 64 MiB");
        self.deactivate();
        let e = self
            .ext::<clap_plugin_state>(CLAP_EXT_STATE)
            .context("CLAP state unsupported")?;
        let mut cursor = std::io::Cursor::new(bytes);
        let stream = clap_istream {
            ctx: (&mut cursor as *mut std::io::Cursor<Vec<u8>>).cast(),
            read: Some(read_state),
        };
        ensure!(
            unsafe { e.load.context("CLAP state load unavailable")?(self.plugin, &stream) },
            "CLAP state load failed"
        );
        service_main_thread();
        self.prepare_ports()?;
        self.read_parameters()?;
        self.activate()
    }
    pub fn save_state(&self, path: &Path) -> Result<()> {
        let e = self
            .ext::<clap_plugin_state>(CLAP_EXT_STATE)
            .context("CLAP state unsupported")?;
        let mut bytes = Vec::<u8>::new();
        let stream = clap_ostream {
            ctx: (&mut bytes as *mut Vec<u8>).cast(),
            write: Some(write_state),
        };
        ensure!(
            unsafe { e.save.context("CLAP save unavailable")?(self.plugin, &stream) },
            "CLAP state save failed"
        );
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut f = tempfile::NamedTempFile::new_in(parent)?;
        f.write_all(&bytes)?;
        f.persist(path)?;
        Ok(())
    }
    fn push(&mut self, e: Event) -> Result<(), DeviceError> {
        if self.events.len() == self.events.capacity() {
            return Err(DeviceError::InvalidConfig("CLAP event capacity exceeded"));
        }
        self.events.push(e);
        Ok(())
    }
    fn midi(&mut self, time: u32, bytes: [u8; 3]) -> Result<(), DeviceError> {
        self.push(Event::Midi(clap_event_midi {
            header: header::<clap_event_midi>(CLAP_EVENT_MIDI, time),
            port_index: 0,
            data: bytes,
        }))
    }
}
unsafe extern "C" fn read_state(s: *const clap_istream, b: *mut c_void, n: u64) -> i64 {
    if n > 64 * 1024 * 1024 {
        return -1;
    }
    unsafe {
        (&mut *((*s).ctx as *mut std::io::Cursor<Vec<u8>>))
            .read(std::slice::from_raw_parts_mut(b.cast(), n as usize))
            .map_or(-1, |n| n as i64)
    }
}
unsafe extern "C" fn write_state(s: *const clap_ostream, b: *const c_void, n: u64) -> i64 {
    let out = unsafe { &mut *((*s).ctx as *mut Vec<u8>) };
    if n > 64 * 1024 * 1024 || out.len() + n as usize > 64 * 1024 * 1024 {
        return -1;
    }
    out.extend_from_slice(unsafe { std::slice::from_raw_parts(b.cast(), n as usize) });
    n as i64
}
impl Drop for PreparedClap {
    fn drop(&mut self) {
        if !self.plugin.is_null() {
            *self.host.service.pointer.lock().unwrap() = 0;
            self.deactivate();
            unsafe {
                if let Some(f) = (*self.plugin).destroy {
                    f(self.plugin);
                }
            }
        }
        unsafe {
            if let Some(f) = self.entry.deinit {
                f();
            }
        }
    }
}
impl DeviceProcessor for PreparedClap {
    fn accepts_note_expression(&self) -> bool {
        self.metadata.native_notes
    }
    fn kind(&self) -> model::DeviceKind {
        model::DeviceKind::Clap
    }
    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            instance_token: self.token,
            process_count: self.count,
            gain_reduction_db: 0.,
            latency_samples: self.metadata.latency,
            tail_samples: self.metadata.tail,
            is_plugin: true,
            restart_flags: self.host.service.restart.load(Ordering::Relaxed),
        }
    }
    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        self.set(name, value as f64)
            .map_err(|_| DeviceError::UnknownParameter { kind: self.kind() })
    }
    fn accepts_parameter_offsets(&self) -> bool {
        true
    }
    fn set_parameter_at(&mut self, name: &str, value: f32, offset: u32) -> Result<(), DeviceError> {
        self.set(name, value as f64)
            .map_err(|_| DeviceError::UnknownParameter { kind: self.kind() })?;
        let index = self
            .parameters
            .iter()
            .position(|p| name == p.key || name == p.name || name.parse::<u32>().ok() == Some(p.id))
            .ok_or(DeviceError::InvalidConfig("unknown CLAP parameter"))?;
        let value = self.pending[index].take().unwrap();
        if self.timed.len() == self.timed.capacity() {
            return Err(DeviceError::InvalidConfig("CLAP automation queue full"));
        }
        self.timed.push((index, offset, value));
        Ok(())
    }
    fn reset(&mut self) {
        self.reset_pending = true;
    }

    fn process(
        &mut self,
        ctx: ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        let _role = AudioRole::enter();
        if ctx.frames > self.config.max_frames
            || left.len() < ctx.frames
            || right.len() < ctx.frames
        {
            return Err(DeviceError::BufferTooShort);
        }
        if self.reset_pending
            || events
                .iter()
                .any(|e| matches!(e.kind, DeviceEventKind::Flush))
        {
            unsafe {
                if let Some(f) = (*self.plugin).reset {
                    f(self.plugin);
                }
            }
            self.reset_pending = false;
            self.tunings.clear();
        }
        self.events.clear();
        for i in 0..self.pending.len() {
            if let Some(value) = self.pending[i].take() {
                let p = &self.parameters[i];
                self.push(Event::Parameter(clap_event_param_value {
                    header: header::<clap_event_param_value>(CLAP_EVENT_PARAM_VALUE, 0),
                    param_id: p.id,
                    cookie: p.cookie as *mut c_void,
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value,
                }))?;
            }
        }
        for i in 0..self.timed.len() {
            let (index, time, value) = self.timed[i];
            let p = &self.parameters[index];
            self.push(Event::Parameter(clap_event_param_value {
                header: header::<clap_event_param_value>(CLAP_EVENT_PARAM_VALUE, time),
                param_id: p.id,
                cookie: p.cookie as *mut c_void,
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            }))?;
        }
        self.timed.clear();
        for e in events {
            match e.kind {
                DeviceEventKind::NoteExpression {
                    note_id,
                    channel,
                    key,
                    expression,
                    value,
                } => {
                    if !self.metadata.native_notes {
                        return Err(DeviceError::InvalidConfig(
                            "CLAP plugin does not accept native note expression",
                        ));
                    }
                    self.push(Event::Expression(clap_event_note_expression {
                        header: header::<clap_event_note_expression>(
                            CLAP_EVENT_NOTE_EXPRESSION,
                            e.offset,
                        ),
                        expression_id: expression as i32,
                        note_id: note_id as i32,
                        port_index: 0,
                        channel: channel as i16,
                        key: key as i16,
                        value: value
                            + if expression == 2 {
                                self.tunings
                                    .iter()
                                    .find(|v| v.0 == note_id)
                                    .map_or(0., |v| v.1)
                            } else {
                                0.
                            },
                    }))?;
                }
                DeviceEventKind::NoteOn {
                    note_id,
                    channel,
                    key,
                    velocity,
                    ..
                }
                | DeviceEventKind::NoteOff {
                    note_id,
                    channel,
                    key,
                    velocity,
                } => {
                    let on = matches!(e.kind, DeviceEventKind::NoteOn { .. });
                    if note_id > i32::MAX as u64 {
                        return Err(DeviceError::InvalidConfig("CLAP note ID exhausted"));
                    }
                    if let Some(i) = self.tunings.iter().position(|v| v.0 == note_id) {
                        self.tunings.swap_remove(i);
                    }
                    if let DeviceEventKind::NoteOn { pitch, .. } = e.kind {
                        self.tunings
                            .try_push((note_id, (pitch - key as f32) as f64))
                            .map_err(|_| {
                                DeviceError::InvalidConfig("CLAP active-note budget exceeded")
                            })?;
                    }
                    if self.metadata.native_notes {
                        self.push(Event::Note(clap_event_note {
                            header: header::<clap_event_note>(
                                if on {
                                    CLAP_EVENT_NOTE_ON
                                } else {
                                    CLAP_EVENT_NOTE_OFF
                                },
                                e.offset,
                            ),
                            note_id: note_id as i32,
                            port_index: 0,
                            channel: channel as i16,
                            key: key as i16,
                            velocity: velocity as f64,
                        }))?;
                        if let DeviceEventKind::NoteOn { pitch, .. } = e.kind {
                            if (pitch - key as f32).abs() > 1e-6 {
                                self.push(Event::Expression(clap_event_note_expression {
                                    header: header::<clap_event_note_expression>(
                                        CLAP_EVENT_NOTE_EXPRESSION,
                                        e.offset,
                                    ),
                                    expression_id: CLAP_NOTE_EXPRESSION_TUNING,
                                    note_id: note_id as i32,
                                    port_index: 0,
                                    channel: channel as i16,
                                    key: key as i16,
                                    value: (pitch - key as f32) as f64,
                                }))?;
                            }
                        }
                    } else {
                        if let DeviceEventKind::NoteOn { pitch, .. } = e.kind {
                            if (pitch - key as f32).abs() > 1e-6 {
                                return Err(DeviceError::InvalidConfig(
                                    "microtonal pitch requires a CLAP native note port",
                                ));
                            }
                        }
                        self.midi(
                            e.offset,
                            [
                                if on { 0x90 } else { 0x80 } | channel,
                                key,
                                (velocity * 127.).round() as u8,
                            ],
                        )?;
                    }
                }
                DeviceEventKind::Controller {
                    channel,
                    controller,
                    value,
                } => self.midi(e.offset, [0xb0 | channel, controller, value])?,
                DeviceEventKind::Midi { bytes, .. } => self.midi(e.offset, bytes)?,
                DeviceEventKind::Flush => {
                    // Transport flushes reset the instance before state chase. In particular,
                    // Surge 1.3.4 does not safely handle a wildcard-key NOTE_CHOKE.
                    if e.offset != 0 {
                        return Err(DeviceError::InvalidConfig(
                            "CLAP flush must begin a transport slice",
                        ));
                    }
                }
            }
        }
        self.events.sort_unstable_by_key(|e| {
            let h = e.header();
            (
                h.time,
                match h.type_ {
                    CLAP_EVENT_NOTE_OFF | CLAP_EVENT_NOTE_CHOKE => 0,
                    CLAP_EVENT_NOTE_ON => 2,
                    CLAP_EVENT_NOTE_EXPRESSION => 3,
                    _ => 1,
                },
            )
        });
        let input = clap_input_events {
            ctx: (&mut self.events as *mut Vec<Event>).cast(),
            size: Some(size),
            get: Some(get),
        };
        let output = clap_output_events {
            ctx: ptr::null_mut(),
            try_push: Some(output),
        };
        self.inputs.bind(ctx.frames);
        self.outputs.bind(ctx.frames);
        if !self.inputs.audio.is_empty() {
            self.inputs.audio[0][0][..ctx.frames].copy_from_slice(&left[..ctx.frames]);
            self.inputs.audio[0][1][..ctx.frames].copy_from_slice(&right[..ctx.frames]);
        }
        let mut transport: clap_event_transport = unsafe { std::mem::zeroed() };
        transport.header = header::<clap_event_transport>(CLAP_EVENT_TRANSPORT, 0);
        transport.flags = CLAP_TRANSPORT_HAS_TEMPO
            | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
            | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE
            | CLAP_TRANSPORT_HAS_TIME_SIGNATURE
            | if ctx.transport.running {
                CLAP_TRANSPORT_IS_PLAYING
            } else {
                0
            };
        transport.song_pos_beats = (ctx.transport.beat_position
            * clap_sys::fixedpoint::CLAP_BEATTIME_FACTOR as f64)
            .round() as i64;
        transport.song_pos_seconds = (ctx.transport.project_frame / ctx.transport.sample_rate
            * clap_sys::fixedpoint::CLAP_SECTIME_FACTOR as f64)
            .round() as i64;
        transport.tempo = ctx.transport.bpm;
        transport.tsig_num = ctx.transport.meter[0] as u16;
        transport.tsig_denom = ctx.transport.meter[1] as u16;
        let process = clap_process {
            steady_time: ctx.block_start_sample as i64,
            frames_count: ctx.frames as u32,
            transport: &transport,
            audio_inputs: self.inputs.raw.as_ptr(),
            audio_outputs: self.outputs.raw.as_mut_ptr(),
            audio_inputs_count: self.inputs.raw.len() as u32,
            audio_outputs_count: self.outputs.raw.len() as u32,
            in_events: &input,
            out_events: &output,
        };
        let status = unsafe {
            (*self.plugin)
                .process
                .ok_or(DeviceError::InvalidConfig("CLAP process unavailable"))?(
                self.plugin,
                &process,
            )
        };
        if status == CLAP_PROCESS_ERROR {
            return Err(DeviceError::InvalidConfig("CLAP processing failed"));
        }
        left[..ctx.frames].copy_from_slice(&self.outputs.audio[0][0][..ctx.frames]);
        let r = if self.outputs.raw[0].channel_count == 1 {
            0
        } else {
            1
        };
        right[..ctx.frames].copy_from_slice(&self.outputs.audio[0][r][..ctx.frames]);
        self.count += 1;
        Ok(())
    }
}
