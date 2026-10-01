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
    pub compatibility: Vec<WavCompatibility>,
    pub frames: u64,
    pub rate: u32,
    pub channels: u16,
}
/// Read-only compatibility interpretations, preserving the original asset bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WavCompatibility {
    /// PCM fmt extension/padding is ignored after validating the base format.
    PcmFmtExtension,
    /// A mono 24-bit PCM data chunk included its final zero alignment pad.
    IncludedDataPad,
}
struct WavLayout {
    info: Info,
    bits: u16,
    data: std::ops::Range<usize>,
}
fn word(bytes: &[u8], at: usize) -> u16 { u16::from_le_bytes([bytes[at],bytes[at+1]]) }
fn dword(bytes: &[u8], at: usize) -> u32 { u32::from_le_bytes(bytes[at..at+4].try_into().unwrap()) }
fn pcm_wav_layout(bytes: &[u8]) -> Result<Option<WavLayout>> {
    ensure!(bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE", "invalid WAV RIFF header");
    let end = (dword(bytes,4) as usize).checked_add(8).context("WAV RIFF length overflow")?;
    ensure!(end >= 12 && end <= bytes.len(), "truncated WAV RIFF payload");
    let mut at = 12usize;
    let mut format = None;
    let mut data = None;
    while at < end {
        crate::host::check_cancelled()?;
        ensure!(end-at >= 8, "truncated WAV chunk header");
        let len = dword(bytes,at+4) as usize;
        let start = at+8;
        let chunk_end = start.checked_add(len).context("WAV chunk length overflow")?;
        ensure!(chunk_end <= end, "truncated WAV chunk payload");
        match &bytes[at..at+4] {
            b"fmt " => { ensure!(format.is_none(), "duplicate WAV format chunk"); format = Some(start..chunk_end); }
            b"data" => { ensure!(data.is_none(), "duplicate WAV data chunk"); data = Some(start..chunk_end); }
            _ => {}
        }
        at = chunk_end.checked_add(len&1).context("WAV padding overflow")?;
        ensure!(at <= end, "truncated WAV alignment padding");
    }
    let format = format.context("missing WAV format chunk")?;
    let mut data = data.context("missing WAV data chunk")?;
    ensure!(format.len() >= 16 && format.len() <= 65536, "invalid WAV format length");
    let f = &bytes[format.clone()];
    if word(f,0) != 1 { return Ok(None); }
    let channels = word(f,2);
    let rate = dword(f,4);
    let align = word(f,12) as usize;
    let bits = word(f,14);
    ensure!(matches!(channels,1|2) && rate > 0 && matches!(bits,8|16|24|32), "unsupported PCM WAV format");
    ensure!(align == channels as usize * (bits as usize/8) && dword(f,8) as u64 == rate as u64 * align as u64, "inconsistent PCM WAV frame layout");
    let mut compatibility = Vec::new();
    if format.len() > 16 {
        ensure!(format.len() >= 18, "truncated PCM WAV extension");
        let declared = word(f,16) as usize;
        ensure!(declared <= format.len()-18, "truncated PCM WAV extension payload");
        ensure!(f[18+declared..].iter().all(|byte| *byte == 0), "nonzero undeclared PCM WAV extension padding");
        if format.len() != 18 { compatibility.push(WavCompatibility::PcmFmtExtension); }
    }
    if data.len()%align != 0 {
        // Verified upstream quirk: the RIFF data size counts the single zero pad
        // following an odd-length, complete mono 24-bit PCM payload.
        ensure!(channels == 1 && bits == 24 && data.len()%3 == 1 && (data.len()-1)%2 == 1 && bytes[data.end-1] == 0,
            "PCM WAV data is not a complete set of frames");
        data.end -= 1;
        compatibility.push(WavCompatibility::IncludedDataPad);
    }
    Ok(Some(WavLayout { info:Info {compatibility,frames:(data.len()/align) as u64,rate,channels},bits,data }))
}
fn decode_pcm(bytes: &[u8], layout: &WavLayout) -> Result<Vec<f32>> {
    let width = layout.bits as usize/8;
    bytes[layout.data.clone()].chunks_exact(width).enumerate().map(|(index,x)| {
        if index%4096 == 0 { crate::host::check_cancelled()?; }
        Ok(match width {
            1 => (x[0] as f32-128.)/128.,
            2 => i16::from_le_bytes(x.try_into().unwrap()) as f32/32768.,
            3 => { let value=(x[0] as i32)|((x[1] as i32)<<8)|((x[2] as i32)<<16); ((value<<8)>>8) as f32/8388608. },
            4 => i32::from_le_bytes(x.try_into().unwrap()) as f32/2147483648.,
            _ => unreachable!(),
        })
    }).collect()
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
            compatibility: Vec::new(),
            frames: s.samples.unwrap_or(0),
            rate: s.sample_rate,
            channels: s.channels as u16,
        }
    } else {
        let layout = pcm_wav_layout(bytes)?;
        if let Some(layout) = &layout && !layout.info.compatibility.is_empty() {
            return Ok(layout.info.clone());
        }
        let r = hound::WavReader::new(std::io::Cursor::new(bytes.clone()))?;
        Info {
            compatibility: Vec::new(),
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
    if let Some(cache) = &cache
        && let Some((info, frames)) = cache
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
    } else if let Some(layout) = pcm_wav_layout(bytes)? && !layout.info.compatibility.is_empty() {
        decode_pcm(bytes,&layout)?
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

#[cfg(test)]
mod wav_compatibility_tests {
    use super::*;
    fn wav(bits:u16, extension:&[u8], payload:&[u8], declared:u32, suffix:bool)->Vec<u8> {
        let mut body=Vec::new();
        body.extend_from_slice(b"WAVEfmt ");body.extend_from_slice(&(16u32+extension.len() as u32).to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&48000u32.to_le_bytes());body.extend_from_slice(&(48000u32*(bits as u32/8)).to_le_bytes());
        body.extend_from_slice(&(bits/8).to_le_bytes());body.extend_from_slice(&bits.to_le_bytes());body.extend_from_slice(extension);
        body.extend_from_slice(b"data");body.extend_from_slice(&declared.to_le_bytes());body.extend_from_slice(payload);
        if payload.len()%2 == 1 {body.push(0);}
        if suffix {body.extend_from_slice(b"smpl");body.extend_from_slice(&4u32.to_le_bytes());body.extend_from_slice(&[0;4]);}
        let mut bytes=Vec::from(&b"RIFF"[..]);bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());bytes.extend_from_slice(&body);bytes
    }
    fn snapshot(bytes:Vec<u8>)->crate::assets::AssetSnapshot {
        crate::assets::AssetSnapshot {version:(bytes.len() as u64,0),bytes:bytes.into()}
    }
    #[test]
    fn pcm_fmt20_decodes_without_rewriting_original_bytes() {
        let original=wav(16,&[0,0,0,0],&[0,64,0,192],4,true);
        let asset=snapshot(original.clone());
        let info=info_bytes(Path::new("fixture.wav"),&asset.bytes).unwrap();
        assert_eq!(info.compatibility,vec![WavCompatibility::PcmFmtExtension]);
        assert_eq!(info.frames,2);
        let (decoded,frames)=decode(Path::new("fixture.wav"),2,asset).unwrap();
        assert_eq!(decoded.frames,info.frames);assert_eq!(frames,vec![[0.5;2],[-0.5;2]]);
        assert_eq!(original,wav(16,&[0,0,0,0],&[0,64,0,192],4,true));
    }
    #[test]
    fn included_mono24_zero_pad_is_not_decoded_as_a_partial_sample() {
        let canonical = snapshot(wav(24,&[0,0],&[0,0,64],3,true));
        let canonical_info = info_bytes(Path::new("canonical.wav"),&canonical.bytes).unwrap();
        assert!(canonical_info.compatibility.is_empty());
        assert_eq!(decode(Path::new("canonical.wav"),1,canonical).unwrap().1,vec![[0.5;2]]);
        // One signed24 frame is odd-length; upstream declareddata erroneously
        // includes itszero alignmentpad, followed by anotherRIFFchunk.
        let asset=snapshot(wav(24,&[0,0],&[0,0,64,0],4,true));
        let info=info_bytes(Path::new("fixture.wav"),&asset.bytes).unwrap();
        assert_eq!(info.compatibility,vec![WavCompatibility::IncludedDataPad]);assert_eq!(info.frames,1);
        let (decoded,frames)=decode(Path::new("fixture.wav"),1,asset).unwrap();
        assert_eq!(decoded.frames,1);assert_eq!(frames,vec![[0.5;2]]);
    }
    #[test]
    fn real_partial_frames_bad_extensions_and_truncation_reject() {
        for bytes in [wav(24,&[0,0],&[0,0,64,1],4,true),wav(16,&[0,0],&[0,64,1],3,true),wav(16,&[0,0,1,0],&[0,64],2,true)] {
            assert!(info_bytes(Path::new("bad.wav"),&snapshot(bytes).bytes).is_err());
        }
        let mut bytes=wav(16,&[0,0,0,0],&[0,64],2,true);bytes.pop();
        assert!(info_bytes(Path::new("bad.wav"),&snapshot(bytes).bytes).is_err());
    }
    #[test]
    fn compatibility_preserves_limits_and_cancellation() {
        let bytes=wav(16,&[0,0,0,0],&[0,64,0,192],4,true);
        assert!(decode(Path::new("fixture.wav"),1,snapshot(bytes.clone())).is_err());
        let context=crate::host::HostContext::default();
        context.cancelled.store(true,std::sync::atomic::Ordering::Relaxed);
        assert!(context.run(|| info_bytes(Path::new("fixture.wav"),&snapshot(bytes).bytes)).is_err());
    }
}
