//! Prepared SFZ playback. Importing, decoding, and opcode compilation are worker
//! operations; event handling and rendering use bounded, preallocated storage.
use super::sfz_dsp::{RegionDsp, VoiceDsp};
use super::*;
use std::{collections::BTreeMap, sync::Arc};

const NOTE_CAPACITY: usize = 1024;
const MAX_REGIONS: usize = 65536;
#[derive(Clone, Copy, Debug, PartialEq)]
enum Trigger {
    Attack,
    First,
    Legato,
    Release,
    ReleaseKey,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum LoopMode {
    None,
    OneShot,
    Continuous,
    Sustain,
}
#[derive(Clone, Copy)]
struct Gate {
    cc: usize,
    low: f32,
    high: f32,
}
#[derive(Clone)]
struct RuntimeParam {
    base: f64,
    routes: Vec<(usize, f64, Option<[f32; 128]>)>,
}
impl RuntimeParam {
    fn compile(
        op: &BTreeMap<String, String>,
        name: &str,
        default: f64,
        curves: &BTreeMap<u16, Vec<f32>>,
    ) -> Result<Self, DeviceError> {
        let base = finite(op, name, default)?;
        let mut routes = Vec::new();
        for cc in 0..128 {
            let a = format!("{name}_oncc{cc}");
            let b = format!("{name}_cc{cc}");
            if op.contains_key(&a) && op.contains_key(&b) {
                return Err(error("ambiguous controller aliases"));
            }
            if op.contains_key(&a) || op.contains_key(&b) {
                let depth = finite(op, if op.contains_key(&a) { &a } else { &b }, 0.)?;
                let curve = format!("{name}_curvecc{cc}");
                let id = bounded(op, &curve, 0., 0., 255., true)? as u16;
                let table = if id == 0 && !curves.contains_key(&0) {
                    None
                } else {
                    Some(
                        super::sfz_dsp::curve_table(id, curves)
                            .map_err(|e| error(format!("{name}: {e}")))?,
                    )
                };
                routes.push((cc, depth, table));
            }
        }
        Ok(Self { base, routes })
    }
    fn get(&self, cc: &[f32; 128]) -> f64 {
        self.base
            + self
                .routes
                .iter()
                .map(|(i, d, c)| {
                    let value = cc[*i];
                    let value = if let Some(c) = c {
                        let p = value.clamp(0., 1.) * 127.;
                        let i = p.floor() as usize;
                        let j = (i + 1).min(127);
                        c[i] + (c[j] - c[i]) * (p - i as f32)
                    } else {
                        value
                    };
                    *d * value as f64
                })
                .sum::<f64>()
    }
}
struct Region {
    sample: Option<usize>,
    key: [u8; 2],
    velocity: [u8; 2],
    trigger: Trigger,
    gates: Vec<Gate>,
    on_gates: Vec<Gate>,
    switch: Option<u8>,
    previous: Option<u8>,
    switch_range: Option<[u8; 2]>,
    root: f64,
    tune: f64,
    transpose: f64,
    keytrack: f64,
    offset: RuntimeParam,
    offset_random: f64,
    end: f64,
    loops: Option<[f64; 2]>,
    loop_mode: LoopMode,
    delay: RuntimeParam,
    delay_random: f64,
    pitch_random: f64,
    amp_random: f32,
    rt_decay: f32,
    bend_up: f64,
    bend_down: f64,
    sequence: usize,
    sequence_len: u64,
    sequence_pos: u64,
    random: [f64; 2],
    group: i64,
    off_by: i64,
    off_fast: bool,
    off_time: f32,
    polyphony: usize,
    note_polyphony: usize,
    sustain_cc: usize,
    dsp: Arc<RegionDsp>,
}
struct Sample {
    audio: Arc<[[f32; 2]]>,
    rate: f64,
    loops: Option<[f64; 2]>,
}
fn decoded_bytes(frames: usize) -> Result<usize, DeviceError> {
    let bytes = frames
        .checked_mul(std::mem::size_of::<[f32; 2]>())
        .ok_or_else(|| error("decoded SFZ sample footprint overflow"))?;
    if bytes > isize::MAX as usize {
        return Err(error(
            "decoded SFZ samples exceed platform address-space allocation bound",
        ));
    }
    Ok(bytes)
}
fn sample_loops(path: &std::path::Path) -> Result<Option<[f64; 2]>, DeviceError> {
    if !path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("wav"))
    {
        return Ok(None);
    }
    let asset = crate::assets::snapshot(path, 512 * 1024 * 1024).map_err(error)?;
    let b = &asset.bytes;
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Ok(None);
    }
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let size = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        let end = start
            .checked_add(size)
            .ok_or_else(|| error("WAV chunk overflow"))?;
        if end > b.len() {
            return Err(error("truncated WAV metadata"));
        }
        if &b[pos..pos + 4] == b"smpl" && size >= 60 {
            let count = u32::from_le_bytes(b[start + 28..start + 32].try_into().unwrap());
            if count > 0 {
                let ty = u32::from_le_bytes(b[start + 40..start + 44].try_into().unwrap());
                if ty != 0 {
                    return Err(error("only forward WAV loops are supported"));
                }
                let a = u32::from_le_bytes(b[start + 44..start + 48].try_into().unwrap()) as f64;
                let z =
                    u32::from_le_bytes(b[start + 48..start + 52].try_into().unwrap()) as f64 + 1.;
                return Ok(Some([a, z]));
            }
        }
        pos = end
            .checked_add(size % 2)
            .ok_or_else(|| error("WAV chunk overflow"))?;
    }
    Ok(None)
}
#[derive(Clone, Copy)]
struct Note {
    active: bool,
    down: bool,
    sustained: bool,
    id: u64,
    channel: u8,
    key: u8,
    velocity: u8,
    pitch: f32,
    started_frame: u64,
    #[cfg(test)]
    age: u64,
    volume: f32,
    expression: f32,
    pan: [f32; 2],
    tuning: f64,
}
impl Default for Note {
    fn default() -> Self {
        Self {
            active: false,
            down: false,
            sustained: false,
            id: 0,
            channel: 0,
            key: 0,
            velocity: 0,
            pitch: 0.,
            started_frame: 0,
            #[cfg(test)]
            age: 0,
            volume: 1.,
            expression: 1.,
            pan: [1.; 2],
            tuning: 1.,
        }
    }
}
#[derive(Clone)]
struct Voice {
    active: bool,
    note: usize,
    region: usize,
    pos: f64,
    step: f64,
    cached_pitch_bits: Option<u64>,
    cached_pitch_multiplier: f64,
    delay: u64,
    age: u64,
    released: bool,
    choked: bool,
    choke_gain: f32,
    gain: f32,
    dsp: Option<VoiceDsp>,
    looped: bool,
    choke_time: f32,
}
impl Default for Voice {
    fn default() -> Self {
        Self {
            choke_time: 0.008,
            looped: false,
            active: false,
            note: 0,
            region: 0,
            pos: 0.,
            step: 1.,
            cached_pitch_bits: None,
            cached_pitch_multiplier: 1.,
            delay: 0,
            age: 0,
            released: false,
            choked: false,
            choke_gain: 1.,
            gain: 1.,
            dsp: None,
        }
    }
}
/// Captured only during preparation; restoring copies bounded playback state.
/// It never owns or replaces recordings, regions, or compiled modulation routes.
struct SfzCheckpoint {
    voices: Vec<Voice>,
    notes: Box<[Note; NOTE_CAPACITY]>,
    cc: [[f32; 128]; 16],
    switches: [Option<u8>; 16],
    previous: [Option<u8>; 16],
    musical_previous: [Option<u8>; 16],
    bend: [f64; 16],
    virtual_cc: [[f32; 16]; 16],
    sequence: Vec<[u64; 16]>,
    frame_clock: u64,
    rng: u64,
    gain: f32,
}
pub struct Sfz {
    regions: Vec<Region>,
    samples: Vec<Sample>,
    voices: Vec<Voice>,
    notes: Box<[Note; NOTE_CAPACITY]>,
    cc: [[f32; 128]; 16],
    defaults: [f32; 128],
    switches: [Option<u8>; 16],
    switch_default: Option<u8>,
    switch_keys: [bool; 128],
    previous: [Option<u8>; 16],
    musical_previous: [Option<u8>; 16],
    bend: [f64; 16],
    virtual_cc: [[f32; 16]; 16],
    sequence: Vec<[u64; 16]>,
    sequence_used: Vec<bool>,
    frame_clock: u64,
    rng: u64,
    seed: u64,
    sample_rate: f64,
    core: ProcessorCore,
    gain: f32,
    region_activity: Vec<u64>,
    checkpoint: Option<Box<SfzCheckpoint>>,
    #[cfg(test)]
    force_pitch_recalculation: bool,
    #[cfg(test)]
    force_legacy_note_age: bool,
    statistics: SfzStatistics,
    parameter_events: [(u32, usize, f32); 1024],
    parameter_count: usize,
}
fn error(message: impl std::fmt::Display) -> DeviceError {
    eprintln!("SFZ: {message}");
    DeviceError::InvalidConfig("SFZ preparation failed; see diagnostic")
}
fn finite(op: &BTreeMap<String, String>, name: &str, default: f64) -> Result<f64, DeviceError> {
    let n = op
        .get(name)
        .map_or(Ok(default), |s| s.parse::<f64>().map_err(error))?;
    if n.is_finite() {
        Ok(n)
    } else {
        Err(error(format!("non-finite {name}")))
    }
}
fn bounded(
    op: &BTreeMap<String, String>,
    name: &str,
    default: f64,
    min: f64,
    max: f64,
    integer: bool,
) -> Result<f64, DeviceError> {
    let value = finite(op, name, default)?;
    if value < min || value > max || (integer && value.fract() != 0.) {
        return Err(error(format!(
            "invalid {name}={value}; expected {}..{}{}",
            min,
            max,
            if integer { " integer" } else { "" }
        )));
    }
    Ok(value)
}
fn key(op: &BTreeMap<String, String>, name: &str, default: u8) -> Result<u8, DeviceError> {
    let Some(value) = op.get(name) else {
        return Ok(default);
    };
    let n = crate::sfz::parse_key(value).map_err(error)?;
    u8::try_from(n)
        .ok()
        .filter(|n| *n <= 127)
        .ok_or_else(|| error(format!("invalid key {name}={value}")))
}
fn gates(op: &BTreeMap<String, String>, low: &str, high: &str) -> Result<Vec<Gate>, DeviceError> {
    let mut result = Vec::new();
    for cc in 0..128 {
        let a = format!("{low}{cc}");
        let b = format!("{high}{cc}");
        if op.contains_key(&a) || op.contains_key(&b) {
            let lo = finite(op, &a, 0.)?;
            let hi = finite(op, &b, 127.)?;
            if lo < 0. || hi > 127. || lo > hi {
                return Err(error(format!("invalid CC gate {a}/{b}")));
            }
            result.push(Gate {
                cc,
                low: lo as f32 / 127.,
                high: hi as f32 / 127.,
            });
        }
    }
    Ok(result)
}
/// Runtime capability is intentionally separate from importer syntax recognition.
fn runtime_opcode(name: &str) -> bool {
    matches!(
        name,
        "sample"
            | "lokey"
            | "hikey"
            | "key"
            | "lovel"
            | "hivel"
            | "trigger"
            | "sw_lokey"
            | "sw_hikey"
            | "sw_last"
            | "sw_default"
            | "sw_previous"
            | "sw_label"
            | "pitch_keycenter"
            | "pitch_keytrack"
            | "transpose"
            | "tune"
            | "offset"
            | "offset_random"
            | "end"
            | "loop_start"
            | "loop_end"
            | "loop_mode"
            | "delay"
            | "delay_random"
            | "pitch_random"
            | "rt_decay"
            | "seq_length"
            | "seq_position"
            | "lorand"
            | "hirand"
            | "group"
            | "off_by"
            | "off_mode"
            | "off_time"
            | "polyphony"
            | "note_polyphony"
            | "sustain_cc"
            | "amp_random"
            | "group_label"
            | "region_label"
            | "master_label"
            | "label"
            | "bend_up"
            | "bend_down"
    ) || [
        "offset_oncc",
        "offset_curvecc",
        "delay_cc",
        "offset_cc",
        "delay_oncc",
        "delay_curvecc",
    ]
    .iter()
    .any(|prefix| {
        name.strip_prefix(prefix)
            .and_then(|n| n.parse::<usize>().ok())
            .is_some_and(|n| n < 128)
    }) || ["locc", "hicc", "on_locc", "on_hicc"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .and_then(|n| n.parse::<usize>().ok())
            .is_some_and(|n| n < 128)
    })
}
/// Semantic preparation diagnostics without opening sample recordings. This is
/// useful for corpus auditing; a successful audit is not reference qualification.
pub fn unsupported_behaviors(program: &crate::sfz::Program) -> Vec<String> {
    let mut result = Vec::new();
    for region in &program.regions {
        for (name, value) in &region.opcodes {
            if !runtime_opcode(name) && !super::sfz_dsp::supports_opcode(name) {
                let source = region.opcode_sources.get(name).unwrap_or(&region.source);
                result.push(format!(
                    "{}:{}: unsupported {name}={value}",
                    source.path.display(),
                    source.line
                ));
            }
        }
        for name in ["offset", "delay"] {
            if let Err(e) = RuntimeParam::compile(&region.opcodes, name, 0., &program.curves) {
                result.push(format!(
                    "{}:{}: {name}: {e}",
                    region.source.path.display(),
                    region.source.line
                ));
            }
        }
        if let Err(e) = RegionDsp::compile_with_curves(&region.opcodes, &program.curves) {
            result.push(format!(
                "{}:{}: {e}",
                region.source.path.display(),
                region.source.line
            ));
        }
    }
    result.sort();
    result.dedup();
    result
}
impl Sfz {
    pub fn new(d: &model::Device, c: AudioConfig, token: u64) -> Result<Self, DeviceError> {
        let config = d
            .sfz
            .as_ref()
            .ok_or_else(|| error("missing SFZ configuration"))?;
        Self::prepare(d, c, token, config.program.clone())
    }
    fn prepare(
        d: &model::Device,
        c: AudioConfig,
        token: u64,
        program: crate::sfz::Program,
    ) -> Result<Self, DeviceError> {
        program.validate().map_err(error)?;
        let config = d
            .sfz
            .as_ref()
            .ok_or_else(|| error("missing SFZ configuration"))?;
        if config.max_voices == 0 || config.max_voices > 4096 || program.regions.len() > MAX_REGIONS
        {
            return Err(error("SFZ region/voice budget exceeded"));
        }
        if config.max_sample_frames == 0 || config.max_sample_frames > model::MAX_SFZ_SAMPLE_FRAMES
        {
            return Err(error("invalid SFZ sample budget"));
        }
        for region in &program.regions {
            for name in region.opcodes.keys() {
                if !runtime_opcode(name) && !super::sfz_dsp::supports_opcode(name) {
                    let source = region.opcode_sources.get(name).unwrap_or(&region.source);
                    return Err(error(format!(
                        "{}:{}: unsupported behavior{name}",
                        source.path.display(),
                        source.line
                    )));
                }
            }
        }
        let mut preflight = std::collections::BTreeSet::new();
        let mut required = 0usize;
        for source in &program.regions {
            if let Some(path) = &source.sample {
                let identity = crate::assets::resolver().resolve(path).map_err(error)?;
                if preflight.insert(identity.clone()) {
                    let info = crate::audio_file::info(&identity).map_err(error)?;
                    if info.frames == 0 {
                        return Err(error("cannot preflight empty/unknown sample frame count"));
                    }
                    let frames = usize::try_from(info.frames)
                        .map_err(|_| error("sample frame count overflow"))?;
                    required = required
                        .checked_add(frames)
                        .ok_or_else(|| error("sample footprint overflow"))?;
                }
            }
        }
        let required_bytes = decoded_bytes(required)?;
        if required > config.max_sample_frames {
            return Err(error(format!(
                "SFZ requires{required} decoded stereo frames ({}bytes), sample_budget_frames={}; increase explicit budget before loading",
                required_bytes, config.max_sample_frames
            )));
        }
        let mut samples = Vec::new();
        let mut paths = BTreeMap::new();
        let mut total = 0usize;
        let mut regions = Vec::new();
        let mut scopes = BTreeMap::new();
        let mut switch_default = None;
        // This preparation-local cache has one immutable custom-curve table.
        // Use the complete effective map as identity, including selectors.
        let mut dsp_cache = BTreeMap::<BTreeMap<String, String>, Arc<RegionDsp>>::new();
        for source in &program.regions {
            crate::host::check_cancelled().map_err(error)?;
            let op = &source.opcodes;
            for name in op.keys() {
                if !runtime_opcode(name) && !super::sfz_dsp::supports_opcode(name) {
                    return Err(error(format!(
                        "{}: unsupported behavior {name}",
                        source.source.path.display()
                    )));
                }
            }
            let sample = if let Some(path) = &source.sample {
                let identity = crate::assets::resolver().resolve(path).map_err(error)?;
                if let Some(&index) = paths.get(&identity) {
                    Some(index)
                } else {
                    let (info, audio) =
                        crate::audio_file::load_shared(&identity, config.max_sample_frames - total)
                            .map_err(error)?;
                    if audio.is_empty() {
                        return Err(error("empty SFZ sample"));
                    }
                    total = total
                        .checked_add(audio.len())
                        .ok_or_else(|| error("sample budget overflow"))?;
                    let loops = sample_loops(&identity)?;
                    let index = samples.len();
                    samples.push(Sample {
                        audio,
                        rate: info.rate as f64,
                        loops,
                    });
                    paths.insert(identity, index);
                    Some(index)
                }
            } else {
                None
            };
            let frames = sample.map_or(1., |i| samples[i].audio.len() as f64);
            let offset = finite(op, "offset", 0.)?;
            let end = finite(op, "end", frames - 1.)? + 1.;
            // end=-1 is an authored silent control region: it still matches
            // and applies group choke, but consumes no playback voice.
            if offset < 0.
                || end < 0.
                || (sample.is_some()
                    && end != 0.
                    && (offset >= frames || end <= offset || end > frames))
            {
                return Err(error(format!(
                    "{}:{} invalid sample offset/end: offset={offset}, inclusive_end={}, sample_frames={frames}, sample={:?}",
                    source.source.path.display(),
                    source.source.line,
                    end - 1.,
                    source.sample
                )));
            }
            let loop_mode = match op.get("loop_mode").map(String::as_str).unwrap_or(
                if sample.is_some_and(|i| samples[i].loops.is_some()) {
                    "loop_continuous"
                } else {
                    "no_loop"
                },
            ) {
                "no_loop" => LoopMode::None,
                "one_shot" => LoopMode::OneShot,
                "loop_continuous" => LoopMode::Continuous,
                "loop_sustain" => LoopMode::Sustain,
                v => return Err(error(format!("unsupported loop_mode {v}"))),
            };
            let loops = if sample.is_some()
                && end != 0.
                && matches!(loop_mode, LoopMode::Continuous | LoopMode::Sustain)
            {
                let embedded = sample.and_then(|i| samples[i].loops);
                let a = finite(op, "loop_start", embedded.map_or(0., |x| x[0]))?;
                let b = finite(op, "loop_end", embedded.map_or(frames - 1., |x| x[1] - 1.))? + 1.;
                if a < 0. || b <= a || b > end {
                    return Err(error("invalid sample loop"));
                }
                Some([a, b])
            } else {
                None
            };
            let trigger = match op.get("trigger").map(String::as_str).unwrap_or("attack") {
                "attack" => Trigger::Attack,
                "first" => Trigger::First,
                "legato" => Trigger::Legato,
                "release" => Trigger::Release,
                "release_key" => Trigger::ReleaseKey,
                v => return Err(error(format!("unsupported trigger {v}"))),
            };
            let default_key = key(op, "key", 60)?;
            let keys = if op.contains_key("key") {
                [default_key; 2]
            } else {
                [key(op, "lokey", 0)?, key(op, "hikey", 127)?]
            };
            let velocities = [
                bounded(op, "lovel", 0., 0., 127., true)? as u8,
                bounded(op, "hivel", 127., 0., 127., true)? as u8,
            ];
            if keys[0] > keys[1] || velocities[0] > velocities[1] {
                return Err(error("inverted key/velocity range"));
            }
            let sequence_len = bounded(op, "seq_length", 1., 1., 65536., true)? as u64;
            let sequence_pos = bounded(op, "seq_position", 1., 1., 65536., true)? as u64;
            if sequence_len == 0 || sequence_pos == 0 {
                return Err(error("invalid sequence range"));
            }
            let scope_key = (source.group_id, sequence_len);
            let next = scopes.len();
            let sequence = *scopes.entry(scope_key).or_insert(next);
            let switch = op
                .contains_key("sw_last")
                .then(|| key(op, "sw_last", 0))
                .transpose()?;
            let previous = op
                .contains_key("sw_previous")
                .then(|| key(op, "sw_previous", 0))
                .transpose()?;
            let switch_range = if op.contains_key("sw_lokey") || op.contains_key("sw_hikey") {
                Some([key(op, "sw_lokey", 0)?, key(op, "sw_hikey", 127)?])
            } else {
                None
            };
            if op.contains_key("sw_default") {
                let value = key(op, "sw_default", 0)?;
                // Later authored defaults replace earlier defaults (sfizz
                // instrument switch initialization); contradictory defaults are
                // not a preparation error.
                switch_default = Some(value);
            }
            let random = [finite(op, "lorand", 0.)?, finite(op, "hirand", 1.)?];
            if random[0] < 0. || random[1] > 1. || random[0] > random[1] {
                return Err(error("invalid random range"));
            }
            let off_fast = match op.get("off_mode").map(String::as_str).unwrap_or("fast") {
                "fast" | "time" => true,
                "normal" => false,
                v => return Err(error(format!("unsupported off_mode {v}"))),
            };
            regions.push(Region {
                sample,
                key: keys,
                velocity: velocities,
                trigger,
                gates: gates(op, "locc", "hicc")?,
                on_gates: gates(op, "on_locc", "on_hicc")?,
                switch,
                previous,
                switch_range,
                root: key(op, "pitch_keycenter", default_key)? as f64,
                tune: finite(op, "tune", 0.)?,
                transpose: finite(op, "transpose", 0.)?,
                keytrack: finite(op, "pitch_keytrack", 100.)?,
                offset: RuntimeParam::compile(op, "offset", 0., &program.curves)?,
                offset_random: finite(op, "offset_random", 0.)?,
                end,
                loops,
                loop_mode,
                delay: RuntimeParam::compile(op, "delay", 0., &program.curves)?,
                delay_random: finite(op, "delay_random", 0.)?,
                pitch_random: finite(op, "pitch_random", 0.)?,
                amp_random: bounded(op, "amp_random", 0., -144., 144., false)? as f32,
                rt_decay: bounded(op, "rt_decay", 0., 0., 200., false)? as f32,
                bend_up: finite(op, "bend_up", 200.)?,
                bend_down: finite(op, "bend_down", -200.)?,
                sequence,
                sequence_len,
                sequence_pos,
                random,
                group: bounded(op, "group", 0., 0., i32::MAX as f64, true)? as i64,
                off_by: bounded(op, "off_by", 0., 0., i32::MAX as f64, true)? as i64,
                off_fast,
                off_time: bounded(op, "off_time", 0.006, 0., 3600., false)? as f32,
                polyphony: bounded(op, "polyphony", config.max_voices as f64, 1., 4096., true)?
                    as usize,
                note_polyphony: bounded(
                    op,
                    "note_polyphony",
                    config.max_voices as f64,
                    0.,
                    4096.,
                    true,
                )?
                .max(1.) as usize,
                sustain_cc: bounded(op, "sustain_cc", 64., 0., 127., true)? as usize,
                dsp: if let Some(compiled) = dsp_cache.get(op) {
                    Arc::clone(compiled)
                } else {
                    let compiled = Arc::new(
                        RegionDsp::compile_with_curves(op, &program.curves).map_err(error)?,
                    );
                    dsp_cache.insert(op.clone(), Arc::clone(&compiled));
                    compiled
                },
            });
        }
        if regions.iter().any(|r| {
            r.sustain_cc >= 128
                || r.delay.base < 0.
                || r.delay_random < 0.
                || r.offset_random < 0.
                || r.polyphony == 0
                || r.note_polyphony == 0
        }) {
            return Err(error("invalid runtime region range"));
        }
        let mut defaults = [0.; 128];
        defaults[7] = 100. / 127.;
        defaults[11] = 1.;
        for (cc, value) in &program.controls {
            if *cc < 128 {
                defaults[*cc as usize] = *value / 127.;
            }
        }
        let seed = if config.seed == 0 {
            0x9e3779b97f4a7c15
        } else {
            config.seed
        };
        let mut switch_keys = [false; 128];
        for region in &regions {
            if let Some(key) = region.switch.filter(|key| {
                region
                    .switch_range
                    .is_none_or(|[a, b]| (a..=b).contains(key))
            }) {
                switch_keys[key as usize] = true;
            }
        }
        let mut instance = Self {
            regions,
            samples,
            voices: (0..config.max_voices).map(|_| Voice::default()).collect(),
            notes: Box::new([Note::default(); NOTE_CAPACITY]),
            cc: [defaults; 16],
            defaults,
            switches: [switch_default; 16],
            switch_default,
            switch_keys,
            previous: [None; 16],
            musical_previous: [None; 16],
            bend: [0.; 16],
            virtual_cc: [[0.; 16]; 16],
            sequence: vec![[0; 16]; scopes.len()],
            sequence_used: vec![false; scopes.len()],
            frame_clock: 0,
            rng: seed,
            seed,
            sample_rate: c.sample_rate as f64,
            core: ProcessorCore::new(d.kind, token, c.max_frames),
            gain: 1.,
            region_activity: vec![0; program.regions.len()],
            checkpoint: None,
            #[cfg(test)]
            force_pitch_recalculation: false,
            #[cfg(test)]
            force_legacy_note_age: false,
            statistics: SfzStatistics {
                decoded_frames: total,
                compiled_dsp_programs: dsp_cache.len(),
                compiled_dsp_bytes_shallow: dsp_cache.len() * std::mem::size_of::<RegionDsp>(),
                max_voices: config.max_voices,
                max_sample_frames: config.max_sample_frames,
                ..Default::default()
            },
            parameter_events: [(0, 0, 0.); 1024],
            parameter_count: 0,
        };
        for (name, value) in &d.params {
            instance.set_parameter(name, *value)?;
        }
        Ok(instance)
    }
    fn capture_checkpoint(&mut self) -> Result<(), DeviceError> {
        crate::host::check_cancelled().map_err(error)?;
        if self.parameter_count != 0 {
            return Err(DeviceError::InvalidConfig(
                "SFZ checkpoint requires consumed parameter events",
            ));
        }
        let checkpoint = SfzCheckpoint {
            voices: self.voices.clone(),
            notes: self.notes.clone(),
            cc: self.cc,
            switches: self.switches,
            previous: self.previous,
            musical_previous: self.musical_previous,
            bend: self.bend,
            virtual_cc: self.virtual_cc,
            sequence: self.sequence.clone(),
            frame_clock: self.frame_clock,
            rng: self.rng,
            gain: self.gain,
        };
        self.checkpoint = Some(Box::new(checkpoint));
        Ok(())
    }
    fn restore_checkpoint(&mut self) -> Result<bool, DeviceError> {
        let Some(checkpoint) = &self.checkpoint else {
            return Ok(false);
        };
        if checkpoint.voices.len() != self.voices.len()
            || checkpoint.sequence.len() != self.sequence.len()
        {
            return Err(DeviceError::InvalidConfig(
                "incompatible SFZ checkpoint dimensions",
            ));
        }
        self.voices.clone_from_slice(&checkpoint.voices);
        self.notes.copy_from_slice(checkpoint.notes.as_slice());
        self.cc = checkpoint.cc;
        self.switches = checkpoint.switches;
        self.previous = checkpoint.previous;
        self.musical_previous = checkpoint.musical_previous;
        self.bend = checkpoint.bend;
        self.virtual_cc = checkpoint.virtual_cc;
        self.sequence.copy_from_slice(&checkpoint.sequence);
        self.frame_clock = checkpoint.frame_clock;
        self.rng = checkpoint.rng;
        self.gain = checkpoint.gain;
        self.parameter_count = 0;
        Ok(true)
    }
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / ((1u64 << 53) as f64)
    }
    fn note_age(&self, note: &Note) -> u64 {
        #[cfg(test)]
        if self.force_legacy_note_age {
            return note.age;
        }
        self.frame_clock.wrapping_sub(note.started_frame)
    }
    fn note_slot(&mut self, id: u64) -> usize {
        if let Some(index) = self.notes.iter().position(|n| n.active && n.id == id) {
            for voice in &mut self.voices {
                if voice.active && voice.note == index {
                    voice.active = false;
                }
            }
            return index;
        }
        if let Some(index) = self.notes.iter().position(|n| !n.active) {
            return index;
        }
        self.statistics.stolen_notes = self.statistics.stolen_notes.saturating_add(1);
        let index = self
            .notes
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| self.note_age(n))
            .unwrap()
            .0;
        for voice in &mut self.voices {
            if voice.active && voice.note == index {
                voice.active = false;
                self.statistics.stolen_voices = self.statistics.stolen_voices.saturating_add(1);
            }
        }
        index
    }
    fn selected(
        &self,
        index: usize,
        note: &Note,
        trigger: Trigger,
        random: f64,
        legato: bool,
    ) -> bool {
        let r = &self.regions[index];
        let channel = note.channel as usize;
        let trigger_matches = match trigger {
            Trigger::Attack => match r.trigger {
                Trigger::Attack => true,
                Trigger::First => !legato,
                Trigger::Legato => legato,
                _ => false,
            },
            _ => r.trigger == trigger,
        };
        trigger_matches
            && r.on_gates.is_empty()
            && (r.key[0]..=r.key[1]).contains(&note.key)
            && (r.velocity[0]..=r.velocity[1]).contains(&note.velocity)
            && r.gates
                .iter()
                .all(|g| self.cc[channel][g.cc] >= g.low && self.cc[channel][g.cc] <= g.high)
            && r.switch
                .is_none_or(|key| self.switches[channel] == Some(key))
            && r.previous
                .is_none_or(|key| self.previous[channel] == Some(key))
            && random >= r.random[0]
            && (random < r.random[1] || r.random[1] == 1.)
    }
    fn trigger(&mut self, note: usize, trigger: Trigger, legato: bool, elapsed: u64, random: f64) {
        let ch = self.notes[note].channel as usize;
        self.virtual_cc[ch][7] = random as f32;
        self.virtual_cc[ch][5] = self.notes[note].key as f32 / 127.;
        if trigger != Trigger::Attack {
            self.virtual_cc[ch][3] = 0.;
        }
        if trigger == Trigger::Attack {
            self.virtual_cc[ch][3] = self.notes[note].velocity as f32 / 127.;
            if !self.switch_keys[self.notes[note].key as usize] {
                self.virtual_cc[ch][12] = self.musical_previous[ch]
                    .map_or(0., |previous| self.notes[note].key as f32 - previous as f32);
            }
        }
        self.sequence_used.fill(false);
        for index in 0..self.regions.len() {
            if !self.selected(index, &self.notes[note], trigger, random, legato) {
                continue;
            }
            self.statistics.matched_regions = self.statistics.matched_regions.saturating_add(1);
            let r = &self.regions[index];
            let scope = r.sequence;
            let ch = self.notes[note].channel as usize;
            self.sequence_used[scope] = true;
            if self.sequence[scope][ch] % r.sequence_len + 1 == r.sequence_pos {
                self.start(index, note, random, elapsed);
            }
        }
        for (index, used) in self.sequence_used.iter().enumerate() {
            if *used {
                let ch = self.notes[note].channel as usize;
                self.sequence[index][ch] = self.sequence[index][ch].wrapping_add(1);
            }
        }
    }
    fn start(&mut self, index: usize, note: usize, random: f64, elapsed: u64) {
        let region = &self.regions[index];
        let owner = self.notes[note];
        // Incoming group chokes old regions whose off_by names that group.
        if region.group != 0 {
            for voice in &mut self.voices {
                if voice.active
                    && self.regions[voice.region].off_by == region.group
                    && voice.note != note
                {
                    let old = &self.regions[voice.region];
                    voice.released = true;
                    voice.choked = old.off_fast;
                    voice.choke_time = old.off_time;
                    if voice.delay > 0 {
                        voice.active = false;
                    }
                    if !voice.choked {
                        if let Some(dsp) = &mut voice.dsp {
                            dsp.release();
                        }
                    }
                }
            }
        }
        let Some(sample) = region.sample.filter(|_| region.end != 0.) else {
            self.region_activity[index] = self.region_activity[index].saturating_add(1);
            return;
        };
        // SFZ limits count individual region voices, including sisters started
        // earlier in the same event. Stealing releases all sisters owned by the
        // selected logical note, preserving note-ID ownership independently.
        loop {
            let group_count = self
                .voices
                .iter()
                .filter(|v| v.active && !v.choked && self.regions[v.region].group == region.group)
                .count();
            let key_count = self
                .voices
                .iter()
                .filter(|v| {
                    v.active
                        && !v.choked
                        && self.regions[v.region].group == region.group
                        && self.notes[v.note].key == owner.key
                })
                .count();
            let key_limit = key_count >= region.note_polyphony;
            if group_count < region.polyphony && !key_limit {
                break;
            }
            let old = self
                .voices
                .iter()
                .filter(|v| {
                    v.active
                        && !v.choked
                        && self.regions[v.region].group == region.group
                        && (group_count >= region.polyphony
                            || !key_limit
                            || (self.notes[v.note].key == owner.key
                                && self.notes[v.note].velocity <= owner.velocity))
                })
                .max_by_key(|v| {
                    (
                        if key_limit && group_count < region.polyphony {
                            std::cmp::Reverse(self.notes[v.note].velocity)
                        } else {
                            std::cmp::Reverse(0)
                        },
                        v.age,
                    )
                })
                .map(|v| v.note);
            let Some(old) = old else { break };
            for voice in &mut self.voices {
                if voice.active && voice.note == old {
                    self.statistics.stolen_voices = self.statistics.stolen_voices.saturating_add(1);
                    voice.released = true;
                    voice.choked = true;
                    voice.choke_time = 0.006;
                    if voice.delay > 0 {
                        voice.active = false;
                    }
                }
            }
        }
        let slot = if let Some(slot) = self.voices.iter().position(|v| !v.active) {
            slot
        } else {
            let old = self
                .voices
                .iter()
                .max_by_key(|v| (v.choked, v.age))
                .unwrap()
                .note;
            for voice in &mut self.voices {
                if voice.active && voice.note == old {
                    voice.active = false;
                    self.statistics.stolen_voices = self.statistics.stolen_voices.saturating_add(1);
                }
            }
            self.voices.iter().position(|v| !v.active).unwrap()
        };
        let r = &self.regions[index];
        let rate = self.samples[sample].rate;
        let cents = (owner.pitch as f64 - r.root) * r.keytrack
            + r.transpose * 100.
            + r.tune
            + random * r.pitch_random;
        let step = rate / self.sample_rate * 2f64.powf(cents / 1200.);
        let delay = ((r.delay.get(&self.cc[owner.channel as usize]) + random * r.delay_random)
            * self.sample_rate)
            .round()
            .max(0.) as u64;
        let advanced = elapsed.saturating_sub(delay);
        let gain = if matches!(r.trigger, Trigger::Release | Trigger::ReleaseKey) {
            10f32.powf(-r.rt_decay * self.note_age(&owner) as f32 / self.sample_rate as f32 / 20.)
        } else {
            1.
        } * 10f32.powf((random as f32 * r.amp_random) / 20.);
        let mut dsp = r.dsp.start(owner.key, owner.velocity, self.sample_rate);
        dsp.latch(
            &r.dsp,
            &self.cc[owner.channel as usize],
            &self.virtual_cc[owner.channel as usize],
        );
        let position = r.offset.get(&self.cc[owner.channel as usize])
            + random * r.offset_random
            + advanced as f64 * step;
        if !position.is_finite() || position >= r.end || position < 0. {
            self.statistics.dropped_regions = self.statistics.dropped_regions.saturating_add(1);
            return;
        }
        self.region_activity[index] = self.region_activity[index].saturating_add(1);
        self.statistics.started_voices = self.statistics.started_voices.saturating_add(1);
        self.voices[slot] = Voice {
            choke_time: 0.008,
            looped: false,
            active: true,
            note,
            region: index,
            pos: position,
            step,
            cached_pitch_bits: None,
            cached_pitch_multiplier: 1.,
            delay: delay.saturating_sub(elapsed),
            age: elapsed,
            released: false,
            choked: false,
            choke_gain: 1.,
            gain,
            dsp: Some(dsp),
        };
    }
    fn release(&mut self, note: usize, key_release: bool) {
        let random = self.random();
        if key_release {
            self.trigger(note, Trigger::ReleaseKey, false, 0, random);
        }
        let channel = self.notes[note].channel as usize;
        let mut sustained = false;
        for voice in &mut self.voices {
            if voice.active && voice.note == note {
                let region = &self.regions[voice.region];
                if region.loop_mode != LoopMode::OneShot
                    && !matches!(region.trigger, Trigger::Release | Trigger::ReleaseKey)
                {
                    if self.cc[channel][region.sustain_cc] >= 0.5 {
                        sustained = true;
                    } else if !voice.released {
                        if voice.delay > 0 {
                            voice.active = false;
                            continue;
                        }
                        voice.released = true;
                        if let Some(dsp) = &mut voice.dsp {
                            dsp.release();
                        }
                    }
                }
            }
        }
        let previously_sustained = self.notes[note].sustained;
        self.notes[note].sustained = sustained;
        if sustained {
            return;
        }
        if !key_release && !previously_sustained {
            return;
        }
        self.trigger(note, Trigger::Release, false, 0, random);
    }
    fn event(&mut self, event: DeviceEventKind) {
        match event {
            DeviceEventKind::NoteOn {
                pitch,
                note_id,
                channel,
                key,
                velocity,
                ..
            } => {
                let ch = channel as usize;
                if self.switch_keys[key as usize] {
                    self.switches[ch] = Some(key);
                }
                let legato = self.notes.iter().any(|n| {
                    n.active && n.down && n.channel == channel && !self.switch_keys[n.key as usize]
                });
                let slot = self.note_slot(note_id);
                self.notes[slot] = Note {
                    active: true,
                    started_frame: self.frame_clock,
                    down: true,
                    id: note_id,
                    channel,
                    key,
                    pitch,
                    velocity: (velocity.clamp(0., 1.) * 127.).round() as u8,
                    ..Note::default()
                };
                let random = self.random();
                self.trigger(slot, Trigger::Attack, legato, 0, random);
                self.previous[ch] = Some(key);
                if !self.switch_keys[key as usize] {
                    self.musical_previous[ch] = Some(key);
                }
            }
            DeviceEventKind::NoteOff { note_id, .. } => {
                if let Some(note) = self
                    .notes
                    .iter()
                    .position(|n| n.active && n.down && n.id == note_id)
                {
                    self.notes[note].down = false;
                    self.release(note, true);
                }
            }
            DeviceEventKind::NoteChoke { note_id, .. } => {
                if let Some(note) = self.notes.iter().position(|n| n.active && n.id == note_id) {
                    self.notes[note].down = false;
                    self.notes[note].sustained = false;
                    for voice in &mut self.voices {
                        if voice.active && voice.note == note {
                            voice.released = true;
                            voice.choked = true;
                            voice.choke_time = 0.008;
                            if voice.delay > 0 {
                                voice.active = false;
                            }
                        }
                    }
                }
            }
            DeviceEventKind::NoteExpression {
                note_id,
                expression,
                value,
                ..
            } => {
                if let Some(note) = self.notes.iter_mut().find(|n| n.active && n.id == note_id) {
                    match expression {
                        0 => note.volume = value as f32,
                        1 => {
                            let p = (value as f32).clamp(0., 1.);
                            note.pan = [(2. * (1. - p)).sqrt(), (2. * p).sqrt()];
                        }
                        2 => note.tuning = 2f64.powf(value / 12.),
                        4 => note.expression = value as f32,
                        _ => {}
                    }
                }
            }
            DeviceEventKind::Controller {
                channel,
                controller,
                value,
            } => self.controller(channel, controller, value),
            DeviceEventKind::Midi { bytes, len } => {
                if len >= 3 {
                    match bytes[0] & 0xf0 {
                        0xb0 => self.controller(bytes[0] & 15, bytes[1], bytes[2]),
                        0xe0 => {
                            let value = (bytes[1] as u16 | ((bytes[2] as u16) << 7)) as f64;
                            self.bend[(bytes[0] & 15) as usize] = (value - 8192.) / 8192.;
                        }
                        _ => {}
                    }
                }
            }
            DeviceEventKind::Flush => self.reset(),
        }
    }
    fn controller(&mut self, channel: u8, controller: u8, value: u8) {
        self.controller_value(channel, controller, value as f32);
    }
    fn controller_value(&mut self, channel: u8, controller: u8, value: f32) {
        let ch = channel as usize;
        let cc = controller as usize;
        let old = self.cc[ch][cc];
        self.cc[ch][cc] = value / 127.;
        if controller == 120 {
            for voice in &mut self.voices {
                if voice.active && self.notes[voice.note].channel == channel {
                    voice.active = false;
                }
            }
            return;
        }
        if controller == 123 {
            for index in 0..self.notes.len() {
                if self.notes[index].active
                    && self.notes[index].down
                    && self.notes[index].channel == channel
                {
                    self.notes[index].down = false;
                    self.release(index, true);
                }
            }
        }
        if old >= 0.5 && value < 63.5 {
            for index in 0..self.notes.len() {
                if self.notes[index].active
                    && self.notes[index].sustained
                    && self.notes[index].channel == channel
                    && self.voices.iter().any(|voice| {
                        voice.active
                            && !voice.released
                            && voice.note == index
                            && self.regions[voice.region].sustain_cc == cc
                            && self.regions[voice.region].loop_mode != LoopMode::OneShot
                            && !matches!(
                                self.regions[voice.region].trigger,
                                Trigger::Release | Trigger::ReleaseKey
                            )
                    })
                {
                    self.release(index, false);
                }
            }
        }
        // CC-triggered regions use a dedicated logical owner and the same bounded
        // voice storage. They trigger only when this CC enters the gate.
        let mut owner = None;
        let mut random = None;
        for index in 0..self.regions.len() {
            let region = &self.regions[index];
            if region.on_gates.is_empty()
                || !region
                    .on_gates
                    .iter()
                    .any(|g| g.cc == cc && old < g.low || g.cc == cc && old > g.high)
                || !region
                    .on_gates
                    .iter()
                    .all(|g| self.cc[ch][g.cc] >= g.low && self.cc[ch][g.cc] <= g.high)
            {
                continue;
            }
            let slot = *owner.get_or_insert_with(|| {
                let id = u64::MAX - (channel as u64 * 128 + controller as u64);
                let slot = self.note_slot(id);
                self.notes[slot] = Note {
                    active: true,
                    started_frame: self.frame_clock,
                    id,
                    channel,
                    key: 60,
                    velocity: 127,
                    pitch: 60.,
                    ..Note::default()
                };
                slot
            });
            let random = *random.get_or_insert_with(|| self.random());
            self.start(index, slot, random, 0);
        }
    }
}
impl DeviceProcessor for Sfz {
    fn sfz_region_activity(&self) -> Option<&[u64]> {
        Some(&self.region_activity)
    }
    fn prepare_loop_checkpoint(&mut self) -> Result<(), DeviceError> {
        self.capture_checkpoint()
    }
    fn restore_loop_checkpoint(&mut self) -> Result<bool, DeviceError> {
        self.restore_checkpoint()
    }
    fn sfz_statistics(&self) -> Option<SfzStatistics> {
        let mut statistics = self.statistics;
        statistics.active_voices = self.voices.iter().filter(|v| v.active).count();
        statistics.active_notes = self.notes.iter().filter(|n| n.active).count();
        Some(statistics)
    }
    fn kind(&self) -> model::DeviceKind {
        model::DeviceKind::Sfz
    }
    fn debug_state(&self) -> DeviceDebugState {
        DeviceDebugState {
            tail_samples: u32::MAX,
            ..self.core.debug_state()
        }
    }
    fn has_note(&self, id: u64) -> bool {
        self.notes
            .iter()
            .any(|n| n.active && n.id == id && (n.down || n.sustained))
            || self
                .voices
                .iter()
                .any(|v| v.active && self.notes[v.note].id == id)
    }
    fn accepts_note_expression(&self, kind: u8) -> bool {
        matches!(kind, 0 | 1 | 2 | 4)
    }
    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        match name {
            "gain_db" => {
                self.gain =
                    db_to_amplitude(parameter_value(self.kind(), "gain_db", value, -120., 24.)?);
                Ok(())
            }
            name if name
                .strip_prefix("cc")
                .and_then(|n| n.parse::<usize>().ok())
                .is_some_and(|n| n < 128) =>
            {
                if !value.is_finite() || !(0. ..=127.).contains(&value) {
                    return Err(DeviceError::InvalidConfig(
                        "SFZ CC parameter outside 0..127",
                    ));
                }
                let cc = name[2..].parse::<usize>().unwrap();
                self.defaults[cc] = value / 127.;
                for channel in 0..16 {
                    self.controller_value(channel, cc as u8, value);
                }
                Ok(())
            }
            _ => Err(DeviceError::UnknownParameter { kind: self.kind() }),
        }
    }
    fn accepts_parameter_offsets(&self) -> bool {
        true
    }
    fn set_parameter_at(&mut self, name: &str, value: f32, offset: u32) -> Result<(), DeviceError> {
        let index = if name == "gain_db" {
            128
        } else {
            name.strip_prefix("cc")
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|i| *i < 128)
                .ok_or(DeviceError::UnknownParameter { kind: self.kind() })?
        };
        if !value.is_finite()
            || if index == 128 {
                !(-120. ..=24.).contains(&value)
            } else {
                !(0. ..=127.).contains(&value)
            }
        {
            return Err(DeviceError::InvalidConfig("invalid SFZ parameter"));
        }
        if self.parameter_count == self.parameter_events.len() {
            return Err(DeviceError::InvalidConfig(
                "SFZ parameter event budget exceeded",
            ));
        }
        // Stable insertion keeps simultaneous events in host-supplied order.
        let mut i = self.parameter_count;
        while i > 0 && self.parameter_events[i - 1].0 > offset {
            self.parameter_events[i] = self.parameter_events[i - 1];
            i -= 1;
        }
        self.parameter_events[i] = (offset, index, value);
        self.parameter_count += 1;
        Ok(())
    }
    fn reset(&mut self) {
        self.parameter_count = 0;
        self.frame_clock = 0;
        self.region_activity.fill(0);
        self.statistics = SfzStatistics {
            decoded_frames: self.statistics.decoded_frames,
            compiled_dsp_programs: self.statistics.compiled_dsp_programs,
            compiled_dsp_bytes_shallow: self.statistics.compiled_dsp_bytes_shallow,
            max_voices: self.voices.len(),
            max_sample_frames: self.statistics.max_sample_frames,
            ..Default::default()
        };
        for voice in &mut self.voices {
            voice.active = false;
        }
        self.notes.fill(Note::default());
        self.cc = [self.defaults; 16];
        self.switches = [self.switch_default; 16];
        self.previous = [None; 16];
        self.musical_previous = [None; 16];
        self.bend = [0.; 16];
        self.virtual_cc = [[0.; 16]; 16];
        self.sequence.fill([0; 16]);
        self.rng = self.seed;
    }
    fn process(
        &mut self,
        ctx: ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        self.core
            .begin_process(ctx.frames, left.len(), right.len())?;
        if self.parameter_events[..self.parameter_count]
            .iter()
            .any(|(offset, _, _)| *offset as usize >= ctx.frames)
        {
            self.parameter_count = 0;
            return Err(DeviceError::InvalidConfig(
                "invalid SFZ parameter event offset",
            ));
        }
        // Validate the entire batch before changing any voice/selection state.
        let mut previous = 0;
        for event in events {
            if event.offset as usize >= ctx.frames || event.offset < previous {
                return Err(DeviceError::InvalidConfig("invalid SFZ event offset"));
            }
            previous = event.offset;
            match event.kind {
                DeviceEventKind::NoteOn { elapsed_frames, .. } if elapsed_frames > 0 => {
                    return Err(DeviceError::InvalidConfig(
                        "SFZ seek requires prepared history replay",
                    ));
                }
                DeviceEventKind::NoteOn {
                    channel,
                    key,
                    pitch,
                    velocity,
                    ..
                } if channel >= 16
                    || key >= 128
                    || !pitch.is_finite()
                    || !velocity.is_finite()
                    || !(0. ..=1.).contains(&velocity) =>
                {
                    return Err(DeviceError::InvalidConfig("invalid SFZ note event"));
                }
                DeviceEventKind::Controller {
                    channel,
                    controller,
                    value,
                } if channel >= 16 || controller >= 128 || value >= 128 => {
                    return Err(DeviceError::InvalidConfig("invalid SFZ controller event"));
                }
                DeviceEventKind::Midi { bytes, len }
                    if len > 3 || (len >= 3 && (bytes[1] >= 128 || bytes[2] >= 128)) =>
                {
                    return Err(DeviceError::InvalidConfig("invalid SFZ MIDI event"));
                }
                DeviceEventKind::NoteExpression {
                    expression, value, ..
                } if !value.is_finite()
                    || !match expression {
                        0 => (0. ..=4.).contains(&value),
                        2 => (-120. ..=120.).contains(&value),
                        _ => (0. ..=1.).contains(&value),
                    } =>
                {
                    return Err(DeviceError::InvalidConfig("invalid SFZ note expression"));
                }
                _ => {}
            }
        }
        let mut event = 0;
        let mut parameter = 0;
        let parameter_count = self.parameter_count;
        self.parameter_count = 0;
        for frame in 0..ctx.frames {
            while parameter < parameter_count
                && self.parameter_events[parameter].0 as usize == frame
            {
                let (_, index, value) = self.parameter_events[parameter];
                parameter += 1;
                if index == 128 {
                    self.gain = db_to_amplitude(value);
                } else {
                    for channel in 0..16 {
                        self.controller_value(channel, index as u8, value);
                    }
                }
            }
            while event < events.len() && events[event].offset as usize == frame {
                self.event(events[event].kind);
                event += 1;
            }
            let mut pair = [0.; 2];
            for voice in &mut self.voices {
                if !voice.active {
                    continue;
                }
                voice.age = voice.age.wrapping_add(1);
                if voice.delay > 0 {
                    voice.delay -= 1;
                    continue;
                }
                let region = &self.regions[voice.region];
                let sample = &self.samples[region.sample.unwrap()];
                let owner = self.notes[voice.note];
                let loops = if region.loop_mode == LoopMode::Continuous
                    || region.loop_mode == LoopMode::Sustain && !voice.released
                {
                    region.loops
                } else {
                    None
                };
                if let Some([start, end]) = loops {
                    if voice.pos >= end {
                        voice.pos = start + (voice.pos - start).rem_euclid(end - start);
                        voice.looped = true;
                    }
                }
                if voice.pos >= region.end || voice.pos < 0. {
                    voice.active = false;
                    continue;
                }
                let dsp = voice.dsp.as_mut().unwrap();
                let pitch = dsp.pitch_cents(&region.dsp, &self.cc[owner.channel as usize]);
                let interpolation_loop =
                    if voice.looped || loops.is_some_and(|[_, end]| voice.pos >= end - 2.) {
                        loops
                    } else {
                        None
                    };
                let input =
                    super::sampler::interpolate(&sample.audio, voice.pos, interpolation_loop);
                let output = dsp.next(&region.dsp, input, &self.cc[owner.channel as usize], 0., 1.);
                if voice.choked {
                    voice.choke_gain = (voice.choke_gain
                        - 1. / (self.sample_rate as f32
                            * voice.choke_time.max(1. / self.sample_rate as f32)))
                    .max(0.);
                }
                let gain =
                    self.gain * voice.gain * owner.volume * owner.expression * voice.choke_gain;
                for ch in 0..2 {
                    pair[ch] += output[ch] * gain * owner.pan[ch];
                }
                let pitch_exponent = (pitch as f64
                    + if self.bend[owner.channel as usize] >= 0. {
                        region.bend_up * self.bend[owner.channel as usize]
                    } else {
                        -region.bend_down * self.bend[owner.channel as usize]
                    })
                    / 1200.;
                #[cfg(test)]
                if self.force_pitch_recalculation {
                    voice.cached_pitch_bits = None;
                }
                if voice.cached_pitch_bits != Some(pitch_exponent.to_bits()) {
                    voice.cached_pitch_multiplier = 2f64.powf(pitch_exponent);
                    voice.cached_pitch_bits = Some(pitch_exponent.to_bits());
                }
                // Keep the original multiplication order, including the
                // separately cached per-note expression tuning multiplier.
                voice.pos += voice.step * owner.tuning * voice.cached_pitch_multiplier;
                if dsp.finished() || voice.choke_gain == 0. {
                    voice.active = false;
                }
            }
            #[cfg(test)]
            if self.force_legacy_note_age {
                for note in self.notes.iter_mut() {
                    if note.active {
                        note.age = note.age.wrapping_add(1);
                    }
                }
            }
            self.frame_clock = self.frame_clock.wrapping_add(1);
            left[frame] = pair[0];
            right[frame] = pair[1];
        }
        for (index, note) in self.notes.iter_mut().enumerate() {
            if note.active
                && !note.down
                && !note.sustained
                && !self.voices.iter().any(|v| v.active && v.note == index)
            {
                note.active = false;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> TransportSnapshot {
        TransportSnapshot {
            sample_rate: 1000.,
            running: true,
            sample_position: 0,
            beat_position: 0.,
            current_tick: 0.,
            project_frame: 0.,
            bpm: 120.,
            meter: [4, 4],
            loop_ticks: 0,
            ended: false,
        }
    }
    fn fixture(body: &str) -> (tempfile::TempDir, Sfz) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.wav");
        let mut wav = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 1000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..1000 {
            wav.write_sample(1f32).unwrap();
        }
        wav.finalize().unwrap();
        let path = dir.path().join("program.sfz");
        std::fs::write(&path, format!("<control> set_cc7=127 set_cc11=127\n{body}")).unwrap();
        let program = crate::sfz::load(&path, &Default::default()).unwrap();
        let device = model::Device {
            sfz: Some(model::SfzConfig {
                path: path.to_string_lossy().into(),
                program,
                defines: BTreeMap::new(),
                max_voices: 32,
                max_sample_frames: model::DEFAULT_SFZ_SAMPLE_FRAMES,
                seed: 42,
                embed_assets: false,
                source_overlays: vec![],
            }),
            id: model::Id::new("sfz"),
            kind: model::DeviceKind::Sfz,
            params: BTreeMap::new(),
            asset_versions: vec![],
            patch: None,
            generation: 0,
            rack: None,
            sample: None,
            sidechain: None,
            vst3: None,
        };
        let synth = Sfz::new(
            &device,
            AudioConfig {
                sample_rate: 1000.,
                max_frames: 128,
                offline: true,
            },
            1,
        )
        .unwrap();
        (dir, synth)
    }
    fn on(id: u64, key: u8) -> DeviceEventKind {
        DeviceEventKind::NoteOn {
            sample_zone: None,
            pitch: key as f32,
            elapsed_frames: 0,
            note_id: id,
            channel: 0,
            key,
            velocity: 1.,
        }
    }
    fn off(id: u64, key: u8) -> DeviceEventKind {
        DeviceEventKind::NoteOff {
            note_id: id,
            channel: 0,
            key,
            velocity: 0.,
        }
    }
    fn cc(number: u8, value: u8) -> DeviceEventKind {
        DeviceEventKind::Controller {
            channel: 0,
            controller: number,
            value,
        }
    }
    fn render(s: &mut Sfz, frames: usize, events: &[(u32, DeviceEventKind)]) -> Vec<f32> {
        let events: Vec<_> = events
            .iter()
            .map(|&(offset, kind)| DeviceEvent { offset, kind })
            .collect();
        let mut left = vec![0.; frames];
        let mut right = vec![0.; frames];
        s.process(
            ProcessContext {
                frames,
                block_start_sample: 0,
                transport: snapshot(),
            },
            &events,
            &mut left,
            &mut right,
        )
        .unwrap();
        left
    }
    #[test]
    fn broad_switch_range_does_not_consume_musical_notes_or_other_layers() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav sw_lokey=10 sw_hikey=100 sw_last=10 sw_default=10 <region> key=60 <group> <region> sample=sample.wav key=10",
        );
        assert_eq!(render(&mut s, 1, &[(0, on(1, 60))]), vec![1.]);
        s.reset();
        assert_eq!(render(&mut s, 1, &[(0, on(2, 10))]), vec![1.]);
        assert_eq!(render(&mut s, 1, &[(0, on(3, 60))]), vec![2.]);
    }
    #[test]
    fn later_switch_default_wins_and_out_of_cycle_sequence_remains_unreachable() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav sw_lokey=10 sw_hikey=11 sw_default=10 <region> key=60 sw_last=10 <group> sw_lokey=10 sw_hikey=11 sw_default=11 <region> sample=sample.wav key=60 sw_last=11",
        );
        assert_eq!(render(&mut s, 1, &[(0, on(1, 60))]), vec![1.]);
        let (_dir, mut s) =
            fixture("<group> sample=sample.wav seq_length=2 <region> seq_position=3");
        assert_eq!(
            render(&mut s, 2, &[(0, on(1, 60)), (1, on(2, 60))]),
            vec![0., 0.]
        );
        assert_eq!(s.sfz_region_activity().unwrap(), &[0]);
    }
    #[test]
    fn absolute_note_clock_matches_legacy_release_checkpoint_and_oldest_stealing() {
        let source = "<group> sample=sample.wav ampeg_release=0.001 <region> <region> trigger=release rt_decay=20";
        let (_a, mut clock) = fixture(source);
        let (_b, mut legacy) = fixture(source);
        legacy.force_legacy_note_age = true;
        assert_eq!(
            render(&mut clock, 128, &[(7, on(1, 60)), (80, on(2, 64))]),
            render(&mut legacy, 128, &[(7, on(1, 60)), (80, on(2, 64))])
        );
        clock.capture_checkpoint().unwrap();
        legacy.capture_checkpoint().unwrap();
        for _ in 0..3 {
            assert_eq!(render(&mut clock, 128, &[]), render(&mut legacy, 128, &[]));
        }
        let events = [(3, off(1, 60)), (80, off(2, 64))];
        assert_eq!(
            render(&mut clock, 128, &events),
            render(&mut legacy, 128, &events)
        );
        clock.restore_checkpoint().unwrap();
        legacy.restore_checkpoint().unwrap();
        assert_eq!(
            render(&mut clock, 128, &events),
            render(&mut legacy, 128, &events)
        );
        // A pause performs no callback and therefore advances neither age path.
        assert_eq!(
            clock.note_age(&clock.notes[0]),
            legacy.note_age(&legacy.notes[0])
        );
        clock.reset();
        legacy.reset();
        for block in 0..8 {
            let events: Vec<_> = (0..128)
                .map(|frame| (frame, on((block * 128 + frame + 1) as u64, 60)))
                .collect();
            assert_eq!(
                render(&mut clock, 128, &events),
                render(&mut legacy, 128, &events)
            );
        }
        assert_eq!(
            render(&mut clock, 1, &[(0, on(1025, 60))]),
            render(&mut legacy, 1, &[(0, on(1025, 60))])
        );
        assert!(!clock.notes.iter().any(|n| n.active && n.id == 1));
        assert_eq!(
            clock
                .notes
                .iter()
                .map(|n| (n.active, n.id))
                .collect::<Vec<_>>(),
            legacy
                .notes
                .iter()
                .map(|n| (n.active, n.id))
                .collect::<Vec<_>>()
        );
        assert_eq!(clock.statistics.stolen_notes, 1);
    }
    #[test]
    #[ignore = "release performance probe; reports identical fixed-frame cached and forced-recalculation paths"]
    fn benchmark_pitch_cache_fixed_256_layer_processor() {
        let body = format!(
            "<group> sample=sample.wav loop_mode=loop_continuous loop_start=0 loop_end=999 {}",
            "<region> ".repeat(256)
        );
        let (_dir, mut s) = fixture(&body);
        s.sample_rate = 48000.;
        s.voices.resize_with(256, Voice::default);
        for region in &mut s.regions {
            region.polyphony = 256;
            region.note_polyphony = 256;
        }
        let mut hashes = Vec::new();
        for (recalculate, legacy_age) in [(true, true), (true, false), (false, false)] {
            s.reset();
            s.force_pitch_recalculation = recalculate;
            s.force_legacy_note_age = legacy_age;
            let mut left = [0.; 128];
            let mut right = [0.; 128];
            let events = [DeviceEvent {
                offset: 0,
                kind: on(1, 60),
            }];
            let start = std::time::Instant::now();
            let mut hash = 0xcbf29ce484222325u64;
            for block in 0..188 {
                s.process(
                    ProcessContext {
                        frames: 128,
                        block_start_sample: block * 128,
                        transport: snapshot(),
                    },
                    if block == 0 { &events } else { &[] },
                    &mut left,
                    &mut right,
                )
                .unwrap();
                for value in left.iter().chain(&right) {
                    hash ^= value.to_bits() as u64;
                    hash = hash.wrapping_mul(0x100000001b3);
                }
            }
            assert_eq!(s.sfz_statistics().unwrap().active_voices, 256);
            eprintln!(
                "pitch_cache forced_recalculate={recalculate} legacy_note_age={legacy_age} frames=24064 voices=256 seconds={} hash={hash:016x}",
                start.elapsed().as_secs_f64()
            );
            hashes.push(hash);
        }
        assert!(hashes.iter().all(|hash| *hash == hashes[0]));
    }
    #[test]
    fn pitch_cache_is_bit_exact_with_cc_bend_lfo_and_owned_expression() {
        for modulation in ["", "pitch_oncc1=200 lfo01_freq=3 lfo01_pitch=120"] {
            let source = format!(
                "<region> sample=sample.wav loop_mode=loop_continuous loop_start=0 loop_end=999 {modulation}"
            );
            let (_a, mut cached) = fixture(&source);
            let (_b, mut recalculated) = fixture(&source);
            let audio: std::sync::Arc<[[f32; 2]]> = (0..1000)
                .map(|n| {
                    let x = (n as f32 * 0.07).sin();
                    [x, x]
                })
                .collect::<Vec<_>>()
                .into();
            cached.samples[0].audio = audio.clone();
            recalculated.samples[0].audio = audio;
            recalculated.force_pitch_recalculation = true;
            let events = [
                (0, on(1, 60)),
                (73, cc(1, 127)),
                (
                    101,
                    DeviceEventKind::Midi {
                        bytes: [0xe0, 0, 96],
                        len: 3,
                    },
                ),
                (
                    149,
                    DeviceEventKind::NoteExpression {
                        note_id: 1,
                        channel: 0,
                        key: 60,
                        expression: 2,
                        value: 0.37,
                    },
                ),
                (
                    231,
                    DeviceEventKind::Midi {
                        bytes: [0xe0, 0, 32],
                        len: 3,
                    },
                ),
                (301, cc(1, 0)),
                (410, off(1, 60)),
            ];
            let mut actual = Vec::new();
            let mut expected = Vec::new();
            for block in 0..4 {
                let events: Vec<_> = events
                    .iter()
                    .filter(|(offset, _)| *offset >= block * 128 && *offset < (block + 1) * 128)
                    .map(|(offset, event)| (*offset - block * 128, *event))
                    .collect();
                actual.extend(render(&mut cached, 128, &events));
                expected.extend(render(&mut recalculated, 128, &events));
            }
            assert_eq!(
                actual.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
            for (a, b) in cached.voices.iter().zip(&recalculated.voices) {
                assert_eq!(a.pos.to_bits(), b.pos.to_bits());
            }
        }
    }
    #[test]
    fn negative_one_endpoint_preserves_silent_control_choke_without_voice() {
        for silence_sample in ["*silence", "sample.wav"] {
            let source = format!(
                "<group> sample=sample.wav loop_mode=loop_continuous group=1 off_by=2 off_time=0.001 <region> key=60 <group> sample={silence_sample} end=-1 group=2 <region> key=61"
            );
            let (_dir, mut s) = fixture(&source);
            assert_eq!(render(&mut s, 1, &[(0, on(1, 60))]), vec![1.]);
            assert_eq!(s.sfz_statistics().unwrap().active_voices, 1);
            assert_eq!(render(&mut s, 1, &[(0, on(2, 61))]), vec![0.]);
            assert_eq!(s.sfz_statistics().unwrap().active_voices, 0);
            assert_eq!(s.sfz_statistics().unwrap().started_voices, 1);
            assert_eq!(s.sfz_region_activity().unwrap(), &[1, 1]);
            assert!(s.has_note(1));
        }
    }
    #[test]
    fn prepared_dsp_shares_only_complete_identical_effective_maps() {
        let body = format!("<group> sample=sample.wav {}", "<region> ".repeat(256));
        let (_dir, s) = fixture(&body);
        assert!(
            s.regions
                .iter()
                .all(|r| Arc::ptr_eq(&r.dsp, &s.regions[0].dsp))
        );
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav <region> key=60 <region> key=61 <region> key=60 volume=-6.0206",
        );
        assert!(!Arc::ptr_eq(&s.regions[0].dsp, &s.regions[1].dsp));
        assert!(!Arc::ptr_eq(&s.regions[0].dsp, &s.regions[2].dsp));
        assert!((render(&mut s, 1, &[(0, on(1, 60))])[0] - 1.5).abs() < 1e-5);
    }
    #[test]
    fn unrelated_falling_controller_does_not_reevaluate_custom_sustain_or_rng() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav sustain_cc=1 ampeg_release=0.001 <region> <region> trigger=release volume=-6.0206",
        );
        render(
            &mut s,
            1,
            &[(0, cc(1, 127)), (0, cc(2, 127)), (0, on(1, 60))],
        );
        render(&mut s, 1, &[(0, off(1, 60))]);
        assert!(s.notes[0].sustained);
        let random = s.rng;
        assert_eq!(render(&mut s, 1, &[(0, cc(2, 0))]), vec![1.]);
        assert_eq!(s.rng, random);
        assert!(s.notes[0].sustained);
        assert!(
            !s.voices
                .iter()
                .filter(|v| v.active && v.note == 0)
                .any(|v| v.released)
        );
        assert!((render(&mut s, 1, &[(0, cc(1, 0))])[0] - 0.5).abs() < 1e-5);
        assert!(!s.notes[0].sustained);
        assert_ne!(s.rng, random);
    }
    #[test]
    fn all_matching_layers_start_and_note_expression_reaches_every_layer() {
        let (_dir, mut s) =
            fixture("<group> sample=sample.wav ampeg_release=0.01 <region> <region>");
        assert_eq!(render(&mut s, 1, &[(0, on(1, 60))]), vec![2.]);
        let expression = DeviceEventKind::NoteExpression {
            note_id: 1,
            channel: 0,
            key: 60,
            expression: 4,
            value: 0.25,
        };
        assert_eq!(render(&mut s, 1, &[(0, expression)]), vec![0.5]);
        assert!(s.has_note(1));
        render(&mut s, 16, &[(0, off(1, 60))]);
        assert!(!s.has_note(1));
    }
    #[test]
    fn group_sequence_advances_once_for_layered_event_and_reset_repeats() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav seq_length=2 <region> seq_position=1 volume=0 <region> seq_position=1 volume=0 <region> seq_position=2 volume=-6.0206",
        );
        let first = render(&mut s, 1, &[(0, on(1, 60))]);
        assert_eq!(first, vec![2.]);
        s.voices.iter_mut().for_each(|v| v.active = false);
        assert!((render(&mut s, 1, &[(0, on(2, 60))])[0] - 0.5).abs() < 1e-5);
        s.reset();
        assert_eq!(render(&mut s, 1, &[(0, on(3, 60))]), first);
    }
    #[test]
    fn sustain_and_release_triggers_keep_note_id_ownership() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav ampeg_release=0.001 <region> <region> trigger=release volume=-6.0206",
        );
        render(&mut s, 1, &[(0, cc(64, 127)), (0, on(1, 60))]);
        assert_eq!(render(&mut s, 1, &[(0, off(1, 60))]), vec![1.]);
        assert!(s.notes.iter().any(|n| n.id == 1 && n.sustained));
        let value = render(&mut s, 1, &[(0, cc(64, 0))])[0];
        assert!((value - 0.5).abs() < 1e-5);
        assert!(s.has_note(1));
    }
    #[test]
    fn keyswitch_first_legato_and_previous_are_channel_history() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav lokey=40 sw_lokey=20 sw_hikey=21 sw_default=20 <region> sw_last=20 trigger=first <region> sw_last=21 trigger=legato",
        );
        assert_eq!(render(&mut s, 1, &[(0, on(1, 20))]), vec![0.]);
        assert_eq!(render(&mut s, 1, &[(0, on(2, 60))]), vec![1.]);
        assert_eq!(
            render(&mut s, 1, &[(0, on(3, 21)), (0, on(4, 62))]),
            vec![2.]
        );
    }
    #[test]
    fn random_event_is_correlated_across_layers_and_block_partition_invariant() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav <region> lorand=0 hirand=0.5 <region> lorand=0 hirand=0.5 <region> lorand=0.5 hirand=1 volume=-6.0206",
        );
        let events = [(0, on(1, 60)), (2, on(2, 60)), (4, on(3, 60))];
        let whole = render(&mut s, 6, &events);
        s.reset();
        let mut parts = render(&mut s, 2, &[(0, on(1, 60))]);
        parts.extend(render(&mut s, 2, &[(0, on(2, 60))]));
        parts.extend(render(&mut s, 2, &[(0, on(3, 60))]));
        assert_eq!(whole, parts);
        assert!(whole[0] == 2. || (whole[0] - 0.5).abs() < 1e-5);
    }
    #[test]
    fn overlapping_same_key_ids_are_independent_and_choke_is_bounded() {
        let (_dir, mut s) = fixture("<region> sample=sample.wav ampeg_release=0.004");
        render(&mut s, 1, &[(0, on(1, 60)), (0, on(2, 60))]);
        render(&mut s, 6, &[(0, off(1, 60))]);
        assert!(!s.has_note(1));
        assert!(s.has_note(2));
        render(
            &mut s,
            16,
            &[(
                0,
                DeviceEventKind::NoteChoke {
                    note_id: 2,
                    channel: 0,
                    key: 60,
                },
            )],
        );
        assert!(!s.has_note(2));
    }
    #[test]
    fn loop_delay_and_offset_use_frame_units() {
        let (_dir, mut s) = fixture(
            "<region> sample=sample.wav offset=995 end=999 loop_start=995 loop_end=999 loop_mode=loop_continuous delay=0.002",
        );
        let y = render(&mut s, 10, &[(0, on(1, 60))]);
        assert_eq!(&y[..2], &[0., 0.]);
        assert_eq!(&y[2..], &[1.; 8]);
    }
    #[test]
    fn elapsed_seek_is_rejected_before_note_selection() {
        let (_dir, mut s) = fixture("<region> sample=sample.wav");
        let mut event = on(1, 60);
        if let DeviceEventKind::NoteOn {
            ref mut elapsed_frames,
            ..
        } = event
        {
            *elapsed_frames = 100;
        }
        let mut l = [0.];
        let mut r = [0.];
        assert!(
            s.process(
                ProcessContext {
                    frames: 1,
                    block_start_sample: 0,
                    transport: snapshot()
                },
                &[DeviceEvent {
                    offset: 0,
                    kind: event
                }],
                &mut l,
                &mut r
            )
            .is_err()
        );
        assert!(!s.has_note(1));
    }
    #[test]
    fn note_polyphony_counts_voices_and_chokes_sisters_like_reference() {
        for (limit, expected) in [
            (0, [1, 1, 1]),
            (1, [1, 1, 1]),
            (2, [2, 2, 2]),
            (3, [2, 2, 2]),
            (4, [2, 4, 4]),
        ] {
            let (_dir, mut s) = fixture(&format!(
                "<group> sample=sample.wav group=1 note_polyphony={limit} <region> <region>"
            ));
            for (i, count) in expected.into_iter().enumerate() {
                render(&mut s, 10, &[(0, on(i as u64 + 1, 60))]);
                assert_eq!(
                    s.voices.iter().filter(|v| v.active && !v.choked).count(),
                    count,
                    "limit{limit} note{i}"
                );
            }
        }
    }
    #[test]
    fn parameter_events_apply_at_exact_offset_without_rounding_hdcc() {
        let (_dir, mut s) = fixture("<region> sample=sample.wav volume_oncc24=-20");
        s.set_parameter_at("cc24", 63.5, 2).unwrap();
        let y = render(&mut s, 4, &[(0, on(1, 60))]);
        assert_eq!(&y[..2], &[1., 1.]);
        assert!((y[2] - 10f32.powf(-10. / 20.)).abs() < 1e-6);
        assert_eq!(s.cc[0][24], 0.5);
    }
    #[test]
    fn keyswitch_previous_and_musical_interval_use_distinct_history() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav sw_lokey=10 sw_hikey=100 sw_last=10 sw_default=10 <region> lokey=40",
        );
        render(&mut s, 1, &[(0, on(1, 60)), (0, on(2, 64))]);
        assert_eq!(s.virtual_cc[0][12], 4.);
        render(&mut s, 1, &[(0, on(3, 10))]);
        assert_eq!(s.previous[0], Some(10));
        assert_eq!(s.musical_previous[0], Some(64));
        assert_eq!(s.virtual_cc[0][12], 4.);
        render(&mut s, 1, &[(0, on(4, 67))]);
        assert_eq!(s.virtual_cc[0][12], 3.);
    }
    #[test]
    fn virtual_controller_history_updates_on_each_event() {
        let (_dir, mut s) = fixture("<region> sample=sample.wav");
        render(&mut s, 1, &[(0, on(1, 60))]);
        assert_eq!(s.virtual_cc[0][12], 0.);
        render(&mut s, 1, &[(0, on(2, 64))]);
        assert_eq!(s.virtual_cc[0][12], 4.);
        render(&mut s, 1, &[(0, off(1, 60))]);
        assert_eq!(s.virtual_cc[0][5], 60. / 127.);
        assert_eq!(s.virtual_cc[0][12], 4.);
    }
    #[test]
    fn loop_preserves_intro_before_first_loop_pass() {
        let (_dir, mut s) =
            fixture("<region> sample=sample.wav loop_start=4 loop_end=7 loop_mode=loop_continuous");
        s.samples[0].audio = (0..1000).map(|i| [i as f32; 2]).collect::<Vec<_>>().into();
        let values = render(&mut s, 12, &[(0, on(1, 60))]);
        assert_eq!(values, vec![0., 1., 2., 3., 4., 5., 6., 7., 4., 5., 6., 7.]);
    }
    #[test]
    fn controller_gates_velocity_bounds_and_release_duration_decay() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav <region> locc1=64 hicc1=127 lovel=100 hivel=127 <region> trigger=release rt_decay=20 volume=0",
        );
        assert_eq!(render(&mut s, 10, &[(0, on(1, 60))])[0], 0.);
        render(&mut s, 100, &[(0, cc(1, 64))]);
        let y = render(&mut s, 1, &[(0, off(1, 60))]);
        assert!((y[0] - 10f32.powf(-2.2 / 20.)).abs() < 1e-5);
        s.reset();
        assert_eq!(
            render(&mut s, 1, &[(0, cc(1, 63)), (0, on(2, 60))]),
            vec![0.]
        );
        s.reset();
        assert_eq!(
            render(&mut s, 1, &[(0, cc(1, 64)), (0, on(2, 60))]),
            vec![1.]
        );
    }
    #[test]
    fn delayed_attack_cancels_at_key_release_and_silent_region_chokes() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav <region> key=60 delay=0.1 group=1 off_by=2 <region> sample=*silence key=61 group=2",
        );
        render(&mut s, 1, &[(0, on(1, 60))]);
        assert!(s.has_note(1));
        render(&mut s, 1, &[(0, off(1, 60))]);
        assert!(!s.voices.iter().any(|v| v.active));
        render(&mut s, 1, &[(0, on(2, 60)), (0, on(3, 61))]);
        assert!(!s.voices.iter().any(|v| v.active));
    }
    #[test]
    fn automation_does_not_replace_program_controller_defaults_on_reset() {
        let (_dir, mut s) = fixture("<region> sample=sample.wav");
        s.set_parameter_at("cc24", 100., 0).unwrap();
        render(&mut s, 1, &[]);
        assert_eq!(s.cc[0][24], 100. / 127.);
        s.reset();
        assert_eq!(s.cc[0][24], 0.);
        s.set_parameter("cc24", 64.).unwrap();
        s.reset();
        assert_eq!(s.cc[0][24], 64. / 127.);
    }
    #[test]
    fn statistics_report_shared_samples_candidates_and_voice_steals() {
        let (_dir, mut s) =
            fixture("<group> sample=sample.wav group=1 note_polyphony=1 <region> <region>");
        render(&mut s, 10, &[(0, on(1, 60))]);
        let statistics = s.sfz_statistics().unwrap();
        assert_eq!(statistics.decoded_frames, 1000);
        assert_eq!(
            statistics.max_sample_frames,
            model::DEFAULT_SFZ_SAMPLE_FRAMES
        );
        assert_eq!(statistics.matched_regions, 2);
        assert_eq!(statistics.started_voices, 2);
        assert_eq!(statistics.stolen_voices, 1);
        assert_eq!(statistics.active_voices, 1);
        assert_eq!(statistics.dropped_regions, 0);
        s.reset();
        assert_eq!(s.sfz_statistics().unwrap().started_voices, 0);
        assert_eq!(s.sfz_statistics().unwrap().decoded_frames, 1000);
    }
    #[test]
    fn note_polyphony_velocity_selfmask_protects_louder_voices() {
        let (_dir, mut s) = fixture("<region> sample=sample.wav group=1 note_polyphony=1");
        for (i, (velocity, count)) in [(127, 1), (64, 2), (64, 2)].into_iter().enumerate() {
            let mut note = on(i as u64 + 1, 60);
            if let DeviceEventKind::NoteOn {
                velocity: ref mut v,
                ..
            } = note
            {
                *v = velocity as f32 / 127.;
            }
            render(&mut s, 10, &[(0, note)]);
            assert_eq!(
                s.voices.iter().filter(|v| v.active && !v.choked).count(),
                count
            );
        }
        assert!(s.voices.iter().any(|v| v.active && s.notes[v.note].id == 1));
    }
    #[test]
    fn checkpoint_restores_exact_owned_voice_expression_controller_and_selection_state() {
        let (_dir, mut s) = fixture(
            "<group> sample=sample.wav seq_length=2 delay_random=0.003 pitch_random=20 ampeg_release=0.01 <region> seq_position=1 lorand=0 hirand=0.5 <region> seq_position=1 lorand=0.5 hirand=1 <region> seq_position=2 volume=-6",
        );
        render(&mut s, 10, &[(0, cc(1, 90)), (0, on(1, 60))]);
        s.capture_checkpoint().unwrap();
        let events = [
            (
                0,
                DeviceEventKind::NoteExpression {
                    note_id: 1,
                    channel: 0,
                    key: 60,
                    expression: 4,
                    value: 0.25,
                },
            ),
            (2, on(2, 64)),
            (6, off(1, 60)),
        ];
        let expected = render(&mut s, 32, &events);
        for _ in 0..3 {
            assert!(s.restore_checkpoint().unwrap());
            assert_eq!(s.cc[0][1], 90. / 127.);
            assert!(s.has_note(1));
            assert_eq!(render(&mut s, 32, &events), expected);
        }
        s.reset();
        assert!(s.restore_checkpoint().unwrap());
        assert_eq!(render(&mut s, 32, &events), expected);
    }
    #[test]
    fn runtime_offset_builtin_inverse_curve_is_prepared_and_applied() {
        let (_dir, mut s) =
            fixture("<region> sample=sample.wav offset_oncc25=100 offset_curvecc25=2");
        render(&mut s, 1, &[(0, on(1, 60))]);
        let voice = s.voices.iter().find(|v| v.active).unwrap();
        assert_eq!(voice.pos, 101.);
        s.reset();
        render(&mut s, 1, &[(0, cc(25, 127)), (0, on(1, 60))]);
        let voice = s.voices.iter().find(|v| v.active).unwrap();
        assert_eq!(voice.pos, 1.);
    }
    #[test]
    fn controller_latched_note_parameters_follow_same_frame_event_order() {
        let (_dir, mut s) = fixture("<region> sample=sample.wav ampeg_attack_oncc1=0.01");
        let values = render(
            &mut s,
            1,
            &[
                (0, cc(1, 0)),
                (0, on(1, 60)),
                (0, cc(1, 127)),
                (0, on(2, 60)),
            ],
        );
        assert!(
            (values[0] - 1.1).abs() < 1e-6,
            "earlier note attack latched later CC: {:?}",
            values
        );
        assert_eq!(s.sfz_region_activity().unwrap(), &[2]);
    }
    #[test]
    fn decoded_footprint_checks_platform_address_space_without_allocating() {
        assert_eq!(decoded_bytes(100).unwrap(), 800);
        assert!(decoded_bytes(usize::MAX).is_err());
        let bound = isize::MAX as usize / 8;
        assert!(decoded_bytes(bound).is_ok());
        assert!(decoded_bytes(bound + 1).is_err());
    }
    #[test]
    fn note_virtual_velocity_is_captured_before_later_overlapping_note() {
        let (_dir, mut s) = fixture("<region> sample=sample.wav amplitude_oncc131=100");
        render(&mut s, 1, &[(0, on(1, 60))]);
        let mut second = on(2, 60);
        if let DeviceEventKind::NoteOn {
            ref mut velocity, ..
        } = second
        {
            *velocity = 64. / 127.;
        }
        let value = render(&mut s, 1, &[(0, second)])[0];
        assert!((value - (1. + (64f32 / 127.).powi(3))).abs() < 1e-6);
    }
}
