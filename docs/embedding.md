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
`restore_checked` inside the receiving asset context. Version 1 requires exactly
one performed association per MIDI track, keyed by both track and source IDs;
duplicates, missing/unknown associations, invalid event buffers and stale assets
are rejected. Track order may change. Snapshots reference assets, not bundle
their bytes, and do not promise future bit-identical plugin rendering.

`snapshot::SessionSummary::new(session, revision)` provides a bounded status view
(at most 1,000 tracks with short labels). `inspect::performance_page` pages
performed data; `Compiled::explain_notes` and `provenance::diff` page explanations
and key-based changes. These pages cap rows at 1,000 and serialized row payloads
at 1 MiB; a single oversized row errors. Full graph inspection rejects output
above 1 MiB; filter by track or request `summary`. Older explicit full performance,
patch and automation views remain available for callers controlling their input.

Enable `HostContext::provenance` to retain the definition and last
pattern-returning call for each score note. Explanations explicitly report that
occurrence spans are unavailable; this is not a universal transform history.
Origins are excluded from equality and playable snapshots. Identity comes from
note keys, not source offsets: named paths can survive unrelated insertions,
whereas positional components can shift. Renames appear as remove/add and
duplicate keys are reported as ambiguous, never guessed.

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

Compatible source tempo edits preserve held-note release/expression obligations
at their original remaining wall-clock times. Explicit seek/restart/mode changes
still create discontinuities. A shortened piece does not immediately cancel held
voices. No incremental compiler or cross-process capability cache is implied.

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
