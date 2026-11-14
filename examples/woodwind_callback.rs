//! Time Muz's real-time processing path without opening an audio device.
//! cargo run --release --example woodwind_callback -- SONG.muz [seconds] [threads]
//!
//! For each block size it reports the calling (audio) thread's user-mode
//! cycles per block, as megacycles per second of audio, and wall-clock time
//! per block. Cycles do not count while the thread is descheduled, so they
//! hold steady on a loaded machine where wall-clock time does not; `%core`
//! is the mean against a 4 GHz core. With worker threads the audio thread's
//! cycles include waiting for them. `threads` (default: the live engine's
//! choice) is the number of threads processing tracks; 1 is serial.
use anyhow::{Context, Result, ensure};
use muz::audio::{AudioConfig, AudioEngine};
use std::{env, path::Path, time::Instant};

/// A user-mode cycle counter for the calling thread (Linux perf events).
struct Cycles(i32);
impl Cycles {
    fn open() -> Option<Self> {
        // perf_event_attr: PERF_TYPE_HARDWARE, PERF_COUNT_HW_CPU_CYCLES,
        // exclude_kernel and exclude_hv.
        let mut attr = [0u64; 16];
        attr[0] = (std::mem::size_of_val(&attr) as u64) << 32;
        attr[5] = 1 << 5 | 1 << 6;
        let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, attr.as_ptr(), 0, -1, -1, 0) };
        (fd >= 0).then_some(Self(fd as i32))
    }
    fn read(&self) -> u64 {
        let mut value = 0u64;
        let n = unsafe { libc::read(self.0, (&raw mut value).cast(), 8) };
        assert_eq!(n, 8, "reading the cycle counter");
        value
    }
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .context("usage: woodwind_callback SONG.muz [seconds] [threads]")?;
    let seconds = args.next().as_deref().unwrap_or("10").parse::<f64>()?;
    ensure!(
        seconds > 0.0 && seconds <= 600.0,
        "seconds must be in (0, 600]"
    );
    let threads = args.next().map(|t| t.parse::<usize>()).transpose()?;
    ensure!(
        args.next().is_none(),
        "expected SONG.muz [seconds] [threads]"
    );
    let session = muz::compile::compile(Path::new(&path))?.session;
    let sample_rate = 48_000.0;
    let cycles = Cycles::open();
    if cycles.is_none() {
        eprintln!("no cycle counter (perf_event_paranoid?): wall-clock time only");
    }
    println!(
        "frames threads  mean_Mc/s  p99_Mc/s  %core   mean_us  p99_us  max_us  deadline_us  overruns"
    );
    for frames in [64, 128, 256] {
        let config = AudioConfig {
            sample_rate,
            max_frames: frames,
            offline: false,
        };
        let mut engine = AudioEngine::new(&session, config)?;
        if let Some(threads) = threads {
            engine.set_render_threads(threads)?;
        }
        let mut samples = vec![0.; frames * 2];
        let count = (seconds * f64::from(sample_rate) / frames as f64).ceil() as usize;
        let mut times = Vec::with_capacity(count);
        let mut spent = Vec::with_capacity(count);
        let deadline = frames as f64 * 1e6 / f64::from(sample_rate);
        let blocks_per_second = f64::from(sample_rate) / frames as f64;
        engine.set_running(true);
        for _ in 0..count {
            let before = cycles.as_ref().map_or(0, Cycles::read);
            let start = Instant::now();
            engine.render_interleaved(&mut samples, 2)?;
            times.push(start.elapsed().as_secs_f64() * 1e6);
            let after = cycles.as_ref().map_or(0, Cycles::read);
            spent.push((after - before) as f64 * blocks_per_second / 1e6);
        }
        let percentile = |v: &mut Vec<f64>, p: f64| {
            v.sort_unstable_by(f64::total_cmp);
            v[((count as f64 * p).ceil() as usize).saturating_sub(1)]
        };
        let mean = |v: &[f64]| v.iter().sum::<f64>() / count as f64;
        let (mean_us, mean_mc) = (mean(&times), mean(&spent));
        let p99_us = percentile(&mut times, 0.99);
        let p99_mc = percentile(&mut spent, 0.99);
        let overruns = count - times.partition_point(|v| *v <= deadline);
        println!(
            "{frames:6} {:7} {mean_mc:10.0} {p99_mc:9.0} {:5.1}% {mean_us:9.1} {p99_us:7.1} {:7.1} {deadline:12.1} {overruns:9}",
            engine.live_threads(),
            mean_mc / 40.,
            times[count - 1],
        );
    }
    Ok(())
}
