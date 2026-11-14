//! Time Muz's real-time processing path without opening an audio device.
//! cargo run --release --example woodwind_callback -- SONG.muz [seconds]
use anyhow::{Context, Result, ensure};
use muz::audio::{AudioConfig, AudioEngine};
use std::{env, path::Path, time::Instant};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .context("usage: woodwind_callback SONG.muz [seconds]")?;
    let seconds = args.next().as_deref().unwrap_or("10").parse::<f64>()?;
    ensure!(
        seconds > 0.0 && seconds <= 600.0,
        "seconds must be in (0, 600]"
    );
    ensure!(args.next().is_none(), "expected SONG.muz [seconds]");
    let session = muz::compile::compile(Path::new(&path))?.session;
    let sample_rate = 48_000.0;
    println!("frames  mean_us  p99_us  p99.9_us  max_us  deadline_us  overruns");
    for frames in [64, 128, 256] {
        let config = AudioConfig {
            sample_rate,
            max_frames: frames,
            offline: false,
        };
        let mut engine = AudioEngine::new(&session, config)?;
        let mut samples = vec![0.; frames * 2];
        let count = (seconds * f64::from(sample_rate) / frames as f64).ceil() as usize;
        let mut times = Vec::with_capacity(count);
        let deadline = frames as f64 * 1e6 / f64::from(sample_rate);
        engine.set_running(true);
        for _ in 0..count {
            let start = Instant::now();
            engine.render_interleaved(&mut samples, 2)?;
            times.push(start.elapsed().as_secs_f64() * 1e6);
        }
        times.sort_unstable_by(f64::total_cmp);
        let percentile = |p: f64| times[((count as f64 * p).ceil() as usize).saturating_sub(1)];
        let overruns = count - times.partition_point(|v| *v <= deadline);
        println!(
            "{frames:6} {:8.1} {:7.1} {:9.1} {:7.1} {deadline:12.1} {overruns:9}",
            times.iter().sum::<f64>() / count as f64,
            percentile(0.99),
            percentile(0.999),
            times[count - 1],
        );
    }
    Ok(())
}
