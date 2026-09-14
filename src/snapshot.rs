//! Versioned playable transfer. Session JSON alone is an inspection description,
//! not a playback archive. Asset tokens are weak size/mtime revisions, not hashes.
use crate::{
    Session,
    midi::ImportedMidi,
    model::{Id, TrackSource},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

pub const MAX_SNAPSHOT_BYTES: usize = 256 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformedTrack {
    pub track_id: Id,
    pub source_id: Id,
    pub midi: ImportedMidi,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetRevision {
    pub reference: PathBuf,
    pub bytes: u64,
    pub modified_nanos: u128,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayableSnapshotV1 {
    pub version: u32,
    pub description: Session,
    pub performed: Vec<PerformedTrack>,
    pub assets: Vec<AssetRevision>,
}
impl PlayableSnapshotV1 {
    pub fn capture(session: &Session) -> Result<Self> {
        validate_events(session)?;
        let mut assets = Vec::new();
        for d in devices(session) {
            for (i, path) in crate::assets::paths(d).into_iter().enumerate() {
                let (bytes, modified_nanos) = if let Some(v) = d.asset_versions.get(i) {
                    *v
                } else {
                    file_revision(&path)?
                };
                assets.push(AssetRevision {
                    reference: path,
                    bytes,
                    modified_nanos,
                });
            }
        }
        assets.sort_by(|a, b| a.reference.cmp(&b.reference));
        assets.dedup_by(|a, b| {
            a.reference == b.reference && a.bytes == b.bytes && a.modified_nanos == b.modified_nanos
        });
        Ok(Self {
            version: 1,
            description: session.clone(),
            performed: session
                .tracks
                .iter()
                .filter_map(|t| match &t.source {
                    TrackSource::Midi(m) => Some(PerformedTrack {
                        track_id: t.id.clone(),
                        source_id: m.id.clone(),
                        midi: m.imported.clone(),
                    }),
                    _ => None,
                })
                .collect(),
            assets,
        })
    }
    pub fn decode_checked(bytes: &[u8], max_bytes: usize) -> Result<Self> {
        ensure!(
            bytes.len() <= max_bytes.min(MAX_SNAPSHOT_BYTES),
            "snapshot byte limit exceeded"
        );
        let value: Self = serde_json::from_slice(bytes)?;
        value.validate_associations()?;
        Ok(value)
    }
    fn validate_associations(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported playable snapshot version {}",
            self.version
        );
        let mut tracks = BTreeSet::new();
        let mut sources = BTreeSet::new();
        for t in &self.description.tracks {
            ensure!(tracks.insert(&t.id), "duplicate track ID {}", t.id);
            ensure!(
                sources.insert(t.source.id()),
                "duplicate source ID {}",
                t.source.id()
            );
        }
        let mut seen = BTreeSet::new();
        for p in &self.performed {
            ensure!(
                seen.insert(&p.track_id),
                "duplicate performed association {}",
                p.track_id
            );
            let t = self
                .description
                .tracks
                .iter()
                .find(|t| t.id == p.track_id)
                .ok_or_else(|| anyhow::anyhow!("unknown performed track {}", p.track_id))?;
            let TrackSource::Midi(m) = &t.source else {
                anyhow::bail!("performed association targets non-MIDI track")
            };
            ensure!(m.id == p.source_id, "performed source ID mismatch");
            ensure!(m.summary == p.midi.summary, "performed summary mismatch");
            validate_midi(&p.midi)?;
        }
        ensure!(
            self.description
                .tracks
                .iter()
                .filter(|t| matches!(t.source, TrackSource::Midi(_)))
                .count()
                == seen.len(),
            "missing performed association"
        );
        Ok(())
    }
    /// Restore embedded event data without performing asset I/O. A non-filesystem
    /// host must separately validate every asset revision before preparation.
    pub fn restore_description(mut self) -> Result<Session> {
        self.validate_associations()?;
        for p in self.performed {
            let t = self
                .description
                .tracks
                .iter_mut()
                .find(|t| t.id == p.track_id)
                .unwrap();
            let TrackSource::Midi(m) = &mut t.source else {
                unreachable!()
            };
            m.imported = p.midi;
        }
        validate_events(&self.description)?;
        Ok(self.description)
    }
    pub fn restore_checked(self) -> Result<Session> {
        let mut expected = BTreeSet::new();
        for a in &self.assets {
            ensure!(
                expected.insert(a.reference.clone()),
                "duplicate asset reference"
            );
            ensure!(
                file_revision(&a.reference)? == (a.bytes, a.modified_nanos),
                "asset revision changed: {}",
                a.reference.display()
            );
        }
        let required: BTreeSet<_> = devices(&self.description)
            .flat_map(crate::assets::paths)
            .collect();
        ensure!(
            expected == required,
            "asset manifest does not match description"
        );
        self.restore_description()
    }
}
fn devices(s: &Session) -> impl Iterator<Item = &crate::model::Device> {
    s.tracks
        .iter()
        .flat_map(|t| std::iter::once(&t.instrument).chain(&t.inserts))
        .chain(s.buses.iter().flat_map(|b| &b.inserts))
        .chain(&s.master.inserts)
}
fn file_revision(path: &std::path::Path) -> Result<(u64, u128)> {
    let m = std::fs::metadata(path)?;
    Ok((
        m.len(),
        m.modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    ))
}
/// Shared validation for deserialized and direct embedding data, before indexing
/// fixed-size expression/message buffers or calculating event endpoints.
pub fn validate_events(s: &Session) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut sources = BTreeSet::new();
    for t in &s.tracks {
        ensure!(ids.insert(&t.id), "duplicate track ID");
        ensure!(sources.insert(t.source.id()), "duplicate source ID");
        if let TrackSource::Midi(m) = &t.source {
            ensure!(m.channel < 16, "invalid MIDI channel");
            validate_midi(&m.imported)?;
        }
    }
    Ok(())
}
pub fn validate_midi(m: &ImportedMidi) -> Result<()> {
    let limits = crate::limits::ExpansionLimits::default();
    ensure!(
        m.notes.len() <= limits.notes
            && m.controllers.len() <= limits.controls
            && m.messages.len() <= limits.raw
            && m.tempos.len() <= limits.controls,
        "performed event limit exceeded"
    );
    ensure!(m.summary.ppq > 0, "MIDI PPQ must be positive");
    ensure!(
        m.summary.notes as usize == m.notes.len()
            && m.summary.controllers as usize == m.controllers.len()
            && m.summary.tempos as usize == m.tempos.len(),
        "inconsistent performed event counts"
    );
    for n in &m.notes {
        ensure!(
            n.channel < 16 && n.key < 128 && n.attack_velocity < 128 && n.release_velocity < 128,
            "invalid MIDI note range"
        );
        ensure!(
            n.duration_ticks > 0 && n.start_tick.checked_add(n.duration_ticks).is_some(),
            "invalid or overflowing note duration"
        );
        if let Some(p) = n.performance {
            ensure!(
                p.pitch.is_finite()
                    && (0.0..=127.).contains(&p.pitch)
                    && p.velocity.is_finite()
                    && (0.0..=1.).contains(&p.velocity),
                "invalid note performance"
            );
            ensure!(
                p.expression.len as usize <= p.expression.points.len(),
                "invalid expression length"
            );
            let mut last = [-1.; 7];
            for point in &p.expression.points[..p.expression.len as usize] {
                ensure!(point.kind < 7, "unknown expression kind");
                let (lo, hi) = match point.kind {
                    0 => (0., 4.),
                    2 => (-120., 120.),
                    _ => (0., 1.),
                };
                ensure!(
                    point.phase.is_finite()
                        && (0.0..=1.).contains(&point.phase)
                        && point.phase > last[point.kind as usize]
                        && point.value.is_finite()
                        && (lo..=hi).contains(&point.value),
                    "invalid expression point"
                );
                last[point.kind as usize] = point.phase;
            }
        }
    }
    for c in &m.controllers {
        ensure!(
            c.tick <= m.summary.end_tick && c.channel < 16 && c.controller < 128 && c.value < 128,
            "invalid controller event"
        );
    }
    for t in &m.tempos {
        ensure!(t.micros_per_quarter > 0, "invalid tempo event");
    }
    for c in &m.messages {
        let status = c.bytes[0] >> 4;
        let len = match status {
            0xc | 0xd => 2,
            0x8..=0xe => 3,
            _ => 0,
        };
        ensure!(
            len > 0
                && c.len == len
                && c.tick <= m.summary.end_tick
                && c.bytes[1..len as usize].iter().all(|b| *b < 128),
            "invalid channel message"
        );
    }
    ensure!(
        m.notes
            .windows(2)
            .all(|w| w[0].start_tick <= w[1].start_tick)
            && m.controllers.windows(2).all(|w| w[0].tick <= w[1].tick)
            && m.tempos.windows(2).all(|w| w[0].tick <= w[1].tick)
            && m.messages.windows(2).all(|w| w[0].tick <= w[1].tick),
        "performed events must be time ordered"
    );
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub revision: u64,
    pub title: String,
    pub tracks: Vec<TrackSummary>,
    pub total_tracks: usize,
    pub truncated: bool,
}
#[derive(Debug, Serialize)]
pub struct TrackSummary {
    pub id: String,
    pub name: String,
    pub notes: usize,
    pub controllers: usize,
    pub messages: usize,
}
impl SessionSummary {
    pub fn new(s: &Session, revision: u64) -> Self {
        let short = |s: &str| s.chars().take(64).collect::<String>();
        let tracks = s
            .tracks
            .iter()
            .take(1000)
            .map(|t| {
                let (notes, controllers, messages) = match &t.source {
                    TrackSource::Midi(m) => (
                        m.imported.notes.len(),
                        m.imported.controllers.len(),
                        m.imported.messages.len(),
                    ),
                    TrackSource::Pattern(p) => (p.notes.len(), 0, 0),
                };
                TrackSummary {
                    id: short(t.id.as_str()),
                    name: short(&t.name),
                    notes,
                    controllers,
                    messages,
                }
            })
            .collect();
        Self {
            revision,
            title: short(&s.extras.title),
            tracks,
            total_tracks: s.tracks.len(),
            truncated: s.tracks.len() > 1000,
        }
    }
}
