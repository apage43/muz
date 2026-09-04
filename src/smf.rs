//! Thin structural SMF interchange. This is an advanced file surface, not the composition IR.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Document {
    pub format: u16,
    pub division: u16,
    pub tracks: Vec<Vec<Event>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub delta: u32,
    #[serde(flatten)]
    pub kind: Kind,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    Midi { bytes: Vec<u8> },
    Meta { tag: u8, data: Vec<u8> },
    SysEx { data: Vec<u8> },
    Escape { data: Vec<u8> },
}
fn take<'a>(v: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    ensure!(n <= v.len(), "truncated MIDI field");
    let (a, b) = v.split_at(n);
    *v = b;
    Ok(a)
}
fn vlq(v: &mut &[u8]) -> Result<u32> {
    let mut n = 0;
    for _ in 0..4 {
        let b = take(v, 1)?[0];
        n = (n << 7) | (b & 127) as u32;
        if b < 128 {
            return Ok(n);
        }
    }
    bail!("MIDI VLQ exceeds four bytes")
}
fn put_vlq(out: &mut Vec<u8>, mut n: u32) -> Result<()> {
    ensure!(n < 1 << 28, "delta/payload exceeds SMF VLQ range");
    let mut b = [0u8; 4];
    let mut i = 3;
    b[i] = (n & 127) as u8;
    while {
        n >>= 7;
        n > 0
    } {
        i -= 1;
        b[i] = (n & 127) as u8 | 128;
    }
    out.extend_from_slice(&b[i..]);
    Ok(())
}
pub fn read(path: &Path) -> Result<Document> {
    decode(&std::fs::read(path)?)
}
pub fn decode(bytes: &[u8]) -> Result<Document> {
    ensure!(bytes.len() <= 32 * 1024 * 1024, "SMF exceeds 32 MiB");
    midly::Smf::parse(bytes).context("invalid SMF")?;
    let mut b = bytes;
    ensure!(take(&mut b, 4)? == b"MThd", "missing MIDI header");
    let len = u32::from_be_bytes(take(&mut b, 4)?.try_into()?) as usize;
    let h = take(&mut b, len)?;
    ensure!(h.len() >= 6, "short header");
    let format = u16::from_be_bytes(h[..2].try_into()?);
    let count = u16::from_be_bytes(h[2..4].try_into()?);
    let division = u16::from_be_bytes(h[4..6].try_into()?);
    let mut tracks = vec![];
    for _ in 0..count {
        ensure!(take(&mut b, 4)? == b"MTrk", "missing track");
        let len = u32::from_be_bytes(take(&mut b, 4)?.try_into()?) as usize;
        let mut tr = take(&mut b, len)?;
        let mut running = 0;
        let mut events = vec![];
        while !tr.is_empty() {
            ensure!(events.len() < 1_000_000, "track event budget exceeded");
            let delta = vlq(&mut tr)?;
            let first = *tr.first().ok_or_else(|| anyhow::anyhow!("missing event"))?;
            let status = if first >= 128 {
                take(&mut tr, 1)?[0]
            } else {
                ensure!(running >= 128, "missing running status");
                running
            };
            let kind = match status {
                0x80..=0xef => {
                    running = status;
                    let n = if status >> 4 == 0xc || status >> 4 == 0xd {
                        1
                    } else {
                        2
                    };
                    let data = take(&mut tr, n)?;
                    ensure!(data.iter().all(|b| *b < 128), "invalid MIDI data byte");
                    let mut bytes = vec![status];
                    bytes.extend_from_slice(data);
                    Kind::Midi { bytes }
                }
                0xff => {
                    running = 0;
                    let tag = take(&mut tr, 1)?[0];
                    let n = vlq(&mut tr)? as usize;
                    Kind::Meta {
                        tag,
                        data: take(&mut tr, n)?.to_vec(),
                    }
                }
                0xf0 | 0xf7 => {
                    running = 0;
                    let n = vlq(&mut tr)? as usize;
                    let data = take(&mut tr, n)?.to_vec();
                    if status == 0xf0 {
                        Kind::SysEx { data }
                    } else {
                        Kind::Escape { data }
                    }
                }
                _ => bail!("unsupported SMF status {status:02x}"),
            };
            events.push(Event { delta, kind });
        }
        tracks.push(events);
    }
    Ok(Document {
        format,
        division,
        tracks,
    })
}
pub fn encode(doc: &Document) -> Result<Vec<u8>> {
    ensure!(
        doc.format <= 2
            && doc.tracks.len() <= 65535
            && !doc.tracks.is_empty()
            && (doc.format != 0 || doc.tracks.len() == 1),
        "invalid SMF format or track count"
    );
    if doc.division & 0x8000 == 0 {
        ensure!(doc.division > 0, "PPQN must be positive");
    } else {
        ensure!(
            matches!((doc.division >> 8) as u8 as i8, -24 | -25 | -29 | -30)
                && doc.division & 255 > 0,
            "invalid SMPTE division"
        );
    }
    let mut out = b"MThd\0\0\0\x06".to_vec();
    out.extend_from_slice(&doc.format.to_be_bytes());
    out.extend_from_slice(&(doc.tracks.len() as u16).to_be_bytes());
    out.extend_from_slice(&doc.division.to_be_bytes());
    for tr in &doc.tracks {
        let mut data = vec![];
        for ev in tr {
            put_vlq(&mut data, ev.delta)?;
            match &ev.kind {
                Kind::Midi { bytes } => {
                    ensure!(
                        !bytes.is_empty() && (0x80..=0xef).contains(&bytes[0]),
                        "invalid channel status"
                    );
                    let n = if matches!(bytes[0] >> 4, 0xc | 0xd) {
                        2
                    } else {
                        3
                    };
                    ensure!(
                        bytes.len() == n && bytes[1..].iter().all(|b| *b < 128),
                        "invalid channel message"
                    );
                    data.extend_from_slice(bytes);
                }
                Kind::Meta { tag, data: payload } => {
                    data.extend_from_slice(&[255, *tag]);
                    put_vlq(&mut data, payload.len() as u32)?;
                    data.extend_from_slice(payload);
                }
                Kind::SysEx { data: payload } | Kind::Escape { data: payload } => {
                    data.push(if matches!(&ev.kind, Kind::SysEx { .. }) {
                        240
                    } else {
                        247
                    });
                    put_vlq(&mut data, payload.len() as u32)?;
                    data.extend_from_slice(payload);
                }
            }
        }
        out.extend_from_slice(b"MTrk");
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend(data);
    }
    midly::Smf::parse(&out).context("encoded SMF is invalid")?;
    Ok(out)
}
pub fn write(path: &Path, doc: &Document) -> Result<()> {
    std::fs::write(path, encode(doc)?)?;
    Ok(())
}
pub fn pattern(doc: &Document, track: Option<usize>) -> Result<crate::music::Pattern> {
    use crate::music::{Control, Note, Pattern, RawEvent, b};
    ensure!(
        doc.division & 0x8000 == 0,
        "SMPTE files use clock time; inspect/convert them explicitly before musical placement"
    );
    ensure!(
        doc.format != 2 || track.is_some(),
        "format 2 contains independent sequences; select a track"
    );
    if let Some(t) = track {
        ensure!(t < doc.tracks.len(), "unknown MIDI track");
    }
    let mut out = Pattern::default();
    for (ti, events) in doc
        .tracks
        .iter()
        .enumerate()
        .filter(|(i, _)| track.is_none_or(|t| t == *i))
    {
        let mut tick = 0u64;
        let mut held = std::collections::BTreeMap::<
            (u8, u8),
            std::collections::VecDeque<(u64, u8, usize)>,
        >::new();
        for (i, event) in events.iter().enumerate() {
            tick += event.delta as u64;
            let at = b(tick as i64) / b(doc.division as i64);
            match &event.kind {
                Kind::Midi { bytes } => {
                    let ch = bytes[0] & 15;
                    let key = bytes[1];
                    match bytes[0] >> 4 {
                        9 if bytes[2] > 0 => held
                            .entry((ch, key))
                            .or_default()
                            .push_back((tick, bytes[2], i)),
                        8 | 9 => {
                            if let Some((start, vel, order)) =
                                held.get_mut(&(ch, key)).and_then(|q| q.pop_front())
                            {
                                if tick > start {
                                    let mut n = Note::new(
                                        b(start as i64) / b(doc.division as i64),
                                        b((tick - start) as i64) / b(doc.division as i64),
                                        key as f64,
                                        format!("midi{ti}.{order}"),
                                    );
                                    n.velocity = vel as f64 / 127.;
                                    n.release = if bytes[0] >> 4 == 8 {
                                        bytes[2] as f64 / 127.
                                    } else {
                                        0.
                                    };
                                    n.gate = 1.;
                                    n.voice = format!("track{ti}");
                                    n.data.insert("channel".into(), serde_json::json!(ch));
                                    out.notes.push(n);
                                }
                            }
                        }
                        11 if ch == 0 => out.controls.push(Control {
                            offset_ms: 0.0,
                            at,
                            cc: key,
                            value: bytes[2],
                        }),
                        _ => out.raw.push(RawEvent {
                            at,
                            bytes: bytes.clone(),
                        }),
                    }
                }
                Kind::SysEx { data } => {
                    let mut bytes = vec![240];
                    bytes.extend(data);
                    out.raw.push(RawEvent { at, bytes });
                }
                Kind::Escape { data } => out.raw.push(RawEvent {
                    at,
                    bytes: data.clone(),
                }),
                Kind::Meta { .. } => {}
            }
        }
        ensure!(
            held.values().all(|q| q.is_empty()),
            "MIDI track {ti} has notes without releases"
        );
        out.span = out.span.max(b(tick as i64) / b(doc.division as i64));
    }
    out.notes.sort_by_key(|n| n.at);
    out.validate()?;
    Ok(out)
}
pub fn export(session: &crate::Session) -> Result<Document> {
    use crate::model::TrackSource;
    let mut tracks = vec![];
    let mut tempo = vec![];
    if let Some(TrackSource::Midi(m)) = session.tracks.first().map(|t| &t.source) {
        for t in &m.imported.tempos {
            let n = t.micros_per_quarter;
            tempo.push((
                t.tick * 960 / m.summary.ppq as u64,
                Kind::Meta {
                    tag: 0x51,
                    data: vec![(n >> 16) as u8, (n >> 8) as u8, n as u8],
                },
            ));
        }
    }
    let meter = session.transport.meter();
    tempo.push((
        0,
        Kind::Meta {
            tag: 0x58,
            data: vec![meter[0], meter[1].ilog2() as u8, 24, 8],
        },
    ));
    tracks.push(events(tempo)?);
    for t in &session.tracks {
        if let TrackSource::Midi(m) = &t.source {
            let at = |v: u64| ((v as f64 * 960. / m.summary.ppq as f64).round()) as u64;
            let mut ev = vec![(
                0,
                Kind::Meta {
                    tag: 3,
                    data: t.id.as_str().as_bytes().to_vec(),
                },
            )];
            for n in &m.imported.notes {
                ev.push((
                    at(n.start_tick),
                    Kind::Midi {
                        bytes: vec![0x90 | n.channel, n.key, n.attack_velocity],
                    },
                ));
                ev.push((
                    at(n.start_tick + n.duration_ticks),
                    Kind::Midi {
                        bytes: vec![0x80 | n.channel, n.key, n.release_velocity],
                    },
                ));
            }
            for c in &m.imported.controllers {
                ev.push((
                    at(c.tick),
                    Kind::Midi {
                        bytes: vec![0xb0 | c.channel, c.controller, c.value],
                    },
                ));
            }
            for m in &m.imported.messages {
                ev.push((
                    at(m.tick),
                    Kind::Midi {
                        bytes: m.bytes[..m.len as usize].to_vec(),
                    },
                ));
            }
            tracks.push(events(ev)?);
        }
    }
    Ok(Document {
        format: 1,
        division: 960,
        tracks,
    })
}
fn events(mut input: Vec<(u64, Kind)>) -> Result<Vec<Event>> {
    input.sort_by_key(|(tick, k)| {
        (
            *tick,
            match k {
                Kind::Meta { .. } => 0,
                Kind::Midi { bytes } if bytes[0] >> 4 == 8 => 1,
                _ => 2,
            },
        )
    });
    let mut previous = 0;
    let mut out = vec![];
    for (tick, kind) in input {
        let delta = u32::try_from(tick - previous)?;
        ensure!(delta < 1 << 28, "event gap exceeds SMF range");
        out.push(Event { delta, kind });
        previous = tick;
    }
    out.push(Event {
        delta: 0,
        kind: Kind::Meta {
            tag: 47,
            data: vec![],
        },
    });
    Ok(out)
}
