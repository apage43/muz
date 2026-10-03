# Pending editor-host improvements

This is an implementation proposal, not a description of shipped APIs or completed
verification. It contains only reusable engine work. The companion
[muzpad workbench plan](../../muzpad/docs/workbench-improvements.md) owns application
policy, protocol integration, presentation, and deployment. Neither repository
should copy the other's task list.

## Scope and evidence

Read the [repository rules](../AGENTS.md), [documentation index](README.md),
[composer friction protocol](composer-friction.md), and current
[embedding contract](embedding.md) before implementation. Use focused synthetic
fixtures, never personal compositions as regression fixtures. Do not repair user
projects, metadata, plugin configuration, samples, or unrelated working changes.
The existing user edit to `composer-friction.md` is outside this planning commit.

Evidence below distinguishes current-source mechanisms from isolated runtime
observations. Investigation artifacts were reviewed to form this plan; durable
links here point to repository sources and maintained API documentation, not
agent/session artifacts. The user-reported screenshot is authoritative evidence
of an unhelpful error, but its exact initiating request/action is unknown.

| Finding | Evidence and qualification |
| --- | --- |
| Controls can be admitted behind a submitted revision while the public native baseline is still old. | [`NativeRuntime::check_control` and submission](../src/native.rs) check the accepted revision and capacity but not `revision_reserved`; receipt collection later promotes the baseline. [`pipewire` callback guards and `structural_cancellation_arbitrates_before_fade_not_after_it`](../src/audio/pipewire.rs) explicitly reject an old-revision control after structural cutover. This is source-traced reachability and an existing test read, not an executed screenshot reproduction. |
| Plugin restart has the same operation boundary. | [`prepare_plugin_restart`](../src/native.rs) uses the revision preparation/submission path; the host can retain the same public source ref while runtime revision advances. Source-ref filtering alone is insufficient. |
| Native detail capture is a due hardware block, not a rolling FFT window. | [`PreparedObservation::capture`](../src/audio/observation.rs) copies `frames.min(2048)` only when due. [`AudioEngine` capture site](../src/audio/engine.rs) supplies the block start sample position. Both core validation sites cap detail at 10 Hz. |
| The existing native child delivered the coded low cadence without drops in a small isolated fixture. | Investigation used the existing muzpad release child, reporting core revision `9fedb16a042bfaa7501eabb5e8bedcc7ec7c0778`, a synthetic one-track song and muted physical output. At 48 kHz / 256-frame period, six seconds yielded 60 analysis frames, each 256 waveform samples / 1024 spectrum bins; arrival p50 101.294 ms, p95 106.382 ms, zero observation/analysis drops and zero reported render/stream/transaction faults. This is child-runtime evidence, not full webview, current-source binary equivalence, callback CPU, absolute capture-to-paint age, or a representative personal-project profile. |
| Song-root identity is currently conflated with build/readiness. | Core already separates [`lang::load_with_loader`](../src/lang/mod.rs) from [`compile_with_loader` / `lower`](../src/compile.rs). The host's [native source worker](../../muzpad/src-tauri/src/engine/mod.rs), [pinning path](../../muzpad/src-tauri/src/engine/sources.rs), and [WASM check](../../muzpad/wasm/src/lib.rs) currently perform compile/resource work for entry checks. |
| Dependency failures can hide evaluated songs. | An isolated existing-binary scan of 261 candidates in 34 source-containing pieces returned 32 compile-and-pin positives and 229 negatives; evaluate-only comparison of 34 selected roots found 31 songs, two evaluation failures and one null root. These are different stages and denominators, not a classifier accuracy statistic. Native aggregate scan time was 31.559 s in a harness that bypassed parent eager capture/UI scheduling; it is not end-to-end latency. No piece becomes a core fixture or repair target. |
| Inspection output is bounded but input scanning is not indexed. | [`inspect::page_session` / `page_compiled`](../src/inspect.rs) scan matching streams and count rows even after page capacity. Overview scans all events and provides at most 128 occupancy bins. The [embedding contract](embedding.md#paged-inspection) explicitly documents this. No producer CPU or large-project speedup was measured. |

The native baseline confirms a 10 Hz ceiling and incomplete FFT input, not a slow
FFT implementation. At 48 kHz, 256 samples span 5.333 ms whereas a complete
2048-sample window spans 42.667 ms; zero padding cannot supply missing history.
The current host FFT and UI pipeline live outside core in
[muzpad's analysis worker](../../muzpad/src-tauri/src/engine/analysis.rs). Keep that
ownership rather than introduce a second core visualization/compiler subsystem.

## Implementation and pending-doc protocol

Each owning implementation commit must contain its focused regressions and update
relevant **live** API/reference documentation to match what actually ships. Remove
the implemented scope from this pending file in that same commit; retain only
explicitly unimplemented obligations. Remove the file once no pending scope
remains. Do not leave completed checklists, progress marks, resolved history,
compatibility shims, or a second friction archive. Resolve any applicable live
friction report only in its actual fixing commit, never during planning.

This commit adds only this pending proposal: no source changes, live-doc changes,
builds, tests, formatter runs, packaging, deployment, or engine-pin changes. Future
implementation owners perform the verification below; all acceptance targets are
planned, not measured outcomes.

## P0 Native admission and receipt integrity

### Contract

Extend the existing source-independent [`NativeRuntime`](../src/native.rs), not a
second queue/coordinator API. Preserve thread affinity and borrowed-session
ownership described in [native output coordination](embedding.md#native-output-coordinator).

- A revision reservation excludes publication of controls against the old native
  baseline until its terminal outcome is consumed. Apply this to every admission
  path, including running, panic, audibility, observation, ordinary loop controls,
  prepared transport and plugin-triggered restart. The outstanding window includes
  fade/apply and receipt waiting, including a receipt collected internally but not
  yet returned to the caller. Use the existing `revision_reserved` / pending state
  consistently; do not invent a source-aware scheduler inside core.
- Keep the callback's expected-revision/session guard as the final safety check.
  Low-level deliberately stale operations still reject before mutation. Admission
  refuses synchronously with a typed `Busy`/capacity/error result; refusal publishes
  no operation, changes no listening policy, and creates no promised callback
  outcome. Core does not silently defer, drop, or rebound an action to another
  revision. Hosts must consume errors rather than ignore queue failures.
- Every successfully admitted operation has exactly one terminal applied/rejected
  outcome owned by its operation ticket. Revision promotion, plugin stamps,
  observation consumers, resource lifetime and transport metadata follow that
  receipt, never submission or elapsed time. Preserve one submitted revision,
  64 reserved control outcomes and the coalesced plugin notice; telemetry cannot
  evict reliable outcomes. Preserve checked counter overflow behavior.
- Cancellation before the callback's pending-to-applying arbitration prevents
  mutation and still resolves an admitted operation. After commitment begins,
  await the authoritative applied/rejected result rather than pretend rollback.
  Preserve prepared-handle ownership on every `SubmitRevisionError`, including
  `QueueFull`; narrowly resubmitting that same still-valid owning handle is not a
  blanket retry mechanism for revision-pinned transport requests.
- Keep `StaleRevision`, `SessionMismatch`, `Cancelled`, `Superseded`, `Busy` and
  `QueueFull` distinguishable by typed cause. Preserve existing structured
  diagnostics. If the host cannot describe actual callback rejection with current
  DTOs, minimally extend the existing outcome/ticket context with expected and
  observed runtime revision, not a new diagnostic hierarchy or message matching.
- Policy changes before submission may invalidate an unsubmitted preparation.
  Preserve that guard; do not admit stale listening state. Host re-preparation of
  the already-compiled candidate must use acknowledged policy and valid source
  authorization, not falsely label internal supersession as user cancellation.

### Host dependency

The [workbench operation boundary](../../muzpad/docs/workbench-improvements.md#p0-operation-boundary-and-actionable-status)
continues beyond core receipt consumption through accepted metadata publication.
That host owner holds bounded deferred intentions, reconciles latest desired
analysis/listening state, settles explicit actions and owns compound Stop.
Core does not choose which user actions may be replaced or reissued. A pinned
seek/loop/track request must never be blindly replayed against changed source,
timeline or physical IDs. Safety-critical pause/panic ordering must be specified
and tested across active apply, not disabled during unrelated compilation.

Migrate affected existing embedding callers, especially [`live`](../src/live.rs)
and [`control`](../src/control.rs), so ordinary boundary refusal is handled without
swallowing errors, abandoning tickets or restarting native output. Do not widen
this task into a generic host retry facility.

### Focused regressions and acceptance

Extend [`native_tests`](../src/native_tests.rs) and the existing
[`pipewire` deterministic callback harness](../src/audio/pipewire.rs):

- Hold preparation, submitted value-only/structural apply, structural fade and
  applied-but-undelivered receipt separately; attempt each control kind. An old
  baseline control cannot enter behind the reserved revision through public native
  admission, even while its accepted runtime status still appears old.
- Cover plugin restart with unchanged public source identity/new runtime revision,
  rejected/cancelled revision, policy supersession before submit, poll with
  empty/full destination slices, queue saturation, repeated cancel and shutdown.
  The low-level stale-control guard regression remains meaningful and valid.
- Refusal has no mutation/reservation leak; applied/rejected outcomes occur once,
  preserve authoritative transport and ticket identity, and release the boundary
  at the correct point. No old source/session metadata is promoted.
- Under callback allocation instrumentation, revision/control apply, pool pressure,
  cancellation and retirement retain zero callback heap allocations **and frees**,
  no locks/waits or processor destruction. A headless harness establishes state
  invariants, not real hardware/plugin qualification.

Update [embedding admission/outcomes](embedding.md#revision-submission-and-outcomes)
and [transport/listening](embedding.md#transport-and-listening) in the fixing commit.

## P0 Evaluated root classification

### Contract

Add only a small reusable public language-root classification helper beside
[`lang::load_with_loader`](../src/lang/mod.rs), usable in native and portable
builds. It must invoke the same loader/evaluator and bound `main` versus last
expression semantics under the caller's existing [`HostContext`](../src/host.rs),
then inspect the evaluated root's record `type == "song"`. Reuse
[`Value` / `Record`](../src/lang/eval.rs) and current structured diagnostics.
Do not implement another parser, AST evaluator, compiler, static song detector,
entry scan manager or special native-only semantics.

The public result must distinguish evaluated **Song**, evaluated **NonSong**
(with a useful value-kind/type description), and **Unresolved** evaluation
(with the original structured diagnostic/dependency cause). A song is a root-kind
fact, not proof of valid tracks/graph, sample availability, supported native
plugins, preparation or playable audio. Operation cancellation/limit/global scan
failure must remain distinguishable from an ordinary candidate result; a host
cannot turn an aborted scan into confirmed NonSong or an empty result set.
Finalize minimal Rust names/signatures with the native/WASM consumers before the
core API commit; preserve existing `load_with_loader` callers and semantics.

Classification must not call `compile::lower`, `compile_with_context`, validation,
processor preparation, asset revision stamping, plugin alias/bundle pinning or
sample preparation merely to determine root kind. Reuse dependency identities
already produced by evaluation rather than returning heavyweight compiled/audio
objects. Fresh evaluations retain existing per-capture evaluator/cache isolation,
import resolution, cycle detection, expansion/evaluation limits and cancellation.

Evaluation itself can read imported source and resource-dependent builtins such
as MIDI. Therefore this is not a no-I/O or side-effect-free filesystem guarantee:
the host must supply its snapshot loader/resolver, authority and budgets **before**
evaluation, and evaluation dependency failure yields Unresolved. An AST-only hint
may help order work but never guarantees Song when evaluation remains unresolved.
Do not bypass such dependencies, fabricate assets, default a failed import, or
weaken actual compile/prepare rejection to make candidates appear playable.

### Host dependency and non-goals

The [workbench entry-classification owner](../../muzpad/docs/workbench-improvements.md#p0-entry-classification-and-discovery)
owns declared-candidate retention, deterministic defaults, scan scheduling,
incremental results, readiness policy, cancellation aggregation and UX. That
is the only application task reference here. No project retention/default policy
moves into core. The
[host trust/read policy](../../muzpad/docs/workbench-improvements.md#p0-native-read-policy-and-import-trust)
applies before imported external evaluation. Core's existing `SourceLoader`,
`AssetResolver`, `FileSourceLoader` and host-context hooks already support account
reads or restricted hosts; no global read-policy relaxation is required.
`ScopedFileAssets`, saved-state dependency digests, cancellation and resolver
version checks remain unchanged.

### Focused regressions and acceptance

Extend [`editor_host`](../tests/editor_host.rs) with in-memory synthetic sources:

- Direct song, bound `main`, imported/function-built song and last-expression
  roots agree with existing language loading. Null, pattern, ordinary records,
  strings/comments containing `song(` and wrong root tags are not Song.
- Syntax failure, missing import, import cycle and resource-dependent evaluation
  failure are Unresolved with locations/callers/cause preserved, never NonSong.
  Cancellation at evaluation boundaries remains distinguishable; budgets hold.
- When evaluation succeeds, roots with missing sample/SFZ/state, unconfigured or
  portable-unsupported native plugin, semantic/graph/playing-policy failures still
  classify Song; unchanged compile/prepare rejects readiness precisely.
- Instrument the supplied resolver/loader to prove classification does not trigger
  lowering-only asset stamping, sample decoding or plugin configuration/bundle
  capture. Evaluation-required reads remain explicit and authorized. Dirty/new/
  deleted in-memory imports and declaring-module/contrib resolution still follow
  the supplied loader, with no fallback to deleted project disk files.
- The same root-kind fixtures pass with and without the `desktop` feature.

Update [source loading](embedding.md#source-loading-and-compilation) to document
root classification versus compilation/readiness and its I/O limitations.

## P1 Continuous observation window contract

### Capture and ownership

Build on [`PreparedObservation` and its four-slot pool](../src/audio/observation.rs)
and existing [nondestructive observation](embedding.md#nondestructive-observation):

- While detailed capture is enabled/running, append the selected `(L + R) / 2`
  tap on **every** callback into preallocated 2048-sample circular storage,
  independent of publication deadlines and meter cadence. After warmup, publish
  the newest coherent chronological 2048-sample window at a bounded requested
  hop. Never concatenate sparse published blocks to simulate continuity.
- Accumulation continues even when no publication slot is available. Publish at
  most one newest window per callback; do not backfill a FIFO after a stalled
  consumer. Retain fixed four-slot exclusive ownership, overwrite/drop accounting,
  retirement epochs and latest-wins drain. No callback allocation/deallocation,
  JSON/serialization, FFT, locks/waits, ID resolution or reference destruction.
  Prepare mappings, histories and pool replacement off realtime thread.
- Reset/invalidate history on revision, selected source/capture replacement and
  transport generation changes, including seek/restart/loop discontinuity. Pause/
  disable cannot splice stale pre-pause data into new history. Restoration warms
  a fresh complete window. Never mix identities or label old data as current.
- Track taps remain post-insert/pan and pre-listening mask/routing; master remains
  processed audible output with transaction gain/fade. Stereo anti-phase may cancel
  under the chosen mono semantics; document it rather than imply stereo power.
  Capture does not change playback, routing, sidechains, masks or stream lifecycle.
- Meter and detail state remain independent. Meter-only frames have no detailed
  target/window; detail-only frames cannot erase retained meters or claim fresh
  metering. Hidden/disabled detail stops rolling-history work while independent
  meters and audio continue.

### Boundary metadata and cadence

Specify the existing observation DTO's revised window semantics before cross-repo
implementation. Each detailed frame carries native runtime revision, transport
generation, capture identity/selection epoch, selected target, strictly increasing
sequence, finite positive sample rate, actual contiguous sample count and an
unambiguous **exclusive window-end sample position**. Window start is end minus
count in that sample-clock domain. Partial warmup is explicitly incomplete (or
not published); only a full 2048 contiguous window qualifies for FFT2048. Never
represent zero padding as captured samples. Clarify meter/block timing separately
where a combined frame needs it, and handle clock arithmetic overflow explicitly.

A sample position is not a calibrated wall timestamp or a score tick. A generation
change retires comparisons across discontinuities. The host attaches its complete
accepted source ref; core does not store project/source text identities. Keep u64
values exact; the host bridge uses opaque decimal identities, not JS-number casts.

Make detailed publication cadence configurable within a documented finite ceiling,
using the existing selection/rate control. Update both
[`native` validation](../src/native.rs) and capture validation together, preferably
sharing the bound to prevent divergent acceptance. Zero still disables detail;
invalid frequencies/IDs reject rather than truncate. Preserve meter ceiling and
selection limits unless separate measured evidence justifies a change. The exact
new detail ceiling and selected rate must be agreed with host profiling/transport
budgets; a candidate 30 Hz on a measured 60 Hz display is a **planned qualification
setting**, not achieved performance or a universal 60 fps guarantee. Cadence is
not callback rate and must not create a new control request per captured frame.

Core supplies calibrated input/timing **boundaries**, not FFT/paint policy. The
[workbench analysis owner](../../muzpad/docs/workbench-improvements.md#p1-analysis-correctness-and-delivery)
owns off-RT FFT plan/window/scratch reuse, coherent-gain scaling (including DC and
Nyquist conventions), frequency `k * sampleRate / 2048`, bounded publication,
transport coalescing, web parity and visible rAF painting. Do not move host FFT,
JSON, IPC or visualization into callback or introduce playback PCM transport.

### Focused regressions and acceptance

Extend [`observation` tests](../src/audio/observation.rs),
[`pipewire` capture/allocation tests](../src/audio/pipewire.rs), and public native
observation tests in [`native_tests`](../src/native_tests.rs):

- Deterministic ramps/sines across 128/256/512/non-divisor and supported larger
  callback blocks yield exactly the last 2048 chronological contiguous samples
  after warmup, with correct count, exclusive end position and sample rate at
  44.1/48/96 kHz. At 48 kHz the full window represents 42.667 ms regardless of
  callback block size. Host FFT calibration uses these same synthetic fixtures.
- Seek/loop wrap/revision/reorder/removal/source replacement, pause/resume and
  disable/re-enable flush history and reject retired windows. Removed physical
  targets disable detail; retained meters remain keyed by identity.
- Independent clocks cover meter-only/detail-only/different-rate deliveries, empty
  destination slots, pool stalls, replacement and overflow. Latest complete detail
  stays coherent, drops are counted, memory stays bounded and no catch-up queue
  forms. Allocation instrumentation still reports zero callback allocations/frees.
- Measure callback capture cost versus the current sparse capture in a controlled
  native fixture; preallocation is not proof that continuous copying is free.
  No new render/stream/transaction fault or extra stream start may be traded for
  visual cadence. Record environment, period and enabled streams; distinguish
  headless state tests from native-output qualification.
- At the negotiated publication period, cadence tracks that period plus measured
  callback/coordinator scheduling jitter rather than remaining near the old 100 ms
  ceiling. Measure rather than claim an arbitrary age/speedup. Absolute paint age
  requires valid host clock calibration and remains a workbench acceptance item.

Update [nondestructive observation](embedding.md#nondestructive-observation) in the
owning API commit. The workbench integration owner then performs the core pin,
child/WASM/resource update and coherent bridge-limit migration before deployment.

## Conditional inspection indexes and stable metadata

This is a profile-gated extension, **not** a prerequisite or unconditional compiler
rewrite. The [workbench viewport owner](../../muzpad/docs/workbench-improvements.md#p1-coherent-viewport-and-inspection)
first fixes bounded caching/request coalescing, loaded coverage, scheduling and
coordinator isolation where measured. Only if those measurements still identify
core range scans/overview work or missing producer metadata as material blockers
should an owning core commit select the smallest necessary item here.

Use existing [`inspect::PageRequest` / page envelopes](../src/inspect.rs) and
[inspection contracts](embedding.md#paged-inspection). Preserve retained revision
identity, exact ticks, row/byte limits, totals, first-omitted-row continuation,
stream ordering and explicit errors. Do not restore full unbounded performance
snapshots or increase budgets as a speed fix.

Eligible measured changes are revision-owned off-RT indexes for sorted point-event
ranges and interval-aware note overlap, bounded precomputed overview/LOD data, or
compact stable whole-track metadata such as pitch bounds. A start-only binary
search is incorrect for sustained notes entering from the left. Index construction,
retained bytes, retirement and query CPU must fit explicit measured budgets;
indexes must not leak across revisions or add callback work. Do not clone a second
accepted session/compiler merely to service inspection.

Keep semantic distinctions: current pitched overview is duration occupancy,
including unique active pitches; kit hit density is onset count. Any necessary
additive representation must state its semantics and expose real aggregates,
not invented velocities. Stable pitch bounds come from the complete accepted
track, not the current tile; a host can instead choose a stable scale policy
without any core API change. Complete track-catalog paging already exists; no
catalog endpoint is added just because the app currently ignores continuations.

If the profile gate opens, extend [`inspection_pages`](../tests/inspection_pages.rs)
with synthetic long notes crossing left/right boundaries, exact boundary/empty
ranges, controller/message/tempo pressure, offset/byte continuations, pitch-bound
stability and density/LOD consistency. Compare indexed results to existing precise
range semantics and record construction/query time and retained bytes on the same
bounded synthetic corpus. No implementation or speedup is assumed in this plan.
Update the relevant live inspection documentation only for changes actually shipped.

## Future verification handoff

Run checks once after owning implementation changes and caller migrations land,
not in this planning commit. Commands below are a feature matrix, not evidence
that anything was exercised here:

- Native/default feature: focused library harnesses with `cargo test --lib
  native_tests`, `cargo test --lib observation`, and `cargo test --lib pipewire`;
  focused integration suites with `cargo test --test editor_host --test
  inspection_pages`. Add targeted filters for newly introduced regressions.
- Portable: `cargo test --no-default-features --test editor_host --test
  inspection_pages` for shared classification/inspection semantics, and
  `cargo check --no-default-features --lib --target wasm32-unknown-unknown` with
  the target installed, matching [build capabilities](embedding.md#build-capabilities).
  Native-only types must not leak into portable classification/inspection APIs.
- The integration owner qualifies actual native output and the existing
  application boundary suites after its pin/protocol cutover. Headless tests do
  not certify installed native plugins or hardware timing; app/browser acceptance
  belongs only in the companion plan, not duplicated here.

Report exactly the commands/outcomes observed, native environment and measurement
scope. Capture regression evidence for the planned invariants; never present the
isolated investigation baseline or an unexecuted acceptance target as proof of
implemented behavior. No language features, compiler overhaul, general telemetry,
new trust framework in core, or user music repairs belong in these commits.
