//! Cached output at a mixer strip or at the end of a prefix of its chain.

use super::*;

/// The cache key for a top-level processor's output, distinct from strip IDs.
pub fn device_target(id: &str) -> String {
    format!("@device/{id}")
}

struct Site {
    key: String,
    strip: String,
    /// Number of devices in the captured prefix, including the instrument on tracks.
    prefix: Option<usize>,
    is_track: bool,
}

fn sites(session: &Session) -> impl Iterator<Item = Site> + '_ {
    let strips = session.tracks.iter().map(|t| {
        (t.id.as_str(), Some(&t.instrument), t.inserts.as_slice())
    }).chain(session.buses.iter().chain(std::iter::once(&session.master)).map(|b| {
        (b.id.as_str(), None, b.inserts.as_slice())
    }));
    strips.flat_map(|(strip, instrument, inserts)| {
        let is_track = instrument.is_some();
        std::iter::once(Site { key: strip.to_owned(), strip: strip.to_owned(), prefix: None, is_track })
            .chain(instrument.into_iter().chain(inserts).enumerate().map(move |(index, d)| Site {
                key: device_target(d.id.as_str()), strip: strip.to_owned(), prefix: Some(index + 1), is_track,
            }))
    })
}

impl Site {
    fn is_physical_track(&self) -> bool { self.is_track && self.prefix.is_none() }

    fn strip_inserts<'a>(&self, session: &'a mut Session) -> &'a mut Vec<Device> {
        if self.is_track {
            &mut session.tracks.iter_mut().find(|t| t.id.as_str() == self.strip).unwrap().inserts
        } else {
            &mut session.buses.iter_mut().chain(std::iter::once(&mut session.master))
                .find(|b| b.id.as_str() == self.strip).unwrap().inserts
        }
    }

    fn project(&self, session: &Session) -> Session {
        let mut pass = session.clone();
        if let Some(prefix) = self.prefix {
            let removed: Vec<_> = self.strip_inserts(&mut pass)
                .split_off(prefix - usize::from(self.is_track)).iter().flat_map(device_ids).collect();
            forget_devices(&mut pass, &removed);
        }
        pass
    }

    fn print(&self, session: &Session, rate: u32) -> Result<String> {
        if self.prefix.is_none() {
            crate::fingerprint::tap(session, &self.strip, rate)
        } else {
            crate::fingerprint::tap(&self.project(session), &self.strip, rate)
        }
    }

    fn stream(&self, session: &mut Session, state: &TrackFreeze) -> Result<()> {
        let record = state.record.as_ref().expect("a fresh freeze has a record");
        if self.is_physical_track() {
            return stream_track(session, &self.strip, &state.audio, record.start_seconds, record.latency_samples);
        }
        let stream = stream_device(&self.strip, &state.audio, record.start_seconds, record.latency_samples)?;
        let inserts = self.strip_inserts(session);
        let count = self.prefix.map_or(inserts.len(), |n| n - usize::from(self.is_track));
        let mut removed: Vec<_> = inserts.drain(..count).flat_map(|d| device_ids(&d)).collect();
        if self.is_track {
            let track = session.tracks.iter_mut().find(|t| t.id.as_str() == self.strip).unwrap();
            removed.extend(device_ids(&track.instrument));
            track.instrument = stream;
            clear_performance(track);
        } else {
            inserts.insert(0, stream);
        }
        forget_devices(session, &removed);
        Ok(())
    }
}

/// Apply cached mixer outputs to a playback description. Keep strip and route identities,
/// but release processors whose paths to the master all end at a cached output.
pub fn apply_graph(session: &mut Session, dir: &Path, rate: u32) -> Result<Vec<TrackFreeze>> {
    let cpu = cpu_records(dir);
    let mut strip_prints = BTreeMap::new();
    let targets: Vec<Site> = sites(session).collect();
    let mut results = Vec::new();
    let mut replacements: BTreeMap<String, (Site, usize)> = BTreeMap::new();
    for site in targets {
        if !audio_path(dir, &site.key).is_file() && !cpu.contains_key(&site.key) {
            continue;
        }
        let strip_print = match strip_prints.entry(site.strip.clone()) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(crate::fingerprint::tap(session, &site.strip, rate)?)
            }
        };
        let freeze_print = if site.prefix.is_some() && audio_path(dir, &site.key).is_file() {
            site.print(session, rate)?
        } else {
            strip_print.clone()
        };
        let state = cache_state(dir, &site.key, rate, &freeze_print, strip_print, &cpu);
        if state.state == FreezeState::Fresh {
            let rank = |s: &Site| s.prefix.unwrap_or(usize::MAX);
            let replace = replacements.get(&site.strip).is_none_or(|(old, _)| rank(&site) > rank(old));
            if replace { replacements.insert(site.strip.clone(), (site, results.len())); }
        }
        results.push(state);
    }
    let mut cut_bus = false;
    for (site, i) in replacements.into_values() {
        cut_bus |= !site.is_track;
        site.stream(session, &results[i])?;
    }
    if cut_bus { prune_behind_master(session)?; }
    Ok(results)
}

fn prune_behind_master(session: &mut Session) -> Result<()> {
    let needed = crate::fingerprint::dependencies(session, &[session.master.id.as_str().to_owned()])?;
    let mut removed = Vec::new();
    for track in &mut session.tracks {
        if needed.tracks.contains(track.id.as_str()) { continue; }
        removed.extend(std::iter::once(&track.instrument).chain(&track.inserts).flat_map(device_ids));
        track.instrument = silent_device(&track.id);
        track.inserts.clear();
        clear_performance(track);
    }
    for bus in &mut session.buses {
        if needed.buses.contains(bus.id.as_str()) { continue; }
        removed.extend(bus.inserts.drain(..).flat_map(|d| device_ids(&d)));
    }
    forget_devices(session, &removed);
    Ok(())
}

/// A zero-input track with no events only needs a transparent, zero-latency processor.
fn silent_device(track: &crate::model::Id) -> Device {
    Device {
        sfz: None, asset_versions: Vec::new(), patch: None, generation: 0, rack: None,
        sample: None, impulse: None, sidechain: None, id: crate::model::Id::new(format!("{track}.silent")),
        kind: DeviceKind::Gain, params: BTreeMap::new(), vst3: None,
        keyswitches: Default::default(), bend_range: None,
    }
}

/// Freeze tracks, buses, the master or top-level processor outputs.
/// Physical tracks keep the batched/partial render path used by `muz freeze`.
pub fn freeze_graph_with(
    session: &Session, dir: &Path, keys: &[String], options: &RenderOptions,
    progress: Option<&crate::control::RenderProgress>,
) -> Result<Frozen> {
    anyhow::ensure!(!keys.is_empty(), "nothing to freeze");
    let available: BTreeMap<_, _> = sites(session).map(|s| (s.key.clone(), s)).collect();
    let requested: Vec<_> = keys.iter().map(|key| available.get(key)
        .with_context(|| format!("unknown freeze target {key}"))).collect::<Result<_>>()?;
    let tracks: Vec<_> = requested.iter().filter(|s| s.is_physical_track()).map(|s| s.key.clone()).collect();
    let mut reports = Vec::new();
    let mut outcomes = Vec::new();
    if !tracks.is_empty() {
        let frozen = freeze_with(session, dir, &tracks, options, progress)?;
        reports.push(frozen.report);
        outcomes.extend(frozen.tracks);
    }
    for site in requested.into_iter().filter(|s| !s.is_physical_track()) {
        std::fs::create_dir_all(dir)?;
        let rate = options.sample_rate;
        let fingerprint = site.print(session, rate)?;
        let taps = vec![site.strip.to_owned()];
        let mut pass = site.project(session);
        if !crate::fingerprint::dependencies(&pass, &taps)?.tracks.is_empty() {
            crate::render::keep_tap_dependencies(&mut pass, &taps)?;
        }
        let options = RenderOptions {
            tap: taps, seconds: None, section: None, start: None, history: None,
            solo: Vec::new(), mute: Vec::new(), format: "float32".into(), ..options.clone()
        };
        let report = crate::render::render_with(pass, &new_audio_path(dir, &site.key), &options, progress)?;
        remove_other_formats(dir, &site.key);
        let record = FreezeRecord {
            track: site.key.clone(), fingerprint, sample_rate: rate, start_seconds: 0.0,
            latency_samples: report.latency_samples, engine: crate::fingerprint::engine_identity(), spans: None,
        };
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut temp, &record)?;
        temp.persist(record_path(dir, &site.key))?;
        outcomes.push(TrackFrozen { track: site.key.clone(), rendered: [0.0, report.seconds], partial: false });
        reports.push(report);
    }
    Ok(Frozen { report: crate::render::combine_tap_reports(reports, dir, options), tracks: outcomes })
}

/// Profile the authored graph, so even a piece that misses live deadlines can
/// show the cost of every strip and processor. Costs are fractions of real time.
pub fn measure_graph(
    session: &Session, dir: &Path, options: &RenderOptions,
    progress: Option<&crate::control::RenderProgress>,
) -> Result<RenderReport> {
    std::fs::create_dir_all(dir)?;
    let scratch = tempfile::Builder::new().suffix(".wav").tempfile_in(dir)?;
    let options = RenderOptions { tap: Vec::new(), ..options.clone() };
    let report = crate::render::profile(session.clone(), scratch.path(), &options, progress)?;
    let rate = options.sample_rate;
    let mut fingerprints = BTreeMap::new();
    let mut costs = BTreeMap::new();
    let mut put = |key: &str, strip: &str, seconds: f64| -> Result<()> {
        let fingerprint = match fingerprints.entry(strip.to_owned()) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(crate::fingerprint::tap(session, strip, rate)?)
            }
        };
        costs.insert(key.to_owned(), CpuRecord {
            fingerprint: fingerprint.clone(), sample_rate: rate, seconds, song_seconds: report.seconds,
        });
        Ok(())
    };
    for (track, seconds) in &report.track_seconds { put(track, track, *seconds)?; }
    let times: BTreeMap<_, _> = report.processor_seconds.iter().map(|(id, seconds)| (id.as_str(), *seconds)).collect();
    for track in &session.tracks {
        for device in std::iter::once(&track.instrument).chain(&track.inserts) {
            let id = device.id.as_str();
            if let Some(seconds) = times.get(id) { put(&device_target(id), track.id.as_str(), *seconds)?; }
        }
    }
    for bus in session.buses.iter().chain(std::iter::once(&session.master)) {
        let strip = bus.id.as_str();
        let mut total = 0.0;
        for device in &bus.inserts {
            let id = device.id.as_str();
            if let Some(seconds) = times.get(id) {
                put(&device_target(id), strip, *seconds)?;
                total += seconds;
            }
        }
        put(strip, strip, total)?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut temp, &costs)?;
    temp.persist(dir.join("cpu.json"))?;
    Ok(report)
}
