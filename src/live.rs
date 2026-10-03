use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use serde::Serialize;
use thiserror::Error;

use crate::{
    Session,
    audio::{DeliveredEvents, PipeWireStatus, TransportSnapshot},
    host::HostContext,
    model::TrackSource,
    native::{
        NativeError, NativeErrorCode, NativeEvent, NativeRuntime, PluginRestartRequest,
        PreparedNativeRevision, RevisionTicket, RuntimeDeviceStatus, ShutdownReport,
    },
    source::{SourceError, parse_project},
    watch::{SourceWatcher, WatchError},
};

const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(12);
const LATENCY_WINDOW: usize = 256;

#[derive(Debug, Error)]
pub enum LiveSessionError {
    #[error(transparent)]
    Native(#[from] NativeError),
    #[error("source `{path}` is invalid")]
    InvalidSource {
        path: PathBuf,
        #[source]
        source: SourceError,
    },
    #[error(transparent)]
    Watch(#[from] WatchError),
    #[error("audio acknowledged revision {actual}, expected {expected}")]
    ReceiptRevisionMismatch { expected: u64, actual: u64 },
    #[error("audio acknowledged observed generation {actual}, expected {expected}")]
    ReceiptGenerationMismatch { expected: u64, actual: u64 },
    #[error("audio acknowledged operation {actual}, expected {expected}")]
    ReceiptOperationMismatch { expected: u64, actual: u64 },
    #[error("audio returned a transaction receipt when none was pending")]
    UnexpectedReceipt,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticKind {
    Read,
    Parse,
    UnsupportedStructure,
    Prepare,
    AudioApply,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReloadDiagnostic {
    pub observed_generation: u64,
    pub kind: DiagnosticKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct LatencySummary {
    pub samples: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}


#[derive(Clone, Copy, Debug, Serialize)]
pub struct RuntimeTransportStatus {
    pub sample_rate: f64,
    pub running: bool,
    pub sample_position: u64,
    pub beat_position: f64,
    pub current_tick: f64,
    pub project_frame: f64,
    pub bpm: f64,
    pub meter: [u8; 2],
    pub loop_ticks: u64,
    pub loop_phase: f64,
    pub end_tick: u64,
    pub end_project_frame: f64,
    pub ended: bool,
}

impl From<TransportSnapshot> for RuntimeTransportStatus {
    fn from(snapshot: TransportSnapshot) -> Self {
        Self {
            sample_rate: snapshot.sample_rate,
            running: snapshot.running,
            sample_position: snapshot.sample_position,
            beat_position: snapshot.beat_position,
            current_tick: snapshot.current_tick,
            project_frame: snapshot.project_frame,
            bpm: snapshot.bpm,
            meter: snapshot.meter,
            loop_ticks: snapshot.loop_ticks,
            end_tick: 0,
            end_project_frame: 0.0,
            ended: snapshot.ended,
            loop_phase: if snapshot.loop_ticks == 0 {
                0.0
            } else {
                let loop_beats = snapshot.loop_ticks as f64 / crate::model::TICKS_PER_BEAT as f64;
                (snapshot.beat_position % loop_beats) / loop_beats
            },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LiveStatus {
    pub source: PathBuf,
    pub observed_generation: u64,
    pub applied_revision: u64,
    pub in_flight_observed_generation: Option<u64>,
    pub queued_observed_generation: Option<u64>,
    pub pending_revision: Option<u64>,
    pub last_error: Option<ReloadDiagnostic>,
    pub last_commit_latency_ms: Option<f64>,
    pub latency: Option<LatencySummary>,
    pub title: String,
    pub track_count: usize,
    pub runtime_revision: u64,
    pub runtime_mapping_pending: bool,
    pub transport: RuntimeTransportStatus,
    pub last_peak: f32,
    pub last_rms: f32,
    pub delivered_events: DeliveredEvents,
    pub devices: Option<Vec<RuntimeDeviceStatus>>,
    pub stream_generation: u64,
    pub stream_start_count: u64,
    pub callback_count: u64,
    pub render_faults: u64,
    pub stream_errors: u64,
    pub transaction_commits: u64,
    pub transaction_faults: u64,
    pub structural_commits: u64,
    pub structural_transition_active: bool,
    pub audio: PipeWireStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum LiveEvent {
    Submitted {
        revision: u64,
        observed_generation: u64,
    },
    Queued {
        observed_generation: u64,
    },
    Rejected {
        diagnostic: ReloadDiagnostic,
    },
    Committed {
        revision: u64,
        observed_generation: u64,
        latency_ms: f64,
        sample_position: u64,
        beat_position: f64,
        callback_count: u64,
        structural: bool,
        faded: bool,
    },
}

struct Candidate {
    session: Session,
    observed_generation: u64,
    event_started: Instant,
}

struct PendingRevision {
    ticket: RevisionTicket,
    // Plugin restarts keep the accepted source and only promote its plugin stamp.
    candidate: Option<Candidate>,
    latency_before_prepare: Duration,
}

pub struct LiveSession {
    source: PathBuf,
    watcher: SourceWatcher,
    runtime: NativeRuntime,
    context: HostContext,
    applied: Session,
    observed_generation: u64,
    applied_generation: u64,
    applied_revision: u64,
    in_flight: Option<PendingRevision>,
    prepared: Option<(PreparedNativeRevision, PendingRevision)>,
    queued: Option<Candidate>,
    restart_request: Option<PluginRestartRequest>,
    last_error: Option<ReloadDiagnostic>,
    latencies: VecDeque<Duration>,
}

impl LiveSession {
    pub fn start(source: impl AsRef<Path>, start_playing: bool) -> Result<Self, LiveSessionError> {
        Self::start_with_debounce(source, start_playing, DEFAULT_DEBOUNCE)
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn applied(&self) -> &Session {
        &self.applied
    }

    pub fn start_with_debounce(
        source: impl AsRef<Path>,
        start_playing: bool,
        debounce: Duration,
    ) -> Result<Self, LiveSessionError> {
        Self::start_backend(source, start_playing, debounce, false)
    }

    pub fn start_backend(
        source: impl AsRef<Path>,
        start_playing: bool,
        debounce: Duration,
        headless: bool,
    ) -> Result<Self, LiveSessionError> {
        let source = source.as_ref().to_path_buf();
        let context = crate::host::current().unwrap_or_else(HostContext::cli);
        let applied = fresh_context(&context)
            .run(|| parse_project(&source))
            .map_err(|source_error| LiveSessionError::InvalidSource {
                path: source.clone(),
                source: source_error,
            })?;
        let runtime = NativeRuntime::start_backend(
            &applied,
            &fresh_context(&context),
            start_playing,
            headless,
        )?;
        let watcher = SourceWatcher::new_many(project_watch_targets(&source, &applied), debounce)?;

        Ok(Self {
            source,
            watcher,
            runtime,
            context,
            applied,
            observed_generation: 0,
            applied_generation: 0,
            applied_revision: 0,
            in_flight: None,
            prepared: None,
            queued: None,
            restart_request: None,
            last_error: None,
            latencies: VecDeque::with_capacity(LATENCY_WINDOW),
        })
    }

    pub fn poll(&mut self, timeout: Duration) -> Result<Vec<LiveEvent>, LiveSessionError> {
        let mut events = Vec::new();
        // Promotion must precede every borrow of the accepted description.
        self.drain_events(&mut events)?;
        if let Err(error) = self.runtime.service_plugins(&self.applied, self.applied_revision)
            && error.code != NativeErrorCode::Busy
        {
            return Err(error.into());
        }
        self.drain_events(&mut events)?;

        if let Some(batch) = self.watcher.poll(timeout)? {
            self.observed_generation = self
                .observed_generation
                .checked_add(batch.event_count as u64)
                .ok_or_else(|| NativeError {
                    code: NativeErrorCode::RevisionOverflow,
                    message: "source observation generation overflow".into(),
                })?;
            self.load_candidate(batch.first_event, &mut events);
        }

        self.drain_events(&mut events)?;
        self.submit_next(&mut events);
        Ok(events)
    }

    pub fn status(&self) -> LiveStatus {
        let runtime = self.runtime.status();
        LiveStatus {
            source: self.source.clone(),
            observed_generation: self.observed_generation,
            applied_revision: self.applied_revision,
            in_flight_observed_generation: self
                .in_flight
                .as_ref()
                .map(|pending| pending.ticket.source_generation),
            queued_observed_generation: self
                .queued
                .as_ref()
                .map(|candidate| candidate.observed_generation)
                .or_else(|| self.prepared.as_ref().map(|(_, pending)| pending.ticket.source_generation)),
            pending_revision: self.in_flight.as_ref().map(|pending| pending.ticket.revision),
            last_error: self.last_error.clone(),
            last_commit_latency_ms: self.latencies.back().copied().map(duration_ms),
            latency: summarize_latencies(&self.latencies),
            title: self.applied.extras.title.clone(),
            track_count: self.applied.tracks.len(),
            runtime_revision: runtime.runtime_revision,
            runtime_mapping_pending: runtime.runtime_mapping_pending,
            transport: RuntimeTransportStatus {
                end_tick: runtime.transport.end_tick,
                end_project_frame: runtime.transport.end_project_frame,
                ..runtime.transport.snapshot.into()
            },
            last_peak: runtime.last_peak,
            last_rms: runtime.last_rms,
            delivered_events: runtime.delivered_events,
            devices: (!runtime.runtime_mapping_pending).then_some(runtime.devices),
            stream_generation: runtime.audio.stream_generation,
            stream_start_count: runtime.stream_start_count,
            callback_count: runtime.audio.callback_count,
            render_faults: runtime.audio.render_faults,
            stream_errors: runtime.audio.stream_errors,
            transaction_commits: runtime.audio.transaction_commits,
            transaction_faults: runtime.audio.transaction_faults,
            structural_commits: runtime.audio.structural_commits,
            structural_transition_active: runtime.audio.structural_transition_active,
            audio: runtime.audio,
        }
    }

    // These methods preserve the CLI/socket admission contract. Callback
    // rejection is reported by poll, never mistaken for an applied control.
    pub fn set_running(&mut self, running: bool) -> Result<(), LiveSessionError> {
        self.runtime.set_running(self.applied_revision, running)?;
        Ok(())
    }

    pub fn restart(&mut self) -> Result<(), LiveSessionError> {
        self.runtime.restart(
            &self.applied,
            self.applied_revision,
            &fresh_context(&self.context),
        )?;
        Ok(())
    }

    pub fn seek_ticks(&mut self, tick: u64) -> Result<(), LiveSessionError> {
        self.runtime.seek_ticks(
            &self.applied,
            self.applied_revision,
            tick,
            &fresh_context(&self.context),
        )?;
        Ok(())
    }

    pub fn set_loop(&mut self, range: Option<(u64, u64)>) -> Result<(), LiveSessionError> {
        self.runtime.set_loop(
            &self.applied,
            self.applied_revision,
            range,
            &fresh_context(&self.context),
        )?;
        Ok(())
    }

    pub fn panic(&mut self) -> Result<(), LiveSessionError> {
        self.runtime.panic(self.applied_revision)?;
        Ok(())
    }

    pub fn shutdown(self) -> ShutdownReport {
        self.runtime.shutdown()
    }

    fn load_candidate(&mut self, event_started: Instant, events: &mut Vec<LiveEvent>) {
        let session = match fresh_context(&self.context).run(|| parse_project(&self.source)) {
            Ok(session) => session,
            Err(error) => {
                self.queued = None;
                // An unsubmitted description is no longer the latest source.
                self.prepared = None;
                let kind = if matches!(error, SourceError::ReadProject { .. }) {
                    DiagnosticKind::Read
                } else {
                    DiagnosticKind::Parse
                };
                self.reject_at(kind, self.observed_generation, error.to_string(), events);
                return;
            }
        };
        let candidate = Candidate {
            session,
            observed_generation: self.observed_generation,
            event_started,
        };
        if self.in_flight.is_some() {
            events.push(LiveEvent::Queued {
                observed_generation: candidate.observed_generation,
            });
            self.queued = Some(candidate);
        } else {
            // Only unsubmitted work may be superseded here.
            self.prepared = None;
            self.queued = None;
            self.submit(candidate, events);
        }
    }

    fn submit(&mut self, mut candidate: Candidate, events: &mut Vec<LiveEvent>) {
        self.runtime.plugin_stamp().apply(&mut candidate.session);
        let latency_before_prepare = candidate.event_started.elapsed();
        let prepared = match self.runtime.prepare_revision(
            &self.applied,
            &candidate.session,
            self.applied_revision,
            candidate.observed_generation,
            &fresh_context(&self.context),
        ) {
            Ok(prepared) => prepared,
            Err(error) if error.code == NativeErrorCode::Busy => {
                events.push(LiveEvent::Queued {
                    observed_generation: candidate.observed_generation,
                });
                self.queued = Some(candidate);
                return;
            }
            Err(error) => {
                self.reject_at(
                    DiagnosticKind::Prepare,
                    candidate.observed_generation,
                    error.to_string(),
                    events,
                );
                return;
            }
        };
        let pending = PendingRevision {
            ticket: prepared.ticket(),
            candidate: Some(candidate),
            latency_before_prepare,
        };
        self.submit_prepared(prepared, pending, events);
    }

    fn submit_restart(&mut self, request: PluginRestartRequest, events: &mut Vec<LiveEvent>) {
        let prepared = match self.runtime.prepare_plugin_restart(
            &self.applied,
            self.applied_revision,
            self.applied_generation,
            &request,
            &fresh_context(&self.context),
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.reject_at(
                    DiagnosticKind::Prepare,
                    self.applied_generation,
                    error.to_string(),
                    events,
                );
                return;
            }
        };
        let pending = PendingRevision {
            ticket: prepared.ticket(),
            candidate: None,
            latency_before_prepare: Duration::ZERO,
        };
        self.submit_prepared(prepared, pending, events);
    }

    fn submit_prepared(
        &mut self,
        prepared: PreparedNativeRevision,
        pending: PendingRevision,
        events: &mut Vec<LiveEvent>,
    ) {
        match self.runtime.submit_revision(prepared) {
            Ok(ticket) => {
                self.clear_error_through(ticket.source_generation);
                self.in_flight = Some(pending);
                events.push(LiveEvent::Submitted {
                    revision: ticket.revision,
                    observed_generation: ticket.source_generation,
                });
            }
            Err(failure) if failure.error.code == NativeErrorCode::QueueFull => {
                // Keep ownership and retry without repeating plugin construction.
                events.push(LiveEvent::Queued {
                    observed_generation: pending.ticket.source_generation,
                });
                self.prepared = Some((failure.prepared, pending));
            }
            Err(failure) => {
                self.reject_at(
                    DiagnosticKind::Prepare,
                    pending.ticket.source_generation,
                    failure.error.to_string(),
                    events,
                );
                drop(failure.prepared);
            }
        }
    }

    fn submit_next(&mut self, events: &mut Vec<LiveEvent>) {
        if self.in_flight.is_some() {
            return;
        }
        if let Some(candidate) = self.queued.take() {
            self.prepared = None;
            self.submit(candidate, events);
        } else if let Some((prepared, pending)) = self.prepared.take() {
            self.submit_prepared(prepared, pending, events);
        } else if let Some(request) = self.restart_request.take() {
            self.submit_restart(request, events);
        }
    }

    fn drain_events(&mut self, events: &mut Vec<LiveEvent>) -> Result<(), LiveSessionError> {
        loop {
            let mut batch = std::array::from_fn::<_, 8, _>(|_| None);
            let count = self.runtime.poll(&mut batch);
            for event in batch.into_iter().flatten() {
                self.accept_event(event, events)?;
            }
            if !count.remaining {
                return Ok(());
            }
        }
    }

    fn take_pending(&mut self, ticket: RevisionTicket) -> Result<PendingRevision, LiveSessionError> {
        let pending = self.in_flight.as_ref().ok_or(LiveSessionError::UnexpectedReceipt)?;
        if ticket.operation != pending.ticket.operation {
            return Err(LiveSessionError::ReceiptOperationMismatch {
                expected: pending.ticket.operation.0,
                actual: ticket.operation.0,
            });
        }
        if ticket.revision != pending.ticket.revision {
            return Err(LiveSessionError::ReceiptRevisionMismatch {
                expected: pending.ticket.revision,
                actual: ticket.revision,
            });
        }
        if ticket.source_generation != pending.ticket.source_generation {
            return Err(LiveSessionError::ReceiptGenerationMismatch {
                expected: pending.ticket.source_generation,
                actual: ticket.source_generation,
            });
        }
        Ok(self.in_flight.take().expect("matching pending revision"))
    }

    fn accept_event(
        &mut self,
        event: NativeEvent,
        events: &mut Vec<LiveEvent>,
    ) -> Result<(), LiveSessionError> {
        match event {
            NativeEvent::RevisionApplied {
                ticket,
                transport,
                callback_count,
                latency_ms,
                structural,
                faded,
                plugin_stamp,
            } => {
                let pending = self.take_pending(ticket)?;
                let watch_result = if let Some(candidate) = pending.candidate {
                    self.applied = candidate.session;
                    self.watcher.replace_targets(project_watch_targets(&self.source, &self.applied))
                } else {
                    Ok(())
                };
                plugin_stamp.apply(&mut self.applied);
                self.applied_revision = ticket.revision;
                self.applied_generation = ticket.source_generation;
                let latency = pending.latency_before_prepare
                    + Duration::from_secs_f64(latency_ms.max(0.0) / 1_000.0);
                if self.latencies.len() == LATENCY_WINDOW {
                    self.latencies.pop_front();
                }
                self.latencies.push_back(latency);
                self.clear_error_through(ticket.source_generation);
                events.push(LiveEvent::Committed {
                    revision: ticket.revision,
                    observed_generation: ticket.source_generation,
                    latency_ms: duration_ms(latency),
                    sample_position: transport.snapshot.sample_position,
                    beat_position: transport.snapshot.beat_position,
                    callback_count,
                    structural,
                    faded,
                });
                watch_result?;
            }
            NativeEvent::RevisionRejected { ticket, error } => {
                self.take_pending(ticket)?;
                self.reject_at(
                    DiagnosticKind::AudioApply,
                    ticket.source_generation,
                    error.to_string(),
                    events,
                );
            }
            NativeEvent::ControlApplied { .. } => {}
            NativeEvent::ControlRejected { kind, error, .. } => {
                self.reject_at(
                    DiagnosticKind::AudioApply,
                    self.observed_generation,
                    format!("{kind:?} control rejected: {error}"),
                    events,
                );
            }
            NativeEvent::PluginRestartRequested(request) => self.restart_request = Some(request),
            NativeEvent::OutputFault(error) => {
                self.reject_at(
                    DiagnosticKind::AudioApply,
                    self.observed_generation,
                    error.to_string(),
                    events,
                );
            }
        }
        Ok(())
    }

    fn clear_error_through(&mut self, generation: u64) {
        if self.last_error.as_ref().is_some_and(|error| error.observed_generation <= generation) {
            self.last_error = None;
        }
    }

    fn reject_at(
        &mut self,
        kind: DiagnosticKind,
        observed_generation: u64,
        message: String,
        events: &mut Vec<LiveEvent>,
    ) {
        let diagnostic = ReloadDiagnostic {
            observed_generation,
            kind,
            message,
        };
        if self.last_error.as_ref().is_none_or(|error| error.observed_generation <= observed_generation) {
            self.last_error = Some(diagnostic.clone());
        }
        events.push(LiveEvent::Rejected { diagnostic });
    }
}


fn project_watch_targets(source: &Path, session: &Session) -> Vec<PathBuf> {
    let root = source.parent().unwrap_or_else(|| Path::new("."));
    let mut targets = Vec::with_capacity(session.tracks.len() + 1);
    targets.push(source.to_path_buf());
    targets.extend(session.extras.dependencies.iter().cloned());
    for track in &session.tracks {
        if let TrackSource::Midi(midi) = &track.source
            && !midi.asset.starts_with("@source/")
        {
            targets.push(root.join(&midi.asset));
        }
    }
    targets
}

fn fresh_context(context: &HostContext) -> HostContext {
    HostContext {
        cancelled: Arc::new(AtomicBool::new(false)),
        ..context.clone()
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn summarize_latencies(latencies: &VecDeque<Duration>) -> Option<LatencySummary> {
    if latencies.is_empty() {
        return None;
    }
    let mut sorted: Vec<_> = latencies.iter().copied().collect();
    sorted.sort_unstable();
    let percentile = |percent: usize| {
        let rank = (sorted.len() * percent).div_ceil(100);
        duration_ms(sorted[rank.saturating_sub(1)])
    };
    Some(LatencySummary {
        samples: sorted.len(),
        p50_ms: percentile(50),
        p95_ms: percentile(95),
        max_ms: duration_ms(*sorted.last().expect("latency set is non-empty")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn write_song(path: &Path, title: &str) {
        std::fs::write(
            path,
            format!(
                r#"song({{title:"{title}",tracks:[track("lead",note(60,4b),synth("pad"))]}})"#
            ),
        )
        .unwrap();
    }

    fn fixture() -> (tempfile::TempDir, LiveSession) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.muz");
        write_song(&path, "accepted");
        let live = HostContext::default()
            .run(|| LiveSession::start_backend(&path, false, Duration::ZERO, true))
            .unwrap();
        (dir, live)
    }

    fn candidate(live: &mut LiveSession, title: &str, generation: u64, events: &mut Vec<LiveEvent>) {
        write_song(live.source(), title);
        live.observed_generation = generation;
        live.load_candidate(Instant::now(), events);
    }

    // Control receipt draining explicitly, rather than relying on filesystem
    // event counts or whether the headless callback won a scheduling race.
    fn settle(live: &mut LiveSession, events: &mut Vec<LiveEvent>) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            live.drain_events(events).unwrap();
            live.submit_next(events);
            let status = live.status();
            if live.in_flight.is_none()
                && live.prepared.is_none()
                && live.queued.is_none()
                && status.runtime_revision == status.applied_revision
                && !status.runtime_mapping_pending
            {
                break;
            }
            assert!(Instant::now() < deadline, "revision did not settle: {status:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn headless_promotes_only_receipts_and_coalesces_latest_source() {
        let (_dir, mut live) = fixture();
        let initial = live.status();
        assert_eq!(initial.applied_revision, 0);
        assert_eq!(initial.runtime_revision, 0);
        assert_eq!(initial.observed_generation, 0);
        assert_eq!(initial.audio.host, "headless");
        assert!(!initial.transport.running);
        let mut events = Vec::new();

        candidate(&mut live, "first", 1, &mut events);
        assert_eq!(live.applied().extras.title, "accepted");
        assert_eq!(live.status().applied_revision, 0);
        assert_eq!(live.status().pending_revision, Some(1));
        candidate(&mut live, "discarded", 2, &mut events);
        candidate(&mut live, "latest", 3, &mut events);
        assert_eq!(live.status().queued_observed_generation, Some(3));
        assert_eq!(live.applied().extras.title, "accepted");

        settle(&mut live, &mut events);
        let commits: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                LiveEvent::Committed { revision, observed_generation, .. } => {
                    Some((*revision, *observed_generation))
                }
                _ => None,
            })
            .collect();
        assert_eq!(commits, [(1, 1), (2, 3)]);
        assert_eq!(live.applied().extras.title, "latest");
        let status = live.status();
        assert_eq!(status.applied_revision, 2);
        assert_eq!(status.runtime_revision, 2);
        assert_eq!(live.runtime.status().source_generation, 3);
        assert!(!status.runtime_mapping_pending);
        assert_eq!(status.stream_start_count, initial.stream_start_count);
        assert_eq!(status.render_faults, 0);
        assert_eq!(status.latency.unwrap().samples, 2);
        assert!(status.last_commit_latency_ms.unwrap().is_finite());
        assert!(status.last_error.is_none());
        let json = serde_json::to_value(status).unwrap();
        assert!(json["devices"][0].get("plugin").is_none());
        assert!(live.shutdown().error.is_none());
    }

    #[test]
    fn source_candidates_wait_for_controls_and_keep_only_the_latest() {
        let (_dir, mut live) = fixture();
        let mut events = Vec::new();
        live.set_running(false).unwrap();
        candidate(&mut live, "discarded", 1, &mut events);
        assert_eq!(live.status().queued_observed_generation, Some(1));
        assert!(live.status().last_error.is_none());
        candidate(&mut live, "latest after control", 2, &mut events);
        assert_eq!(live.status().queued_observed_generation, Some(2));
        assert!(live.in_flight.is_none());
        assert!(live.status().last_error.is_none());

        settle(&mut live, &mut events);
        let commits: Vec<_> = events.iter().filter_map(|event| match event {
            LiveEvent::Committed { revision, observed_generation, .. } => {
                Some((*revision, *observed_generation))
            }
            _ => None,
        }).collect();
        assert_eq!(commits, [(1, 2)]);
        assert!(!events.iter().any(|event| matches!(event, LiveEvent::Rejected { .. })));
        assert_eq!(live.applied().extras.title, "latest after control");
        assert_eq!(live.runtime.status().source_generation, 2);
        assert!(live.shutdown().error.is_none());
    }

    #[test]
    fn newer_parse_failure_survives_an_older_applied_receipt() {
        let (_dir, mut live) = fixture();
        let mut events = Vec::new();
        candidate(&mut live, "first", 1, &mut events);
        candidate(&mut live, "queued", 2, &mut events);
        std::fs::write(live.source(), "song({tracks:").unwrap();
        live.observed_generation = 3;
        live.load_candidate(Instant::now(), &mut events);
        assert!(live.queued.is_none());

        settle(&mut live, &mut events);
        assert_eq!(live.applied().extras.title, "first");
        assert_eq!(live.status().applied_revision, 1);
        let error = live.status().last_error.unwrap();
        assert_eq!(error.observed_generation, 3);
        assert_eq!(error.kind, DiagnosticKind::Parse);
        assert_eq!(live.runtime.status().source_generation, 1);

        candidate(&mut live, "recovered", 4, &mut events);
        settle(&mut live, &mut events);
        assert_eq!(live.applied().extras.title, "recovered");
        assert_eq!(live.status().applied_revision, 2);
        assert!(live.status().last_error.is_none());
        assert!(live.shutdown().error.is_none());
    }

    #[test]
    fn request_contexts_share_services_but_not_cancellation() {
        let (_dir, mut live) = fixture();
        live.context.cancelled.store(true, Ordering::Relaxed);
        let first = fresh_context(&live.context);
        let second = fresh_context(&live.context);
        assert!(Arc::ptr_eq(&first.assets, &live.context.assets));
        assert!(Arc::ptr_eq(&first.decoded_assets, &live.context.decoded_assets));
        assert!(!Arc::ptr_eq(&first.cancelled, &second.cancelled));
        first.cancelled.store(true, Ordering::Relaxed);
        assert!(!second.is_cancelled());

        let mut events = Vec::new();
        candidate(&mut live, "fresh request", 1, &mut events);
        settle(&mut live, &mut events);
        assert_eq!(live.applied().extras.title, "fresh request");
        assert!(live.context.cancelled.load(Ordering::Relaxed));
        assert!(live.shutdown().error.is_none());
    }

    #[test]
    fn shutdown_returns_pending_revision_outcome() {
        let (_dir, mut live) = fixture();
        let mut events = Vec::new();
        candidate(&mut live, "pending shutdown", 1, &mut events);
        let ticket = live.in_flight.as_ref().unwrap().ticket;
        let report = live.shutdown();
        assert!(report.error.is_none());
        let terminal_count = report.events.iter().filter(|event| match event {
            NativeEvent::RevisionApplied { ticket: actual, .. }
            | NativeEvent::RevisionRejected { ticket: actual, .. } => actual == &ticket,
            _ => false,
        }).count();
        assert_eq!(terminal_count, 1);
    }

    #[test]
    fn json5_watcher_uses_native_revision_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json5");
        let source = r#"{
            schema:1,
            transport:{mode:'loop',bpm:120,meter:[4,4],loop_beats:4},
            master:{id:'master'},
            tracks:[{
                id:'lead',
                source:{kind:'pattern',id:'phrase',notes:[
                    {id:'n',at:0,duration:1,key:60,velocity:0.8},
                ]},
                instrument:{id:'voice',kind:'builtin.poly_synth',params:{gain_db:-12}},
                output:{id:'out',to:'master',gain_db:0},
            }],
        }"#;
        std::fs::write(&path, source).unwrap();
        let mut live = HostContext::default()
            .run(|| LiveSession::start_backend(&path, false, Duration::ZERO, true))
            .unwrap();
        std::fs::write(&path, source.replace("bpm:120", "bpm:90")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut committed = false;
        while !committed {
            for event in live.poll(Duration::from_millis(2)).unwrap() {
                if let LiveEvent::Committed { observed_generation, .. } = event {
                    assert!(observed_generation > 0);
                    committed = true;
                }
            }
            assert!(Instant::now() < deadline, "JSON5 watcher did not commit: {:?}", live.status());
        }
        settle(&mut live, &mut Vec::new());
        let status = live.status();
        assert!(status.applied_revision > 0);
        assert_eq!(status.applied_revision, status.runtime_revision);
        assert!(!status.runtime_mapping_pending);
        assert!(status.last_error.is_none());
        assert!(live.shutdown().error.is_none());
    }
}
