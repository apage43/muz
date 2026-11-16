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
/// Weak cache: prepared processors own recordings, never playback state; an
/// entry lives exactly as long as some processor holds its recording.
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
fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}
fn dword(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn pcm_wav_layout(bytes: &[u8]) -> Result<Option<WavLayout>> {
    ensure!(
        bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "invalid WAV RIFF header"
    );
    let end = (dword(bytes, 4) as usize)
        .checked_add(8)
        .context("WAV RIFF length overflow")?;
    ensure!(
        end >= 12 && end <= bytes.len(),
        "truncated WAV RIFF payload"
    );
    let mut at = 12usize;
    let mut format = None;
    let mut data = None;
    while at < end {
        crate::host::check_cancelled()?;
        ensure!(end - at >= 8, "truncated WAV chunk header");
        let len = dword(bytes, at + 4) as usize;
        let start = at + 8;
        let chunk_end = start
            .checked_add(len)
            .context("WAV chunk length overflow")?;
        ensure!(chunk_end <= end, "truncated WAV chunk payload");
        match &bytes[at..at + 4] {
            b"fmt " => {
                ensure!(format.is_none(), "duplicate WAV format chunk");
                format = Some(start..chunk_end);
            }
            b"data" => {
                ensure!(data.is_none(), "duplicate WAV data chunk");
                data = Some(start..chunk_end);
            }
            _ => {}
        }
        at = chunk_end
            .checked_add(len & 1)
            .context("WAV padding overflow")?;
        ensure!(at <= end, "truncated WAV alignment padding");
    }
    let format = format.context("missing WAV format chunk")?;
    let mut data = data.context("missing WAV data chunk")?;
    ensure!(
        format.len() >= 16 && format.len() <= 65536,
        "invalid WAV format length"
    );
    let f = &bytes[format.clone()];
    if word(f, 0) != 1 {
        return Ok(None);
    }
    let channels = word(f, 2);
    let rate = dword(f, 4);
    let align = word(f, 12) as usize;
    let bits = word(f, 14);
    ensure!(
        matches!(channels, 1 | 2) && rate > 0 && matches!(bits, 8 | 16 | 24 | 32),
        "unsupported PCM WAV format"
    );
    ensure!(
        align == channels as usize * (bits as usize / 8)
            && dword(f, 8) as u64 == rate as u64 * align as u64,
        "inconsistent PCM WAV frame layout"
    );
    let mut compatibility = Vec::new();
    if format.len() > 16 {
        ensure!(format.len() >= 18, "truncated PCM WAV extension");
        let declared = word(f, 16) as usize;
        ensure!(
            declared <= format.len() - 18,
            "truncated PCM WAV extension payload"
        );
        ensure!(
            f[18 + declared..].iter().all(|byte| *byte == 0),
            "nonzero undeclared PCM WAV extension padding"
        );
        if format.len() != 18 {
            compatibility.push(WavCompatibility::PcmFmtExtension);
        }
    }
    if data.len() % align != 0 {
        // Verified upstream quirk: the RIFF data size counts the single zero pad
        // following an odd-length, complete mono 24-bit PCM payload.
        ensure!(
            channels == 1
                && bits == 24
                && data.len() % 3 == 1
                && (data.len() - 1) % 2 == 1
                && bytes[data.end - 1] == 0,
            "PCM WAV data is not a complete set of frames"
        );
        data.end -= 1;
        compatibility.push(WavCompatibility::IncludedDataPad);
    }
    Ok(Some(WavLayout {
        info: Info {
            compatibility,
            frames: (data.len() / align) as u64,
            rate,
            channels,
        },
        bits,
        data,
    }))
}
fn decode_pcm(bytes: &[u8], layout: &WavLayout) -> Result<Vec<f32>> {
    let width = layout.bits as usize / 8;
    bytes[layout.data.clone()]
        .chunks_exact(width)
        .enumerate()
        .map(|(index, x)| {
            if index % 4096 == 0 {
                crate::host::check_cancelled()?;
            }
            Ok(match width {
                1 => (x[0] as f32 - 128.) / 128.,
                2 => i16::from_le_bytes(x.try_into().unwrap()) as f32 / 32768.,
                3 => {
                    let value = (x[0] as i32) | ((x[1] as i32) << 8) | ((x[2] as i32) << 16);
                    ((value << 8) >> 8) as f32 / 8388608.
                }
                4 => i32::from_le_bytes(x.try_into().unwrap()) as f32 / 2147483648.,
                _ => unreachable!(),
            })
        })
        .collect()
}
/// A recording's length in seconds from its header alone, without reading the
/// samples. `None` for a header this reader cannot parse (playback reports it).
/// Extensions audio assets may have: WAV, FLAC and, with the `wavpack`
/// feature, WavPack (`wv`).
pub fn readable_extension(extension: &str) -> bool {
    let e = extension.to_ascii_lowercase();
    matches!(e.as_str(), "wav" | "flac") || (cfg!(feature = "wavpack") && e == "wv")
}
/// Whether [`load`] and [`info`] read this path's format.
pub fn readable(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(readable_extension)
}
#[cfg(feature = "wavpack")]
fn decode_wavpack(bytes: &[u8], max_samples: usize) -> Result<Vec<f32>> {
    Ok(crate::wavpack::decode(bytes, max_samples)?.1)
}
#[cfg(not(feature = "wavpack"))]
fn decode_wavpack(_: &[u8], _: usize) -> Result<Vec<f32>> {
    anyhow::bail!("reading WavPack needs the `wavpack` feature")
}
fn is_wavpack(path: &Path) -> bool {
    path.extension().is_some_and(|x| x.eq_ignore_ascii_case("wv"))
}
/// The extension written audio gets by default: `wv` with the `wavpack`
/// feature, else `wav`.
pub const COMPACT_EXTENSION: &str = if cfg!(feature = "wavpack") { "wv" } else { "wav" };

/// Sample layout of a written file.
#[derive(Clone, Copy, Debug)]
pub struct WriteSpec {
    pub channels: u16,
    pub rate: u32,
    /// 16 or 24 for integer samples, 32 for float.
    pub bits: u16,
    pub float: bool,
    /// Frames that will be written; WavPack declares them up front.
    pub frames: u64,
}

/// An audio file encoder chosen by the destination's extension: `.wv` writes
/// lossless WavPack (feature `wavpack`), anything else WAV. Samples are
/// written interleaved, floats as they are and integers right-justified.
pub enum AudioWriter {
    Wav(hound::WavWriter<std::io::BufWriter<std::fs::File>>),
    #[cfg(feature = "wavpack")]
    WavPack(crate::wavpack::Encoder),
}
impl AudioWriter {
    /// Encode into `file`, in the format `path`'s extension names (`file` may
    /// be a temporary file that later becomes `path`).
    pub fn new(path: &Path, file: std::fs::File, spec: WriteSpec) -> Result<Self> {
        let file = std::io::BufWriter::with_capacity(128 * 1024, file);
        if is_wavpack(path) {
            #[cfg(feature = "wavpack")]
            return Ok(Self::WavPack(crate::wavpack::Encoder::new(
                file, spec.channels, spec.rate, spec.bits, spec.float, spec.frames,
            )?));
            #[cfg(not(feature = "wavpack"))]
            anyhow::bail!("{}: writing WavPack needs the `wavpack` feature", path.display());
        }
        Ok(Self::Wav(hound::WavWriter::new(
            file,
            hound::WavSpec {
                channels: spec.channels,
                sample_rate: spec.rate,
                bits_per_sample: spec.bits,
                sample_format: if spec.float { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
            },
        )?))
    }
    pub fn write_f32(&mut self, x: f32) -> Result<()> {
        match self {
            Self::Wav(w) => w.write_sample(x)?,
            #[cfg(feature = "wavpack")]
            Self::WavPack(w) => w.write_f32(x)?,
        }
        Ok(())
    }
    pub fn write_int(&mut self, x: i32) -> Result<()> {
        match self {
            Self::Wav(w) => w.write_sample(x)?,
            #[cfg(feature = "wavpack")]
            Self::WavPack(w) => w.write(x)?,
        }
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        match self {
            Self::Wav(w) => w.finalize()?,
            #[cfg(feature = "wavpack")]
            Self::WavPack(w) => w.finish()?,
        }
        Ok(())
    }
}

/// A file's samples as stored, for exact comparison: floats as bits,
/// integers right-justified. Reads WAV and, with the feature, WavPack.
fn stored_samples(path: &Path) -> Result<(WriteSpec, Vec<i32>)> {
    #[cfg(feature = "wavpack")]
    if is_wavpack(path) {
        let (i, raw) = crate::wavpack::decode_raw(&std::fs::read(path)?, usize::MAX)?;
        let bits = if i.float { 32 } else { i.bytes_per_sample * 8 };
        let spec = WriteSpec { channels: i.channels, rate: i.rate, bits, float: i.float, frames: i.frames };
        return Ok((spec, raw));
    }
    let r = hound::WavReader::open(path)?;
    let h = r.spec();
    let spec = WriteSpec {
        channels: h.channels,
        rate: h.sample_rate,
        bits: h.bits_per_sample,
        float: h.sample_format == hound::SampleFormat::Float,
        frames: r.duration() as u64,
    };
    let raw = if spec.float {
        r.into_samples::<f32>().map(|x| x.map(|x| x.to_bits() as i32)).collect::<Result<_, _>>()?
    } else {
        r.into_samples::<i32>().collect::<Result<_, _>>()?
    };
    Ok((spec, raw))
}

/// Losslessly re-encode the WAV or WavPack file `from` as `to`, in the format
/// `to`'s extension names, with the same channels, rate and sample format.
/// `to` is written whole (through a temporary file beside it) only after it
/// decodes to exactly the samples of `from`, bit for bit; `from` is left in
/// place. Returns the two files' sizes in bytes.
pub fn reencode(from: &Path, to: &Path) -> Result<(u64, u64)> {
    let (spec, samples) = stored_samples(from).with_context(|| from.display().to_string())?;
    let parent = to.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    // The temporary keeps `to`'s extension, so it can be read back as that format.
    let name = to.file_name().context("no file name")?.to_string_lossy();
    let tmp = tempfile::Builder::new().prefix(".reencode-").suffix(&format!("-{name}")).tempfile_in(parent)?;
    let mut writer = AudioWriter::new(to, tmp.reopen()?, spec)?;
    for &x in &samples {
        if spec.float {
            writer.write_f32(f32::from_bits(x as u32))?;
        } else {
            writer.write_int(x)?;
        }
    }
    writer.finish()?;
    let (check, decoded) = stored_samples(tmp.path())?;
    ensure!(
        (check.channels, check.rate, check.bits, check.float) == (spec.channels, spec.rate, spec.bits, spec.float)
            && decoded == samples,
        "re-encoding {} as {} is not exact",
        from.display(),
        to.display()
    );
    tmp.persist(to)?;
    Ok((std::fs::metadata(from)?.len(), std::fs::metadata(to)?.len()))
}

pub fn header_seconds(path: &Path) -> Option<f64> {
    let resolver = crate::assets::resolver();
    let reader = resolver.open(&resolver.resolve(path).ok()?).ok()?;
    let reader = std::io::BufReader::new(reader);
    let flac = path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("flac"));
    #[cfg(feature = "wavpack")]
    if is_wavpack(path) {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(reader, 1 << 20), &mut bytes).ok()?;
        let info = crate::wavpack::info(std::io::Cursor::new(bytes)).ok()?;
        return (info.rate > 0).then(|| info.frames as f64 / info.rate as f64);
    }
    let (frames, rate) = if flac {
        let info = claxon::FlacReader::new(reader).ok()?.streaminfo();
        (info.samples?, info.sample_rate)
    } else {
        let wav = hound::WavReader::new(reader).ok()?;
        (wav.duration() as u64, wav.spec().sample_rate)
    };
    (rate > 0).then(|| frames as f64 / rate as f64)
}
pub fn info(path: &Path) -> Result<Info> {
    let asset = crate::assets::snapshot(path, 512 * 1024 * 1024)?;
    info_bytes(path, &asset.bytes)
}
fn info_bytes(path: &Path, bytes: &std::sync::Arc<[u8]>) -> Result<Info> {
    crate::host::check_cancelled()?;
    ensure!(readable(path), "unsupported audio asset: {}", path.display());
    #[cfg(feature = "wavpack")]
    if is_wavpack(path) {
        let w = crate::wavpack::info(std::io::Cursor::new(&bytes[..]))?;
        return Ok(Info { compatibility: Vec::new(), frames: w.frames, rate: w.rate, channels: w.channels });
    }
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
        if let Some(layout) = &layout
            && !layout.info.compatibility.is_empty()
        {
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
        "assets must be mono/stereo WAV, FLAC or WavPack"
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
        // Only dead entries are dropped: every live entry names a recording a
        // prepared processor still owns, so evicting it would only force the
        // next preparation (a seek replacement, a revision) to decode it again.
        entries.retain(|_, (_, frames, _)| frames.strong_count() > 0);
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
    let raw = if is_wavpack(path) {
        decode_wavpack(bytes, max)?
    } else if path
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
    } else if let Some(layout) = pcm_wav_layout(bytes)?
        && !layout.info.compatibility.is_empty()
    {
        decode_pcm(bytes, &layout)?
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
    fn wav(bits: u16, extension: &[u8], payload: &[u8], declared: u32, suffix: bool) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(b"WAVEfmt ");
        body.extend_from_slice(&(16u32 + extension.len() as u32).to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&48000u32.to_le_bytes());
        body.extend_from_slice(&(48000u32 * (bits as u32 / 8)).to_le_bytes());
        body.extend_from_slice(&(bits / 8).to_le_bytes());
        body.extend_from_slice(&bits.to_le_bytes());
        body.extend_from_slice(extension);
        body.extend_from_slice(b"data");
        body.extend_from_slice(&declared.to_le_bytes());
        body.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            body.push(0);
        }
        if suffix {
            body.extend_from_slice(b"smpl");
            body.extend_from_slice(&4u32.to_le_bytes());
            body.extend_from_slice(&[0; 4]);
        }
        let mut bytes = Vec::from(&b"RIFF"[..]);
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);
        bytes
    }
    fn snapshot(bytes: Vec<u8>) -> crate::assets::AssetSnapshot {
        crate::assets::AssetSnapshot {
            version: (bytes.len() as u64, 0),
            bytes: bytes.into(),
        }
    }
    #[test]
    fn pcm_fmt20_decodes_without_rewriting_original_bytes() {
        let original = wav(16, &[0, 0, 0, 0], &[0, 64, 0, 192], 4, true);
        let asset = snapshot(original.clone());
        let info = info_bytes(Path::new("fixture.wav"), &asset.bytes).unwrap();
        assert_eq!(info.compatibility, vec![WavCompatibility::PcmFmtExtension]);
        assert_eq!(info.frames, 2);
        let (decoded, frames) = decode(Path::new("fixture.wav"), 2, asset).unwrap();
        assert_eq!(decoded.frames, info.frames);
        assert_eq!(frames, vec![[0.5; 2], [-0.5; 2]]);
        assert_eq!(original, wav(16, &[0, 0, 0, 0], &[0, 64, 0, 192], 4, true));
    }
    #[test]
    fn included_mono24_zero_pad_is_not_decoded_as_a_partial_sample() {
        let canonical = snapshot(wav(24, &[0, 0], &[0, 0, 64], 3, true));
        let canonical_info = info_bytes(Path::new("canonical.wav"), &canonical.bytes).unwrap();
        assert!(canonical_info.compatibility.is_empty());
        assert_eq!(
            decode(Path::new("canonical.wav"), 1, canonical).unwrap().1,
            vec![[0.5; 2]]
        );
        // One signed24 frame is odd-length; upstream declareddata erroneously
        // includes itszero alignmentpad, followed by anotherRIFFchunk.
        let asset = snapshot(wav(24, &[0, 0], &[0, 0, 64, 0], 4, true));
        let info = info_bytes(Path::new("fixture.wav"), &asset.bytes).unwrap();
        assert_eq!(info.compatibility, vec![WavCompatibility::IncludedDataPad]);
        assert_eq!(info.frames, 1);
        let (decoded, frames) = decode(Path::new("fixture.wav"), 1, asset).unwrap();
        assert_eq!(decoded.frames, 1);
        assert_eq!(frames, vec![[0.5; 2]]);
    }
    #[test]
    fn real_partial_frames_bad_extensions_and_truncation_reject() {
        for bytes in [
            wav(24, &[0, 0], &[0, 0, 64, 1], 4, true),
            wav(16, &[0, 0], &[0, 64, 1], 3, true),
            wav(16, &[0, 0, 1, 0], &[0, 64], 2, true),
        ] {
            assert!(info_bytes(Path::new("bad.wav"), &snapshot(bytes).bytes).is_err());
        }
        let mut bytes = wav(16, &[0, 0, 0, 0], &[0, 64], 2, true);
        bytes.pop();
        assert!(info_bytes(Path::new("bad.wav"), &snapshot(bytes).bytes).is_err());
    }
    #[test]
    fn compatibility_preserves_limits_and_cancellation() {
        let bytes = wav(16, &[0, 0, 0, 0], &[0, 64, 0, 192], 4, true);
        assert!(decode(Path::new("fixture.wav"), 1, snapshot(bytes.clone())).is_err());
        let context = crate::host::HostContext::default();
        context
            .cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            context
                .run(|| info_bytes(Path::new("fixture.wav"), &snapshot(bytes).bytes))
                .is_err()
        );
    }
}
