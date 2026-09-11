use crate::{Session, lang::{Diagnostic, suggest_vocabulary}, model::TrackSource};
use anyhow::Result;
pub fn session(s: &Session, view: &str) -> Result<serde_json::Value> {
    match view {
        "graph"=>Ok(serde_json::to_value(s)?),
        "automation"=>Ok(serde_json::to_value(&s.extras.automation)?),
        "sections"=>Ok(serde_json::to_value(&s.extras.sections)?),
        "performance"=>Ok(serde_json::Value::Array(s.tracks.iter().map(|t|match &t.source { TrackSource::Midi(m)=>serde_json::json!({"track":t.id,"ppq":m.imported.summary.ppq,"notes":m.imported.notes,"controllers":m.imported.controllers,"channel_events":m.imported.messages,"tempos":m.imported.tempos}),_=>serde_json::json!({"track":t.id,"source":t.source}) }).collect())),
        _=>return Err(Diagnostic::new("view must be graph, performance, automation or sections")
            .helps(suggest_vocabulary(
                "views",
                view,
                ["graph", "performance", "automation", "sections"],
            ))
            .err()),
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
        let helps = if s.tracks.is_empty() {
            Vec::new()
        } else {
            suggest_vocabulary("tracks", id, s.tracks.iter().map(|t| t.id.as_str()))
        };
        s.tracks.retain(|t| {
            t.id.as_str() == id
                || t.id
                    .as_str()
                    .strip_prefix(id)
                    .is_some_and(|v| v.starts_with('.'))
        });
        if s.tracks.is_empty() {
            return Err(Diagnostic::new(format!("unknown inspection track '{id}'"))
                .helps(helps)
                .err());
        }
    }
    if let Some(name) = section {
        let helps = if s.extras.sections.is_empty() {
            Vec::new()
        } else {
            suggest_vocabulary(
                "sections",
                name,
                s.extras.sections.iter().map(|sec| sec.name.as_str()),
            )
        };
        let sec = s
            .extras
            .sections
            .iter()
            .find(|sec| sec.name == name)
            .ok_or_else(|| {
                Diagnostic::new(format!("unknown section '{name}'"))
                    .helps(helps)
                    .err()
            })?;
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
