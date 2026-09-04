use crate::{
    audio::{AudioConfig, AudioEngine},
    compile,
    model::TrackSource,
};
use anyhow::{Result, bail};
use serde::Serialize;
use std::path::Path;
#[derive(Debug, Serialize, serde::Deserialize)]
pub struct RenderReport {
    pub frames: u64,
    pub sample_rate: u32,
    pub seconds: f64,
    pub sample_peak_dbfs: f64,
    pub rms_dbfs: f64,
    pub output: String,
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
    let seconds = options.seconds;
    let section = options.section.as_deref();
    let tail = options.tail.unwrap_or(s.extras.tail);
    if !tail.is_finite() || tail < 0.0 || tail > 600.0 {
        bail!("tail must be 0..600 seconds");
    }
    let mut start = 0.0;
    let end = if let Some(section) = section {
        let sec = s
            .extras
            .sections
            .iter()
            .find(|s| s.name == section)
            .ok_or_else(|| anyhow::anyhow!("unknown section {section}"))?;
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
            bail!("start must be a nonnegative time in seconds");
        }
        start = at;
    }
    let duration = seconds.unwrap_or(end - start);
    if !duration.is_finite() || duration <= 0.0 || duration > 36000.0 {
        bail!("duration must be in 0..36000 seconds");
    }
    if !(8000..=192000).contains(&rate) {
        bail!("sample rate must be 8000..192000");
    }
    let mut engine = AudioEngine::new(
        &s,
        AudioConfig {
            sample_rate: rate as f32,
            max_frames: block,
        },
    )?;
    engine.set_solo(&options.solo)?;
    engine.set_tap(options.tap.as_deref())?;
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = tempfile::NamedTempFile::new_in(parent)?;
    let (bits, sample_format) = match options.format.as_str() {
        "float32" => (32, hound::SampleFormat::Float),
        "pcm16" => (16, hound::SampleFormat::Int),
        "pcm24" => (24, hound::SampleFormat::Int),
        _ => bail!("format must be float32, pcm16 or pcm24"),
    };
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format,
    };
    let mut writer = hound::WavWriter::new(tmp.as_file(), spec)?;
    let begin = (start * rate as f64).round() as u64 + engine.latency_samples() as u64;
    let frames = (duration * rate as f64).round() as u64;
    let total = begin + frames;
    let mut processed = 0u64;
    let mut last_progress = 0;
    let mut buf = vec![0.0f32; block * 2];
    let (mut peak, mut square) = (0.0f64, 0.0f64);
    let mut rng = 0x5ad9312u64;
    engine.set_running(true);
    while processed < total {
        let count = (total - processed).min(block as u64) as usize;
        engine.render_interleaved(&mut buf[..count * 2], 2)?;
        for (i, pair) in buf[..count * 2].chunks_exact(2).enumerate() {
            if processed + i as u64 >= begin {
                for &x in pair {
                    if !x.is_finite() {
                        bail!("non-finite audio; output preserved");
                    }
                    peak = peak.max(x.abs() as f64);
                    square += (x as f64).powi(2);
                    if sample_format == hound::SampleFormat::Float {
                        writer.write_sample(x)?;
                    } else {
                        if x.abs() > 1.0 {
                            bail!("PCM export would clip; lower the master or use float32");
                        }
                        let scale = (1u64 << (bits - 1)) as f64;
                        let mut uniform = || {
                            rng ^= rng << 13;
                            rng ^= rng >> 7;
                            rng ^= rng << 17;
                            (rng >> 11) as f64 / (1u64 << 53) as f64
                        };
                        let dither = uniform() - uniform();
                        writer.write_sample(
                            (x as f64 * scale + dither)
                                .round()
                                .clamp(-scale, scale - 1.0) as i32,
                        )?;
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
    writer.finalize()?;
    tmp.persist(out)?;
    Ok(RenderReport {
        frames,
        sample_rate: rate,
        seconds: frames as f64 / rate as f64,
        sample_peak_dbfs: 20.0 * peak.max(1e-20).log10(),
        rms_dbfs: 10.0 * (square / (frames * 2) as f64).max(1e-40).log10(),
        output: out.display().to_string(),
    })
}
