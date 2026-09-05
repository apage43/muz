use crate::{Session, model::TrackSource};
use anyhow::{Result, bail};
pub fn session(s: &Session, view: &str) -> Result<serde_json::Value> {
    match view {
        "graph"=>Ok(serde_json::to_value(s)?),
        "automation"=>Ok(serde_json::to_value(&s.extras.automation)?),
        "sections"=>Ok(serde_json::to_value(&s.extras.sections)?),
        "performance"=>Ok(serde_json::Value::Array(s.tracks.iter().map(|t|match &t.source { TrackSource::Midi(m)=>serde_json::json!({"track":t.id,"ppq":m.imported.summary.ppq,"notes":m.imported.notes,"controllers":m.imported.controllers,"channel_events":m.imported.messages,"tempos":m.imported.tempos}),_=>serde_json::json!({"track":t.id,"source":t.source}) }).collect())),
        _=>bail!("view must be graph, performance, automation or sections"),
    }
}
pub fn filtered(
    s: &Session,
    view: &str,
    section: Option<&str>,
    track: Option<&str>,
) -> Result<serde_json::Value> {
    if section.is_none() && track.is_none() {
        return session(s, view);
    }
    let mut s = s.clone();
    if let Some(id) = track {
        s.tracks.retain(|t| {
            t.id.as_str() == id
                || t.id
                    .as_str()
                    .strip_prefix(id)
                    .is_some_and(|v| v.starts_with('.'))
        });
        if s.tracks.is_empty() {
            bail!("unknown inspection track '{id}'")
        }
    }
    if let Some(name) = section {
        let sec = s
            .extras
            .sections
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| anyhow::anyhow!("unknown section '{name}'"))?;
        let (a, b) = (
            crate::compile::tick(sec.start),
            crate::compile::tick(sec.end),
        );
        for t in &mut s.tracks {
            if let TrackSource::Midi(m) = &mut t.source {
                m.imported
                    .notes
                    .retain(|n| n.start_tick < b && n.start_tick + n.duration_ticks > a);
                m.imported.controllers.retain(|c| c.tick >= a && c.tick < b);
                m.imported.messages.retain(|m| m.tick >= a && m.tick < b);
            }
        }
    }
    session(&s, view)
}
