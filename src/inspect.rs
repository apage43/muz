use crate::{
    Session,
    compile::Compiled,
    lang::{Diagnostic, suggest_vocabulary},
    model::{Bus, Device, TrackSource},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const MAX_PAGE_ROWS: usize = 1000;
pub const MAX_PAGE_BYTES: usize = 1024 * 1024;

/// Stop serialization before an oversized response can allocate its full bytes.
pub(crate) fn bounded_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    struct Writer(Vec<u8>);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > MAX_PAGE_BYTES {
                return Err(std::io::Error::other("one inspection row exceeds 1 MiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer(Vec::new());
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.0)
}

fn bounded_value<T: Serialize>(value: &T) -> Result<serde_json::Value> {
    Ok(serde_json::from_slice(&bounded_json(value)?)?)
}
fn default_limit() -> usize {
    100
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRequest {
    #[serde(default)]
    pub revision: Option<u64>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub track: Option<String>,
    #[serde(default)]
    pub start_tick: Option<u64>,
    #[serde(default)]
    pub end_tick: Option<u64>,
}
impl Default for PageRequest {
    fn default() -> Self {
        Self {
            revision: None,
            offset: 0,
            limit: default_limit(),
            track: None,
            start_tick: None,
            end_tick: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct InspectionPage {
    pub revision: u64,
    pub view: String,
    pub rows: Vec<serde_json::Value>,
    pub total: usize,
    pub next: Option<usize>,
}
struct Builder {
    offset: usize,
    limit: usize,
    seen: usize,
    bytes: usize,
    full: bool,
    rows: Vec<serde_json::Value>,
}
impl Builder {
    fn new(r: &PageRequest) -> Result<Self> {
        ensure!(
            r.limit > 0,
            "inspection page limit must be greater than zero"
        );
        ensure!(
            r.limit <= MAX_PAGE_ROWS,
            "inspection page limit exceeds {MAX_PAGE_ROWS} rows"
        );
        if let (Some(a), Some(b)) = (r.start_tick, r.end_tick) {
            ensure!(
                a < b,
                "inspection time range must have start_tick < end_tick"
            )
        }
        Ok(Self {
            offset: r.offset,
            limit: r.limit,
            seen: 0,
            bytes: 0,
            full: false,
            rows: vec![],
        })
    }
    fn push<T: Serialize>(&mut self, row: T) -> Result<()> {
        let take = self.seen >= self.offset && self.rows.len() < self.limit && !self.full;
        self.seen += 1;
        if !take {
            return Ok(());
        }
        let encoded = bounded_json(&row)?;
        let size = encoded.len();
        ensure!(size <= MAX_PAGE_BYTES, "one inspection row exceeds 1 MiB");
        if self.bytes + size <= MAX_PAGE_BYTES {
            self.bytes += size;
            self.rows.push(serde_json::from_slice(&encoded)?)
        } else {
            // Continuation must resume at this first omitted row. Once the byte
            // budget is exhausted, do not skip ahead to later smaller rows.
            self.full = true;
        }
        Ok(())
    }
    fn push_lazy<T: Serialize>(&mut self, make: impl FnOnce() -> T) -> Result<()> {
        let take = self.seen >= self.offset && self.rows.len() < self.limit && !self.full;
        self.seen += 1;
        if !take {
            return Ok(());
        }
        let encoded = bounded_json(&make())?;
        let size = encoded.len();
        ensure!(size <= MAX_PAGE_BYTES, "one inspection row exceeds 1 MiB");
        if self.bytes + size <= MAX_PAGE_BYTES {
            self.bytes += size;
            self.rows.push(serde_json::from_slice(&encoded)?)
        } else {
            self.full = true
        }
        Ok(())
    }
    fn finish(self, revision: u64, view: &str) -> InspectionPage {
        let end = self.offset.saturating_add(self.rows.len());
        InspectionPage {
            revision,
            view: view.into(),
            rows: self.rows,
            total: self.seen,
            next: (end < self.seen).then_some(end),
        }
    }
}
fn revision(r: &PageRequest, actual: u64) -> Result<()> {
    if let Some(want) = r.revision {
        ensure!(
            want == actual,
            "stale inspection revision {want}; current revision is {actual}"
        )
    }
    Ok(())
}
fn device(d: &Device) -> serde_json::Value {
    serde_json::json!({"id":d.id,"kind":d.kind,"generation":d.generation,"controls":d.control_values(),"has_patch":d.patch.is_some()})
}
fn bus(kind: &str, b: &Bus) -> serde_json::Value {
    serde_json::json!({"kind":kind,"id":b.id,"detail":{"name":b.name,"inserts":b.inserts.iter().map(device).collect::<Vec<_>>(),"output":b.output,"sends":b.sends}})
}
fn devices(s: &Session) -> impl Iterator<Item = (&str, &Device)> {
    s.master
        .inserts
        .iter()
        .map(|d| ("master", d))
        .chain(
            s.buses
                .iter()
                .flat_map(|b| b.inserts.iter().map(move |d| (b.id.as_str(), d))),
        )
        .chain(s.tracks.iter().flat_map(|t| {
            std::iter::once(&t.instrument)
                .chain(t.inserts.iter())
                .map(move |d| (t.id.as_str(), d))
        }))
}
fn selected(owner: &str, r: &PageRequest) -> bool {
    r.track.as_deref().is_none_or(|x| x == owner)
}
fn at(t: u64, r: &PageRequest) -> bool {
    r.start_tick.is_none_or(|a| t >= a) && r.end_tick.is_none_or(|b| t < b)
}
fn overlap(a: u64, b: u64, r: &PageRequest) -> bool {
    r.end_tick.is_none_or(|z| a < z) && r.start_tick.is_none_or(|z| b > z)
}

pub fn page_session(
    s: &Session,
    actual: u64,
    view: &str,
    r: &PageRequest,
) -> Result<InspectionPage> {
    revision(r, actual)?;
    if let Some(id) = r.track.as_deref() {
        let known = match view {
            "graph" | "performance" | "performance_overview" => {
                s.tracks.iter().any(|t| t.id.as_str() == id)
            }
            "patches" | "patch_nodes" | "patch_detail" => {
                id == "master"
                    || s.buses.iter().any(|b| b.id.as_str() == id)
                    || s.tracks.iter().any(|t| t.id.as_str() == id)
            }
            "automation" | "automation_points" => {
                s.extras.automation.iter().any(|a| a.target == id)
            }
            _ => true,
        };
        ensure!(known, "unknown inspection selector '{id}' for {view}");
    }
    let mut p = Builder::new(r)?;
    match view {
        "summary" => p.push(crate::snapshot::SessionSummary::new(s, actual)?)?,
        "graph" => {
            if r.track.is_none() {
                p.push_lazy(||serde_json::json!({"kind":"transport","id":"transport","detail":s.transport}))?;
                p.push_lazy(|| bus("master", &s.master))?;
                for b in &s.buses {
                    p.push_lazy(|| bus("bus", b))?
                }
            }
            for t in s.tracks.iter().filter(|t| selected(t.id.as_str(), r)) {
                p.push_lazy(||{let source=match &t.source{TrackSource::Pattern(x)=>serde_json::json!({"kind":"pattern","id":x.id,"notes":x.notes.len()}),TrackSource::Midi(x)=>serde_json::json!({"kind":"midi","id":x.id,"asset":x.asset,"channel":x.channel,"all_channels":x.all_channels,"summary":x.summary})};serde_json::json!({"kind":"track","id":t.id,"detail":{"name":t.name,"source":source,"instrument":device(&t.instrument),"inserts":t.inserts.iter().map(device).collect::<Vec<_>>(),"output":t.output,"sends":t.sends}})})?
            }
        }
        "patches" => {
            for (owner, d) in devices(s).filter(|(o, d)| selected(o, r) && d.patch.is_some()) {
                let patch = d.patch.as_ref().unwrap();
                let nodes = patch
                    .get("nodes")
                    .and_then(|x| x.as_array())
                    .map_or(0, Vec::len);
                p.push_lazy(||serde_json::json!({"owner":owner,"device":d.id,"kind":d.kind,"nodes":nodes,"controls":d.control_values(),"asset_files":crate::assets::paths(d)}))?
            }
        }
        "patch_nodes" => {
            for (owner, d) in devices(s).filter(|(o, _)| selected(o, r)) {
                if let Some(nodes) = d
                    .patch
                    .as_ref()
                    .and_then(|x| x.get("nodes"))
                    .and_then(|x| x.as_array())
                {
                    for (index, node) in nodes.iter().enumerate() {
                        p.push_lazy(||serde_json::json!({"owner":owner,"device":d.id,"index":index,"node":node}))?
                    }
                }
            }
        }
        "patch_detail" => {
            for (owner, d) in devices(s).filter(|(o, _)| selected(o, r)) {
                if let Some(serde_json::Value::Object(fields)) = &d.patch {
                    p.push_lazy(|| {
                        let fields: serde_json::Map<_, _> = fields
                            .iter()
                            .filter(|(key, _)| key.as_str() != "nodes")
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect();
                        serde_json::json!({"owner":owner,"device":d.id,"detail":fields})
                    })?
                }
            }
        }
        "automation" => {
            for (index, lane) in s
                .extras
                .automation
                .iter()
                .enumerate()
                .filter(|(_, x)| r.track.as_deref().is_none_or(|t| t == x.target))
            {
                p.push_lazy(||serde_json::json!({"index":index,"target":lane.target,"shape":lane.shape,"points":lane.points.len()}))?
            }
        }
        "automation_points" => {
            for (lane, l) in s
                .extras
                .automation
                .iter()
                .enumerate()
                .filter(|(_, x)| r.track.as_deref().is_none_or(|t| t == x.target))
            {
                for (index, point) in l.points.iter().enumerate() {
                    p.push_lazy(||serde_json::json!({"lane":lane,"target":l.target,"index":index,"point":point}))?
                }
            }
        }
        "sections" => {
            for x in &s.extras.sections {
                p.push(x)?
            }
        }
        "track_groups" => {
            for x in &s.extras.track_groups {
                p.push(x)?
            }
        }
        "performance" => {
            for t in s.tracks.iter().filter(|t| selected(t.id.as_str(), r)) {
                if let TrackSource::Midi(m) = &t.source {
                    for e in m.imported.notes.iter().filter(|x| {
                        overlap(
                            x.start_tick,
                            x.start_tick.saturating_add(x.duration_ticks),
                            r,
                        )
                    }) {
                        p.push_lazy(
                            || serde_json::json!({"track":t.id,"stream":"notes","event":e}),
                        )?
                    }
                    for e in m.imported.controllers.iter().filter(|x| at(x.tick, r)) {
                        p.push_lazy(
                            || serde_json::json!({"track":t.id,"stream":"controllers","event":e}),
                        )?
                    }
                    for e in m.imported.messages.iter().filter(|x| at(x.tick, r)) {
                        p.push_lazy(
                            || serde_json::json!({"track":t.id,"stream":"messages","event":e}),
                        )?
                    }
                    for e in m.imported.tempos.iter().filter(|x| at(x.tick, r)) {
                        p.push_lazy(
                            || serde_json::json!({"track":t.id,"stream":"tempos","event":e}),
                        )?
                    }
                }
            }
        }
        "performance_overview" => {
            const BINS: u64 = 128;
            for t in s.tracks.iter().filter(|t| selected(t.id.as_str(), r)) {
                let TrackSource::Midi(m) = &t.source else {
                    continue;
                };
                let start = r.start_tick.unwrap_or(0);
                let end = match r.end_tick {
                    Some(end) => end,
                    None if m.imported.summary.end_tick > start => m.imported.summary.end_tick,
                    None => start.checked_add(1).ok_or_else(|| {
                        anyhow::anyhow!("performance overview range overflows u64 ticks")
                    })?,
                };
                let span = end.checked_sub(start).ok_or_else(|| {
                    anyhow::anyhow!("performance overview range has end before start")
                })?;
                let width = span.div_ceil(BINS).max(1);
                let count = span.div_ceil(width);
                let count_usize = usize::try_from(count).expect("overview has at most 128 bins");
                let mut note_delta = vec![0i64; count_usize + 1];
                let mut controllers = vec![0usize; count_usize];
                let mut messages = vec![0usize; count_usize];
                let mut tempos = vec![0usize; count_usize];
                for x in &m.imported.notes {
                    let x_end = x.start_tick.saturating_add(x.duration_ticks);
                    if x.start_tick < end && x_end > start {
                        let first =
                            ((x.start_tick.max(start) - start) / width).min(count - 1) as usize;
                        let last = ((x_end.min(end).saturating_sub(1) - start) / width)
                            .min(count - 1) as usize;
                        note_delta[first] += 1;
                        note_delta[last + 1] -= 1;
                    }
                }
                for x in &m.imported.controllers {
                    if x.tick >= start && x.tick < end {
                        controllers[((x.tick - start) / width).min(count - 1) as usize] += 1
                    }
                }
                for x in &m.imported.messages {
                    if x.tick >= start && x.tick < end {
                        messages[((x.tick - start) / width).min(count - 1) as usize] += 1
                    }
                }
                for x in &m.imported.tempos {
                    if x.tick >= start && x.tick < end {
                        tempos[((x.tick - start) / width).min(count - 1) as usize] += 1
                    }
                }
                let mut active = 0i64;
                for bin in 0..count {
                    let index = bin as usize;
                    active += note_delta[index];
                    let notes = active as usize;
                    let a = start.saturating_add(bin.saturating_mul(width)).min(end);
                    let b = a.saturating_add(width).min(end);
                    let controllers = controllers[index];
                    let messages = messages[index];
                    let tempos = tempos[index];
                    p.push_lazy(||serde_json::json!({"track":t.id,"start_tick":a,"end_tick":b,"notes":notes,"controllers":controllers,"messages":messages,"tempos":tempos}))?;
                }
            }
        }
        _ => return Err(unknown(view)),
    }
    Ok(p.finish(actual, view))
}
pub fn page_compiled(
    c: &Compiled,
    actual: u64,
    view: &str,
    r: &PageRequest,
) -> Result<InspectionPage> {
    if !matches!(view, "locations" | "score" | "diagnostics") {
        return page_session(&c.session, actual, view, r);
    }
    revision(r, actual)?;
    let mut p = Builder::new(r)?;
    match view {
        "locations" => {
            for (key, location) in &c.locations {
                p.push_lazy(|| serde_json::json!({"key":key,"location":location}))?
            }
        }
        "score" => {
            for t in c.score.iter().filter(|t| selected(&t.id, r)) {
                for note in t.pattern.notes.iter().filter(|n| {
                    overlap(
                        crate::compile::tick(crate::music::real(n.at)),
                        crate::compile::tick(crate::music::real(n.at + n.dur)),
                        r,
                    )
                }) {
                    p.push_lazy(|| serde_json::json!({"track":t.id,"policy":t.policy,"note":note}))?
                }
            }
        }
        "diagnostics" => {
            for d in c
                .diagnostics
                .iter()
                .filter(|d| selected(&d.track, r) && at(crate::compile::tick(d.beat), r))
            {
                p.push_lazy(|| d)?
            }
        }
        _ => unreachable!(),
    }
    Ok(p.finish(actual, view))
}
fn unknown(view: &str) -> anyhow::Error {
    Diagnostic::new("unknown inspection view")
        .helps(suggest_vocabulary(
            "views",
            view,
            [
                "summary",
                "graph",
                "patches",
                "patch_nodes",
                "patch_detail",
                "performance",
                "performance_overview",
                "automation",
                "automation_points",
                "sections",
                "track_groups",
                "locations",
                "score",
                "diagnostics",
            ],
        ))
        .err()
}

/// Compatibility full views reject responses that would require continuation.
pub fn session(s: &Session, view: &str) -> Result<serde_json::Value> {
    let value = match view {
        "summary" => bounded_value(&crate::snapshot::SessionSummary::new(s, 0)?)?,
        "graph" => bounded_value(s)?,
        "automation" => {
            ensure!(
                s.extras.automation.len() <= MAX_PAGE_ROWS,
                "full automation inspection exceeds {MAX_PAGE_ROWS} rows; request pages explicitly"
            );
            bounded_value(&s.extras.automation)?
        }
        "sections" => {
            ensure!(
                s.extras.sections.len() <= MAX_PAGE_ROWS,
                "full sections inspection exceeds {MAX_PAGE_ROWS} rows; request pages explicitly"
            );
            bounded_value(&s.extras.sections)?
        }
        _ => {
            let x = page_session(
                s,
                0,
                view,
                &PageRequest {
                    limit: MAX_PAGE_ROWS,
                    ..Default::default()
                },
            )?;
            ensure!(
                x.next.is_none(),
                "full {view} inspection exceeds bounded response; request pages explicitly"
            );
            serde_json::Value::Array(x.rows)
        }
    };
    ensure!(
        bounded_json(&value)?.len() <= MAX_PAGE_BYTES,
        "full {view} inspection exceeds 1 MiB; request pages explicitly"
    );
    Ok(value)
}
pub fn filtered(
    s: &Session,
    view: &str,
    section: Option<&str>,
    track: Option<&str>,
) -> Result<serde_json::Value> {
    let mut r = PageRequest {
        track: track.map(str::to_owned),
        limit: MAX_PAGE_ROWS,
        ..Default::default()
    };
    if let Some(name) = section {
        let x = s
            .extras
            .sections
            .iter()
            .find(|x| x.name == name)
            .ok_or_else(|| {
                Diagnostic::new(format!("unknown section '{name}'"))
                    .helps(suggest_vocabulary(
                        "sections",
                        name,
                        s.extras.sections.iter().map(|x| x.name.as_str()),
                    ))
                    .err()
            })?;
        r.start_tick = Some(crate::compile::tick(x.start));
        r.end_tick = Some(crate::compile::tick(x.end))
    }
    let x = page_session(s, 0, view, &r)?;
    ensure!(
        x.next.is_none(),
        "full {view} inspection exceeds bounded response; request pages explicitly"
    );
    Ok(serde_json::Value::Array(x.rows))
}
pub fn performance_page(
    s: &Session,
    track: &str,
    offset: usize,
    limit: usize,
) -> Result<crate::provenance::Page<serde_json::Value>> {
    let x = page_session(
        s,
        0,
        "performance",
        &PageRequest {
            offset,
            limit,
            track: Some(track.into()),
            ..Default::default()
        },
    )?;
    Ok(crate::provenance::Page {
        rows: x.rows,
        total: x.total,
        next: x.next,
    })
}
