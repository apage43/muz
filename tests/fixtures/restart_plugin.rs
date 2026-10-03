use clap_sys::{entry::*, ext::audio_ports::*, factory::plugin_factory::*, host::*, plugin::*, process::*, version::*};
use std::{ffi::{c_char, c_void, CStr}, ptr, sync::atomic::{AtomicBool, Ordering}};

static ARMED: AtomicBool = AtomicBool::new(false);
#[unsafe(no_mangle)]
pub extern "C" fn arm_restart() { ARMED.store(true, Ordering::Release); }

static DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION, id: c"muz.test.restart".as_ptr(), name: c"Restart tone".as_ptr(),
    vendor: c"muz test".as_ptr(), url: ptr::null(), manual_url: ptr::null(), support_url: ptr::null(),
    version: c"1".as_ptr(), description: c"Synthetic oscillator with controlled host restart".as_ptr(), features: ptr::null(),
};
struct Instance { plugin: clap_plugin, host: *const clap_host, phase: f64, step: f64 }
unsafe fn instance(plugin: *const clap_plugin) -> &'static mut Instance { unsafe { &mut *((*plugin).plugin_data as *mut Instance) } }
unsafe extern "C" fn initialize(_: *const clap_plugin) -> bool { true }
unsafe extern "C" fn destroy(plugin: *const clap_plugin) { drop(unsafe { Box::from_raw((*plugin).plugin_data as *mut Instance) }); }
unsafe extern "C" fn activate(plugin: *const clap_plugin, rate: f64, _: u32, _: u32) -> bool {
    if !rate.is_finite() || rate <= 0.0 { return false; }
    unsafe { instance(plugin).step = 440.0 / rate; }
    true
}
unsafe extern "C" fn deactivate(_: *const clap_plugin) {}
unsafe extern "C" fn start(_: *const clap_plugin) -> bool { true }
unsafe extern "C" fn stop(_: *const clap_plugin) {}
unsafe extern "C" fn reset(plugin: *const clap_plugin) { unsafe { instance(plugin).phase = 0.0; } }
unsafe extern "C" fn process(plugin: *const clap_plugin, process: *const clap_process) -> clap_process_status {
    let state = unsafe { instance(plugin) };
    let process = unsafe { &*process };
    if ARMED.swap(false, Ordering::AcqRel) {
        if let Some(restart) = unsafe { (*state.host).request_restart } { unsafe { restart(state.host); } }
    }
    if process.audio_outputs_count == 0 { return CLAP_PROCESS_ERROR; }
    let output = unsafe { &mut *process.audio_outputs };
    for index in 0..process.frames_count as usize {
        let sample = (state.phase * std::f64::consts::TAU).sin() as f32 * 0.1;
        state.phase = (state.phase + state.step).fract();
        for channel in 0..output.channel_count as usize {
            unsafe { *(*output.data32.add(channel)).add(index) = sample; }
        }
    }
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
unsafe extern "C" fn extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_EXT_AUDIO_PORTS { &PORTS as *const _ as *const c_void } else { ptr::null() }
}
unsafe extern "C" fn main_thread(_: *const clap_plugin) {}
unsafe extern "C" fn count(_: *const clap_plugin_factory) -> u32 { 1 }
unsafe extern "C" fn descriptor(_: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor {
    if index == 0 { &DESCRIPTOR } else { ptr::null() }
}
unsafe extern "C" fn create(_: *const clap_plugin_factory, host: *const clap_host, id: *const c_char) -> *const clap_plugin {
    if host.is_null() || unsafe { CStr::from_ptr(id) } != c"muz.test.restart" { return ptr::null(); }
    let mut state = Box::new(Instance {
        plugin: clap_plugin { desc: &DESCRIPTOR, plugin_data: ptr::null_mut(), init: Some(initialize), destroy: Some(destroy),
            activate: Some(activate), deactivate: Some(deactivate), start_processing: Some(start), stop_processing: Some(stop),
            reset: Some(reset), process: Some(process), get_extension: Some(extension), on_main_thread: Some(main_thread) },
        host, phase: 0.0, step: 0.0,
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
