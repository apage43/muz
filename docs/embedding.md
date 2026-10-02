# Embedding muz

Embed the Rust library when your application owns source documents, assets, and
audio output. This guide assumes familiarity with Rust and the
[song/track model](language.md#songs-and-tracks). Keep host UI and browser bridges
in the consuming project.

[Documentation index](README.md) · Offline: `muz docs embedding`

## Contents

- [Build capabilities](#build-capabilities)
- [Host lifecycle](#host-lifecycle)
- [Source loading and compilation](#source-loading-and-compilation)
- [Assets and versions](#assets-and-versions)
- [Validation and preparation](#validation-and-preparation)
- [Editor inspection](#editor-inspection)
- [Playable snapshots](#playable-snapshots)
- [Live revisions](#live-revisions)

## Build capabilities

The default `desktop` Cargo feature includes the CLI, PipeWire output, native
VST3/CLAP hosting, filesystem watching, control socket, and render jobs. Disable
default features for the language, compiler, model, inspection, native synthesis,
effects, and audio engine without those desktop dependencies.

For example, with the Rust WebAssembly target installed, run from the checkout:

```sh
cargo check --lib --no-default-features --target wasm32-unknown-unknown
```

The portable build rejects native plugin instruments explicitly. It does not
provide browser audio output itself: the embedding host owns scheduling and
output buffers. `AudioEngine::render_interleaved` produces PCM at the host's
configured sample rate, in blocks no larger than its configured capacity.

## Host lifecycle

1. Create a `HostContext` with limits, cancellation, and an asset resolver.
2. Provide a `SourceLoader` and compile the selected source revision.
3. Validate and prepare the resulting session under the same host context and
   audio configuration, outside the audio callback.
4. Render into host-owned output buffers in blocks within the configured capacity.
5. For edits, prepare a transaction off the audio thread and apply it at a block
   boundary. Retire old objects off the callback.

Source compilation, asset decoding, and processor construction belong to
preparation. The host is responsible for output scheduling, path boundaries,
asset revision tokens, and presentation policy.

### Host context

`host::HostContext` owns expansion/evaluation/graph limits, cancellation and an
`Arc<dyn assets::AssetResolver>`. Use `compile_with_context(path, loader, &context)`
to compile and `context.run(|| AudioEngine::new(...))` (or transaction preparation)
to prepare with the same services. Scopes are synchronous, thread-local and
unwind-safe; explicitly install the context on each preparation thread. They do
not change audio callback behavior. Independent default contexts do not share
cancellation; CLI constructors retain the process interrupt flag and environment
graph-budget default.

The checked session captures its `HostContext`, including its cancellation token,
asset service and graph budget. Preparing it later uses that captured context;
telemetry allocates from the resulting engine's `graph_budget()`. Structural
cutovers reject a different budget before mutation. Expansion, graph preparation,
bounded reads and decoding have cancellation checkpoints off the audio thread.

## Source loading and compilation

`lang::SourceLoader` supplies module identity resolution, source text, and the
contrib library root. `lang::load_with_loader` evaluates through that loader;
`compile::compile_with_loader` also lowers and checks the resulting session.
Pass an `Rc<dyn SourceLoader>`. A fresh compilation uses a fresh evaluator/cache,
so unsaved revisions are isolated. Imports, dependencies, and cycle detection
use the same host-provided identities. `FileSourceLoader` preserves ordinary
desktop behavior. Standard modules remain embedded; `lang::STANDARD_MODULES`
exposes their import names and source text for hosts that want to display them.

The host's `resolve` implementation must normalize equivalent paths consistently
and enforce its own filesystem/mount boundaries. Relative imports are passed
relative to the declaring module. `contrib_modules` optionally provides names
for missing-module suggestions.

## Assets and versions

Asset resolvers provide normalized identities, `(byte_length, revision_token)`
versions and independently seekable readers. `FileAssets` uses filesystem paths
and weak size/mtime tokens; `MemoryAssets` shares immutable bytes with independent
cursors. MIDI, structural SMF and WAV/FLAC readers use these services. Reads and
decoded frame counts are bounded. Hosts must normalize paths and enforce mount
boundaries themselves, and change tokens whenever asset bytes change. A scope
does not propagate automatically to a child process: desktop render workers use
filesystem assets. Native plugin loading/state APIs remain desktop-only; an
in-memory resolver does not imply browser plugin support.

`AssetResolver::snapshot` returns immutable bytes bound to a version. The default
implementation reads in bounded chunks and rejects metadata changes during the
read; memory assets share immutable storage directly. Hosts must change versions
whenever content changes. Device preparation also compares the opened version to
the authored stamp. WAV/FLAC decoding and VST3/CLAP state reads use this byte
service. Filesystem metadata versions are change detectors, not cryptographic
content identities or protection against a writer deliberately restoring metadata.

`audio_file::load_shared` shares native-rate stereo recordings within a host's
bounded weak cache, keyed by resolver identity, resolved path and version. Each
reader/sampler still owns its playback state; a cache hit still enforces the
caller's frame limit. Missing, stale, oversized and unsupported assets produce
distinct errors with the asset path.

## Validation and preparation

`description::ValidatedSession::new` checks an immutable session description
before processor construction. `AudioEngine::from_validated` accepts this checked
borrow and prepares audio-configuration-dependent state and external resources.
The compatibility `AudioEngine::new` entry point uses the same boundary. A checked
borrow does not certify external asset availability or runtime route scheduling;
those checks still run during preparation.

`description` owns shared Extras and parameter specifications; `diagnostic` owns
source-neutral locations/errors. Previous compile/source/lang imports remain
re-exports. The legacy `Device` DTO is accepted through `validate_device`, which
returns a tagged payload view; `patch_description::ValidatedPatch` checks typed
operations, node edges and configuration-independent ranges. Runtime preparation
still checks rates, resources, plugin support and sample regions.

Voice patches cross `patch_description::ValidatedPatch` once per preparation:
operations and outputs are typed, graph references resolve to node indices, and
lifetime, sample budget, module directory and voice mode are checked settings.
Controls are exposed separately from structural identity. Runtime preparation
consumes this representation directly; only recording-dependent regions and
sample-rate-dependent storage remain runtime checks. Source lowering uses the
same checked conversion as wire descriptions.

### Scheduled event capacity

Performed MIDI schedules are prepared against the candidate's single timeline.
Preparation rejects conservative callback bounds above the fixed event capacity,
including expression updates, releases and seek restoration. Revision application
also checks accumulated held-note obligations before any mutation. Dense material
may require simpler expression or fewer simultaneous events; events are never
silently dropped. Expression programs serialize only their actual points;
prepared voice state stays fixed-size on the audio thread. Seek uses per-controller/message binary-search histories and a
prepared interval index for sounding notes, not scans over elapsed score history.

Legacy loop-pattern schedules are also capacity-checked before playback. The
conservative bound includes repeated onsets/releases within a block, notes held
over multiple cycles, and obligations retained across edits. Ignored out-of-loop
notes are omitted from the prepared callback schedule. Pattern edits therefore
require `PreparedValueTransaction::prepare_with_config`, just like other schedule
edits. Config-free preparation remains available for non-schedule value changes.

## Editor inspection

### Locations and diagnostics

`Compiled::locations` maps keys such as `track.lead`, `device.lead.instrument`,
and `route.lead.out` to the declaration locations already retained by lowering.
This is declaration attribution, not a complete expansion history of every
generated note. Locations serialize their path, one-based line and Unicode
character column, and source excerpt. Browser editors using UTF-16 positions
must convert character columns to their document offsets.

`lang::Diagnostic::to_json` exposes the same primary-location selection, help,
and caller locations as terminal diagnostics. Avoid parsing the terminal text
to recover locations.

### Paged inspection

`snapshot::SessionSummary::new(session, revision)` provides a bounded status view.
It retains exact track IDs (rejecting IDs above 1,024 bytes), abbreviates display
names and titles, and reports truncation after 1,000 tracks. Never use an
abbreviated display label as a lookup identity.

Given a compiled revision and its revision number, request a page as follows:

```rust
let request = muz::inspect::PageRequest {
    revision: Some(accepted_revision),
    offset: 0,
    limit: 100,
    track: Some("lead".into()),
    start_tick: Some(0),
    end_tick: Some(4 * muz::compile::PPQ as u64),
};
let page = muz::inspect::page_compiled(
    &compiled, accepted_revision, "performance", &request
)?;
```

`page_session` supports `summary`, `graph`, `patches`, `patch_nodes`,
`patch_detail`, `automation`, `automation_points`, `sections`, `track_groups`,
`performance`, and `performance_overview`. `page_compiled` additionally supports
`score`, `diagnostics`, and `locations`. Responses contain `revision`, `view`,
`rows`, `total`, and `next`; pass `next` as the following offset.

Track and tick filters are applied before pagination where meaningful: tick ranges affect score,
performance, diagnostics, and performance overview, not sections or
second-based automation points. An offset past the end returns empty rows and no
continuation. A zero limit is an error. A requested revision must match the
retained compilation.

Limits are 1–1,000 rows and 1 MiB of serialized row payload per page; a single
oversized row is an error. Nested patch nodes and automation points have their
own views, separate from summaries. Dense arrangement views should request `performance_overview`, then fetch visible `performance` ranges.
Overview bins include sorted unique MIDI `pitches` overlapping each interval,
so hosts can retain pitch contours and sustained spans without retrieving every
note. Their time resolution is approximate (at most 128 bins per track/range);
they are not individually selectable notes.

Graph rows contain compact `transport`, `master`, `bus`, and `track` metadata,
in that order. Transport detail retains `mode` and `meter`, plus `bpm` and
`loop_ticks` for loops or `meter_source` for one-shot playback. Track source
summaries retain MIDI `ppq` and `end_tick`; outputs and sends retain routing and
gain metadata. Device details expose identity, controls, patch presence, and
plugin identity without plugin state, normalized SFZ programs, sample maps, or
patch bodies. Fetch performed notes, controllers, messages, and tempo changes
through `performance`; fetch patch metadata through the patch-family views.

Only selected detail rows are materialized; bounded serialization stops before
allocating an oversized encoded row. Counting/filtering still scans the relevant
in-memory collections and is not an incremental index.

The CLI uses the same page envelope and defaults `muz inspect FILE` to `summary`:

```sh
muz inspect song.muz --view performance --track lead \
  --start-tick 0 --end-tick 3840000 --limit 100
```

The control socket accepts the same `revision`, `offset`, `limit`, `track`,
`start_tick`, and `end_tick` fields. Its default inspect view is also `summary`.
The newline protocol wraps the page as `{"ok":true,"result":PAGE}`.
Graph requests use this page envelope even without filters; they never serialize
the full session as one row. `inspect::session` returns the compact graph rows as
an array and rejects rather than truncates when continuation is needed. It
retains the existing full automation and section shapes within their row and
byte limits; performance and patch-family results also use their paginated row
shapes.

### Expanded track groups

The `track_groups` inspection view retains authored groups after lowering;
serialized `Session` snapshots retain them in `extras.track_groups`. A kit
produces `{id, kind: "kit", members: [{track, label}]}`: `id` is the logical track
ID, `track` is a physical track ID, and `label` is the original kit voice name.
Hosts can group kit lanes without parsing dotted IDs, guessing from pitches or
instrument names, or losing custom voice labels. Membership is presentation
metadata; output routing, event streams, and track controls use physical IDs.
Empty kits have no members. Older snapshots without this optional field deserialize
with no groups. Source navigation uses `Compiled::locations["track." + id]` for
both the group and each expanded voice.

### Note provenance and revision differences

Enable `HostContext::provenance` to retain an interned occurrence chain for each
score note, its definition, and field-specific latest relevant edits. Placement,
repeat and expansion contexts share parents and are limited to depth 128 under
the host expansion budget. Imported MIDI explanations identify the resolved asset
path/version, source track and event order, with the import call only as a source
fallback. Provenance is optional and excluded from musical equality and playable
snapshots.

`Compiled::explain_notes(track, offset, limit)` returns bounded explanations.
`latest_pattern_call` is a compatibility alias for the latest relevant primitive
edit, as is the newer `latest_edit`; it is not a history of arbitrary returning
wrappers. `field_edits` is the precise field-to-location map and `occurrences`
lists the interned placement path. Identity comes from note keys, not source
offsets: named paths can survive unrelated insertions, whereas positional
components can shift. Renames appear as remove/add and duplicate keys are
reported as ambiguous, never guessed.

`provenance::diff_page(old, new, category, offset, limit)` pages `notes`,
`metadata`, `occurrences`, `controllers`, `messages`, `tempos`, `performed`, or
`consequences` independently. Note additions/removals and key renames are in
`occurrences`; `notes` contains changed sounding fields. Controller, message and
tempo rows compare ordered streams without inventing event identities. Metadata
names each changed note or session field. Consequence rows describe the predicted
reconciliation and processor retention/replacement; they do not assert that an
apply succeeded. `try_visit_processor_consequences` provides the same processor
decisions without allocating a complete JSON row vector.

## Playable snapshots

Ordinary `Session` serialization deliberately omits imported/performed event
arrays. It is an inspection description, not a playable transfer format. Use
`snapshot::PlayableSnapshotV1::capture`, `decode_checked(bytes, byte_limit)` and
`restore_checked` inside the receiving `HostContext`. The decoder honors the
smaller of the caller's limit and the 256 MiB format maximum. Version 1 requires exactly
one performed association per MIDI track, keyed by both track and source IDs;
duplicates, missing/unknown associations, summary mismatches, unordered streams,
invalid numeric/event buffers, incomplete manifests and stale assets are rejected.
Track order may change. Snapshots reference assets, not bundle their bytes. A
non-filesystem host may use `restore_description`, but must validate every manifest
revision through its own resolver before preparation. Transfers do not promise
future bit-identical plugin rendering.

## Live revisions

### Prepare and apply

`PreparedTransaction::prepare` classifies presentation, controls, schedules and
structural changes. Presentation and compatible native schedule changes construct
no processors. The static control subset is poly synth, gain and voice patch;
other controls conservatively prepare structurally. Automation/tail, resource,
mode and incompatible-source changes remain structural. Apply checks base
revision, an in-process description signature and prepared configuration before mutation, swaps prepared storage and
leaves retired objects in the transaction for destruction off callback. Use
`prepare_with_config` for direct value transactions changing timelines.

Portable hosts may pass `None` for the optional `event_started` `Instant`; that
field is latency telemetry, not preparation correctness. Structural transactions
expose `needs_fade`, `fade_out`, `fade_in`, and `fade_recovery`. Pass those masks
to `AudioEngine::render_interleaved_with_fade` at the host's block boundary;
`fade_recovery` restores the old engine if a prepared cutover cannot apply.

Compatible source tempo edits preserve held-note release/expression obligations
at their original remaining wall-clock times. Explicit seek/restart/mode changes
still create discontinuities. A shortened piece does not immediately cancel held
voices. No incremental compiler or cross-process capability cache is implied.

### Temporary listening controls

For temporary listening controls, `AudioEngine::set_track_audibility(ids, ramp)`
accepts an exact physical-track allowlist (empty silences all tracks). It gates
both outputs and pre/post-fader sends after route delay compensation, without
changing the session, revision, transport, or running voices. `ramp=true` uses a
5 ms transition; `false` initializes the mask immediately. Hosts own mute/solo
and group policies and must reapply the mask after structural transactions.
Shared bus tails decay naturally; analysis taps and sidechain detectors remain
pre-mask so listening controls do not change musical processing.

## Next steps

The [workflow guide](workflow.md) shows CLI and socket clients using these
facilities. For public Rust definitions, start with [host](../src/host.rs),
[inspection](../src/inspect.rs), and [transactions](../src/audio/transaction.rs).
These APIs expose general host facilities; musical policy belongs in source.

## SFZ assets and saved state

Native SFZ compilation stores a normalized `sfz::Program` in `model::SfzConfig`.
The resolver snapshots mapping sources/includes and samples during preparation;
all dependencies participate in device revision stamps. Host resolvers must
supply independently seekable readers and stable identities/versions for these
assets, including on WASM where a filesystem resolver is unavailable.

SFZ `DeviceState` defaults to **linked local assets**. Version 4 serializes a compact source/options descriptor, a normalized-program
SHA256 fingerprint, and the complete SHA256 dependency table, without copying
recordings or duplicating inherited regions. Restore verifies every dependency
before parsing through a resolver restricted to that closure, then compares the
reconstructed normalized fingerprint. Version 3 states remain readable. Missing or changed
libraries fail with a diagnostic; reinstall the identical licensed assets at the
saved paths. Automatic relocation is not currently provided. Embedding hosts can restore through
`DeviceState::decode_with_resolver` and `DeviceState::host_context_with_resolver` to provide the saved identities through
their own explicitly authorized asset resolver, including in WASM. Native CLAP
and DAWProject restoration authorizes only the five SFZ asset directories under
the normally discovered contrib installation (`MUZ_CONTRIB_DIR`, checkout, or
XDG installation). Custom libraries require `MUZ_SFZ_ASSET_ROOTS`, a platform
path-list of directories explicitly granted by the user. Serialized paths never
create grants; canonical resolution rejects symlinks escaping granted roots.
An embedding host must independently authorize its resolver before passing it to
`decode_with_resolver`; an implicit default HostContext does not grant native
filesystem access.

Set `embed_assets: true` only when redistribution is permitted. Embedding is
explicit, retains the existing 32 MiB total asset budget and 64 MiB state budget,
and rewrites normalized dependency/sample paths into the host's memory resolver.
Large libraries require linked state; this option does not grant redistribution
rights. Native and embedded restoration uses the saved normalized program, not
reparsing an original mapping at a machine-specific path.

Effect racks cannot contain SFZ instruments. State generation rejects this topology
before reading or packaging dependencies; save the SFZ instrument separately.

SFZ graph accounting reports every normalized region, distinct sample dependency,
modulation declaration and configured voice slot. Regions and sample identities
cost one graph unit each; modulation terms and voice slots cost one per 32 entries.
Large roots can exceed the default 4096-unit graph budget. Set a deliberate host
`graph_units` limit or `MUZ_GRAPH_BUDGET` (maximum 65536) after inspecting the
reported contributors; loading one root does not count as a single instrument
allocation. `sample_budget_frames` independently bounds shared decoded stereo
frames: the default is 64 Mi frames (512 MiB), and the maximum is 1024 Mi frames
(8 GiB). The original Virtuosity full kit needs about 4.9 GiB. Raise it explicitly
for a verified library footprint rather than relying
on a larger implicit default. Processor SFZ statistics expose decoded frames,
voice occupancy and steals/drops for host inspection.

Live SFZ seeks prepare a separate engine by replaying accepted history off the audio
thread, then swap it at a callback boundary. Old processors are retired on the
coordinator thread. Audition loops capture SFZ voices, DSP, note identities,
controllers, keyswitches, sequences and random state at the prepared start; each
wrap restores preallocated state and the event schedule. Effect tails retain their
existing loop behavior. Authored controller history repeats with the loop; controls
changed after the checkpoint return to the captured boundary value on wrap.

Replay is bounded to one hour of project frames per live transport request and
supports cancellation. While a source revision is pending, transport preparation
reports a retryable diagnostic. Accepted edits rebuild active SFZ loop checkpoints;
an invalidated range disables audition looping with a preparation diagnostic.
Seeking outside an active audition range returns to its start; seeking inside
replays forward from the captured start. Failed or superseded candidates preserve
the accepted runtime and are destroyed outside the callback.

WAV inspection exposes `audio_file::Info.compatibility` flags for two verified
PCM container quirks in pinned libraries: zero padding after a PCM `fmt` extension,
and a mono 24-bit `data` size that incorrectly includes its single final zero RIFF
pad. Metadata and decoding use the same validated frame range; original bytes,
hashes and sample identity remain unchanged. The decoder checks RIFF/chunk bounds,
base PCM layout and extension bounds, and rejects actual partial frames, nonzero
extra payload and truncation. These compatibility interpretations run off-thread
under the existing asset, decoded-frame and cancellation limits.


The metadata-only [state qualification](../contrib/sfz-state-qualification.json)
records actual linked roundtrips across all six SFZ families. Large normalized
programs are represented by compact descriptors: Standard KSOP uses 689,215 state
bytes, METAL Full 754,037, Shinyguitar 214,738, and Darkblack 01 405,066 on the
audited machine. These observations retain the unchanged 64 MiB cap and include
no recordings. See [reproduction instructions](sfz-state-qualification.md).
