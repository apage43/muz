use anyhow::Result;
use std::path::Path;

/// How `muz analyze` divides the file for its per-window peak list.
#[derive(Clone, Debug)]
pub enum PeakWindows {
    Seconds(f64),
    /// Bars at a constant tempo, bar 1 starting at time zero.
    Bars {
        bpm: f64,
        beats_per_bar: f64,
    },
    /// The bars of a compiled song, following its tempo and meter maps, bar 1
    /// starting at time zero.
    SongBars {
        transport: crate::model::Transport,
        tempos: Vec<crate::midi::MidiTempo>,
    },
}

/// Bar start times in seconds of a song's meter and tempo maps, up to `until`.
fn song_bar_starts(
    transport: &crate::model::Transport,
    tempos: &[crate::midi::MidiTempo],
    until: f64,
) -> Vec<f64> {
    let mut starts = vec![];
    let mut beat = 0.0;
    loop {
        let seconds = crate::compile::seconds_at(beat, tempos);
        if !starts.is_empty() && seconds >= until {
            break starts;
        }
        starts.push(seconds);
        let bar = transport.bar_at(beat);
        beat = bar.start + bar.quarters();
    }
}

pub fn analyze(path: &Path) -> Result<serde_json::Value> {
    analyze_with(path, None)
}

/// Whole-file measurements plus, when `windows` is given, the sample and 4x
/// true peak of every window, so an over can be located.
pub fn analyze_with(path: &Path, windows: Option<PeakWindows>) -> Result<serde_json::Value> {
    let named = |error: anyhow::Error| crate::lang::Diagnostic::named(error, path);
    let reader = hound::WavReader::open(path)
        .map_err(|error| named(anyhow::anyhow!("cannot read WAV: {error}")))?;
    let spec = reader.spec();
    let values: Vec<f64> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|v| v.map(|v| v as f64))
            .collect::<Result<_, _>>()
            .map_err(|error| named(anyhow::anyhow!("{error}")))?,
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .map(|v| v.map(|v| v as f64 / (1u64 << (spec.bits_per_sample - 1)) as f64))
            .collect::<Result<_, _>>()
            .map_err(|error| named(anyhow::anyhow!("{error}")))?,
    };
    if values.iter().any(|v| !v.is_finite()) {
        return Err(named(anyhow::anyhow!(
            "audio contains non-finite samples; re-render the mix"
        )));
    }
    let peak = values.iter().map(|x| x.abs()).fold(0.0, f64::max);
    let squares = values.iter().map(|x| x * x).sum::<f64>();
    let mut lr = 0.0;
    let (mut ll, mut rr) = (0.0, 0.0);
    if spec.channels >= 2 {
        for f in values.chunks_exact(spec.channels as usize) {
            lr += f[0] * f[1];
            ll += f[0] * f[0];
            rr += f[1] * f[1];
        }
    }
    let mut meter = ebur128::EbuR128::new(
        spec.channels as u32,
        spec.sample_rate,
        ebur128::Mode::I | ebur128::Mode::LRA | ebur128::Mode::TRUE_PEAK,
    )?;
    meter.add_frames_f64(&values)?;
    let integrated = meter.loudness_global()?;
    let true_peak = (0..spec.channels as u32)
        .map(|c| meter.true_peak(c))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .fold(0.0, f64::max);
    let range = meter.loudness_range()?;
    let channels = spec.channels.max(1) as usize;
    let frames = values.len() / channels;
    let rate = f64::from(spec.sample_rate);
    let duration = frames as f64 / rate;
    let uniform = |window: f64| {
        let count = ((frames.saturating_sub(1) as f64 / (window * rate)) as usize) + 1;
        (0..count).map(|i| i as f64 * window).collect::<Vec<_>>()
    };
    // Window start times in seconds; each window ends where the next starts.
    let starts: Option<Vec<f64>> = match &windows {
        None => None,
        Some(PeakWindows::Seconds(seconds)) => {
            anyhow::ensure!(
                seconds.is_finite() && *seconds > 0.0,
                "--window must be a positive number of seconds"
            );
            Some(uniform(*seconds))
        }
        Some(PeakWindows::Bars { bpm, beats_per_bar }) => {
            anyhow::ensure!(
                bpm.is_finite() && *bpm > 0.0 && beats_per_bar.is_finite() && *beats_per_bar > 0.0,
                "--bpm and --beats-per-bar must be positive"
            );
            Some(uniform(60.0 / bpm * beats_per_bar))
        }
        Some(PeakWindows::SongBars { transport, tempos }) => {
            Some(song_bar_starts(transport, tempos, duration))
        }
    };
    let slot = |frame: usize| {
        starts.as_ref().map_or(0, |starts| {
            starts
                .partition_point(|s| *s * rate <= frame as f64 + 1e-6)
                .saturating_sub(1)
        })
    };
    let mut window_peaks = vec![(0.0f64, 0.0f32); starts.as_ref().map_or(0, Vec::len)];
    let (mut sample_peak_frame, mut sample_peak) = (0, 0.0f64);
    let (mut true_peak_frame, mut true_peak_estimate) = (0, 0.0f32);
    for (index, value) in values.iter().enumerate() {
        let frame = index / channels;
        if value.abs() > sample_peak {
            (sample_peak, sample_peak_frame) = (value.abs(), frame);
        }
        if let Some(entry) = window_peaks.get_mut(slot(frame)) {
            entry.0 = entry.0.max(value.abs());
        }
    }
    for channel in 0..channels {
        let samples = values
            .iter()
            .skip(channel)
            .step_by(channels)
            .map(|v| *v as f32);
        crate::audio::true_peak::for_each_true_peak(samples, |frame, estimate| {
            if estimate > true_peak_estimate {
                (true_peak_estimate, true_peak_frame) = (estimate, frame);
            }
            if let Some(entry) = window_peaks.get_mut(slot(frame)) {
                entry.1 = entry.1.max(estimate);
            }
        });
    }
    let dbfs = |x: f64| 20.0 * x.max(1e-20).log10();
    let mut report = serde_json::json!({"integrated_lufs": integrated.is_finite().then_some(integrated), "true_peak_dbtp":20.0*true_peak.max(1e-20).log10(), "loudness_range_lu": range.is_finite().then_some(range), "seconds":values.len()as f64/spec.channels as f64/spec.sample_rate as f64,"sample_rate":spec.sample_rate,"channels":spec.channels,"sample_peak_dbfs":20.0*peak.max(1e-20).log10(),"rms_dbfs":10.0*(squares/values.len().max(1)as f64).max(1e-40).log10(),"dc":values.iter().sum::<f64>()/values.len().max(1)as f64,"correlation":lr/(ll*rr).sqrt().max(1e-20)});
    report["sample_peak_seconds"] = (sample_peak_frame as f64 / rate).into();
    report["true_peak_seconds"] = (true_peak_frame as f64 / rate).into();
    if let Some(starts) = &starts {
        let bars = matches!(
            windows,
            Some(PeakWindows::Bars { .. } | PeakWindows::SongBars { .. })
        );
        report["windows"] = window_peaks
            .iter()
            .enumerate()
            .map(|(i, (sample, estimate))| {
                let mut row = serde_json::json!({
                    "start_seconds": starts[i],
                    "end_seconds": starts.get(i + 1).copied().unwrap_or(duration).min(duration),
                    "sample_peak_dbfs": dbfs(*sample),
                    "true_peak_dbtp": dbfs(f64::from(*estimate)),
                });
                if bars {
                    row["bar"] = (i + 1).into();
                }
                row
            })
            .collect();
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, rate: u32, frames: impl Fn(usize) -> f32, count: usize) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for n in 0..count {
            let x = frames(n);
            writer.write_sample(x).unwrap();
            writer.write_sample(x * 0.5).unwrap();
        }
        writer.finalize().unwrap();
    }

    /// A quarter-rate tone at 45 degrees from 2 s to 3 s is found in bar 2
    /// (120 BPM, 2 s bars), its sample peak 3 dB under its true peak.
    #[test]
    fn bar_windows_locate_an_inter_sample_peak() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("over.wav");
        let rate = 8_000;
        write(
            &path,
            rate,
            |n| {
                if (2 * rate as usize..3 * rate as usize).contains(&n) {
                    (std::f64::consts::FRAC_PI_2 * n as f64 + std::f64::consts::FRAC_PI_4).sin()
                        as f32
                } else {
                    0.01 * (n as f32 * 0.01).sin()
                }
            },
            8 * rate as usize,
        );
        let report = analyze_with(
            &path,
            Some(PeakWindows::Bars {
                bpm: 120.0,
                beats_per_bar: 4.0,
            }),
        )
        .unwrap();
        let windows = report["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 4);
        let bar = &windows[1];
        assert_eq!(bar["bar"], 2);
        assert!((bar["sample_peak_dbfs"].as_f64().unwrap() + 3.01).abs() < 0.01);
        // Switching the tone on and off adds a little Gibbs overshoot.
        let true_peak = bar["true_peak_dbtp"].as_f64().unwrap();
        assert!((-0.02..0.2).contains(&true_peak), "{bar}");
        // Bar 3 starts a second after the tone stops.
        assert!(windows[2]["true_peak_dbtp"].as_f64().unwrap() < -39.0);
        let at = report["true_peak_seconds"].as_f64().unwrap();
        assert!((2.0..3.0).contains(&at), "{at}");
        // The existing whole-file meter agrees with the windowed estimate.
        let whole = report["true_peak_dbtp"].as_f64().unwrap();
        assert!((whole - true_peak).abs() < 0.1, "{whole}");
        assert!(analyze(&path).unwrap().get("windows").is_none());
    }

    /// The shared estimator and the ebur128 meter read tones up to 19 kHz
    /// within a small fraction of a dB of each other. Nearer Nyquist the two
    /// interpolation filters differ (about 0.7 dB at 20 and 21.5 kHz).
    #[test]
    fn windowed_estimate_matches_the_whole_file_meter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hf.wav");
        let tau = 2.0 * std::f64::consts::PI;
        let mut worst = 0.0f64;
        for (frequency, phase) in [(1_000.0, 0.2), (15_000.0, 0.3), (19_000.0, 1.1)] {
            write(
                &path,
                48_000,
                |n| (0.5 * (tau * frequency * n as f64 / 48_000.0 + phase).sin()) as f32,
                48_000,
            );
            let report = analyze_with(&path, Some(PeakWindows::Seconds(0.25))).unwrap();
            let windowed = report["windows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w["true_peak_dbtp"].as_f64().unwrap())
                .fold(f64::NEG_INFINITY, f64::max);
            let difference = windowed - report["true_peak_dbtp"].as_f64().unwrap();
            println!("{frequency} Hz: estimate - ebur128 = {difference:.4} dB");
            worst = worst.max(difference.abs());
        }
        assert!(worst < 0.15, "{worst}");
    }
}
