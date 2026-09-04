use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::Serialize;
use thiserror::Error;

use crate::{
    Session,
    audio::{
        DeliveredEvents, DeviceDebugState, PipeWireError, PipeWireOutput, PipeWireStatus,
        PreparedTransaction, TransactionReceipt, TransportSnapshot,
        pipewire::TransportCommandQueueFull,
    },
    model::{DeviceKind, Id, TrackSource, Vst3Config},
    plan_reconciliation,
    source::{SourceError, parse_project},
    watch::{SourceWatcher, WatchError},
};

const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(12);
const LATENCY_WINDOW: usize = 256;

#[derive(Debug, Error)]
pub enum LiveSessionError {
    #[error("failed to read source `{path}`: {source}")]
    ReadSource {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("source `{path}` is invalid: {source}")]
    InvalidSource {
        path: PathBuf,
        #[source]
        source: SourceError,
    },
    #[error(transparent)]
    Audio(#[from] PipeWireError),
    #[error(transparent)]
    Watch(#[from] WatchError),
    #[error(transparent)]
    TransportCommand(#[from] TransportCommandQueueFull),
    #[error("audio acknowledged revision {actual}, expected {expected}")]
    ReceiptRevisionMismatch { expected: u64, actual: u64 },
    #[error("audio acknowledged observed generation {actual}, expected {expected}")]
    ReceiptGenerationMismatch { expected: u64, actual: u64 },
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

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeDeviceStatus {
    pub id: Id,
    pub kind: DeviceKind,
    pub instance_token: u64,
    pub process_count: u64,
    pub gain_reduction_db: f32,
    pub latency_samples: u32,
    pub tail_samples: u32,
    pub is_plugin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<Vst3Config>,
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
    pub applied: Session,
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

pub struct LiveSession {
    source: PathBuf,
    watcher: SourceWatcher,
    output: PipeWireOutput,
    applied: Session,
    observed_generation: u64,
    applied_revision: u64,
    in_flight: Option<Candidate>,
    queued: Option<Candidate>,
    last_error: Option<ReloadDiagnostic>,
    latencies: VecDeque<Duration>,
}

impl LiveSession {
    pub fn start(source: impl AsRef<Path>, start_playing: bool) -> Result<Self, LiveSessionError> {
        Self::start_with_debounce(source, start_playing, DEFAULT_DEBOUNCE)
    }

    pub fn start_with_debounce(
        source: impl AsRef<Path>,
        start_playing: bool,
        debounce: Duration,
    ) -> Result<Self, LiveSessionError> {
        let source = source.as_ref().to_path_buf();
        let applied =
            parse_project(&source).map_err(|source_error| LiveSessionError::InvalidSource {
                path: source.clone(),
                source: source_error,
            })?;
        let output = PipeWireOutput::start(&applied, start_playing)?;
        let watcher = SourceWatcher::new_many(project_watch_targets(&source, &applied), debounce)?;

        Ok(Self {
            source,
            watcher,
            output,
            applied,
            observed_generation: 0,
            applied_revision: 0,
            in_flight: None,
            queued: None,
            last_error: None,
            latencies: VecDeque::with_capacity(LATENCY_WINDOW),
        })
    }

    pub fn poll(&mut self, timeout: Duration) -> Result<Vec<LiveEvent>, LiveSessionError> {
        let mut events = Vec::new();
        self.drain_receipts(&mut events)?;

        if let Some(batch) = self.watcher.poll(timeout)? {
            self.observed_generation = self
                .observed_generation
                .saturating_add(batch.event_count as u64);
            self.load_candidate(batch.first_event, &mut events);
        }

        self.drain_receipts(&mut events)?;
        Ok(events)
    }

    pub fn status(&self) -> LiveStatus {
        let runtime = self.output.runtime_snapshot();
        let (runtime_mapping_pending, devices) = map_runtime_devices(
            &self.applied,
            self.applied_revision,
            runtime.runtime_revision,
            &runtime.devices,
        );
        LiveStatus {
            source: self.source.clone(),
            observed_generation: self.observed_generation,
            applied_revision: self.applied_revision,
            in_flight_observed_generation: self
                .in_flight
                .as_ref()
                .map(|candidate| candidate.observed_generation),
            queued_observed_generation: self
                .queued
                .as_ref()
                .map(|candidate| candidate.observed_generation),
            pending_revision: self
                .in_flight
                .as_ref()
                .map(|_| self.applied_revision.saturating_add(1)),
            last_error: self.last_error.clone(),
            last_commit_latency_ms: self.latencies.back().copied().map(duration_ms),
            latency: summarize_latencies(&self.latencies),
            applied: self.applied.clone(),
            runtime_revision: runtime.runtime_revision,
            runtime_mapping_pending,
            transport: RuntimeTransportStatus {
                end_tick: runtime.end_tick,
                end_project_frame: runtime.end_project_frame,
                ..runtime.transport.into()
            },
            last_peak: finite_sample(runtime.last_peak),
            last_rms: finite_sample(runtime.last_rms),
            delivered_events: runtime.delivered_events,
            devices,
            stream_generation: runtime.stream_generation,
            stream_start_count: runtime.stream_start_count,
            callback_count: runtime.callback_count,
            render_faults: runtime.render_faults,
            stream_errors: runtime.stream_errors,
            transaction_commits: runtime.transaction_commits,
            transaction_faults: runtime.transaction_faults,
            structural_commits: runtime.structural_commits,
            structural_transition_active: runtime.structural_transition_active,
            audio: self.output.status(),
        }
    }

    pub fn set_running(&mut self, running: bool) -> Result<(), LiveSessionError> {
        self.output.set_running(running)?;
        Ok(())
    }

    pub fn restart(&mut self) -> Result<(), LiveSessionError> {
        self.output.restart()?;
        Ok(())
    }

    pub fn seek_ticks(&mut self, tick: u64) -> Result<(), LiveSessionError> {
        self.output.seek_ticks(tick)?;
        Ok(())
    }

    fn load_candidate(&mut self, event_started: Instant, events: &mut Vec<LiveEvent>) {
        let session = match parse_project(&self.source) {
            Ok(session) => session,
            Err(error) => {
                self.queued = None;
                self.reject(DiagnosticKind::Parse, error.to_string(), events);
                return;
            }
        };
        let candidate = Candidate {
            session,
            observed_generation: self.observed_generation,
            event_started,
        };

        if self.in_flight.is_some() {
            let generation = candidate.observed_generation;
            self.queued = Some(candidate);
            events.push(LiveEvent::Queued {
                observed_generation: generation,
            });
        } else {
            self.submit(candidate, events);
        }
    }

    fn submit(&mut self, candidate: Candidate, events: &mut Vec<LiveEvent>) {
        let candidate_generation = candidate.observed_generation;
        let plan =
            match plan_reconciliation(self.applied_revision, &self.applied, &candidate.session) {
                Ok(plan) => plan,
                Err(error) => {
                    self.reject_at(
                        DiagnosticKind::Prepare,
                        candidate_generation,
                        error.to_string(),
                        events,
                    );
                    return;
                }
            };
        let transaction = match PreparedTransaction::prepare(
            &self.applied,
            &candidate.session,
            &plan,
            candidate.observed_generation,
            candidate.event_started,
            self.output.audio_config(),
        ) {
            Ok(transaction) => transaction,
            Err(error) => {
                self.reject_at(
                    DiagnosticKind::Prepare,
                    candidate_generation,
                    error.to_string(),
                    events,
                );
                return;
            }
        };
        let revision = transaction.revision();
        match self.output.submit_transaction(Box::new(transaction)) {
            Ok(()) => {
                let observed_generation = candidate.observed_generation;
                self.in_flight = Some(candidate);
                self.last_error = None;
                events.push(LiveEvent::Submitted {
                    revision,
                    observed_generation,
                });
            }
            Err(full) => {
                drop(full.into_transaction());
                let generation = candidate.observed_generation;
                self.queued = Some(candidate);
                events.push(LiveEvent::Queued {
                    observed_generation: generation,
                });
            }
        }
    }

    fn drain_receipts(&mut self, events: &mut Vec<LiveEvent>) -> Result<(), LiveSessionError> {
        while let Some(receipt) = self.output.poll_transaction_receipt() {
            self.accept_receipt(receipt, events)?;
        }
        Ok(())
    }

    fn accept_receipt(
        &mut self,
        receipt: TransactionReceipt,
        events: &mut Vec<LiveEvent>,
    ) -> Result<(), LiveSessionError> {
        let pending = self
            .in_flight
            .take()
            .ok_or(LiveSessionError::UnexpectedReceipt)?;
        let expected_revision = self.applied_revision.saturating_add(1);
        if receipt.revision != expected_revision {
            return Err(LiveSessionError::ReceiptRevisionMismatch {
                expected: expected_revision,
                actual: receipt.revision,
            });
        }
        if receipt.observed_generation != pending.observed_generation {
            return Err(LiveSessionError::ReceiptGenerationMismatch {
                expected: pending.observed_generation,
                actual: receipt.observed_generation,
            });
        }

        if receipt.committed {
            let watch_result = self
                .watcher
                .replace_targets(project_watch_targets(&self.source, &pending.session));
            self.applied = pending.session;
            self.applied_revision = receipt.revision;
            let latency = receipt
                .committed_at
                .saturating_duration_since(receipt.transaction.event_started());
            if self.latencies.len() == LATENCY_WINDOW {
                self.latencies.pop_front();
            }
            self.latencies.push_back(latency);
            self.last_error = None;
            events.push(LiveEvent::Committed {
                revision: receipt.revision,
                observed_generation: receipt.observed_generation,
                latency_ms: duration_ms(latency),
                sample_position: receipt.sample_position,
                beat_position: receipt.transport.beat_position,
                callback_count: receipt.callback_count,
                structural: receipt.structural,
                faded: receipt.faded,
            });
            watch_result?;
        } else {
            let message = receipt
                .failure
                .map(|failure| failure.to_string())
                .unwrap_or_else(|| "audio rejected the transaction without a reason".to_owned());
            self.reject_at(
                DiagnosticKind::AudioApply,
                receipt.observed_generation,
                message,
                events,
            );
        }
        drop(receipt);

        if let Some(candidate) = self.queued.take() {
            self.submit(candidate, events);
        }
        Ok(())
    }

    fn reject(&mut self, kind: DiagnosticKind, message: String, events: &mut Vec<LiveEvent>) {
        self.reject_at(kind, self.observed_generation, message, events);
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
        self.last_error = Some(diagnostic.clone());
        events.push(LiveEvent::Rejected { diagnostic });
    }
}

fn map_runtime_devices(
    applied: &Session,
    applied_revision: u64,
    runtime_revision: u64,
    states: &[DeviceDebugState],
) -> (bool, Option<Vec<RuntimeDeviceStatus>>) {
    let expected_count = applied.master.inserts.len()
        + applied
            .buses
            .iter()
            .map(|bus| bus.inserts.len())
            .sum::<usize>()
        + applied
            .tracks
            .iter()
            .map(|track| 1 + track.inserts.len())
            .sum::<usize>();
    if runtime_revision != applied_revision || states.len() != expected_count {
        return (true, None);
    }

    let mut devices = Vec::with_capacity(expected_count);
    let mut states = states.iter().copied();
    for device in &applied.master.inserts {
        push_runtime_device(&mut devices, device, states.next().unwrap());
    }
    for bus in &applied.buses {
        for device in &bus.inserts {
            push_runtime_device(&mut devices, device, states.next().unwrap());
        }
    }
    for track in &applied.tracks {
        push_runtime_device(&mut devices, &track.instrument, states.next().unwrap());
        for device in &track.inserts {
            push_runtime_device(&mut devices, device, states.next().unwrap());
        }
    }
    (false, Some(devices))
}

fn push_runtime_device(
    devices: &mut Vec<RuntimeDeviceStatus>,
    device: &crate::model::Device,
    state: DeviceDebugState,
) {
    devices.push(RuntimeDeviceStatus {
        id: device.id.clone(),
        kind: device.kind,
        instance_token: state.instance_token,
        process_count: state.process_count,
        gain_reduction_db: finite_sample(state.gain_reduction_db),
        latency_samples: state.latency_samples,
        tail_samples: state.tail_samples,
        is_plugin: state.is_plugin,
        plugin: device.vst3.clone(),
    });
}

fn project_watch_targets(source: &Path, session: &Session) -> Vec<PathBuf> {
    let root = source.parent().unwrap_or_else(|| Path::new("."));
    let mut targets = Vec::with_capacity(session.tracks.len() + 1);
    targets.push(source.to_path_buf());
    targets.extend(session.extras.dependencies.iter().cloned());
    for track in &session.tracks {
        if let TrackSource::Midi(midi) = &track.source {
            if !midi.asset.starts_with("@source/") {
                targets.push(root.join(&midi.asset));
            }
        }
    }
    targets
}

fn finite_sample(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
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
