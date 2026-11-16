use crate::{
    audio::{AudioConfig, AudioEngine},
    compile,
    lang::{Diagnostic, suggest_vocabulary},
    model::TrackSource,
};
use anyhow::{Result, bail};
use serde::Serialize;
use std::path::Path;

/// Name the output path on filesystem and encoder failures, which carry no span.
fn on_path(error: impl Into<anyhow::Error>, path: &Path) -> anyhow::Error {
    Diagnostic::named(error.into(), path)
}
#[derive(Debug, Serialize, serde::Deserialize)]
pub struct RenderReport {
    pub source: Option<String>,
    pub revision: Option<u64>,
    pub scope: RenderOptions,
    pub track_meters: Vec<TrackMeter>,
    /// Wall-clock seconds each physical track's chain (instrument and
    /// inserts) took over the whole render, history included, on whichever
    /// thread ran it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub track_seconds: Vec<(String, f64)>,
    /// Wall-clock seconds the engine spent rendering, buses and master included.
    #[serde(default)]
    pub cpu_seconds: f64,
    /// Latency of the rendered output: its first frame is the graph's output
    /// this many samples after the start, which compensation removes. A
    /// track tap's is its chain's latency.
    #[serde(default)]
    pub latency_samples: u64,
    pub frames: u64,
    pub sample_rate: u32,
    pub seconds: f64,
    pub sample_peak_dbfs: f64,
    pub rms_dbfs: f64,
    pub output: String,
    /// A batch output with several taps: one report per `TAP.wav` in `output`,
    /// a directory. The top-level peak and RMS are then the loudest tap's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub taps: Vec<RenderReport>,
}
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct TrackMeter {
    pub track: String,
    pub peak_dbfs: f64,
    pub rms_dbfs: f64,
}
pub fn render(
    path: &Path,
    out: &Path,
    seconds: Option<f64>,
    section: Option<&str>,
    solo: &[String],
    rate: u32,
    block: usize,
) -> Result<RenderReport> {
    let c = compile::compile(path)?;
    render_with(
        c.session,
        out,
        &RenderOptions {
            seconds,
            section: section.map(str::to_owned),
            solo: solo.to_vec(),
            sample_rate: rate,
            block_size: block,
            ..Default::default()
        },
        None,
    )
}
#[derive(Clone, Debug, clap::Args, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RenderOptions {
    /// Total output duration in seconds, including release audio; not extended by --tail.
    #[arg(long)]
    pub seconds: Option<f64>,
    /// Render a named section with preceding playback history.
    #[arg(long)]
    pub section: Option<String>,
    #[arg(long)]
    pub solo: Vec<String>,
    /// Silence a track, kit group (its `NAME.*` tracks) or bus: its output and
    /// sends, after --solo. Repeat for more. Taps keep their audio.
    #[arg(long, value_name = "TRACK_GROUP_OR_BUS")]
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "tap_list::deserialize")]
    pub mute: Vec<String>,
    #[arg(long, default_value_t = 48000)]
    pub sample_rate: u32,
    #[arg(long, default_value_t = 256)]
    pub block_size: usize,
    /// Clock-time output start in seconds; overrides the selected section's start.
    #[arg(long)]
    pub start: Option<f64>,
    /// Release/effect time after the score or section end; does not extend --seconds.
    #[arg(long)]
    pub tail: Option<f64>,
    /// Process only this many seconds before the output start instead of the whole
    /// song before it. Faster but approximate: notes sounding at the history start
    /// are retriggered there, and longer instrument/effect history is lost.
    #[arg(long, value_name = "SECONDS")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<f64>,
    /// Capture a track, kit group or bus output instead of the master. A group name
    /// (a kit's `drums`) taps the sum of its `drums.*` tracks. Append `:post` for the
    /// point after the output fader and its `.out` lane. `TRACK@PROBE` captures a
    /// voice patch's named probe signal, raw, without changing the mix. Repeat to write several
    /// taps from one pass; -o then names a directory that receives TAP.wav files.
    /// `muz render --list-taps SOURCE` lists the accepted names.
    #[arg(long, value_name = "TRACK_GROUP_OR_BUS")]
    #[serde(with = "tap_list")]
    pub tap: Vec<String>,
    #[arg(long, default_value = "float32")]
    pub format: String,
    /// Threads for independent tracks; output is bit-identical for any count.
    #[arg(long, default_value_t = 1)]
    pub threads: usize,
    /// With --threads, keep every plugin-hosting track on the rendering thread.
    #[arg(long)]
    pub serial_plugins: bool,
}
impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            seconds: None,
            section: None,
            solo: Vec::new(),
            mute: Vec::new(),
            sample_rate: 48000,
            block_size: 256,
            start: None,
            tail: None,
            history: None,
            tap: Vec::new(),
            format: "float32".into(),
            threads: 1,
            serial_plugins: false,
        }
    }
}
/// Recipes, the control protocol and reports keep a single tap as a plain string;
/// several taps use a list.
mod tap_list {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(taps: &[String], s: S) -> Result<S::Ok, S::Error> {
        match taps {
            [] => s.serialize_none(),
            [one] => s.serialize_some(one),
            many => s.serialize_some(many),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Taps {
            One(String),
            Many(Vec<String>),
        }
        Ok(match Option::<Taps>::deserialize(d)? {
            None => Vec::new(),
            Some(Taps::One(one)) => vec![one],
            Some(Taps::Many(many)) => many,
        })
    }
}
/// Dither seeds: a tap file from a multi-tap pass equals the same single-tap render.
const RENDER_DITHER_SEED: u64 = 0x5ad9312;
const STEM_DITHER_SEED: u64 = 0xa52983;

pub fn render_with(
    s: crate::Session,
    out: &Path,
    options: &RenderOptions,
    job: Option<&crate::control::RenderProgress>,
) -> Result<RenderReport> {
    if options.tap.len() > 1 {
        return Err(Diagnostic::new("several taps need a directory output")
            .help("use disk `muz render --tap A --tap B -o DIR`, or a batch output with a tap list; queued renders take one tap")
            .err());
    }
    let rate = options.sample_rate;
    let block = options.block_size;
    let source = s.extras.source.as_ref().map(|p| p.display().to_string());
    let (mut engine, first, duration) = prepare(s, options)?;
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| on_path(e, out))?;
    let tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| on_path(e, out))?;
    let (bits, sample_format) = match options.format.as_str() {
        "float32" => (32, hound::SampleFormat::Float),
        "pcm16" => (16, hound::SampleFormat::Int),
        "pcm24" => (24, hound::SampleFormat::Int),
        other => {
            return Err(Diagnostic::new("format must be float32, pcm16 or pcm24")
                .helps(suggest_vocabulary(
                    "formats",
                    other,
                    ["float32", "pcm16", "pcm24"],
                ))
                .err());
        }
    };
    let frames = (duration * rate as f64).round() as u64;
    let file = tmp.reopen().map_err(|e| on_path(e, out))?;
    let spec = crate::audio_file::WriteSpec {
        channels: 2,
        rate,
        bits,
        float: sample_format == hound::SampleFormat::Float,
        frames,
    };
    let mut writer =
        crate::audio_file::AudioWriter::new(out, file, spec).map_err(|e| on_path(e, out))?;
    let begin = first + engine.latency_samples() as u64;
    let total = begin + frames;
    let mut processed = 0u64;
    let mut last_progress = 0;
    let mut buf = vec![0.0f32; block * 2];
    let (mut peak, mut square) = (0.0f64, 0.0f64);
    let mut meter = vec![(0.0f64, 0.0f64, 0u64); engine.track_count()];
    let mut cpu = Cpu::new(&engine);
    let mut rng = RENDER_DITHER_SEED;
    engine.set_running(true);
    while processed < total {
        crate::audio::clap::service_main_thread();
        if crate::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("render interrupted; output preserved");
        }
        let count = (total - processed).min(block as u64) as usize;
        engine.render_interleaved(&mut buf[..count * 2], 2)?;
        cpu.add(&engine);
        accumulate_meters(&engine, &mut meter, processed, count, first, frames);
        for (i, pair) in buf[..count * 2].as_chunks::<2>().0.iter().enumerate() {
            if processed + i as u64 >= begin {
                for &x in pair {
                    if !x.is_finite() {
                        bail!("non-finite audio; output preserved");
                    }
                    peak = peak.max(x.abs() as f64);
                    square += (x as f64).powi(2);
                    if sample_format == hound::SampleFormat::Float {
                        writer.write_f32(x).map_err(|e| on_path(e, out))?;
                    } else {
                        if x.abs() > 1.0 {
                            return Err(Diagnostic::new(
                                "PCM export would clip; lower the master or use float32",
                            )
                            .help("use --format float32, or lower the master level")
                            .err());
                        }
                        let scale = (1u64 << (bits - 1)) as f64;
                        let mut uniform = || {
                            rng ^= rng << 13;
                            rng ^= rng >> 7;
                            rng ^= rng << 17;
                            (rng >> 11) as f64 / (1u64 << 53) as f64
                        };
                        let dither = uniform() - uniform();
                        writer
                            .write_int(
                                (x as f64 * scale + dither)
                                    .round()
                                    .clamp(-scale, scale - 1.0)
                                    as i32,
                            )
                            .map_err(|e| on_path(e, out))?;
                    }
                }
            }
        }
        processed += count as u64;
        if let Some(job) = job {
            job.processed
                .store(processed, std::sync::atomic::Ordering::Relaxed);
            job.total.store(total, std::sync::atomic::Ordering::Relaxed);
            if processed - last_progress >= rate as u64 || processed == total {
                if let Some(file) = &job.file {
                    std::fs::write(file, serde_json::to_vec(&[processed, total])?)?;
                }
                last_progress = processed;
            }
            if job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                bail!("render cancelled; output preserved");
            }
        }
    }
    writer.finish().map_err(|e| on_path(e, out))?;
    tmp.persist(out).map_err(|e| on_path(e, out))?;
    Ok(RenderReport {
        source,
        revision: None,
        scope: options.clone(),
        track_meters: track_meters(&engine, meter),
        track_seconds: cpu.track_seconds(&engine),
        cpu_seconds: cpu.total,
        latency_samples: engine.latency_samples() as u64,
        frames,
        sample_rate: rate,
        seconds: frames as f64 / rate as f64,
        sample_peak_dbfs: 20.0 * peak.max(1e-20).log10(),
        rms_dbfs: 10.0 * (square / (frames * 2) as f64).max(1e-40).log10(),
        output: out.display().to_string(),
        taps: Vec::new(),
    })
}

/// Per-track peak, sum of squares and sample count inside the output window,
/// which starts at frame `first` plus each track's latency.
fn accumulate_meters(
    engine: &AudioEngine,
    meter: &mut [(f64, f64, u64)],
    processed: u64,
    count: usize,
    first: u64,
    frames: u64,
) {
    for (index, values) in meter.iter_mut().enumerate() {
        let (_, l, r, lat) = engine.track_audio(index);
        let begin = first + lat as u64;
        for i in 0..count {
            let frame = processed + i as u64;
            if frame >= begin && frame < begin + frames {
                values.0 = values.0.max(l[i].abs() as f64).max(r[i].abs() as f64);
                values.1 += (l[i] as f64).powi(2) + (r[i] as f64).powi(2);
                values.2 += 2;
            }
        }
    }
}
/// Per-track and whole-render processing time, summed over render calls.
struct Cpu {
    tracks: Vec<f64>,
    total: f64,
}
impl Cpu {
    fn new(engine: &AudioEngine) -> Self {
        Self { tracks: vec![0.0; engine.track_count()], total: 0.0 }
    }
    fn add(&mut self, engine: &AudioEngine) {
        for (i, seconds) in self.tracks.iter_mut().enumerate() {
            *seconds += engine.track_cpu_seconds(i);
        }
        self.total += engine.render_cpu_seconds();
    }
    /// Physical tracks only, like the meters: probe copies are left out.
    fn track_seconds(&self, engine: &AudioEngine) -> Vec<(String, f64)> {
        self.tracks
            .iter()
            .enumerate()
            .filter(|(i, _)| !engine.track_audio(*i).0.contains('@'))
            .map(|(i, seconds)| (engine.track_audio(i).0.to_owned(), *seconds))
            .collect()
    }
}
fn track_meters(engine: &AudioEngine, meter: Vec<(f64, f64, u64)>) -> Vec<TrackMeter> {
    meter
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !engine.track_audio(*i).0.contains('@'))
        .map(|(i, (peak, square, n))| TrackMeter {
            track: engine.track_audio(i).0.to_owned(),
            peak_dbfs: 20. * peak.max(1e-20).log10(),
            rms_dbfs: 10. * (square / n.max(1) as f64).max(1e-40).log10(),
        })
        .collect()
}

/// What a tap names: a physical track, a bus, the sum of an expanded group, or
/// one of those after its output fader.
#[derive(Clone, Copy)]
enum Target {
    Track(usize),
    Bus(usize),
    Group(usize),
    Post(crate::audio::PostTap),
}
/// Resolve a validated tap name, enabling a post-fader point it names.
fn target(engine: &mut AudioEngine, id: &str) -> Option<Target> {
    if let Some(base) = id.strip_suffix(":post") {
        return engine.post_tap_for(base).map(Target::Post);
    }
    (0..engine.track_count())
        .find(|&i| engine.track_audio(i).0 == id)
        .map(Target::Track)
        .or_else(|| {
            (0..engine.bus_count())
                .find(|&i| engine.bus_audio(i).0 == id)
                .map(Target::Bus)
        })
        .or_else(|| engine.tap_group_for(id).map(Target::Group))
}

/// A tap's audio after the latest block.
fn tap_audio(engine: &AudioEngine, target: Target) -> (&[f32], &[f32], usize) {
    match target {
        Target::Track(i) => {
            let (_, left, right, latency) = engine.track_audio(i);
            (left, right, latency)
        }
        Target::Bus(i) => {
            let (_, left, right, latency) = engine.bus_audio(i);
            (left, right, latency)
        }
        Target::Group(i) => engine.tap_group_audio(i),
        Target::Post(tap) => engine.post_tap_audio(tap),
    }
}

/// Several taps from one pass as one report: `output` is the directory, `taps`
/// holds each file's report, and the peak and RMS are the loudest tap's.
pub fn render_taps_report(
    s: crate::Session,
    out: &Path,
    options: &RenderOptions,
) -> Result<RenderReport> {
    Ok(combine_tap_reports(render_taps(s, out, options)?, out, options))
}

/// The one report of a multi-tap pass (one tap's report stays as it is):
/// `output` names `out`, `taps` holds each file's report, the track meters
/// and costs (the same for every tap of a pass) move to the top, and the peak
/// and RMS are the loudest tap's.
pub fn combine_tap_reports(mut taps: Vec<RenderReport>, out: &Path, options: &RenderOptions) -> RenderReport {
    if taps.len() == 1 {
        return taps.pop().unwrap();
    }
    let first = taps.first_mut().expect("a pass writes at least one tap");
    let track_meters = std::mem::take(&mut first.track_meters);
    let track_seconds = std::mem::take(&mut first.track_seconds);
    let cpu_seconds = first.cpu_seconds;
    let source = first.source.clone();
    for tap in &mut taps {
        tap.track_meters.clear();
        tap.track_seconds.clear();
        tap.source = None;
    }
    let loudest = |f: fn(&RenderReport) -> f64| {
        taps.iter().map(f).fold(f64::NEG_INFINITY, f64::max)
    };
    RenderReport {
        source,
        revision: None,
        scope: options.clone(),
        track_meters,
        track_seconds,
        cpu_seconds,
        latency_samples: 0,
        frames: taps[0].frames,
        sample_rate: taps[0].sample_rate,
        seconds: taps[0].seconds,
        sample_peak_dbfs: loudest(|r| r.sample_peak_dbfs),
        rms_dbfs: loudest(|r| r.rms_dbfs),
        output: out.display().to_string(),
        taps,
    }
}

/// Several graph boundaries from one engine pass, each written to `out/TAP.wav`.
/// Every file equals the single-tap render of that boundary, sample for sample.
pub fn render_taps(
    s: crate::Session,
    out: &Path,
    options: &RenderOptions,
) -> Result<Vec<RenderReport>> {
    if options.tap.len() < 2 {
        bail!("render_taps needs two or more taps; use render_with for one output");
    }
    if out
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
    {
        return Err(Diagnostic::new(format!(
            "several taps write one file per tap into a directory, not {}",
            out.display()
        ))
        .help("pass a directory to -o; each tap is written as DIR/TAP.wav")
        .err());
    }
    let paths: Vec<_> = options.tap.iter().map(|id| out.join(format!("{id}.wav"))).collect();
    render_taps_to(s, &paths, options, None)
}

/// One or more taps from one engine pass, `options.tap[i]` written to
/// `paths[i]` in the format its extension names (WAV, or WavPack for `.wv`).
/// Every file equals the single-tap render of that boundary, sample for
/// sample. Each report's `scope.tap` names its one tap. `progress`, when
/// given, gains this pass's frames in `total` when it starts and in
/// `processed` as it goes, so concurrent passes can share one; its `cancel`
/// stops the pass.
pub fn render_taps_to(
    s: crate::Session,
    paths: &[std::path::PathBuf],
    options: &RenderOptions,
    progress: Option<&crate::control::RenderProgress>,
) -> Result<Vec<RenderReport>> {
    if options.tap.is_empty() || options.tap.len() != paths.len() {
        bail!("render_taps_to needs one path per tap");
    }
    let source = s.extras.source.as_ref().map(|p| p.display().to_string());
    let (mut engine, first, duration) = prepare(s, options)?;
    for path in paths {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| on_path(e, path))?;
        }
    }
    let rate = options.sample_rate;
    let block = options.block_size;
    let frames = (duration * rate as f64).round() as u64;
    let targets: Vec<Target> = options
        .tap
        .iter()
        .map(|id| target(&mut engine, id).expect("taps are validated during preparation"))
        .collect();
    let mut sinks = targets
        .iter()
        .zip(paths)
        .map(|(&target, path)| {
            SampleSink::new(
                path.clone(),
                first + tap_audio(&engine, target).2 as u64,
                frames,
                rate,
                &options.format,
                RENDER_DITHER_SEED,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let total = sinks.iter().map(|s| s.begin + frames).max().unwrap_or(0);
    let mut meter = vec![(0.0f64, 0.0f64, 0u64); engine.track_count()];
    let mut cpu = Cpu::new(&engine);
    let mut processed = 0;
    let mut buffer = vec![0.; block * 2];
    use std::sync::atomic::Ordering::Relaxed;
    if let Some(p) = progress {
        p.total.fetch_add(total, Relaxed);
    }
    engine.set_running(true);
    while processed < total {
        crate::audio::clap::service_main_thread();
        if crate::INTERRUPTED.load(Relaxed) {
            bail!("render interrupted; existing outputs preserved");
        }
        if let Some(p) = progress {
            if p.cancel.load(Relaxed) {
                bail!("render cancelled; existing outputs preserved");
            }
            p.processed.fetch_add((total - processed).min(block as u64), Relaxed);
        }
        let count = (total - processed).min(block as u64) as usize;
        engine.render_interleaved(&mut buffer[..count * 2], 2)?;
        cpu.add(&engine);
        accumulate_meters(&engine, &mut meter, processed, count, first, frames);
        for (&target, sink) in targets.iter().zip(&mut sinks) {
            let (left, right, _) = tap_audio(&engine, target);
            for frame in 0..count {
                let at = processed + frame as u64;
                if at >= sink.begin && at < sink.begin + frames {
                    sink.write(left[frame])?;
                    sink.write(right[frame])?;
                }
            }
        }
        processed += count as u64;
    }
    let meters = track_meters(&engine, meter);
    let mut reports = sinks
        .into_iter()
        .map(SampleSink::finish)
        .collect::<Result<Vec<_>>>()?;
    for ((report, id), &target) in reports.iter_mut().zip(&options.tap).zip(&targets) {
        report.source = source.clone();
        report.scope = options.clone();
        report.scope.tap = vec![id.clone()];
        report.track_meters = meters.clone();
        report.track_seconds = cpu.track_seconds(&engine);
        report.cpu_seconds = cpu.total;
        report.latency_samples = tap_audio(&engine, target).2 as u64;
    }
    Ok(reports)
}

/// Taps grouped into independent passes: taps whose dependencies (see
/// [`crate::fingerprint::dependencies`]) share a track or bus render in the
/// same pass, so shared work runs once. Groups list tap indices in order.
pub fn independent_tap_groups(s: &crate::Session, taps: &[String]) -> Result<Vec<Vec<usize>>> {
    let deps = taps
        .iter()
        .map(|t| crate::fingerprint::dependencies(s, std::slice::from_ref(t)))
        .collect::<Result<Vec<_>>>()?;
    let mut group: Vec<usize> = (0..taps.len()).collect();
    fn root(group: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while group[r] != r {
            r = group[r];
        }
        group[i] = r;
        r
    }
    for i in 0..taps.len() {
        for j in 0..i {
            let shared = !deps[i].tracks.is_disjoint(&deps[j].tracks) || !deps[i].buses.is_disjoint(&deps[j].buses);
            if shared {
                let (a, b) = (root(&mut group, i), root(&mut group, j));
                group[a.max(b)] = a.min(b);
            }
        }
    }
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut roots: Vec<usize> = Vec::new();
    for i in 0..taps.len() {
        let r = root(&mut group, i);
        match roots.iter().position(|&x| x == r) {
            Some(k) => out[k].push(i),
            None => {
                roots.push(r);
                out.push(vec![i]);
            }
        }
    }
    Ok(out)
}

/// [`render_taps_to`], split into [`independent_tap_groups`] that each render
/// only their own dependencies ([`keep_tap_dependencies`]), several passes at
/// once, sharing `progress` ([`render_taps_to`]). `options.threads` is the
/// budget: up to that many passes run
/// together, each with an equal share for its own tracks. Every file equals
/// the single-tap render of the whole graph, sample for sample, for any
/// budget. Reports come back in tap order, each with its own pass's track
/// meters and costs. Passes run on worker threads, inside the caller's host
/// context (a CLI one when there is none), which also shares decoded
/// recordings and prepared programs between them.
pub fn render_taps_parallel(
    s: &crate::Session,
    paths: &[std::path::PathBuf],
    options: &RenderOptions,
    progress: Option<&crate::control::RenderProgress>,
) -> Result<Vec<RenderReport>> {
    if options.tap.is_empty() || options.tap.len() != paths.len() {
        bail!("render_taps_parallel needs one path per tap");
    }
    let groups = independent_tap_groups(s, &options.tap)?;
    let budget = options.threads.max(1);
    let workers = budget.min(groups.len());
    let share = (budget / workers).clamp(1, 256);
    let context = crate::host::current().unwrap_or_else(crate::host::HostContext::cli);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Result<Vec<RenderReport>>>>> =
        groups.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let pass = |g: &[usize]| -> Result<Vec<RenderReport>> {
        let taps: Vec<String> = g.iter().map(|&i| options.tap[i].clone()).collect();
        let mut session = s.clone();
        keep_tap_dependencies(&mut session, &taps)?;
        let options = RenderOptions { tap: taps, threads: share, ..options.clone() };
        let paths: Vec<_> = g.iter().map(|&i| paths[i].clone()).collect();
        render_taps_to(session, &paths, &options, progress)
    };
    let work = || {
        context.run(|| {
            loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(g) = groups.get(k) else { break };
                let result = pass(g);
                let failed = result.is_err();
                *results[k].lock().unwrap() = Some(result);
                if failed {
                    // Stop handing out passes; running ones finish.
                    next.store(groups.len(), std::sync::atomic::Ordering::Relaxed);
                }
            }
        })
    };
    if workers == 1 {
        work();
    } else {
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(work);
            }
        });
    }
    let mut reports: Vec<Option<RenderReport>> = (0..paths.len()).map(|_| None).collect();
    for (g, result) in groups.iter().zip(results) {
        match result.into_inner().unwrap() {
            Some(Ok(pass)) => {
                for (&i, report) in g.iter().zip(pass) {
                    reports[i] = Some(report);
                }
            }
            Some(Err(e)) => return Err(e),
            None => bail!("render stopped before every pass ran"),
        }
    }
    Ok(reports.into_iter().map(|r| r.expect("every tap belongs to a pass")).collect())
}

/// Expanded-track groups: every proper dotted prefix of a track id (`drums` for
/// `drums.kick`) that is not itself a track, in first-seen order.
fn tap_groups(s: &crate::Session) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    for t in &s.tracks {
        for (at, _) in t.id.as_str().match_indices('.') {
            let prefix = &t.id.as_str()[..at];
            if !prefix.is_empty()
                && !groups.iter().any(|g| g == prefix)
                && !s.tracks.iter().any(|o| o.id.as_str() == prefix)
            {
                groups.push(prefix.to_owned());
            }
        }
    }
    groups
}

/// Every tap name in listing order: tracks, kit/expanded groups, buses, master.
fn tap_names(s: &crate::Session) -> Vec<String> {
    s.tracks
        .iter()
        .map(|t| t.id.to_string())
        .chain(tap_groups(s))
        .chain(s.buses.iter().map(|b| b.id.to_string()))
        .chain(std::iter::once(s.master.id.to_string()))
        .collect()
}

/// The tap listing printed by `muz render --list-taps`.
pub fn list_taps(s: &crate::Session) -> String {
    let groups = tap_groups(s);
    let mut out = String::from("tracks (after inserts and pan, before output gain and sends):\n");
    for t in &s.tracks {
        out += &format!("  {}\n", t.id);
    }
    if !groups.is_empty() {
        out += "groups (sum of the member tracks' taps):\n";
        for g in &groups {
            let pieces = s
                .tracks
                .iter()
                .filter(|t| t.id.as_str().strip_prefix(g.as_str()).is_some_and(|r| r.starts_with('.')))
                .count();
            out += &format!("  {g} ({pieces} tracks)\n");
        }
    }
    let probes: Vec<String> = s
        .tracks
        .iter()
        .flat_map(|t| probe_names(t).into_iter().map(move |p| format!("{}@{p}", t.id)))
        .collect();
    if !probes.is_empty() {
        out += "probes (a voice patch's named signal, raw and summed over voices; the mix is unchanged):\n";
        for p in &probes {
            out += &format!("  {p}\n");
        }
    }
    out += "buses (after the bus chain, before its output fader and `.out` lane):\n";
    for b in &s.buses {
        out += &format!("  {}\n", b.id);
    }
    out += &format!("  {} (master)\n", s.master.id);
    out += "post-fader: append :post to a track, group or bus (not master), such as \
            `lead:post`, for its tap times its output fader and `.out` lane, before \
            sends, mute and solo\n";
    out
}

/// Taps name a physical track, an expanded-track group (the sum of its tracks), or
/// a bus (including `master`), each at most once.
/// A probe tap `TRACK@PROBE` hears a voice patch's named signal. It adds a
/// muted copy of the track whose patch emits that signal, raw and summed over
/// voices, instead of its output. The copy keeps the track's notes, controllers
/// and instrument automation and drops its inserts, pan, sends and fader, so the
/// copy's voices run exactly as the track's do and the mix is unchanged.
fn add_probe_tracks(s: &mut crate::Session, taps: &[String]) -> Result<Vec<String>> {
    let mut added = Vec::new();
    for tap in taps {
        let Some((track, probe)) = tap.rsplit_once('@') else {
            continue;
        };
        if s.tracks.iter().any(|t| t.id.as_str() == tap) {
            continue;
        }
        if tap.ends_with(":post") {
            return Err(Diagnostic::new(format!("probe tap {tap} has no post-fader point"))
                .help(format!("tap {} for the signal itself", tap.trim_end_matches(":post")))
                .err());
        }
        let Some(source) = s.tracks.iter().find(|t| t.id.as_str() == track) else {
            let names: Vec<&str> = s.tracks.iter().map(|t| t.id.as_str()).collect();
            return Err(Diagnostic::new(format!("probe tap {tap} names no track {track}"))
                .helps(suggest_vocabulary("tracks", track, names))
                .err());
        };
        let declared = probe_names(source);
        let Some(mut patch) = source
            .instrument
            .patch
            .clone()
            .filter(|_| declared.iter().any(|n| n == probe))
        else {
            let error = Diagnostic::new(format!("track {track} has no probe {probe}"));
            return Err(if declared.is_empty() {
                error.help("declare `probes: {name: signal}` on the track's voice_patch")
            } else {
                error.helps(suggest_vocabulary("probes", probe, declared.iter().map(String::as_str)))
            }
            .err());
        };
        patch["listen"] = serde_json::json!(probe);
        let mut copy = source.clone();
        copy.id = crate::model::Id::new(tap.as_str());
        copy.name = tap.clone();
        copy.instrument.patch = Some(patch);
        let TrackSource::Midi(m) = &mut copy.source;
        m.id = crate::model::Id::new(format!("{tap}.events"));
        copy.inserts.clear();
        copy.sends.clear();
        copy.output.id = crate::model::Id::new(format!("{tap}.out"));
        copy.output.gain_db = 0.;
        s.tracks.push(copy);
        added.push(tap.clone());
    }
    Ok(added)
}

/// The probes a track's voice patch declares.
fn probe_names(t: &crate::model::Track) -> Vec<String> {
    t.instrument
        .patch
        .as_ref()
        .and_then(|p| p.get("probes"))
        .and_then(|p| p.as_object())
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default()
}

fn validate_taps(s: &crate::Session, taps: &[String]) -> Result<()> {
    let owned = tap_names(s);
    let names: Vec<&str> = owned.iter().map(String::as_str).collect();
    for (i, full) in taps.iter().enumerate() {
        if taps[..i].contains(full) {
            return Err(
                Diagnostic::new(format!("tap {full} is listed more than once"))
                    .help("pass each --tap once")
                    .err(),
            );
        }
        let tap = full.strip_suffix(":post").unwrap_or(full);
        if tap != full && tap == s.master.id.as_str() {
            return Err(Diagnostic::new(format!("the master has no post-fader tap: use --tap {tap}"))
                .help("the master has no output fader; its tap is the mix")
                .err());
        }
        if !names.contains(&tap) {
            let mut help = suggest_vocabulary("taps", tap, names.iter().copied());
            if let Some((group, _)) = tap.rsplit_once('.') {
                let pieces: Vec<&str> = s
                    .tracks
                    .iter()
                    .map(|t| t.id.as_str())
                    .filter(|id| {
                        id.strip_prefix(group)
                            .is_some_and(|r| r.starts_with('.'))
                    })
                    .collect();
                if !pieces.is_empty() {
                    let shown = pieces.iter().take(16).copied().collect::<Vec<_>>().join(", ");
                    let more = pieces.len().saturating_sub(16);
                    help.push(if more > 0 {
                        format!("tracks of {group}: {shown}, and {more} more")
                    } else {
                        format!("tracks of {group}: {shown}")
                    });
                }
            }
            help.push(format!(
                "`muz render --list-taps SOURCE` lists all {} tap names; a group name taps the sum of its tracks",
                names.len()
            ));
            return Err(Diagnostic::new(format!("unknown tap {full}")).helps(help).err());
        }
    }
    Ok(())
}

/// Offline render takes its timeline from the first track's MIDI source, which
/// every compiled `.muz` session has.
fn timeline_source(s: &crate::Session) -> Result<&crate::model::MidiTrackSource> {
    match s.tracks.first().map(|t| &t.source) {
        Some(TrackSource::Midi(src)) => Ok(src),
        None => Err(Diagnostic::new("offline render needs at least one track").err()),
    }
}

/// Returns the engine, the first output frame counted from where it starts
/// processing (before latency), and the output duration in seconds.
fn prepare(mut s: crate::Session, options: &RenderOptions) -> Result<(AudioEngine, u64, f64)> {
    let rate = options.sample_rate;
    let block = options.block_size;
    let seconds = options.seconds;
    let section = options.section.as_deref();
    let probes = add_probe_tracks(&mut s, &options.tap)?;
    validate_taps(&s, &options.tap)?;
    let tail = options.tail.unwrap_or(s.extras.tail);
    if !tail.is_finite() || tail < 0.0 || tail > 600.0 {
        return Err(Diagnostic::new("tail must be 0..600 seconds")
            .help("pass --tail with a value in 0..600 seconds")
            .err());
    }
    let mut start = 0.0;
    let end = if let Some(section) = section {
        let names: Vec<&str> = s
            .extras
            .sections
            .iter()
            .map(|sec| sec.name.as_str())
            .collect();
        let sec = s
            .extras
            .sections
            .iter()
            .find(|sec| sec.name == section)
            .ok_or_else(|| {
                let error = Diagnostic::new(format!("unknown section {section}"));
                if names.is_empty() {
                    error.err()
                } else {
                    error
                        .helps(suggest_vocabulary(
                            "sections",
                            section,
                            names.iter().copied(),
                        ))
                        .err()
                }
            })?;
        let tempos = &timeline_source(&s)?.imported.tempos;
        start = compile::seconds_at(sec.start, tempos);
        compile::seconds_at(sec.end, tempos) + tail
    } else {
        let src = timeline_source(&s)?;
        compile::seconds_at(
            src.summary.end_tick as f64 / src.summary.ppq as f64,
            &src.imported.tempos,
        ) + tail
    };
    if let Some(at) = options.start {
        if !at.is_finite() || at < 0.0 {
            return Err(
                Diagnostic::new("start must be a nonnegative time in seconds")
                    .help("pass --start 0 or a positive number of seconds")
                    .err(),
            );
        }
        start = at;
    }
    let duration = seconds.unwrap_or(end - start);
    if !duration.is_finite() || duration <= 0.0 || duration > 36000.0 {
        return Err(Diagnostic::new("duration must be in 0..36000 seconds")
            .help("pass --seconds or --start for a positive duration under 36000 seconds")
            .err());
    }
    if !(1..=256).contains(&options.threads) {
        return Err(Diagnostic::new("render threads must be 1..256")
            .help("pass --threads 1 for serial rendering or up to the available cores")
            .err());
    }
    if !(8000..=192000).contains(&rate) {
        return Err(Diagnostic::new("sample rate must be 8000..192000")
            .help("pass --sample-rate between 8000 and 192000")
            .err());
    }
    if let Some(name) = section {
        let cut = compile::tick(
            s.extras
                .sections
                .iter()
                .find(|sec| sec.name == name)
                .unwrap()
                .end,
        );
        for t in &mut s.tracks {
            let TrackSource::Midi(m) = &mut t.source;
            m.imported.notes.retain(|n| n.start_tick < cut);
            for n in &mut m.imported.notes {
                n.duration_ticks = n.duration_ticks.min(cut - n.start_tick);
            }
            m.imported.controllers.retain(|c| c.tick < cut);
            m.imported.messages.retain(|c| c.tick < cut);
            m.imported.tempos.retain(|t| t.tick <= cut);
            let channels: std::collections::BTreeSet<_> =
                m.imported.notes.iter().map(|n| n.channel).collect();
            for channel in channels {
                for controller in [64, 66, 67] {
                    m.imported.controllers.push(crate::midi::MidiController {
                        tick: cut,
                        channel,
                        controller,
                        value: 0,
                        source_order: u32::MAX,
                    });
                }
            }
            m.summary.end_tick = cut;
            m.imported.summary.end_tick = cut;
            m.imported.summary.notes = m.imported.notes.len() as u32;
            m.imported.summary.controllers = m.imported.controllers.len() as u32;
            m.imported.summary.tempos = m.imported.tempos.len() as u32;
            m.summary = m.imported.summary.clone();
        }
    }
    let output_frame = (start * rate as f64).round() as u64;
    let origin = match options.history {
        None => 0,
        Some(history) => bound_history(&mut s, options, output_frame, history)?,
    };
    let mut engine = AudioEngine::new(
        &s,
        AudioConfig {
            sample_rate: rate as f32,
            max_frames: block,
            offline: true,
        },
    )?;
    if origin > 0 {
        engine.seek_project_frame(origin);
    }
    engine.set_solo(&options.solo)?;
    engine.set_mute(&options.mute)?;
    engine.set_mute(&probes)?;
    engine.set_tap(match options.tap.as_slice() {
        [one] => Some(one.as_str()),
        _ => None,
    })?;
    engine.set_render_threads(options.threads)?;
    engine.set_plugins_on_render_thread(options.serial_plugins);
    engine.set_cpu_timing(true);
    Ok((engine, output_frame - origin, duration))
}

/// Choose the project frame where a `--history` render starts processing, and
/// make the performance start cleanly there. The start is `history` seconds
/// before the output, rounded down to a multiple of the block size so blocks
/// fall on the same frames as a render from the song start; zero means full
/// history, identical to omitting the bound. Notes already sounding at that
/// frame are retriggered there with their remaining duration (no device can
/// resume an arbitrary voice mid-note) in source order; earlier notes are
/// dropped. Controllers keep their full history, and the transport restores
/// their latest values on the first block as in a live seek.
fn bound_history(
    s: &mut crate::Session,
    options: &RenderOptions,
    output_frame: u64,
    history: f64,
) -> Result<u64> {
    if !history.is_finite() || history < 0.0 {
        return Err(
            Diagnostic::new("history must be a nonnegative time in seconds")
                .help(
                    "pass --history 0 or more seconds, or omit it for history from the song start",
                )
                .err(),
        );
    }
    let rate = options.sample_rate as f64;
    let block = options.block_size.max(1) as u64;
    let timeline = crate::audio::transport::TempoTimeline::compile(rate, &s.transport, &s.tracks)
        .map_err(|e| Diagnostic::new(e).err())?;
    let wanted = output_frame.saturating_sub((history * rate).round() as u64);
    let mut origin = wanted / block * block;
    // A transport started at or after the musical end restarts from zero, so
    // an excerpt of the release tail starts its history inside the score.
    let end = timeline.end_project_frame().ceil() as u64;
    if origin >= end {
        origin = end.saturating_sub(1) / block * block;
    }
    if origin == 0 {
        return Ok(0);
    }
    let at = timeline.project_frame_to_tick(origin as f64);
    let retrigger = at.ceil() as u64;
    for t in &mut s.tracks {
        let TrackSource::Midi(m) = &mut t.source;
        m.imported.notes.retain_mut(|n| {
            if (n.start_tick as f64) >= at {
                return true;
            }
            let end = n.start_tick + n.duration_ticks;
            if end <= retrigger {
                return false;
            }
            n.start_tick = retrigger;
            n.duration_ticks = end - retrigger;
            true
        });
        // Retriggered notes now share one tick, where only source order
        // separates them, and may meet notes that already start there. Source
        // order need not follow performed order (a humanized or rolled chord
        // sounds in another order than it was written), so restore the
        // performed ordering the event list requires.
        m.imported
            .notes
            .sort_by_key(|n| (n.start_tick, n.source_order));
        m.imported.summary.notes = m.imported.notes.len() as u32;
        m.summary = m.imported.summary.clone();
    }
    Ok(origin)
}

/// All dry stems come from one engine pass, retaining shared sidechain/history behavior.
pub fn stems(s: crate::Session, out: &Path, options: &RenderOptions) -> Result<Vec<RenderReport>> {
    stems_of(s, out, options, &[])
}

/// Physical tracks named by `names`, in session order. A name selects the track
/// with that id or, like `--solo`, every expanded track `NAME.*` of a kit group.
/// No names select every track.
pub fn select_tracks(s: &crate::Session, names: &[String]) -> Result<Vec<usize>> {
    let ids: Vec<&str> = s.tracks.iter().map(|t| t.id.as_str()).collect();
    let matches = |id: &str, name: &str| {
        id == name
            || id
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with('.'))
    };
    for name in names {
        if !ids.iter().any(|id| matches(id, name)) {
            return Err(Diagnostic::new(format!("unknown stem track {name}"))
                .helps(suggest_vocabulary("tracks", name, ids.iter().copied()))
                .err());
        }
    }
    Ok((0..ids.len())
        .filter(|&i| names.is_empty() || names.iter().any(|name| matches(ids[i], name)))
        .collect())
}

/// Reduce the graph to what `taps` read (see
/// [`crate::fingerprint::dependencies`]): other tracks are removed, and the
/// inserts of buses no tap reads are cleared, with the automation lanes of
/// every removed device. Every tap then renders exactly as in the whole
/// graph, sample for sample, without the work nothing reads. Probe taps keep
/// their track. The render timeline compiles from every track's tempo map
/// and extent, so tracks are removed only when all of them share one (as a
/// compiled song's do); otherwise every track stays.
pub fn keep_tap_dependencies(s: &mut crate::Session, taps: &[String]) -> Result<()> {
    let needed = crate::fingerprint::dependencies(s, taps)?;
    if needed.tracks.is_empty() {
        bail!("a render needs at least one track");
    }
    fn timeline(t: &crate::model::Track) -> (&[crate::midi::MidiTempo], u64, u32, u64, u32) {
        let TrackSource::Midi(m) = &t.source;
        (&m.imported.tempos, m.imported.summary.end_tick, m.imported.summary.ppq, m.summary.end_tick, m.summary.ppq)
    }
    let shared = s.tracks.windows(2).all(|w| timeline(&w[0]) == timeline(&w[1]));
    if shared {
        s.tracks.retain(|t| needed.tracks.contains(t.id.as_str()));
    }
    for bus in std::iter::once(&mut s.master).chain(&mut s.buses) {
        if !needed.buses.contains(bus.id.as_str()) {
            bus.inserts.clear();
        }
    }
    // Mirror the engine's lane attachment: a route by id, a device by `id.` prefix.
    let mut routes = std::collections::BTreeSet::new();
    let mut devices = Vec::new();
    for t in &s.tracks {
        routes.extend(
            std::iter::once(&t.output)
                .chain(&t.sends)
                .map(|r| r.id.as_str()),
        );
        devices.extend(std::iter::once(&t.instrument).chain(&t.inserts).map(|d| format!("{}.", d.id)));
    }
    for b in std::iter::once(&s.master).chain(&s.buses) {
        routes.extend(b.output.iter().chain(&b.sends).map(|r| r.id.as_str()));
        devices.extend(b.inserts.iter().map(|d| format!("{}.", d.id)));
    }
    let routes: std::collections::BTreeSet<String> =
        routes.into_iter().map(str::to_owned).collect();
    s.extras.automation.retain(|lane| {
        routes.contains(&lane.target) || devices.iter().any(|d| lane.target.starts_with(d.as_str()))
    });
    Ok(())
}

/// Dry stems of the tracks selected by `names` (all tracks when empty) from one
/// pass. Tracks the selection does not read are not processed at all.
pub fn stems_of(
    mut s: crate::Session,
    out: &Path,
    options: &RenderOptions,
    names: &[String],
) -> Result<Vec<RenderReport>> {
    if !options.solo.is_empty() || !options.tap.is_empty() {
        bail!("stems chooses its own taps; use render for a selected tap or solo");
    }
    if !options.mute.is_empty() {
        bail!("dry stems are track taps, which --mute does not change; use --mute with --wet or render");
    }
    let selected = select_tracks(&s, names)?;
    let wanted: Vec<String> = selected
        .iter()
        .map(|&i| s.tracks[i].id.as_str().to_owned())
        .collect();
    if !names.is_empty() {
        keep_tap_dependencies(&mut s, &wanted)?;
    }
    std::fs::create_dir_all(out).map_err(|e| on_path(e, out))?;
    let source = s.extras.source.as_ref().map(|p| p.display().to_string());
    let (mut engine, first, duration) = prepare(s, options)?;
    let rate = options.sample_rate;
    let block = options.block_size;
    let frames = (duration * rate as f64).round() as u64;
    let written: Vec<usize> = (0..engine.track_count())
        .filter(|&i| wanted.iter().any(|id| id == engine.track_audio(i).0))
        .collect();
    let mut sinks = written
        .iter()
        .map(|&i| {
            let (id, _, _, lat) = engine.track_audio(i);
            SampleSink::new(
                out.join(format!("{id}.wav")),
                first + lat as u64,
                frames,
                rate,
                &options.format,
                STEM_DITHER_SEED,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let total = sinks.iter().map(|s| s.begin + frames).max().unwrap_or(0);
    let mut processed = 0;
    let mut buffer = vec![0.; block * 2];
    engine.set_running(true);
    while processed < total {
        crate::audio::clap::service_main_thread();
        if crate::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("stem export interrupted; existing outputs preserved");
        }
        let count = (total - processed).min(block as u64) as usize;
        engine.render_interleaved(&mut buffer[..count * 2], 2)?;
        for (&i, sink) in written.iter().zip(&mut sinks) {
            let (_, left, right, _) = engine.track_audio(i);
            for frame in 0..count {
                let at = processed + frame as u64;
                if at >= sink.begin && at < sink.begin + frames {
                    sink.write(left[frame])?;
                    sink.write(right[frame])?;
                }
            }
        }
        processed += count as u64;
    }
    let mut reports = sinks
        .into_iter()
        .map(SampleSink::finish)
        .collect::<Result<Vec<_>>>()?;
    for (&i, report) in written.iter().zip(&mut reports) {
        report.source = source.clone();
        report.scope = options.clone();
        report.scope.tap = vec![engine.track_audio(i).0.to_owned()];
    }
    std::fs::write(
        out.join("README.txt"),
        "Physical-track taps from one engine pass, after inserts and before output gain/sends/master. Shared returns can be exported with render --tap BUS.\n",
    )
    .map_err(|e| on_path(e, out))?;
    Ok(reports)
}
struct SampleSink {
    path: std::path::PathBuf,
    tmp: tempfile::NamedTempFile,
    writer: Option<crate::audio_file::AudioWriter>,
    begin: u64,
    frames: u64,
    rate: u32,
    bits: u16,
    float: bool,
    peak: f64,
    squares: f64,
    rng: u64,
}
impl SampleSink {
    fn new(
        path: std::path::PathBuf,
        begin: u64,
        frames: u64,
        rate: u32,
        format: &str,
        seed: u64,
    ) -> Result<Self> {
        let (bits, sample_format) = match format {
            "float32" => (32, hound::SampleFormat::Float),
            "pcm16" => (16, hound::SampleFormat::Int),
            "pcm24" => (24, hound::SampleFormat::Int),
            other => {
                return Err(Diagnostic::new("unknown WAV format")
                    .helps(suggest_vocabulary(
                        "formats",
                        other,
                        ["float32", "pcm16", "pcm24"],
                    ))
                    .err());
            }
        };
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let tmp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|e| on_path(e, &path))?;
        let file = tmp.reopen().map_err(|e| on_path(e, &path))?;
        let writer = crate::audio_file::AudioWriter::new(
            &path,
            file,
            crate::audio_file::WriteSpec {
                channels: 2,
                rate,
                bits,
                float: sample_format == hound::SampleFormat::Float,
                frames,
            },
        )
        .map_err(|e| on_path(e, &path))?;
        Ok(Self {
            path,
            tmp,
            writer: Some(writer),
            begin,
            frames,
            rate,
            bits,
            float: sample_format == hound::SampleFormat::Float,
            peak: 0.,
            squares: 0.,
            rng: seed,
        })
    }
    fn write(&mut self, x: f32) -> Result<()> {
        let path = &self.path;
        if !x.is_finite() {
            bail!("non-finite audio in {}", path.display());
        }
        self.peak = self.peak.max(x.abs() as f64);
        self.squares += (x as f64).powi(2);
        if self.float {
            self.writer
                .as_mut()
                .unwrap()
                .write_f32(x)
                .map_err(|e| on_path(e, path))?;
        } else {
            if x.abs() > 1. {
                return Err(Diagnostic::new(
                    "PCM stem would clip; use float32 or lower its source",
                )
                .help("use --format float32, or lower that source's level")
                .err());
            }
            let scale = (1u64 << (self.bits - 1)) as f64;
            let mut random = || {
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 7;
                self.rng ^= self.rng << 17;
                (self.rng >> 11) as f64 / (1u64 << 53) as f64
            };
            let dither = random() - random();
            self.writer
                .as_mut()
                .unwrap()
                .write_int(
                    (x as f64 * scale + dither)
                        .round()
                        .clamp(-scale, scale - 1.) as i32,
                )
                .map_err(|e| on_path(e, path))?;
        }
        Ok(())
    }
    fn finish(mut self) -> Result<RenderReport> {
        self.writer
            .take()
            .unwrap()
            .finish()
            .map_err(|e| on_path(e, &self.path))?;
        self.tmp
            .persist(&self.path)
            .map_err(|e| on_path(e, &self.path))?;
        Ok(RenderReport {
            source: None,
            revision: None,
            scope: RenderOptions::default(),
            track_meters: Vec::new(),
            track_seconds: Vec::new(),
            cpu_seconds: 0.0,
            latency_samples: 0,
            frames: self.frames,
            sample_rate: self.rate,
            seconds: self.frames as f64 / self.rate as f64,
            sample_peak_dbfs: 20. * self.peak.max(1e-20).log10(),
            rms_dbfs: 10.
                * (self.squares / (2 * self.frames).max(1) as f64)
                    .max(1e-40)
                    .log10(),
            output: self.path.display().to_string(),
            taps: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stem_selection_keeps_sidechain_sources_and_drops_bus_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.muz");
        std::fs::write(
            &path,
            r#"song({tracks:[
                track("other",phrase("C4:q"),synth("bell"),{sends:{hall:0}}),
                track("kick",phrase("C2:q"),synth("init")),
                track("bass",phrase("C2:q"),synth("init"),{chain:[fx("compressor",{sidechain:"kick"})]}),
                track("lead",phrase("C4:q"),synth("bell"),{sends:{hall:0}})
            ],buses:[bus("hall",[fx("compressor",{sidechain:"other"})])],
            automation:[automation("other.out",curve([[0b,0]],"step")),
                automation("hall.fx0.ratio",curve([[0b,2]],"step")),
                automation("lead.out",curve([[0b,0]],"step"))]})"#,
        )
        .unwrap();
        let mut s = crate::compile::compile(&path).unwrap().session;
        super::keep_tap_dependencies(&mut s, &["bass".into(), "lead".into()]).unwrap();
        let ids: Vec<_> = s.tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["kick", "bass", "lead"]);
        assert!(s.buses.iter().all(|b| b.inserts.is_empty()) && s.master.inserts.is_empty());
        let lanes: Vec<_> = s
            .extras
            .automation
            .iter()
            .map(|a| a.target.as_str())
            .collect();
        assert_eq!(lanes, ["lead.out"]);
        crate::audio::AudioEngine::new(
            &s,
            crate::audio::AudioConfig {
                sample_rate: 48000.,
                max_frames: 256,
                offline: true,
            },
        )
        .unwrap();
    }
}
