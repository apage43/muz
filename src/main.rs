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
    /// Create a small editable song and local module.
    Midi {
        #[command(subcommand)]
        command: MidiCommand,
    },
    New {
        directory: PathBuf,
    },
    /// Read the built-in guide (language, production, or workflow).
    Docs {
        #[arg(default_value = "language")]
        topic: String,
    },
    /// Normalize source indentation without changing comments or expressions.
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
    /// Bounce disk source, or submit the server's applied revision with --socket.
    Render {
        source: Option<PathBuf>,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[command(flatten)]
        options: RenderOptions,
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
    Jobs {
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
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
fn run(cli: Cli) -> Result<()> {
    match cli.command {
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
            MidiCommand::Export { source, output } => muz::smf::write(
                &output,
                &muz::smf::export(&muz::compile::compile(&source)?.session)?,
            ),
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
                "performance" => include_str!("../docs/performance.md"),
                "workflow" => include_str!("../README.md"),
                _ => bail!("topic must be language, performance, production or workflow"),
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
                    serde_json::json!({"ok":true,"tracks":c.score.len(),"notes":c.score.iter().map(|t|t.pattern.notes.len()).sum::<usize>(),"diagnostics":c.diagnostics}),
                )
            } else {
                println!(
                    "ok: {} ({} tracks, {} notes)",
                    source.display(),
                    c.score.len(),
                    c.score.iter().map(|t| t.pattern.notes.len()).sum::<usize>()
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
                let host = muz::plugins::open(&path, class.as_deref(), 48000, 256)?;
                print(
                    serde_json::json!({"parameter":parameter,"plain":value,"normalized":host.plain_to_normalized(&parameter,value)?}),
                )
            }
            DeviceCommand::List => print(muz::plugins::installed()),
            DeviceCommand::Inspect { path, class } => {
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
