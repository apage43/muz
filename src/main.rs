use anyhow::{Result, bail};
// Standard output may be a pipe that its reader closes early (`| head -1`,
// `grep -q`). Rust ignores SIGPIPE, so `std::println!` panics there; these
// stop writing quietly instead and leave the exit status to the command.
macro_rules! println {
    ($($t:tt)*) => { write_stdout(format_args!("{}\n", format_args!($($t)*))) };
}
macro_rules! print {
    ($($t:tt)*) => { write_stdout(format_args!($($t)*)) };
}
fn write_stdout(args: std::fmt::Arguments) {
    use std::io::Write;
    static CLOSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if CLOSED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let mut out = std::io::stdout().lock();
    match out.write_fmt(args).and_then(|()| out.flush()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
            CLOSED.store(true, std::sync::atomic::Ordering::Relaxed)
        }
        Err(e) => panic!("failed printing to stdout: {e}"),
    }
}
use clap::{Parser, Subcommand};
use muz::{
    control::{ControlCommand, ControlServer, DEFAULT_SOCKET_PATH, send_command},
    lang::{Diagnostic, suggest_vocabulary},
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
    /// Export an editable DAWProject handoff with a fidelity report.
    Export {
        source: PathBuf,
        #[arg(long, default_value = "dawproject")]
        format: String,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long, default_value = "bitwig-linux")]
        profile: String,
    },
    /// Evaluate a source module or musical value without preparing audio.
    ///
    /// SOURCE is a module path, or `-` to read the module from standard input
    /// (relative `use` paths then resolve against the current directory). With
    /// --expr, print EXPR evaluated in the module's top-level scope instead of the
    /// module's own value; the expression sees its bindings and `use` names, and
    /// relative `use` paths resolve against the module's directory. The module
    /// then evaluates as if imported: `is_entry()` is false in it.
    Eval {
        source: PathBuf,
        #[arg(long, value_name = "EXPR")]
        expr: Option<String>,
    },
    /// Import, export, or inspect MIDI files.
    Midi {
        #[command(subcommand)]
        command: MidiCommand,
    },
    /// Create a small editable song and local module.
    New { directory: PathBuf },
    /// Read bundled documentation; omit TOPIC for the guide index.
    Docs {
        #[arg(default_value = "index", long_help = docs_topic_help())]
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
        #[arg(long, default_value = "summary")]
        view: String,
        #[arg(long)]
        section: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        track: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        start_tick: Option<u64>,
        #[arg(long)]
        end_tick: Option<u64>,
    },
    /// Bounce disk source, or queue the server's applied revision with --socket.
    Render {
        source: Option<PathBuf>,
        #[arg(short, long, required_unless_present = "list_taps")]
        output: Option<PathBuf>,
        /// Print every name --tap accepts (tracks, kit groups, buses, master) and exit.
        /// A group such as a kit's `drums` taps the sum of its `drums.*` tracks.
        #[arg(long)]
        list_taps: bool,
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
        /// Export only this physical track or kit group (NAME.*); repeat for more.
        /// Dry stems still process the sidechain sources the selection reads.
        #[arg(long = "track", value_name = "TRACK")]
        tracks: Vec<String>,
        #[command(flatten)]
        options: RenderOptions,
    },
    Analyze {
        path: PathBuf,
        #[arg(long)]
        json: bool,
        /// Also list sample and true peak for every window of this many seconds.
        #[arg(long, value_name = "SECONDS", conflicts_with = "bpm")]
        window: Option<f64>,
        /// Also list sample and true peak for every bar at this constant tempo
        /// (bar 1 starts at 0 s).
        #[arg(long, value_name = "BPM")]
        bpm: Option<f64>,
        /// Beats per bar for --bpm.
        #[arg(long, value_name = "BEATS", default_value_t = 4.0)]
        beats_per_bar: f64,
        /// Also list sample and true peak for every bar of this song, following
        /// its tempo and meter maps (bar 1 starts at 0 s).
        #[arg(long, value_name = "SONG", conflicts_with_all = ["bpm", "window"])]
        song: Option<PathBuf>,
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
    /// Replace the standalone Muz source in an exported native plugin state.
    ReplaceSource {
        state: PathBuf,
        source: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
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
        /// A state to start from: a CLAP state, or a VST3 state or .vstpreset.
        #[arg(long, conflicts_with = "preset")]
        load: Option<PathBuf>,
        /// A CLAP plugin's own preset file, loaded through its preset loader.
        #[arg(long)]
        preset: Option<PathBuf>,
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
        /// Write each kit as one track under its own name instead of one
        /// track per voice; choke groups are then not written.
        #[arg(long)]
        merge_kits: bool,
    },
}
// Keep offline lookup, help, and diagnostics on the same topic registry.
const DOC_TOPICS: &[(&str, &str)] = &[
    ("index", include_str!("../docs/README.md")),
    (
        "getting-started",
        include_str!("../docs/getting-started.md"),
    ),
    ("language", include_str!("../docs/language.md")),
    ("performance", include_str!("../docs/performance.md")),
    ("instruments", include_str!("../docs/instruments.md")),
    ("production", include_str!("../docs/production.md")),
    ("synthesis", include_str!("../docs/synthesis.md")),
    ("workflow", include_str!("../docs/workflow.md")),
    ("dawproject", include_str!("../docs/dawproject.md")),
    (
        "dawproject-validation",
        include_str!("../docs/dawproject-validation.md"),
    ),
    ("embedding", include_str!("../docs/embedding.md")),
];

fn docs_topic_help() -> String {
    format!(
        "Guide to display. Available topics: {}",
        DOC_TOPICS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn documentation(topic: &str) -> Result<&'static str> {
    DOC_TOPICS
        .iter()
        .find_map(|(name, text)| (*name == topic).then_some(*text))
        .ok_or_else(|| {
            Diagnostic::new(format!("unknown documentation topic '{topic}'"))
                .helps(suggest_vocabulary(
                    "topics",
                    topic,
                    DOC_TOPICS.iter().map(|(name, _)| *name),
                ))
                .err()
        })
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
            if let Some(diagnostic) = e.chain()
                .find_map(|cause| cause.downcast_ref::<muz::lang::Diagnostic>())
            {
                eprintln!("muz: {diagnostic}");
            } else {
                eprintln!("muz: {e:#}");
            }
            ExitCode::FAILURE
        }
    }
}
/// A load that leaves the plugin's state byte-identical did not apply the file,
/// unless the file is that state.
fn state_changed(before: &[u8], after: &[u8], file: &std::path::Path, state: bool) -> Result<()> {
    if after != before || (state && std::fs::read(file)? == before) {
        return Ok(());
    }
    bail!(
        "the plugin's state is unchanged after loading {}: it did not apply the file{}",
        file.display(),
        if state {
            "; --load passes a state the plugin saved (CLAP) or a VST3 state or .vstpreset; load a CLAP plugin's own preset files with --preset"
        } else {
            " (or the preset equals the plugin's current state)"
        }
    )
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
        Command::Export {
            source,
            format,
            output,
            strict,
            report,
            profile,
        } => {
            if format != "dawproject" {
                bail!("unsupported export format '{format}'");
            }
            let c = muz::compile::compile(&source)?;
            let plan = muz::dawproject::Plan::new(&c, &profile)?;
            for entry in plan.report.warnings() {
                eprintln!(
                    "warning [{}] {}: {} -> {}",
                    entry.code, entry.physical_path, entry.intended, entry.exported
                );
            }
            let report_path =
                report.unwrap_or_else(|| output.with_extension("dawproject.report.json"));
            plan.write(&output, strict, Some(&report_path))?;
            println!("{}", output.display());
            Ok(())
        }
        Command::Eval { source, expr } => {
            let (path, text) = if source.as_os_str() == "-" {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
                (std::env::current_dir()?.join("<stdin>"), Some(text))
            } else {
                (source, None)
            };
            print(muz::lang::evaluate(&path, text.as_deref(), expr.as_deref())?.display_json())
        }
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
            MidiCommand::Export {
                source,
                output,
                merge_kits,
            } => {
                let (value, deps) = muz::lang::load(&source)?;
                let doc = if let Ok(pattern) = value.pattern() {
                    muz::smf::export_pattern(pattern, 120.)?
                } else {
                    {
                        let c = muz::compile::validate_compiled(
                            muz::compile::lower(value, &source, deps)?,
                            &source,
                        )?;
                        muz::smf::export_with(&c.session, merge_kits)?
                    }
                };
                muz::smf::write(&output, &doc)
            }
        },
        Command::New { directory } => {
            std::fs::create_dir_all(&directory)?;
            let p = directory.join("song.muz");
            if p.exists() {
                return Err(Diagnostic::new(format!("{} already exists", p.display()))
                    .help("choose another directory or delete the existing file")
                    .err());
            }
            std::fs::write(&p, include_str!("../templates/song.muz"))?;
            print(serde_json::json!({"source":p}))
        }
        Command::Docs { topic } => {
            println!("{}", documentation(&topic)?);
            Ok(())
        }
        Command::Fmt { source, check } => {
            if source.is_empty() {
                bail!("give at least one source file");
            }
            for p in source {
                let old = std::fs::read_to_string(&p)?;
                let new =
                    muz::lang::format::format(&old).map_err(|e| muz::lang::syntax(&p, &old, e))?;
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
                    offline: false,
                },
            )?;
            if json {
                print(
                    serde_json::json!({"ok":true,"tracks":c.score.len(),"notes":c.score.iter().map(|t|t.pattern.notes.len()).sum::<usize>(),"diagnostics":c.diagnostics,"graph":c.session.graph_resources(),"graph_budget":muz::model::graph_budget().map_err(anyhow::Error::msg)?,"voice_patches":muz::description::voice_patch_nodes(&c.session).into_iter().map(|(device, nodes)| serde_json::json!({"device":device,"nodes":nodes})).collect::<Vec<_>>(),"voice_patch_node_limit":muz::patch_description::MAX_NODES}),
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
                for (device, nodes) in muz::description::voice_patch_nodes(&c.session) {
                    println!(
                        "voice patch {device}: {nodes}/{} nodes",
                        muz::patch_description::MAX_NODES
                    );
                }
                let transport = &c.session.transport;
                for d in &c.diagnostics {
                    eprintln!(
                        "{}: {} {}: {}",
                        d.code,
                        d.track,
                        d.position(transport),
                        d.message
                    );
                }
                Ok(())
            }
        }
        Command::Inspect {
            source,
            view,
            section,
            json: _,
            track,
            offset,
            limit,
            mut start_tick,
            mut end_tick,
        } => {
            let c = muz::compile::inspect(&source)?;
            if let Some(name) = section {
                let helps = if c.session.extras.sections.is_empty() {
                    Vec::new()
                } else {
                    suggest_vocabulary(
                        "sections",
                        &name,
                        c.session.extras.sections.iter().map(|s| s.name.as_str()),
                    )
                };
                let sec = c
                    .session
                    .extras
                    .sections
                    .iter()
                    .find(|s| s.name == name)
                    .ok_or_else(|| {
                        Diagnostic::new(format!("unknown section {name}"))
                            .helps(helps)
                            .err()
                    })?;
                start_tick = Some(muz::compile::tick(sec.start));
                end_tick = Some(muz::compile::tick(sec.end));
            }
            let view = if view == "piano" {
                "diagnostics"
            } else {
                &view
            };
            let request = muz::inspect::PageRequest {
                revision: None,
                offset,
                limit,
                track,
                start_tick,
                end_tick,
            };
            print(muz::inspect::page_compiled(&c, 0, view, &request)?)
        }
        Command::Render {
            source,
            output,
            list_taps,
            socket,
            options,
        } => {
            if list_taps {
                let source = source.ok_or_else(|| anyhow::anyhow!("--list-taps needs a source"))?;
                let c = muz::compile::compile(&source)?;
                print!("{}", muz::render::list_taps(&c.session));
                return Ok(());
            }
            let output = output.expect("clap requires -o unless --list-taps");
            if let Some(socket) = socket {
                if source.is_some() {
                    bail!("choose disk source or --socket for the applied revision");
                }
                if options.tap.len() > 1 {
                    bail!("a queued render takes one --tap; render several taps from disk source");
                }
                client(socket, ControlCommand::Render { output, options })
            } else {
                let source =
                    source.ok_or_else(|| anyhow::anyhow!("render needs a source or --socket"))?;
                let c = muz::compile::compile(&source)?;
                if options.tap.len() > 1 {
                    return print(muz::render::render_taps(c.session, &output, &options)?);
                }
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
            tracks,
            mut options,
        } => {
            let c = muz::compile::compile(&source)?;
            if !wet {
                return print(muz::render::stems_of(
                    c.session, &output, &options, &tracks,
                )?);
            }
            let selected = muz::render::select_tracks(&c.session, &tracks)?;
            std::fs::create_dir_all(&output)?;
            let mut reports = vec![];
            for t in selected.iter().map(|&i| &c.session.tracks[i]) {
                let id = t.id.as_str().to_owned();
                if wet {
                    options.solo = vec![id.clone()];
                    options.tap.clear();
                } else {
                    options.solo.clear();
                    options.tap = vec![id.clone()];
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
        Command::Analyze {
            path,
            json: _,
            window,
            bpm,
            beats_per_bar,
            song,
        } => {
            use muz::analysis::PeakWindows;
            let song = match song {
                Some(song) => {
                    let compiled = muz::compile::compile(&song)?;
                    let tempos = match compiled.session.tracks.first().map(|t| &t.source) {
                        Some(muz::model::TrackSource::Midi(m)) => m.imported.tempos.clone(),
                        None => vec![],
                    };
                    Some(PeakWindows::SongBars {
                        transport: compiled.session.transport,
                        tempos,
                    })
                }
                None => None,
            };
            let windows = song
                .or(bpm.map(|bpm| PeakWindows::Bars { bpm, beats_per_bar }))
                .or(window.map(PeakWindows::Seconds));
            print(muz::analysis::analyze_with(&path, windows)?)
        }
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
            if let Some(error) = session.shutdown().error {
                return Err(error.into());
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
            DeviceCommand::ReplaceSource {
                state,
                source,
                output,
            } => {
                let input =
                    muz::assets::read_bounded(&state, muz::dawproject::MAX_CLAP_PRESET_BYTES)?;
                let framed = input.starts_with(b"clap");
                let payload = if framed {
                    let (plugin_id, payload) = muz::dawproject::clap_preset_payload(&input)?;
                    let previous = muz::device_state::DeviceState::decode(payload)?;
                    anyhow::ensure!(
                        plugin_id == previous.role.plugin_id(),
                        "CLAP preset plug-in ID does not match Muz state role"
                    );
                    payload
                } else {
                    anyhow::ensure!(
                        input.len() <= muz::device_state::MAX_STATE_BYTES,
                        "Muz device state too large"
                    );
                    input.as_slice()
                };
                let previous = muz::device_state::DeviceState::decode(payload)?;
                let new_source =
                    String::from_utf8(muz::assets::read_bounded(&source, 1024 * 1024)?)?;
                let device =
                    muz::device_state::reconstruct(&new_source, previous.device.id.as_str())?;
                let next = previous.replace_device(new_source, device)?;
                let bytes = next.encode()?;
                let parent = output
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(std::path::Path::new("."));
                let mut staged = tempfile::NamedTempFile::new_in(parent)?;
                use std::io::Write;
                if framed {
                    muz::dawproject::write_clap_preset(&mut staged, next.role.plugin_id(), &bytes)?;
                } else {
                    staged.write_all(&bytes)?;
                }
                staged.persist(&output)?;
                print(
                    serde_json::json!({"state":output,"device":next.device.id,"parameters":next.parameters.len()}),
                )
            }
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
                preset,
                output,
            } => {
                let path = resolve_device(&path)?;
                if muz::plugins::is_clap(&path) {
                    let mut host = muz::plugins::open_clap(&path, class.as_deref(), 48000, 256)?;
                    let before = host.state_bytes()?;
                    if let Some(load) = &load {
                        host.load_state(load)?;
                        state_changed(&before, &host.state_bytes()?, load, true)?;
                    }
                    if let Some(preset) = &preset {
                        host.load_preset(preset)?;
                        state_changed(&before, &host.state_bytes()?, preset, false)?;
                    }
                    host.save_state(&output)?;
                    return print(serde_json::json!({"state":output}));
                }
                if preset.is_some() {
                    bail!(
                        "--preset uses a CLAP plugin's preset loader; for a VST3 plugin pass a .vstpreset or saved state with --load"
                    );
                }
                let mut host = muz::plugins::open(&path, class.as_deref(), 48000, 256)?;
                if let Some(load) = &load {
                    let before = host.state_bytes()?;
                    host.load_state(load)?;
                    state_changed(&before, &host.state_bytes()?, load, true)?;
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
