use anyhow::Result;
use std::path::Path;
pub fn analyze(path: &Path) -> Result<serde_json::Value> {
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
    Ok(
        serde_json::json!({"integrated_lufs": integrated.is_finite().then_some(integrated), "true_peak_dbtp":20.0*true_peak.max(1e-20).log10(), "loudness_range_lu": range.is_finite().then_some(range), "seconds":values.len()as f64/spec.channels as f64/spec.sample_rate as f64,"sample_rate":spec.sample_rate,"channels":spec.channels,"sample_peak_dbfs":20.0*peak.max(1e-20).log10(),"rms_dbfs":10.0*(squares/values.len().max(1)as f64).max(1e-40).log10(),"dc":values.iter().sum::<f64>()/values.len().max(1)as f64,"correlation":lr/(ll*rr).sqrt().max(1e-20)}),
    )
}
