//! Native asset decoding, exclusively on the coordinator/worker.
use anyhow::{Context, Result, ensure};
use std::path::Path;
pub struct Info {
    pub frames: u64,
    pub rate: u32,
    pub channels: u16,
}
pub fn info(path: &Path) -> Result<Info> {
    let i = if path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("flac"))
    {
        let r = claxon::FlacReader::open(path)?;
        let s = r.streaminfo();
        Info {
            frames: s.samples.unwrap_or(0),
            rate: s.sample_rate,
            channels: s.channels as u16,
        }
    } else {
        let r = hound::WavReader::open(path)?;
        Info {
            frames: r.duration() as u64,
            rate: r.spec().sample_rate,
            channels: r.spec().channels,
        }
    };
    ensure!(
        matches!(i.channels, 1 | 2) && i.rate > 0,
        "assets must be mono/stereo WAV or FLAC"
    );
    Ok(i)
}
pub fn load(path: &Path, max_frames: usize) -> Result<(Info, Vec<[f32; 2]>)> {
    let i = info(path).with_context(|| path.display().to_string())?;
    ensure!(
        i.frames <= max_frames as u64,
        "sample allocation limit exceeded"
    );
    let max = max_frames.saturating_mul(i.channels as usize);
    let raw = if path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("flac"))
    {
        let mut r = claxon::FlacReader::open(path)?;
        let scale = (1u64 << (r.streaminfo().bits_per_sample - 1)) as f32;
        r.samples()
            .take(max + 1)
            .map(|x| x.map(|x| x as f32 / scale))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let r = hound::WavReader::open(path)?;
        let spec = r.spec();
        match spec.sample_format {
            hound::SampleFormat::Float => r
                .into_samples::<f32>()
                .take(max + 1)
                .collect::<Result<Vec<_>, _>>()?,
            hound::SampleFormat::Int => {
                let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
                r.into_samples::<i32>()
                    .take(max + 1)
                    .map(|x| x.map(|x| x as f32 / scale))
                    .collect::<Result<Vec<_>, _>>()?
            }
        }
    };
    ensure!(
        raw.len() <= max && !raw.is_empty() && raw.iter().all(|v| v.is_finite()),
        "invalid/oversized sample data"
    );
    let frames = raw
        .chunks_exact(i.channels as usize)
        .map(|x| [x[0], *x.get(1).unwrap_or(&x[0])])
        .collect();
    Ok((i, frames))
}
