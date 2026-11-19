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
- [Native output coordinator](#native-output-coordinator)
- [Cached renders and freezing](#cached-renders-and-freezing)

## Build capabilities

The default `desktop` Cargo feature includes the CLI, PipeWire output, native
VST3/CLAP hosting, filesystem watching, control socket, and render jobs. Disable
default features for the language, compiler, model, inspection, native synthesis,
effects, and audio engine without those desktop dependencies.

For example, with the Rust WebAssembly target installed, run from the checkout:

```sh
cargo check --lib --no-default-features --target wasm32-unknown-unknown
```

The default `jit` feature compiles voice patches to native code with
Cranelift when they are prepared ([compiled patches](synthesis.md#compiled-patches));
a portable build leaves it out and interprets them, with identical output. A
host whose process may not generate code at run time needs nothing: patches
fall back to the interpreter. `muz::audio::set_patch_jit(false)` asks for the
interpreter outright.

The portable build rejects native plugin instruments explicitly. It does not
provide browser audio output itself: the embedding host owns scheduling and
output buffers. `AudioEngine::render_interleaved` produces PCM at the host's
configured sample rate, in blocks no larger than its configured capacity.

## Host lifecycle

1. Create a `HostContext` with limits, cancellation, and an asset resolver.
2. Provide a `SourceLoader` and compile the selected source revision.
3. For native output, construct `native::NativeRuntime` on a dedicated coordinator
   thread; prepare, submit, poll, and service it on that same thread.
4. For portable or application-owned output, validate and prepare under the host
   context, then render into host-owned buffers within the configured capacity.
5. Retain accepted and candidate compilations in the host. Promote a native
   candidate only on `NativeEvent::RevisionApplied`; retire old callback objects
   through the coordinator.

Source compilation, asset decoding, and processor construction belong to
preparation. The host owns path boundaries, asset revision tokens, and presentation
policy. `NativeRuntime` owns native output scheduling and processor lifecycle;
portable hosts own their output scheduling.

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
cutovers reject a different budget before mutation.

The graph budget has two ceilings. `model::MAX_LIVE_GRAPH_BUDGET` (65,536) bounds
live output, whose callback telemetry tables are sized by the budget value;
`NativeRuntime::start` and `prepare_revision` refuse a larger `graph_units` with
`ResourceLimit` (`model::check_live_graph_budget`). Preparation itself, and so
every offline render, accepts up to `model::MAX_OFFLINE_GRAPH_BUDGET` (1,048,576),
since an offline render has no deadline and no callback telemetry; memory is its
only bound. `MUZ_GRAPH_BUDGET` accepts the offline ceiling, so `muz render` can
use it while `muz serve` refuses it. `MAX_GRAPH_BUDGET` remains the live ceiling. Expansion, graph preparation,
bounded reads and decoding have cancellation checkpoints off the audio thread.

## Source loading and compilation

`lang::SourceLoader` supplies module identity resolution, source text, and the
contrib library root. `lang::load_with_loader` evaluates through that loader;
`compile::compile_with_loader` also lowers and checks the resulting session.
Pass an `Rc<dyn SourceLoader>`. These entry points use a fresh evaluator, so
unsaved revisions are isolated; a host that recompiles after each edit can keep
unchanged modules with a [module cache](#recompiling-after-an-edit). Imports,
dependencies, and cycle detection use the same host-provided identities. `FileSourceLoader` preserves ordinary
desktop behavior. Standard modules remain embedded; `lang::STANDARD_MODULES`
exposes their import names and source text for hosts that want to display them.

The host's `resolve` implementation must normalize equivalent paths consistently
and enforce its own filesystem/mount boundaries. Relative imports are passed
relative to the declaring module. `contrib_modules` optionally provides names
for missing-module suggestions.

`lang::classify_root_with_context(path, loader, &context)` uses the same evaluator,
bound `main`/last-expression selection and dependency identities without lowering,
validation, asset stamping or processor preparation. `RootClassification` is
`Song { dependencies }`, `NonSong { description, dependencies }`, or
`Unresolved { error }` preserving the original structured evaluation diagnostic.
Cancellation and evaluation/expansion budgets return the outer error, never a
confirmed non-song or a successfully empty scan. A song root means only an
evaluated record with `type == "song"`: missing samples/state, unconfigured plugins,
invalid graphs and unsupported playback policies still fail actual compilation or
preparation. Classification is not guaranteed I/O-free: imports and evaluation
builtins such as MIDI use the caller's loader/resolver. Authorize snapshot reads,
trust and budgets before evaluation; there is no global resolver relaxation.

### Recompiling after an edit

An editor or watcher that recompiles a piece on each save should keep one
`lang::ModuleCache` and one `HostContext` across revisions and compile with
`compile::compile_with_cache(path, loader, &context, &cache)` (or evaluate with
`lang::load_with_cache`). The result, diagnostics, dependencies and note
provenance included, is the one a fresh compilation returns. The evaluation
report's operation counts can differ only where a reused module's `memo` calls
found results another, re-evaluated module had computed. `live::LiveSession` does this for `muz` live sessions.

The cache keeps each module's evaluated value with what its evaluation read: its
source text, each import's requested and resolved identity, each asset a builtin
read (`midi`, `midi_tempos`, `clip`) with its resolution and version, and its
operations, provenance entries and memo results. A later evaluation reuses a
module when the loader returns the same text, every import resolves to the same
module and that module is itself reused, and every asset resolves the same way
with the same version. Reuse loads the module's imports in their original order,
then restores its dependencies, operation count, provenance entries and memo
results as if it had been evaluated. An edited module is therefore evaluated
again with every module that imports it, directly or not; modules it imports are
reused. A module that reads `steps()` is never kept. A different root, contrib
root, provenance setting, expansion limits or evaluation budget clears the cache.
A reuse that would cross the evaluation, provenance or call-depth budget falls
back to evaluation, which fails as a fresh one does.

The loader must keep resolution stable for a cache's lifetime and the resolver
must change an asset's version whenever its bytes change, as preparation already
requires. `ModuleCache` holds `Rc` values: keep it on the compiling thread.
`ModuleCache::reused()` lists the modules the last evaluation reused, which a host
can show or log. Lineage, occurrence and memo numbers that only tell values apart
during tracing are unique in the process rather than per evaluation, so values
from different evaluations never collide.

Lowering reuses normalized SFZ programs through `HostContext::sfz_sources`
(`sfz::LoadedPrograms`, at most 16): a program is reused for the same resolver,
path and options while its root resolves the same way and every dependency (SFZ
sources and samples) keeps its version. Compiled devices share the kept program
(`SfzConfig::program` is an `Arc`); a change through `Arc::make_mut` copies it.
On Kingfisher Line (23 tracks, a 12,000-region SFZ guitar) a one-line edit to the
root recompiles in about 0.3 s with both caches, against 9 s before; a fresh
compilation takes about 2 s. An edit inside an imported module costs that
module's own evaluation as well (0.4 s for the guitar parts, 0.8 s for the winds).

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

Native plugin aliases resolve their configuration through the active
`HostContext.assets` snapshot, with an 8 MiB limit and config-relative plugin/state
paths preserved. A host must pin the configured file bytes (or explicit `{}` when
there are no aliases). Resolver `NotFound` means an empty alias map; other grant,
version, and read failures propagate. There is no fallback disk read that bypasses
the supplied resolver.

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
Preparation computes each track's conservative per-block callback bound,
including expression updates, releases and seek restoration, and sizes that
track's event buffer from it: the bound plus headroom for a later revision's held
notes, the larger of the bound itself and `MAX_EVENTS_PER_BLOCK` (256). The
buffer is allocated during preparation and never grows on the callback;
`ScheduledEvents` refuses a push past its capacity rather than allocating. There
is no fixed per-block ceiling and no setting to raise one.

Instruments may accept fewer events than a schedule's bound.
`DeviceProcessor::reserve_scheduled_events` grows a device's per-block conversion
storage during preparation, and `scheduled_event_capacity` reports what it then
accepts. CLAP grows to any bound. A VST3 instrument accepts at most 1,022
scheduled events per block, bounded by its 2,048-value parameter queues, which
also hold a block of sample-accurate automation. Native instruments have no
limit. Preparation refuses a schedule its instrument cannot accept. A track also
holds at most `MAX_ACTIVE_NOTES` (256) notes at once, and preparation refuses a
schedule that could hold more.

Revision application checks accumulated held-note obligations before any
mutation. The incoming schedule's bound plus the held notes' releases and
remaining expression updates must fit the incoming schedule's buffer and the
receiving instrument. The receiving instrument is the retained one when a
structural transaction keeps it, and that instrument was sized for its old
schedule. The incoming notes plus the held ones must also fit the active-note
table. A revision that does not fit is refused unchanged. Events are never
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

Graph validation retains source diagnostics in
`audio::EngineError::Source(Box<Diagnostic>)`, including unknown automation targets,
latency-changing targets, and duplicate lanes. The diagnostic is boxed to keep
ordinary engine errors small. Its `Display` remains the source
diagnostic's terminal display, while `Error::source` exposes the typed
`lang::Diagnostic`. When using `anyhow`, a root `downcast_ref` alone does not
recover a diagnostic nested inside an engine error; search the source chain:

```rust
let diagnostic = error.chain()
    .find_map(|cause| cause.downcast_ref::<muz::lang::Diagnostic>())
    .map(muz::lang::Diagnostic::to_json);
```

`Diagnostic::named` and `Diagnostic::locate` also retain structured diagnostics
found in source chains when adding fallback attribution. Keep the structured
`message`, `location`, and `help` fields rather than scraping formatted text.
The CLI prints the recovered diagnostic once, rather than duplicating its
terminal rendering through the outer engine error and its source.

Native runtime boundaries preserve the same diagnostic in the public
`NativeError::diagnostic: Option<Box<Diagnostic>>` field and expose it through
`Error::source`. Existing `code`, `message`, and `Display` behavior remain
unchanged. Hosts can consume that field directly or use the same source-chain
lookup above; `SubmitRevisionError` also exposes its `NativeError` source.
Unknown automation targets may compile successfully and fail only during
`NativeRuntime::prepare_revision`, so map preparation errors as structured
failures too, without promoting the rejected candidate.

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
`notes` counts duration occupancy, not new attacks. Additive `onsets` counts only
note starts inside each half-open bin and the requested range; kit-hit density
should use this field rather than infer velocity or count sustained occupancy.

Graph rows contain compact `transport`, `master`, `bus`, and `track` metadata,
in that order. Transport detail retains `mode` (always `one_shot`), `meter`, and
`meter_source`. Track source
summaries retain MIDI `ppq` and `end_tick`; outputs and sends retain routing and
gain metadata. Device details expose identity, controls, patch presence, and
plugin identity without plugin state, normalized SFZ programs, sample maps, or
patch bodies. Fetch performed notes, controllers, messages, and tempo changes
through `performance`; fetch patch metadata through the patch-family views.

Only selected detail rows are materialized; bounded serialization stops before
allocating an oversized encoded row. Counting/filtering still scans the relevant
in-memory collections and is not an incremental index.

#### Inspection cost and host policy

The bounded producer remains scan-based: no revision-owned inspection index,
precomputed overview store, or whole-track pitch-bound endpoint is retained.
Hosts should complete catalog continuations, cache bounded track-filtered pages,
coalesce navigation requests, and choose a stable pitch scale independently of
the currently loaded tile. These policies do not require another accepted
compiler/session or a larger inspection budget.

Qualification used the unchanged workbench default viewport fixture: 18,696 UTF-8
source bytes, 172 physical tracks, 150 buses and 38,028 notes, with provenance
enabled. Its source SHA-256 was
`652c47b809630cad5a2fb915a6c6db1d2e086782f63aa2579410c749a9e87807`.
The optimized native producer and corrected Chromium host were measured
separately; their costs are not interchangeable:

| Measured boundary | Observed p95 |
| --- | --- |
| Native selected `lane0` performance, beats [64,80), query plus final page serialization | 0.141 ms |
| Native selected overview, largest measured case | 0.512 ms |
| Native complete graph catalog, 324 rows / 108,980 encoded bytes | 3.378 ms |
| Browser selected performance, combined WASM query and serialization | 0.3 ms |
| Browser selected overview, combined WASM query and serialization | 1.0 ms |
| Browser host inspection including queue, worker delivery, parsing and adapter | 7.6 ms |
| Browser structural layout / drawing | 0.5 / 2.9 ms |

Actual browser navigation exercised more than 128 pages; retained cache entries
stayed capped at 128, with peak encoded retention 1,806,659 bytes and at most four
active requests. Native live Rust heap accounting measured 76,455,389 bytes for
the retained compilation, 39,470,919 bytes for the session-only baseline and
89,155,079 bytes at compile peak; 40 dropped page/serialization results retained
zero additional bytes. These are allocation-layout measurements, not RSS or
WASM linear-memory capacity.

The joint producer/host profile did not identify a material core scan, overview,
or metadata blocker, so no index or producer API expansion was selected.
All-track stress queries are not the corrected host's visible-track request
shape. These observations are fixture-specific, exclude audio-callback work,
and do not establish an end-to-end frame rate or analysis/display age.

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
fallback. Raw events (`note_on`, `note_off`, `cc`, `bend` and the other channel,
SysEx and meta calls, and `raw_midi`) are traced the same way: each keeps the call
that made it, its placement occurrences and its latest relevant edits (`at`,
`offset_ms` and `bytes`). Provenance is optional and excluded from musical equality
and playable snapshots.

`Compiled::explain_notes(track, offset, limit)` returns bounded explanations.
`latest_pattern_call` is a compatibility alias for the latest relevant primitive
edit, as is the newer `latest_edit`; it is not a history of arbitrary returning
wrappers. `field_edits` is the precise field-to-location map and `occurrences`
lists the interned placement path. Identity comes from note keys, not source
offsets: named paths can survive unrelated insertions, whereas positional
components can shift. Renames appear as remove/add and duplicate keys are
reported as ambiguous, never guessed.

After a track's score notes, the page continues with one row per raw note: a
note-on with nonzero velocity in the score track's `pattern.raw`, keyed `raw<N>`
by its index there, the same id the compiled `MidiNote` carries. `total` counts
both. A raw note's `definition` is its `note_on` call; its `occurrences` and
`field_edits` are the note-on event's (a `map_raw` that changes its bytes is its
latest edit). The note-off and other raw events keep provenance on
`RawEvent::provenance` but have no rows. A raw note's identity is positional
(`identity_quality` says so): adding or removing an earlier raw event renumbers
it.

`provenance::diff_page(old, new, category, offset, limit)` pages `notes`,
`metadata`, `occurrences`, `controllers`, `messages`, `tempos`, `performed`, or
`consequences` independently. Note additions/removals and key renames are in
`occurrences`; `notes` contains changed sounding fields. Controller, message and
tempo rows compare ordered streams without inventing event identities. Metadata
names each changed note or session field. Consequence rows describe the predicted
reconciliation and processor retention/replacement; they do not assert that an
apply succeeded. `try_visit_processor_consequences` provides the same processor
decisions without allocating a complete JSON row vector.

### Keyswitch presses

An instrument's [named keyswitches](instruments.md#named-keyswitches) are on
its device: `Device::keyswitches` is a `BTreeMap<String, model::Keyswitch>`,
name to `Keyswitch { key: u8, channel: Option<u8> }` (channel 0-based, `None`
for every channel). It holds the declared entries and, for an `sfz()`
instrument, the program's `sw_label` keys. It serializes as
`"keyswitches": {"pinch": {"key": 21, "channel": 15}}` and is omitted when empty.

Notes are not rewritten. Ask the track which press a performed note is:

```rust
for track in &compiled.session.tracks {
    let muz::model::TrackSource::Midi(source) = &track.source;
    for note in &source.imported.notes {
        match track.keyswitch(note) {
            Some(name) => println!("{} presses {name} at tick {}", track.id, note.start_tick),
            None => println!("{} sounds key {}", track.id, note.key),
        }
    }
}
```

`Device::keyswitch(channel, key)` is the same lookup without a note; an entry
naming the channel wins over an any-channel one. The lookup covers every
performed note: score notes, raw `note_on`/`note_off` pairs and imported MIDI.
The table is descriptive. It is outside `Device::same_structural_identity`, so
a change never replaces a processor, and outside output fingerprints.
Paged inspection shows it too: the `graph` view lists `keyswitches` with the
instrument, and a `performance` note row that is a press carries
`"keyswitch": name`.

### Pitch bend in semitones

An instrument's [bend range](instruments.md#bend-range) is on its device:
`Device::bend_range` is an `Option<model::BendRange>`, `BendRange { up: f64,
down: f64 }` in semitones. It holds the declaration or, for an `sfz()`
instrument, the program's uniform `bend_up`/`bend_down`. It serializes as
`"bend_range": {"up": 2.0, "down": 12.0}` and is omitted when absent.

Bend messages stay as they are in `ImportedMidi::messages` (status `0xE0`).
Ask the track what each one means, or what is sounding at a tick:

```rust
let track = &compiled.session.tracks[0];
let muz::model::TrackSource::Midi(source) = &track.source;
for message in &source.imported.messages {
    if let Some((channel, semitones)) = track.bend(message) {
        println!("channel {channel} bends {semitones:+.2} st at tick {}", message.tick);
    }
}
// The last bend at or before the tick on that channel, 0 before the first.
let now: Option<f64> = track.bend_at(2, 960);
```

Both return `None` when the instrument declares no range. `bend_at` searches
the sorted messages, so a viewer drawing a whole lane should walk them once
with `bend` instead. `BendRange::semitones(value)` converts a 14-bit value
(0..=16383, center 8192) directly; it inverts `bend(v)`, so `bend(1)` reads as
exactly `up` and `bend(-1)` as exactly `-down`. RPN 0 messages in the stream
are not read. Per-note tuning expression (`Program::value`) is a separate,
range-free path.

The range is descriptive. It is outside `Device::same_structural_identity`,
so a change never replaces a processor, and outside output fingerprints.
Paged inspection shows it too: the `graph` view lists `bend_range` with the
instrument, and a `performance` message row that is a bend carries
`"bend_semitones"`.

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
structural changes (`transaction::preparation_effect` reports which). Only
structural revisions construct processors or fade; the others prepare off the
callback and apply allocation-free:

- **Presentation:** names, titles, sections and the song `tail`. The tail only
  bounds offline renders; live playback never reads it.
- **Controls:** a native device's control parameters (every device with
  parameter specs, voice patches, gain), and a route's level, such as a track
  `gain:` or a send level. A level change keeps the route's connection, so no
  track fades. Plugin, rack and SFZ controls belong to their instance and stay
  structural.
- **Schedules:** compatible MIDI sources, transport and tempo changes, and
  changed automation points whose targets are unchanged. Each route's level and
  lanes and each device's lanes are swapped into the running engine;
  processors, delay lines and voices stay. A note-expression check needs the
  instrument, so a source may change only to expression slots the running
  instrument already accepted: native instruments then check values against
  the standard slot ranges, while plugins and racks, whose ranges belong to the
  instance, must already have accepted each exact value.

Adding, removing, moving or replacing devices, routes or tracks, new or removed
automation targets, resource, latency and mode parameters, and incompatible
sources remain structural. Apply checks base revision, an in-process description
signature and prepared configuration before mutation, swaps prepared storage and
leaves retired objects in the transaction for destruction off callback. Use
`prepare_with_config` for direct value transactions changing timelines.

A structural transaction prepares a complete candidate engine, then swaps the
running processors of the devices it keeps into it. The candidate's own
processors for those devices never run. `PreparedTransaction::recycle_into`
moves them from the applied transaction into an `audio::ProcessorPool`, and
`prepare_with_pool` places pooled processors at the next structural revision's
kept devices instead of preparing them again. After the first structural
revision, each prepares only the native devices it adds or replaces. Plugins and
racks are never pooled and are prepared on every structural revision.
`NativeRuntime` keeps a pool. The pool holds one processor per pooled device
(shared sample and program caches are not copied) and drops entries when the
audio configuration changes or a device leaves the session.

Portable hosts may pass `None` for the optional `event_started` `Instant`; that
field is latency telemetry, not preparation correctness. Structural transactions
expose `needs_fade`, `fade_out`, `fade_in`, and `fade_recovery`. Pass those masks
to `AudioEngine::render_interleaved_with_fade` at the host's block boundary;
`fade_recovery` restores the old engine if a prepared cutover cannot apply.

Compatible source tempo edits preserve held-note release/expression obligations
at their original remaining wall-clock times. Explicit seek/restart/mode changes
still create discontinuities. A shortened piece does not immediately cancel held
voices. Recompiling with a [module cache](#recompiling-after-an-edit) reuses
unchanged modules within one process; there is no cross-process cache.

### Temporary listening controls

For temporary listening controls, `AudioEngine::set_track_audibility(ids, ramp)`
accepts an exact physical-track allowlist (empty silences all tracks). It gates
both outputs and pre/post-fader sends after route delay compensation, without
changing the session, revision, transport, or running voices. `ramp=true` uses a
5 ms transition; `false` initializes the mask immediately. Hosts own mute/solo
and group policies and must reapply the mask after structural transactions.
Shared bus tails decay naturally; analysis taps and sidechain detectors remain
pre-mask so listening controls do not change musical processing.

## Native output coordinator

With the `desktop` feature, `native::NativeRuntime` is the source-independent
native output boundary. It consumes borrowed `Session` descriptions, not paths,
source text, watchers, or an owned `Compiled`. Its public DTOs and exact method
signatures live in [src/native.rs](../src/native.rs). `RuntimeDeviceStatus` belongs
to `muz::native`; its serialized device fields remain unchanged.

### Ownership and startup

Call `NativeRuntime::start(&session, &context, running)` on the plugin coordinator
thread. Public startup attempts real native output and returns an audio error
when unavailable; it never falls back to a silent clock or browser engine.
Explicit CLI `--headless` and lifecycle tests use the crate-private backend
constructor. A headless result is not a hardware or installed-plugin qualification.

The runtime and `PreparedNativeRevision` are neither `Send` nor `Sync`. Construct,
prepare, service, poll, discard prepared handles, and shut down on their owning
thread. Keep that thread available between requests for CLAP main-thread work and
receipt retirement. Preparation/replay may block in foreign plugin code;
cooperative cancellation cannot interrupt a hung plugin. A host needing failure
isolation supervises its own engine process.

No runtime or prepared handle borrows the host's `Session` or `Compiled` after a
call. The host retains its accepted and candidate compilations, inspection data,
source locations, provenance, and export pins. The runtime retains signatures,
ID mappings, listening policy, and owned engine/prepared state, not another
permanent accepted `Session` clone.

Each startup, preparation, or replay installs the supplied `HostContext::run`
on the actual coordinator thread. Clone the accepted resolver, decoded cache,
and budgets for a new operation, but give that operation a fresh cancellation
`Arc<AtomicBool>`. Do not cancel an accepted-session or export token to supersede
a candidate. Context scopes do not propagate to other threads automatically.

### Revision submission and outcomes

Accepted/runtime revision and source generation start at zero. To rebuild:

1. Compile a candidate in the host, preserving the accepted compilation.
2. Apply `runtime.plugin_stamp().apply(&mut candidate.session)`.
3. Call `prepare_revision(accepted, candidate, base_revision, source_generation,
   context)`. The accepted signature and revision must match the runtime.
4. Submit the returned owning handle with `submit_revision`.
5. Drain `poll` and promote only its matching `RevisionApplied`. Apply that
   event's `plugin_stamp` to the newly accepted description before another borrow.
   `RevisionRejected` leaves the old description and audio accepted.

Every successful submission returns a `RevisionTicket` containing its operation,
base revision, candidate revision, and caller-supplied source generation. Revisions
advance only on an applied receipt, including presentation-only and plugin-restart
revisions. Failed attempts may reuse `base + 1`; operation IDs never repeat.
Counters reject overflow. Source generation is host metadata, not a revision or an
automatic cancellation counter.

There is at most one submitted revision and one owned unsubmitted preparation.
Another preparation supersedes an older unsubmitted handle, not submitted work.
Preparing against an unacknowledged baseline returns `Busy`; retain only the
latest queued compile request in the host. `SubmitRevisionError` returns both
`error` and `prepared: Box<PreparedNativeRevision>` on every refusal. In particular,
retry `QueueFull` with `runtime.submit_revision(*failure.prepared)` rather than
repeating expensive construction or erasing ownership into an `anyhow::Error`.

`poll(&mut [Option<NativeEvent>])` is nonblocking, writes only empty slots, and
returns `DrainCount { written, remaining }`. Empty/full slices are safe. Consume
and promote returned revisions before servicing or preparing against the accepted
description. Polling also retires callback-owned objects on the coordinator;
it never recompiles or watches source.

Each admitted revision or control receives exactly one terminal applied/rejected
event. Reliable outcomes are reserved before admission: one submitted revision,
64 outstanding controls, and one coalesced plugin-restart notice. Capacity
exhaustion rejects synchronously; telemetry never displaces operation outcomes.

A submitted revision reserves admission until its terminal event is actually
returned in an empty `poll` destination slot. All controls, including pause/panic,
listening/observation, loop and prepared transport, synchronously return typed
`Busy` during that boundary; no command, listening change or promised control
outcome is published. Internal receipt collection into a full/empty destination
does not release it. The callback's expected-revision guard remains the final
safety check for deliberately low-level stale operations. Rejections preserve
their ticket plus `NativeError.expected_revision` / `observed_revision` when
callback revision context is available. Hosts own bounded deferred intentions,
safety-critical ordering and metadata publication beyond the core boundary.

`cancel_operation` arbitrates with the callback's pending-to-applying transition.
`Prevented` guarantees no mutation and a submitted operation still receives a
rejection. `PendingOutcome` means application may already have started; await the
authoritative event rather than claim rollback.
`AlreadyResolved` covers undelivered terminal outcomes and the last 64 resolved
identities; unknown or expired IDs return `UnknownOperation`. A dropped,
never-submitted handle cancels without a callback outcome. Retain results in the
host if longer request history is needed.

`shutdown` consumes the runtime, stops/joins output before reclamation, and
returns `ShutdownReport { events, status, error }` with final outcomes. Destroy
plugins and retired transactions on the coordinator, not the callback. Drop is
a safe reclamation fallback; use explicit shutdown to retain final outcomes.

### Plugin servicing and status

Call `service_plugins(accepted, expected_revision)` between operations. It services
CLAP main-thread requests and emits a coalesced `PluginRestartRequested`, keyed
by actual instance tokens. Source work may defer that restart. If callback state
has advanced before its receipt is promoted, drain events before servicing again.

Idle servicing is cheap (well under a millisecond, linear in tracks and
devices), so call it as often as the coordinator is free. Every call that
borrows the accepted description checks it against the accepted baseline. The
first borrow after each accepted revision is hashed in full, which is one pass over the
description, including resolved SFZ regions (about 100 ms for a very large
sampled instrument). After that, the runtime trusts the same borrow while its
address, the accepted revision and a structural fingerprint (track and device
identities, plugin generations, event and region counts) recur. The borrow the
runtime was started from counts as already checked. This covers
`service_plugins`, `seek_ticks`, `set_loop`, `restart` and
`prepare_plugin_restart`'s admission. `prepare_revision` and the preparation
behind a plugin restart always hash in full. Do not edit the accepted Session
in place between borrows except to apply a returned plugin stamp. An edit that
keeps the fingerprint is caught only at the next preparation.

Use `prepare_plugin_restart` with the accepted description and the received
request, then submit the same prepared-handle type. This temporarily clones and
stamps the accepted Session for preparation; it does not recompile disk or edited
source, and does not clone the whole Compiled. Restart success advances the
revision but may keep source generation unchanged. Apply the returned plugin
stamp to the accepted Session before the next borrow, retaining immutable old
export/diff pins according to host policy. Failure leaves the accepted stamp
unchanged and does not permanently suppress restart flags.

`status()` reports accepted/runtime/source identities, optional pending revision,
transport, finite peak/RMS, delivered events, devices, native output health, stream
starts, and observation drops. When callback state has advanced without its
matching receipt promotion, `runtime_mapping_pending` prevents old device IDs
being attached to new telemetry. Health and callback counters are actual backend
measurements, not synthesized success values.

### Transport and listening

Native controls return `ControlTicket` on admission; wait for `ControlApplied` or
`ControlRejected` for the callback outcome and transport status. Every control is
guarded by its expected revision. A queued control that becomes stale rejects
before mutation. Commands retain FIFO order; audibility and observation changes
do not invent transport discontinuities.
Prepared transport operations and revision preparation return `Busy` while earlier
control outcomes remain undelivered, including internally collected receipts.
Consume the pause outcome before preparing a dependent seek.

- `set_running(false)` pauses without rewinding.
- `restart` seeks to the active audition-loop start or zero, preserving playing
  state. Hosts own play-at-end policy and any combined pause/acknowledged-seek
  action they call “Stop.”
- `seek_ticks` uses integer ticks of the session's timeline, not seconds:
  `muz::compile::PPQ` per quarter beat for compiled source, the file's PPQ for
  imported MIDI. `end_tick` and loop ranges use the same units.
- `set_loop` takes a half-open range with `start < end`. Enabling seeks its start;
  disabling keeps position. Seeking outside an active range clamps to its start.
- `panic` terminates voices; it is not a fabricated rewind.

#### Reading the playhead off the coordinator

`status()` runs on the coordinator, which may be busy for long periods in
preparation, SFZ replay, plugin servicing, or a hung plugin. To draw a playhead,
call `runtime.transport_reader()` once and give the returned `TransportReader`
(`Send + Sync + Clone`) to any thread. `read()` returns the latest
`TransportReading` published by the audio callback: the `TransportSnapshot`
(beat, tick, project frame, sample position, running, bpm, meter, bar start and
index, ended), the loop range, song end, `revision`, transport `generation`,
`callback_count`, and `anchor`, the `Instant` the publishing callback began.

The callback already publishes its telemetry once per block through a seqlock
of atomics. The reader reads only the transport fields of that publication. The
callback takes no locks and allocates nothing for it. It reads the clock once
per block and makes a few dozen relaxed stores. A read makes about twenty
relaxed loads and never allocates. It retries only while a publication is
in progress, so it never returns a torn reading, and it never blocks the
callback or the coordinator. The reader holds the telemetry independently of the
runtime: after `shutdown`, it keeps returning the final publication and
`callback_count` stops advancing.

A reading reflects the end of the publishing callback's block. That is the
next block to render, so it leads audible output by about one period plus device
latency. The callback releases receipts only after the publication that shows
their effect. Once `poll` has returned `ControlApplied` or `RevisionApplied`,
every later `read()` and `status()` shows that operation's loop range, running
state, revision and at least its `generation`, and every later control acts on
it. Hosts can therefore chain controls on outcomes alone: set a loop, wait for
its `ControlApplied`, then `restart` at once. A reading may also show later
playback, such as a loop wrap that advances `generation` again.

To interpolate between callbacks:

1. Keep the last reading. On each frame, read again. If `callback_count`
   changed, replace the kept reading.
2. If `!new.continuous_with(&kept)`, the generation changed: drop any smoothing
   and jump to the new position. Seeks, loop wraps, pause and resume, restarts,
   and revisions that move the position all advance `generation`. A revision
   that preserves the position changes only `revision`. Interpolation stays
   valid across it, but redraw anything derived from the old description.
3. Draw `kept.beat_at(Instant::now(), horizon)`. This adds the elapsed time
   since `anchor` at the published `bpm`. It holds still while paused or
   ended and never passes the song's end. It extrapolates at most `horizon`, so
   a stalled or suspended output holds the playhead instead of letting it run
   away. Use about two callback periods (`status().audio.negotiated_period`
   frames at the sample rate); the extrapolation then spans at most one period's
   tempo change. `elapsed_at` returns the same span in seconds, for project
   frames or sample positions.

Near a loop end, extrapolation can overshoot by up to `horizon` before the
wrap's reading arrives. Clamp to `loop_range` in the host if that matters.
Subtract a constant latency only for audio alignment; it does not affect
smoothness.

SFZ seek/restart/loop preparation replays accepted history off callback with the
accepted asset context and a one-hour frame ceiling; see
[SFZ transport](#sfz-transport) for what is replayed and what it costs. Native
controls never use a bare output seek to bypass required SFZ state. Candidate
preparation rebuilds valid loop checkpoints before publication unless the
revision changes no schedule. A range past a
shortened candidate's end clears at cutover and is reported in its receipt;
replay failure for a valid range rejects the candidate, preserving the accepted
engine and loop.

`set_audibility(expected_revision, exact_ids)` validates physical track IDs and
rejects unknown or duplicate IDs. Empty means silence. The initial policy enables
all tracks; an explicit allowlist survives rebuild by ID intersection, leaving
new tracks silent until the host updates it. The existing 5 ms ramp gates outputs
and pre/post sends after compensation while voices, insert processing, detector
sidechains, and pre-mask analysis continue. The host resolves mute/solo/group
policy; authored Session controls are unchanged. NativeRuntime reapplies the
policy to structural and whole-engine replay replacements **before** replay fills
routed effect history, so muted sources cannot reappear as bus/master tails at
cutover. A changed
policy invalidates older prepared handles rather than publishing stale listening
state. Direct portable `AudioEngine` users retain responsibility for reapplying
their own masks.

### Nondestructive observation

Configure `ObservationSelection` through `set_observation`; it is a reliable,
revision-guarded control. Master capture observes the processed audible mix;
selected tracks are measured after inserts/pan and before routing/listening masks.
Capture never changes output, sidechains, latency compensation, or musical
revision. Do not use the output-replacing `AudioEngine::set_tap` for live analysis.
Hosts displaying a mixer can use `set_graph_observation` to also meter the
first 1,000 non-master buses after their inserts and before output routing,
including buses playing a frozen signal.

Meter frames also carry processing time. `TrackMeter::cpu` is the wall-clock
time the track's chain (instrument and inserts) took over the meter window,
as a fraction of the window's real-time duration: 0.25 means that track alone
used a quarter of real time. Under parallel processing it is the track's own
time on whichever thread ran it, so tracks may sum past one.
`ObservationFrame::callback_load` is the engine's whole render call (tracks,
buses, master) over the same window, measured on the audio thread; outside the
engine the callback's own work is not included. Only metered tracks are timed,
and only while meters publish: each timed chain costs two monotonic clock reads
per transport slice (about 33 ns each on Linux x86_64, so 100 metered tracks
add about 6.5 µs to a 5.3 ms block), and with meters off nothing is timed. Both
are zero in a frame without meters. Offline, `AudioEngine::set_cpu_timing(true)`
times every track; `track_cpu_seconds(index)` and `render_cpu_seconds()` read
the latest render call. Renders do this and report `RenderReport::track_seconds`
(per physical track over the whole render, history included) and `cpu_seconds`.

Select at most 1,000 physical track meters, an optional master meter, and one
detailed `ObservationTarget::Master` or `Track(id)`. Meter frequency is at most
20 Hz; detail frequency at most `native::MAX_ANALYSIS_HZ` (30 Hz). This is a finite
candidate ceiling, not a measured callback/IPC/paint performance guarantee. Zero
disables that stream; hidden/disabled detail stops history work independently of
meters. Unknown IDs, duplicates, excess selections and invalid frequencies reject
rather than truncate. The meter limit does not cap playable tracks.

`drain_observations` nonblockingly fills only empty caller slots without draining
reliable events. A preallocated four-slot pool provides bounded latest-wins
delivery and observable cumulative dropped counts. `ObservationFrame` identifies
revision, transport generation, capture epoch, strictly increasing sequence,
finite positive sample rate and running state. Detail appends the selected
`(L + R) / 2` tap on every running callback into preallocated circular history;
only a fully warmed chronological 2,048-sample window is published. Anti-phase
stereo may cancel under these mono semantics. `sample_count` is actual captured
count (2,048 for detail), not zero-padding. `sample_position` is the exclusive
window end; start is end minus count, in the physical sample-clock domain, not
wall time or score ticks. A meter-only frame has no target/window and count zero.
`meters_present` and optional `meter_sample_position` identify fresh block meters,
whose position is that hardware block's exclusive end. Independent clocks may
coalesce the latest meter/detail payloads with distinct boundary positions;
detail-only delivery must not erase retained meter state.

Revision, selection replacement, seek/restart/loop discontinuity and pause/resume
reset history and advance `capture_epoch`. Restoration warms a new full window;
a hardware block spanning a loop boundary contributes only its post-boundary
suffix. Reorder/removal preserves physical identities, disables removed detail
targets and intersects meters by ID. History accumulates through pool stalls,
publishes at most one newest window per callback and never backfills a queue.
Prepared buffers/mappings and retired objects stay off callback. Compute FFT,
waveform reduction and serialization in the host; an FFT2048 real spectrum may
include 1,025 bins with Nyquist. Native playback PCM need not cross app IPC.

Focused actual-output qualification is reproducible with
`cargo test --test native_observation_qualification -- --ignored --nocapture`.
On Linux 7.2.8 x86_64 / rustc 1.98.1, a muted actual PipeWire default output at
48 kHz, stereo, requested/negotiated 256 frames delivered 89 full windows in
three seconds at the 30 Hz candidate setting. Observed arrival p50/p95 was
32.721/38.006 ms; sample-hop p50/p95 was 1,536/1,792 samples. The full window
spanned 42.667 ms, with two observation drops, zero reported render/stream/
transaction faults and one stream start. Pause/seek/resume invalidation was also
exercised. This is a small synthetic native fixture, not plugin qualification,
end-to-end webview throughput or calibrated capture-to-paint age.

An optimized one-off capture-only harness (meters disabled, 48 kHz / 256 frames,
nine repetitions of 30,000 callbacks) measured median 0.00719 microseconds per
callback for the former sparse 10 Hz copying mechanism, versus 1.11172 for
continuous 10 Hz and 1.12391 for continuous 30 Hz. This comparison includes pool
publication under a stalled consumer, not synthesis, hardware callback CPU,
IPC, FFT or paint; it establishes that preallocation does not make continuous
capture free. Deterministic callback tests separately instrument allocations
**and frees** across capture/replacement, control/revision apply and retirement.
Rates and costs must be requalified for the actual host/environment.

### Source-watching CLI host

`live::LiveSession` owns the same NativeRuntime, not a second native lifecycle.
It retains the accepted Session, `.muz` compilation, dependency watch targets,
debounce, one latest queued source candidate, reload diagnostics, and
watch-to-callback latency summaries. Ordinary candidates receive the current
plugin stamp before preparation. Plugin restarts use accepted source even when
the file on disk has an invalid edit, and do not increment watcher generation.
If admitted controls temporarily block preparation, LiveSession keeps the latest
source candidate queued and retries after collecting their outcomes.
Deferred plugin restart requests belong to that accepted instance set. A revision
commit discards its old tokens; surviving plugins' flags are rediscovered against
the new baseline, so a source commit removing the requester cannot enqueue a
stale-token restart.

Live transport methods return the admitted `ControlTicket`; socket controls return
`{"queued":true,"operation":...,"expected_revision":...}`, not callback success.
Their matching `LiveEvent::ControlApplied` / `ControlRejected` retains operation
identity and authoritative outcome; rejection also updates `LiveEvent::Rejected`
and `last_error`. Ordinary boundary refusal returns typed `busy` / `queue_full`
socket errors without publishing a control. Status remains actual telemetry.
Only a matching revision receipt promotes source/watch targets; an older receipt
does not erase a newer diagnostic. Policy supersession re-prepares the retained
compiled candidate after acknowledged outcomes, rather than reporting user
cancellation or restarting output. Socket shutdown is a coordinator lifecycle
request, not a pinned panic that could be refused during admission pressure.
`LiveSession::shutdown` forwards the native final-outcome report.

## Next steps

The [workflow guide](workflow.md) shows CLI and socket clients using these
facilities. For public Rust definitions, start with [host](../src/host.rs),
[native runtime](../src/native.rs), [inspection](../src/inspect.rs), and
[transactions](../src/audio/transaction.rs).
These APIs expose general host facilities; musical policy belongs in source.

## Cached renders and freezing

Three general pieces cache any part of a render; freezing a track is one
composition of them.

**Taps.** `render::RenderOptions::tap` renders a track (after inserts and pan,
before its fader), a group, a bus or the master. `RenderReport::latency_samples`
is the tap's latency: its file starts that many samples into the graph's output,
so it is aligned to song time, and a track tap's latency is its chain's.

### Output fingerprints

`fingerprint::tap(session, tap, sample_rate)` returns a hex SHA-256 of
everything that determines that tap's audio: each feeding track's performed
events on its own channel, its instrument and insert descriptions and controls,
their automation, sidechain sources (recursively), routes and their levels and
lanes into a bus, the tempo and meter maps, the current versions of every asset
those devices read (through the host's resolver), the sample rate and
`fingerprint::engine_identity()` (package version and source commit). Anything
else (other tracks' notes, the tap's own fader unless it is a `:post` tap,
mute, solo, names) leaves it unchanged. Equal fingerprints mean the same engine
renders the same tap audio; compare them to decide whether a cached render is
still valid. `fingerprint::dependencies(session, taps)` returns the walk itself:
the ids of the tracks and buses whose processing can reach any of the taps.

`fingerprint::tap_spans(session, tap, rate, span_seconds)` splits the same
input by time: `structure` digests everything but the performed events
(devices, controls, automation, routes, assets, tempo map, extent, rate,
engine), and `digests[i]` the events of every chain reaching the tap that start
in span `i`, with `ends[i]` their latest end. Note ids and source order are left
out, so inserting a note leaves later spans alone. `TapSpans::changed(&other)`
returns the song seconds where two versions' events differ, when their
structures match. Plugin restart stamps,
[named keyswitches](#keyswitch-presses) and
[bend ranges](#pitch-bend-in-semitones) are excluded. Hashing walks the reachable
descriptions once (a large resolved SFZ program dominates) and stats each asset.

### Rendering only what a tap reads

`render::keep_tap_dependencies(&mut session, taps)` reduces a description to
those dependencies: other tracks are removed, the inserts of buses no tap reads
are cleared, and the lanes of removed devices are dropped. Every one of `taps`
then renders exactly as from the whole graph, sample for sample, without the
rest of the mix. (The render timeline compiles from every track's tempo map and
extent, so tracks are kept when those differ; a compiled song's never do.)
`muz stems TRACK` and freezing use it.

`render::render_taps_parallel(&session, paths, options)` builds on it: it
splits taps into `render::independent_tap_groups` (taps whose dependencies share
a track or bus render together), prunes each group to its own dependencies and
renders the groups as concurrent passes, up to `options.threads` at once, each
with an equal share of the threads for its own tracks. Each file equals the
whole-graph single-tap render for any budget. The passes run inside the caller's
host context (a CLI one when there is none), so they share decoded recordings
and prepared programs; `audio_file::load_shared_many` decodes a set of
recordings on worker threads, which SFZ preparation uses for its samples.

`render::render_tap_blocks(session, options, sink)` renders one tap as a stream,
handing each block of output (aligned like a file render of the same options)
to `sink`, which can stop the render: for comparing a render with earlier output
as it runs.

### Song-time streams

A sampler zone with `song_seconds` (`at:` in source) plays a recording in song
time; see [recordings in song time](instruments.md#recordings-in-song-time). Its
position comes from the transport's project frame each block, so it stays
sample-aligned through seeks, loops, restarts and pause/resume with no state.
The sampler's `latency_samples` parameter delays such zones and reports the
latency, so the engine compensates as for the chain the stream replaces. Being
an ordinary device, it goes through `prepare_revision` like any instrument
change, and tracks that do not change keep their processors.

### Freezing

`freeze::stream_track(session, track, audio, start_seconds, latency_samples)`
rewrites a description so `track` plays `audio` as its chain output: the
instrument becomes a sampler with one song-time zone, inserts and their lanes
are removed, and the track's performed events are dropped (its tempo map and
extent stay). Its output route, sends, groups and sidechain readers are
unchanged. Played from a tap render of the track with that render's
`latency_samples`, the mix is identical to the unfrozen one: the stream sits
exactly where the instrument and inserts did, before the fader and sends, with
the same compensation. Without the latency, downstream fader automation would
land that many samples later than in the original (live route automation is
evaluated at processing time, not compensated).

To freeze a track by hand: render with `tap: [TRACK]` from the song start
(no `start`, `section` or `history`) at the playback rate; record the
report's `latency_samples` and `fingerprint::tap(&session, TRACK, rate)`; then,
for each later description whose fingerprint still matches, call
`stream_track(&mut candidate, TRACK, wav, 0.0, latency)` before
`prepare_revision` or rendering. The `freeze` module does this bookkeeping for a
cache directory: `freeze` renders tracks with `render_taps_parallel`, each pass
processing only its tracks' dependencies (the tracks and their sidechain
sources, with other fresh freezes streamed), and writes their audio, a `FreezeRecord` (`TRACK.json`) and measured costs (`cpu.json`);
`status` reports `FreezeState::{Fresh, Stale, Missing}`, costs and each audio
path; `apply` streams the fresh ones and returns every state, and
`stale_warnings` words the stale ones. A stale freeze is simply not applied, so
the track renders live. `freeze_with` takes a `control::RenderProgress` (every
pass adds its frames; `cancel` stops them) and returns `Frozen`: the report and
a `TrackFrozen` per track saying which song seconds were rendered and whether
the rest was kept.

For mixer graphs, `freeze::freeze_graph_with` accepts physical tracks, buses,
the master and processor keys returned by `freeze::device_target`. A processor
key captures its strip through that processor. `freeze::apply_graph` replaces
the captured prefix with a song-time stream and keeps the remaining processors
and routes. When a bus is streamed, it walks back from the master through
remaining inputs and sidechains and releases any original processors with no
live consumer. A bus print carries the latency of the full prefix, including
its inputs. It plays as one mix for audition. Mute or solo the bus to control
the print, or remove the freeze to audition its inputs separately.
`freeze::measure_graph` profiles the authored graph once and records each strip
and top-level processor's cost. `NativeRuntime::set_route_audibility` auditions
an exact set of outputs and sends. Calling `set_audibility` returns to track
audition.

**Partial refreeze.** Each `FreezeRecord` carries the track's `TapSpans`
(2-second spans). When a freeze is stale but its structure is unchanged (only
performed events changed), `freeze` renders the track from a pre-roll 20 s
(then 60 s) before the first changed span with bounded history, and checks the
pre-roll against the old file: at least a second of identical output, some of it
audible, before the change. It splices only then, and keeps rendering past the
last changed span (and its notes' ends) until the output matches the old file
again for a second, or to the end. A pre-roll that would reach the song start
renders from there with full history, which needs no check. A full render of the
track runs at the same time and is cancelled when the partial one succeeds, so
an attempt that never converges costs no time. Instruments whose state follows
the whole song never converge from a mid-song pre-roll: a voice patch's noise
stream, an SFZ program's round robins, a plugin whose output varies between
renders. Those re-render in full (or from the song start, when the change is
within the first minute). `remove(dir, track)` deletes a freeze (audio in either
format and record) so the track plays live; `compact(dir)` re-encodes WAV
freezes as WavPack without rendering.

Freeze audio is `TRACK.wv` (lossless float WavPack) with the default `wavpack`
feature and `TRACK.wav` without it; `freeze::audio_path(dir, track)` finds
whichever exists, and both are read. Two general pieces carry this:
`audio_file::AudioWriter` encodes by the destination's extension (`.wv`
WavPack, otherwise WAV), which every render output and `render::render_taps_to`
(one pass, a path per tap) go through; and the asset decoder reads `.wv` like
WAV and FLAC, so any sampler zone can play one. `audio_file::reencode(from, to)`
converts losslessly between the two, writing `to` only after it decodes to the
same samples bit for bit. The feature links the system libwavpack (5.0 or
later, found with pkg-config) through a small FFI (`src/wavpack.rs`).
`live::LiveSession::start_transformed` takes a `SessionTransform` run on every
compiled description before preparation, which is how `muz serve` applies
freezes; any host rewrite fits there.

A frozen stream is limited by the sampler's 64 Mi-frame budget (about 23 minutes
of stereo at 48 kHz) and the 512 MiB asset read limit, which applies to the
file as stored (about 22 minutes of float32 WAV; a WavPack freeze usually holds
far longer).

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
Instruments with an identical program share its preparation through
`HostContext::sfz_programs` and are charged for the program once; see
[one program on several tracks](sfz.md#one-program-on-several-tracks). Reuse one
context across preparations to keep sharing between a running engine and its
replacement. The same context's `sfz_sources` keeps the normalized program
itself across compilations (see [recompiling after an edit](#recompiling-after-an-edit)).
Large roots can exceed the default 4096-unit graph budget. Set a deliberate host
`graph_units` limit or `MUZ_GRAPH_BUDGET` (live maximum 65536, offline 1048576) after inspecting the
reported contributors; loading one root does not count as a single instrument
allocation. `sample_budget_frames` independently bounds shared decoded stereo
frames: the default is 64 Mi frames (512 MiB), and the maximum is 1024 Mi frames
(8 GiB). The original Virtuosity full kit needs about 4.9 GiB. Raise it explicitly
for a verified library footprint rather than relying
on a larger implicit default. Processor SFZ statistics expose decoded frames,
voice occupancy and steals/drops for host inspection.

### SFZ transport

An SFZ instrument's state at a position depends on everything it has played:
round robins, random layers, keyswitches, controllers, held and releasing voices.
A live seek, restart or loop therefore replays the SFZ track instruments and their
event schedules from zero to the target off the audio thread. At a callback
boundary the running engine seeks as it does without SFZ and adopts the replayed
instruments, their schedule positions and any loop checkpoint. The replaced
instruments are retired on the coordinator thread. Every other device seeks as
in a session without SFZ: notes overlapping the target restart with their elapsed
offset, prior controllers are restored, and effects and plugins keep no
reconstructed history.

NativeRuntime keeps an SFZ transport rig per accepted revision. Its first
transport action prepares each SFZ instrument once, reusing the prepared program
and decoded recordings of the running engine through `HostContext::sfz_programs`
and `decoded_assets`. Start the runtime and call transport with the same
context to share them. Later actions duplicate that pristine instrument instead
of preparing it again. Replay renders only SFZ voices. Snapshots of the replay,
at most 32, spread at least a second apart across the timeline, let a seek start
from the nearest earlier one. A low-priority background thread records them
after the rig is built. A preparation interrupts it and resumes from the latest
snapshot. The state at the last loop's start is kept too, so restarts and seeks
inside that loop replay only from its start. Replay from a snapshot renders the
same blocks as replay from zero, so the result is identical. A new accepted
revision discards the rig.

On Glass Hearts (12,427 regions, 2,278 recordings on one SFZ guitar, plus NAM
amps and VST3 instruments), a release build measured these times on a loaded
machine. The rig's first transport action took about 0.4 s. Seeks after the
background warm-up of about 1.6 s took 1–55 ms to prepare, and the callback
applied them within a period. A seek deep into the timeline before warm-up
finished took up to about 0.7 s. Whole-engine replay took 8–48 s.

Sessions with an SFZ instrument inside a rack or as an insert still prepare a
separate engine by replaying the whole graph from zero, then swap it. That
includes effect history, at the cost of preparing every device again.

Audition loops capture SFZ voices, DSP, note identities, controllers,
keyswitches, sequences and random state at the prepared start; each wrap
restores preallocated state and the event schedule. Effect tails retain their
existing loop behavior. Authored controller history repeats with the loop;
controls changed after the checkpoint return to the captured boundary value on
wrap.

Replay is bounded to one hour of project frames per live transport request and
supports cancellation. While a source revision is pending, transport preparation
reports a retryable diagnostic. A revision that replaces no schedule (controls,
route levels, automation points, presentation) changes no SFZ history, so it
keeps the running loop checkpoint and replays nothing. Other candidates rebuild
active SFZ loop checkpoints before cutover by replaying the whole engine; an
out-of-range loop is cleared at that cutover, while a valid-range replay failure
rejects the candidate.
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
