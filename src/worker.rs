//! Background bounces execute another muz process, isolating plugin faults from playback.
use crate::{
    Session,
    control::RenderProgress,
    midi::ImportedMidi,
    model::TrackSource,
    render::{RenderOptions, RenderReport},
};
use anyhow::{Context, Result, bail};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
#[derive(serde::Serialize, serde::Deserialize)]
struct Input {
    session: Session,
    events: Vec<ImportedMidi>,
    options: RenderOptions,
    output: PathBuf,
}
pub fn run(input: &Path, report: &Path, progress: &Path) -> Result<()> {
    let mut input: Input = serde_json::from_slice(&std::fs::read(input)?)?;
    for (t, events) in input.session.tracks.iter_mut().zip(input.events) {
        if let TrackSource::Midi(m) = &mut t.source {
            m.imported = events;
        }
    }
    let progress = RenderProgress {
        file: Some(progress.to_owned()),
        ..Default::default()
    };
    let result = crate::render::render_with(
        input.session,
        &input.output,
        &input.options,
        Some(&progress),
    )?;
    std::fs::write(report, serde_json::to_vec(&result)?)?;
    Ok(())
}
pub fn bounce(
    session: Session,
    output: PathBuf,
    options: RenderOptions,
    progress: Arc<RenderProgress>,
) -> Result<RenderReport> {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("input.json");
    let report = dir.path().join("report.json");
    let progress_file = dir.path().join("progress.json");
    let log = dir.path().join("worker.log");
    let render_output = dir.path().join("audio.wav");
    let destination = output.clone();
    let events = session
        .tracks
        .iter()
        .map(|t| match &t.source {
            TrackSource::Midi(m) => m.imported.clone(),
            _ => ImportedMidi::default(),
        })
        .collect();
    std::fs::write(
        &input,
        serde_json::to_vec(&Input {
            session,
            events,
            options,
            output: render_output.clone(),
        })?,
    )?;
    let mut child = std::process::Command::new("/proc/self/exe")
        .arg("render-worker")
        .arg(&input)
        .arg(&report)
        .arg(&progress_file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&log)?)
        .spawn()
        .context("start muz render worker")?;
    loop {
        if progress.cancel.load(Ordering::Relaxed) || crate::INTERRUPTED.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("render cancelled");
        }
        if let Ok(data) = std::fs::read(&progress_file) {
            if let Ok([done, total]) = serde_json::from_slice::<[u64; 2]>(&data) {
                progress.processed.store(done, Ordering::Relaxed);
                progress.total.store(total, Ordering::Relaxed);
            }
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                let log = std::fs::read_to_string(log).unwrap_or_default();
                bail!(
                    "render worker exited {status}: {}",
                    log.chars().take(8000).collect::<String>()
                );
            }
            let parent = destination
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            std::fs::create_dir_all(parent)?;
            let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
            std::io::copy(&mut std::fs::File::open(render_output)?, tmp.as_file_mut())?;
            tmp.persist(&destination)?;
            let mut result: RenderReport = serde_json::from_slice(&std::fs::read(report)?)?;
            result.output = destination.display().to_string();
            return Ok(result);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}
