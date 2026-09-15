//! CLAP hosting. Native pointers stay alive with the module; only the coordinator services
//! main-thread callbacks. Audio processing uses preallocated ports and event storage.
use super::{
    AudioConfig, DeviceDebugState, DeviceError, DeviceEvent, DeviceEventKind, DeviceProcessor,
    ProcessContext,
    gui::{EditorWindow, WindowEvent},
};
use crate::model;
use anyhow::{Context, Result, ensure};
use clap_sys::{
    audio_buffer::*,
    entry::*,
    events::*,
    ext::{
        audio_ports::*, gui::*, latency::*, note_ports::*, params::*, posix_fd_support::*,
        state::*, tail::*, thread_check::*, timer_support::*,
    },
    factory::plugin_factory::*,
    host::*,
    id::*,
    plugin::*,
    process::*,
    stream::*,
    version::*,
};
use parking_lot::Mutex;
use std::{
    cell::Cell,
    collections::BTreeMap,
    ffi::{CStr, CString, c_char, c_void},
    io::{Read, Write},
    path::Path,
    ptr,
    sync::{
        Arc, LazyLock, Weak,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::ThreadId,
    time::{Duration, Instant},
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
    info: Mutex<LiveInfo>,
    /// Host timers this instance registered.
    timers: Mutex<Timers>,
    /// Descriptors this instance wants readiness for, with its flags.
    fds: Mutex<BTreeMap<i32, clap_posix_fd_flags>>,
}
/// Serve-thread view of one live instance: the device it serves and its editor.
struct LiveInfo {
    device: Option<model::Id>,
    name: String,
    /// Instance token the applied graph runs this device as.
    token: u64,
    gui: GuiState,
}
#[derive(Default)]
struct Timers {
    next_id: clap_id,
    entries: BTreeMap<clap_id, HostTimer>,
}
struct HostTimer {
    period: Duration,
    due: Instant,
}
impl Timers {
    fn insert(&mut self, period_ms: u32) -> clap_id {
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == CLAP_INVALID_ID {
            self.next_id = 1;
        }
        let period = Duration::from_millis(u64::from(period_ms));
        self.entries.insert(
            self.next_id,
            HostTimer {
                period,
                due: Instant::now() + period,
            },
        );
        self.next_id
    }
    fn remove(&mut self, id: clap_id) -> bool {
        self.entries.remove(&id).is_some()
    }
    /// Ids due now, each rescheduled by whole periods so a host that falls
    /// behind reports a missed timer once instead of firing a catch-up storm.
    fn due(&mut self, now: Instant) -> Vec<clap_id> {
        let mut due = Vec::new();
        for (id, timer) in self.entries.iter_mut() {
            if timer.due > now {
                continue;
            }
            due.push(*id);
            let period = timer.period.as_nanos().max(1);
            let missed = now.duration_since(timer.due).as_nanos() / period + 1;
            let skip = timer
                .period
                .saturating_mul(u32::try_from(missed).unwrap_or(u32::MAX));
            timer.due = timer
                .due
                .checked_add(skip)
                .unwrap_or_else(|| now + timer.period);
        }
        due
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.entries.values().map(|timer| timer.due).min()
    }
}
#[derive(Default)]
struct GuiState {
    /// `clap.gui` is present and accepts an X11 parent window.
    available: bool,
    editor: Option<Editor>,
    /// Newest client size the plugin asked the host for.
    resize: Option<(u32, u32)>,
    /// Visibility the plugin asked for last.
    visible: Option<bool>,
    /// The plugin withdrew its view; `true` asks for a `destroy` acknowledgement.
    closed: Option<bool>,
}
struct Editor {
    window: EditorWindow,
}
/// One editor request a plugin made through `clap_host_gui`.
#[derive(Clone, Copy)]
enum EditorRequest {
    Resize(u32, u32),
    Visible(bool),
    Closed(bool),
}
static SERVICES: LazyLock<Mutex<Vec<Weak<MainService>>>> = LazyLock::new(|| Mutex::new(Vec::new()));
/// Serve-thread reactor: plugins that asked for a main-thread callback get one.
/// Work is collected under the registry lock and the plugin is called after it
/// is released, because a callback may reenter the host API.
pub fn service_main_thread() {
    let mut pending = Vec::new();
    {
        let mut list = SERVICES.lock();
        list.retain(|weak| {
            let Some(service) = weak.upgrade() else {
                return false;
            };
            if service.thread == std::thread::current().id()
                && service.callback.swap(false, Ordering::Relaxed)
                && let Some(plugin) = live_plugin(&service)
            {
                pending.push((service, plugin));
            }
            true
        });
    }
    for (_, plugin) in pending {
        unsafe {
            if let Some(f) = (*plugin).on_main_thread {
                f(plugin);
            }
        }
    }
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
unsafe extern "C" fn latency_changed(_: *const clap_host) {
    // CLAP allows a latency change only while a plugin is being activated, and
    // every preparation reads the latency afterwards; a plugin that changes it
    // later must call request_restart, which does rebuild. Ignoring the late
    // announcement is deliberate: JUCE plugins (CHOWTapeModel 2.11.4) announce
    // one after every activation with a value that moves between incidences
    // (for example 39, 40 and 6 samples for one configuration), and reacting to
    // it re-prepared the graph forever, fading playback instead of playing it.
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
// JUCE's CLAP wrapper drives its Linux message loop from these two services: it
// registers a ~20 ms timer and, when the host offers it, the file descriptors of
// JUCE's own event loop, then drains the JUCE queue from the callbacks. A host
// that does not advertise them gets a created, mapped, parented editor that
// never paints, because a plugin can only register what the host answers for.
unsafe extern "C" fn register_timer(
    h: *const clap_host,
    period_ms: u32,
    timer_id: *mut clap_id,
) -> bool {
    if timer_id.is_null() || period_ms == 0 {
        return false;
    }
    let service = Arc::clone(&unsafe { host(h) }.service);
    let id = service.timers.lock().insert(period_ms);
    unsafe { *timer_id = id };
    true
}
unsafe extern "C" fn unregister_timer(h: *const clap_host, timer_id: clap_id) -> bool {
    let service = Arc::clone(&unsafe { host(h) }.service);
    service.timers.lock().remove(timer_id)
}
unsafe extern "C" fn register_fd(h: *const clap_host, fd: i32, flags: clap_posix_fd_flags) -> bool {
    if fd < 0 || flags == 0 {
        return false;
    }
    let service = Arc::clone(&unsafe { host(h) }.service);
    let mut fds = service.fds.lock();
    if fds.contains_key(&fd) {
        return false;
    }
    fds.insert(fd, flags);
    true
}
unsafe extern "C" fn modify_fd(h: *const clap_host, fd: i32, flags: clap_posix_fd_flags) -> bool {
    let service = Arc::clone(&unsafe { host(h) }.service);
    if flags == 0 {
        return service.fds.lock().remove(&fd).is_some();
    }
    match service.fds.lock().get_mut(&fd) {
        Some(entry) => {
            *entry = flags;
            true
        }
        None => false,
    }
}
unsafe extern "C" fn unregister_fd(h: *const clap_host, fd: i32) -> bool {
    let service = Arc::clone(&unsafe { host(h) }.service);
    service.fds.lock().remove(&fd).is_some()
}
static THREAD: clap_host_thread_check = clap_host_thread_check {
    is_main_thread: Some(is_main),
    is_audio_thread: Some(is_audio),
};
static TIMERS: clap_host_timer_support = clap_host_timer_support {
    register_timer: Some(register_timer),
    unregister_timer: Some(unregister_timer),
};
static DESCRIPTORS: clap_host_posix_fd_support = clap_host_posix_fd_support {
    register_fd: Some(register_fd),
    modify_fd: Some(modify_fd),
    unregister_fd: Some(unregister_fd),
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
static GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(hints_changed),
    request_resize: Some(request_resize),
    request_show: Some(request_show),
    request_hide: Some(request_hide),
    closed: Some(gui_closed),
};
/// Editor requests are recorded here and serviced by the main thread, so a plugin
/// calling them from its own UI thread never touches a window itself. Each kind
/// keeps its own slot: a resize between two visibility requests must not erase
/// either of them.
fn request(h: *const clap_host, request: EditorRequest) {
    let mut info = unsafe { host(h) }.service.info.lock();
    match request {
        EditorRequest::Resize(width, height) => info.gui.resize = Some((width, height)),
        EditorRequest::Visible(visible) => info.gui.visible = Some(visible),
        EditorRequest::Closed(destroyed) => info.gui.closed = Some(destroyed),
    }
}
unsafe extern "C" fn hints_changed(_: *const clap_host) {}
unsafe extern "C" fn request_resize(h: *const clap_host, width: u32, height: u32) -> bool {
    request(h, EditorRequest::Resize(width, height));
    true
}
unsafe extern "C" fn request_show(h: *const clap_host) -> bool {
    request(h, EditorRequest::Visible(true));
    true
}
unsafe extern "C" fn request_hide(h: *const clap_host) -> bool {
    request(h, EditorRequest::Visible(false));
    true
}
unsafe extern "C" fn gui_closed(h: *const clap_host, destroyed: bool) {
    request(h, EditorRequest::Closed(destroyed));
}
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
    } else if id == CLAP_EXT_GUI {
        &GUI as *const _ as _
    } else if id == CLAP_EXT_TIMER_SUPPORT {
        &TIMERS as *const _ as _
    } else if id == CLAP_EXT_POSIX_FD_SUPPORT {
        &DESCRIPTORS as *const _ as _
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
    /// `device` names the source device this instance serves, so live commands
    /// (`muz gui`, `muz state`) can address the running instance.
    pub fn open(
        path: &Path,
        id: Option<&str>,
        config: AudioConfig,
        token: u64,
        device: Option<&model::Id>,
    ) -> Result<Self> {
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
            info: Mutex::new(LiveInfo {
                device: device.cloned(),
                name: String::new(),
                token,
                gui: GuiState::default(),
            }),
            timers: Mutex::new(Timers::default()),
            fds: Mutex::new(BTreeMap::new()),
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
            *s.host.service.pointer.lock() = s.plugin as usize;
            s.host.service.info.lock().name = s.metadata.name.clone();
            SERVICES.lock().push(Arc::downgrade(&s.host.service));
            ensure!(
                (*s.plugin).init.context("missing plugin init")?(s.plugin),
                "CLAP init failed"
            );
        }
        service_main_thread();
        s.prepare_ports()?;
        s.read_parameters()?;
        s.activate()?;
        // Queried after init and activation: plugins reject extension calls earlier.
        s.host.service.info.lock().gui.available = editor_available(s.plugin);
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
        let bytes = crate::assets::read_bounded(path, 64 * 1024 * 1024)?;
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
        write_state_file(path, &state_bytes(self.plugin)?)
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
fn state_bytes(plugin: *const clap_plugin) -> Result<Vec<u8>> {
    let e = unsafe { plugin_state(plugin) }.context("CLAP state unsupported")?;
    let mut bytes = Vec::<u8>::new();
    let stream = clap_ostream {
        ctx: (&mut bytes as *mut Vec<u8>).cast(),
        write: Some(write_state),
    };
    ensure!(
        unsafe { e.save.context("CLAP save unavailable")?(plugin, &stream) },
        "CLAP state save failed"
    );
    Ok(bytes)
}
fn write_state_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut f = tempfile::NamedTempFile::new_in(parent)?;
    f.write_all(bytes)?;
    f.persist(path)?;
    Ok(())
}
// --- live instances -------------------------------------------------------
//
// Preparation, retirement and the drop of a retired engine all happen on the
// serve thread, so it owns every live instance's lifetime and may also drive
// its editor and read its state. State and editor functions are main-thread
// operations in CLAP; the audio thread keeps processing throughout.

/// `clap.gui` present and accepting an X11 parent window.
fn editor_available(plugin: *const clap_plugin) -> bool {
    unsafe { plugin_gui(plugin) }.is_some_and(|gui| unsafe {
        gui.is_api_supported
            .map(|f| f(plugin, CLAP_WINDOW_API_X11.as_ptr(), false))
            .unwrap_or(false)
    })
}
/// SAFETY: the extension table belongs to the module, which outlives the instance.
unsafe fn plugin_gui(plugin: *const clap_plugin) -> Option<&'static clap_plugin_gui> {
    unsafe {
        ((*plugin).get_extension?(plugin, CLAP_EXT_GUI.as_ptr()) as *const clap_plugin_gui).as_ref()
    }
}
/// SAFETY: as [`plugin_gui`].
unsafe fn plugin_state(plugin: *const clap_plugin) -> Option<&'static clap_plugin_state> {
    unsafe {
        ((*plugin).get_extension?(plugin, CLAP_EXT_STATE.as_ptr()) as *const clap_plugin_state)
            .as_ref()
    }
}
/// SAFETY: as [`plugin_gui`].
unsafe fn plugin_timers(plugin: *const clap_plugin) -> Option<&'static clap_plugin_timer_support> {
    unsafe {
        ((*plugin).get_extension?(plugin, CLAP_EXT_TIMER_SUPPORT.as_ptr())
            as *const clap_plugin_timer_support)
            .as_ref()
    }
}
/// SAFETY: as [`plugin_gui`].
unsafe fn plugin_descriptors(
    plugin: *const clap_plugin,
) -> Option<&'static clap_plugin_posix_fd_support> {
    unsafe {
        ((*plugin).get_extension?(plugin, CLAP_EXT_POSIX_FD_SUPPORT.as_ptr())
            as *const clap_plugin_posix_fd_support)
            .as_ref()
    }
}
fn live_services() -> Vec<Arc<MainService>> {
    SERVICES.lock().iter().filter_map(Weak::upgrade).collect()
}
fn live_plugin(service: &Arc<MainService>) -> Option<*const clap_plugin> {
    let plugin = *service.pointer.lock() as *const clap_plugin;
    (!plugin.is_null()).then_some(plugin)
}
/// The live instance the applied graph runs `device` as.
fn live_service(device: &model::Id, token: u64) -> Option<Arc<MainService>> {
    live_services().into_iter().find(|service| {
        let serves = {
            let info = service.info.lock();
            info.device.as_ref() == Some(device) && info.token == token
        };
        serves && live_plugin(service).is_some()
    })
}
/// Editor state of one live instance.
#[derive(Clone, Copy, Debug)]
pub struct GuiStatus {
    /// The instance exposes an X11 CLAP editor.
    pub available: bool,
    /// An editor is open.
    pub open: bool,
    /// X11 window holding the open editor.
    pub window: Option<u32>,
}
/// Editor state of the applied instance of `device`. `None` while the device has
/// no live instance.
pub fn gui_status(device: &model::Id, token: u64) -> Option<GuiStatus> {
    let service = live_service(device, token)?;
    let info = service.info.lock();
    Some(GuiStatus {
        available: info.gui.available,
        open: info.gui.editor.is_some(),
        window: info.gui.editor.as_ref().map(|editor| editor.window.id()),
    })
}
/// Open the editor of the applied `device` instance; already-open editors are
/// kept. Returns the X11 window the editor is embedded in.
pub fn open_gui(device: &model::Id, token: u64) -> Result<u32> {
    let service = live_service(device, token)
        .with_context(|| format!("device {device} has no live plugin instance"))?;
    if let Some(editor) = service.info.lock().gui.editor.as_ref() {
        return Ok(editor.window.id());
    }
    let plugin = live_plugin(&service).context("the live plugin instance is gone")?;
    let editor = create_editor(plugin, &editor_title(&service))?;
    let window = editor.window.id();
    let mut info = service.info.lock();
    if info.gui.editor.is_none() {
        info.gui.editor = Some(editor);
    }
    Ok(window)
}
/// Close the editor of the applied `device` instance; `false` when none was open.
pub fn close_gui(device: &model::Id, token: u64) -> Result<bool> {
    let service = live_service(device, token)
        .with_context(|| format!("device {device} has no live plugin instance"))?;
    let Some(editor) = take_editor(&service) else {
        return Ok(false);
    };
    if let Some(plugin) = live_plugin(&service) {
        drop_view(plugin, true);
    }
    drop(editor);
    Ok(true)
}
/// Write the state of the applied `device` instance to `path`. Main-thread only.
pub fn save_live_state(device: &model::Id, token: u64, path: &Path) -> Result<()> {
    let service = live_service(device, token)
        .with_context(|| format!("device {device} has no live plugin instance"))?;
    let plugin = live_plugin(&service).context("the live plugin instance is gone")?;
    write_state_file(path, &state_bytes(plugin)?)
}
/// Service editor windows and plugin requests. Serve-thread only.
pub fn service_editors() {
    for service in live_services() {
        let Some(plugin) = live_plugin(&service) else {
            // The instance was retired on this thread; drop the window and every
            // registration untouched, without calling the plugin again.
            let mut info = service.info.lock();
            info.gui.editor = None;
            info.gui.resize = None;
            info.gui.visible = None;
            info.gui.closed = None;
            info.gui.available = false;
            drop(info);
            service.timers.lock().entries.clear();
            service.fds.lock().clear();
            continue;
        };
        service_window(&service, plugin);
        service_request(&service, plugin);
    }
}
fn service_window(service: &Arc<MainService>, plugin: *const clap_plugin) {
    let events = {
        let mut info = service.info.lock();
        match info.gui.editor.as_mut() {
            // A lost X connection closes the editor instead of the serve loop.
            Some(editor) => editor
                .window
                .poll()
                .unwrap_or_else(|_| vec![WindowEvent::Close]),
            None => Vec::new(),
        }
    };
    for event in events {
        match event {
            WindowEvent::Close => {
                if let Some(editor) = take_editor(service) {
                    drop_view(plugin, true);
                    drop(editor);
                }
            }
            WindowEvent::Resized(width, height) => {
                resize_from_window(service, plugin, width, height)
            }
        }
    }
}
fn resize_from_window(
    service: &Arc<MainService>,
    plugin: *const clap_plugin,
    width: u32,
    height: u32,
) {
    let Some(gui) = (unsafe { plugin_gui(plugin) }) else {
        return;
    };
    let (mut width, mut height) = (width, height);
    let resizable = unsafe { gui.can_resize.map(|f| f(plugin)).unwrap_or(false) };
    unsafe {
        if resizable {
            if let Some(adjust) = gui.adjust_size {
                adjust(plugin, &mut width, &mut height);
            }
            if let Some(set) = gui.set_size {
                set(plugin, width, height);
            }
        } else if let Some(size) = gui.get_size {
            // A fixed-size plugin keeps its own size; put the window back.
            size(plugin, &mut width, &mut height);
        }
    }
    if let Some(editor) = service.info.lock().gui.editor.as_mut() {
        let _ = editor.window.set_size(width, height);
    }
}
fn service_request(service: &Arc<MainService>, plugin: *const clap_plugin) {
    let (resize, visible, closed) = {
        let mut info = service.info.lock();
        (
            info.gui.resize.take(),
            info.gui.visible.take(),
            info.gui.closed.take(),
        )
    };
    if resize.is_none() && visible.is_none() && closed.is_none() {
        return;
    }
    let Some(gui) = (unsafe { plugin_gui(plugin) }) else {
        return;
    };
    if let Some((width, height)) = resize {
        let (mut width, mut height) = (width, height);
        unsafe {
            if let Some(adjust) = gui.adjust_size {
                adjust(plugin, &mut width, &mut height);
            }
            if let Some(set) = gui.set_size {
                set(plugin, width, height);
            }
        }
        if let Some(editor) = service.info.lock().gui.editor.as_mut() {
            let _ = editor.window.set_size(width, height);
        }
    }
    if let Some(visible) = visible {
        if !visible {
            if let Some(hide) = gui.hide {
                unsafe { hide(plugin) };
            }
        } else if service.info.lock().gui.editor.is_none() {
            match create_editor(plugin, &editor_title(service)) {
                Ok(editor) => service.info.lock().gui.editor = Some(editor),
                Err(error) => eprintln!("muz: plugin editor: {error:#}"),
            }
        } else if let Some(show) = gui.show {
            unsafe { show(plugin) };
        }
    }
    if let Some(destroyed) = closed
        && let Some(editor) = take_editor(service)
    {
        // The plugin dropped its view; acknowledge only when it asks for it.
        if destroyed && let Some(destroy) = gui.destroy {
            unsafe { destroy(plugin) };
        }
        drop(editor);
    }
}
/// Fire due host timers and ready descriptors for every live instance.
/// Serve-thread only; registries are snapshotted before any plugin call, because
/// a callback may register timers or descriptors again.
pub fn service_plugin_io() {
    let services = live_services();
    let ready = ready_descriptors(&services);
    for service in services {
        let Some(plugin) = live_plugin(&service) else {
            continue;
        };
        if let Some(timers) = unsafe { plugin_timers(plugin) } {
            // Bound first: a `for` over a temporary guard would hold the timer
            // lock while the plugin runs a callback that may register again.
            let due = service.timers.lock().due(Instant::now());
            for id in due {
                if let Some(on_timer) = timers.on_timer {
                    unsafe { on_timer(plugin, id) };
                }
            }
        }
        let Some(descriptors) = (unsafe { plugin_descriptors(plugin) }) else {
            continue;
        };
        let pending: Vec<(i32, clap_posix_fd_flags)> = {
            let fds = service.fds.lock();
            fds.iter()
                .filter_map(|(fd, flags)| {
                    let ready = ready.get(fd)? & flags;
                    (ready != 0).then_some((*fd, ready))
                })
                .collect()
        };
        for (fd, flags) in pending {
            if let Some(on_fd) = descriptors.on_fd {
                unsafe { on_fd(plugin, fd, flags) };
            }
        }
    }
}
/// Earliest deadline across the host timers of every live instance, so the serve
/// loop can wake in time for the most urgent one.
pub fn next_timer_deadline() -> Option<Instant> {
    live_services()
        .iter()
        .filter(|service| live_plugin(service).is_some())
        .filter_map(|service| service.timers.lock().next_deadline())
        .min()
}
/// Readiness of every registered descriptor, polled once for all instances that
/// share one. CLAP describes the notifications as level-triggered, which is what
/// a zero-timeout `poll` reports.
fn ready_descriptors(services: &[Arc<MainService>]) -> BTreeMap<i32, clap_posix_fd_flags> {
    let mut watched: BTreeMap<i32, clap_posix_fd_flags> = BTreeMap::new();
    for service in services {
        if live_plugin(service).is_none() {
            continue;
        }
        let fds = service.fds.lock();
        for (fd, flags) in fds.iter() {
            watched
                .entry(*fd)
                .and_modify(|known| *known |= *flags)
                .or_insert(*flags);
        }
    }
    let mut poll: Vec<libc::pollfd> = watched
        .iter()
        .map(|(fd, flags)| libc::pollfd {
            fd: *fd,
            events: poll_events(*flags),
            revents: 0,
        })
        .collect();
    if poll.is_empty() {
        return BTreeMap::new();
    }
    let count = unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as libc::nfds_t, 0) };
    if count <= 0 {
        return BTreeMap::new();
    }
    poll.into_iter()
        .filter_map(|entry| {
            let flags = poll_flags(entry.revents);
            (flags != 0).then_some((entry.fd, flags))
        })
        .collect()
}
fn poll_events(flags: clap_posix_fd_flags) -> libc::c_short {
    let mut events = 0;
    if flags & CLAP_POSIX_FD_READ != 0 {
        events |= libc::POLLIN;
    }
    if flags & CLAP_POSIX_FD_WRITE != 0 {
        events |= libc::POLLOUT;
    }
    events
}
fn poll_flags(revents: libc::c_short) -> clap_posix_fd_flags {
    let mut flags = 0;
    if revents & (libc::POLLIN | libc::POLLPRI) != 0 {
        flags |= CLAP_POSIX_FD_READ;
    }
    if revents & libc::POLLOUT != 0 {
        flags |= CLAP_POSIX_FD_WRITE;
    }
    if revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        flags |= CLAP_POSIX_FD_ERROR;
    }
    flags
}
fn create_editor(plugin: *const clap_plugin, title: &str) -> Result<Editor> {
    let gui = unsafe { plugin_gui(plugin) }.context("plugin has no CLAP editor")?;
    let api = CLAP_WINDOW_API_X11.as_ptr();
    ensure!(
        unsafe { gui.is_api_supported.context("missing gui support query")?(plugin, api, false) },
        "plugin does not support an X11 CLAP editor"
    );
    ensure!(
        unsafe { gui.create.context("missing gui create")?(plugin, api, false) },
        "CLAP editor creation failed"
    );
    embed_editor(plugin, gui, title).inspect_err(|_| {
        if let Some(destroy) = gui.destroy {
            unsafe { destroy(plugin) };
        }
    })
}
fn embed_editor(plugin: *const clap_plugin, gui: &clap_plugin_gui, title: &str) -> Result<Editor> {
    let (mut width, mut height) = (0, 0);
    ensure!(
        unsafe { gui.get_size.context("missing gui size")?(plugin, &mut width, &mut height) },
        "CLAP editor has no size"
    );
    let window = EditorWindow::open(title, width, height)?;
    let parent = clap_window {
        api: CLAP_WINDOW_API_X11.as_ptr(),
        specific: clap_window_handle {
            x11: window.id() as clap_xwnd,
        },
    };
    ensure!(
        unsafe { gui.set_parent.context("missing gui set_parent")?(plugin, &parent) },
        "CLAP editor rejected the parent window"
    );
    ensure!(
        unsafe { gui.show.context("missing gui show")?(plugin) },
        "CLAP editor could not be shown"
    );
    Ok(Editor { window })
}
/// Hide and destroy the plugin's view; `hide` is skipped when the plugin already
/// closed it itself.
fn drop_view(plugin: *const clap_plugin, hide_first: bool) {
    let Some(gui) = (unsafe { plugin_gui(plugin) }) else {
        return;
    };
    unsafe {
        if hide_first && let Some(hide) = gui.hide {
            hide(plugin);
        }
        if let Some(destroy) = gui.destroy {
            destroy(plugin);
        }
    }
}
fn take_editor(service: &Arc<MainService>) -> Option<Editor> {
    service.info.lock().gui.editor.take()
}
fn editor_title(service: &Arc<MainService>) -> String {
    let info = service.info.lock();
    match (info.name.is_empty(), info.device.as_ref()) {
        (false, Some(device)) => format!("muz — {} ({device})", info.name),
        (false, None) => format!("muz — {}", info.name),
        (true, Some(device)) => format!("muz — {device}"),
        (true, None) => "muz — plugin editor".to_owned(),
    }
}
impl Drop for PreparedClap {
    fn drop(&mut self) {
        if !self.plugin.is_null() {
            *self.host.service.pointer.lock() = 0;
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
    fn accepts_note_expression(&self, _kind: u8) -> bool {
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
