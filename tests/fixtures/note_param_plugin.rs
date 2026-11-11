//! Synthetic CLAP instrument for per-note parameter tests. Each voice outputs a DC
//! level of (its per-note value, else the global Level) + its per-note modulation,
//! and every received event is logged with its absolute frame.
use clap_sys::{entry::*, events::*, ext::{audio_ports::*, note_ports::*, params::*, state::*}, factory::plugin_factory::*, host::*, plugin::*, process::*, stream::*, version::*};
use std::{ffi::{c_char, c_void, CStr}, ptr, sync::Mutex};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Entry { pub kind: u16, pub frame: u64, pub note_id: i32, pub key: i16, pub param: u32, pub value: f64, pub known: bool }
static LOG: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
/// Move up to `cap` logged events into `out`, oldest first.
#[unsafe(no_mangle)]
pub extern "C" fn drain_log(out: *mut Entry, cap: usize) -> usize {
    let mut log = LOG.lock().unwrap();
    let n = log.len().min(cap);
    for (i, e) in log.drain(..n).enumerate() { unsafe { *out.add(i) = e; } }
    n
}

static DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION, id: c"muz.test.note-param".as_ptr(), name: c"Note parameter probe".as_ptr(),
    vendor: c"muz test".as_ptr(), url: ptr::null(), manual_url: ptr::null(), support_url: ptr::null(),
    version: c"1".as_ptr(), description: c"Per-note parameter probe".as_ptr(), features: ptr::null(),
};
const NOTE_ID: u32 = CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_AUTOMATABLE_PER_NOTE_ID | CLAP_PARAM_IS_MODULATABLE | CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID;
// id, name, flags. Level is fully per-note; Tone is global only; Keyed is per key
// only; Pitch (like Surge XT's) is modulatable but not settable per note ID.
const PARAMS: [(u32, &CStr, u32); 4] = [
    (1, c"Level", NOTE_ID),
    (2, c"Tone", CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_MODULATABLE),
    (3, c"Keyed", CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_AUTOMATABLE_PER_KEY | CLAP_PARAM_IS_MODULATABLE | CLAP_PARAM_IS_MODULATABLE_PER_KEY),
    (4, c"Pitch", CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_MODULATABLE | CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID | CLAP_PARAM_IS_MODULATABLE_PER_KEY | CLAP_PARAM_IS_MODULATABLE_PER_CHANNEL | CLAP_PARAM_IS_MODULATABLE_PER_PORT),
];
struct Voice { note_id: i32, key: i16, value: Option<f64>, modulation: f64 }
struct Instance { plugin: clap_plugin, level: f64, voices: Vec<Voice> }
unsafe fn instance(plugin: *const clap_plugin) -> &'static mut Instance { unsafe { &mut *((*plugin).plugin_data as *mut Instance) } }
unsafe extern "C" fn initialize(_: *const clap_plugin) -> bool { true }
unsafe extern "C" fn destroy(plugin: *const clap_plugin) { drop(unsafe { Box::from_raw((*plugin).plugin_data as *mut Instance) }); }
unsafe extern "C" fn activate(_: *const clap_plugin, rate: f64, _: u32, _: u32) -> bool { rate.is_finite() && rate > 0.0 }
unsafe extern "C" fn deactivate(_: *const clap_plugin) {}
unsafe extern "C" fn start(_: *const clap_plugin) -> bool { true }
unsafe extern "C" fn stop(_: *const clap_plugin) {}
unsafe extern "C" fn reset(plugin: *const clap_plugin) { unsafe { instance(plugin).voices.clear(); } }
fn apply(state: &mut Instance, header: *const clap_event_header, start: u64) {
    let h = unsafe { &*header };
    if h.space_id != CLAP_CORE_EVENT_SPACE_ID { return; }
    let frame = start + h.time as u64;
    let mut entry = Entry { kind: h.type_, frame, ..Default::default() };
    match h.type_ {
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF | CLAP_EVENT_NOTE_CHOKE => {
            let e = unsafe { &*(header as *const clap_event_note) };
            (entry.note_id, entry.key, entry.value) = (e.note_id, e.key, e.velocity);
            state.voices.retain(|v| v.note_id != e.note_id);
            if h.type_ == CLAP_EVENT_NOTE_ON {
                state.voices.push(Voice { note_id: e.note_id, key: e.key, value: None, modulation: 0.0 });
            }
            entry.known = true;
        }
        CLAP_EVENT_PARAM_VALUE | CLAP_EVENT_PARAM_MOD => {
            let (param, note_id, key, value) = if h.type_ == CLAP_EVENT_PARAM_VALUE {
                let e = unsafe { &*(header as *const clap_event_param_value) };
                (e.param_id, e.note_id, e.key, e.value)
            } else {
                let e = unsafe { &*(header as *const clap_event_param_mod) };
                (e.param_id, e.note_id, e.key, e.amount)
            };
            (entry.param, entry.note_id, entry.key, entry.value) = (param, note_id, key, value);
            if note_id < 0 {
                entry.known = true;
                if param == 1 && h.type_ == CLAP_EVENT_PARAM_VALUE { state.level = value; }
            } else if let Some(v) = state.voices.iter_mut().find(|v| v.note_id == note_id && (key < 0 || v.key == key)) {
                entry.known = true;
                if param == 1 {
                    if h.type_ == CLAP_EVENT_PARAM_VALUE { v.value = Some(value); } else { v.modulation = value; }
                }
            }
        }
        CLAP_EVENT_NOTE_EXPRESSION => {
            let e = unsafe { &*(header as *const clap_event_note_expression) };
            (entry.param, entry.note_id, entry.key, entry.value) =
                (e.expression_id as u32, e.note_id, e.key, e.value);
            entry.known = state.voices.iter().any(|v| v.note_id == e.note_id);
        }
        _ => return,
    }
    LOG.lock().unwrap().push(entry);
}
unsafe extern "C" fn process(plugin: *const clap_plugin, process: *const clap_process) -> clap_process_status {
    let state = unsafe { instance(plugin) };
    let process = unsafe { &*process };
    if process.audio_outputs_count == 0 { return CLAP_PROCESS_ERROR; }
    let start = process.steady_time.max(0) as u64;
    let events = unsafe { &*process.in_events };
    let count = unsafe { events.size.unwrap()(events) };
    let mut next = 0;
    let output = unsafe { &mut *process.audio_outputs };
    for index in 0..process.frames_count {
        while next < count {
            let header = unsafe { events.get.unwrap()(events, next) };
            if unsafe { (*header).time } > index { break; }
            apply(state, header, start);
            next += 1;
        }
        let sample: f64 = state.voices.iter().map(|v| v.value.unwrap_or(state.level) + v.modulation).sum();
        for channel in 0..output.channel_count as usize {
            unsafe { *(*output.data32.add(channel)).add(index as usize) = sample as f32; }
        }
    }
    while next < count { apply(state, unsafe { events.get.unwrap()(events, next) }, start); next += 1; }
    CLAP_PROCESS_CONTINUE
}
unsafe extern "C" fn count_ports(_: *const clap_plugin, input: bool) -> u32 { u32::from(!input) }
unsafe extern "C" fn get_port(_: *const clap_plugin, index: u32, input: bool, info: *mut clap_audio_port_info) -> bool {
    if input || index != 0 || info.is_null() { return false; }
    let info = unsafe { &mut *info };
    info.id = 1; info.name.fill(0); info.flags = CLAP_AUDIO_PORT_IS_MAIN; info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr(); info.in_place_pair = u32::MAX;
    true
}
static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports { count: Some(count_ports), get: Some(get_port) };
unsafe extern "C" fn count_notes(_: *const clap_plugin, input: bool) -> u32 { u32::from(input) }
unsafe extern "C" fn get_notes(_: *const clap_plugin, index: u32, input: bool, info: *mut clap_note_port_info) -> bool {
    if !input || index != 0 || info.is_null() { return false; }
    let info = unsafe { &mut *info };
    info.id = 1; info.name.fill(0); info.supported_dialects = CLAP_NOTE_DIALECT_CLAP; info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    true
}
static NOTES: clap_plugin_note_ports = clap_plugin_note_ports { count: Some(count_notes), get: Some(get_notes) };
unsafe extern "C" fn param_count(_: *const clap_plugin) -> u32 { PARAMS.len() as u32 }
unsafe extern "C" fn param_info(_: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool {
    let Some((id, name, flags)) = PARAMS.get(index as usize) else { return false };
    let info = unsafe { &mut *info };
    info.id = *id; info.flags = *flags; info.cookie = ptr::null_mut();
    info.name.fill(0); info.module.fill(0);
    for (i, b) in name.to_bytes().iter().enumerate() { info.name[i] = *b as c_char; }
    info.min_value = 0.0; info.max_value = 1.0; info.default_value = 0.5;
    true
}
unsafe extern "C" fn param_value(plugin: *const clap_plugin, id: u32, out: *mut f64) -> bool {
    if !PARAMS.iter().any(|p| p.0 == id) { return false; }
    unsafe { *out = if id == 1 { instance(plugin).level } else { 0.5 }; }
    true
}
unsafe extern "C" fn param_flush(plugin: *const clap_plugin, events: *const clap_input_events, _: *const clap_output_events) {
    let state = unsafe { instance(plugin) };
    let events = unsafe { &*events };
    for i in 0..unsafe { events.size.unwrap()(events) } { apply(state, unsafe { events.get.unwrap()(events, i) }, 0); }
}
static PARAMS_EXT: clap_plugin_params = clap_plugin_params {
    count: Some(param_count), get_info: Some(param_info), get_value: Some(param_value),
    value_to_text: None, text_to_value: None, flush: Some(param_flush),
};
unsafe extern "C" fn save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let bytes = unsafe { instance(plugin).level }.to_le_bytes();
    unsafe { (*stream).write.unwrap()(stream, bytes.as_ptr().cast(), 8) == 8 }
}
unsafe extern "C" fn load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    let mut bytes = [0u8; 8];
    if unsafe { (*stream).read.unwrap()(stream, bytes.as_mut_ptr().cast(), 8) } != 8 { return false; }
    unsafe { instance(plugin).level = f64::from_le_bytes(bytes); }
    true
}
static STATE: clap_plugin_state = clap_plugin_state { save: Some(save), load: Some(load) };
unsafe extern "C" fn extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_AUDIO_PORTS { &PORTS as *const _ as *const c_void }
    else if id == CLAP_EXT_NOTE_PORTS { &NOTES as *const _ as *const c_void }
    else if id == CLAP_EXT_PARAMS { &PARAMS_EXT as *const _ as *const c_void }
    else if id == CLAP_EXT_STATE { &STATE as *const _ as *const c_void }
    else { ptr::null() }
}
unsafe extern "C" fn main_thread(_: *const clap_plugin) {}
unsafe extern "C" fn count(_: *const clap_plugin_factory) -> u32 { 1 }
unsafe extern "C" fn descriptor(_: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor {
    if index == 0 { &DESCRIPTOR } else { ptr::null() }
}
unsafe extern "C" fn create(_: *const clap_plugin_factory, host: *const clap_host, id: *const c_char) -> *const clap_plugin {
    if host.is_null() || unsafe { CStr::from_ptr(id) } != c"muz.test.note-param" { return ptr::null(); }
    let mut state = Box::new(Instance {
        plugin: clap_plugin { desc: &DESCRIPTOR, plugin_data: ptr::null_mut(), init: Some(initialize), destroy: Some(destroy),
            activate: Some(activate), deactivate: Some(deactivate), start_processing: Some(start), stop_processing: Some(stop),
            reset: Some(reset), process: Some(process), get_extension: Some(extension), on_main_thread: Some(main_thread) },
        level: 0.5, voices: Vec::new(),
    });
    state.plugin.plugin_data = (&mut *state as *mut Instance).cast();
    let plugin = &state.plugin as *const _;
    let _ = Box::into_raw(state);
    plugin
}
static FACTORY: clap_plugin_factory = clap_plugin_factory { get_plugin_count: Some(count), get_plugin_descriptor: Some(descriptor), create_plugin: Some(create) };
unsafe extern "C" fn entry_init(_: *const c_char) -> bool { true }
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn factory(id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID { &FACTORY as *const _ as *const c_void } else { ptr::null() }
}
#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry { clap_version: CLAP_VERSION, init: Some(entry_init), deinit: Some(entry_deinit), get_factory: Some(factory) };
