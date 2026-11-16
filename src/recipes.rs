//! Named output collections are source data; execution reuses the render queue.
use crate::{
    compile,
    lang::{Diagnostic, Evaluator, suggest_vocabulary},
    render::RenderOptions,
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub struct PreparedRecipe {
    pub names: Vec<String>,
    pub requests: Vec<(crate::Session, PathBuf, RenderOptions)>,
    /// Per output, the tracks whose faders level matching may move (`options.match`).
    pub matches: Vec<Option<Match>>,
}

/// An output's `match`: what it named and the tracks whose faders move together.
#[derive(Clone)]
pub struct Match {
    /// The option as written: a track or group name, or a list of them.
    pub named: serde_json::Value,
    /// The tracks it covers, in session order, each once.
    pub tracks: Vec<String>,
}

/// The tracks one `match` name covers: the track with that id, or else every
/// track of the group `NAME.*` (a kit's voices, an expanded part), as a group
/// tap sums them. Probe copies (`ID@PROBE`) belong to no group.
fn match_members(session: &crate::Session, name: &str) -> Vec<String> {
    if session.tracks.iter().any(|t| t.id.as_str() == name) {
        return vec![name.to_owned()];
    }
    session
        .tracks
        .iter()
        .map(|t| t.id.as_str())
        .filter(|id| {
            !id.contains('@')
                && id
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
        .map(str::to_owned)
        .collect()
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
        matches: Vec::new(),
    };
    for item in outputs {
        let row = item.record()?;
        for field in row.keys() {
            if !["name", "song", "options"].contains(&field.as_str()) {
                return Err(
                    Diagnostic::new(format!("unknown render output field '{field}'"))
                        .helps(suggest_vocabulary(
                            "fields",
                            field,
                            ["name", "song", "options"],
                        ))
                        .err(),
                );
            }
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
        if !seen.insert(name.to_owned()) {
            let existing: Vec<&str> = seen.iter().map(String::as_str).collect();
            return Err(Diagnostic::new(format!("duplicate render output '{name}'"))
                .help(format!(
                    "output names already used: {}",
                    existing.join(", ")
                ))
                .err());
        }
        let song = row.get("song").context("render output needs song")?.clone();
        let compiled = compile::lower(song, source, evaluator.dependencies.clone())
            .with_context(|| format!("render output '{name}'"))?;
        ensure!(
            !compiled.diagnostics.iter().any(|d| d.severity == "error"),
            "render output '{name}' failed playing policy checks"
        );
        if let Some(options) = row.get("options") {
            for (field, value) in options.record()?.iter() {
                if let crate::lang::Value::Num(q) = value {
                    use crate::lang::Unit;
                    let compatible = match field.as_str() {
                        "start" | "seconds" | "tail" | "history" => {
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
        let mut options_json = row.get("options").map(|value| value.json());
        let matched = match options_json
            .as_mut()
            .and_then(|o| o.as_object_mut())
            .and_then(|o| o.remove("match"))
        {
            None => None,
            Some(named) => {
                let names: Vec<String> = match &named {
                    serde_json::Value::String(one) => vec![one.clone()],
                    serde_json::Value::Array(list)
                        if !list.is_empty() && list.iter().all(|v| v.is_string()) =>
                    {
                        list.iter()
                            .map(|v| v.as_str().unwrap_or_default().to_owned())
                            .collect()
                    }
                    _ => bail!(
                        "render output '{name}': match names a track or group, or a list of them"
                    ),
                };
                let session = &compiled.session;
                let mut tracks = Vec::<String>::new();
                for track in &names {
                    let members = match_members(session, track);
                    if members.is_empty() {
                        return Err(Diagnostic::new(format!(
                            "render output '{name}': match names no track or group '{track}'"
                        ))
                        .helps(suggest_vocabulary(
                            "tracks",
                            track,
                            session.tracks.iter().map(|t| t.id.as_str()),
                        ))
                        .help("match names a track, a group of tracks NAME.* (a kit), or a list of them")
                        .err());
                    }
                    for member in members {
                        if tracks.contains(&member) {
                            bail!("render output '{name}': match covers track '{member}' twice");
                        }
                        tracks.push(member);
                    }
                }
                for track in &tracks {
                    let lane = format!("{track}.out");
                    if session.extras.automation.iter().any(|a| a.target == lane) {
                        return Err(Diagnostic::new(format!(
                            "render output '{name}': match track '{track}' has an automated output fader"
                        ))
                        .help("level matching moves a static fader; automate a gain insert instead, or remove the lane")
                        .err());
                    }
                }
                tracks.sort_by_key(|t| session.tracks.iter().position(|s| s.id.as_str() == t));
                Some(Match { named, tracks })
            }
        };
        let options: RenderOptions = match options_json {
            Some(value) => serde_json::from_value(value).map_err(|error| {
                Diagnostic::new(format!("invalid render options: {error:#}"))
                    .help(
                        "start, seconds, tail and history take a scalar or seconds value; \
                         sample_rate takes a scalar or Hz value; block_size takes a scalar",
                    )
                    .err()
            })?,
            None => RenderOptions::default(),
        };
        result.names.push(name.to_owned());
        result.matches.push(matched);
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
    let mut recipe = prepare(source, name, output)?;
    let faders = match_faders(&mut recipe)?;
    let reports = crate::control::render_batch(recipe.requests)?;
    std::fs::create_dir_all(output)?;
    let mut rows = Vec::new();
    let mut failed = false;
    for (name, result) in recipe.names.iter().zip(reports) {
        match result {
            Ok(report) => {
                // A several-tap output is matched by its master tap, if it has one.
                let audio = if report.taps.is_empty() {
                    Some(report.output.clone())
                } else {
                    report
                        .taps
                        .iter()
                        .find(|t| t.scope.tap.first().is_some_and(|tap| tap == "master"))
                        .map(|t| t.output.clone())
                };
                let analysis = match audio {
                    Some(audio) if match_levels => {
                        Some(crate::analysis::analyze(Path::new(&audio))?)
                    }
                    _ => None,
                };
                let mut row =
                    serde_json::json!({"name": name, "report": report, "analysis": analysis});
                if let Some(fader) = faders.get(name) {
                    row["match"] = fader.clone();
                }
                rows.push(row);
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

/// Level-match outputs that name a `match` before they render. Each such
/// output's tracks are measured together after their faders (their summed
/// post-fader taps, before sends, mute and solo) in that output's own render
/// scope; their static faders then move by one offset so that sum's
/// integrated loudness equals the quietest among the outputs matching the same
/// tracks. Nothing else in the song changes, so solo and in-context outputs of
/// one candidate receive the same offset.
fn match_faders(
    recipe: &mut PreparedRecipe,
) -> Result<std::collections::BTreeMap<String, serde_json::Value>> {
    let mut faders = std::collections::BTreeMap::new();
    let matched: Vec<(usize, Match)> = recipe
        .matches
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.clone().map(|m| (i, m)))
        .collect();
    if matched.is_empty() {
        return Ok(faders);
    }
    let scratch = tempfile::tempdir()?;
    let requests = matched
        .iter()
        .map(|(i, m)| {
            let (session, _, options) = &recipe.requests[*i];
            let mut options = options.clone();
            options.solo.clear();
            options.mute.clear();
            options.tap = m.tracks.iter().map(|t| format!("{t}:post")).collect();
            options.format = "float32".into();
            let out = scratch.path().join(&recipe.names[*i]);
            let out = if m.tracks.len() == 1 {
                out.with_extension("wav")
            } else {
                out
            };
            (session.clone(), out, options)
        })
        .collect();
    let mut levels = vec![];
    for ((i, m), report) in matched.iter().zip(crate::control::render_batch(requests)?) {
        let name = &recipe.names[*i];
        let named = m.named.to_string();
        let report =
            report.map_err(|error| anyhow::anyhow!("measuring {named} for '{name}': {error}"))?;
        let measured = if report.taps.is_empty() {
            PathBuf::from(&report.output)
        } else {
            let sum = scratch.path().join(format!("{name}.sum.wav"));
            sum_wavs(report.taps.iter().map(|t| Path::new(&t.output)), &sum)?;
            sum
        };
        let lufs = crate::analysis::analyze(&measured)?["integrated_lufs"]
            .as_f64()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "render output '{name}': match {named} has no measurable loudness in its render scope; \
                     lengthen the scope or remove match"
                )
            })?;
        let session = &recipe.requests[*i].0;
        let before: Vec<f64> = m
            .tracks
            .iter()
            .map(|track| {
                session
                    .tracks
                    .iter()
                    .find(|t| t.id.as_str() == track)
                    .map(|t| f64::from(t.output.gain_db))
                    .expect("validated match track")
            })
            .collect();
        levels.push((*i, m, lufs, before));
    }
    for (i, m, lufs, before) in &levels {
        let target = levels
            .iter()
            .filter(|l| l.1.tracks == m.tracks)
            .map(|l| l.2)
            .fold(f64::INFINITY, f64::min);
        let offset = target - lufs;
        let session = &mut recipe.requests[*i].0;
        for (track, fader) in m.tracks.iter().zip(before) {
            let out = &mut session
                .tracks
                .iter_mut()
                .find(|t| t.id.as_str() == track)
                .expect("validated match track")
                .output;
            out.gain_db = (fader + offset) as f32;
            ensure!(
                out.gain_db >= -120.0,
                "render output '{}': matching '{track}' needs a fader below -120 dB",
                recipe.names[*i]
            );
        }
        let record = if let [fader] = before[..] {
            serde_json::json!({
                "track": m.named,
                "tap_lufs": lufs - fader,
                "fader_db": fader,
                "offset_db": offset,
                "matched_fader_db": fader + offset,
            })
        } else {
            serde_json::json!({
                "track": m.named,
                "tracks": m.tracks,
                "post_lufs": lufs,
                "faders_db": before,
                "offset_db": offset,
                "matched_faders_db": before.iter().map(|f| f + offset).collect::<Vec<_>>(),
            })
        };
        faders.insert(recipe.names[*i].clone(), record);
    }
    Ok(faders)
}

/// Sum float tap files of one pass (same rate, channels and length) into `out`.
fn sum_wavs<'a>(paths: impl Iterator<Item = &'a Path>, out: &Path) -> Result<()> {
    let mut spec = None;
    let mut sum: Vec<f32> = Vec::new();
    for path in paths {
        let mut reader = hound::WavReader::open(path)?;
        ensure!(
            *spec.get_or_insert(reader.spec()) == reader.spec(),
            "tap files of one pass differ in format"
        );
        let samples: Vec<f32> = reader.samples::<f32>().collect::<Result<_, _>>()?;
        if sum.len() < samples.len() {
            sum.resize(samples.len(), 0.0);
        }
        for (s, x) in sum.iter_mut().zip(samples) {
            *s += x;
        }
    }
    let mut writer = hound::WavWriter::create(out, spec.context("no tap files to sum")?)?;
    for s in sum {
        writer.write_sample(s)?;
    }
    writer.finalize()?;
    Ok(())
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
        let taps: Vec<&str> = row["report"]["taps"]
            .as_array()
            .map(|taps| taps.iter().filter_map(|t| t["scope"]["tap"].as_str()).collect())
            .unwrap_or_default();
        if taps.is_empty() {
            html.push_str(&format!("<article><button data-name=\"{name}\">Listen to {name}</button> <small>{gain:.2} dB listening adjustment</small><audio controls preload=metadata data-gain=\"{gain}\" src=\"{name}.wav\"></audio></article>"));
        }
        for tap in taps {
            // Tap names are track and bus ids (letters, digits, `._-:`); `:` is
            // escaped so the path never reads as a URL scheme.
            let file = tap.replace(':', "%3A");
            let label = html_text(tap);
            html.push_str(&format!("<article><button data-name=\"{name}\">Listen to {name}: {label}</button> <small>{gain:.2} dB listening adjustment</small><audio controls preload=metadata data-gain=\"{gain}\" src=\"{name}/{file}.wav\"></audio></article>"));
        }
    }
    html.push_str("<script>const audios=[...document.querySelectorAll('audio')];let active=null;const matching=document.querySelector('#matching');function gains(){for(const a of audios)a.volume=matching.checked?Math.min(1,Math.pow(10,Number(a.dataset.gain)/20)):1}matching.onchange=gains;gains();for(const a of audios)a.onplay=()=>{for(const b of audios)if(a!==b)b.pause();active=a};for(const b of document.querySelectorAll('button'))b.onclick=()=>{const a=b.parentNode.querySelector('audio');const t=active?active.currentTime:0;if(active)active.pause();a.currentTime=Math.min(t,Number.isFinite(a.duration)?a.duration: t);a.play()};</script>");
    html
}

fn html_text(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
