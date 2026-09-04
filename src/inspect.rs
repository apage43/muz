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
