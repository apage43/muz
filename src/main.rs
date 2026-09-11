use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use muz::{
    control::{ControlCommand, ControlServer, DEFAULT_SOCKET_PATH, send_command},
    live::LiveSession,
    render::RenderOptions,
};
use std::{path::PathBuf, process::ExitCode, time::Duration};
#[derive(Parser)]
#[command(
    name = "muz",
    version,
    about = "Write, perform, produce. A headless music production studio."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Evaluate a source module or musical value without preparing audio.
    Eval { source: PathBuf },
    /// Import, export, or inspect MIDI files.
    Midi {
        #[command(subcommand)]
        command: MidiCommand,
    },
    /// Create a small editable song and local module.
    New { directory: PathBuf },
    /// Read the built-in guide (language, production, synthesis, or workflow).
    Docs {
        #[arg(default_value = "language")]
        topic: String,
    },
    /// Format source with aligned drum lanes, consistent spacing, and line wrapping.
    Fmt {
        source: Vec<PathBuf>,
        #[arg(long)]
        check: bool,
    },
    Check {
        source: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Inspect {
        source: PathBuf,
        #[arg(long, default_value = "score")]
        view: String,
        #[arg(long)]
        section: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Bounce disk source, or queue the server's applied revision with --socket.
    Render {
        source: Option<PathBuf>,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[command(flatten)]
        options: RenderOptions,
    },
    /// Render a named source collection, optionally matching listening levels.
    Batch {
        source: PathBuf,
        recipe: String,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        match_levels: bool,
    },
    /// Export physical tracks after inserts. --wet makes solo auditions through the master.
    Stems {
        source: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        wet: bool,
        #[command(flatten)]
        options: RenderOptions,
    },
    Analyze {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Serve {
        source: PathBuf,
        #[arg(long)]
        stopped: bool,
        #[arg(long)]
        headless: bool,
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    Status {
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Play {
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    Stop {
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    Restart {
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    Panic {
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    Shutdown {
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    Seek {
        #[arg(long)]
        tick: Option<u64>,
        #[arg(long)]
        beat: Option<f64>,
        #[arg(long)]
        section: Option<String>,
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    Loop {
        section: Option<String>,
        #[arg(long)]
        start: Option<f64>,
        #[arg(long)]
        end: Option<f64>,
        #[arg(long)]
        off: bool,
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    /// Loop a named section and start playback.
    Audition {
        section: String,
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    /// Show queued/running bounces and their completion, source and revision.
    Jobs {
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    /// Cancel a queued or running bounce, preserving existing output.
    Cancel {
        id: u64,
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    /// Send a newline JSON API command. All commands are also available to socket clients.
    Call {
        request: String,
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
    Devices {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    #[command(hide = true)]
    RenderWorker {
        input: PathBuf,
        report: PathBuf,
        progress: PathBuf,
    },
}
#[derive(Subcommand)]
enum DeviceCommand {
    List,
    /// Convert a plugin's displayed/plain value to the source's normalized value.
    Convert {
        path: PathBuf,
        parameter: String,
        value: f64,
        #[arg(long)]
        class: Option<String>,
    },
    Inspect {
        path: PathBuf,
        #[arg(long)]
        class: Option<String>,
    },
    State {
        path: PathBuf,
        #[arg(long)]
        class: Option<String>,
        #[arg(long)]
        load: Option<PathBuf>,
        #[arg(short, long)]
        output: PathBuf,
    },
}
#[derive(Subcommand)]
enum MidiCommand {
    Inspect {
        path: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Encode {
        path: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    Export {
        source: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
}
fn main() -> ExitCode {
    if let Err(e) =
        ctrlc::set_handler(|| muz::INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed))
    {
        eprintln!("muz: cannot install signal handler: {e}");
    }
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("muz: {e:#}");
            ExitCode::FAILURE
        }
    }
}
fn print(v: impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}
fn client(socket: PathBuf, c: ControlCommand) -> Result<()> {
    let r = send_command(socket, c)?;
    println!("{}", r.json());
    r.into_result()?;
    Ok(())
}
/// Device commands take a bundle path or the name of a user alias, matching the
/// alias sources already resolve through `plugins.json`.
fn resolve_device(path: &std::path::Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    match muz::plugins::alias_path(&path.to_string_lossy())? {
        Some(resolved) => Ok(resolved),
        None => Ok(path.to_path_buf()),
    }
}
fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Eval { source } => print(muz::lang::load(&source)?.0.json()),
        Command::Midi { command } => match command {
            MidiCommand::Inspect { path, output } => {
                let doc = muz::smf::read(&path)?;
                if let Some(out) = output {
                    std::fs::write(out, serde_json::to_string_pretty(&doc)?)?;
                    Ok(())
                } else {
                    print(doc)
                }
            }
            MidiCommand::Encode { path, output } => {
                muz::smf::write(&output, &serde_json::from_slice(&std::fs::read(path)?)?)
            }
            MidiCommand::Export { source, output } => {
                let (value, deps) = muz::lang::load(&source)?;
                let doc = if let Ok(pattern) = value.pattern() {
                    muz::smf::export_pattern(pattern, 120.)?
                } else {
                    {
                        let c = muz::compile::lower(value, &source, deps)?;
                        if c.diagnostics.iter().any(|d| d.severity == "error") {
                            bail!("source fails its playing policy; run muz check for diagnostics")
                        }
                        muz::smf::export(&c.session)?
                    }
                };
                muz::smf::write(&output, &doc)
            }
        },
        Command::New { directory } => {
            std::fs::create_dir_all(&directory)?;
            let p = directory.join("song.muz");
            if p.exists() {
                bail!("{} already exists", p.display());
            }
            std::fs::write(&p, include_str!("../templates/song.muz"))?;
            print(serde_json::json!({"source":p}))
        }
        Command::Docs { topic } => {
            let text = match topic.as_str() {
                "language" => include_str!("../docs/language.md"),
                "production" => include_str!("../docs/production.md"),
                "synthesis" => include_str!("../docs/synthesis.md"),
                "performance" => include_str!("../docs/performance.md"),
                "workflow" => include_str!("../README.md"),
                _ => {
                    bail!("topic must be language, performance, production, synthesis or workflow")
                }
            };
            println!("{text}");
            Ok(())
        }
        Command::Fmt { source, check } => {
            if source.is_empty() {
                bail!("give at least one source file");
            }
            for p in source {
                let old = std::fs::read_to_string(&p)?;
                let new = muz::lang::format::format(&old)?;
                if old != new {
                    if check {
                        bail!("{} needs formatting", p.display());
                    }
                    std::fs::write(p, new)?;
                }
            }
            Ok(())
        }
        Command::Check { source, json } => {
            let c = muz::compile::compile(&source)?;
            let _prepared = muz::audio::AudioEngine::new(
                &c.session,
                muz::audio::AudioConfig {
                    sample_rate: 48000.,
                    max_frames: 256,
                },
            )?;
            if json {
                print(
                    serde_json::json!({"ok":true,"tracks":c.score.len(),"notes":c.score.iter().map(|t|t.pattern.notes.len()).sum::<usize>(),"diagnostics":c.diagnostics,"graph":c.session.graph_resources(),"graph_budget":muz::model::graph_budget().map_err(anyhow::Error::msg)?}),
                )
            } else {
                println!(
                    "ok: {} ({} tracks, {} notes)",
                    source.display(),
                    c.score.len(),
                    c.score.iter().map(|t| t.pattern.notes.len()).sum::<usize>()
                );
                let r = c.session.graph_resources();
                println!(
                    "expanded graph: {} tracks, {} devices, {} buses, {} routes, {} sample zones; {}/{} resource units",
                    r.tracks,
                    r.devices,
                    r.buses,
                    r.routes,
                    r.sample_zones,
                    r.units,
                    muz::model::graph_budget().map_err(anyhow::Error::msg)?
                );
                println!(
                    "main contributors: {}",
                    r.contributors
                        .iter()
                        .map(|(name, n)| format!("{name}: {n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                for d in c.diagnostics {
                    eprintln!("{}: {} beat {}: {}", d.code, d.track, d.beat, d.message);
                }
                Ok(())
            }
        }
        Command::Inspect {
            source,
            view,
            section,
            json: _,
        } => {
            let mut c = muz::compile::inspect(&source)?;
            if let Some(name) = section {
                let sec = c
                    .session
                    .extras
                    .sections
                    .iter()
                    .find(|s| s.name == name)
                    .ok_or_else(|| anyhow::anyhow!("unknown section {name}"))?;
                let (a, b) = (sec.start, sec.end);
                for t in &mut c.score {
                    t.pattern.notes.retain(|n| {
                        muz::music::real(n.at) < b && muz::music::real(n.at + n.dur) > a
                    });
                }
                c.diagnostics.retain(|d| d.beat >= a && d.beat < b);
                for t in &mut c.session.tracks {
                    if let muz::model::TrackSource::Midi(m) = &mut t.source {
                        m.imported.notes.retain(|n| {
                            n.start_tick < muz::compile::tick(b)
                                && n.start_tick + n.duration_ticks > muz::compile::tick(a)
                        });
                        m.imported.controllers.retain(|n| {
                            n.tick >= muz::compile::tick(a) && n.tick < muz::compile::tick(b)
                        });
                    }
                }
            }
            match view.as_str() {
                "score" => print(c.score),
                "diagnostics" | "piano" => print(c.diagnostics),
                _ => print(muz::inspect::session(&c.session, &view)?),
            }
        }
        Command::Render {
            source,
            output,
            socket,
            options,
        } => {
            if let Some(socket) = socket {
                if source.is_some() {
                    bail!("choose disk source or --socket for the applied revision");
                }
                client(socket, ControlCommand::Render { output, options })
            } else {
                let source =
                    source.ok_or_else(|| anyhow::anyhow!("render needs a source or --socket"))?;
                let c = muz::compile::compile(&source)?;
                print(muz::render::render_with(
                    c.session, &output, &options, None,
                )?)
            }
        }
        Command::Batch {
            source,
            recipe,
            output,
            match_levels,
        } => print(muz::recipes::run(&source, &recipe, &output, match_levels)?),
        Command::Stems {
            source,
            output,
            wet,
            mut options,
        } => {
            let c = muz::compile::compile(&source)?;
            if !wet {
                return print(muz::render::stems(c.session, &output, &options)?);
            }
            std::fs::create_dir_all(&output)?;
            let mut reports = vec![];
            for t in &c.session.tracks {
                let id = t.id.as_str().to_owned();
                if wet {
                    options.solo = vec![id.clone()];
                    options.tap = None;
                } else {
                    options.solo.clear();
                    options.tap = Some(id.clone());
                }
                reports.push(muz::render::render_with(
                    c.session.clone(),
                    &output.join(format!("{id}.wav")),
                    &options,
                    None,
                )?);
            }
            std::fs::write(
                output.join("README.txt"),
                if wet {
                    "Solo auditions include returns and nonlinear master processing; they are not summable stems.\n"
                } else {
                    "Physical-track taps are after inserts and before output gain/sends/master. Shared returns can be exported with render --tap BUS.\n"
                },
            )?;
            print(reports)
        }
        Command::Analyze { path, json: _ } => print(muz::analysis::analyze(&path)?),
        Command::Serve {
            source,
            stopped,
            headless,
            socket,
        } => {
            let mut session =
                LiveSession::start_backend(&source, !stopped, Duration::from_millis(20), headless)?;
            let mut control = ControlServer::bind(&socket)?;
            eprintln!(
                "muz serving {} on {} ({})",
                source.display(),
                socket.display(),
                if headless { "silent clock" } else { "PipeWire" }
            );
            loop {
                for ev in session.poll(Duration::from_millis(10))? {
                    eprintln!("{}", serde_json::to_string(&ev)?);
                }
                control.service(&mut session)?;
                if control.shutdown || muz::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
            }
            Ok(())
        }
        Command::Status { socket, json: _ } => client(socket, ControlCommand::Status),
        Command::Play { socket } => client(socket, ControlCommand::Play),
        Command::Stop { socket } => client(socket, ControlCommand::Stop),
        Command::Restart { socket } => client(socket, ControlCommand::Restart),
        Command::Panic { socket } => client(socket, ControlCommand::Panic),
        Command::Shutdown { socket } => client(socket, ControlCommand::Shutdown),
        Command::Seek {
            socket,
            tick,
            beat,
            section,
        } => client(
            socket,
            ControlCommand::Seek {
                tick,
                beat,
                section,
            },
        ),
        Command::Loop {
            socket,
            section,
            start,
            end,
            off,
        } => client(
            socket,
            ControlCommand::Loop {
                section,
                start,
                end,
                off,
            },
        ),
        Command::Audition { socket, section } => {
            client(
                socket.clone(),
                ControlCommand::Loop {
                    section: Some(section),
                    start: None,
                    end: None,
                    off: false,
                },
            )?;
            client(socket, ControlCommand::Play)
        }
        Command::Jobs { socket } => client(socket, ControlCommand::Jobs),
        Command::Cancel { socket, id } => client(socket, ControlCommand::Cancel { id }),
        Command::Call { socket, request } => client(socket, serde_json::from_str(&request)?),
        Command::Devices { command } => match command {
            DeviceCommand::Convert {
                path,
                parameter,
                value,
                class,
            } => {
                let path = resolve_device(&path)?;
                if muz::plugins::is_clap(&path) {
                    bail!("CLAP parameters already use plain values; inspect their min/max ranges");
                }
                let host = muz::plugins::open(&path, class.as_deref(), 48000, 256)?;
                print(
                    serde_json::json!({"parameter":parameter,"plain":value,"normalized":host.plain_to_normalized(&parameter,value)?}),
                )
            }
            DeviceCommand::List => print(
                serde_json::json!({"native":muz::plugins::native_names(),"plugins":muz::plugins::installed()}),
            ),
            DeviceCommand::Inspect { path, class } => {
                let path = resolve_device(&path)?;
                if let Some(native) = muz::plugins::native(&path.to_string_lossy()) {
                    return print(native);
                }
                if muz::plugins::is_clap(&path) {
                    let host = muz::plugins::open_clap(&path, class.as_deref(), 48000, 256)?;
                    return print(
                        serde_json::json!({"metadata":host.metadata(),"parameters":host.parameters()}),
                    );
                }
                let host = muz::plugins::open(&path, class.as_deref(), 48000, 256)?;
                print(
                    serde_json::json!({"metadata":host.metadata(),"parameters":host.parameters()}),
                )
            }
            DeviceCommand::State {
                path,
                class,
                load,
                output,
            } => {
                let path = resolve_device(&path)?;
                if muz::plugins::is_clap(&path) {
                    let mut host = muz::plugins::open_clap(&path, class.as_deref(), 48000, 256)?;
                    if let Some(load) = load {
                        host.load_state(&load)?;
                    }
                    host.save_state(&output)?;
                    return print(serde_json::json!({"state":output}));
                }
                let mut host = muz::plugins::open(&path, class.as_deref(), 48000, 256)?;
                if let Some(load) = load {
                    host.load_state(&load)?;
                }
                host.save_state(&output)?;
                print(serde_json::json!({"state":output}))
            }
        },
        Command::RenderWorker {
            input,
            report,
            progress,
        } => muz::worker::run(&input, &report, &progress),
    }
}
