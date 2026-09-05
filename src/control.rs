use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::live::LiveSession;

pub const DEFAULT_SOCKET_PATH: &str = "/tmp/muz.sock";
const MAX_REQUEST_BYTES: usize = 8 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CLIENTS: usize = 64;
const MAX_ACCEPTS_PER_SERVICE: usize = 16;
const IO_BUDGET_PER_CLIENT: usize = 64 * 1024;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlCommand {
    Status,
    Play,
    Stop,
    Restart,
    Panic,
    Shutdown,
    Seek {
        #[serde(default)]
        tick: Option<u64>,
        #[serde(default)]
        beat: Option<f64>,
        #[serde(default)]
        section: Option<String>,
    },
    Loop {
        #[serde(default)]
        section: Option<String>,
        #[serde(default)]
        start: Option<f64>,
        #[serde(default)]
        end: Option<f64>,
        #[serde(default)]
        off: bool,
    },
    Inspect {
        #[serde(default = "score_view")]
        view: String,
        #[serde(default)]
        section: Option<String>,
        #[serde(default)]
        track: Option<String>,
    },
    Devices,
    Render {
        output: PathBuf,
        #[serde(flatten)]
        options: crate::render::RenderOptions,
    },
    Jobs,
    Cancel {
        id: u64,
    },
    Analyze {
        path: PathBuf,
    },
}
fn score_view() -> String {
    "graph".into()
}
#[derive(Debug, Error)]
pub enum ControlError {
    #[error("control socket path `{path}` exists and is not a socket")]
    UnsafeExistingPath { path: PathBuf },
    #[error("control socket `{path}` is already in use")]
    SocketInUse { path: PathBuf },
    #[error("control socket `{path}` changed while checking whether it was stale")]
    SocketPathChanged { path: PathBuf },
    #[error("control socket operation failed for `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("control response exceeded {MAX_RESPONSE_BYTES} bytes")]
    ResponseTooLarge,
    #[error("control server returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("control server rejected the command: {0}")]
    ServerRejected(String),
}

#[derive(Debug)]
pub struct ClientResponse {
    json: String,
    ok: bool,
    error_message: Option<String>,
}

impl ClientResponse {
    pub fn json(&self) -> &str {
        &self.json
    }

    pub fn into_result(self) -> Result<(), ControlError> {
        if self.ok {
            Ok(())
        } else {
            Err(ControlError::ServerRejected(
                self.error_message
                    .unwrap_or_else(|| "unspecified server error".to_owned()),
            ))
        }
    }
}

#[derive(Serialize)]
struct ErrorResponse<'a> {
    ok: bool,
    error: WireError<'a>,
}

#[derive(Serialize)]
struct WireError<'a> {
    code: &'a str,
    message: &'a str,
}

struct Client {
    stream: UnixStream,
    accepted_at: Instant,
    request: Vec<u8>,
    response: Option<Vec<u8>>,
    written: usize,
    done: bool,
}

impl Client {
    fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            accepted_at: Instant::now(),
            request: Vec::with_capacity(256),
            response: None,
            written: 0,
            done: false,
        })
    }

    fn queue_error(&mut self, code: &'static str, message: &str) {
        if self.response.is_some() {
            return;
        }
        self.response = Some(encode_line(&ErrorResponse {
            ok: false,
            error: WireError { code, message },
        }));
    }

    fn service(&mut self, api: &mut Api, now: Instant) {
        if self.response.is_none() {
            self.read_request(api);
            if self.response.is_none()
                && now.saturating_duration_since(self.accepted_at) >= CLIENT_TIMEOUT
            {
                self.queue_error("request_timeout", "request was not completed in time");
            }
        }
        self.write_response(now);
    }

    fn read_request(&mut self, api: &mut Api) {
        let mut chunk = [0_u8; 2048];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.queue_error("invalid_request", "request ended before a newline");
                    return;
                }
                Ok(count) => {
                    self.request.extend_from_slice(&chunk[..count]);
                    if self.request.len() > MAX_REQUEST_BYTES {
                        self.queue_error("request_too_large", "request exceeds 8192 bytes");
                        return;
                    }
                    if let Some(newline) = self.request.iter().position(|byte| *byte == b'\n') {
                        let line = &self.request[..newline];
                        match serde_json::from_slice::<ControlCommand>(line) {
                            Ok(command) => match api.handle(command) {
                                Ok(value) => {
                                    let response =
                                        encode_line(&serde_json::json!({"ok":true,"result":value}));
                                    if response.len() > MAX_RESPONSE_BYTES {
                                        self.queue_error("response_too_large","response exceeds 16 MiB; filter inspection by section/track");
                                    } else {
                                        self.response = Some(response);
                                    }
                                }
                                Err(error) => {
                                    self.queue_error("command_failed", &format!("{error:#}"))
                                }
                            },
                            Err(error) => self.queue_error("invalid_request", &error.to_string()),
                        }
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(error) => {
                    self.queue_error("request_io", &error.to_string());
                    return;
                }
            }
        }
    }

    fn write_response(&mut self, now: Instant) {
        let Some(response) = self.response.as_ref() else {
            return;
        };
        let limit = (self.written + IO_BUDGET_PER_CLIENT).min(response.len());
        while self.written < limit {
            match self.stream.write(&response[self.written..limit]) {
                Ok(0) => {
                    self.done = true;
                    return;
                }
                Ok(count) => self.written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.done = true;
                    return;
                }
            }
        }
        if self.written == response.len() {
            self.done = true;
        } else if now.saturating_duration_since(self.accepted_at) >= CLIENT_TIMEOUT {
            self.done = true;
        }
    }
}

pub struct ControlServer {
    listener: UnixListener,
    path: PathBuf,
    bound_device: u64,
    bound_inode: u64,
    clients: Vec<Client>,
    jobs: Vec<RenderJob>,
    pub shutdown: bool,
}

impl ControlServer {
    pub fn bind(path: impl AsRef<Path>) -> Result<Self, ControlError> {
        let path = path.as_ref().to_path_buf();
        remove_stale_socket(&path)?;
        let listener = UnixListener::bind(&path).map_err(|source| ControlError::Io {
            path: path.clone(),
            source,
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| ControlError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.file_type().is_socket() {
            return Err(ControlError::SocketPathChanged { path });
        }
        let bound_device = metadata.dev();
        let bound_inode = metadata.ino();
        if let Err(source) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
            remove_if_same_socket(&path, bound_device, bound_inode);
            return Err(ControlError::Io {
                path: path.clone(),
                source,
            });
        }
        if let Err(source) = listener.set_nonblocking(true) {
            remove_if_same_socket(&path, bound_device, bound_inode);
            return Err(ControlError::Io {
                path: path.clone(),
                source,
            });
        }
        let current = fs::symlink_metadata(&path).map_err(|source| ControlError::Io {
            path: path.clone(),
            source,
        })?;
        if !current.file_type().is_socket()
            || current.dev() != bound_device
            || current.ino() != bound_inode
        {
            return Err(ControlError::SocketPathChanged { path });
        }
        Ok(Self {
            listener,
            path,
            bound_device,
            bound_inode,
            clients: Vec::new(),
            jobs: Vec::new(),
            shutdown: false,
        })
    }

    pub fn service(&mut self, session: &mut LiveSession) -> Result<(), ControlError> {
        self.accept_clients()?;
        let now = Instant::now();
        let mut api = Api {
            session,
            jobs: &mut self.jobs,
            shutdown: &mut self.shutdown,
        };
        for client in &mut self.clients {
            client.service(&mut api, now);
        }
        self.clients.retain(|client| !client.done);
        if !self.shutdown {
            start_render_jobs(&mut self.jobs);
        }
        Ok(())
    }

    fn accept_clients(&mut self) -> Result<(), ControlError> {
        for _ in 0..MAX_ACCEPTS_PER_SERVICE {
            if self.clients.len() >= MAX_CLIENTS {
                break;
            }
            match self.listener.accept() {
                Ok((stream, _)) => match Client::new(stream) {
                    Ok(client) => self.clients.push(client),
                    Err(_) => continue,
                },
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(source) => {
                    return Err(ControlError::Io {
                        path: self.path.clone(),
                        source,
                    });
                }
            }
        }
        Ok(())
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        for job in &self.jobs {
            job.progress
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        for job in &mut self.jobs {
            if let Some(worker) = job.worker.take() {
                let _ = worker.join();
            }
        }
        remove_if_same_socket(&self.path, self.bound_device, self.bound_inode);
    }
}

pub fn send_command(
    path: impl AsRef<Path>,
    command: ControlCommand,
) -> Result<ClientResponse, ControlError> {
    let path = path.as_ref();
    let mut stream = UnixStream::connect(path).map_err(|source| ControlError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    stream
        .set_write_timeout(Some(CLIENT_TIMEOUT))
        .map_err(|source| ControlError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    stream
        .set_read_timeout(Some(CLIENT_TIMEOUT))
        .map_err(|source| ControlError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut request =
        serde_json::to_vec(&command).expect("control command serialization cannot fail");
    request.push(b'\n');
    stream
        .write_all(&request)
        .map_err(|source| ControlError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    let mut bytes = Vec::new();
    stream
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| ControlError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ControlError::ResponseTooLarge);
    }
    if bytes.last() != Some(&b'\n') || bytes.iter().filter(|byte| **byte == b'\n').count() != 1 {
        return Err(ControlError::InvalidResponse(
            "expected exactly one newline-terminated JSON value".to_owned(),
        ));
    }
    let value: Value = serde_json::from_slice(&bytes[..bytes.len() - 1])
        .map_err(|error| ControlError::InvalidResponse(error.to_string()))?;
    let ok = value
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| ControlError::InvalidResponse("missing boolean `ok` field".to_owned()))?;
    let error_message = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let json = String::from_utf8(bytes[..bytes.len() - 1].to_vec())
        .map_err(|error| ControlError::InvalidResponse(error.to_string()))?;
    Ok(ClientResponse {
        json,
        ok,
        error_message,
    })
}

fn remove_if_same_socket(path: &Path, device: u64, inode: u64) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_socket() && metadata.dev() == device && metadata.ino() == inode {
        let _ = fs::remove_file(path);
    }
}

fn remove_stale_socket(path: &Path) -> Result<(), ControlError> {
    let initial = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ControlError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !initial.file_type().is_socket() {
        return Err(ControlError::UnsafeExistingPath {
            path: path.to_path_buf(),
        });
    }
    match UnixStream::connect(path) {
        Ok(_) => {
            return Err(ControlError::SocketInUse {
                path: path.to_path_buf(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ControlError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    let current = fs::symlink_metadata(path).map_err(|source| ControlError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !current.file_type().is_socket()
        || current.dev() != initial.dev()
        || current.ino() != initial.ino()
    {
        return Err(ControlError::SocketPathChanged {
            path: path.to_path_buf(),
        });
    }
    fs::remove_file(path).map_err(|source| ControlError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn encode_line(value: &impl Serialize) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("wire response serialization cannot fail");
    bytes.push(b'\n');
    bytes
}

#[derive(Default)]
pub struct RenderProgress {
    pub file: Option<PathBuf>,
    pub processed: std::sync::atomic::AtomicU64,
    pub total: std::sync::atomic::AtomicU64,
    pub cancel: std::sync::atomic::AtomicBool,
}
const MAX_RENDER_WORKERS: usize = 2;
const MAX_QUEUED_RENDERS: usize = 32;

struct PendingRender {
    session: crate::Session,
    options: crate::render::RenderOptions,
}
struct RenderJob {
    pending: Option<PendingRender>,
    revision: u64,
    source: String,
    worker: Option<std::thread::JoinHandle<()>>,
    id: u64,
    output: PathBuf,
    progress: std::sync::Arc<RenderProgress>,
    result: std::sync::Arc<std::sync::Mutex<Option<Result<crate::render::RenderReport, String>>>>,
}
impl RenderJob {
    fn status(&self) -> Value {
        use std::sync::atomic::Ordering::Relaxed;
        let result = self.result.lock().unwrap();
        let state = match result.as_ref() {
            Some(Ok(_)) => "finished",
            Some(Err(_)) if self.progress.cancel.load(Relaxed) => "cancelled",
            Some(Err(_)) => "failed",
            None if self.pending.is_some() => "queued",
            None => "running",
        };
        serde_json::json!({"id":self.id,"output":self.output,"processed":self.progress.processed.load(Relaxed),"total":self.progress.total.load(Relaxed),"state":state,"revision":self.revision,"source":self.source,"result":*result})
    }
}
fn start_render_jobs(jobs: &mut [RenderJob]) {
    use std::sync::atomic::Ordering::Relaxed;
    let mut running = jobs
        .iter()
        .filter(|j| j.pending.is_none() && j.result.lock().unwrap().is_none())
        .count();
    for job in jobs {
        if job.worker.as_ref().is_some_and(|w| w.is_finished()) {
            let _ = job.worker.take().unwrap().join();
        }
        if job.pending.is_none() {
            continue;
        }
        if job.progress.cancel.load(Relaxed) {
            job.pending = None;
            *job.result.lock().unwrap() = Some(Err("render cancelled".into()));
            continue;
        }
        if running >= MAX_RENDER_WORKERS {
            continue;
        }
        let pending = job.pending.take().unwrap();
        let out = job.output.clone();
        let p = job.progress.clone();
        let r = job.result.clone();
        let revision = job.revision;
        let source = job.source.clone();
        job.worker = Some(std::thread::spawn(move || {
            let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::worker::bounce(pending.session, out, pending.options, p).map(|mut report| {
                    report.revision = Some(revision);
                    report.source = Some(source);
                    report
                })
            }))
            .map_err(|_| "render worker panicked".to_owned())
            .and_then(|v| v.map_err(|e| format!("{e:#}")));
            *r.lock().unwrap() = Some(value);
        }));
        running += 1;
    }
}

/// Run a finite collection through the same bounded scheduler and isolated
/// workers as live bounces. Every session is already captured before work starts.
pub fn render_batch(
    requests: Vec<(crate::Session, PathBuf, crate::render::RenderOptions)>,
) -> anyhow::Result<Vec<Result<crate::render::RenderReport, String>>> {
    use std::sync::{Arc, Mutex, atomic::Ordering::Relaxed};
    anyhow::ensure!(
        !requests.is_empty() && requests.len() <= MAX_RENDER_WORKERS + MAX_QUEUED_RENDERS,
        "a render recipe needs 1..34 outputs"
    );
    let mut outputs = std::collections::BTreeSet::new();
    let mut jobs = Vec::new();
    for (session, output, options) in requests {
        anyhow::ensure!(outputs.insert(output.clone()), "duplicate recipe output");
        let source = session
            .extras
            .source
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        jobs.push(RenderJob {
            pending: Some(PendingRender { session, options }),
            revision: 0,
            source,
            worker: None,
            id: jobs.len() as u64 + 1,
            output,
            progress: Arc::new(RenderProgress::default()),
            result: Arc::new(Mutex::new(None)),
        });
    }
    loop {
        if crate::INTERRUPTED.load(Relaxed) {
            for job in &jobs {
                job.progress.cancel.store(true, Relaxed);
            }
        }
        start_render_jobs(&mut jobs);
        if jobs.iter().all(|job| job.result.lock().unwrap().is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    Ok(jobs
        .into_iter()
        .map(|mut job| {
            if let Some(worker) = job.worker.take() {
                let _ = worker.join();
            }
            let result = job.result.lock().unwrap().take().unwrap();
            result.map(|mut report| {
                report.revision = None;
                report
            })
        })
        .collect())
}
struct Api<'a> {
    session: &'a mut LiveSession,
    jobs: &'a mut Vec<RenderJob>,
    shutdown: &'a mut bool,
}
impl Api<'_> {
    fn handle(&mut self, command: ControlCommand) -> anyhow::Result<Value> {
        use std::sync::{Arc, Mutex, atomic::Ordering};
        match command {
            ControlCommand::Status => return Ok(serde_json::to_value(self.session.status())?),
            ControlCommand::Play => self.session.set_running(true)?,
            ControlCommand::Stop => self.session.set_running(false)?,
            ControlCommand::Restart => self.session.restart()?,
            ControlCommand::Panic => self.session.panic()?,
            ControlCommand::Shutdown => {
                self.session.panic()?;
                for j in self.jobs.iter() {
                    j.progress.cancel.store(true, Ordering::Relaxed);
                }
                *self.shutdown = true;
            }
            ControlCommand::Seek {
                tick,
                beat,
                section,
            } => {
                anyhow::ensure!(
                    usize::from(tick.is_some())
                        + usize::from(beat.is_some())
                        + usize::from(section.is_some())
                        == 1,
                    "seek needs exactly one of tick, beat or section"
                );
                let pos = if let Some(tick) = tick {
                    tick
                } else {
                    let beat = if let Some(name) = section {
                        self.section(&name)?.0
                    } else {
                        beat.unwrap()
                    };
                    anyhow::ensure!(beat.is_finite() && beat >= 0.0, "invalid beat");
                    crate::compile::tick(beat)
                };
                self.session.seek_ticks(pos)?;
            }
            ControlCommand::Loop {
                section,
                start,
                end,
                off,
            } => {
                if off {
                    self.session.set_loop(None)?;
                } else {
                    let (a, b) = if let Some(name) = section {
                        self.section(&name)?
                    } else {
                        (
                            start.unwrap_or(0.0),
                            end.ok_or_else(|| anyhow::anyhow!("loop needs a section or end beat"))?,
                        )
                    };
                    anyhow::ensure!(
                        a.is_finite() && b.is_finite() && a >= 0.0 && b > a,
                        "invalid loop range"
                    );
                    self.session
                        .set_loop(Some((crate::compile::tick(a), crate::compile::tick(b))))?;
                }
            }
            ControlCommand::Inspect {
                view,
                section,
                track,
            } => {
                return crate::inspect::filtered(
                    self.session.applied(),
                    &view,
                    section.as_deref(),
                    track.as_deref(),
                );
            }
            ControlCommand::Devices => {
                return Ok(serde_json::to_value(self.session.status().devices)?);
            }
            ControlCommand::Analyze { path } => return crate::analysis::analyze(&path),
            ControlCommand::Jobs => {
                return Ok(Value::Array(
                    self.jobs.iter().map(RenderJob::status).collect(),
                ));
            }
            ControlCommand::Cancel { id } => {
                let j = self
                    .jobs
                    .iter_mut()
                    .find(|j| j.id == id)
                    .ok_or_else(|| anyhow::anyhow!("unknown job {id}"))?;
                if j.result.lock().unwrap().is_some() {
                    return Ok(j.status());
                }
                j.progress.cancel.store(true, Ordering::Relaxed);
                if j.pending.take().is_some() {
                    *j.result.lock().unwrap() = Some(Err("render cancelled".into()));
                }
                return Ok(j.status());
            }
            ControlCommand::Render { output, options } => {
                anyhow::ensure!(!*self.shutdown, "server is shutting down");
                anyhow::ensure!(
                    self.jobs.iter().filter(|j| j.pending.is_some()).count() < MAX_QUEUED_RENDERS,
                    "render queue is full (32 waiting); inspect jobs or cancel a queued render"
                );
                anyhow::ensure!(
                    !self
                        .jobs
                        .iter()
                        .any(|j| j.output == output && j.result.lock().unwrap().is_none()),
                    "output already rendering"
                );
                let id = self.jobs.last().map_or(1, |j| j.id + 1);
                let progress = Arc::new(RenderProgress::default());
                let result = Arc::new(Mutex::new(None));
                let session = self.session.applied().clone();
                let revision = self.session.status().applied_revision;
                let source = self.session.source().display().to_string();
                self.jobs.push(RenderJob {
                    pending: Some(PendingRender { session, options }),
                    revision,
                    source,
                    worker: None,
                    id,
                    output,
                    progress,
                    result,
                });
                start_render_jobs(self.jobs);
                return Ok(self.jobs.last().unwrap().status());
            }
        }
        Ok(serde_json::json!({"queued":true}))
    }
    fn section(&self, name: &str) -> anyhow::Result<(f64, f64)> {
        let s = self
            .session
            .applied()
            .extras
            .sections
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| anyhow::anyhow!("unknown section {name}"))?;
        Ok((s.start, s.end))
    }
}
