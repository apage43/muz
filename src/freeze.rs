//! Track freezing, composed from general pieces: a render tap captures a
//! track's chain output, [`crate::fingerprint::tap`] names what that output
//! depends on, and a sampler zone placed in song time
//! ([`crate::model::SampleZone::song_seconds`]) plays it back. This module only
//! keeps a cache directory of those renders and rewrites a description to use
//! the fresh ones. The engine knows nothing of freezing.
//!
//! A cache directory holds, per frozen track, `TRACK.wav` (the track's tap
//! from the song start, at the recorded rate) and `TRACK.json` (a
//! [`FreezeRecord`]), plus `cpu.json`: the latest measured chain cost of each
//! track ([`CpuRecord`]).
use crate::model::{Device, DeviceKind, SampleZone, Session, Track, TrackSource};
use crate::render::{RenderOptions, RenderReport};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The cache directory beside a song: `SONG_DIR/.muz-freeze`.
pub fn default_dir(song: &Path) -> PathBuf {
    song.parent().unwrap_or(Path::new(".")).join(".muz-freeze")
}

/// What a cached render captured.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FreezeRecord {
    pub track: String,
    /// [`crate::fingerprint::tap`] of the track when it was rendered.
    pub fingerprint: String,
    pub sample_rate: u32,
    /// Song time of the file's first frame.
    pub start_seconds: f64,
    /// The track chain's latency the stream declares, so compensation and
    /// downstream automation timing stay those of the original chain.
    #[serde(default)]
    pub latency_samples: u64,
    pub engine: String,
}

/// A track's chain cost from the latest render that ran it unfrozen.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CpuRecord {
    pub fingerprint: String,
    pub sample_rate: u32,
    /// Wall-clock seconds its chain took over the whole song.
    pub seconds: f64,
    /// The song's duration in that render, for a real-time fraction.
    pub song_seconds: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreezeState {
    /// The cached render matches the track as it is now.
    Fresh,
    /// A render exists but the track (or an asset, the rate or the engine)
    /// changed since.
    Stale,
    Missing,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackFreeze {
    pub track: String,
    pub state: FreezeState,
    pub audio: PathBuf,
    /// The record of a fresh or stale render.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<FreezeRecord>,
    /// Measured chain cost, when `cpu.json` has one for the current track.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuRecord>,
}

fn file_name(track: &str) -> String {
    track.replace(['/', '\\'], "_")
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn cpu_records(dir: &Path) -> BTreeMap<String, CpuRecord> {
    read_json(&dir.join("cpu.json")).unwrap_or_default()
}

/// Every physical track's freeze state in `dir` at `sample_rate`.
pub fn status(session: &Session, dir: &Path, sample_rate: u32) -> Result<Vec<TrackFreeze>> {
    let cpu = cpu_records(dir);
    session
        .tracks
        .iter()
        .map(|track| {
            let id = track.id.as_str();
            let fingerprint = crate::fingerprint::tap(session, id, sample_rate)?;
            let audio = dir.join(format!("{}.wav", file_name(id)));
            let record: Option<FreezeRecord> =
                read_json(&dir.join(format!("{}.json", file_name(id))));
            let state = match &record {
                _ if !audio.is_file() => FreezeState::Missing,
                Some(r) if r.fingerprint == fingerprint && r.sample_rate == sample_rate => {
                    FreezeState::Fresh
                }
                Some(_) => FreezeState::Stale,
                None => FreezeState::Missing,
            };
            let cpu = cpu
                .get(id)
                .filter(|c| c.fingerprint == fingerprint && c.sample_rate == sample_rate)
                .cloned();
            Ok(TrackFreeze { track: id.to_owned(), state, audio, record, cpu })
        })
        .collect()
}

/// Make `track` play `audio` (a stereo or mono WAV/FLAC whose frame 0 sits
/// at song time `start_seconds`) as its chain output: its instrument becomes
/// a sampler with one song-time zone declaring `latency_samples` (pass a
/// tap's [`RenderReport::latency_samples`] to reproduce its chain exactly),
/// its inserts and their automation are removed, and its performed events
/// are dropped (the tempo map and extent stay). Output route, sends, groups
/// and sidechain readers are unchanged.
pub fn stream_track(
    session: &mut Session,
    track: &str,
    audio: &Path,
    start_seconds: f64,
    latency_samples: u64,
) -> Result<()> {
    let Some(t) = session.tracks.iter_mut().find(|t| t.id.as_str() == track) else {
        bail!("unknown track {track}");
    };
    let audio = std::path::absolute(audio)?;
    let removed: Vec<String> = std::iter::once(&t.instrument)
        .chain(&t.inserts)
        .flat_map(device_ids)
        .collect();
    t.instrument = Device {
        sfz: None,
        asset_versions: Vec::new(),
        patch: None,
        generation: 0,
        rack: None,
        sample: Some(vec![SampleZone {
            path: audio.display().to_string(),
            gain_db: 0.0,
            root: 60.0,
            keys: [0, 127],
            velocity: [0.0, 1.0],
            offset_seconds: 0.0,
            loop_seconds: None,
            one_shot: false,
            song_seconds: Some(start_seconds),
        }]),
        impulse: None,
        sidechain: None,
        id: crate::model::Id::new(format!("{track}.stream")),
        kind: DeviceKind::Sampler,
        params: if latency_samples > 0 {
            BTreeMap::from([("latency_samples".to_owned(), latency_samples as f32)])
        } else {
            BTreeMap::new()
        },
        vst3: None,
        keyswitches: Default::default(),
    };
    t.inserts.clear();
    clear_performance(t);
    let routes: Vec<String> = session
        .tracks
        .iter()
        .flat_map(|t| std::iter::once(&t.output).chain(&t.sends))
        .chain(session.buses.iter().flat_map(|b| b.output.iter().chain(&b.sends)))
        .map(|r| r.id.as_str().to_owned())
        .collect();
    session.extras.automation.retain(|lane| {
        routes.contains(&lane.target)
            || !removed.iter().any(|id| {
            lane.target
                .strip_prefix(id.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
        })
    });
    Ok(())
}

fn clear_performance(t: &mut Track) {
    let TrackSource::Midi(m) = &mut t.source;
    m.imported.notes.clear();
    m.imported.controllers.clear();
    m.imported.messages.clear();
    m.imported.summary.notes = 0;
    m.imported.summary.controllers = 0;
    m.summary = m.imported.summary.clone();
}

fn device_ids(device: &Device) -> Vec<String> {
    let mut ids = vec![device.id.as_str().to_owned()];
    if let Some(rack) = &device.rack {
        ids.extend(rack.branches.iter().flatten().flat_map(device_ids));
    }
    ids
}

/// Stream every track with a fresh render in `dir`; others render normally.
/// Returns every track's state so the caller can report stale ones.
pub fn apply(session: &mut Session, dir: &Path, sample_rate: u32) -> Result<Vec<TrackFreeze>> {
    let states = status(session, dir, sample_rate)?;
    for state in &states {
        if state.state == FreezeState::Fresh {
            let latency = state.record.as_ref().map_or(0, |r| r.latency_samples);
            stream_track(session, &state.track, &state.audio, 0.0, latency)?;
        }
    }
    Ok(states)
}

/// One line per stale freeze, for a caller to warn with.
pub fn stale_warnings(states: &[TrackFreeze]) -> Vec<String> {
    states
        .iter()
        .filter(|s| s.state == FreezeState::Stale)
        .map(|s| {
            format!(
                "freeze of track {} is stale (the track, an asset, the rate or the engine changed); rendering it live — run `muz freeze` or pass --refreeze",
                s.track
            )
        })
        .collect()
}

/// Render `tracks`' taps over the whole song into `dir` in one pass, with
/// every other fresh freeze applied, and record what each captured. Also
/// records the measured chain cost of every track the pass ran unfrozen.
pub fn freeze(session: &Session, dir: &Path, tracks: &[String], options: &RenderOptions) -> Result<RenderReport> {
    if tracks.is_empty() {
        bail!("nothing to freeze");
    }
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    let rate = options.sample_rate;
    let mut fingerprints = BTreeMap::new();
    for t in &session.tracks {
        fingerprints.insert(t.id.as_str().to_owned(), crate::fingerprint::tap(session, t.id.as_str(), rate)?);
    }
    let mut pass = session.clone();
    let states = status(session, dir, rate)?;
    for state in &states {
        if state.state == FreezeState::Fresh && !tracks.contains(&state.track) {
            let latency = state.record.as_ref().map_or(0, |r| r.latency_samples);
            stream_track(&mut pass, &state.track, &state.audio, 0.0, latency)?;
        }
    }
    let streamed: Vec<&str> = states
        .iter()
        .filter(|s| s.state == FreezeState::Fresh && !tracks.contains(&s.track))
        .map(|s| s.track.as_str())
        .collect();
    let options = RenderOptions {
        tap: tracks.to_vec(),
        seconds: None,
        section: None,
        start: None,
        history: None,
        solo: Vec::new(),
        mute: Vec::new(),
        ..options.clone()
    };
    let report = if tracks.len() == 1 {
        let out = dir.join(format!("{}.wav", file_name(&tracks[0])));
        crate::render::render_with(pass, &out, &options, None)?
    } else {
        let files: Vec<String> = tracks.iter().map(|t| file_name(t)).collect();
        if files.iter().zip(tracks).any(|(f, t)| f != t) {
            bail!("track ids with path separators freeze one at a time");
        }
        crate::render::render_taps_report(pass, dir, &options)?
    };
    for track in tracks {
        let latency_samples = if report.taps.is_empty() {
            report.latency_samples
        } else {
            report
                .taps
                .iter()
                .find(|t| t.scope.tap.first() == Some(track))
                .map_or(0, |t| t.latency_samples)
        };
        let record = FreezeRecord {
            track: track.clone(),
            fingerprint: fingerprints[track].clone(),
            sample_rate: rate,
            start_seconds: 0.0,
            latency_samples,
            engine: crate::fingerprint::engine_identity(),
        };
        std::fs::write(
            dir.join(format!("{}.json", file_name(track))),
            serde_json::to_vec_pretty(&record)?,
        )?;
    }
    record_cpu(dir, &report, &fingerprints, &streamed, rate)?;
    Ok(report)
}

/// Render the song once with fresh freezes applied, only to measure the
/// chain cost of every unfrozen track into `cpu.json`.
pub fn measure(session: &Session, dir: &Path, options: &RenderOptions) -> Result<RenderReport> {
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    let rate = options.sample_rate;
    let mut fingerprints = BTreeMap::new();
    for t in &session.tracks {
        fingerprints.insert(t.id.as_str().to_owned(), crate::fingerprint::tap(session, t.id.as_str(), rate)?);
    }
    let mut pass = session.clone();
    let states = apply(&mut pass, dir, rate)?;
    let streamed: Vec<&str> = states
        .iter()
        .filter(|s| s.state == FreezeState::Fresh)
        .map(|s| s.track.as_str())
        .collect();
    let scratch = tempfile::Builder::new().suffix(".wav").tempfile_in(dir)?;
    let options = RenderOptions { tap: Vec::new(), ..options.clone() };
    let report = crate::render::render_with(pass, scratch.path(), &options, None)?;
    record_cpu(dir, &report, &fingerprints, &streamed, rate)?;
    Ok(report)
}

fn record_cpu(
    dir: &Path,
    report: &RenderReport,
    fingerprints: &BTreeMap<String, String>,
    streamed: &[&str],
    rate: u32,
) -> Result<()> {
    let mut cpu = cpu_records(dir);
    for (track, seconds) in &report.track_seconds {
        if streamed.contains(&track.as_str()) {
            continue;
        }
        let Some(fingerprint) = fingerprints.get(track) else { continue };
        cpu.insert(
            track.clone(),
            CpuRecord {
                fingerprint: fingerprint.clone(),
                sample_rate: rate,
                seconds: *seconds,
                song_seconds: report.seconds,
            },
        );
    }
    std::fs::write(dir.join("cpu.json"), serde_json::to_vec_pretty(&cpu)?)?;
    Ok(())
}
