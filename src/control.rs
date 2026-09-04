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

use crate::live::{LiveSession, LiveStatus};

pub const DEFAULT_SOCKET_PATH: &str = "/tmp/muz.sock";
const MAX_REQUEST_BYTES: usize = 8 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CLIENTS: usize = 64;
const MAX_ACCEPTS_PER_SERVICE: usize = 16;
const IO_BUDGET_PER_CLIENT: usize = 64 * 1024;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ControlCommand {
    Status,
    Play,
    Stop,
    Restart,
    Seek { tick: u64 },
}

impl<'de> Deserialize<'de> for ControlCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireCommand {
            command: String,
            tick: Option<u64>,
        }

        let wire = WireCommand::deserialize(deserializer)?;
        match (wire.command.as_str(), wire.tick) {
            ("status", None) => Ok(Self::Status),
            ("play", None) => Ok(Self::Play),
            ("stop", None) => Ok(Self::Stop),
            ("restart", None) => Ok(Self::Restart),
            ("seek", Some(tick)) => Ok(Self::Seek { tick }),
            ("seek", None) => Err(serde::de::Error::missing_field("tick")),
            ("status" | "play" | "stop" | "restart", Some(_)) => {
                Err(serde::de::Error::unknown_field("tick", &["command"]))
            }
            _ => Err(serde::de::Error::unknown_variant(
                &wire.command,
                &["status", "play", "stop", "restart", "seek"],
            )),
        }
    }
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
struct SuccessResponse {
    ok: bool,
    status: LiveStatus,
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

    fn queue_status(&mut self, status: LiveStatus) {
        self.response = Some(encode_line(&SuccessResponse { ok: true, status }));
    }

    fn service(&mut self, session: &mut LiveSession, now: Instant) {
        if self.response.is_none() {
            self.read_request(session);
            if self.response.is_none()
                && now.saturating_duration_since(self.accepted_at) >= CLIENT_TIMEOUT
            {
                self.queue_error("request_timeout", "request was not completed in time");
            }
        }
        self.write_response(now);
    }

    fn read_request(&mut self, session: &mut LiveSession) {
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
                            Ok(command) => {
                                let result = match command {
                                    ControlCommand::Status => Ok(()),
                                    ControlCommand::Play => session.set_running(true),
                                    ControlCommand::Stop => session.set_running(false),
                                    ControlCommand::Restart => session.restart(),
                                    ControlCommand::Seek { tick } => session.seek_ticks(tick),
                                };
                                match result {
                                    Ok(()) => self.queue_status(session.status()),
                                    Err(error) => {
                                        self.queue_error("command_queue_full", &error.to_string())
                                    }
                                }
                            }
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
        })
    }

    pub fn service(&mut self, session: &mut LiveSession) -> Result<(), ControlError> {
        self.accept_clients()?;
        let now = Instant::now();
        for client in &mut self.clients {
            client.service(session, now);
        }
        self.clients.retain(|client| !client.done);
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
