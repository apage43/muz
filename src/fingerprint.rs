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
    let (base, post) = match tap.strip_suffix(":post") {
        Some(base) => (base, true),
        None => (tap, false),
    };
    if session.tracks.iter().any(|t| t.id.as_str() == base) {
        graph.track(base, post);
    } else if base == session.master.id.as_str() {
        graph.bus(base, false);
    } else if session.buses.iter().any(|b| b.id.as_str() == base) {
        graph.bus(base, post);
    } else {
        let members = group_members(session, base);
        if members.is_empty() {
            bail!("unknown tap {tap}");
        }
        for member in members {
            graph.track(&member, post);
        }
    }
    let mut digest = crate::snapshot::Digest::new();
    let mut add = |value: &dyn erased::Hashable| value.add_to(&mut digest);
    add(&("muz output fingerprint", 1u32, engine_identity(), sample_rate, tap))?;
    add(&session.transport)?;
    // The tempo timeline compiles from every track's map and extent.
    for t in &session.tracks {
        let TrackSource::Midi(m) = &t.source;
        add(&(&m.imported.tempos, m.summary.end_tick, m.summary.ppq))?;
    }
    for (key, item) in &graph.items {
        add(key)?;
        add(item)?;
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
    Ok(digest.finish().iter().map(|b| format!("{b:02x}")).collect())
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
