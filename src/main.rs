use anyhow::Result;
use clap::{Parser, Subcommand};
use muz::{
    control::{ControlCommand, ControlServer, DEFAULT_SOCKET_PATH, send_command},
    live::LiveSession,
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
    Render {
        source: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        seconds: Option<f64>,
        #[arg(long)]
        section: Option<String>,
        #[arg(long)]
        solo: Vec<String>,
        #[arg(long, default_value_t = 48000)]
        sample_rate: u32,
        #[arg(long, default_value_t = 256)]
        block_size: usize,
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
    Seek {
        #[arg(long)]
        tick: u64,
        #[arg(long,default_value=DEFAULT_SOCKET_PATH)]
        socket: PathBuf,
    },
}
fn main() -> ExitCode {
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
        Command::Check { source, json } => {
            let c = muz::compile::compile(&source)?;
            if json {
                print(
                    serde_json::json!({"ok":true,"tracks":c.score.len(),"diagnostics":c.diagnostics}),
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
            section: _,
            json: _,
        } => {
            let c = muz::compile::compile(&source)?;
            match view.as_str() {
                "score" => print(c.score),
                "graph" => print(c.session),
                "performance" => print(c.session.tracks),
                "diagnostics" | "piano" => print(c.diagnostics),
                _ => anyhow::bail!("view must be score, performance, graph or diagnostics"),
            }
        }
        Command::Render {
            source,
            output,
            seconds,
            section,
            solo,
            sample_rate,
            block_size,
        } => print(muz::render::render(
            &source,
            &output,
            seconds,
            section.as_deref(),
            &solo,
            sample_rate,
            block_size,
        )?),
        Command::Analyze { path, json: _ } => print(muz::analysis::analyze(&path)?),
        Command::Serve {
            source,
            stopped,
            socket,
        } => {
            let mut session = LiveSession::start(&source, !stopped)?;
            let mut control = ControlServer::bind(socket)?;
            eprintln!("{}", serde_json::to_string(&session.status())?);
            loop {
                for ev in session.poll(Duration::from_millis(10))? {
                    eprintln!("{}", serde_json::to_string(&ev)?);
                }
                control.service(&mut session)?;
            }
        }
        Command::Status { socket, json: _ } => client(socket, ControlCommand::Status),
        Command::Play { socket } => client(socket, ControlCommand::Play),
        Command::Stop { socket } => client(socket, ControlCommand::Stop),
        Command::Restart { socket } => client(socket, ControlCommand::Restart),
        Command::Seek { socket, tick } => client(socket, ControlCommand::Seek { tick }),
    }
}
