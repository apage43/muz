# Embedding muz

The default `desktop` Cargo feature includes the CLI, PipeWire output, native
VST3/CLAP hosting, filesystem watching, control socket, and render jobs. Existing
desktop builds retain these facilities. A library consumer can disable defaults
to build the language, compiler, musical model, inspection, native synthesis,
effects, and audio engine without those platform dependencies:

```sh
cargo check --lib --no-default-features --target wasm32-unknown-unknown
```

The portable build rejects native plugin instruments explicitly. It does not
provide browser audio output itself: the embedding host owns scheduling and
output buffers. `AudioEngine::render_interleaved` produces PCM at the host's
configured sample rate, in blocks no larger than its configured capacity.

## Unsaved source and module loading

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

`host::HostContext` owns expansion/evaluation/graph limits, cancellation and an
`Arc<dyn assets::AssetResolver>`. Use `compile_with_context(path, loader, &context)`
to compile and `context.run(|| AudioEngine::new(...))` (or transaction preparation)
to prepare with the same services. Scopes are synchronous, thread-local and
unwind-safe; explicitly install the context on each preparation thread. They do
not change audio callback behavior. Independent default contexts do not share
cancellation; CLI constructors retain the process interrupt flag and environment
graph-budget default.

Asset resolvers provide normalized identities, `(byte_length, revision_token)`
versions and independently seekable readers. `FileAssets` uses filesystem paths
and weak size/mtime tokens; `MemoryAssets` shares immutable bytes with independent
cursors. MIDI, structural SMF and WAV/FLAC readers use these services. Reads and
decoded frame counts are bounded. Hosts must normalize paths and enforce mount
boundaries themselves, and change tokens whenever asset bytes change. A scope
does not propagate automatically to a child process: desktop render workers use
filesystem assets. Native plugin loading/state APIs remain desktop-only; an
in-memory resolver does not imply browser plugin support.

## Editor inspection

`Compiled::locations` maps keys such as `track.lead`, `device.lead.instrument`,
and `route.lead.out` to the declaration locations already retained by lowering.
This is declaration attribution, not a complete expansion history of every
generated note. Locations serialize their path, one-based line and Unicode
character column, and source excerpt. Browser editors using UTF-16 positions
must convert character columns to their document offsets.

`lang::Diagnostic::to_json` exposes the same primary-location selection, help,
and caller locations as terminal diagnostics. Avoid parsing the terminal text
to recover locations.

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

`snapshot::SessionSummary::new(session, revision)` provides a bounded status view.
It retains exact track IDs (rejecting IDs above 1,024 bytes), abbreviates display
names and titles, and reports truncation after 1,000 tracks. Never use an
abbreviated display label as a lookup identity.

Detailed inspection uses one page contract:

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
`rows`, `total`, and `next`; pass `next` as the following offset. Track and tick
filters are applied before pagination where meaningful: tick ranges affect score,
performance, diagnostics, and performance overview, not sections or
second-based automation points. An offset past the end returns empty rows and no
continuation. A zero limit is an error. A requested revision must match the
retained compilation. Limits are 1–1,000 rows and 1 MiB of serialized row payload
per page; a single oversized row is an error. Nested patch nodes and automation
points are deliberately separate from their summaries. Dense arrangement views
should request `performance_overview`, then fetch visible `performance` ranges.
Overview bins include sorted unique MIDI `pitches` overlapping each interval,
so hosts can retain pitch contours and sustained spans without retrieving every
note. Their time resolution is approximate (at most 128 bins per track/range);
they are not individually selectable notes.
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
For compatibility, an otherwise unfiltered explicit `--view graph` (and the
equivalent socket request) returns the former full graph shape, but rejects it
above 1 MiB. `inspect::session` also preserves the old full automation and section
shapes within their row and byte limits. Its performance and patch-family results
use the new row shapes and reject rather than truncate when continuation is needed.

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

## Description and reload boundaries

Performed MIDI schedules are prepared against the candidate's single timeline.
Preparation rejects conservative callback bounds above the fixed event capacity,
including expression updates, releases and seek restoration. Revision application
also checks accumulated held-note obligations before any mutation. Dense material
may require simpler expression or fewer simultaneous events; events are never
silently dropped. Seek uses per-controller/message binary-search histories and a
prepared interval index for sounding notes, not scans over elapsed score history.

`description` owns shared Extras and parameter specifications; `diagnostic` owns
source-neutral locations/errors. Previous compile/source/lang imports remain
re-exports. The legacy `Device` DTO is accepted through `validate_device`, which
returns a tagged payload view; `patch_description::ValidatedPatch` checks typed
operations, node edges and configuration-independent ranges. Runtime preparation
still checks rates, resources, plugin support and sample regions.

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

For temporary listening controls, `AudioEngine::set_track_audibility(ids, ramp)`
accepts an exact physical-track allowlist (empty silences all tracks). It gates
both outputs and pre/post-fader sends after route delay compensation, without
changing the session, revision, transport, or running voices. `ramp=true` uses a
5 ms transition; `false` initializes the mask immediately. Hosts own mute/solo
and group policies and must reapply the mask after structural transactions.
Shared bus tails decay naturally; analysis taps and sidechain detectors remain
pre-mask so listening controls do not change musical processing.

Keep host UI and browser bridges in the consuming project. These interfaces are
general embedding facilities; they add no musical policies or language builtins.

## Expanded track groups

The graph's `extras.track_groups` retains authored groups after lowering. A kit
produces `{id, kind: "kit", members: [{track, label}]}`: `id` is the logical track
ID, `track` is a physical track ID, and `label` is the original kit voice name.
Hosts can group kit lanes without parsing dotted IDs, guessing from pitches or
instrument names, or losing custom voice labels. Membership is presentation
metadata; output routing, event streams, and track controls use physical IDs.
Empty kits have no members. Older snapshots without this optional field deserialize
with no groups. Source navigation uses `Compiled::locations["track." + id]` for
both the group and each expanded voice.
# Checked session preparation

Legacy loop-pattern schedules are also capacity-checked before playback. The
conservative bound includes repeated onsets/releases within a block, notes held
over multiple cycles, and obligations retained across edits. Ignored out-of-loop
notes are omitted from the prepared callback schedule. Pattern edits therefore
require `PreparedValueTransaction::prepare_with_config`, just like other schedule
edits. Config-free preparation remains available for non-schedule value changes.

The checked session captures its `HostContext`, including its cancellation token,
asset service and graph budget. Preparing it later uses that captured context;
telemetry allocates from the resulting engine's `graph_budget()`. Structural
cutovers reject a different budget before mutation. Expansion, graph preparation,
bounded reads and decoding have cancellation checkpoints off the audio thread.

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

Voice patches cross `patch_description::ValidatedPatch` once per preparation:
operations and outputs are typed, graph references resolve to node indices, and
lifetime, sample budget, module directory and voice mode are checked settings.
Controls are exposed separately from structural identity. Runtime preparation
consumes this representation directly; only recording-dependent regions and
sample-rate-dependent storage remain runtime checks. Source lowering uses the
same checked conversion as wire descriptions.

`description::ValidatedSession::new` checks an immutable session description
before processor construction. `AudioEngine::from_validated` accepts this checked
borrow and prepares audio-configuration-dependent state and external resources.
The compatibility `AudioEngine::new` entry point uses the same boundary. A checked
borrow does not certify external asset availability or runtime route scheduling;
those checks still run during preparation.
