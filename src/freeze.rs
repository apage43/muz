//! Track freezing, composed from general pieces: a render tap captures a
//! track's chain output, [`crate::fingerprint::tap`] names what that output
//! depends on, and a sampler zone placed in song time
//! ([`crate::model::SampleZone::song_seconds`]) plays it back. This module only
//! keeps a cache directory of those renders and rewrites a description to use
//! the fresh ones. The engine knows nothing of freezing.
//!
//! A cache directory holds, per frozen track, its audio (the track's tap from
//! the song start, at the recorded rate) and `TRACK.json` (a
//! [`FreezeRecord`]), plus `cpu.json`: the latest measured chain cost of each
//! track ([`CpuRecord`]). The audio is `TRACK.wv`, lossless WavPack, with the
//! `wavpack` feature and `TRACK.wav` without it; either is read, so a cache
//! written by one build stays usable by the other where the format allows.
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
    /// The track chain's latency the stream declares, so compensation (and
    /// with it the mix's alignment and the route lanes' timing) stays that of
    /// the original chain.
    #[serde(default)]
    pub latency_samples: u64,
    pub engine: String,
    /// The input by time, so a later edit can re-render only where it
    /// changed ([`freeze_with`]). Absent in older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spans: Option<crate::fingerprint::TapSpans>,
}

/// The length of the spans a record's input is digested by.
const SPAN_SECONDS: f64 = 2.0;

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

/// Audio formats a freeze may be stored in, preferred first.
const AUDIO_EXTENSIONS: &[&str] = if cfg!(feature = "wavpack") { &["wv", "wav"] } else { &["wav"] };

/// Where `track`'s freeze audio is: the existing file this build reads, or
/// where a new freeze would be written.
pub fn audio_path(dir: &Path, track: &str) -> PathBuf {
    let name = file_name(track);
    AUDIO_EXTENSIONS
        .iter()
        .map(|e| dir.join(format!("{name}.{e}")))
        .find(|p| p.is_file())
        .unwrap_or_else(|| new_audio_path(dir, track))
}

fn new_audio_path(dir: &Path, track: &str) -> PathBuf {
    dir.join(format!("{}.{}", file_name(track), crate::audio_file::COMPACT_EXTENSION))
}

fn record_path(dir: &Path, track: &str) -> PathBuf {
    dir.join(format!("{}.json", file_name(track)))
}

/// Delete `track`'s freeze (its audio in any format and its record) so it
/// renders live again. Returns whether there was one. Its measured cost stays.
pub fn remove(dir: &Path, track: &str) -> Result<bool> {
    let name = file_name(track);
    let mut removed = false;
    for path in ["wv", "wav", "json"].iter().map(|e| dir.join(format!("{name}.{e}"))) {
        match std::fs::remove_file(&path) {
            Ok(()) => removed = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| path.display().to_string()),
        }
    }
    Ok(removed)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn cpu_records(dir: &Path) -> BTreeMap<String, CpuRecord> {
    read_json(&dir.join("cpu.json")).unwrap_or_default()
}

/// Every physical track's freeze state in `dir` at `sample_rate`.
pub fn status(session: &Session, dir: &Path, sample_rate: u32) -> Result<Vec<TrackFreeze>> {
    Ok(states(session, dir, sample_rate)?.into_iter().map(|(state, _)| state).collect())
}

/// [`status`] with each track's current fingerprint.
fn states(session: &Session, dir: &Path, sample_rate: u32) -> Result<Vec<(TrackFreeze, String)>> {
    let cpu = cpu_records(dir);
    session
        .tracks
        .iter()
        .map(|track| {
            let id = track.id.as_str();
            let fingerprint = crate::fingerprint::tap(session, id, sample_rate)?;
            let audio = audio_path(dir, id);
            let record: Option<FreezeRecord> = read_json(&record_path(dir, id));
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
            Ok((TrackFreeze { track: id.to_owned(), state, audio, record, cpu }, fingerprint))
        })
        .collect()
}

/// Make `track` play `audio` (a stereo or mono recording whose frame 0 sits
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
        bend_range: None,
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

/// Render `tracks`' taps over the whole song into `dir`, with every other
/// fresh freeze applied, and record what each captured. Rendering processes
/// only what the taps read, the tracks and their sidechain sources, in
/// independent passes run at once within `options.threads`
/// ([`crate::render::render_taps_parallel`]). Also records the measured
/// chain cost of every track a pass ran unfrozen.
pub fn freeze(session: &Session, dir: &Path, tracks: &[String], options: &RenderOptions) -> Result<RenderReport> {
    Ok(freeze_with(session, dir, tracks, options, None)?.report)
}

/// How [`freeze_with`] made one track's freeze.
#[derive(Clone, Debug, Serialize)]
pub struct TrackFrozen {
    pub track: String,
    /// Song seconds rendered: the whole song, or the re-rendered span of a
    /// partial refreeze.
    pub rendered: [f64; 2],
    /// Whether the earlier freeze was kept outside `rendered`.
    pub partial: bool,
}

/// A [`freeze_with`] result: the render report (as [`freeze`] returns it)
/// and how each track was made.
#[derive(Debug, Serialize)]
pub struct Frozen {
    pub report: RenderReport,
    pub tracks: Vec<TrackFrozen>,
}

/// [`freeze`], reporting progress (frames of every pass, which run at once,
/// summed into `progress`; its `cancel` stops them) and how each track was
/// made.
pub fn freeze_with(
    session: &Session,
    dir: &Path,
    tracks: &[String],
    options: &RenderOptions,
    progress: Option<&crate::control::RenderProgress>,
) -> Result<Frozen> {
    if tracks.is_empty() {
        bail!("nothing to freeze");
    }
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    let rate = options.sample_rate;
    let (states, fingerprints): (Vec<TrackFreeze>, BTreeMap<String, String>) = states(session, dir, rate)?
        .into_iter()
        .map(|(state, print)| {
            let track = state.track.clone();
            (state, (track, print))
        })
        .unzip();
    let mut pass = session.clone();
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
    let spans: BTreeMap<&String, crate::fingerprint::TapSpans> = tracks
        .iter()
        .map(|t| Ok((t, crate::fingerprint::tap_spans(session, t, rate, SPAN_SECONDS)?)))
        .collect::<Result<_>>()?;
    let record = |track: &String, latency_samples: u64| FreezeRecord {
        track: track.clone(),
        fingerprint: fingerprints[track].clone(),
        sample_rate: rate,
        start_seconds: 0.0,
        latency_samples,
        engine: crate::fingerprint::engine_identity(),
        spans: Some(spans[track].clone()),
    };
    // A stale freeze whose structure is unchanged re-renders only around the
    // spans whose events changed, where that converges.
    let candidates: Vec<(&String, PathBuf, u64, [f64; 2])> = tracks
        .iter()
        .filter_map(|track| {
            let state = states.iter().find(|s| &s.track == track)?;
            let old = state.record.as_ref().filter(|_| state.state == FreezeState::Stale)?;
            let changed = old.spans.as_ref()?.changed(&spans[track])?;
            (old.sample_rate == rate && old.start_seconds == 0.0)
                .then(|| (track, state.audio.clone(), old.latency_samples, changed))
        })
        .collect();
    let mut made = Vec::new();
    if !candidates.is_empty() {
        // Each candidate's partial attempt races a full render of it, which
        // a converged attempt cancels: a failed attempt costs no time.
        let share = RenderOptions { threads: (options.threads / (2 * candidates.len())).max(1), ..options.clone() };
        let results = parallel(&candidates, options.threads, |(track, audio, latency, changed)| {
            refreeze(&pass, dir, track, audio, *latency, *changed, &share)
        });
        for ((track, ..), result) in candidates.iter().zip(results) {
            let (outcome, report) = result?;
            std::fs::write(record_path(dir, track), serde_json::to_vec_pretty(&record(track, report.latency_samples))?)?;
            made.push((outcome, report));
        }
    }
    let full: Vec<String> = tracks.iter().filter(|t| !made.iter().any(|(m, _)| &m.track == *t)).cloned().collect();
    let mut taps = Vec::new();
    let mut pass_costs = (Vec::new(), 0.0);
    if !full.is_empty() {
        let options = RenderOptions { tap: full.clone(), ..options.clone() };
        let paths: Vec<PathBuf> = full.iter().map(|t| new_audio_path(dir, t)).collect();
        // Independent tracks render in separate passes at once, each
        // processing only what its taps read: the tracks and their sidechain
        // sources (streamed where those are frozen), nothing downstream.
        taps = crate::render::render_taps_parallel(&pass, &paths, &options, progress)?;
        for (track, tap) in full.iter().zip(&taps) {
            remove_other_formats(dir, track);
            std::fs::write(record_path(dir, track), serde_json::to_vec_pretty(&record(track, tap.latency_samples))?)?;
        }
        // Each pass's reports carry that pass's costs; the passes share no track.
        let mut costs: Vec<(String, f64)> = Vec::new();
        let mut cpu_seconds = 0.0;
        for tap in &mut taps {
            let mut seen = false;
            for (track, seconds) in std::mem::take(&mut tap.track_seconds) {
                if !costs.iter().any(|(t, _)| *t == track) {
                    costs.push((track, seconds));
                    seen = true;
                }
            }
            if seen {
                cpu_seconds += tap.cpu_seconds;
            }
        }
        pass_costs = (costs, cpu_seconds);
    }
    let song = taps.first().or(made.first().map(|(_, r)| r)).map_or(0.0, |r| r.seconds);
    let mut outcomes: Vec<TrackFrozen> = full
        .iter()
        .map(|t| TrackFrozen { track: t.clone(), rendered: [0.0, song], partial: false })
        .collect();
    for (outcome, report) in made {
        outcomes.push(outcome);
        taps.push(report);
    }
    // In the order asked for.
    let order = |t: &str| tracks.iter().position(|x| x == t);
    let mut paired: Vec<(TrackFrozen, RenderReport)> = outcomes.into_iter().zip(taps).collect();
    paired.sort_by_key(|(o, _)| order(&o.track));
    let (outcomes, mut taps): (Vec<_>, Vec<_>) = paired.into_iter().unzip();
    (taps[0].track_seconds, taps[0].cpu_seconds) = pass_costs;
    let report = crate::render::combine_tap_reports(taps, dir, &options);
    record_cpu(dir, &report, &fingerprints, &streamed, rate)?;
    Ok(Frozen { report, tracks: outcomes })
}

/// A freeze in the other format would shadow or outlive a new one.
fn remove_other_formats(dir: &Path, track: &str) {
    let fresh = new_audio_path(dir, track);
    for e in AUDIO_EXTENSIONS {
        let other = dir.join(format!("{}.{e}", file_name(track)));
        if other != fresh {
            let _ = std::fs::remove_file(other);
        }
    }
}

/// `f` over `items` on up to `workers` threads, inside the caller's host
/// context; results in order.
fn parallel<T: Sync, R: Send>(items: &[T], workers: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = workers.clamp(1, items.len().max(1));
    let context = crate::host::current().unwrap_or_else(crate::host::HostContext::cli);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<R>>> = items.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let work = || {
        context.run(|| loop {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(item) = items.get(i) else { break };
            *results[i].lock().unwrap() = Some(f(item));
        })
    };
    if workers == 1 {
        work();
    } else {
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(work);
            }
        });
    }
    results.into_iter().map(|r| r.into_inner().unwrap().expect("every item ran")).collect()
}

/// Refreeze a stale `track` whose structure is unchanged: a partial attempt
/// ([`partial_refreeze`]) and a full render of it run at once, and the first
/// to finish validly wins, the partial one cancelling the full one.
fn refreeze(
    pass: &Session,
    dir: &Path,
    track: &str,
    audio: &Path,
    latency: u64,
    changed: [f64; 2],
    options: &RenderOptions,
) -> Result<(TrackFrozen, RenderReport)> {
    let full_render = crate::control::RenderProgress {
        file: None,
        processed: Default::default(),
        total: Default::default(),
        cancel: Default::default(),
    };
    let scratch = tempfile::Builder::new()
        .suffix(&format!(".{}", crate::audio_file::COMPACT_EXTENSION))
        .tempfile_in(dir)?
        .into_temp_path();
    let context = crate::host::current().unwrap_or_else(crate::host::HostContext::cli);
    let (partial, full) = std::thread::scope(|scope| {
        let full = scope.spawn(|| {
            context.run(|| -> Result<RenderReport> {
                let mut session = pass.clone();
                crate::render::keep_tap_dependencies(&mut session, &[track.to_owned()])?;
                let options = RenderOptions { tap: vec![track.to_owned()], ..options.clone() };
                let paths = [scratch.to_path_buf()];
                Ok(crate::render::render_taps_to(session, &paths, &options, Some(&full_render))?.remove(0))
            })
        });
        let partial = context.run(|| partial_refreeze(pass, dir, track, audio, latency, changed, options));
        if matches!(partial, Ok(Some(_))) {
            full_render.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        (partial, full.join().expect("the full render does not panic"))
    });
    if let Some((rendered, mut report)) = partial? {
        report.latency_samples = latency;
        return Ok((TrackFrozen { track: track.to_owned(), rendered, partial: true }, report));
    }
    let mut report = full?;
    let path = new_audio_path(dir, track);
    scratch.persist(&path)?;
    remove_other_formats(dir, track);
    report.output = path.display().to_string();
    let song = report.seconds;
    Ok((TrackFrozen { track: track.to_owned(), rendered: [0.0, song], partial: false }, report))
}

/// Seconds of identical output, with at least `AUDIBLE` of it not silent,
/// that count as the old and new renders having converged.
const MATCH_SECONDS: f64 = 1.0;
const AUDIBLE_SECONDS: f64 = 0.1;
/// Pre-rolls tried before the first changed span, in seconds. Each try
/// prepares the chain again, which for a large sampled instrument costs more
/// than rendering a minute of it, so they are few and long.
const PREROLLS: [f64; 2] = [20.0, 60.0];

/// Re-render `track` from a pre-roll before `changed` (song seconds) with
/// bounded history, and splice it into its old freeze at `audio`: the
/// pre-roll's output must match the old file exactly (a null test over at
/// least [`MATCH_SECONDS`], some of it audible) before the change starts,
/// longer pre-rolls are tried when it does not, and after the change the
/// render runs until it matches the old file again (or to the end). A
/// pre-roll that would reach the song start renders from there with full
/// history, which needs no test. Returns the rendered song seconds and a
/// report of the new file, or `None` when no pre-roll converges (a plugin
/// whose output varies between renders never does) or the render covered
/// the whole song, and the track needs a full render.
fn partial_refreeze(
    pass: &Session,
    dir: &Path,
    track: &str,
    audio: &Path,
    latency: u64,
    changed: [f64; 2],
    options: &RenderOptions,
) -> Result<Option<([f64; 2], RenderReport)>> {
    let rate = options.sample_rate;
    let block = options.block_size.max(1) as u64;
    let (_, frames) = crate::audio_file::load(audio, usize::MAX)?;
    let old: Vec<f32> = frames.into_iter().flatten().collect();
    let total = (old.len() / 2) as u64;
    let at = |seconds: f64| ((seconds * rate as f64) as u64).min(total);
    let (from, until) = (at(changed[0]), at(changed[1].ceil()).max(at(changed[0])));
    let need = (MATCH_SECONDS * rate as f64) as u64;
    let audible = (AUDIBLE_SECONDS * rate as f64) as u64;
    let mut session = pass.clone();
    crate::render::keep_tap_dependencies(&mut session, &[track.to_owned()])?;
    for preroll in PREROLLS {
        let start = from.saturating_sub((preroll * rate as f64) as u64) / block * block;
        // From the song start the render is exact: no pre-roll to test.
        let exact = start == 0;
        let options = RenderOptions {
            tap: vec![track.to_owned()],
            start: (!exact).then(|| start as f64 / rate as f64),
            history: (!exact).then_some(0.0),
            ..options.clone()
        };
        // Frames since the last difference, and how many of them sound.
        let (mut run, mut sounding) = (0u64, 0u64);
        let mut converged_before = from == 0;
        let mut rendered: Vec<f32> = Vec::new();
        let mut end = None;
        let rendered_latency = crate::render::render_tap_blocks(session.clone(), &options, |first, block| {
            for (i, pair) in block.chunks_exact(2).enumerate() {
                let frame = start + first + i as u64;
                if frame >= total {
                    return Ok(false);
                }
                if frame == from {
                    if !exact && (run < need || sounding < audible) {
                        return Ok(false);
                    }
                    converged_before = true;
                }
                if frame >= from {
                    rendered.extend_from_slice(pair);
                }
                let old = &old[frame as usize * 2..frame as usize * 2 + 2];
                if old[0].to_bits() == pair[0].to_bits() && old[1].to_bits() == pair[1].to_bits() {
                    run += 1;
                    sounding += (pair[0] != 0.0 || pair[1] != 0.0) as u64;
                } else {
                    (run, sounding) = (0, 0);
                }
                if frame >= until && run >= need && sounding >= audible {
                    end = Some(frame + 1);
                    return Ok(false);
                }
            }
            Ok(true)
        })?;
        if !converged_before {
            if exact {
                return Ok(None);
            }
            continue;
        }
        anyhow::ensure!(rendered_latency == latency, "the chain's latency changed");
        let end = end.unwrap_or(from + (rendered.len() / 2) as u64);
        anyhow::ensure!(end == from + (rendered.len() / 2) as u64, "partial render ended early");
        if end < total && end < until {
            bail!("partial render of {track} stopped before its change");
        }
        if exact && end == total {
            return Ok(None);
        }
        let mut samples = old;
        samples.splice(from as usize * 2..end as usize * 2, rendered);
        anyhow::ensure!(samples.len() as u64 == total * 2, "partial render changed the length");
        let report = write_freeze(dir, track, &samples, rate)?;
        return Ok(Some(([start as f64 / rate as f64, end as f64 / rate as f64], report)));
    }
    Ok(None)
}

/// Write a freeze's audio (interleaved stereo float) and report it like a
/// tap render.
fn write_freeze(dir: &Path, track: &str, samples: &[f32], rate: u32) -> Result<RenderReport> {
    let path = new_audio_path(dir, track);
    let frames = (samples.len() / 2) as u64;
    let tmp = tempfile::Builder::new().suffix(&format!(".{}", crate::audio_file::COMPACT_EXTENSION)).tempfile_in(dir)?;
    let spec = crate::audio_file::WriteSpec { channels: 2, rate, bits: 32, float: true, frames };
    let mut writer = crate::audio_file::AudioWriter::new(&path, tmp.reopen()?, spec)?;
    let (mut peak, mut square) = (0.0f64, 0.0f64);
    for &x in samples {
        peak = peak.max(x.abs() as f64);
        square += (x as f64).powi(2);
        writer.write_f32(x)?;
    }
    writer.finish()?;
    tmp.persist(&path)?;
    remove_other_formats(dir, track);
    let mut scope = RenderOptions::default();
    scope.tap = vec![track.to_owned()];
    scope.sample_rate = rate;
    Ok(RenderReport {
        source: None,
        revision: None,
        scope,
        track_meters: Vec::new(),
        track_seconds: Vec::new(),
        cpu_seconds: 0.0,
        latency_samples: 0,
        frames,
        sample_rate: rate,
        seconds: frames as f64 / rate as f64,
        sample_peak_dbfs: 20.0 * peak.max(1e-20).log10(),
        rms_dbfs: 10.0 * (square / samples.len().max(1) as f64).max(1e-40).log10(),
        output: path.display().to_string(),
        taps: Vec::new(),
    })
}

/// One freeze [`compact`] re-encoded.
#[derive(Clone, Debug, Serialize)]
pub struct Compacted {
    pub track: String,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

/// Re-encode every WAV freeze in `dir` (one with a record) as lossless
/// WavPack, without rendering: each `.wv` replaces its `.wav` only after it
/// decodes to the same samples bit for bit. Records are untouched, so each
/// freeze stays exactly as fresh or stale as it was. Needs the `wavpack`
/// feature.
pub fn compact(dir: &Path) -> Result<Vec<Compacted>> {
    if !cfg!(feature = "wavpack") {
        bail!("compacting freezes needs the `wavpack` feature");
    }
    let mut done = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| dir.display().to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    entries.sort();
    for json in entries {
        let Some(record) = read_json::<FreezeRecord>(&json) else { continue };
        let wav = json.with_extension("wav");
        if !wav.is_file() || json != record_path(dir, &record.track) {
            continue;
        }
        let (before_bytes, after_bytes) = crate::audio_file::reencode(&wav, &json.with_extension("wv"))?;
        std::fs::remove_file(&wav)?;
        done.push(Compacted { track: record.track, before_bytes, after_bytes });
    }
    Ok(done)
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
