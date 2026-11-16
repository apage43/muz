//! Lossless WavPack (`.wv`) reading and writing through the system libwavpack
//! (feature `wavpack`, found with pkg-config). Only what audio assets and
//! render outputs need: mono or stereo, 16/24-bit integer or 32-bit float,
//! lossless. A float stream round-trips bit for bit, signed zeros, denormals
//! and all.
use anyhow::{Context, Result, bail, ensure};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::{Read, Seek, SeekFrom, Write};

#[repr(C)]
struct WavpackContext {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
struct WavpackConfig {
    bitrate: f32,
    shaping_weight: f32,
    bits_per_sample: c_int,
    bytes_per_sample: c_int,
    qmode: c_int,
    flags: c_int,
    xmode: c_int,
    num_channels: c_int,
    float_norm_exp: c_int,
    block_samples: i32,
    worker_threads: i32,
    sample_rate: i32,
    channel_mask: i32,
    md5_checksum: [u8; 16],
    md5_read: u8,
    num_tag_strings: c_int,
    tag_strings: usize,
}

#[repr(C)]
struct StreamReader64 {
    read_bytes: unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> i32,
    write_bytes: unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> i32,
    get_pos: unsafe extern "C" fn(*mut c_void) -> i64,
    set_pos_abs: unsafe extern "C" fn(*mut c_void, i64) -> c_int,
    set_pos_rel: unsafe extern "C" fn(*mut c_void, i64, c_int) -> c_int,
    push_back_byte: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    get_length: unsafe extern "C" fn(*mut c_void) -> i64,
    can_seek: unsafe extern "C" fn(*mut c_void) -> c_int,
    truncate_here: unsafe extern "C" fn(*mut c_void) -> c_int,
    close: unsafe extern "C" fn(*mut c_void) -> c_int,
}

type BlockOutput = unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> c_int;

const OPEN_NORMALIZE: c_int = 0x10;
const MODE_FLOAT: c_int = 0x8;
const CONFIG_EXTRA_MODE: c_int = 0x2000000;

unsafe extern "C" {
    fn WavpackOpenFileInputEx64(
        reader: *mut StreamReader64,
        wv_id: *mut c_void,
        wvc_id: *mut c_void,
        error: *mut c_char,
        flags: c_int,
        norm_offset: c_int,
    ) -> *mut WavpackContext;
    fn WavpackGetMode(wpc: *mut WavpackContext) -> c_int;
    fn WavpackGetNumSamples64(wpc: *mut WavpackContext) -> i64;
    fn WavpackGetSampleRate(wpc: *mut WavpackContext) -> u32;
    fn WavpackGetNumChannels(wpc: *mut WavpackContext) -> c_int;
    fn WavpackGetBytesPerSample(wpc: *mut WavpackContext) -> c_int;
    fn WavpackUnpackSamples(wpc: *mut WavpackContext, buffer: *mut i32, samples: u32) -> u32;
    fn WavpackGetNumErrors(wpc: *mut WavpackContext) -> c_int;
    fn WavpackGetErrorMessage(wpc: *mut WavpackContext) -> *const c_char;
    fn WavpackCloseFile(wpc: *mut WavpackContext) -> *mut WavpackContext;
    fn WavpackOpenFileOutput(
        blockout: BlockOutput,
        wv_id: *mut c_void,
        wvc_id: *mut c_void,
    ) -> *mut WavpackContext;
    fn WavpackSetConfiguration64(
        wpc: *mut WavpackContext,
        config: *mut WavpackConfig,
        total_samples: i64,
        chan_ids: *const u8,
    ) -> c_int;
    fn WavpackPackInit(wpc: *mut WavpackContext) -> c_int;
    fn WavpackPackSamples(wpc: *mut WavpackContext, buffer: *mut i32, count: u32) -> c_int;
    fn WavpackFlushSamples(wpc: *mut WavpackContext) -> c_int;
}

/// A stream's layout, from its first block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    pub frames: u64,
    pub rate: u32,
    pub channels: u16,
    /// 32-bit float samples; otherwise integers of `bytes_per_sample` bytes.
    pub float: bool,
    pub bytes_per_sample: u16,
}

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// The reader callbacks' state: a seekable source and the byte the library
/// may push back.
struct Source<'a> {
    inner: &'a mut dyn ReadSeek,
    pushed: Option<u8>,
    length: i64,
}

unsafe fn source<'a>(id: *mut c_void) -> &'a mut Source<'a> {
    unsafe { &mut *(id as *mut Source) }
}

unsafe extern "C" fn read_bytes(id: *mut c_void, data: *mut c_void, count: i32) -> i32 {
    let s = unsafe { source(id) };
    let out = unsafe { std::slice::from_raw_parts_mut(data as *mut u8, count.max(0) as usize) };
    let mut n = 0;
    if let Some(b) = s.pushed.take()
        && !out.is_empty()
    {
        out[0] = b;
        n = 1;
    }
    while n < out.len() {
        match s.inner.read(&mut out[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    n as i32
}
unsafe extern "C" fn write_bytes(_: *mut c_void, _: *mut c_void, _: i32) -> i32 {
    0
}
unsafe extern "C" fn get_pos(id: *mut c_void) -> i64 {
    let s = unsafe { source(id) };
    match s.inner.stream_position() {
        Ok(p) => p as i64 - s.pushed.is_some() as i64,
        Err(_) => -1,
    }
}
unsafe extern "C" fn set_pos_abs(id: *mut c_void, pos: i64) -> c_int {
    unsafe { set_pos_rel(id, pos, 0) }
}
unsafe extern "C" fn set_pos_rel(id: *mut c_void, delta: i64, mode: c_int) -> c_int {
    let s = unsafe { source(id) };
    let back = s.pushed.take().is_some() as i64;
    let to = match mode {
        0 => SeekFrom::Start(delta.max(0) as u64),
        1 => SeekFrom::Current(delta - back),
        _ => SeekFrom::End(delta),
    };
    if s.inner.seek(to).is_ok() { 0 } else { -1 }
}
unsafe extern "C" fn push_back_byte(id: *mut c_void, c: c_int) -> c_int {
    unsafe { source(id) }.pushed = Some(c as u8);
    c
}
unsafe extern "C" fn get_length(id: *mut c_void) -> i64 {
    unsafe { source(id) }.length
}
unsafe extern "C" fn can_seek(_: *mut c_void) -> c_int {
    1
}
unsafe extern "C" fn truncate_here(_: *mut c_void) -> c_int {
    -1
}
unsafe extern "C" fn close(_: *mut c_void) -> c_int {
    0
}

/// An open decoder; closes on drop. Borrows its source for its lifetime.
struct Decoder<'a> {
    context: *mut WavpackContext,
    _source: Box<Source<'a>>,
    _reader: Box<StreamReader64>,
    info: StreamInfo,
}

impl<'a> Decoder<'a> {
    fn open(inner: &'a mut dyn ReadSeek) -> Result<Self> {
        let length = inner.seek(SeekFrom::End(0))? as i64;
        inner.seek(SeekFrom::Start(0))?;
        let mut source = Box::new(Source { inner, pushed: None, length });
        let mut reader = Box::new(StreamReader64 {
            read_bytes,
            write_bytes,
            get_pos,
            set_pos_abs,
            set_pos_rel,
            push_back_byte,
            get_length,
            can_seek,
            truncate_here,
            close,
        });
        let mut error = [0 as c_char; 81];
        let context = unsafe {
            WavpackOpenFileInputEx64(
                &mut *reader,
                &mut *source as *mut Source as *mut c_void,
                std::ptr::null_mut(),
                error.as_mut_ptr(),
                OPEN_NORMALIZE,
                0,
            )
        };
        if context.is_null() {
            let message = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy().into_owned();
            bail!("invalid WavPack stream: {message}");
        }
        let info = unsafe {
            StreamInfo {
                frames: WavpackGetNumSamples64(context).max(0) as u64,
                rate: WavpackGetSampleRate(context),
                channels: WavpackGetNumChannels(context).clamp(0, u16::MAX as c_int) as u16,
                float: WavpackGetMode(context) & MODE_FLOAT != 0,
                bytes_per_sample: WavpackGetBytesPerSample(context).clamp(0, 8) as u16,
            }
        };
        let decoder = Self { context, _source: source, _reader: reader, info };
        ensure!(
            decoder.info.rate > 0 && matches!(decoder.info.channels, 1 | 2),
            "WavPack assets must be mono or stereo"
        );
        ensure!(
            decoder.info.float || (1..=4).contains(&decoder.info.bytes_per_sample),
            "unsupported WavPack sample size"
        );
        Ok(decoder)
    }

    fn error(&self) -> String {
        unsafe { CStr::from_ptr(WavpackGetErrorMessage(self.context)) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Decoder<'_> {
    fn drop(&mut self) {
        unsafe { WavpackCloseFile(self.context) };
    }
}

/// A stream's layout from its header, reading as little as the library needs.
pub fn info(mut reader: impl Read + Seek) -> Result<StreamInfo> {
    Ok(Decoder::open(&mut reader)?.info)
}

/// Decode a whole stream into interleaved samples normalized to ±1.0 (float
/// streams exactly as stored), at most `max_samples` of them.
pub fn decode(bytes: &[u8], max_samples: usize) -> Result<(StreamInfo, Vec<f32>)> {
    let (info, raw) = decode_raw(bytes, max_samples)?;
    let scale = (1u64 << (info.bytes_per_sample * 8 - 1)) as f32;
    let samples = if info.float {
        raw.into_iter().map(|x| f32::from_bits(x as u32)).collect()
    } else {
        raw.into_iter().map(|x| x as f32 / scale).collect()
    };
    Ok((info, samples))
}

/// Decode a whole stream as stored: a float's bits, or an integer
/// right-justified in its bytes.
pub fn decode_raw(bytes: &[u8], max_samples: usize) -> Result<(StreamInfo, Vec<i32>)> {
    let mut cursor = std::io::Cursor::new(bytes);
    let decoder = Decoder::open(&mut cursor)?;
    let info = decoder.info;
    let channels = info.channels as usize;
    let total = (info.frames as usize).saturating_mul(channels);
    ensure!(total <= max_samples, "sample allocation limit exceeded");
    let mut out = Vec::with_capacity(total);
    let mut buffer = vec![0i32; 4096 * channels];
    loop {
        crate::host::check_cancelled()?;
        let frames = unsafe {
            WavpackUnpackSamples(decoder.context, buffer.as_mut_ptr(), (buffer.len() / channels) as u32)
        } as usize;
        if frames == 0 {
            break;
        }
        ensure!(out.len() + frames * channels <= max_samples, "sample allocation limit exceeded");
        out.extend_from_slice(&buffer[..frames * channels]);
    }
    let errors = unsafe { WavpackGetNumErrors(decoder.context) };
    ensure!(errors == 0, "corrupt WavPack stream: {} ({errors} bad blocks)", decoder.error());
    ensure!(out.len() == total, "truncated WavPack stream: {} of {total} samples", out.len());
    Ok((info, out))
}

/// The block callback's sink.
struct Sink {
    out: Box<dyn Write + Send>,
    error: Option<std::io::Error>,
}

unsafe extern "C" fn block_out(id: *mut c_void, data: *mut c_void, count: i32) -> c_int {
    let sink = unsafe { &mut *(id as *mut Sink) };
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, count.max(0) as usize) };
    match sink.out.write_all(bytes) {
        Ok(()) => 1,
        Err(e) => {
            sink.error = Some(e);
            0
        }
    }
}

/// A lossless WavPack encoder of a stream whose length is known up front.
pub struct Encoder {
    context: *mut WavpackContext,
    sink: Box<Sink>,
    channels: usize,
    buffer: Vec<i32>,
    frames: u64,
    written: u64,
}

// The context is owned by the encoder and only used through `&mut self`.
unsafe impl Send for Encoder {}

impl Encoder {
    /// `bits` is 16 or 24 for integer samples, or 32 with `float`.
    pub fn new(
        out: impl Write + Send + 'static,
        channels: u16,
        rate: u32,
        bits: u16,
        float: bool,
        frames: u64,
    ) -> Result<Self> {
        ensure!(matches!(channels, 1 | 2), "WavPack output must be mono or stereo");
        ensure!(
            if float { bits == 32 } else { matches!(bits, 16 | 24) },
            "WavPack output is 16 or 24-bit integer or 32-bit float"
        );
        let mut sink = Box::new(Sink { out: Box::new(out), error: None });
        let context = unsafe {
            WavpackOpenFileOutput(block_out, &mut *sink as *mut Sink as *mut c_void, std::ptr::null_mut())
        };
        ensure!(!context.is_null(), "cannot start a WavPack encoder");
        let mut encoder = Self {
            context,
            sink,
            channels: channels as usize,
            buffer: Vec::with_capacity(4096 * channels as usize),
            frames,
            written: 0,
        };
        let mut config = WavpackConfig {
            bits_per_sample: bits as c_int,
            bytes_per_sample: (bits / 8) as c_int,
            num_channels: channels as c_int,
            channel_mask: if channels == 2 { 3 } else { 4 },
            sample_rate: rate as i32,
            float_norm_exp: if float { 127 } else { 0 },
            // `-x1`: a little more analysis when encoding, decoding unchanged.
            flags: CONFIG_EXTRA_MODE,
            xmode: 1,
            ..Default::default()
        };
        let ok = unsafe {
            WavpackSetConfiguration64(encoder.context, &mut config, frames as i64, std::ptr::null())
                != 0
                && WavpackPackInit(encoder.context) != 0
        };
        if !ok {
            bail!("cannot configure the WavPack encoder: {}", encoder.error());
        }
        Ok(encoder)
    }

    fn error(&mut self) -> String {
        if let Some(e) = self.sink.error.take() {
            return e.to_string();
        }
        unsafe { CStr::from_ptr(WavpackGetErrorMessage(self.context)) }
            .to_string_lossy()
            .into_owned()
    }

    /// One sample: an integer right-justified in `bits`, or a float's bits.
    pub fn write(&mut self, sample: i32) -> Result<()> {
        self.buffer.push(sample);
        if self.buffer.len() == self.buffer.capacity() {
            self.pack()?;
        }
        Ok(())
    }

    pub fn write_f32(&mut self, sample: f32) -> Result<()> {
        self.write(sample.to_bits() as i32)
    }

    fn pack(&mut self) -> Result<()> {
        let frames = self.buffer.len() / self.channels;
        if frames == 0 {
            return Ok(());
        }
        self.written += frames as u64;
        ensure!(self.written <= self.frames, "more WavPack frames than declared");
        let ok = unsafe { WavpackPackSamples(self.context, self.buffer.as_mut_ptr(), frames as u32) };
        self.buffer.clear();
        if ok == 0 {
            bail!("WavPack encoding failed: {}", self.error());
        }
        Ok(())
    }

    /// Flush the last block; every declared frame must have been written.
    pub fn finish(mut self) -> Result<()> {
        ensure!(self.buffer.len() % self.channels == 0, "partial WavPack frame");
        self.pack()?;
        ensure!(
            self.written == self.frames,
            "WavPack stream declared {} frames but got {}",
            self.frames,
            self.written
        );
        if unsafe { WavpackFlushSamples(self.context) } == 0 {
            bail!("WavPack encoding failed: {}", self.error());
        }
        self.sink.out.flush().context("writing WavPack")?;
        Ok(())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { WavpackCloseFile(self.context) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);
    impl Write for Shared {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn float_round_trips_bit_for_bit() {
        let mut samples: Vec<f32> = (0..20_000)
            .map(|i| ((i as f32) * 0.013).sin() * 0.7 + (i % 7) as f32 * 1e-6)
            .collect();
        // Silence, signed zero, denormals, out-of-range peaks.
        samples[100..4000].fill(0.0);
        samples[5] = -0.0;
        samples[6] = f32::from_bits(1);
        samples[7] = -f32::MIN_POSITIVE / 3.0;
        samples[8] = 3.75;
        let out = Shared::default();
        let mut e = Encoder::new(out.clone(), 2, 48000, 32, true, 10_000).unwrap();
        for &x in &samples {
            e.write_f32(x).unwrap();
        }
        e.finish().unwrap();
        let bytes = out.0.lock().unwrap().clone();
        assert!(bytes.len() < samples.len() * 4);
        let (info, decoded) = decode(&bytes, usize::MAX).unwrap();
        assert_eq!(
            info,
            StreamInfo { frames: 10_000, rate: 48000, channels: 2, float: true, bytes_per_sample: 4 }
        );
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&decoded), bits(&samples));
        assert_eq!(super::info(std::io::Cursor::new(&bytes)).unwrap(), info);
    }

    #[test]
    fn integers_scale_like_wav_and_errors_are_reported() {
        let out = Shared::default();
        let mut e = Encoder::new(out.clone(), 1, 44100, 24, false, 3).unwrap();
        for x in [0, 1 << 22, -(1 << 23)] {
            e.write(x).unwrap();
        }
        e.finish().unwrap();
        let bytes = out.0.lock().unwrap().clone();
        let (info, decoded) = decode(&bytes, 16).unwrap();
        assert_eq!((info.channels, info.float, info.bytes_per_sample), (1, false, 3));
        assert_eq!(decoded, [0.0, 0.5, -1.0]);
        assert!(decode(&bytes, 2).is_err(), "allocation limit");
        assert!(decode(&bytes[..bytes.len() / 2], 16).is_err(), "truncation");
        assert!(decode(b"RIFF....WAVE", 16).is_err());
        let short = Encoder::new(Shared::default(), 2, 48000, 32, true, 4).unwrap();
        assert!(short.finish().is_err(), "a declared length is enforced");
    }
}
