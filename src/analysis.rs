use anyhow::{Result, bail};
use std::path::Path;
pub fn analyze(path: &Path) -> Result<serde_json::Value> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let values: Vec<f64> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|v| v.map(|v| v as f64))
            .collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .map(|v| v.map(|v| v as f64 / (1u64 << (spec.bits_per_sample - 1)) as f64))
            .collect::<Result<_, _>>()?,
    };
    if values.iter().any(|v| !v.is_finite()) {
        bail!("audio contains non-finite samples");
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
    Ok(
        serde_json::json!({"seconds":values.len()as f64/spec.channels as f64/spec.sample_rate as f64,"sample_rate":spec.sample_rate,"channels":spec.channels,"sample_peak_dbfs":20.0*peak.max(1e-20).log10(),"rms_dbfs":10.0*(squares/values.len().max(1)as f64).max(1e-40).log10(),"dc":values.iter().sum::<f64>()/values.len().max(1)as f64,"correlation":lr/(ll*rr).sqrt().max(1e-20)}),
    )
}
