use crate::{
    audio::{AudioConfig, AudioEngine},
    compile,
    model::TrackSource,
};
use anyhow::{Result, bail};
use serde::Serialize;
use std::path::Path;
#[derive(Debug, Serialize)]
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
    let mut s = c.session;
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
        compile::seconds_at(sec.end, tempos) + s.extras.tail
    } else {
        let src = match &s.tracks[0].source {
            TrackSource::Midi(src) => src,
            _ => unreachable!(),
        };
        compile::seconds_at(
            src.summary.end_tick as f64 / src.summary.ppq as f64,
            &src.imported.tempos,
        ) + s.extras.tail
    };
    if !solo.is_empty() {
        for id in solo {
            if !s
                .tracks
                .iter()
                .any(|t| t.id.as_str() == id || t.id.as_str().starts_with(&format!("{id}.")))
            {
                bail!("unknown solo track '{id}'");
            }
        }
        for t in &mut s.tracks {
            if !solo
                .iter()
                .any(|id| t.id.as_str() == id || t.id.as_str().starts_with(&format!("{id}.")))
            {
                t.output.gain_db = -120.0;
                for r in &mut t.sends {
                    r.gain_db = -120.0;
                }
            }
        }
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
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = tempfile::NamedTempFile::new_in(parent)?;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::new(tmp.as_file(), spec)?;
    let begin = (start * rate as f64).round() as u64;
    let frames = (duration * rate as f64).round() as u64;
    let total = begin + frames;
    let mut processed = 0u64;
    let mut buf = vec![0.0f32; block * 2];
    let (mut peak, mut square) = (0.0f64, 0.0f64);
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
                    writer.write_sample(x)?;
                }
            }
        }
        processed += count as u64;
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
