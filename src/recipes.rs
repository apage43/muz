//! Named output collections are source data; execution reuses the render queue.
use crate::{compile, lang::Evaluator, render::RenderOptions};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub struct PreparedRecipe {
    pub names: Vec<String>,
    pub requests: Vec<(crate::Session, PathBuf, RenderOptions)>,
}

pub fn prepare(source: &Path, name: &str, output: &Path) -> Result<PreparedRecipe> {
    let mut evaluator = Evaluator::new();
    let module = evaluator.module(source)?;
    let root = module
        .get(name)
        .or_else(|| module.get("__result").and_then(|v| v.get(name)))
        .ok_or_else(|| anyhow::anyhow!("source has no render recipe '{name}'"))?;
    let outputs = root
        .array()
        .context("render recipe must be a list of {name, song, options} records")?;
    ensure!(
        !outputs.is_empty() && outputs.len() <= 34,
        "a render recipe needs 1..34 outputs"
    );
    let mut seen = BTreeSet::new();
    let mut result = PreparedRecipe {
        names: Vec::new(),
        requests: Vec::new(),
    };
    for item in outputs {
        let row = item.record()?;
        for field in row.keys() {
            ensure!(
                ["name", "song", "options"].contains(&field.as_str()),
                "unknown render output field '{field}'"
            );
        }
        let name = row
            .get("name")
            .context("render output needs name")?
            .text()?;
        ensure!(
            !name.is_empty()
                && name.len() <= 120
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "render output names use letters, numbers, hyphens and underscores"
        );
        ensure!(
            seen.insert(name.to_owned()),
            "duplicate render output '{name}'"
        );
        let song = row.get("song").context("render output needs song")?.clone();
        let compiled = compile::lower(song, source, evaluator.dependencies.clone())
            .with_context(|| format!("render output '{name}'"))?;
        ensure!(
            !compiled.diagnostics.iter().any(|d| d.severity == "error"),
            "render output '{name}' failed playing policy checks"
        );
        if let Some(options) = row.get("options") {
            for (field, value) in options.record()? {
                if let crate::lang::Value::Num(q) = value {
                    use crate::lang::Unit;
                    let compatible = match field.as_str() {
                        "start" | "seconds" | "tail" => {
                            matches!(q.unit, Unit::Scalar | Unit::Seconds)
                        }
                        "sample_rate" => matches!(q.unit, Unit::Scalar | Unit::Hz),
                        "block_size" => q.unit == Unit::Scalar,
                        _ => true,
                    };
                    ensure!(compatible, "render option '{field}' has incompatible units");
                }
            }
        }
        let options: RenderOptions = row
            .get("options")
            .map(|v| serde_json::from_value(v.json()))
            .transpose()
            .context("invalid render options")?
            .unwrap_or_default();
        result.names.push(name.to_owned());
        result.requests.push((
            compiled.session,
            output.join(format!("{name}.wav")),
            options,
        ));
    }
    Ok(result)
}

pub fn run(
    source: &Path,
    name: &str,
    output: &Path,
    match_levels: bool,
) -> Result<serde_json::Value> {
    let recipe = prepare(source, name, output)?;
    let reports = crate::control::render_batch(recipe.requests)?;
    std::fs::create_dir_all(output)?;
    let mut rows = Vec::new();
    let mut failed = false;
    for (name, result) in recipe.names.iter().zip(reports) {
        match result {
            Ok(report) => {
                let analysis = if match_levels {
                    Some(crate::analysis::analyze(Path::new(&report.output))?)
                } else {
                    None
                };
                rows.push(
                    serde_json::json!({"name": name, "report": report, "analysis": analysis}),
                );
            }
            Err(error) => {
                failed = true;
                rows.push(serde_json::json!({"name": name, "error": error}));
            }
        }
    }
    // Attenuate to the quietest measurable candidate: comparison gain never
    // changes production processing and never boosts a candidate into clipping.
    let reference = rows
        .iter()
        .filter_map(|r| r["analysis"]["integrated_lufs"].as_f64())
        .reduce(f64::min);
    for row in &mut rows {
        let gain = reference
            .zip(row["analysis"]["integrated_lufs"].as_f64())
            .map(|(a, b)| a - b)
            .unwrap_or(0.0);
        row["listening_gain_db"] = serde_json::json!(gain);
    }
    let summary = serde_json::json!({"source":source,"recipe":name,"outputs":rows,
        "level_matching":if match_levels {"post-render listening gain; silent/too-short candidates remain unmatched"} else {"off"}});
    atomic_write(
        &output.join("renders.json"),
        &serde_json::to_vec_pretty(&summary)?,
    )?;
    atomic_write(
        &output.join("listen.html"),
        player(&rows, match_levels).as_bytes(),
    )?;
    if failed {
        bail!(
            "one or more recipe outputs failed; see {}",
            output.join("renders.json").display()
        );
    }
    Ok(summary)
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.write_all(content)?;
    file.persist(path)?;
    Ok(())
}

fn player(rows: &[serde_json::Value], matched: bool) -> String {
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>Compare bounces</title><style>body{font:18px system-ui;max-width:760px;margin:3em auto;background:#171a22;color:#eee}article{margin:2em 0}audio{width:100%}button{font:inherit}small{color:#aab}</style><h1>Compare bounces</h1><p>Switch candidates at the same playback position. Gain is applied only while listening.</p><label><input id=matching type=checkbox",
    );
    if matched {
        html.push_str(" checked");
    }
    html.push_str("> Match listening levels</label>");
    for row in rows.iter().filter(|r| r.get("report").is_some()) {
        let name = row["name"].as_str().unwrap(); // Validated filename alphabet.
        let gain = row["listening_gain_db"].as_f64().unwrap();
        html.push_str(&format!("<article><button data-name=\"{name}\">Listen to {name}</button> <small>{gain:.2} dB listening adjustment</small><audio controls preload=metadata data-gain=\"{gain}\" src=\"{name}.wav\"></audio></article>"));
    }
    html.push_str("<script>const audios=[...document.querySelectorAll('audio')];let active=null;const matching=document.querySelector('#matching');function gains(){for(const a of audios)a.volume=matching.checked?Math.min(1,Math.pow(10,Number(a.dataset.gain)/20)):1}matching.onchange=gains;gains();for(const a of audios)a.onplay=()=>{for(const b of audios)if(a!==b)b.pause();active=a};for(const b of document.querySelectorAll('button'))b.onclick=()=>{const a=b.parentNode.querySelector('audio');const t=active?active.currentTime:0;if(active)active.pause();a.currentTime=Math.min(t,Number.isFinite(a.duration)?a.duration: t);a.play()};</script>");
    html
}
