//! Synthetic CLAP tone/effect that checks the host's threading and sleep contracts.
//!
//! Every class has one stereo input and output: `out = in / 2 + tone + hum + echo`. A MIDI
//! note-on starts the tone and requests a main-thread callback; each callback raises the tone
//! level, read per frame, so output depends on when the host services it. Classes differ in
//! the status `process` returns, the hum, and the echo (see `KINDS`).
use clap_sys::{
    entry::*, events::*, ext::audio_ports::*, ext::tail::*, ext::thread_check::*,
    factory::plugin_factory::*, host::*, plugin::*, process::*, version::*,
};
use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

const ECHO: usize = 1500;
static VIOLATIONS: AtomicU32 = AtomicU32::new(0);
static MAIN_CALLS: AtomicU32 = AtomicU32::new(0);
static BLOCK_ARMED: AtomicBool = AtomicBool::new(false);

/// Contract breaches seen by any instance: wrong thread role, overlapping or unstarted process.
#[unsafe(no_mangle)]
pub extern "C" fn thread_violations() -> u32 {
    VIOLATIONS.load(Ordering::Acquire)
}
#[unsafe(no_mangle)]
pub extern "C" fn main_thread_calls() -> u32 {
    MAIN_CALLS.load(Ordering::Acquire)
}
/// Make every `muz.test.block` instance wait for its main thread in its next process call.
#[unsafe(no_mangle)]
pub extern "C" fn arm_block() {
    BLOCK_ARMED.store(true, Ordering::Release);
}
fn violation() {
    VIOLATIONS.fetch_add(1, Ordering::AcqRel);
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Continue,
    /// `CLAP_PROCESS_SLEEP` whenever no tone sounds and the input block is silent.
    Sleep,
    IfNotQuiet,
    Tail,
}
struct Kind {
    id: &'static CStr,
    status: Status,
    hum: f32,
    echo: bool,
    /// Wait inside `process` for a main-thread callback once armed.
    block: bool,
}
const fn kind(id: &'static CStr, status: Status, hum: f32, echo: bool, block: bool) -> Kind {
    Kind { id, status, hum, echo, block }
}
const IDS: [&CStr; 7] = [
    c"muz.test.tone",
    c"muz.test.hum",
    c"muz.test.sleep",
    c"muz.test.quiet",
    c"muz.test.echo",
    c"muz.test.tail",
    c"muz.test.block",
];
static KINDS: [Kind; 7] = [
    kind(IDS[0], Status::Continue, 0.0, false, false),
    kind(IDS[1], Status::Continue, 1e-4, false, false),
    kind(IDS[2], Status::Sleep, 0.0, false, false),
    kind(IDS[3], Status::IfNotQuiet, 0.0, false, false),
    kind(IDS[4], Status::Continue, 0.0, true, false),
    kind(IDS[5], Status::Tail, 0.0, true, false),
    kind(IDS[6], Status::Continue, 0.0, false, true),
];
struct Descriptors([clap_plugin_descriptor; 7]);
unsafe impl Sync for Descriptors {}
const fn descriptor_for(id: &'static CStr) -> clap_plugin_descriptor {
    clap_plugin_descriptor {
        clap_version: CLAP_VERSION,
        id: id.as_ptr(),
        name: c"Thread tone".as_ptr(),
        vendor: c"muz test".as_ptr(),
        url: ptr::null(),
        manual_url: ptr::null(),
        support_url: ptr::null(),
        version: c"1".as_ptr(),
        description: c"Synthetic tone checking host threading".as_ptr(),
        features: ptr::null(),
    }
}
static DESCRIPTORS: Descriptors = Descriptors([
    descriptor_for(IDS[0]),
    descriptor_for(IDS[1]),
    descriptor_for(IDS[2]),
    descriptor_for(IDS[3]),
    descriptor_for(IDS[4]),
    descriptor_for(IDS[5]),
    descriptor_for(IDS[6]),
]);

struct Instance {
    plugin: clap_plugin,
    host: *const clap_host,
    check: *const clap_host_thread_check,
    kind: &'static Kind,
    rate: f64,
    phase: f64,
    step: f64,
    gate: bool,
    processing: bool,
    blocked: bool,
    busy: AtomicBool,
    level: AtomicU32,
    callbacks: AtomicU32,
    delay: Vec<[f32; 2]>,
    position: usize,
}
unsafe fn instance(plugin: *const clap_plugin) -> &'static mut Instance {
    unsafe { &mut *((*plugin).plugin_data as *mut Instance) }
}
/// Record a violation unless the host reports exactly the expected thread role.
unsafe fn expect_role(state: &Instance, audio: bool) {
    let Some(check) = (unsafe { state.check.as_ref() }) else {
        return violation();
    };
    let is_main = unsafe { check.is_main_thread.unwrap()(state.host) };
    let is_audio = unsafe { check.is_audio_thread.unwrap()(state.host) };
    if is_audio != audio || is_main == audio {
        violation();
    }
}
unsafe extern "C" fn initialize(plugin: *const clap_plugin) -> bool {
    let state = unsafe { instance(plugin) };
    state.check = unsafe {
        (*state.host).get_extension.unwrap()(state.host, CLAP_EXT_THREAD_CHECK.as_ptr())
    }
    .cast();
    unsafe { expect_role(state, false) };
    true
}
unsafe extern "C" fn destroy(plugin: *const clap_plugin) {
    unsafe { expect_role(instance(plugin), false) };
    drop(unsafe { Box::from_raw((*plugin).plugin_data as *mut Instance) });
}
unsafe extern "C" fn activate(plugin: *const clap_plugin, rate: f64, _: u32, _: u32) -> bool {
    let state = unsafe { instance(plugin) };
    unsafe { expect_role(state, false) };
    state.rate = rate;
    rate.is_finite() && rate > 0.0
}
unsafe extern "C" fn deactivate(plugin: *const clap_plugin) {
    unsafe { expect_role(instance(plugin), false) };
}
unsafe extern "C" fn start(plugin: *const clap_plugin) -> bool {
    let state = unsafe { instance(plugin) };
    unsafe { expect_role(state, true) };
    if std::mem::replace(&mut state.processing, true) {
        violation();
    }
    true
}
unsafe extern "C" fn stop(plugin: *const clap_plugin) {
    let state = unsafe { instance(plugin) };
    unsafe { expect_role(state, true) };
    if !std::mem::replace(&mut state.processing, false) {
        violation();
    }
}
unsafe extern "C" fn reset(plugin: *const clap_plugin) {
    let state = unsafe { instance(plugin) };
    unsafe { expect_role(state, true) };
    state.phase = 0.0;
    state.gate = false;
    state.delay.fill([0.0; 2]);
}
unsafe extern "C" fn process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    let state = unsafe { instance(plugin) };
    let process = unsafe { &*process };
    if state.busy.swap(true, Ordering::AcqRel) || !state.processing {
        violation();
    }
    unsafe { expect_role(state, true) };
    if process.audio_outputs_count != 1 || process.audio_inputs_count != 1 {
        return CLAP_PROCESS_ERROR;
    }
    if state.kind.block && !state.blocked && BLOCK_ARMED.load(Ordering::Acquire) {
        state.blocked = true;
        let before = state.callbacks.load(Ordering::Acquire);
        unsafe { (*state.host).request_callback.unwrap()(state.host) };
        let started = std::time::Instant::now();
        while state.callbacks.load(Ordering::Acquire) == before {
            if started.elapsed() > std::time::Duration::from_secs(30) {
                state.busy.store(false, Ordering::Release);
                return CLAP_PROCESS_ERROR;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    let input = unsafe { &*process.audio_inputs };
    let output = unsafe { &mut *process.audio_outputs };
    let events = unsafe { &*process.in_events };
    let count = unsafe { events.size.unwrap()(events) };
    let mut next = 0;
    let mut silent_input = true;
    for frame in 0..process.frames_count {
        while next < count {
            let header = unsafe { &*events.get.unwrap()(events, next) };
            if header.time > frame {
                break;
            }
            next += 1;
            if header.type_ != CLAP_EVENT_MIDI {
                continue;
            }
            let data = unsafe { &*(header as *const clap_event_header).cast::<clap_event_midi>() }.data;
            match data[0] & 0xf0 {
                0x90 if data[2] > 0 => {
                    state.gate = true;
                    state.step = 440.0 * 2f64.powf((data[1] as f64 - 69.0) / 12.0) / state.rate;
                    unsafe { (*state.host).request_callback.unwrap()(state.host) };
                }
                0x80 | 0x90 => state.gate = false,
                _ => {}
            }
        }
        let index = frame as usize;
        let level = f32::from_bits(state.level.load(Ordering::Acquire));
        let tone = if state.gate {
            let sample = (state.phase * std::f64::consts::TAU).sin() as f32 * level;
            state.phase = (state.phase + state.step).fract();
            sample
        } else {
            0.0
        };
        let dry = [0, 1].map(|channel| unsafe { *(*input.data32.add(channel)).add(index) });
        silent_input &= dry == [0.0; 2];
        let mut wet = dry.map(|sample| sample * 0.5 + tone + state.kind.hum);
        if state.kind.echo {
            let slot = &mut state.delay[state.position];
            wet[0] += slot[0];
            wet[1] += slot[1];
            *slot = dry;
            state.position = (state.position + 1) % ECHO;
        }
        for channel in 0..2 {
            unsafe { *(*output.data32.add(channel)).add(index) = wet[channel] };
        }
    }
    state.busy.store(false, Ordering::Release);
    match state.kind.status {
        Status::Continue => CLAP_PROCESS_CONTINUE,
        Status::Sleep if !state.gate && silent_input => CLAP_PROCESS_SLEEP,
        Status::Sleep => CLAP_PROCESS_CONTINUE,
        Status::IfNotQuiet => CLAP_PROCESS_CONTINUE_IF_NOT_QUIET,
        Status::Tail => CLAP_PROCESS_TAIL,
    }
}
unsafe extern "C" fn main_thread(plugin: *const clap_plugin) {
    let state = unsafe { instance(plugin) };
    unsafe { expect_role(state, false) };
    MAIN_CALLS.fetch_add(1, Ordering::AcqRel);
    let level = f32::from_bits(state.level.load(Ordering::Acquire));
    state.level.store((level + 0.01).to_bits(), Ordering::Release);
    state.callbacks.fetch_add(1, Ordering::AcqRel);
}
unsafe extern "C" fn count_ports(_: *const clap_plugin, _: bool) -> u32 {
    1
}
unsafe extern "C" fn get_port(
    _: *const clap_plugin,
    index: u32,
    _: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if index != 0 || info.is_null() {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = 1;
    info.name.fill(0);
    info.flags = CLAP_AUDIO_PORT_IS_MAIN;
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = u32::MAX;
    true
}
static PORTS: clap_plugin_audio_ports =
    clap_plugin_audio_ports { count: Some(count_ports), get: Some(get_port) };
unsafe extern "C" fn tail(_: *const clap_plugin) -> u32 {
    ECHO as u32
}
static TAIL: clap_plugin_tail = clap_plugin_tail { get: Some(tail) };
unsafe extern "C" fn extension(plugin: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_TAIL && unsafe { instance(plugin) }.kind.status == Status::Tail {
        &TAIL as *const _ as *const c_void
    } else {
        ptr::null()
    }
}
unsafe extern "C" fn count(_: *const clap_plugin_factory) -> u32 {
    KINDS.len() as u32
}
unsafe extern "C" fn descriptor(
    _: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    DESCRIPTORS.0.get(index as usize).map_or(ptr::null(), |d| d as *const _)
}
unsafe extern "C" fn create(
    _: *const clap_plugin_factory,
    host: *const clap_host,
    id: *const c_char,
) -> *const clap_plugin {
    let id = unsafe { CStr::from_ptr(id) };
    let Some(index) = KINDS.iter().position(|kind| kind.id == id) else {
        return ptr::null();
    };
    if host.is_null() {
        return ptr::null();
    }
    let mut state = Box::new(Instance {
        plugin: clap_plugin {
            desc: &DESCRIPTORS.0[index],
            plugin_data: ptr::null_mut(),
            init: Some(initialize),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(deactivate),
            start_processing: Some(start),
            stop_processing: Some(stop),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(extension),
            on_main_thread: Some(main_thread),
        },
        host,
        check: ptr::null(),
        kind: &KINDS[index],
        rate: 48000.0,
        phase: 0.0,
        step: 0.0,
        gate: false,
        processing: false,
        blocked: false,
        busy: AtomicBool::new(false),
        level: AtomicU32::new(0.1f32.to_bits()),
        callbacks: AtomicU32::new(0),
        delay: vec![[0.0; 2]; ECHO],
        position: 0,
    });
    state.plugin.plugin_data = (&mut *state as *mut Instance).cast();
    let plugin = &state.plugin as *const _;
    let _ = Box::into_raw(state);
    plugin
}
static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(count),
    get_plugin_descriptor: Some(descriptor),
    create_plugin: Some(create),
};
unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
    true
}
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn factory(id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const _ as *const c_void
    } else {
        ptr::null()
    }
}
#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(factory),
};
