//! Background bounces execute another muz process, isolating plugin faults from playback.
use crate::{
    Session,
    control::RenderProgress,
    render::{RenderOptions, RenderReport},
    snapshot::PlayableSnapshotV1,
};
use anyhow::{Context, Result, bail};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
#[derive(serde::Serialize, serde::Deserialize)]
struct Input {
    snapshot: PlayableSnapshotV1,
    options: RenderOptions,
    output: PathBuf,
}
pub fn run(input: &Path, report: &Path, progress: &Path) -> Result<()> {
    anyhow::ensure!(
        std::fs::metadata(input)?.len() <= crate::snapshot::MAX_SNAPSHOT_BYTES as u64,
        "worker input exceeds byte limit"
    );
    let input: Input = serde_json::from_slice(&std::fs::read(input)?)?;
    let session = input.snapshot.restore_checked()?;
    let progress = RenderProgress {
        file: Some(progress.to_owned()),
        ..Default::default()
    };
    let result = if input.options.tap.len() > 1 {
        crate::render::render_taps_report(session, &input.output, &input.options)?
    } else {
        crate::render::render_with(session, &input.output, &input.options, Some(&progress))?
    };
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
    // Several taps write a directory of TAP.wav files: OUT.wav becomes OUT/.
    let several = options.tap.len() > 1;
    let render_output = dir.path().join(if several { "audio" } else { "audio.wav" });
    let destination = if several {
        output.with_extension("")
    } else {
        output.clone()
    };
    let snapshot = PlayableSnapshotV1::capture(&session)?;
    std::fs::write(
        &input,
        serde_json::to_vec(&Input {
            snapshot,
            options,
            output: render_output.clone(),
        })?,
    )?;
    let executable = std::env::current_exe().context("locate muz executable")?;
    let mut child = std::process::Command::new(executable)
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
        if let Ok(data) = std::fs::read(&progress_file)
            && let Ok([done, total]) = serde_json::from_slice::<[u64; 2]>(&data)
        {
            progress.processed.store(done, Ordering::Relaxed);
            progress.total.store(total, Ordering::Relaxed);
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                let log = std::fs::read_to_string(log).unwrap_or_default();
                bail!(
                    "render worker exited {status}: {}",
                    log.chars().take(8000).collect::<String>()
                );
            }
            let mut result: RenderReport = serde_json::from_slice(&std::fs::read(report)?)?;
            if several {
                std::fs::create_dir_all(&destination)?;
                for tap in &mut result.taps {
                    let file = Path::new(&tap.output)
                        .file_name()
                        .context("tap report names its file")?
                        .to_owned();
                    let target = destination.join(&file);
                    persist_copy(&render_output.join(&file), &target)?;
                    tap.output = target.display().to_string();
                }
            } else {
                persist_copy(&render_output, &destination)?;
            }
            result.output = destination.display().to_string();
            return Ok(result);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Copy a finished file next to its destination, then rename it into place.
fn persist_copy(from: &Path, to: &Path) -> Result<()> {
    let parent = to
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut std::fs::File::open(from)?, tmp.as_file_mut())?;
    tmp.persist(to)?;
    Ok(())
}
