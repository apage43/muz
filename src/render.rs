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
    pub frames: u64,
    pub sample_rate: u32,
    pub seconds: f64,
    pub sample_peak_dbfs: f64,
    pub rms_dbfs: f64,
    pub output: String,
}
#[derive(Debug, Serialize, serde::Deserialize)]
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
    render_session(c.session, out, seconds, section, solo, rate, block, None)
}
pub fn render_session(
    s: crate::Session,
    out: &Path,
    seconds: Option<f64>,
    section: Option<&str>,
    solo: &[String],
    rate: u32,
    block: usize,
    job: Option<&crate::control::RenderProgress>,
) -> Result<RenderReport> {
    render_with(
        s,
        out,
        &RenderOptions {
            seconds,
            section: section.map(str::to_owned),
            solo: solo.to_vec(),
            sample_rate: rate,
            block_size: block,
            ..Default::default()
        },
        job,
    )
}
#[derive(Clone, Debug, clap::Args, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RenderOptions {
    #[arg(long)]
    pub seconds: Option<f64>,
    #[arg(long)]
    pub section: Option<String>,
    #[arg(long)]
    pub solo: Vec<String>,
    #[arg(long, default_value_t = 48000)]
    pub sample_rate: u32,
    #[arg(long, default_value_t = 256)]
    pub block_size: usize,
    #[arg(long)]
    pub start: Option<f64>,
    #[arg(long)]
    pub tail: Option<f64>,
    #[arg(long)]
    pub tap: Option<String>,
    #[arg(long, default_value = "float32")]
    pub format: String,
}
impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            seconds: None,
            section: None,
            solo: Vec::new(),
            sample_rate: 48000,
            block_size: 256,
            start: None,
            tail: None,
            tap: None,
            format: "float32".into(),
        }
    }
}
pub fn render_with(
    s: crate::Session,
    out: &Path,
    options: &RenderOptions,
    job: Option<&crate::control::RenderProgress>,
) -> Result<RenderReport> {
    let rate = options.sample_rate;
    let block = options.block_size;
    let source = s.extras.source.as_ref().map(|p| p.display().to_string());
    let (mut engine, start, duration) = prepare(s, options)?;
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
                .err())
        }
    };
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format,
    };
    let file = tmp.reopen().map_err(|e| on_path(e, out))?;
    let mut writer =
        hound::WavWriter::new(std::io::BufWriter::with_capacity(128 * 1024, file), spec)
            .map_err(|e| on_path(e, out))?;
    let begin = (start * rate as f64).round() as u64 + engine.latency_samples() as u64;
    let frames = (duration * rate as f64).round() as u64;
    let total = begin + frames;
    let mut processed = 0u64;
    let mut last_progress = 0;
    let mut buf = vec![0.0f32; block * 2];
    let (mut peak, mut square) = (0.0f64, 0.0f64);
    let mut meter = vec![(0.0f64, 0.0f64, 0u64); engine.track_count()];
    let mut rng = 0x5ad9312u64;
    engine.set_running(true);
    while processed < total {
        crate::audio::clap::service_main_thread();
        if crate::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("render interrupted; output preserved");
        }
        let count = (total - processed).min(block as u64) as usize;
        engine.render_interleaved(&mut buf[..count * 2], 2)?;
        for (index, values) in meter.iter_mut().enumerate() {
            let (_, l, r, lat) = engine.track_audio(index);
            let begin = (start * rate as f64).round() as u64 + lat as u64;
            for i in 0..count {
                let frame = processed + i as u64;
                if frame >= begin && frame < begin + frames {
                    values.0 = values.0.max(l[i].abs() as f64).max(r[i].abs() as f64);
                    values.1 += (l[i] as f64).powi(2) + (r[i] as f64).powi(2);
                    values.2 += 2;
                }
            }
        }
        for (i, pair) in buf[..count * 2].chunks_exact(2).enumerate() {
            if processed + i as u64 >= begin {
                for &x in pair {
                    if !x.is_finite() {
                        bail!("non-finite audio; output preserved");
                    }
                    peak = peak.max(x.abs() as f64);
                    square += (x as f64).powi(2);
                    if sample_format == hound::SampleFormat::Float {
                        writer.write_sample(x).map_err(|e| on_path(e, out))?;
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
                            .write_sample(
                                (x as f64 * scale + dither)
                                    .round()
                                    .clamp(-scale, scale - 1.0) as i32,
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
    writer.finalize().map_err(|e| on_path(e, out))?;
    tmp.persist(out).map_err(|e| on_path(e, out))?;
    Ok(RenderReport {
        source,
        revision: None,
        scope: options.clone(),
        track_meters: meter
            .into_iter()
            .enumerate()
            .map(|(i, (peak, square, n))| TrackMeter {
                track: engine.track_audio(i).0.to_owned(),
                peak_dbfs: 20. * peak.max(1e-20).log10(),
                rms_dbfs: 10. * (square / n.max(1) as f64).max(1e-40).log10(),
            })
            .collect(),
        frames,
        sample_rate: rate,
        seconds: frames as f64 / rate as f64,
        sample_peak_dbfs: 20.0 * peak.max(1e-20).log10(),
        rms_dbfs: 10.0 * (square / (frames * 2) as f64).max(1e-40).log10(),
        output: out.display().to_string(),
    })
}

fn prepare(mut s: crate::Session, options: &RenderOptions) -> Result<(AudioEngine, f64, f64)> {
    let rate = options.sample_rate;
    let block = options.block_size;
    let seconds = options.seconds;
    let section = options.section.as_deref();
    let tail = options.tail.unwrap_or(s.extras.tail);
    if !tail.is_finite() || tail < 0.0 || tail > 600.0 {
        return Err(Diagnostic::new("tail must be 0..600 seconds")
            .help("pass --tail with a value in 0..600 seconds")
            .err());
    }
    let mut start = 0.0;
    let end = if let Some(section) = section {
        let names: Vec<&str> = s.extras.sections.iter().map(|sec| sec.name.as_str()).collect();
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
                        .helps(suggest_vocabulary("sections", section, names.iter().copied()))
                        .err()
                }
            })?;
        let tempos = match &s.tracks[0].source {
            TrackSource::Midi(src) => &src.imported.tempos,
            _ => unreachable!(),
        };
        start = compile::seconds_at(sec.start, tempos);
        compile::seconds_at(sec.end, tempos) + tail
    } else {
        let src = match &s.tracks[0].source {
            TrackSource::Midi(src) => src,
            _ => unreachable!(),
        };
        compile::seconds_at(
            src.summary.end_tick as f64 / src.summary.ppq as f64,
            &src.imported.tempos,
        ) + tail
    };
    if let Some(at) = options.start {
        if !at.is_finite() || at < 0.0 {
            return Err(Diagnostic::new("start must be a nonnegative time in seconds")
                .help("pass --start 0 or a positive number of seconds")
                .err());
        }
        start = at;
    }
    let duration = seconds.unwrap_or(end - start);
    if !duration.is_finite() || duration <= 0.0 || duration > 36000.0 {
        return Err(Diagnostic::new("duration must be in 0..36000 seconds")
            .help("pass --seconds or --start for a positive duration under 36000 seconds")
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
            if let TrackSource::Midi(m) = &mut t.source {
                m.imported.notes.retain(|n| n.start_tick < cut);
                for n in &mut m.imported.notes {
                    n.duration_ticks = n.duration_ticks.min(cut - n.start_tick);
                }
                m.imported.controllers.retain(|c| c.tick < cut);
                m.imported.messages.retain(|c| c.tick < cut);
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
            }
        }
    }
    let mut engine = AudioEngine::new(
        &s,
        AudioConfig {
            sample_rate: rate as f32,
            max_frames: block,
            offline: true,
        },
    )?;
    engine.set_solo(&options.solo)?;
    engine.set_tap(options.tap.as_deref())?;
    Ok((engine, start, duration))
}

/// All dry stems come from one engine pass, retaining shared sidechain/history behavior.
pub fn stems(s: crate::Session, out: &Path, options: &RenderOptions) -> Result<Vec<RenderReport>> {
    if !options.solo.is_empty() || options.tap.is_some() {
        bail!("stems chooses its own taps; use render for a selected tap or solo");
    }
    std::fs::create_dir_all(out).map_err(|e| on_path(e, out))?;
    let source = s.extras.source.as_ref().map(|p| p.display().to_string());
    let (mut engine, start, duration) = prepare(s, options)?;
    let rate = options.sample_rate;
    let block = options.block_size;
    let frames = (duration * rate as f64).round() as u64;
    let mut sinks = (0..engine.track_count())
        .map(|i| {
            let (id, _, _, lat) = engine.track_audio(i);
            SampleSink::new(
                out.join(format!("{id}.wav")),
                (start * rate as f64).round() as u64 + lat as u64,
                frames,
                rate,
                &options.format,
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
        for (i, sink) in sinks.iter_mut().enumerate() {
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
    for (i, report) in reports.iter_mut().enumerate() {
        report.source = source.clone();
        report.scope = options.clone();
        report.scope.tap = Some(engine.track_audio(i).0.to_owned());
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
    writer: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
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
                    .err())
            }
        };
        let tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())
            .map_err(|e| on_path(e, &path))?;
        let file = tmp.reopen().map_err(|e| on_path(e, &path))?;
        let writer = hound::WavWriter::new(
            std::io::BufWriter::with_capacity(128 * 1024, file),
            hound::WavSpec {
                channels: 2,
                sample_rate: rate,
                bits_per_sample: bits,
                sample_format,
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
            rng: 0xa52983,
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
                .write_sample(x)
                .map_err(|e| on_path(e, path))?;
        } else {
            if x.abs() > 1. {
                return Err(Diagnostic::new("PCM stem would clip; use float32 or lower its source")
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
                .write_sample(
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
            .finalize()
            .map_err(|e| on_path(e, &self.path))?;
        self.tmp
            .persist(&self.path)
            .map_err(|e| on_path(e, &self.path))?;
        Ok(RenderReport {
            source: None,
            revision: None,
            scope: RenderOptions::default(),
            track_meters: Vec::new(),
            frames: self.frames,
            sample_rate: self.rate,
            seconds: self.frames as f64 / self.rate as f64,
            sample_peak_dbfs: 20. * self.peak.max(1e-20).log10(),
            rms_dbfs: 10.
                * (self.squares / (2 * self.frames).max(1) as f64)
                    .max(1e-40)
                    .log10(),
            output: self.path.display().to_string(),
        })
    }
}
