//! CLAP entry points for standalone native Muz instruments and effects.
use clap_sys::{
    entry::clap_plugin_entry,
    events::*,
    ext::{audio_ports::*, latency::*, note_ports::*, params::*, render::*, state::*, tail::*},
    factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory},
    host::clap_host,
    id::CLAP_INVALID_ID,
    plugin::{clap_plugin, clap_plugin_descriptor},
    plugin_features::{CLAP_PLUGIN_FEATURE_AUDIO_EFFECT, CLAP_PLUGIN_FEATURE_INSTRUMENT},
    process::*,
    stream::*,
    version::CLAP_VERSION,
};
use muz::{
    audio::{self, AudioConfig, DeviceEvent, DeviceEventKind, DeviceProcessor, ProcessContext},
    device_state::{DeviceRole, DeviceState, FX_ID, INSTRUMENT_ID},
    model,
};
use std::{
    collections::BTreeMap,
    ffi::{CStr, c_char, c_void},
    ptr, slice,
    sync::{
        Arc, Mutex, MutexGuard, RwLock,
        atomic::{AtomicU32, Ordering},
    },
};

const INSTRUMENT_ID_C: &[u8] = b"com.plausiblyreliable.muz.instrument\0";
const FX_ID_C: &[u8] = b"com.plausiblyreliable.muz.fx\0";
const INSTRUMENT_NAME: &[u8] = b"Muz Instrument\0";
const FX_NAME: &[u8] = b"Muz FX\0";
const VENDOR: &[u8] = b"Muz\0";
const VERSION: &[u8] = b"0.1.0\0";
const DESCRIPTION: &[u8] = b"Native Muz processor with editable source state\0";
struct Ptrs([*const c_char; 2]);
unsafe impl Sync for Ptrs {}
static INSTRUMENT_FEATURES: Ptrs = Ptrs([CLAP_PLUGIN_FEATURE_INSTRUMENT.as_ptr(), ptr::null()]);
static FX_FEATURES: Ptrs = Ptrs([CLAP_PLUGIN_FEATURE_AUDIO_EFFECT.as_ptr(), ptr::null()]);
static INSTRUMENT_DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: INSTRUMENT_ID_C.as_ptr().cast(),
    name: INSTRUMENT_NAME.as_ptr().cast(),
    vendor: VENDOR.as_ptr().cast(),
    url: ptr::null(),
    manual_url: ptr::null(),
    support_url: ptr::null(),
    version: VERSION.as_ptr().cast(),
    description: DESCRIPTION.as_ptr().cast(),
    features: INSTRUMENT_FEATURES.0.as_ptr(),
};
static FX_DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: FX_ID_C.as_ptr().cast(),
    name: FX_NAME.as_ptr().cast(),
    vendor: VENDOR.as_ptr().cast(),
    url: ptr::null(),
    manual_url: ptr::null(),
    support_url: ptr::null(),
    version: VERSION.as_ptr().cast(),
    description: DESCRIPTION.as_ptr().cast(),
    features: FX_FEATURES.0.as_ptr(),
};

struct Instance {
    plugin: clap_plugin,
    inner: Mutex<Inner>,
    view: RwLock<Option<Arc<StateView>>>,
    role: DeviceRole,
    latency: AtomicU32,
    tail: AtomicU32,
}
struct StateView {
    state: Arc<DeviceState>,
    ranges: Vec<ParamRange>,
    values: Vec<AtomicU32>,
}
impl StateView {
    fn new(state: Arc<DeviceState>, ranges: Vec<ParamRange>) -> Self {
        let values = state
            .parameters
            .iter()
            .map(|p| AtomicU32::new(p.value.to_bits()))
            .collect();
        Self {
            state,
            ranges,
            values,
        }
    }
    fn set(&self, index: usize, value: f32) {
        self.values[index].store(value.to_bits(), Ordering::Relaxed);
    }
    fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }
}
#[derive(Clone, Copy)]
struct TrackedNote {
    host_id: i32,
    internal_id: u64,
    port_index: i16,
    channel: u8,
    key: u8,
    midi: bool,
    released: bool,
    choked: bool,
    ended: bool,
}
struct Inner {
    #[cfg(test)]
    panic_next_process: bool,
    host: *const clap_host,
    role: DeviceRole,
    state: Option<Arc<DeviceState>>,
    current_values: Vec<f32>,
    ranges: Vec<ParamRange>,
    view: Option<Arc<StateView>>,
    processor: Option<Box<dyn DeviceProcessor>>,
    active: bool,
    ever_activated: bool,
    loaded_external_state: bool,
    processing: bool,
    config: AudioConfig,
    left: Vec<f32>,
    right: Vec<f32>,
    events: Vec<DeviceEvent>,
    param_events: Vec<(u32, u32, f32)>,
    segment_events: Vec<DeviceEvent>,
    frame_position: u64,
    offline: bool,
    overflow: bool,
    active_notes: Vec<TrackedNote>,
    next_note_id: u64,
}

#[derive(Clone, Copy)]
struct ParamRange {
    min: f32,
    max: f32,
    default: f32,
}
impl ParamRange {
    fn accepts(self, value: f32) -> bool {
        value.is_finite() && (self.min..=self.max).contains(&value)
    }
}
fn parameter_ranges(state: &DeviceState) -> Option<Vec<ParamRange>> {
    let patch = if state.device.kind == muz::model::DeviceKind::VoicePatch {
        Some(muz::patch_description::ValidatedPatch::from_json(state.device.patch.as_ref()?).ok()?)
    } else {
        None
    };
    state
        .parameters
        .iter()
        .map(|param| {
            if let Some(patch) = &patch {
                let (min, max) = patch.control_range(&param.path)?;
                Some(ParamRange {
                    min,
                    max,
                    default: patch
                        .controls()
                        .get(&param.path)
                        .copied()
                        .unwrap_or(param.value),
                })
            } else if state.device.kind == muz::model::DeviceKind::Sfz {
                let cc = param.path.strip_prefix("cc")?.parse::<u16>().ok()?;
                if cc >= 128 {
                    return None;
                }
                Some(ParamRange {
                    min: 0.,
                    max: 127.,
                    default: state.device.sfz.as_ref()?.controller_default(cc),
                })
            } else if state.device.kind == muz::model::DeviceKind::Rack {
                let (min, max, default) =
                    muz::device_state::control_range(&state.device, &param.path)?;
                Some(ParamRange { min, max, default })
            } else {
                let spec = muz::description::parameter_specs(state.device.kind)
                    .iter()
                    .find(|s| s.name == param.path)?;
                Some(ParamRange {
                    min: spec.min,
                    max: spec.max,
                    default: spec.default,
                })
            }
        })
        .collect()
}
fn matches_note_address(
    note_id: i32,
    port_index: i16,
    channel: i16,
    key: i16,
    active: TrackedNote,
) -> bool {
    !active.ended
        && (note_id < 0 || active.host_id == note_id)
        && (port_index < 0 || active.port_index == port_index)
        && (channel < 0 || active.channel == channel as u8)
        && (key < 0 || active.key == key as u8)
}

unsafe fn push_note_end(raw: &clap_process, note: TrackedNote, time: u32) -> bool {
    let Some(out) = (unsafe { raw.out_events.as_ref() }) else {
        return false;
    };
    let Some(push) = out.try_push else {
        return false;
    };
    let event = clap_event_note {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_note>() as u32,
            time,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_NOTE_END,
            flags: 0,
        },
        note_id: note.host_id,
        port_index: note.port_index,
        channel: note.channel as i16,
        key: note.key as i16,
        velocity: 0.0,
    };
    unsafe { push(out, &event.header) }
}

fn midi_event_kind(bytes: [u8; 3]) -> Option<DeviceEventKind> {
    if bytes[0] & 0xf0 == 0xb0 {
        if bytes[1] > 127 || bytes[2] > 127 {
            return None;
        }
        Some(DeviceEventKind::Controller {
            channel: bytes[0] & 0x0f,
            controller: bytes[1],
            value: bytes[2],
        })
    } else {
        Some(DeviceEventKind::Midi { bytes, len: 3 })
    }
}

fn push_midi_note(p: &mut Inner, bytes: [u8; 3], offset: u32) -> bool {
    let channel = bytes[0] & 0x0f;
    let key = bytes[1];
    let velocity = bytes[2];
    if key > 127 || velocity > 127 || p.events.len() == audio::MAX_EVENTS_PER_BLOCK {
        return false;
    }
    if bytes[0] & 0xf0 == 0x90 && velocity != 0 {
        if p.active_notes.len() == audio::MAX_ACTIVE_NOTES {
            return false;
        }
        p.next_note_id = p.next_note_id.wrapping_add(1);
        let note_id = p.next_note_id;
        p.active_notes.push(TrackedNote {
            host_id: -1,
            internal_id: note_id,
            port_index: 0,
            channel,
            key,
            midi: true,
            released: false,
            choked: false,
            ended: false,
        });
        p.events.push(DeviceEvent {
            offset,
            kind: DeviceEventKind::NoteOn {
                sample_zone: None,
                pitch: key as f32,
                elapsed_frames: 0,
                note_id,
                channel,
                key,
                velocity: velocity as f32 / 127.0,
            },
        });
    } else if let Some((index, note)) = p
        .active_notes
        .iter()
        .enumerate()
        .filter(|(_, note)| {
            note.midi && !note.released && note.channel == channel && note.key == key
        })
        .min_by_key(|(_, note)| note.internal_id)
    {
        let note_id = note.internal_id;
        p.active_notes[index].released = true;
        p.events.push(DeviceEvent {
            offset,
            kind: DeviceEventKind::NoteOff {
                note_id,
                channel,
                key,
                velocity: velocity as f32 / 127.0,
            },
        });
    }
    true
}

unsafe fn plugin_instance<'a>(plugin: *const clap_plugin) -> Option<&'a Instance> {
    if plugin.is_null() {
        return None;
    }
    unsafe { ((*plugin).plugin_data as *const Instance).as_ref() }
}

unsafe fn instance<'a>(plugin: *const clap_plugin) -> Option<MutexGuard<'a, Inner>> {
    let instance = unsafe { plugin_instance(plugin) }?;
    instance.inner.try_lock().ok()
}
fn make_processor(
    state: &DeviceState,
    values: &[f32],
    config: AudioConfig,
) -> Option<Box<dyn DeviceProcessor>> {
    let mut device = state.device.clone();
    for (param, value) in state.parameters.iter().zip(values) {
        device.params.insert(param.path.clone(), *value);
    }
    state
        .host_context()
        .ok()?
        .run(|| audio::create_processor(&device, config).ok())
}
unsafe fn init_impl(_plugin: *const clap_plugin) -> bool {
    true
}
unsafe fn destroy_impl(plugin: *const clap_plugin) {
    if !plugin.is_null() {
        let data = unsafe { (*plugin).plugin_data as *mut Instance };
        if !data.is_null() {
            unsafe {
                drop(Box::from_raw(data));
            }
        }
    }
}
unsafe fn activate_impl(plugin: *const clap_plugin, rate: f64, _min: u32, max: u32) -> bool {
    let Some(mut p) = (unsafe { instance(plugin) }) else {
        return false;
    };
    if !rate.is_finite() || rate <= 0.0 || max == 0 || max as usize > audio::MAX_AUDIO_FRAMES {
        return false;
    }
    let config = AudioConfig {
        sample_rate: rate as f32,
        max_frames: max as usize,
        offline: p.offline,
    };
    let processor = p
        .state
        .as_ref()
        .and_then(|s| make_processor(s, &p.current_values, config));
    if p.state.is_some() && processor.is_none() {
        return false;
    }
    p.left.resize(max as usize, 0.0);
    p.right.resize(max as usize, 0.0);
    p.events.clear();
    let remaining = audio::MAX_EVENTS_PER_BLOCK.saturating_sub(p.events.capacity());
    p.events.reserve(remaining);
    let remaining = audio::MAX_EVENTS_PER_BLOCK.saturating_sub(p.param_events.capacity());
    p.param_events.reserve(remaining);
    let remaining = audio::MAX_EVENTS_PER_BLOCK.saturating_sub(p.segment_events.capacity());
    p.segment_events.reserve(remaining);
    let remaining = audio::MAX_ACTIVE_NOTES.saturating_sub(p.active_notes.capacity());
    p.active_notes.reserve(remaining);
    p.processor = processor;
    if let Some(instance) = unsafe { plugin_instance(plugin) } {
        let (latency, tail) = p.processor.as_ref().map_or((0, 0), |d| {
            let debug = d.debug_state();
            (debug.latency_samples, debug.tail_samples)
        });
        instance.latency.store(latency, Ordering::Relaxed);
        instance.tail.store(tail, Ordering::Relaxed);
    }
    p.config = config;
    p.active = true;
    p.ever_activated = true;
    true
}
unsafe fn deactivate_impl(plugin: *const clap_plugin) {
    if let Some(mut p) = unsafe { instance(plugin) } {
        p.processor = None;
        p.active = false;
        p.active_notes.clear();
        if let Some(instance) = unsafe { plugin_instance(plugin) } {
            instance.latency.store(0, Ordering::Relaxed);
            instance.tail.store(0, Ordering::Relaxed);
        }
    }
}
unsafe fn start_processing_impl(plugin: *const clap_plugin) -> bool {
    if let Some(mut p) = unsafe { instance(plugin) } {
        p.processing = p.active;
        p.processing
    } else {
        false
    }
}
unsafe fn stop_processing_impl(plugin: *const clap_plugin) {
    if let Some(mut p) = unsafe { instance(plugin) } {
        p.processing = false;
    }
}
unsafe fn reset_impl(plugin: *const clap_plugin) {
    if let Some(mut p) = unsafe { instance(plugin) } {
        if let Some(d) = &mut p.processor {
            d.reset();
        }
        p.frame_position = 0;
        p.active_notes.clear();
    }
}

fn transport(p: &Inner, raw: *const clap_event_transport) -> audio::TransportSnapshot {
    let t = unsafe { raw.as_ref() };
    let flags = t.map_or(0, |t| t.flags);
    let bpm = if flags & CLAP_TRANSPORT_HAS_TEMPO != 0 {
        t.map_or(120.0, |t| t.tempo)
    } else {
        120.0
    };
    let beat = if flags & CLAP_TRANSPORT_HAS_BEATS_TIMELINE != 0 {
        t.map_or(0.0, |t| t.song_pos_beats as f64 / (1u64 << 31) as f64)
    } else {
        0.0
    };
    let frame = if flags & CLAP_TRANSPORT_HAS_SECONDS_TIMELINE != 0 {
        t.map_or(p.frame_position, |t| {
            ((t.song_pos_seconds as f64 / (1u64 << 31) as f64) * p.config.sample_rate as f64)
                .max(0.0) as u64
        })
    } else {
        p.frame_position
    };
    audio::TransportSnapshot {
        sample_rate: p.config.sample_rate as f64,
        running: flags & CLAP_TRANSPORT_IS_PLAYING != 0,
        sample_position: frame,
        beat_position: beat,
        current_tick: beat * 960.0,
        project_frame: frame as f64,
        bpm,
        meter: [
            t.map_or(4, |t| t.tsig_num as u8),
            t.map_or(4, |t| t.tsig_denom as u8),
        ],
        loop_ticks: 0,
        ended: false,
    }
}
unsafe fn process_impl(
    plugin: *const clap_plugin,
    raw: *const clap_process,
) -> clap_process_status {
    let Some(mut p) = (unsafe { instance(plugin) }) else {
        return CLAP_PROCESS_ERROR;
    };
    #[cfg(test)]
    if p.panic_next_process {
        p.panic_next_process = false;
        panic!("injected CLAP process panic");
    }
    let Some(raw) = (unsafe { raw.as_ref() }) else {
        return CLAP_PROCESS_ERROR;
    };
    let n = raw.frames_count as usize;
    if p.overflow {
        p.overflow = false;
        return CLAP_PROCESS_ERROR;
    }
    if !p.processing
        || n > p.config.max_frames
        || raw.audio_outputs_count != 1
        || raw.audio_outputs.is_null()
    {
        return CLAP_PROCESS_ERROR;
    }
    let out = unsafe { &mut *raw.audio_outputs };
    if out.channel_count != 2 || out.data32.is_null() {
        return CLAP_PROCESS_ERROR;
    }
    let channels = unsafe { slice::from_raw_parts_mut(out.data32, 2) };
    if channels.iter().any(|x| x.is_null()) {
        return CLAP_PROCESS_ERROR;
    }
    let mut input_left = None;
    let mut input_right = None;
    let mut detector_left = None;
    let mut detector_right = None;
    if p.role == DeviceRole::Fx {
        let needs_detector = p
            .state
            .as_ref()
            .is_some_and(|s| s.device.sidechain.is_some());
        if raw.audio_inputs_count < (if needs_detector { 2 } else { 1 })
            || raw.audio_inputs_count > 2
            || raw.audio_inputs.is_null()
        {
            return CLAP_PROCESS_ERROR;
        }
        let input = unsafe { &*raw.audio_inputs };
        if input.channel_count != 2 || input.data32.is_null() {
            return CLAP_PROCESS_ERROR;
        }
        let ins = unsafe { slice::from_raw_parts(input.data32, 2) };
        if ins.iter().any(|x| x.is_null()) {
            return CLAP_PROCESS_ERROR;
        }
        input_left = Some(unsafe { slice::from_raw_parts(ins[0], n) });
        input_right = Some(unsafe { slice::from_raw_parts(ins[1], n) });
        if needs_detector {
            let aux = unsafe { &*raw.audio_inputs.add(1) };
            if aux.channel_count != 2 || aux.data32.is_null() {
                return CLAP_PROCESS_ERROR;
            }
            let channels = unsafe { slice::from_raw_parts(aux.data32, 2) };
            if channels.iter().any(|x| x.is_null()) {
                return CLAP_PROCESS_ERROR;
            }
            detector_left = Some(unsafe { slice::from_raw_parts(channels[0], n) });
            detector_right = Some(unsafe { slice::from_raw_parts(channels[1], n) });
        }
    }
    p.left[..n].fill(0.0);
    p.right[..n].fill(0.0);
    if let (Some(l), Some(r)) = (input_left, input_right) {
        p.left[..n].copy_from_slice(l);
        p.right[..n].copy_from_slice(r);
    }
    p.events.clear();
    p.param_events.clear();
    if let Some(list) = unsafe { raw.in_events.as_ref() } {
        if let (Some(size), Some(get)) = (list.size, list.get) {
            let count = unsafe { size(list) };
            if count as usize > audio::MAX_EVENTS_PER_BLOCK {
                return CLAP_PROCESS_ERROR;
            }
            for i in 0..count {
                let h = unsafe { get(list, i).as_ref() };
                let Some(h) = h else {
                    return CLAP_PROCESS_ERROR;
                };
                if h.space_id != CLAP_CORE_EVENT_SPACE_ID || h.time >= raw.frames_count {
                    continue;
                }
                let kind = match h.type_ {
                    CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF
                        if h.size as usize >= std::mem::size_of::<clap_event_note>() =>
                    {
                        let e = unsafe { &*(h as *const _ as *const clap_event_note) };
                        if e.note_id < -1
                            || !(-1..=0).contains(&e.port_index)
                            || !(-1..=15).contains(&e.channel)
                            || !(-1..=127).contains(&e.key)
                            || !e.velocity.is_finite()
                        {
                            return CLAP_PROCESS_ERROR;
                        }
                        if h.type_ == CLAP_EVENT_NOTE_ON
                            && (e.port_index != 0
                                || !(0..=15).contains(&e.channel)
                                || !(0..=127).contains(&e.key))
                        {
                            return CLAP_PROCESS_ERROR;
                        }
                        if h.type_ == CLAP_EVENT_NOTE_OFF
                            && (e.note_id < 0 || e.channel < 0 || e.key < 0)
                        {
                            for i in 0..p.active_notes.len() {
                                let note = p.active_notes[i];
                                if !note.released
                                    && matches_note_address(
                                        e.note_id,
                                        e.port_index,
                                        e.channel,
                                        e.key,
                                        note,
                                    )
                                {
                                    if p.events.len() == audio::MAX_EVENTS_PER_BLOCK {
                                        return CLAP_PROCESS_ERROR;
                                    }
                                    p.events.push(DeviceEvent {
                                        offset: h.time,
                                        kind: DeviceEventKind::NoteOff {
                                            note_id: note.internal_id,
                                            channel: note.channel,
                                            key: note.key,
                                            velocity: e.velocity as f32,
                                        },
                                    });
                                    p.active_notes[i].released = true;
                                }
                            }
                            continue;
                        }
                        if !(0..=15).contains(&e.channel) || !(0..=127).contains(&e.key) {
                            return CLAP_PROCESS_ERROR;
                        }
                        p.next_note_id = p.next_note_id.wrapping_add(1);
                        let note_id = p.next_note_id;
                        if h.type_ == CLAP_EVENT_NOTE_ON {
                            if p.active_notes.len() == audio::MAX_ACTIVE_NOTES {
                                // A released voice may still be audible and addressable.
                                return CLAP_PROCESS_ERROR;
                            }
                            p.active_notes.push(TrackedNote {
                                host_id: e.note_id,
                                internal_id: note_id,
                                port_index: e.port_index,
                                channel: e.channel as u8,
                                key: e.key as u8,
                                midi: false,
                                released: false,
                                choked: false,
                                ended: false,
                            });
                            DeviceEventKind::NoteOn {
                                sample_zone: None,
                                pitch: e.key as f32,
                                elapsed_frames: 0,
                                note_id,
                                channel: e.channel as u8,
                                key: e.key as u8,
                                velocity: e.velocity as f32,
                            }
                        } else {
                            for i in 0..p.active_notes.len() {
                                let note = p.active_notes[i];
                                if !note.released
                                    && matches_note_address(
                                        e.note_id,
                                        e.port_index,
                                        e.channel,
                                        e.key,
                                        note,
                                    )
                                {
                                    if p.events.len() == audio::MAX_EVENTS_PER_BLOCK {
                                        return CLAP_PROCESS_ERROR;
                                    }
                                    p.events.push(DeviceEvent {
                                        offset: h.time,
                                        kind: DeviceEventKind::NoteOff {
                                            note_id: note.internal_id,
                                            channel: note.channel,
                                            key: note.key,
                                            velocity: e.velocity as f32,
                                        },
                                    });
                                    p.active_notes[i].released = true;
                                }
                            }
                            continue;
                        }
                    }
                    CLAP_EVENT_NOTE_CHOKE
                        if h.size as usize >= std::mem::size_of::<clap_event_note>() =>
                    {
                        let e = unsafe { &*(h as *const _ as *const clap_event_note) };
                        if e.note_id < -1
                            || !(-1..=0).contains(&e.port_index)
                            || !(-1..=15).contains(&e.channel)
                            || !(-1..=127).contains(&e.key)
                        {
                            return CLAP_PROCESS_ERROR;
                        }
                        let mut i = 0;
                        while i < p.active_notes.len() {
                            let active = p.active_notes[i];
                            if !active.choked
                                && matches_note_address(
                                    e.note_id,
                                    e.port_index,
                                    e.channel,
                                    e.key,
                                    active,
                                )
                            {
                                if p.events.len() == audio::MAX_EVENTS_PER_BLOCK {
                                    return CLAP_PROCESS_ERROR;
                                }
                                p.events.push(DeviceEvent {
                                    offset: h.time,
                                    kind: DeviceEventKind::NoteChoke {
                                        note_id: active.internal_id,
                                        channel: active.channel,
                                        key: active.key,
                                    },
                                });
                                p.active_notes[i].choked = true;
                                i += 1;
                            } else {
                                i += 1;
                            }
                        }
                        continue;
                    }
                    CLAP_EVENT_MIDI
                        if h.size as usize >= std::mem::size_of::<clap_event_midi>() =>
                    {
                        let e = unsafe { &*(h as *const _ as *const clap_event_midi) };
                        if e.port_index != 0 {
                            return CLAP_PROCESS_ERROR;
                        }
                        if matches!(e.data[0] & 0xf0, 0x80 | 0x90) {
                            if !push_midi_note(&mut p, e.data, h.time) {
                                return CLAP_PROCESS_ERROR;
                            }
                            continue;
                        }
                        let Some(kind) = midi_event_kind(e.data) else {
                            return CLAP_PROCESS_ERROR;
                        };
                        kind
                    }
                    CLAP_EVENT_NOTE_EXPRESSION
                        if h.size as usize >= std::mem::size_of::<clap_event_note_expression>() =>
                    {
                        let e = unsafe { &*(h as *const _ as *const clap_event_note_expression) };
                        if e.note_id < -1
                            || !(-1..=0).contains(&e.port_index)
                            || !(-1..=15).contains(&e.channel)
                            || !(-1..=127).contains(&e.key)
                            || !(0..=u16::MAX as i32).contains(&e.expression_id)
                            || !e.value.is_finite()
                        {
                            return CLAP_PROCESS_ERROR;
                        }
                        for i in 0..p.active_notes.len() {
                            let active = p.active_notes[i];
                            if matches_note_address(
                                e.note_id,
                                e.port_index,
                                e.channel,
                                e.key,
                                active,
                            ) {
                                if p.events.len() == audio::MAX_EVENTS_PER_BLOCK {
                                    return CLAP_PROCESS_ERROR;
                                }
                                p.events.push(DeviceEvent {
                                    offset: h.time,
                                    kind: DeviceEventKind::NoteExpression {
                                        note_id: active.internal_id,
                                        channel: active.channel,
                                        key: active.key,
                                        expression: e.expression_id as u16,
                                        value: e.value,
                                    },
                                });
                            }
                        }
                        continue;
                    }
                    CLAP_EVENT_PARAM_VALUE
                        if h.size as usize >= std::mem::size_of::<clap_event_param_value>() =>
                    {
                        let e = unsafe { &*(h as *const _ as *const clap_event_param_value) };
                        if !e.value.is_finite() {
                            return CLAP_PROCESS_ERROR;
                        }
                        p.param_events.push((h.time, e.param_id, e.value as f32));
                        continue;
                    }
                    _ => continue,
                };
                if p.events.len() == audio::MAX_EVENTS_PER_BLOCK {
                    return CLAP_PROCESS_ERROR;
                }
                p.events.push(DeviceEvent {
                    offset: h.time,
                    kind,
                });
            }
        }
    }
    let base_transport = transport(&p, raw.transport);
    let Inner {
        processor,
        state,
        current_values,
        ranges,
        view,
        events,
        param_events,
        segment_events,
        active_notes,
        left,
        right,
        frame_position,
        ..
    } = &mut *p;
    let Some(device) = processor else {
        unsafe {
            ptr::copy(left.as_ptr(), channels[0], n);
            ptr::copy(right.as_ptr(), channels[1], n);
        }
        *frame_position = (*frame_position).saturating_add(n as u64);
        return CLAP_PROCESS_CONTINUE;
    };
    let mut start = 0usize;
    let mut action_index = 0usize;
    loop {
        let end = param_events.get(action_index).map_or(n, |e| e.0 as usize);
        if end < start || end > n {
            return CLAP_PROCESS_ERROR;
        }
        if end > start {
            segment_events.clear();
            for e in events.iter() {
                if (start..end).contains(&(e.offset as usize)) {
                    segment_events.push(DeviceEvent {
                        offset: e.offset - start as u32,
                        kind: e.kind,
                    });
                }
            }
            let mut snapshot = base_transport;
            snapshot.sample_position = snapshot.sample_position.saturating_add(start as u64);
            snapshot.project_frame += start as f64;
            snapshot.beat_position += start as f64 * snapshot.bpm / (60.0 * snapshot.sample_rate);
            snapshot.current_tick = snapshot.beat_position * 960.0;
            let ctx = ProcessContext {
                frames: end - start,
                block_start_sample: *frame_position + start as u64,
                transport: snapshot,
            };
            let result = if let (Some(dl), Some(dr)) = (detector_left, detector_right) {
                device.process_sidechain(
                    ctx,
                    &segment_events,
                    &mut left[start..end],
                    &mut right[start..end],
                    &dl[start..end],
                    &dr[start..end],
                )
            } else {
                device.process(
                    ctx,
                    &segment_events,
                    &mut left[start..end],
                    &mut right[start..end],
                )
            };
            if result.is_err() {
                return CLAP_PROCESS_ERROR;
            }
        }
        start = end;
        if action_index == param_events.len() {
            break;
        }
        let (_, id, value) = param_events[action_index];
        let Some(state) = state.as_ref() else {
            return CLAP_PROCESS_ERROR;
        };
        let Some(index) = state.parameters.iter().position(|p| p.id == id) else {
            return CLAP_PROCESS_ERROR;
        };
        let param = &state.parameters[index];
        if !ranges[index].accepts(value) || device.set_parameter(&param.path, value).is_err() {
            return CLAP_PROCESS_ERROR;
        }
        current_values[index] = value;
        if let Some(view) = view {
            view.set(index, value);
        }
        action_index += 1;
    }
    unsafe {
        ptr::copy(left.as_ptr(), channels[0], n);
        ptr::copy(right.as_ptr(), channels[1], n);
    }
    if n > 0 {
        for note in active_notes.iter_mut() {
            if !device.has_note(note.internal_id) {
                note.ended = true;
            }
        }
        // A host wildcard address represents one note lifetime. Keep one
        // undelivered end marker while repeated voices reuse that address.
        let mut i = 0;
        while i < active_notes.len() {
            let note = active_notes[i];
            if note.ended
                && active_notes[..i].iter().any(|other| {
                    other.ended
                        && other.host_id == note.host_id
                        && other.port_index == note.port_index
                        && other.channel == note.channel
                        && other.key == note.key
                })
            {
                active_notes.swap_remove(i);
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i < active_notes.len() {
            let note = active_notes[i];
            if note.ended {
                // NOTE_END addresses the host's voice, not our internal voice.
                // A wildcard (or reused) host ID must not terminate a newer
                // matching voice that is still sounding in this block.
                let same_host_address = |other: &TrackedNote| {
                    other.port_index == note.port_index
                        && other.channel == note.channel
                        && other.key == note.key
                        && (note.host_id < 0 || other.host_id == note.host_id)
                };
                if active_notes.iter().any(|other| {
                    other.internal_id != note.internal_id
                        && same_host_address(other)
                        && device.has_note(other.internal_id)
                }) {
                    i += 1;
                    continue;
                }
                if unsafe { push_note_end(raw, note, (n - 1) as u32) } {
                    active_notes.retain(|other| {
                        !(same_host_address(other) && !device.has_note(other.internal_id))
                    });
                    i = 0;
                    continue;
                }
            }
            i += 1;
        }
    }
    *frame_position = (*frame_position).saturating_add(n as u64);
    CLAP_PROCESS_CONTINUE
}

unsafe fn get_extension_impl(_plugin: *const clap_plugin, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return ptr::null();
    }
    let name = unsafe { CStr::from_ptr(id) };
    if name == CLAP_EXT_STATE {
        &STATE as *const _ as *const c_void
    } else if name == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if name == CLAP_EXT_AUDIO_PORTS {
        &AUDIO_PORTS as *const _ as *const c_void
    } else if name == CLAP_EXT_NOTE_PORTS {
        &NOTE_PORTS as *const _ as *const c_void
    } else if name == CLAP_EXT_LATENCY {
        &LATENCY as *const _ as *const c_void
    } else if name == CLAP_EXT_TAIL {
        &TAIL as *const _ as *const c_void
    } else if name == CLAP_EXT_RENDER {
        &RENDER as *const _ as *const c_void
    } else {
        ptr::null()
    }
}
unsafe fn on_main_thread_impl(_plugin: *const clap_plugin) {}

unsafe fn save_impl(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let Some(instance) = (unsafe { plugin_instance(plugin) }) else {
        return false;
    };
    let view = match instance.view.read() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    };
    let Some(view) = view else { return false };
    let Some(stream) = (unsafe { stream.as_ref() }) else {
        return false;
    };
    let Some(write) = stream.write else {
        return false;
    };
    let mut state = (*view.state).clone();
    for (index, param) in state.parameters.iter_mut().enumerate() {
        let value = view.get(index);
        param.value = value;
        state.device.params.insert(param.path.clone(), value);
    }
    let Ok(bytes) = state.encode() else {
        return false;
    };
    let mut offset = 0;
    while offset < bytes.len() {
        let n = unsafe {
            write(
                stream,
                bytes[offset..].as_ptr().cast(),
                (bytes.len() - offset) as u64,
            )
        };
        if n <= 0 || n as usize > bytes.len() - offset {
            return false;
        }
        offset += n as usize;
    }
    true
}
unsafe fn load_impl(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    let Some(stream) = (unsafe { stream.as_ref() }) else {
        return false;
    };
    let Some(read) = stream.read else {
        return false;
    };
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = unsafe { read(stream, buf.as_mut_ptr().cast(), buf.len() as u64) };
        if n < 0 || n as usize > buf.len() {
            return false;
        }
        if n == 0 {
            break;
        }
        if bytes.len() + n as usize > muz::device_state::MAX_STATE_BYTES {
            return false;
        }
        bytes.extend_from_slice(&buf[..n as usize]);
    }
    let Ok(state) = DeviceState::decode(&bytes) else {
        return false;
    };
    let Some(ranges) = parameter_ranges(&state) else {
        return false;
    };
    let state = Arc::new(state);
    let view = Arc::new(StateView::new(Arc::clone(&state), ranges.clone()));
    let values = state.parameters.iter().map(|p| p.value).collect();
    let (host_ptr, notify_host, should_restart) = {
        let Some(mut p) = (unsafe { instance(plugin) }) else {
            return false;
        };
        if p.active || state.role != p.role {
            return false;
        }
        p.state = Some(state);
        p.current_values = values;
        p.ranges = ranges;
        p.view = Some(Arc::clone(&view));
        p.processor = None;
        // Bitwig requests a processing restart when any host rescan is sent
        // during initial preset import. Delay parameter notification until a
        // subsequent state replacement. FX audio ports are stable from
        // factory creation, so no audio-port rescan is needed.
        let notify_host = p.loaded_external_state;
        p.loaded_external_state = true;
        (p.host, notify_host, notify_host && p.ever_activated)
    };
    let Some(instance) = (unsafe { plugin_instance(plugin) }) else {
        return false;
    };
    let Ok(mut slot) = instance.view.write() else {
        return false;
    };
    *slot = Some(view);
    drop(slot);
    if notify_host && let Some(host) = unsafe { host_ptr.as_ref() } {
        if let Some(get_extension) = host.get_extension {
            let ext = unsafe { get_extension(host_ptr, CLAP_EXT_PARAMS.as_ptr()) };
            if let Some(params) = unsafe { (ext as *const clap_host_params).as_ref() } {
                if let Some(rescan) = params.rescan {
                    unsafe {
                        rescan(host_ptr, CLAP_PARAM_RESCAN_ALL);
                    }
                }
            }
        }
        if should_restart {
            if let Some(request_restart) = host.request_restart {
                unsafe {
                    request_restart(host_ptr);
                }
            }
        }
    }
    true
}
static STATE: clap_plugin_state = clap_plugin_state {
    save: Some(save),
    load: Some(load),
};

unsafe fn audio_count_impl(plugin: *const clap_plugin, input: bool) -> u32 {
    let Some(p) = (unsafe { plugin_instance(plugin) }) else {
        return 0;
    };
    if !input {
        return 1;
    }
    if p.role == DeviceRole::Instrument {
        return 0;
    }
    2
}
fn name_into<const N: usize>(dest: &mut [c_char; N], name: &str) {
    for (d, b) in dest.iter_mut().zip(name.bytes()) {
        *d = b as c_char;
    }
}
unsafe fn audio_get_impl(
    plugin: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if info.is_null() || index >= unsafe { audio_count(plugin, input) } {
        return false;
    }
    let out = unsafe { &mut *info };
    out.id = if input {
        if index == 0 { 0 } else { 2 }
    } else {
        1
    };
    out.name.fill(0);
    name_into(
        &mut out.name,
        if !input {
            "Main Output"
        } else if index == 0 {
            "Main Input"
        } else {
            "Detector"
        },
    );
    out.flags = if index == 0 {
        CLAP_AUDIO_PORT_IS_MAIN
    } else {
        0
    };
    out.channel_count = 2;
    out.port_type = CLAP_PORT_STEREO.as_ptr();
    out.in_place_pair = CLAP_INVALID_ID;
    true
}
static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_count),
    get: Some(audio_get),
};
unsafe fn note_count_impl(plugin: *const clap_plugin, input: bool) -> u32 {
    unsafe { plugin_instance(plugin) }
        .map_or(0, |p| u32::from(input && p.role == DeviceRole::Instrument))
}
unsafe fn note_get_impl(
    plugin: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    if index != 0 || info.is_null() || unsafe { note_count(plugin, input) } == 0 {
        return false;
    }
    let out = unsafe { &mut *info };
    out.id = 0;
    out.name.fill(0);
    name_into(&mut out.name, "Notes");
    out.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    out.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    true
}
static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_count),
    get: Some(note_get),
};
unsafe fn latency_get_impl(plugin: *const clap_plugin) -> u32 {
    unsafe { plugin_instance(plugin) }.map_or(0, |p| p.latency.load(Ordering::Relaxed))
}
unsafe fn tail_get_impl(plugin: *const clap_plugin) -> u32 {
    unsafe { plugin_instance(plugin) }.map_or(0, |p| p.tail.load(Ordering::Relaxed))
}
static LATENCY: clap_plugin_latency = clap_plugin_latency {
    get: Some(latency_get),
};
static TAIL: clap_plugin_tail = clap_plugin_tail {
    get: Some(tail_get),
};
unsafe fn render_hard_realtime_impl(_plugin: *const clap_plugin) -> bool {
    false
}
unsafe fn render_set_impl(plugin: *const clap_plugin, mode: clap_plugin_render_mode) -> bool {
    let Some(mut p) = (unsafe { instance(plugin) }) else {
        return false;
    };
    if p.active {
        return false;
    }
    match mode {
        CLAP_RENDER_REALTIME => p.offline = false,
        CLAP_RENDER_OFFLINE => p.offline = true,
        _ => return false,
    }
    true
}
static RENDER: clap_plugin_render = clap_plugin_render {
    has_hard_realtime_requirement: Some(render_hard_realtime),
    set: Some(render_set),
};

unsafe fn param_count_impl(plugin: *const clap_plugin) -> u32 {
    let Some(p) = (unsafe { plugin_instance(plugin) }) else {
        return 0;
    };
    p.view
        .read()
        .ok()
        .and_then(|v| v.as_ref().map(|v| v.state.parameters.len() as u32))
        .unwrap_or(0)
}
unsafe fn param_info_impl(
    plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_param_info,
) -> bool {
    let Some(p) = (unsafe { plugin_instance(plugin) }) else {
        return false;
    };
    let view = match p.view.read() {
        Ok(slot) => slot.clone(),
        Err(_) => None,
    };
    let Some(view) = view else { return false };
    let state = &view.state;
    let Some(param) = state.parameters.get(index as usize) else {
        return false;
    };
    let Some(info) = (unsafe { info.as_mut() }) else {
        return false;
    };
    let Some(range) = view.ranges.get(index as usize) else {
        return false;
    };
    info.id = param.id;
    info.flags = CLAP_PARAM_IS_AUTOMATABLE;
    info.cookie = ptr::null_mut();
    info.name.fill(0);
    info.module.fill(0);
    name_into(&mut info.name, &param.path);
    info.min_value = range.min as f64;
    info.max_value = range.max as f64;
    info.default_value = range.default as f64;
    true
}
unsafe fn param_value_impl(plugin: *const clap_plugin, id: u32, value: *mut f64) -> bool {
    let Some(p) = (unsafe { plugin_instance(plugin) }) else {
        return false;
    };
    let view = match p.view.read() {
        Ok(slot) => slot.clone(),
        Err(_) => None,
    };
    let Some(view) = view else { return false };
    let Some(index) = view.state.parameters.iter().position(|p| p.id == id) else {
        return false;
    };
    let Some(value) = (unsafe { value.as_mut() }) else {
        return false;
    };
    *value = view.get(index) as f64;
    true
}
unsafe fn value_to_text_impl(
    _plugin: *const clap_plugin,
    _id: u32,
    value: f64,
    out: *mut c_char,
    cap: u32,
) -> bool {
    if out.is_null() || cap == 0 {
        return false;
    }
    let s = format!("{value:.3}");
    let bytes = s.as_bytes();
    if bytes.len() + 1 > cap as usize {
        return false;
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr().cast(), out, bytes.len());
        *out.add(bytes.len()) = 0;
    }
    true
}
unsafe fn text_to_value_impl(
    _plugin: *const clap_plugin,
    _id: u32,
    text: *const c_char,
    out: *mut f64,
) -> bool {
    if text.is_null() || out.is_null() {
        return false;
    }
    let Ok(s) = unsafe { CStr::from_ptr(text) }.to_str() else {
        return false;
    };
    let Ok(value) = s.trim().parse::<f64>() else {
        return false;
    };
    unsafe {
        *out = value;
    }
    true
}
unsafe fn flush_impl(
    plugin: *const clap_plugin,
    input: *const clap_input_events,
    _output: *const clap_output_events,
) {
    let Some(mut p) = (unsafe { instance(plugin) }) else {
        return;
    };
    let Some(list) = (unsafe { input.as_ref() }) else {
        return;
    };
    let (Some(size), Some(get)) = (list.size, list.get) else {
        return;
    };
    let count = unsafe { size(list) };
    if count as usize > audio::MAX_EVENTS_PER_BLOCK {
        p.overflow = true;
        return;
    }
    for i in 0..count {
        let Some(h) = (unsafe { get(list, i).as_ref() }) else {
            continue;
        };
        if h.type_ != CLAP_EVENT_PARAM_VALUE
            || (h.size as usize) < std::mem::size_of::<clap_event_param_value>()
        {
            continue;
        }
        let event = unsafe { &*(h as *const _ as *const clap_event_param_value) };
        let Inner {
            state,
            processor,
            ranges,
            current_values,
            view,
            ..
        } = &mut *p;
        let Some(state) = state.as_ref() else {
            continue;
        };
        let Some(index) = state.parameters.iter().position(|p| p.id == event.param_id) else {
            continue;
        };
        let param = &state.parameters[index];
        let value = event.value as f32;
        if !ranges[index].accepts(value) {
            continue;
        }
        current_values[index] = value;
        if let Some(view) = view {
            view.set(index, value);
        }
        if let Some(d) = processor {
            let _ = d.set_parameter(&param.path, value);
        }
    }
}
static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(param_count),
    get_info: Some(param_info),
    get_value: Some(param_value),
    value_to_text: Some(value_to_text),
    text_to_value: Some(text_to_value),
    flush: Some(flush),
};

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};
unsafe fn factory_count_impl(_factory: *const clap_plugin_factory) -> u32 {
    2
}
unsafe fn factory_descriptor_impl(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    match index {
        0 => &INSTRUMENT_DESC,
        1 => &FX_DESC,
        _ => ptr::null(),
    }
}
unsafe fn factory_create_impl(
    _factory: *const clap_plugin_factory,
    host: *const clap_host,
    id: *const c_char,
) -> *const clap_plugin {
    if host.is_null() || id.is_null() {
        return ptr::null();
    }
    let name = unsafe { CStr::from_ptr(id) };
    let (role, desc) = if name.to_bytes() == INSTRUMENT_ID.as_bytes() {
        (DeviceRole::Instrument, &INSTRUMENT_DESC)
    } else if name.to_bytes() == FX_ID.as_bytes() {
        (DeviceRole::Fx, &FX_DESC)
    } else {
        return ptr::null();
    };
    let device = model::Device {
        sfz: None,
        asset_versions: Vec::new(),
        patch: None,
        generation: 0,
        rack: None,
        sample: None,
        sidechain: None,
        id: model::Id::new("default"),
        kind: if role == DeviceRole::Instrument {
            model::DeviceKind::StudioSynth
        } else {
            model::DeviceKind::Gain
        },
        params: BTreeMap::new(),
        vst3: None,
    };
    let Ok(state) = DeviceState::from_device(role, device) else {
        return ptr::null();
    };
    let Some(ranges) = parameter_ranges(&state) else {
        return ptr::null();
    };
    let state = Arc::new(state);
    let view = Arc::new(StateView::new(Arc::clone(&state), ranges.clone()));
    let current_values = state.parameters.iter().map(|p| p.value).collect();
    let mut p = Box::new(Instance {
        plugin: clap_plugin {
            desc,
            plugin_data: ptr::null_mut(),
            init: Some(init),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(deactivate),
            start_processing: Some(start_processing),
            stop_processing: Some(stop_processing),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(get_extension),
            on_main_thread: Some(on_main_thread),
        },
        view: RwLock::new(Some(Arc::clone(&view))),
        role,
        latency: AtomicU32::new(0),
        tail: AtomicU32::new(0),
        inner: Mutex::new(Inner {
            #[cfg(test)]
            panic_next_process: false,
            host,
            role,
            state: Some(state),
            current_values,
            ranges,
            view: Some(view),
            processor: None,
            active: false,
            ever_activated: false,
            loaded_external_state: false,
            processing: false,
            config: AudioConfig {
                sample_rate: 48000.0,
                max_frames: 0,
                offline: false,
            },
            left: Vec::new(),
            right: Vec::new(),
            events: Vec::new(),
            param_events: Vec::new(),
            segment_events: Vec::new(),
            frame_position: 0,
            offline: false,
            overflow: false,
            active_notes: Vec::new(),
            next_note_id: 0,
        }),
    });
    p.plugin.plugin_data = (&mut *p as *mut Instance).cast();
    let address = &p.plugin as *const clap_plugin;
    let _ = Box::into_raw(p);
    address
}
unsafe fn entry_init_impl(_path: *const c_char) -> bool {
    true
}
unsafe fn entry_deinit_impl() {}
unsafe fn entry_factory_impl(id: *const c_char) -> *const c_void {
    if id.is_null() {
        return ptr::null();
    }
    if unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const _ as *const c_void
    } else {
        ptr::null()
    }
}
// Every CLAP entry point catches Rust panics before crossing the C ABI.

unsafe extern "C" fn init(_plugin: *const clap_plugin) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        init_impl(_plugin)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn destroy(plugin: *const clap_plugin) {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        destroy_impl(plugin)
    }))
    .unwrap_or(())
}

unsafe extern "C" fn activate(plugin: *const clap_plugin, rate: f64, _min: u32, max: u32) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        activate_impl(plugin, rate, _min, max)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn deactivate(plugin: *const clap_plugin) {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        deactivate_impl(plugin)
    }))
    .unwrap_or(())
}

unsafe extern "C" fn start_processing(plugin: *const clap_plugin) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        start_processing_impl(plugin)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn stop_processing(plugin: *const clap_plugin) {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        stop_processing_impl(plugin)
    }))
    .unwrap_or(())
}

unsafe extern "C" fn reset(plugin: *const clap_plugin) {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        reset_impl(plugin)
    }))
    .unwrap_or(())
}

unsafe extern "C" fn process(
    plugin: *const clap_plugin,
    raw: *const clap_process,
) -> clap_process_status {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        process_impl(plugin, raw)
    }))
    .unwrap_or(CLAP_PROCESS_ERROR)
}

unsafe extern "C" fn get_extension(
    _plugin: *const clap_plugin,
    id: *const c_char,
) -> *const c_void {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        get_extension_impl(_plugin, id)
    }))
    .unwrap_or(ptr::null())
}

unsafe extern "C" fn on_main_thread(_plugin: *const clap_plugin) {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        on_main_thread_impl(_plugin)
    }))
    .unwrap_or(())
}

unsafe extern "C" fn save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        save_impl(plugin, stream)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        load_impl(plugin, stream)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn audio_count(plugin: *const clap_plugin, input: bool) -> u32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        audio_count_impl(plugin, input)
    }))
    .unwrap_or(0)
}

unsafe extern "C" fn audio_get(
    plugin: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        audio_get_impl(plugin, index, input, info)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn note_count(plugin: *const clap_plugin, input: bool) -> u32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        note_count_impl(plugin, input)
    }))
    .unwrap_or(0)
}

unsafe extern "C" fn note_get(
    plugin: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        note_get_impl(plugin, index, input, info)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn latency_get(plugin: *const clap_plugin) -> u32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        latency_get_impl(plugin)
    }))
    .unwrap_or(0)
}

unsafe extern "C" fn tail_get(plugin: *const clap_plugin) -> u32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        tail_get_impl(plugin)
    }))
    .unwrap_or(0)
}

unsafe extern "C" fn render_hard_realtime(_plugin: *const clap_plugin) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        render_hard_realtime_impl(_plugin)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn render_set(plugin: *const clap_plugin, mode: clap_plugin_render_mode) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        render_set_impl(plugin, mode)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn param_count(plugin: *const clap_plugin) -> u32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        param_count_impl(plugin)
    }))
    .unwrap_or(0)
}

unsafe extern "C" fn param_info(
    plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_param_info,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        param_info_impl(plugin, index, info)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn param_value(plugin: *const clap_plugin, id: u32, value: *mut f64) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        param_value_impl(plugin, id, value)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn value_to_text(
    _plugin: *const clap_plugin,
    _id: u32,
    value: f64,
    out: *mut c_char,
    cap: u32,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        value_to_text_impl(_plugin, _id, value, out, cap)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn text_to_value(
    _plugin: *const clap_plugin,
    _id: u32,
    text: *const c_char,
    out: *mut f64,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        text_to_value_impl(_plugin, _id, text, out)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn flush(
    plugin: *const clap_plugin,
    input: *const clap_input_events,
    _output: *const clap_output_events,
) {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        flush_impl(plugin, input, _output)
    }))
    .unwrap_or(())
}

unsafe extern "C" fn factory_count(_factory: *const clap_plugin_factory) -> u32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        factory_count_impl(_factory)
    }))
    .unwrap_or(0)
}

unsafe extern "C" fn factory_descriptor(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        factory_descriptor_impl(_factory, index)
    }))
    .unwrap_or(ptr::null())
}

unsafe extern "C" fn factory_create(
    _factory: *const clap_plugin_factory,
    host: *const clap_host,
    id: *const c_char,
) -> *const clap_plugin {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        factory_create_impl(_factory, host, id)
    }))
    .unwrap_or(ptr::null())
}

unsafe extern "C" fn entry_init(_path: *const c_char) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        entry_init_impl(_path)
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn entry_deinit() {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        entry_deinit_impl()
    }))
    .unwrap_or(())
}

unsafe extern "C" fn entry_factory(id: *const c_char) -> *const c_void {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        entry_factory_impl(id)
    }))
    .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_factory),
};

#[cfg(test)]
mod tests {
    use super::*;
    use clap_sys::{audio_buffer::clap_audio_buffer, version::CLAP_VERSION};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static RESTARTS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn restart(_host: *const clap_host) {
        RESTARTS.fetch_add(1, Ordering::Relaxed);
    }
    struct HostSignals {
        rescans: AtomicUsize,
        param_rescans: AtomicUsize,
        restarts: AtomicUsize,
    }
    unsafe extern "C" fn test_restart(host: *const clap_host) {
        let signals = unsafe { &*((*host).host_data as *const HostSignals) };
        signals.restarts.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "C" fn supports_audio_rescan(_host: *const clap_host, flag: u32) -> bool {
        flag == CLAP_AUDIO_PORTS_RESCAN_LIST
    }
    unsafe extern "C" fn audio_rescan(host: *const clap_host, flags: u32) {
        assert_eq!(flags, CLAP_AUDIO_PORTS_RESCAN_LIST);
        let signals = unsafe { &*((*host).host_data as *const HostSignals) };
        signals.rescans.fetch_add(1, Ordering::Relaxed);
    }
    static HOST_AUDIO_PORTS: clap_host_audio_ports = clap_host_audio_ports {
        is_rescan_flag_supported: Some(supports_audio_rescan),
        rescan: Some(audio_rescan),
    };
    unsafe extern "C" fn param_rescan(host: *const clap_host, flags: u32) {
        assert_eq!(flags, CLAP_PARAM_RESCAN_ALL);
        let signals = unsafe { &*((*host).host_data as *const HostSignals) };
        signals.param_rescans.fetch_add(1, Ordering::Relaxed);
    }
    static HOST_PARAMS: clap_host_params = clap_host_params {
        rescan: Some(param_rescan),
        clear: None,
        request_flush: None,
    };
    unsafe extern "C" fn host_extension(
        _host: *const clap_host,
        id: *const c_char,
    ) -> *const c_void {
        if id.is_null() {
            return ptr::null();
        }
        if unsafe { CStr::from_ptr(id) } == CLAP_EXT_AUDIO_PORTS {
            &HOST_AUDIO_PORTS as *const _ as *const c_void
        } else if unsafe { CStr::from_ptr(id) } == CLAP_EXT_PARAMS {
            &HOST_PARAMS as *const _ as *const c_void
        } else {
            ptr::null()
        }
    }
    unsafe extern "C" fn read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
        let (bytes, pos) = unsafe { &mut *((*stream).ctx as *mut (Vec<u8>, usize)) };
        let n = (bytes.len() - *pos).min(size as usize).min(3);
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr().add(*pos), buffer.cast(), n);
        }
        *pos += n;
        n as i64
    }
    unsafe extern "C" fn write(
        stream: *const clap_ostream,
        buffer: *const c_void,
        size: u64,
    ) -> i64 {
        let bytes = unsafe { &mut *((*stream).ctx as *mut Vec<u8>) };
        let n = (size as usize).min(3);
        bytes.extend_from_slice(unsafe { slice::from_raw_parts(buffer.cast(), n) });
        n as i64
    }
    struct ReentrantWrite {
        bytes: Vec<u8>,
        plugin: *const clap_plugin,
        block: *const clap_process,
        process_status: Option<clap_process_status>,
    }
    unsafe extern "C" fn write_while_processing(
        stream: *const clap_ostream,
        buffer: *const c_void,
        size: u64,
    ) -> i64 {
        let ctx = unsafe { &mut *((*stream).ctx as *mut ReentrantWrite) };
        if ctx.process_status.is_none() {
            ctx.process_status = Some(unsafe { process(ctx.plugin, ctx.block) });
        }
        let n = (size as usize).min(3);
        ctx.bytes
            .extend_from_slice(unsafe { slice::from_raw_parts(buffer.cast(), n) });
        n as i64
    }
    unsafe extern "C" fn event_size(list: *const clap_input_events) -> u32 {
        unsafe { (&*((*list).ctx as *const Vec<clap_event_note>)).len() as u32 }
    }
    unsafe extern "C" fn event_get(
        list: *const clap_input_events,
        index: u32,
    ) -> *const clap_event_header {
        unsafe {
            (&*((*list).ctx as *const Vec<clap_event_note>))
                .get(index as usize)
                .map_or(ptr::null(), |e| &e.header)
        }
    }
    unsafe extern "C" fn midi_event_size(list: *const clap_input_events) -> u32 {
        unsafe { (&*((*list).ctx as *const Vec<clap_event_midi>)).len() as u32 }
    }
    unsafe extern "C" fn midi_event_get(
        list: *const clap_input_events,
        index: u32,
    ) -> *const clap_event_header {
        unsafe {
            (&*((*list).ctx as *const Vec<clap_event_midi>))
                .get(index as usize)
                .map_or(ptr::null(), |e| &e.header)
        }
    }
    fn midi(time: u32, data: [u8; 3]) -> clap_event_midi {
        clap_event_midi {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_midi>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_MIDI,
                flags: 0,
            },
            port_index: 0,
            data,
        }
    }
    unsafe extern "C" fn parameter_event_size(list: *const clap_input_events) -> u32 {
        unsafe { (&*((*list).ctx as *const Vec<clap_event_param_value>)).len() as u32 }
    }
    unsafe extern "C" fn parameter_event_get(
        list: *const clap_input_events,
        index: u32,
    ) -> *const clap_event_header {
        unsafe {
            (&*((*list).ctx as *const Vec<clap_event_param_value>))
                .get(index as usize)
                .map_or(ptr::null(), |e| &e.header)
        }
    }
    unsafe extern "C" fn header_size(list: *const clap_input_events) -> u32 {
        unsafe { (&*((*list).ctx as *const Vec<*const clap_event_header>)).len() as u32 }
    }
    unsafe extern "C" fn header_get(
        list: *const clap_input_events,
        index: u32,
    ) -> *const clap_event_header {
        unsafe {
            (&*((*list).ctx as *const Vec<*const clap_event_header>))
                .get(index as usize)
                .copied()
                .unwrap_or(ptr::null())
        }
    }
    #[derive(Default)]
    struct EndSink {
        events: Vec<clap_event_note>,
        reject_once: bool,
        rejected: usize,
    }
    unsafe extern "C" fn push_end(
        list: *const clap_output_events,
        event: *const clap_event_header,
    ) -> bool {
        let sink = unsafe { &mut *((*list).ctx as *mut EndSink) };
        if sink.reject_once {
            sink.reject_once = false;
            sink.rejected += 1;
            return false;
        }
        let event = unsafe { &*(event as *const clap_event_note) };
        assert_eq!(event.header.type_, CLAP_EVENT_NOTE_END);
        sink.events.push(*event);
        true
    }
    fn note(kind: u16, time: u32, id: i32) -> clap_event_note {
        clap_event_note {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_note>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: kind,
                flags: 0,
            },
            note_id: id,
            port_index: 0,
            channel: 0,
            key: 60,
            velocity: 1.0,
        }
    }
    #[test]
    fn midi_cc_decodes_to_native_controller() {
        for controller in [11, 74, 120] {
            assert_eq!(
                midi_event_kind([0xb3, controller, 91]),
                Some(DeviceEventKind::Controller {
                    channel: 3,
                    controller,
                    value: 91
                })
            );
        }
        assert_eq!(midi_event_kind([0xb0, 120, 128]), None);
    }
    #[test]
    fn midi_notes_use_native_voices_offsets_and_fifo_release() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, INSTRUMENT_ID_C.as_ptr().cast()) };
        let mut port = std::mem::MaybeUninit::<clap_note_port_info>::uninit();
        assert!(unsafe { note_get(plugin, 0, true, port.as_mut_ptr()) });
        let port = unsafe { port.assume_init() };
        assert_eq!(
            port.supported_dialects,
            CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI
        );
        let device = serde_json::from_value(serde_json::json!({
            "id":"s", "kind":"builtin.studio_synth", "params":{"release_ms":300.0}
        }))
        .unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        let mut input = (state.encode().unwrap(), 0usize);
        let stream = clap_istream {
            ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!(unsafe { load(plugin, &stream) });
        assert!(unsafe { activate(plugin, 48000.0, 1, 256) });
        assert!(unsafe { start_processing(plugin) });
        let mut left = [0.0f32; 256];
        let mut right = [0.0f32; 256];
        let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut output = clap_audio_buffer {
            data32: channels.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut events = vec![
            midi(64, [0x92, 60, 100]),
            midi(96, [0xb2, 11, 127]),
            midi(192, [0x82, 60, 64]),
        ];
        let list = clap_input_events {
            ctx: (&mut events as *mut Vec<clap_event_midi>).cast(),
            size: Some(midi_event_size),
            get: Some(midi_event_get),
        };
        let block = clap_process {
            steady_time: 0,
            frames_count: 256,
            transport: ptr::null(),
            audio_inputs: ptr::null(),
            audio_outputs: &mut output,
            audio_inputs_count: 0,
            audio_outputs_count: 1,
            in_events: &list,
            out_events: ptr::null(),
        };
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        assert!(left[..64].iter().all(|sample| *sample == 0.0));
        assert!(left[64..192].iter().any(|sample| sample.abs() > 1.0e-5));
        let first_id = {
            let p = unsafe { instance(plugin) }.unwrap();
            assert_eq!(p.events.len(), 3);
            let DeviceEventKind::NoteOn {
                note_id,
                channel,
                key,
                velocity,
                ..
            } = p.events[0].kind
            else {
                panic!("MIDI note-on was not decoded");
            };
            assert_eq!((p.events[0].offset, channel, key), (64, 2, 60));
            assert!((velocity - 100.0 / 127.0).abs() < 1.0e-6);
            assert_eq!(
                p.events[1],
                DeviceEvent {
                    offset: 96,
                    kind: DeviceEventKind::Controller {
                        channel: 2,
                        controller: 11,
                        value: 127,
                    },
                }
            );
            assert!(
                matches!(p.events[2].kind, DeviceEventKind::NoteOff { note_id: id, .. } if id == note_id)
            );
            assert_eq!(p.events[2].offset, 192);
            note_id
        };
        events.clear();
        events.extend([
            midi(0, [0x92, 60, 100]),
            midi(1, [0x92, 60, 110]),
            midi(2, [0x82, 60, 64]),
            midi(3, [0x92, 60, 0]),
        ]);
        assert_eq!(events.len(), 4);
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        let p = unsafe { instance(plugin) }.unwrap();
        assert_eq!(p.events.len(), 4);
        let DeviceEventKind::NoteOn {
            note_id: second_id, ..
        } = p.events[0].kind
        else {
            panic!("first retrigger missing");
        };
        let DeviceEventKind::NoteOn {
            note_id: third_id, ..
        } = p.events[1].kind
        else {
            panic!("second retrigger missing");
        };
        assert_ne!(first_id, second_id);
        assert_ne!(second_id, third_id);
        assert!(
            matches!(p.events[2].kind, DeviceEventKind::NoteOff { note_id, .. } if note_id == second_id)
        );
        assert!(
            matches!(p.events[3].kind, DeviceEventKind::NoteOff { note_id, .. } if note_id == third_id)
        );
        drop(p);
        unsafe {
            stop_processing(plugin);
            deactivate(plugin);
            destroy(plugin);
        }
    }
    #[test]
    fn midi_cc11_changes_studio_synth_audio_after_smoothing() {
        fn render_with_cc11(value: u8) -> f64 {
            let host = clap_host {
                clap_version: CLAP_VERSION,
                host_data: ptr::null_mut(),
                name: ptr::null(),
                vendor: ptr::null(),
                url: ptr::null(),
                version: ptr::null(),
                get_extension: None,
                request_restart: None,
                request_process: None,
                request_callback: None,
            };
            let plugin =
                unsafe { factory_create(&FACTORY, &host, INSTRUMENT_ID_C.as_ptr().cast()) };
            let device = serde_json::from_value(serde_json::json!({
                "id":"s", "kind":"builtin.studio_synth", "params":{}
            }))
            .unwrap();
            let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
            let mut input = (state.encode().unwrap(), 0usize);
            let stream = clap_istream {
                ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
                read: Some(read),
            };
            assert!(unsafe { load(plugin, &stream) });
            assert!(unsafe { activate(plugin, 48000.0, 1, 256) });
            assert!(unsafe { start_processing(plugin) });
            let mut direct = make_processor(
                &state,
                &[],
                AudioConfig {
                    sample_rate: 48000.0,
                    max_frames: 256,
                    offline: false,
                },
            )
            .unwrap();
            let snapshot = {
                let p = unsafe { instance(plugin) }.unwrap();
                transport(&p, ptr::null())
            };
            let mut left = [0.0f32; 256];
            let mut right = [0.0f32; 256];
            let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
            let mut output = clap_audio_buffer {
                data32: channels.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let mut events = vec![midi(0, [0xb0, 11, value]), midi(0, [0x90, 60, 100])];
            let list = clap_input_events {
                ctx: (&mut events as *mut Vec<clap_event_midi>).cast(),
                size: Some(midi_event_size),
                get: Some(midi_event_get),
            };
            let block = clap_process {
                steady_time: 0,
                frames_count: 256,
                transport: ptr::null(),
                audio_inputs: ptr::null(),
                audio_outputs: &mut output,
                audio_inputs_count: 0,
                audio_outputs_count: 1,
                in_events: &list,
                out_events: ptr::null(),
            };
            let mut energy = 0.0;
            for i in 0..40 {
                let mut direct_l = [0.0f32; 256];
                let mut direct_r = [0.0f32; 256];
                let native_events = if i == 0 {
                    vec![
                        DeviceEvent {
                            offset: 0,
                            kind: DeviceEventKind::Controller {
                                channel: 0,
                                controller: 11,
                                value,
                            },
                        },
                        DeviceEvent {
                            offset: 0,
                            kind: DeviceEventKind::NoteOn {
                                sample_zone: None,
                                pitch: 60.0,
                                elapsed_frames: 0,
                                note_id: 1,
                                channel: 0,
                                key: 60,
                                velocity: 100.0 / 127.0,
                            },
                        },
                    ]
                } else {
                    Vec::new()
                };
                direct
                    .process(
                        ProcessContext {
                            frames: 256,
                            block_start_sample: i * 256,
                            transport: snapshot,
                        },
                        &native_events,
                        &mut direct_l,
                        &mut direct_r,
                    )
                    .unwrap();
                assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
                assert_eq!(
                    left, direct_l,
                    "CLAP MIDI CC11 differs from native Controller"
                );
                if i == 0 {
                    let p = unsafe { instance(plugin) }.unwrap();
                    assert!(
                        matches!(p.events[0].kind, DeviceEventKind::Controller { controller: 11, value: v, .. } if v == value)
                    );
                }
                if i >= 20 {
                    energy += left.iter().map(|x| f64::from(*x * *x)).sum::<f64>();
                }
                events.clear();
            }
            unsafe {
                stop_processing(plugin);
                deactivate(plugin);
                destroy(plugin);
            }
            energy
        }
        let high = render_with_cc11(127);
        let low = render_with_cc11(0);
        assert!(high > 1.0, "high CC11 should sound: {high}");
        assert!(
            low < high * 0.01,
            "CC11 0 should attenuate: {low} vs {high}"
        );
    }
    #[test]
    fn process_panic_is_contained_at_c_abi() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, FX_ID_C.as_ptr().cast()) };
        assert!(!plugin.is_null());
        unsafe { instance(plugin) }.unwrap().panic_next_process = true;
        assert_eq!(unsafe { process(plugin, ptr::null()) }, CLAP_PROCESS_ERROR);
        unsafe {
            destroy(plugin);
        }
    }
    #[test]
    fn factory_state_streams_and_fx_audio() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: Some(restart),
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, FX_ID_C.as_ptr().cast()) };
        assert!(!plugin.is_null());
        let device = serde_json::from_value(
            serde_json::json!({"id":"g", "kind":"builtin.gain", "params":{"gain_db":-6.0}}),
        )
        .unwrap();
        let state = DeviceState::from_device(DeviceRole::Fx, device).unwrap();
        let mut input = (state.encode().unwrap(), 0usize);
        let stream = clap_istream {
            ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!(unsafe { load(plugin, &stream) });
        assert_eq!(unsafe { param_count(plugin) }, 1);
        assert_eq!(unsafe { audio_count(plugin, true) }, 2);
        let mut output = Vec::new();
        let stream = clap_ostream {
            ctx: (&mut output as *mut Vec<u8>).cast(),
            write: Some(write),
        };
        assert!(unsafe { save(plugin, &stream) });
        assert_eq!(DeviceState::decode(&output).unwrap(), state);
        assert!(unsafe { activate(plugin, 48000.0, 1, 64) });
        assert!(unsafe { start_processing(plugin) });
        let mut in_l = [1.0f32; 8];
        let mut in_r = [1.0f32; 8];
        let mut out_l = [0.0f32; 8];
        let mut out_r = [0.0f32; 8];
        let mut in_ptrs = [in_l.as_mut_ptr(), in_r.as_mut_ptr()];
        let mut out_ptrs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        let in_buffer = clap_audio_buffer {
            data32: in_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut out_buffer = clap_audio_buffer {
            data32: out_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut block = clap_process {
            steady_time: 0,
            frames_count: 8,
            transport: ptr::null(),
            audio_inputs: &in_buffer,
            audio_outputs: &mut out_buffer,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: ptr::null(),
            out_events: ptr::null(),
        };
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        assert!(out_l.iter().all(|x| (*x - 0.501187).abs() < 0.001));
        // The permanent FX detector input is ignored by effects that do not
        // request a sidechain, even if the host supplies audio on that port.
        let mut detector_l = [1.0f32; 8];
        let mut detector_r = [1.0f32; 8];
        let mut detector_ptrs = [detector_l.as_mut_ptr(), detector_r.as_mut_ptr()];
        let inputs = [
            in_buffer,
            clap_audio_buffer {
                data32: detector_ptrs.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            },
        ];
        block.audio_inputs = inputs.as_ptr();
        block.audio_inputs_count = 2;
        out_l.fill(0.0);
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        assert!(out_l.iter().all(|x| (*x - 0.501187).abs() < 0.001));
        let plugin_address = plugin as usize;
        std::thread::scope(|scope| {
            let reader = scope.spawn(move || {
                let plugin = plugin_address as *const clap_plugin;
                for _ in 0..128 {
                    assert_eq!(unsafe { param_count(plugin) }, 1);
                    let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
                    assert!(unsafe { param_info(plugin, 0, info.as_mut_ptr()) });
                    let id = unsafe { info.assume_init() }.id;
                    let mut value = f64::NAN;
                    assert!(unsafe { param_value(plugin, id, &mut value) });
                    assert_eq!(value, -6.0);
                    let mut bytes = Vec::new();
                    let stream = clap_ostream {
                        ctx: (&mut bytes as *mut Vec<u8>).cast(),
                        write: Some(write),
                    };
                    assert!(unsafe { save(plugin, &stream) });
                }
            });
            for _ in 0..128 {
                assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
            }
            reader.join().unwrap();
        });
        let mut reentrant = ReentrantWrite {
            bytes: Vec::new(),
            plugin,
            block: &block,
            process_status: None,
        };
        let stream = clap_ostream {
            ctx: (&mut reentrant as *mut ReentrantWrite).cast(),
            write: Some(write_while_processing),
        };
        assert!(unsafe { save(plugin, &stream) });
        assert_eq!(reentrant.process_status, Some(CLAP_PROCESS_CONTINUE));
        assert_eq!(DeviceState::decode(&reentrant.bytes).unwrap(), state);
        unsafe {
            stop_processing(plugin);
            deactivate(plugin);
            destroy(plugin);
        }
    }
    #[test]
    fn descriptor_ids_match_exported_device_roles() {
        assert_eq!(
            &INSTRUMENT_ID_C[..INSTRUMENT_ID_C.len() - 1],
            INSTRUMENT_ID.as_bytes()
        );
        assert_eq!(&FX_ID_C[..FX_ID_C.len() - 1], FX_ID.as_bytes());
    }

    #[test]
    fn fresh_instances_save_state_before_host_load() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        for (id, role) in [
            (INSTRUMENT_ID_C, DeviceRole::Instrument),
            (FX_ID_C, DeviceRole::Fx),
        ] {
            let plugin = unsafe { factory_create(&FACTORY, &host, id.as_ptr().cast()) };
            assert!(!plugin.is_null());
            let mut bytes = Vec::new();
            let stream = clap_ostream {
                ctx: (&mut bytes as *mut Vec<u8>).cast(),
                write: Some(write),
            };
            assert!(unsafe { save(plugin, &stream) });
            let state = DeviceState::decode(&bytes).unwrap();
            assert_eq!(state.role, role);
            assert!(!state.parameters.is_empty());
            unsafe {
                destroy(plugin);
            }
        }
    }
    #[test]
    fn duplicate_instruments_keep_state_and_voices_independent() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let a = unsafe { factory_create(&FACTORY, &host, INSTRUMENT_ID_C.as_ptr().cast()) };
        let b = unsafe { factory_create(&FACTORY, &host, INSTRUMENT_ID_C.as_ptr().cast()) };
        assert!(!a.is_null() && !b.is_null() && a != b);
        let device = serde_json::from_value(serde_json::json!({
            "id":"s", "kind":"builtin.studio_synth", "params":{"gain_db":0.0}
        }))
        .unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        let gain = state
            .parameters
            .iter()
            .find(|p| p.path == "gain_db")
            .unwrap()
            .id;
        let bytes = state.encode().unwrap();
        let load_bytes = |plugin, bytes: Vec<u8>| {
            let mut input = (bytes, 0usize);
            let stream = clap_istream {
                ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
                read: Some(read),
            };
            assert!(unsafe { load(plugin, &stream) });
        };
        let save_bytes = |plugin| {
            let mut bytes = Vec::new();
            let stream = clap_ostream {
                ctx: (&mut bytes as *mut Vec<u8>).cast(),
                write: Some(write),
            };
            assert!(unsafe { save(plugin, &stream) });
            bytes
        };
        let set_gain = |plugin, value| {
            let mut events = vec![clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: gain,
                cookie: ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            }];
            let list = clap_input_events {
                ctx: (&mut events as *mut Vec<clap_event_param_value>).cast(),
                size: Some(parameter_event_size),
                get: Some(parameter_event_get),
            };
            unsafe { flush(plugin, &list, ptr::null()) };
        };
        let gain_value = |plugin| {
            let mut value = f64::NAN;
            assert!(unsafe { param_value(plugin, gain, &mut value) });
            value
        };
        load_bytes(a, bytes.clone());
        load_bytes(b, bytes);
        set_gain(a, -12.0);
        assert_eq!(gain_value(a), -12.0);
        assert_eq!(gain_value(b), 0.0);
        let saved_a = save_bytes(a);
        let saved_b = save_bytes(b);
        assert_eq!(
            DeviceState::decode(&saved_a).unwrap().device.params["gain_db"],
            -12.0
        );
        assert_eq!(
            DeviceState::decode(&saved_b).unwrap().device.params["gain_db"],
            0.0
        );

        for plugin in [a, b] {
            assert!(unsafe { activate(plugin, 48000.0, 1, 256) });
            assert!(unsafe { start_processing(plugin) });
        }
        let render = |plugin, notes: &mut Vec<clap_event_note>| {
            let mut left = [0.0f32; 256];
            let mut right = [0.0f32; 256];
            let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
            let mut output = clap_audio_buffer {
                data32: channels.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let list = clap_input_events {
                ctx: (notes as *mut Vec<clap_event_note>).cast(),
                size: Some(event_size),
                get: Some(event_get),
            };
            let block = clap_process {
                steady_time: 0,
                frames_count: 256,
                transport: ptr::null(),
                audio_inputs: ptr::null(),
                audio_outputs: &mut output,
                audio_inputs_count: 0,
                audio_outputs_count: 1,
                in_events: &list,
                out_events: ptr::null(),
            };
            assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
            left
        };
        let sounding = render(a, &mut vec![note(CLAP_EVENT_NOTE_ON, 0, 41)]);
        let silent = render(b, &mut Vec::new());
        assert!(sounding.iter().any(|sample| sample.abs() > 1.0e-5));
        assert!(silent.iter().all(|sample| *sample == 0.0));
        for plugin in [a, b] {
            unsafe {
                stop_processing(plugin);
                deactivate(plugin);
            }
        }
        set_gain(a, -24.0);
        set_gain(b, -6.0);
        load_bytes(a, saved_a);
        load_bytes(b, saved_b);
        assert_eq!(gain_value(a), -12.0);
        assert_eq!(gain_value(b), 0.0);
        unsafe {
            destroy(a);
            destroy(b);
        }
    }
    #[test]
    fn compressor_detector_port_drives_gain_reduction() {
        let signals = HostSignals {
            rescans: AtomicUsize::new(0),
            param_rescans: AtomicUsize::new(0),
            restarts: AtomicUsize::new(0),
        };
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: (&signals as *const HostSignals).cast_mut().cast(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: Some(host_extension),
            request_restart: Some(test_restart),
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, FX_ID_C.as_ptr().cast()) };
        assert_eq!(unsafe { param_count(plugin) }, 1);
        assert_eq!(unsafe { audio_count(plugin, true) }, 2);
        let mut initial_detector = std::mem::MaybeUninit::<clap_audio_port_info>::uninit();
        assert!(unsafe { audio_get(plugin, 1, true, initial_detector.as_mut_ptr()) });
        let initial_detector = unsafe { initial_detector.assume_init() };
        assert_eq!(initial_detector.id, 2);
        assert_eq!(initial_detector.flags & CLAP_AUDIO_PORT_IS_MAIN, 0);
        // Some hosts activate their default instance before applying the
        // imported preset. The first external load is still initialization.
        assert!(unsafe { activate(plugin, 48000.0, 1, 256) });
        unsafe {
            deactivate(plugin);
        }
        let device = serde_json::from_value(serde_json::json!({
            "id":"duck", "kind":"builtin.compressor", "sidechain":"key",
            "params":{"threshold_db":-30.0,"ratio":20.0,"knee_db":0.0,"attack_ms":0.1,"release_ms":10.0}
        })).unwrap();
        let state = DeviceState::from_device(DeviceRole::Fx, device).unwrap();
        assert!(state.source.contains("sidechain"));
        let mut input = (state.encode().unwrap(), 0usize);
        let stream = clap_istream {
            ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!(unsafe { load(plugin, &stream) });
        assert_eq!(signals.rescans.load(Ordering::Relaxed), 0);
        assert_eq!(signals.param_rescans.load(Ordering::Relaxed), 0);
        assert_eq!(signals.restarts.load(Ordering::Relaxed), 0);
        assert_eq!(unsafe { audio_count(plugin, true) }, 2);
        assert!(unsafe { param_count(plugin) } > 1);
        let mut info = std::mem::MaybeUninit::<clap_audio_port_info>::uninit();
        assert!(unsafe { audio_get(plugin, 1, true, info.as_mut_ptr()) });
        let info = unsafe { info.assume_init() };
        assert_eq!(info.id, initial_detector.id);
        assert_eq!(info.flags & CLAP_AUDIO_PORT_IS_MAIN, 0);
        assert_eq!(
            unsafe { CStr::from_ptr(info.name.as_ptr()) }
                .to_str()
                .unwrap(),
            "Detector"
        );
        assert!(unsafe { activate(plugin, 48000.0, 1, 256) });
        assert!(unsafe { start_processing(plugin) });
        let mut main_l = [0.5f32; 256];
        let mut main_r = [0.5f32; 256];
        let mut det_l = [0.0f32; 256];
        let mut det_r = [0.0f32; 256];
        let mut out_l = [0.0f32; 256];
        let mut out_r = [0.0f32; 256];
        let mut main_ptrs = [main_l.as_mut_ptr(), main_r.as_mut_ptr()];
        let mut det_ptrs = [det_l.as_mut_ptr(), det_r.as_mut_ptr()];
        let mut out_ptrs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        let inputs = [
            clap_audio_buffer {
                data32: main_ptrs.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            },
            clap_audio_buffer {
                data32: det_ptrs.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            },
        ];
        let mut output = clap_audio_buffer {
            data32: out_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut block = clap_process {
            steady_time: 0,
            frames_count: 256,
            transport: ptr::null(),
            audio_inputs: inputs.as_ptr(),
            audio_outputs: &mut output,
            audio_inputs_count: 2,
            audio_outputs_count: 1,
            in_events: ptr::null(),
            out_events: ptr::null(),
        };
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        let dry = out_l[255];
        assert!((dry - 0.5).abs() < 1.0e-4);
        det_l.fill(1.0);
        det_r.fill(1.0);
        unsafe { reset(plugin) };
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        assert!(
            out_l[255] < 0.1,
            "detector must compress main input: {}",
            out_l[255]
        );
        block.audio_inputs_count = 1;
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_ERROR);
        unsafe {
            stop_processing(plugin);
            deactivate(plugin);
        }
        input.1 = 0;
        assert!(unsafe { load(plugin, &stream) });
        assert_eq!(signals.rescans.load(Ordering::Relaxed), 0);
        assert_eq!(signals.param_rescans.load(Ordering::Relaxed), 1);
        assert_eq!(signals.restarts.load(Ordering::Relaxed), 1);
        unsafe {
            destroy(plugin);
        }
    }
    #[test]
    fn rack_state_runs_same_impulse_as_shared_processor() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, FX_ID_C.as_ptr().cast()) };
        let authored = r#"let main = {type:"rack",branches:[
            [{type:"fx",name:"gain",id:"a",gain_db:-6},{type:"fx",name:"gain",id:"b",gain_db:-3}],
            [{type:"fx",name:"limiter",id:"lim",lookahead_ms:5ms,ceiling_db:0}]
        ],expose:{trim:"0.0.gain_db"},modulate:[{target:"0.1.gain_db",base:-3,depth:1,rate_hz:1,min:-6,max:0}],trim:-6,mix:0.75,gain_db:-3};"#;
        let device = muz::device_state::reconstruct(authored, "r").unwrap();
        let state = DeviceState::from_device(DeviceRole::Fx, device).unwrap();
        let mut input = (state.encode().unwrap(), 0usize);
        let stream = clap_istream {
            ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!(unsafe { load(plugin, &stream) });
        assert_eq!(unsafe { param_count(plugin) }, 3);
        let trim = state.parameters.iter().find(|p| p.path == "trim").unwrap();
        let mut trim_info = std::mem::MaybeUninit::<clap_param_info>::uninit();
        let trim_index = state
            .parameters
            .iter()
            .position(|p| p.path == "trim")
            .unwrap();
        assert!(unsafe { param_info(plugin, trim_index as u32, trim_info.as_mut_ptr()) });
        let trim_info = unsafe { trim_info.assume_init() };
        assert_eq!(
            (
                trim_info.min_value,
                trim_info.max_value,
                trim_info.default_value
            ),
            (-120.0, 24.0, -6.0)
        );
        assert!(unsafe { activate(plugin, 48000.0, 1, 512) });
        assert_eq!(unsafe { latency_get(plugin) }, 240);
        assert!(unsafe { start_processing(plugin) });
        let mut in_l = [0.0f32; 512];
        in_l[0] = 1.0;
        let mut in_r = in_l;
        let mut out_l = [0.0f32; 512];
        let mut out_r = [0.0f32; 512];
        let mut in_ptrs = [in_l.as_mut_ptr(), in_r.as_mut_ptr()];
        let mut out_ptrs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        let in_buffer = clap_audio_buffer {
            data32: in_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut out_buffer = clap_audio_buffer {
            data32: out_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut block = clap_process {
            steady_time: 0,
            frames_count: 512,
            transport: ptr::null(),
            audio_inputs: &in_buffer,
            audio_outputs: &mut out_buffer,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: ptr::null(),
            out_events: ptr::null(),
        };
        let snapshot = {
            let p = unsafe { instance(plugin) }.unwrap();
            transport(&p, ptr::null())
        };
        let mut direct = make_processor(
            &state,
            &[],
            AudioConfig {
                sample_rate: 48000.0,
                max_frames: 512,
                offline: false,
            },
        )
        .unwrap();
        let mut expected_l = in_l;
        let mut expected_r = in_r;
        direct
            .process(
                ProcessContext {
                    frames: 512,
                    block_start_sample: 0,
                    transport: snapshot,
                },
                &[],
                &mut expected_l,
                &mut expected_r,
            )
            .unwrap();
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        assert_eq!(out_l, expected_l);
        assert_eq!(out_r, expected_r);
        assert!(out_l[240].abs() > 0.1);
        let a_baseline = out_l[240];
        unsafe { reset(plugin) };
        direct.reset();
        direct.set_parameter("trim", -12.0).unwrap();
        out_l.fill(0.0);
        out_r.fill(0.0);
        expected_l.copy_from_slice(&in_l);
        expected_r.copy_from_slice(&in_r);
        let mut events = vec![clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: trim.id,
            cookie: ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value: -12.0,
        }];
        let list = clap_input_events {
            ctx: (&mut events as *mut Vec<clap_event_param_value>).cast(),
            size: Some(parameter_event_size),
            get: Some(parameter_event_get),
        };
        block.in_events = &list;
        direct
            .process(
                ProcessContext {
                    frames: 512,
                    block_start_sample: 0,
                    transport: snapshot,
                },
                &[],
                &mut expected_l,
                &mut expected_r,
            )
            .unwrap();
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        assert_eq!(out_l, expected_l);
        assert!(out_l[240] < 0.9 * a_baseline);
        let mix = state.parameters.iter().find(|p| p.path == "mix").unwrap();
        let mix_index = state
            .parameters
            .iter()
            .position(|p| p.path == "mix")
            .unwrap();
        let mut mix_info = std::mem::MaybeUninit::<clap_param_info>::uninit();
        assert!(unsafe { param_info(plugin, mix_index as u32, mix_info.as_mut_ptr()) });
        let mix_info = unsafe { mix_info.assume_init() };
        assert_eq!(
            (
                mix_info.min_value,
                mix_info.max_value,
                mix_info.default_value
            ),
            (0.0, 1.0, 1.0)
        );
        unsafe { reset(plugin) };
        direct.reset();
        direct.set_parameter("mix", 0.0).unwrap();
        events[0].param_id = mix.id;
        events[0].value = 0.0;
        out_l.fill(0.0);
        out_r.fill(0.0);
        expected_l.copy_from_slice(&in_l);
        expected_r.copy_from_slice(&in_r);
        direct
            .process(
                ProcessContext {
                    frames: 512,
                    block_start_sample: 0,
                    transport: snapshot,
                },
                &[],
                &mut expected_l,
                &mut expected_r,
            )
            .unwrap();
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        assert_eq!(out_l, expected_l);
        assert!((out_l[240] - 10f32.powf(-3.0 / 20.0)).abs() < 1e-5);
        unsafe {
            stop_processing(plugin);
            deactivate(plugin);
            destroy(plugin);
        }
    }
    #[test]
    fn default_gain_is_enumerated_with_native_range_and_value() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, FX_ID_C.as_ptr().cast()) };
        let device = serde_json::from_value(
            serde_json::json!({"id":"g", "kind":"builtin.gain", "params":{}}),
        )
        .unwrap();
        let state = DeviceState::from_device(DeviceRole::Fx, device).unwrap();
        let mut input = (state.encode().unwrap(), 0usize);
        let stream = clap_istream {
            ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!(unsafe { load(plugin, &stream) });
        assert_eq!(unsafe { param_count(plugin) }, 1);
        let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
        assert!(unsafe { param_info(plugin, 0, info.as_mut_ptr()) });
        let info = unsafe { info.assume_init() };
        assert_eq!(info.id, state.parameters[0].id);
        assert_eq!(info.min_value, -120.0);
        assert_eq!(info.max_value, 24.0);
        assert_eq!(info.default_value, 0.0);
        assert_eq!(
            unsafe { CStr::from_ptr(info.name.as_ptr()) }
                .to_str()
                .unwrap(),
            "gain_db"
        );
        let mut value = f64::NAN;
        assert!(unsafe { param_value(plugin, info.id, &mut value) });
        assert_eq!(value, 0.0);
        // Host metadata and state callbacks may run while the audio runtime is busy.
        let runtime_guard = unsafe { plugin_instance(plugin) }
            .unwrap()
            .inner
            .lock()
            .unwrap();
        assert_eq!(unsafe { param_count(plugin) }, 1);
        let mut busy_info = std::mem::MaybeUninit::<clap_param_info>::uninit();
        assert!(unsafe { param_info(plugin, 0, busy_info.as_mut_ptr()) });
        assert!(unsafe { param_value(plugin, info.id, &mut value) });
        let mut saved = Vec::new();
        let stream = clap_ostream {
            ctx: (&mut saved as *mut Vec<u8>).cast(),
            write: Some(write),
        };
        assert!(unsafe { save(plugin, &stream) });
        assert_eq!(DeviceState::decode(&saved).unwrap(), state);
        drop(runtime_guard);
        unsafe {
            destroy(plugin);
        }
    }
    #[test]
    fn voice_patch_control_info_and_cached_validation() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, INSTRUMENT_ID_C.as_ptr().cast()) };
        let device = serde_json::from_value(serde_json::json!({"id":"p","kind":"builtin.voice_patch","params":{},"patch":{"type":"voice_patch","name":"p","nodes":[{"id":"level","op":"param","value":0.2,"min":0.0,"max":1.0}],"output":"level"}})).unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        let mut input = (state.encode().unwrap(), 0usize);
        let stream = clap_istream {
            ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!(unsafe { load(plugin, &stream) });
        let index = state
            .parameters
            .iter()
            .position(|p| p.path == "level")
            .unwrap();
        let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
        assert!(unsafe { param_info(plugin, index as u32, info.as_mut_ptr()) });
        let info = unsafe { info.assume_init() };
        assert_eq!((info.min_value, info.max_value), (0.0, 1.0));
        assert!((info.default_value - 0.2).abs() < 1.0e-6);
        let mut events = vec![clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: info.id,
            cookie: ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value: 0.75,
        }];
        let list = clap_input_events {
            ctx: (&mut events as *mut Vec<clap_event_param_value>).cast(),
            size: Some(parameter_event_size),
            get: Some(parameter_event_get),
        };
        unsafe {
            flush(plugin, &list, ptr::null());
        }
        let mut value = -1.0;
        assert!(unsafe { param_value(plugin, info.id, &mut value) });
        assert_eq!(value, 0.75);
        let mut saved = Vec::new();
        let stream = clap_ostream {
            ctx: (&mut saved as *mut Vec<u8>).cast(),
            write: Some(write),
        };
        assert!(unsafe { save(plugin, &stream) });
        let restored = DeviceState::decode(&saved).unwrap();
        assert_eq!(restored.parameters[index].value, 0.75);
        assert_eq!(restored.device.params["level"], 0.75);
        unsafe {
            destroy(plugin);
        }
    }
    #[test]
    fn natural_note_end_retries_failed_host_push() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, INSTRUMENT_ID_C.as_ptr().cast()) };
        let device = serde_json::from_value(serde_json::json!({"id":"s", "kind":"builtin.studio_synth", "params":{"mode":6.0,"decay_ms":1.0}})).unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        let mut input = (state.encode().unwrap(), 0usize);
        let stream = clap_istream {
            ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!(unsafe { load(plugin, &stream) });
        assert!(unsafe { activate(plugin, 48000.0, 1, 256) });
        assert!(unsafe { start_processing(plugin) });
        let mut left = [0.0f32; 256];
        let mut right = [0.0f32; 256];
        let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut output = clap_audio_buffer {
            data32: channels.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut note_events = vec![note(CLAP_EVENT_NOTE_ON, 0, 42)];
        let list = clap_input_events {
            ctx: (&mut note_events as *mut Vec<clap_event_note>).cast(),
            size: Some(event_size),
            get: Some(event_get),
        };
        let mut ends = EndSink {
            reject_once: true,
            ..Default::default()
        };
        let output_events = clap_output_events {
            ctx: (&mut ends as *mut EndSink).cast(),
            try_push: Some(push_end),
        };
        let block = clap_process {
            steady_time: 0,
            frames_count: 256,
            transport: ptr::null(),
            audio_inputs: ptr::null(),
            audio_outputs: &mut output,
            audio_inputs_count: 0,
            audio_outputs_count: 1,
            in_events: &list,
            out_events: &output_events,
        };
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        note_events.clear();
        for _ in 0..16 {
            assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        }
        assert_eq!(ends.rejected, 1);
        assert_eq!(ends.events.len(), 1);
        let end = ends.events[0];
        assert_eq!(
            (end.note_id, end.port_index, end.channel, end.key),
            (42, 0, 0, 60)
        );
        assert_eq!(end.header.time, 255);
        assert_eq!(unsafe { instance(plugin) }.unwrap().active_notes.len(), 0);
        unsafe {
            stop_processing(plugin);
            deactivate(plugin);
            destroy(plugin);
        }
    }
    #[test]
    fn sfz_embedded_state_accepts_cc_and_isolated_note_expression_through_clap() {
        let dir = std::env::temp_dir().join(format!(
            "muz-clap-sfz-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36u32 + 4096).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48000u32.to_le_bytes());
        wav.extend_from_slice(&96000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4096u32.to_le_bytes());
        for _ in 0..2048 {
            wav.extend_from_slice(&8192i16.to_le_bytes());
        }
        std::fs::write(dir.join("sample.wav"), wav).unwrap();
        std::fs::write(dir.join("program.sfz"), "<control> set_cc11=127\n<region> sample=sample.wav key=60 loop_mode=loop_continuous loop_start=0 loop_end=127 amplitude=100 amplitude_oncc11=100").unwrap();
        let source = format!(
            "let main = sfz({}, {{embed_assets:true,max_voices:16}});",
            serde_json::to_string(&dir.join("program.sfz").display().to_string()).unwrap()
        );
        let device = muz::device_state::reconstruct(&source, "sfz").unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        let encoded = state.encode().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, INSTRUMENT_ID_C.as_ptr().cast()) };
        let mut input = (encoded, 0usize);
        let stream = clap_istream {
            ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!(unsafe { load(plugin, &stream) });
        assert_eq!(unsafe { param_count(plugin) }, 128);
        assert!(unsafe { activate(plugin, 48000., 1, 256) });
        assert!(unsafe { start_processing(plugin) });
        let mut left = [0f32; 256];
        let mut right = [0f32; 256];
        let mut pointers = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut output = clap_audio_buffer {
            data32: pointers.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let first = note(CLAP_EVENT_NOTE_ON, 0, 42);
        let second = note(CLAP_EVENT_NOTE_ON, 0, 43);
        let mut headers = vec![&first.header as *const _, &second.header as *const _];
        let list = clap_input_events {
            ctx: (&mut headers as *mut Vec<*const clap_event_header>).cast(),
            size: Some(header_size),
            get: Some(header_get),
        };
        let block = clap_process {
            steady_time: 0,
            frames_count: 256,
            transport: ptr::null(),
            audio_inputs: ptr::null(),
            audio_outputs: &mut output,
            audio_inputs_count: 0,
            audio_outputs_count: 1,
            in_events: &list,
            out_events: ptr::null(),
        };
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        let mean = |values: &[f32]| values[64..200].iter().copied().sum::<f32>() / 136.;
        let baseline = mean(&left);
        assert!(
            baseline > 0.0001,
            "baseline {baseline}, output {:?}",
            &left[..8]
        );
        let expression = clap_event_note_expression {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_note_expression>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_NOTE_EXPRESSION,
                flags: 0,
            },
            expression_id: CLAP_NOTE_EXPRESSION_VOLUME,
            note_id: 42,
            port_index: 0,
            channel: 0,
            key: 60,
            value: 0.,
        };
        headers.clear();
        headers.push(&expression.header);
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        let isolated = mean(&left);
        assert!(
            (isolated / baseline - 0.5).abs() < 0.001,
            "{isolated} / {baseline}"
        );
        let cc = clap_event_midi {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_midi>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_MIDI,
                flags: 0,
            },
            port_index: 0,
            data: [0xb0, 11, 64],
        };
        headers.clear();
        headers.push(&cc.header);
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        let controlled = mean(&left);
        assert!(
            controlled > 0. && controlled < isolated * 0.75,
            "{controlled} / {isolated}"
        );
        unsafe {
            stop_processing(plugin);
            deactivate(plugin);
            destroy(plugin);
        }
    }
    #[test]
    fn instrument_overlapping_notes_and_wildcard_release() {
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let plugin = unsafe { factory_create(&FACTORY, &host, INSTRUMENT_ID_C.as_ptr().cast()) };
        let device = serde_json::from_value(
            serde_json::json!({"id":"s", "kind":"builtin.studio_synth", "params":{"release_ms":15.0}}),
        )
        .unwrap();
        let state = DeviceState::from_device(DeviceRole::Instrument, device).unwrap();
        let mut input = (state.encode().unwrap(), 0usize);
        let stream = clap_istream {
            ctx: (&mut input as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!(unsafe { load(plugin, &stream) });
        assert_eq!(
            unsafe { param_count(plugin) },
            state.parameters.len() as u32
        );
        assert!(unsafe { activate(plugin, 48000.0, 1, 256) });
        assert!(unsafe { start_processing(plugin) });
        let mut left = [0.0f32; 256];
        let mut right = [0.0f32; 256];
        let mut ptrs = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut output = clap_audio_buffer {
            data32: ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut notes = vec![
            note(CLAP_EVENT_NOTE_ON, 0, -1),
            note(CLAP_EVENT_NOTE_ON, 64, -1),
        ];
        let list = clap_input_events {
            ctx: (&mut notes as *mut Vec<clap_event_note>).cast(),
            size: Some(event_size),
            get: Some(event_get),
        };
        let mut ends = EndSink::default();
        let output_events = clap_output_events {
            ctx: (&mut ends as *mut EndSink).cast(),
            try_push: Some(push_end),
        };
        let mut block = clap_process {
            steady_time: 0,
            frames_count: 256,
            transport: ptr::null(),
            audio_inputs: ptr::null(),
            audio_outputs: &mut output,
            audio_inputs_count: 0,
            audio_outputs_count: 1,
            in_events: &list,
            out_events: &output_events,
        };
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        assert!(left.iter().any(|x| x.abs() > 1.0e-6));
        assert_eq!(unsafe { instance(plugin) }.unwrap().active_notes.len(), 2);
        notes.clear();
        notes.push(note(CLAP_EVENT_NOTE_OFF, 32, -1));
        block.in_events = &list;
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        {
            let p = unsafe { instance(plugin) }.unwrap();
            assert_eq!(p.active_notes.len(), 2);
            assert!(p.active_notes.iter().all(|n| n.released));
        }
        let release_expression = clap_event_note_expression {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_note_expression>() as u32,
                time: 8,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_NOTE_EXPRESSION,
                flags: 0,
            },
            expression_id: CLAP_NOTE_EXPRESSION_VOLUME,
            note_id: -1,
            port_index: 0,
            channel: -1,
            key: -1,
            value: 0.25,
        };
        let release_choke = note(CLAP_EVENT_NOTE_CHOKE, 16, -1);
        let mut release_headers = vec![
            &release_expression.header as *const _,
            &release_choke.header as *const _,
        ];
        let release_list = clap_input_events {
            ctx: (&mut release_headers as *mut Vec<*const clap_event_header>).cast(),
            size: Some(header_size),
            get: Some(header_get),
        };
        block.in_events = &release_list;
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        let mut silence = Vec::<clap_event_note>::new();
        let silence_list = clap_input_events {
            ctx: (&mut silence as *mut Vec<clap_event_note>).cast(),
            size: Some(event_size),
            get: Some(event_get),
        };
        block.in_events = &silence_list;
        for _ in 0..4 {
            assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        }
        assert_eq!(unsafe { instance(plugin) }.unwrap().active_notes.len(), 0);
        assert_eq!(ends.events.len(), 1);
        assert!(
            ends.events
                .iter()
                .all(|e| e.note_id == -1 && e.port_index == 0 && e.channel == 0 && e.key == 60)
        );
        notes.clear();
        notes.push(note(CLAP_EVENT_NOTE_ON, 0, -1));
        notes.push(note(CLAP_EVENT_NOTE_ON, 4, -1));
        block.in_events = &list;
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        assert_eq!(unsafe { instance(plugin) }.unwrap().active_notes.len(), 2);
        let expression = clap_event_note_expression {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_note_expression>() as u32,
                time: 8,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_NOTE_EXPRESSION,
                flags: 0,
            },
            expression_id: CLAP_NOTE_EXPRESSION_VOLUME,
            note_id: -1,
            port_index: 0,
            channel: -1,
            key: -1,
            value: 0.25,
        };
        let choke = note(CLAP_EVENT_NOTE_CHOKE, 16, -1);
        let mut headers = vec![
            &expression.header as *const clap_event_header,
            &choke.header as *const clap_event_header,
        ];
        let list = clap_input_events {
            ctx: (&mut headers as *mut Vec<*const clap_event_header>).cast(),
            size: Some(header_size),
            get: Some(header_get),
        };
        block.in_events = &list;
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        block.in_events = &silence_list;
        for _ in 0..4 {
            assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        }
        assert_eq!(unsafe { instance(plugin) }.unwrap().active_notes.len(), 0);
        assert_eq!(ends.events.len(), 2);
        // The old wildcard-addressed release tail ends after a retrigger.
        // Its NOTE_END must wait while the newer same-key host voice sounds.
        let retrigger_list = clap_input_events {
            ctx: (&mut notes as *mut Vec<clap_event_note>).cast(),
            size: Some(event_size),
            get: Some(event_get),
        };
        notes.clear();
        notes.push(note(CLAP_EVENT_NOTE_ON, 0, -1));
        block.in_events = &retrigger_list;
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        notes.clear();
        notes.push(note(CLAP_EVENT_NOTE_OFF, 0, -1));
        notes.push(note(CLAP_EVENT_NOTE_ON, 4, -1));
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        block.in_events = &silence_list;
        for _ in 0..12 {
            assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        }
        assert_eq!(ends.events.len(), 2);
        assert_eq!(
            unsafe { instance(plugin) }
                .unwrap()
                .active_notes
                .iter()
                .filter(|n| !n.ended)
                .count(),
            1
        );
        notes.clear();
        notes.push(note(CLAP_EVENT_NOTE_OFF, 0, -1));
        block.in_events = &retrigger_list;
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        block.in_events = &silence_list;
        for _ in 0..12 {
            assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        }
        assert_eq!(ends.events.len(), 3);
        assert_eq!(unsafe { instance(plugin) }.unwrap().active_notes.len(), 0);
        // Repeated same-key retriggers leave no silence for the wildcard end.
        // Coalescing must keep pending host identity storage bounded.
        notes.clear();
        notes.push(note(CLAP_EVENT_NOTE_ON, 0, -1));
        block.in_events = &retrigger_list;
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        for _ in 0..audio::MAX_ACTIVE_NOTES + 44 {
            notes.clear();
            notes.push(note(CLAP_EVENT_NOTE_OFF, 0, -1));
            notes.push(note(CLAP_EVENT_NOTE_ON, 4, -1));
            assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
            assert!(unsafe { instance(plugin) }.unwrap().active_notes.len() < 32);
        }
        assert_eq!(ends.events.len(), 3);
        notes.clear();
        notes.push(note(CLAP_EVENT_NOTE_OFF, 0, -1));
        assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        block.in_events = &silence_list;
        for _ in 0..12 {
            assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
        }
        assert_eq!(ends.events.len(), 4);
        assert_eq!(unsafe { instance(plugin) }.unwrap().active_notes.len(), 0);
        // Finished voices retire their addresses; a long sequential melody
        // must not exhaust the bounded note table.
        notes.clear();
        let many_notes = clap_input_events {
            ctx: (&mut notes as *mut Vec<clap_event_note>).cast(),
            size: Some(event_size),
            get: Some(event_get),
        };
        block.in_events = &many_notes;
        for _ in 0..audio::MAX_ACTIVE_NOTES + 44 {
            notes.clear();
            notes.push(note(CLAP_EVENT_NOTE_ON, 0, -1));
            notes.push(note(CLAP_EVENT_NOTE_OFF, 1, -1));
            assert_eq!(unsafe { process(plugin, &block) }, CLAP_PROCESS_CONTINUE);
            assert!(
                unsafe { instance(plugin) }.unwrap().active_notes.len() < audio::MAX_ACTIVE_NOTES
            );
        }
        unsafe {
            stop_processing(plugin);
            deactivate(plugin);
            destroy(plugin);
        }
    }
}
