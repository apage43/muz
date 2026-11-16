//! `MUZ_PLUGIN_TRACE=DIR`: a per-call record of what the host gives each plugin
//! instance and what it returns, for telling host from plugin nondeterminism.
//!
//! [`create_processor`](super::create_processor) wraps every CLAP and VST3
//! processor in a [`PluginTrace`] when the variable names a directory. Each
//! instance writes `DIR/<device id>.<n>.trace`: a header with the device
//! description, then one line per block. Every input the host controls has its
//! own digest, so diffing the traces of two renders of one source shows which
//! input first differed, or that the inputs agree and only the output differs:
//!
//! ```text
//! 41 frame=10496 frames=256 ctx=… events=2:… params=0:… in=… out=…
//! ```
//!
//! `ctx` covers the block start, length and transport; `events` each event
//! with its sample offset; `params` the parameter changes and resets that
//! arrived since the previous block, with their offsets; `in` the input
//! audio (and any sidechain); `out` the output audio. A `skip` line marks a
//! block the plugin was not called for. Digests are FNV-1a over exact values,
//! so equal digests mean bit-equal inputs. Instances of one device id are
//! numbered in creation order, which is stable for a render with one tap.
//!
//! Tracing formats, hashes and writes on the processing thread. It is a
//! diagnostic for offline renders, not for live playback.

use std::{
    fmt::{self, Write as _},
    fs::{File, OpenOptions},
    io::{BufWriter, Write as _},
    path::{Path, PathBuf},
};

use crate::model;

use super::{
    DeviceDebugState, DeviceError, DeviceEvent, DeviceProcessor, ProcessContext, SfzStatistics,
};

pub const ENV: &str = "MUZ_PLUGIN_TRACE";

/// FNV-1a, 64 bits. Also a `fmt::Write` sink, so `Debug` output hashes without
/// allocating; `Debug` prints floats in shortest round-trip form, which
/// distinguishes every distinct finite value.
#[derive(Clone, Copy)]
struct Fnv(u64);
impl Fnv {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn debug(&mut self, value: &dyn fmt::Debug) {
        let _ = write!(self, "{value:?};");
    }
    fn samples(&mut self, samples: &[f32]) {
        for x in samples {
            self.bytes(&x.to_bits().to_le_bytes());
        }
    }
}
impl fmt::Write for Fnv {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

/// The trace directory, when tracing is on.
pub fn directory() -> Option<PathBuf> {
    std::env::var_os(ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// A plugin processor that forwards every call and records each block.
pub struct PluginTrace {
    inner: Box<dyn DeviceProcessor>,
    out: BufWriter<File>,
    call: u64,
    params: Fnv,
    param_count: u32,
    reset: bool,
}

impl PluginTrace {
    /// Wrap `inner`, creating the next free `<id>.<n>.trace` in `dir`.
    pub fn new(
        dir: &Path,
        device: &model::Device,
        inner: Box<dyn DeviceProcessor>,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let stem: String = device
            .id
            .as_str()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || "-_.".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let mut n = 0u32;
        let file = loop {
            let path = dir.join(format!("{stem}.{n}.trace"));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => break file,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e),
            }
        };
        let mut description = Fnv::new();
        description.debug(&device.kind);
        description.debug(&device.params);
        if let Some(plugin) = &device.vst3 {
            description.debug(&(&plugin.bundle_env, &plugin.class_id));
            if let Some(state) = &plugin.state {
                match std::fs::read(state) {
                    Ok(bytes) => description.bytes(&bytes),
                    Err(_) => description.debug(state),
                }
            }
        }
        let mut out = BufWriter::new(file);
        writeln!(
            out,
            "# {} {:?} device={:016x}",
            device.id, device.kind, description.0
        )?;
        Ok(Self {
            inner,
            out,
            call: 0,
            params: Fnv::new(),
            param_count: 0,
            reset: false,
        })
    }

    fn record(
        &mut self,
        ctx: &ProcessContext,
        events: &[DeviceEvent],
        input: Fnv,
        output: Option<(&[f32], &[f32])>,
    ) {
        let mut c = Fnv::new();
        c.debug(ctx);
        let mut e = Fnv::new();
        for event in events {
            e.debug(event);
        }
        let mut p = self.params;
        if self.reset {
            p.debug(&"reset");
        }
        let result = match output {
            Some((left, right)) => {
                let mut o = Fnv::new();
                o.samples(&left[..ctx.frames]);
                o.samples(&right[..ctx.frames]);
                writeln!(
                    self.out,
                    "{} frame={} frames={} ctx={:016x} events={}:{:016x} params={}{}:{:016x} in={:016x} out={:016x}",
                    self.call,
                    ctx.block_start_sample,
                    ctx.frames,
                    c.0,
                    events.len(),
                    e.0,
                    self.param_count,
                    if self.reset { "+reset" } else { "" },
                    p.0,
                    input.0,
                    o.0
                )
            }
            None => writeln!(
                self.out,
                "{} frame={} frames={} skip",
                self.call, ctx.block_start_sample, ctx.frames
            ),
        };
        if result.is_err() {
            eprintln!("{ENV}: could not write a trace line");
        }
        self.call += 1;
        self.params = Fnv::new();
        self.param_count = 0;
        self.reset = false;
    }
}

impl Drop for PluginTrace {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

impl DeviceProcessor for PluginTrace {
    fn sfz_region_activity(&self) -> Option<&[u64]> {
        self.inner.sfz_region_activity()
    }
    fn prepare_loop_checkpoint(&mut self) -> Result<(), DeviceError> {
        self.inner.prepare_loop_checkpoint()
    }
    fn restore_loop_checkpoint(&mut self) -> Result<bool, DeviceError> {
        self.inner.restore_loop_checkpoint()
    }
    fn sfz_statistics(&self) -> Option<SfzStatistics> {
        self.inner.sfz_statistics()
    }
    fn kind(&self) -> model::DeviceKind {
        self.inner.kind()
    }
    fn debug_state(&self) -> DeviceDebugState {
        self.inner.debug_state()
    }
    fn has_note(&self, note_id: u64) -> bool {
        self.inner.has_note(note_id)
    }
    fn set_parameter(&mut self, name: &str, value: f32) -> Result<(), DeviceError> {
        self.params.debug(&(name, value));
        self.param_count += 1;
        self.inner.set_parameter(name, value)
    }
    fn accepts_note_expression(&self, kind: u8) -> bool {
        self.inner.accepts_note_expression(kind)
    }
    fn check_note_expression(&self, kind: u8, value: f64) -> Result<(), String> {
        self.inner.check_note_expression(kind, value)
    }
    fn accepts_parameter_offsets(&self) -> bool {
        self.inner.accepts_parameter_offsets()
    }
    fn block_invariant(&self) -> bool {
        self.inner.block_invariant()
    }
    fn skip_silent_block(&mut self, ctx: &ProcessContext) -> bool {
        let skipped = self.inner.skip_silent_block(ctx);
        if skipped {
            self.record(ctx, &[], Fnv::new(), None);
        }
        skipped
    }
    fn set_parameter_at(&mut self, name: &str, value: f32, offset: u32) -> Result<(), DeviceError> {
        self.params.debug(&(name, value, offset));
        self.param_count += 1;
        self.inner.set_parameter_at(name, value, offset)
    }
    fn reset(&mut self) {
        self.reset = true;
        self.inner.reset();
    }
    fn scheduled_event_capacity(&self) -> usize {
        self.inner.scheduled_event_capacity()
    }
    fn reserve_scheduled_events(&mut self, events: usize) -> Result<(), DeviceError> {
        self.inner.reserve_scheduled_events(events)
    }
    fn process_sidechain(
        &mut self,
        ctx: ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
        detector_left: &[f32],
        detector_right: &[f32],
    ) -> Result<(), DeviceError> {
        let mut input = Fnv::new();
        input.samples(&left[..ctx.frames.min(left.len())]);
        input.samples(&right[..ctx.frames.min(right.len())]);
        input.samples(&detector_left[..ctx.frames.min(detector_left.len())]);
        input.samples(&detector_right[..ctx.frames.min(detector_right.len())]);
        let result =
            self.inner
                .process_sidechain(ctx, events, left, right, detector_left, detector_right);
        if result.is_ok() {
            self.record(&ctx, events, input, Some((left, right)));
        }
        result
    }
    fn process(
        &mut self,
        ctx: ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), DeviceError> {
        let mut input = Fnv::new();
        input.samples(&left[..ctx.frames.min(left.len())]);
        input.samples(&right[..ctx.frames.min(right.len())]);
        let result = self.inner.process(ctx, events, left, right);
        if result.is_ok() {
            self.record(&ctx, events, input, Some((left, right)));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{device::DeviceEventKind, transport::TransportSnapshot};

    /// A stand-in plugin: a gain whose output can be perturbed from outside,
    /// like a plugin with internal nondeterminism.
    struct Fake {
        gain: f32,
        wobble: std::sync::Arc<std::sync::atomic::AtomicU32>,
    }
    impl DeviceProcessor for Fake {
        fn kind(&self) -> model::DeviceKind {
            model::DeviceKind::Clap
        }
        fn debug_state(&self) -> DeviceDebugState {
            DeviceDebugState {
                instance_token: 0,
                process_count: 0,
                gain_reduction_db: 0.,
                latency_samples: 0,
                tail_samples: 0,
                is_plugin: true,
                restart_flags: 0,
            }
        }
        fn set_parameter(&mut self, _: &str, value: f32) -> Result<(), DeviceError> {
            self.gain = value;
            Ok(())
        }
        fn reset(&mut self) {}
        fn process(
            &mut self,
            ctx: ProcessContext,
            _: &[DeviceEvent],
            left: &mut [f32],
            right: &mut [f32],
        ) -> Result<(), DeviceError> {
            for x in left[..ctx.frames]
                .iter_mut()
                .chain(&mut right[..ctx.frames])
            {
                *x = *x * self.gain
                    + f32::from_bits(self.wobble.load(std::sync::atomic::Ordering::Relaxed));
            }
            Ok(())
        }
    }

    fn device() -> model::Device {
        let mut d: model::Device = serde_json::from_value(
            serde_json::json!({"id": "nylon/inst", "kind": "clap", "params": {}}),
        )
        .unwrap();
        d.params.clear();
        d
    }

    fn ctx(block: u64) -> ProcessContext {
        ProcessContext {
            frames: 4,
            block_start_sample: block * 4,
            transport: TransportSnapshot {
                sample_rate: 48000.,
                running: true,
                sample_position: block * 4,
                beat_position: 0.,
                current_tick: 0.,
                project_frame: (block * 4) as f64,
                bpm: 120.,
                meter: [4, 4],
                bar_start: 0.,
                bar_index: 0,
                ended: false,
            },
        }
    }

    /// Run four blocks; `offset` is the second block's note offset, `wobble`
    /// perturbs the output from the third block on.
    fn run(dir: &Path, offset: u32, wobble: f32) -> Vec<String> {
        let hidden = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let mut trace = PluginTrace::new(
            dir,
            &device(),
            Box::new(Fake {
                gain: 1.,
                wobble: hidden.clone(),
            }),
        )
        .unwrap();
        let note = DeviceEvent {
            offset,
            kind: DeviceEventKind::NoteOff {
                note_id: 1,
                channel: 0,
                key: 60,
                velocity: 0.5,
            },
        };
        for block in 0..4u64 {
            if block == 2 {
                trace.set_parameter_at("gain", 0.5, 1).unwrap();
            }
            let mut left = [0.25f32; 4];
            let mut right = [0.5f32; 4];
            let events: &[DeviceEvent] = if block == 1 { &[note] } else { &[] };
            if block >= 2 {
                // State the host does not see, like a plugin's own threads.
                hidden.store(wobble.to_bits(), std::sync::atomic::Ordering::Relaxed);
            }
            trace
                .process(ctx(block), events, &mut left, &mut right)
                .unwrap();
        }
        drop(trace);
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        std::fs::read_to_string(files.last().unwrap())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn field<'a>(line: &'a str, name: &str) -> &'a str {
        line.split(' ')
            .find_map(|f| f.strip_prefix(name).and_then(|f| f.strip_prefix('=')))
            .unwrap()
    }

    #[test]
    fn identical_runs_trace_identically_and_name_instances_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = run(dir.path(), 2, 0.);
        let b = run(dir.path(), 2, 0.);
        assert_eq!(a, b);
        assert_eq!(a.len(), 5, "a header and one line per block");
        assert!(a[0].starts_with("# nylon/inst Clap device="));
        assert!(dir.path().join("nylon_inst.0.trace").exists());
        assert!(dir.path().join("nylon_inst.1.trace").exists());
        assert_eq!(field(&a[2], "events"), field(&b[2], "events"));
        assert!(field(&a[3], "params").starts_with("1:"));
        assert!(field(&a[4], "params").starts_with("0:"));
    }

    #[test]
    fn a_plugin_that_diverges_on_equal_inputs_shows_only_in_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let host = run(dir.path(), 2, 0.);
        let plugin = run(dir.path(), 2, 1e-7);
        for (a, b) in host.iter().zip(&plugin).skip(1) {
            for input in ["frame", "frames", "ctx", "events", "params", "in"] {
                assert_eq!(field(a, input), field(b, input), "{input} in {a}");
            }
        }
        assert_eq!(field(&host[2], "out"), field(&plugin[2], "out"));
        assert_ne!(field(&host[3], "out"), field(&plugin[3], "out"));
    }

    #[test]
    fn a_moved_event_offset_shows_in_the_events_digest_of_its_block() {
        let dir = tempfile::tempdir().unwrap();
        let a = run(dir.path(), 2, 0.);
        let b = run(dir.path(), 3, 0.);
        assert_eq!(field(&a[1], "events"), field(&b[1], "events"));
        assert_ne!(field(&a[2], "events"), field(&b[2], "events"));
        assert_eq!(field(&a[2], "ctx"), field(&b[2], "ctx"));
    }
}
