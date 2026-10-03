# Source-independent native runtime: remaining work

Implement the reusable native boundary required by the Linux desktop host. This
plan owns only muz-core work; muzpad owns its Tauri 2 trusted bundled UI, owned
engine child, source snapshots, project safety, wire protocol, inspection handles,
exports, and packaging. Native plugin code remains trusted code inside that child.
No new language, editor socket protocol, plugin sandbox, or backend framework is
part of this work. No source implementation or verification is implied by this plan.

Each implementation commit must remove its implemented scope here and update the
embedding/workflow documentation in the same commit. Move the enduring API contract
below into `docs/embedding.md` as it ships; delete this plan when exhausted. Do not
retain completed task lists or mirror muzpad's application tasks.

## 1. Extract one coordinator, with host-owned compilation

Owner: `src/native.rs` (new), `src/lib.rs`, `src/live.rs`, native-output internals
in `src/audio/pipewire.rs`, `src/audio/engine.rs`, `src/audio/transaction.rs`, and
focused tests. Keep this one implementation owner while callback/coordinator
ownership changes. Update `src/control.rs` and CLI callers only where the clean
migration requires it; preserve existing socket behavior and wire fields.

Extract, rather than copy, the existing LiveSession lifecycle: reconciliation and
prepared transaction submission/receipts, CLAP main-thread servicing, plugin
restart flags/generations, runtime device mapping, SFZ replay/checkpoints,
transport engine retirement, and shutdown. Retain source parsing, JSON5 support,
watch targets/debounce, latest queued source candidate, reload diagnostics and
latency presentation in LiveSession. It must own a NativeRuntime instead of its
own PipeWireOutput/plugin/transport lifecycle. Preserve explicit headless CLI and
test operation through a crate-private constructor; public NativeRuntime::start
always attempts actual native output and never silently falls back.

NativeRuntime is thread-affine, not Send or Sync. Construct, prepare, service,
poll, discard prepared objects, and shut it down on one plugin coordinator thread.
Keep the thread available between operations to service plugins and receipts;
plugin construction/replay may block it, so cancellation is cooperative, not a
promise to interrupt foreign plugin code. The application supervisor handles a
hung child. Do not spawn an uncoordinated preparation thread for plugin objects.

Borrow Session and HostContext only for each call. Neither NativeRuntime nor a
prepared handle stores a Rust borrow of a caller's Session or Compiled. Runtime
retains revision/signature/ID mappings, policy and necessary owned engine state,
not an additional cloned accepted Session. Preserve the existing transaction's
owned descriptions where structural reconciliation needs them; do not add an
extra Compiled/graph clone merely to satisfy coordinator ownership. Host retains
accepted and candidate Compiled values and promotes only on RevisionApplied.
Inspection, explanation, source locations, provenance diffs and accepted export
pins remain host-owned. A plugin restart does not recompile disk or edited source.

### Frozen Rust value contract

Public names below belong to `muz::native`. They are Rust host DTOs, not serde or
IPC requirements. The desktop adapter maps them to its versioned wire DTOs and
encodes u64 identities losslessly. Existing `audio::{AudioConfig, PipeWireStatus,
TransportSnapshot, DeliveredEvents}` and `model::{Id, DeviceKind, Vst3Config}` are
reused. Move RuntimeDeviceStatus from live to native and migrate all Rust imports;
keep its existing serialized field shape, not a compatibility re-export.

```rust
use crate::{Session, host::HostContext};
use crate::audio::{AudioConfig, PipeWireStatus, TransportSnapshot, DeliveredEvents};
use crate::model::{Id, DeviceKind, Vst3Config};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeOperationId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisionTicket {
    pub operation: NativeOperationId,
    pub base_revision: u64,
    pub revision: u64,
    pub source_generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlTicket {
    pub operation: NativeOperationId,
    pub expected_revision: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginStamp { pub generation: u64 }
#[derive(Clone, Debug)]
pub struct PluginRestartRequest {
    pub expected_revision: u64,
    pub stamp: PluginStamp,
    pub instance_tokens: Vec<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeErrorCode {
    Cancelled, Superseded, StaleRevision, SessionMismatch, Busy, QueueFull,
    InvalidRequest, UnknownTrack, ResourceLimit, RevisionOverflow,
    Prepare, Apply, AudioUnavailable, OutputFault, Shutdown, UnknownOperation,
}
#[derive(Clone, Debug)]
pub struct NativeError {
    pub code: NativeErrorCode,
    pub message: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelDisposition { Prevented, PendingOutcome, AlreadyResolved }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeControlKind { Running, Restart, Panic, Seek, Loop, Audibility, Observation }
#[derive(Clone, Copy, Debug)]
pub struct NativeTransportStatus {
    pub generation: u64,
    pub snapshot: TransportSnapshot,
    pub loop_range: Option<(u64, u64)>,
    pub end_tick: u64,
    pub end_project_frame: f64,
}
#[derive(Clone, Debug)]
pub struct RuntimeDeviceStatus {
    pub restart_flags: u32,
    pub id: Id,
    pub kind: DeviceKind,
    pub instance_token: u64,
    pub process_count: u64,
    pub gain_reduction_db: f32,
    pub latency_samples: u32,
    pub tail_samples: u32,
    pub is_plugin: bool,
    pub plugin: Option<Vst3Config>,
}
#[derive(Clone, Debug)]
pub struct NativeStatus {
    pub accepted_revision: u64,
    pub runtime_revision: u64,
    pub source_generation: u64,
    pub pending_revision: Option<RevisionTicket>,
    pub transport: NativeTransportStatus,
    pub last_peak: f32,
    pub last_rms: f32,
    pub delivered_events: DeliveredEvents,
    pub devices: Vec<RuntimeDeviceStatus>,
    pub runtime_mapping_pending: bool,
    pub stream_start_count: u64,
    pub audio: PipeWireStatus,
    pub observation_dropped: u64,
}
#[derive(Clone, Debug)]
pub enum NativeEvent {
    RevisionApplied {
        ticket: RevisionTicket, transport: NativeTransportStatus,
        callback_count: u64, latency_ms: f64, structural: bool, faded: bool,
        plugin_stamp: PluginStamp,
    },
    RevisionRejected { ticket: RevisionTicket, error: NativeError },
    ControlApplied {
        ticket: ControlTicket, kind: NativeControlKind,
        transport: NativeTransportStatus,
    },
    ControlRejected {
        ticket: ControlTicket, kind: NativeControlKind, error: NativeError,
    },
    PluginRestartRequested(PluginRestartRequest),
    OutputFault(NativeError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrainCount { pub written: usize, pub remaining: bool }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservationTarget { Master, Track(String) }
#[derive(Clone, Debug)]
pub struct ObservationSelection {
    pub meter_tracks: Vec<String>,
    pub master_meter: bool,
    pub analysis: Option<ObservationTarget>,
    pub meter_hz: u8,
    pub analysis_hz: u8,
}
#[derive(Clone, Debug)]
pub struct TrackMeter { pub id: String, pub peak: f32, pub rms: f32 }
#[derive(Clone, Debug)]
pub struct ObservationFrame {
    pub revision: u64,
    pub transport_generation: u64,
    pub sequence: u64,
    pub sample_position: u64,
    pub sample_rate: f32,
    pub running: bool,
    pub master: Option<[f32; 2]>, // peak, RMS
    pub tracks: Vec<TrackMeter>,
    pub target: Option<ObservationTarget>,
    pub mono_samples: Vec<f32>,
    pub dropped: u64,
}
pub struct ShutdownReport {
    pub events: Vec<NativeEvent>,
    pub status: NativeStatus,
    pub error: Option<NativeError>,
}
```

NativeRuntime and PreparedNativeRevision have private fields. PreparedNativeRevision
owns the prepared transaction, cancellation/identity and precomputed cutover
policy, not a caller borrow, and is also thread-affine. Implement Debug without
formatting plugin objects. `SubmitRevisionError` is a public struct with
`pub error: NativeError` and `pub prepared: PreparedNativeRevision`: every failed
submit, including stale/cancelled/queue-full, returns ownership. Implement Debug,
Display and Error without requiring Sync on the prepared payload. NativeError
implements Display and Error. Do not erase ownership into anyhow on queue-full.

### Frozen signatures

The following is a compile-ready consumer type-assertion specification **once the
planned API exists**, not an instruction to add an API trait or stub implementation.
These are inherent methods, and the consuming crate may use these exact types.
No compilation of this specification has been run in this planning wave.

```rust
use muz::{Session, host::HostContext};
use muz::audio::AudioConfig;
use muz::native::*;

const _: fn(&Session, &HostContext, bool) -> Result<NativeRuntime, NativeError>
    = NativeRuntime::start;
const _: fn(&NativeRuntime) -> AudioConfig = NativeRuntime::audio_config;
const _: fn(&NativeRuntime) -> NativeStatus = NativeRuntime::status;
const _: fn(&NativeRuntime) -> PluginStamp = NativeRuntime::plugin_stamp;
const _: fn(&PluginStamp, &mut Session) = PluginStamp::apply;
const _: fn(&PreparedNativeRevision) -> RevisionTicket = PreparedNativeRevision::ticket;
const _: fn(&mut NativeRuntime, &Session, &Session, u64, u64, &HostContext)
    -> Result<PreparedNativeRevision, NativeError> = NativeRuntime::prepare_revision;
const _: fn(&mut NativeRuntime, PreparedNativeRevision)
    -> Result<RevisionTicket, SubmitRevisionError> = NativeRuntime::submit_revision;
const _: fn(&mut NativeRuntime, NativeOperationId)
    -> Result<CancelDisposition, NativeError> = NativeRuntime::cancel_operation;
const _: fn(&mut NativeRuntime, &mut [Option<NativeEvent>]) -> DrainCount
    = NativeRuntime::poll;
const _: fn(&mut NativeRuntime, &Session, u64) -> Result<(), NativeError>
    = NativeRuntime::service_plugins;
const _: fn(&mut NativeRuntime, &Session, u64, u64, &PluginRestartRequest, &HostContext)
    -> Result<PreparedNativeRevision, NativeError> = NativeRuntime::prepare_plugin_restart;
const _: fn(&mut NativeRuntime, u64, bool) -> Result<ControlTicket, NativeError>
    = NativeRuntime::set_running;
const _: fn(&mut NativeRuntime, &Session, u64, &HostContext)
    -> Result<ControlTicket, NativeError> = NativeRuntime::restart;
const _: fn(&mut NativeRuntime, u64) -> Result<ControlTicket, NativeError>
    = NativeRuntime::panic;
const _: fn(&mut NativeRuntime, &Session, u64, u64, &HostContext)
    -> Result<ControlTicket, NativeError> = NativeRuntime::seek_ticks;
const _: fn(&mut NativeRuntime, &Session, u64, Option<(u64, u64)>, &HostContext)
    -> Result<ControlTicket, NativeError> = NativeRuntime::set_loop;
const _: fn(&mut NativeRuntime, u64, &[String]) -> Result<ControlTicket, NativeError>
    = NativeRuntime::set_audibility;
const _: fn(&mut NativeRuntime, u64, &ObservationSelection)
    -> Result<ControlTicket, NativeError> = NativeRuntime::set_observation;
const _: fn(&mut NativeRuntime, &mut [Option<ObservationFrame>]) -> DrainCount
    = NativeRuntime::drain_observations;
const _: fn(NativeRuntime) -> ShutdownReport = NativeRuntime::shutdown;
```

Argument order for prepare_revision is accepted, candidate, base_revision,
source_generation, context. For prepare_plugin_restart it is accepted,
expected_revision, source_generation, request, context. Tick/range units are core
integer ticks (TICKS_PER_BEAT), not seconds or JavaScript beats.

## 2. Revision, cancellation, context and retirement invariants

Start at accepted/runtime revision 0 and source_generation 0. Revisions advance
only on an applied receipt, including presentation-only and plugin-restart
transactions; failed attempts may reuse base+1, but operation IDs never repeat.
Use checked counters, reject overflow. Source generation is caller metadata, not
an engine revision or automatic cancellation counter; plugin restarts may retain
the same source generation. Validate the borrowed accepted Session's signature
against the runtime's accepted baseline before preparation/servicing/transport.

Permit one submitted revision and at most one owned prepared revision per runtime;
new preparation supersedes an earlier unsubmitted prepared handle using an
internal generation guard. Submitted work is not implicitly cancelled. The host
keeps one coalesced latest compile request; return Busy rather than prepare against
an unacknowledged baseline. Prepared handles need not be Clone. Queue-full is
retryable without repeating expensive preparation. Dropping a never-submitted
handle on the coordinator cancels it without a callback outcome.

Install the supplied context using HostContext::run on the actual coordinator
thread for startup, all revision/plugin preparation and SFZ replay. Each request
has a fresh cancellation token, with accepted asset resolver/cache/budgets supplied
by the host for transport/restart. Never cancel a shared accepted-session/export
token to supersede a candidate. Context clones only share intended service Arcs;
no ambient-thread assumption or process-global cancellation in the desktop path.

Before publishing prepared state check cancellation, base/signature, preparation
generation and plugin stamp. At callback cutover check those applicable again
before any fade/mutation. Use an atomic operation lifecycle with a definitive
pending-to-applying/cancelled arbitration point; do not infer not-applied merely
from reading a boolean after submission. Cancellation that wins yields a reliable
rejected receipt; cancellation after applying starts returns PendingOutcome, and
the authoritative applied/rejected receipt still arrives. Prevented means no
mutation can occur; its submitted operation still receives one rejection.
AlreadyResolved applies to the bounded retained outcome window; unknown/expired
IDs return UnknownOperation, never fabricated success. Keep the last 64 resolved
operation identities for cancellation classification. The host retains delivered
results if it needs longer request history.

Reserve reliable outcome storage before accepting each operation. Bound accepted
outstanding controls to 64, submitted revisions to one, and pending plugin-restart
notice to one; reject capacity exhaustion synchronously. Every successfully
submitted ticket has exactly one terminal event, including stale, superseded,
shutdown and cancelled requests. Never drop operation outcomes to make room for
telemetry. poll is nonblocking, services receipts/retirement, writes only into
empty output slots and reports whether more events remain; zero-length/full
output slices are safe. It does not borrow the host's newly accepted Session or
compile/watch/reload it. The caller must promote returned applied revisions before
servicing/preparing again. Expose runtime_mapping_pending while callback state
has advanced but its matching receipt has not yet been promoted by the coordinator;
do not attach old device IDs to new device telemetry.

Reuse existing prepared transaction fade/recovery and ownership-return receipts.
No callback allocations, locks, string resolution, plugin destruction or dropping
producer-owned payloads. Retire transactions, old transport engines, masks and
capture buffers on the coordinator after receipt. Stop/join the callback before
reclaiming final engine state. shutdown consumes runtime, reconciles outstanding
operations after callback quiescence, returns their bounded final events and any
output failure, and destroys plugin state on its owning thread. A Drop fallback
must still reclaim safely; normal callers use shutdown to retain outcomes.

## 3. Plugin servicing and baseline promotion

service_plugins calls the existing CLAP main-thread service and maps restart flags
only against the matching accepted Session/revision. Coalesce restart requests by
instance token and emit PluginRestartRequested via poll; a pending source revision
defers restart preparation. No source watcher or recompilation runs here.

PluginStamp::apply performs the existing stamp_plugins traversal of track
instruments/inserts, buses/master and rack-bearing devices. Before ordinary
candidate preparation, the host calls runtime.plugin_stamp().apply on that
candidate Session; prepare rejects an incompatible stamp rather than quietly
changing its borrowed description. Initial stamp is 0. A requested restart uses
checked current-generation+1, temporarily clones the accepted Session for existing
transaction preparation, and applies the requested stamp to that clone. This is
the required restart candidate, not an extra permanent accepted graph.

prepare_plugin_restart returns the same prepared-revision type. The request must
still match revision and device tokens; failure/cancellation must not permanently
suppress restart flags. Commit the new stamp and acknowledge affected instances
only after receipt. RevisionApplied.plugin_stamp tells the host to apply that
stamp to its accepted Compiled.session (or owned legacy Session) before the next
borrow. Score/provenance/locations remain the same; no whole Compiled clone or disk
compile is required. Hosts retaining immutable old export/diff handles preserve
their old description by their existing revision pinning policy before mutation.
A failed restart leaves both accepted description and stamp unchanged. Consolidate
stamp, device mapping and restart bookkeeping here; delete LiveSession duplicates.

## 4. SFZ-safe transport and listening policy

All commands carry expected revision and receive ControlApplied/ControlRejected
with callback transport generation/status. A queued command becoming stale must
be rejected before mutation, not applied to a replacement engine. Increment
transport generation on discontinuities/transport state changes; audibility and
observation changes do not invent a seek. Preserve FIFO ordering for admitted
commands; any explicit coalescing must emit Superseded for each replaced ticket.

Reuse AudioEngine::prepare_seek/prepare_loop, ValidatedSession and the existing
one-hour replay frame ceiling derived from negotiated sample rate. A loop is
half-open start < end; enabling it seeks to start, disabling retains current
position. A seek outside an active loop clamps to its start. restart seeks loop
start or zero without changing running state. set_running(false) pauses without
rewinding; panic is voice termination, not fabricated transport reset. The desktop
adapter combines pause and acknowledged seek for its Stop action, and owns
play-at-end policy. Never implement SFZ nonzero seek/loop via bare output seek.

Prepare loop/checkpoint state against the candidate Session and context before
revision publication, so a rebuilt SFZ engine has valid loop state at cutover.
Reuse the existing replay/checkpoint algorithms, not a second SFZ renderer. Clear
an existing audition range that exceeds a shortened candidate's end at that
cutover; report the resulting range in its receipt. A valid loop/replay preparation
failure rejects the revision and preserves accepted audio, rather than accepting
and silently losing its loop. Reapply listening/observation mappings to every
structural or replay engine before publication. Reject a preparation whose
captured policy changed before submission, requiring a reprepare; do not apply a
stale listening state merely because source revision matches.

set_audibility validates exact physical IDs against cached accepted mapping,
rejects unknown IDs/duplicates, and prepares an index mask off callback. An empty
allowlist means silence. Apply the existing 5 ms route ramp, gating output and
pre/post-fader sends after compensation, preserving voices, insert processing,
pre-mask analysis and sidechains. Default policy before the first explicit mask
is all tracks; an explicit mask survives rebuild by ID intersection, with new IDs
silent until host policy supplies them. Muzpad alone resolves mute/solo/groups and
can submit the new exact allowlist after acceptance; core must never infer group
names or alter authored Session controls.

Add the output entrypoint `PipeWireOutput::submit_audibility` using a prepared,
revision-guarded owned payload plus reliable control receipt/retirement. Route all
native coordinator listening changes through it; do not call string-based
AudioEngine::set_track_audibility on the callback. Keep direct offline audibility
API behavior for existing hosts. Extend queued output transport/prepared transport
with operation identity, revision guards and reliable outcomes, migrating all
callers instead of retaining a second unguarded native coordinator route.

## 5. Bounded nondestructive observation

Add `PipeWireOutput::set_observation` and `drain_observations`, consumed only through
the coordinator by the desktop host. Their internal prepared-payload types may
remain crate-private; the frozen public host surface is NativeRuntime above.
Prepare index maps and capture storage off callback. Never use AudioEngine::set_tap
for live analysis: it replaces the rendered output and latency selection.

Capture master after master processing and one selected physical track after its
inserts/pan, before routing/listening mask. Selection cannot change audible output,
sidechains, latency compensation, transport or musical revision. Meter values are
finite peak/RMS; master meters reflect the actual audible mix, track meters reflect
the defined pre-mask tap. Keep observation independent of offline destructive tap
selection. Calculate any FFT, waveform reduction and wire serialization outside
the callback; mono_samples is the actual contiguous (L+R)/2 capture, not decimated
samples mislabeled with the original sample rate.

Hard limits: 1,000 explicitly selected track meters, master meter optional, one
optional detailed target, at most 20 meter frames/s, at most 10 detailed frames/s,
at most 2,048 mono samples in a detailed frame. Validate frequencies as 1..=20 and
1..=10 when their respective capture is enabled; zero disables that stream. An
all-disabled selection stops capture. Unknown targets/oversized selections return
an error rather than silently truncating. These limits do not cap playable tracks;
large projects select a bounded meter subset. Use a preallocated four-slot capture
pool and bounded latest-wins delivery, with monotonic sequence and cumulative
dropped counts. No consumer backpressure may stall audio. Independent rate clocks
mean meter-only frames may have empty mono_samples and target None.

ObservationFrame identifies revision, transport generation, capture sample
position/rate and selected target. After pause emit cleared meters/no fabricated
waveform and running=false. A removed target disables detailed capture on revision
cutover, and the next frame exposes target None; remaining selected meter IDs are
intersected without mislabeling indices. Flush old-revision capture on cutover,
retiring storage outside the callback. drain_observations writes only empty caller
slots, is nonblocking, returns bounded counts, and never drains reliable events.
Reusable fixed callback scratch and ownership-return queues must ensure allocation
and deallocation counters stay zero during capture, policy changes and retirement
pressure. Host FFT output is bounded to 1,024 bins; PCM for native playback never
crosses application IPC.

## 6. Runnable implementation boundaries and acceptance

1. Implement the source-independent coordinator and clean LiveSession migration
   together with receipt identities, cancellation arbitration, contexts, plugin
   servicing/stamps, SFZ replay and shutdown. Preserve CLI JSON5/watch/socket
   behavior. No desktop caller should independently reproduce these lifecycles.
2. Extend output/engine queued audibility and nondestructive capture using that
   coordinator's identity/retirement path. Finish both before desktop listening
   and analysis acceptance; no success-shaped stub capability flags.
3. Update `docs/embedding.md`, `docs/workflow.md` and documentation index where
   needed, with exact ownership/thread/cancellation/transport/capture semantics.
   Add focused synthetic cases in native runtime/observation tests; extend existing
   `tests/{lifecycle,revision_boundaries,portable_transactions,host_context,
   audition,sfz_live_transport,sfz_seek,sfz_pause}.rs`. Use synthetic sources,
   never a user's composition or installed sample library as a regression fixture.

Implementation acceptance must exercise real NativeRuntime APIs, not merely mock
host outcomes: accepted borrowed Session/Compiled retention; failed preparation
and apply preserve the prior revision; stale/mismatched/superseded and cancelled
operations at both sides of submission; queue-full returns usable ownership;
bounded output slices and stalled reliable consumers; no lost/duplicate results;
plugin callbacks/restart retry/stamp promotion without disk access; matching
source generations across source and restart revisions; fresh scoped cancellation
and accepted resolver/cache use; SFZ pause/nonzero seek/loop/rebuild checkpoints,
replay limit and cancellation; audibility ramps/routes/groups resolved by caller;
mask retention on structural/replay replacement; observation-on/off leaves output
samples identical, bounded capture/overflow/paused behavior and correct IDs after
reorder/removal; callback no-allocation/no-deallocation under retirement pressure;
shutdown before/after apply returns final outcomes and destroys on owning thread.
Keep native-output-independent deterministic callback tests plus explicit
headless lifecycle tests; headless success is not hardware verification.

Main integration verification, after implementation lands, should run the focused
core tests above and existing editor-host/inspection/provenance/semantic-diff and
snapshot tests for retained metadata/exports. Add CLAP/VST3 installed-plugin and
actual PipeWire acceptance in the desktop integration report, including plugin
restart/failure and real audio health; report only exercised plugin coverage. This
planning wave runs no builds, lint, tests, formatters or native runtime checks.

Muzpad may implement shell/project/supervisor and compile/inspection/export adapter
code against the frozen signatures now. Real native rebuild/transport acceptance
depends on boundary 1; audition/meters/waveform/spectrum depend on boundary 2.
Muzpad owns pinning accepted asset bytes/versioned resolvers and providing them to
existing render_with/playable-snapshot APIs; no new core export process framework
is justified. If those existing facilities prove insufficient, establish a concrete
reusable gap before modifying them. Core does not own project/disk generations,
engine-process epochs, recovery, permission grants, wire framing or source loading.
