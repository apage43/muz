//! Output fingerprints: a stable digest of everything that determines one
//! render tap's audio, for caching renders of it.
//!
//! [`tap`] walks the graph upstream of a tap (the names `muz render --tap`
//! accepts) and hashes only what can reach it: each feeding track's performed
//! events on its channel, its instrument and insert descriptions and controls,
//! their automation lanes, sidechain sources, routes and their levels into a
//! bus, the tempo and meter maps, the versions of every asset those devices
//! read, the sample rate and the engine build. Anything else (another
//! track's notes, a fader downstream of the tap, mute, solo, names) leaves it
//! unchanged. Two equal fingerprints mean the same build renders the same
//! tap audio; a renderer that is not block-size independent would need the
//! block size too, which no fingerprint includes.
use crate::model::{Device, Route, Session, Track, TrackSource};
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The engine build output fingerprints include: the package version and the
/// source commit (`-dirty` when built from an edited tree).
pub fn engine_identity() -> String {
    format!(
        "{}+{}",
        env!("CARGO_PKG_VERSION"),
        option_env!("MUZ_BUILD_COMMIT").unwrap_or("unknown")
    )
}

/// Hex SHA-256 of what determines `tap`'s audio at `sample_rate`. `tap` is
/// a track (its pre-fader chain output), a bus, the master, a kit or
/// expanded group (`NAME`, its members' sum), each optionally with `:post`.
pub fn tap(session: &Session, tap: &str, sample_rate: u32) -> Result<String> {
    let mut graph = Upstream::new(session);
    graph.tap(tap)?;
    digest_graph(session, &graph, tap, sample_rate, false)
}

/// Digest the graph upstream of a tap. Without `events`, every chain's
/// performed notes, controllers and messages are left out (their tempo map
/// and extent stay): what [`TapSpans::structure`] names.
fn digest_graph(session: &Session, graph: &Upstream, tap: &str, sample_rate: u32, structure: bool) -> Result<String> {
    let mut digest = crate::snapshot::Digest::new();
    let mut add = |value: &dyn erased::Hashable| value.add_to(&mut digest);
    if structure {
        add(&("muz structure fingerprint", 1u32, engine_identity(), sample_rate, tap))?;
    } else {
        add(&("muz output fingerprint", 1u32, engine_identity(), sample_rate, tap))?;
    }
    add(&session.transport)?;
    // The tempo timeline compiles from every track's map and extent.
    for t in &session.tracks {
        let TrackSource::Midi(m) = &t.source;
        add(&(&m.imported.tempos, m.summary.end_tick, m.summary.ppq))?;
    }
    for (key, item) in &graph.items {
        add(key)?;
        match item {
            Item::Chain { source, events, idle_after, devices } if structure => {
                let events = crate::midi::ImportedMidi {
                    summary: events.summary.clone(),
                    tempos: events.tempos.clone(),
                    ..Default::default()
                };
                add(&Item::Chain { source: *source, events, idle_after: *idle_after, devices: devices.clone() })?;
            }
            item => add(item)?,
        }
    }
    let lanes: Vec<_> = session
        .extras
        .automation
        .iter()
        .filter(|lane| {
            graph.routes.contains(lane.target.as_str())
                || lane
                    .target
                    .rsplit_once('.')
                    .is_some_and(|(device, _)| graph.devices.contains(device))
        })
        .collect();
    add(&lanes)?;
    let resolver = crate::assets::resolver();
    let mut versions = BTreeMap::new();
    for device in graph.device_descriptions() {
        for path in crate::assets::paths(device) {
            let version = resolver.version(&path).map_err(|e| {
                anyhow::anyhow!("asset {} of device {}: {e:#}", path.display(), device.id)
            })?;
            versions.insert(path, version);
        }
    }
    add(&versions)?;
    Ok(hex(&digest.finish()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A tap's fingerprint split by time, for finding where two versions of a
/// tap's input differ: `structure` digests everything [`tap`] does except
/// the performed events (devices, controls, automation, routes, assets, the
/// tempo map and extent, rate and engine); `digests[i]` digests the events
/// of every chain reaching the tap that start in song seconds
/// `[i, i + 1) * span_seconds`, and `ends[i]` is the latest end of those
/// (a note's release). Equal structures and digests mean equal fingerprints.
/// Event order within a span is kept; note ids and source order, which only
/// name and order the source, are not digested, so an edit leaves later
/// spans' digests alone.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct TapSpans {
    pub structure: String,
    pub span_seconds: f64,
    pub digests: Vec<String>,
    pub ends: Vec<f64>,
}

impl TapSpans {
    /// The song seconds where the events of `self` and `other` (with equal
    /// structures) differ: from the start of the first differing span to the
    /// latest of its end and any differing span's notes' ends. `None` when
    /// they are the same or not comparable.
    pub fn changed(&self, other: &TapSpans) -> Option<[f64; 2]> {
        if self.structure != other.structure || self.span_seconds != other.span_seconds {
            return None;
        }
        let n = self.digests.len().max(other.digests.len());
        let (mut first, mut last) = (None, 0.0f64);
        for i in 0..n {
            let (a, b) = (self.digests.get(i), other.digests.get(i));
            if a == b {
                continue;
            }
            first.get_or_insert(i);
            let end = |s: &TapSpans| s.ends.get(i).copied().unwrap_or(0.0);
            last = last.max((i + 1) as f64 * self.span_seconds).max(end(self)).max(end(other));
        }
        first.map(|i| [i as f64 * self.span_seconds, last])
    }
}

/// [`TapSpans`] of `tap` at `sample_rate`, with spans of `span_seconds`.
pub fn tap_spans(session: &Session, tap: &str, sample_rate: u32, span_seconds: f64) -> Result<TapSpans> {
    anyhow::ensure!(span_seconds > 0.0 && span_seconds.is_finite(), "span length must be positive");
    let mut graph = Upstream::new(session);
    graph.tap(tap)?;
    let structure = digest_graph(session, &graph, tap, sample_rate, true)?;
    let Some(TrackSource::Midi(timeline)) = session.tracks.first().map(|t| &t.source) else {
        bail!("a tap needs at least one track");
    };
    let tempos = &timeline.imported.tempos;
    let seconds = |tick: u64, ppq: u32| crate::compile::seconds_at(tick as f64 / ppq.max(1) as f64, tempos);
    #[derive(Serialize)]
    enum Event<'a> {
        Note {
            start_tick: u64,
            duration_ticks: u64,
            channel: u8,
            key: u8,
            attack_velocity: u8,
            release_velocity: u8,
            performance: &'a Option<crate::expression::Performance>,
            tags: &'a [String],
            annotations: &'a BTreeMap<String, serde_json::Value>,
        },
        Controller { tick: u64, channel: u8, controller: u8, value: u8 },
        Message { tick: u64, bytes: [u8; 3], len: u8 },
    }
    let mut spans: BTreeMap<usize, (crate::snapshot::Digest, f64)> = BTreeMap::new();
    let mut add = |chain: &str, at: f64, end: f64, event: Event| -> Result<()> {
        let index = (at.max(0.0) / span_seconds) as usize;
        let (digest, latest) = spans.entry(index).or_insert_with(|| (crate::snapshot::Digest::new(), 0.0));
        *latest = latest.max(end);
        erased::Hashable::add_to(&(chain, event), digest)
    };
    for (key, item) in &graph.items {
        let Item::Chain { events, .. } = item else { continue };
        let ppq = events.summary.ppq;
        for n in &events.notes {
            let at = seconds(n.start_tick, ppq);
            let end = seconds(n.start_tick + n.duration_ticks, ppq);
            add(key, at, end, Event::Note {
                start_tick: n.start_tick,
                duration_ticks: n.duration_ticks,
                channel: n.channel,
                key: n.key,
                attack_velocity: n.attack_velocity,
                release_velocity: n.release_velocity,
                performance: &n.performance,
                tags: &n.tags,
                annotations: &n.annotations,
            })?;
        }
        for c in &events.controllers {
            let at = seconds(c.tick, ppq);
            add(key, at, at, Event::Controller { tick: c.tick, channel: c.channel, controller: c.controller, value: c.value })?;
        }
        for m in &events.messages {
            let at = seconds(m.tick, ppq);
            add(key, at, at, Event::Message { tick: m.tick, bytes: m.bytes, len: m.len })?;
        }
    }
    let count = spans.keys().next_back().map_or(0, |&i| i + 1);
    let mut digests = vec![String::new(); count];
    let mut ends = vec![0.0; count];
    for (i, (digest, end)) in spans {
        digests[i] = hex(&digest.finish());
        ends[i] = end;
    }
    Ok(TapSpans { structure, span_seconds, digests, ends })
}

/// The tracks and buses (the master included) whose processing can reach a
/// tap's audio, by id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dependencies {
    pub tracks: BTreeSet<String>,
    pub buses: BTreeSet<String>,
}

/// What can reach any of `taps`, by the same walk [`tap`] digests: a track
/// tap needs its own chain and, recursively, its sidechain sources; a bus
/// tap also every track and bus routed into it. A probe tap `TRACK@PROBE`
/// needs `TRACK`'s.
pub fn dependencies(session: &Session, taps: &[String]) -> Result<Dependencies> {
    let mut graph = Upstream::new(session);
    for tap in taps {
        let base = tap.strip_suffix(":post").unwrap_or(tap);
        match base.rsplit_once('@') {
            Some((track, _)) if !session.tracks.iter().any(|t| t.id.as_str() == base) => {
                graph.tap(track)?
            }
            _ => graph.tap(tap)?,
        }
    }
    let mut out = Dependencies::default();
    for key in graph.items.keys() {
        if let Some(id) = key.strip_prefix("track ") {
            out.tracks.insert(id.to_owned());
        } else if let Some(id) = key.strip_prefix("bus ") {
            out.buses.insert(id.to_owned());
        }
    }
    Ok(out)
}

/// Physical tracks of a kit or expanded group, from its recorded members or
/// the `NAME.*` naming convention.
fn group_members(session: &Session, group: &str) -> Vec<String> {
    if let Some(g) = session.extras.track_groups.iter().find(|g| g.id == group) {
        return g.members.iter().map(|m| m.track.clone()).collect();
    }
    let prefix = format!("{group}.");
    session
        .tracks
        .iter()
        .filter(|t| t.id.as_str().starts_with(&prefix))
        .map(|t| t.id.as_str().to_owned())
        .collect()
}

#[derive(Serialize)]
enum Item<'a> {
    /// A track's chain: its channel's events and its devices.
    Chain {
        source: (&'a str, u8, bool),
        events: crate::midi::ImportedMidi,
        idle_after: Option<f64>,
        devices: Vec<Device>,
    },
    Route(&'a Route),
    BusChain(Vec<Device>),
}

/// What can reach a tap, keyed for an order-independent digest.
struct Upstream<'a> {
    session: &'a Session,
    items: BTreeMap<String, Item<'a>>,
    devices: BTreeSet<String>,
    routes: BTreeSet<String>,
}

impl<'a> Upstream<'a> {
    fn new(session: &'a Session) -> Self {
        Self { session, items: BTreeMap::new(), devices: BTreeSet::new(), routes: BTreeSet::new() }
    }

    /// Walk upstream of one tap name.
    fn tap(&mut self, tap: &str) -> Result<()> {
        let session = self.session;
        let (base, post) = match tap.strip_suffix(":post") {
            Some(base) => (base, true),
            None => (tap, false),
        };
        if session.tracks.iter().any(|t| t.id.as_str() == base) {
            self.track(base, post);
        } else if base == session.master.id.as_str() {
            self.bus(base, false);
        } else if session.buses.iter().any(|b| b.id.as_str() == base) {
            self.bus(base, post);
        } else {
            let members = group_members(session, base);
            if members.is_empty() {
                bail!("unknown tap {tap}");
            }
            for member in members {
                self.track(&member, post);
            }
        }
        Ok(())
    }

    fn device_descriptions(&self) -> impl Iterator<Item = &Device> {
        self.items.values().flat_map(|item| match item {
            Item::Chain { devices, .. } | Item::BusChain(devices) => devices.iter(),
            Item::Route(_) => [].iter(),
        })
    }

    fn route(&mut self, route: &'a Route) {
        self.routes.insert(route.id.as_str().to_owned());
        self.items.insert(format!("route {}", route.id), Item::Route(route));
    }

    /// A post-fader send also reads its owner's output fader.
    fn send(&mut self, route: &'a Route, output: Option<&'a Route>) {
        self.route(route);
        if !route.pre
            && let Some(output) = output
            && output.id != route.id
        {
            self.route(output);
        }
    }

    fn devices(&mut self, chain: impl Iterator<Item = &'a Device>) -> Vec<Device> {
        let mut out = Vec::new();
        for device in chain {
            for id in device_ids(device) {
                self.devices.insert(id);
            }
            for source in sidechains(device) {
                self.track(&source, false);
            }
            let mut device = device.clone();
            // A plugin restart stamp is runtime state, not authored input.
            clear_generation(&mut device);
            // Keyswitch names describe notes; nothing they name reaches audio.
            device.keyswitches.clear();
            // So does a declared bend range: the device receives the same bends.
            device.bend_range = None;
            out.push(device);
        }
        out
    }

    fn track(&mut self, id: &str, post: bool) {
        let Some(track) = self.session.tracks.iter().find(|t| t.id.as_str() == id) else {
            return;
        };
        let key = format!("track {id}");
        if !self.items.contains_key(&key) {
            // Reserve first: sidechain cycles are rejected elsewhere, but must
            // not recurse forever here.
            self.items.insert(key.clone(), Item::BusChain(Vec::new()));
            let item = self.chain(track);
            self.items.insert(key, item);
        }
        if post {
            self.route(&track.output);
        }
    }

    fn chain(&mut self, track: &'a Track) -> Item<'a> {
        let TrackSource::Midi(source) = &track.source;
        let mut events = source.imported.clone();
        if !source.all_channels {
            events.notes.retain(|n| n.channel == source.channel);
            events.controllers.retain(|e| e.channel == source.channel);
            events.messages.retain(|e| e.bytes[0] & 15 == source.channel);
        }
        // Whole-asset totals would tie a kit voice to its siblings' edits.
        events.summary = crate::midi::MidiSummary {
            ppq: events.summary.ppq,
            end_tick: events.summary.end_tick,
            ..Default::default()
        };
        let devices = self.devices(std::iter::once(&track.instrument).chain(&track.inserts));
        Item::Chain {
            source: (source.asset.as_str(), source.channel, source.all_channels),
            events, idle_after: track.idle_after, devices }
    }

    fn bus(&mut self, id: &str, post: bool) {
        let session = self.session;
        let Some(bus) = std::iter::once(&session.master)
            .chain(&session.buses)
            .find(|b| b.id.as_str() == id)
        else {
            return;
        };
        let key = format!("bus {id}");
        if !self.items.contains_key(&key) {
            self.items.insert(key.clone(), Item::BusChain(Vec::new()));
            let devices = self.devices(bus.inserts.iter());
            self.items.insert(key, Item::BusChain(devices));
            for track in &session.tracks {
                for route in std::iter::once(&track.output).chain(&track.sends) {
                    if route.to == bus.id {
                        self.track(track.id.as_str(), false);
                        self.send(route, Some(&track.output));
                    }
                }
            }
            for source in &session.buses {
                for route in source.output.iter().chain(&source.sends) {
                    if route.to == bus.id {
                        self.bus(source.id.as_str(), false);
                        self.send(route, source.output.as_ref());
                    }
                }
            }
        }
        if post && let Some(output) = &bus.output {
            self.route(output);
        }
    }
}

fn device_ids(device: &Device) -> Vec<String> {
    let mut ids = vec![device.id.as_str().to_owned()];
    if let Some(rack) = &device.rack {
        ids.extend(rack.branches.iter().flatten().flat_map(device_ids));
    }
    ids
}

fn sidechains(device: &Device) -> Vec<String> {
    let mut out: Vec<String> = device.sidechain.iter().cloned().collect();
    if let Some(rack) = &device.rack {
        out.extend(rack.branches.iter().flatten().flat_map(sidechains));
    }
    out
}

fn clear_generation(device: &mut Device) {
    device.generation = 0;
    if let Some(rack) = &mut device.rack {
        rack.branches.iter_mut().flatten().for_each(clear_generation);
    }
}

mod erased {
    /// Object-safe adapter so one closure can digest values of any type.
    pub trait Hashable {
        fn add_to(&self, digest: &mut crate::snapshot::Digest) -> anyhow::Result<()>;
    }
    impl<T: serde::Serialize + ?Sized> Hashable for T {
        fn add_to(&self, digest: &mut crate::snapshot::Digest) -> anyhow::Result<()> {
            digest.add(self).map_err(|e| anyhow::anyhow!("fingerprint: {e}"))
        }
    }
}
