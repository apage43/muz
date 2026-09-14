//! Native asset decoding, exclusively on the coordinator/worker.
use anyhow::{Context, Result, ensure};
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
type CacheKey = (usize, std::path::PathBuf, (u64, u128));
type CacheEntry = (
    Info,
    Weak<[[f32; 2]]>,
    Arc<dyn crate::assets::AssetResolver>,
);
/// Bounded weak cache: prepared processors own recordings, never playback state.
/// Resolver identity and version isolate hosts. Decoding has one fixed setting:
/// native-rate normalized floating-point stereo frames.
#[derive(Default)]
pub struct DecodedCache {
    entries: Mutex<std::collections::BTreeMap<CacheKey, CacheEntry>>,
}
#[derive(Clone, Debug)]
pub struct Info {
    pub frames: u64,
    pub rate: u32,
    pub channels: u16,
}
pub fn info(path: &Path) -> Result<Info> {
    let asset = crate::assets::snapshot(path, 512 * 1024 * 1024)?;
    info_bytes(path, &asset.bytes)
}
fn info_bytes(path: &Path, bytes: &std::sync::Arc<[u8]>) -> Result<Info> {
    crate::host::check_cancelled()?;
    ensure!(
        path.extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("wav") || x.eq_ignore_ascii_case("flac")),
        "unsupported audio asset: {}",
        path.display()
    );
    let i = if path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("flac"))
    {
        let r = claxon::FlacReader::new(std::io::Cursor::new(bytes.clone()))?;
        let s = r.streaminfo();
        Info {
            frames: s.samples.unwrap_or(0),
            rate: s.sample_rate,
            channels: s.channels as u16,
        }
    } else {
        let r = hound::WavReader::new(std::io::Cursor::new(bytes.clone()))?;
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
    let (info, frames) = load_shared(path, max_frames)?;
    Ok((info, frames.to_vec()))
}
pub fn load_shared(path: &Path, max_frames: usize) -> Result<(Info, Arc<[[f32; 2]]>)> {
    let resolver = crate::assets::resolver();
    let path = resolver.resolve(path)?;
    let asset = crate::assets::snapshot(&path, 512 * 1024 * 1024)?;
    let key = (
        Arc::as_ptr(&resolver) as *const () as usize,
        path.clone(),
        asset.version,
    );
    let cache = crate::host::current().map(|context| context.decoded_assets);
    if let Some(cache) = &cache {
        if let Some((info, frames)) = cache
            .entries
            .lock()
            .unwrap()
            .get(&key)
            .and_then(|(info, frames, _)| frames.upgrade().map(|frames| (info.clone(), frames)))
        {
            ensure!(
                frames.len() <= max_frames,
                "sample allocation limit exceeded"
            );
            return Ok((info, frames));
        }
    }
    let (info, frames) = decode(&path, max_frames, asset)?;
    let frames: Arc<[[f32; 2]]> = frames.into();
    if let Some(cache) = cache {
        let mut entries = cache.entries.lock().unwrap();
        entries.retain(|_, (_, frames, _)| frames.strong_count() > 0);
        if entries.len() >= 128 {
            entries.pop_first();
        }
        entries.insert(key, (info.clone(), Arc::downgrade(&frames), resolver));
    }
    Ok((info, frames))
}
fn decode(
    path: &Path,
    max_frames: usize,
    asset: crate::assets::AssetSnapshot,
) -> Result<(Info, Vec<[f32; 2]>)> {
    let bytes = &asset.bytes;
    let i = info_bytes(path, bytes).with_context(|| path.display().to_string())?;
    ensure!(
        i.frames <= max_frames as u64,
        "sample allocation limit exceeded"
    );
    let max = max_frames.saturating_mul(i.channels as usize);
    let raw = if path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("flac"))
    {
        let mut r = claxon::FlacReader::new(std::io::Cursor::new(bytes.clone()))?;
        let scale = (1u64 << (r.streaminfo().bits_per_sample - 1)) as f32;
        r.samples()
            .take(max.saturating_add(1))
            .enumerate()
            .map(|(index, x)| -> Result<f32> {
                if index % 4096 == 0 {
                    crate::host::check_cancelled()?;
                }
                Ok(x? as f32 / scale)
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let r = hound::WavReader::new(std::io::Cursor::new(bytes.clone()))?;
        let spec = r.spec();
        match spec.sample_format {
            hound::SampleFormat::Float => r
                .into_samples::<f32>()
                .take(max.saturating_add(1))
                .enumerate()
                .map(|(index, x)| -> Result<f32> {
                    if index % 4096 == 0 {
                        crate::host::check_cancelled()?;
                    }
                    Ok(x?)
                })
                .collect::<Result<Vec<_>, _>>()?,
            hound::SampleFormat::Int => {
                let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
                r.into_samples::<i32>()
                    .take(max.saturating_add(1))
                    .enumerate()
                    .map(|(index, x)| -> Result<f32> {
                        if index % 4096 == 0 {
                            crate::host::check_cancelled()?;
                        }
                        Ok(x? as f32 / scale)
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        }
    };
    ensure!(
        raw.len() <= max && !raw.is_empty() && raw.iter().all(|v| v.is_finite()),
        "invalid/oversized sample data"
    );
    crate::host::check_cancelled()?;
    let frames = raw
        .chunks_exact(i.channels as usize)
        .map(|x| [x[0], *x.get(1).unwrap_or(&x[0])])
        .collect();
    Ok((i, frames))
}
