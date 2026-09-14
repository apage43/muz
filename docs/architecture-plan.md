# Architecture and language improvement plan

Status: investigation and implementation handoff; production implementation has not begun.

Prepared 2026-09-13 (America/Los_Angeles) against `main`,
`ea95e537630d2482510cbe46b78aa4c6adc7bb0b`. The working tree was clean on entry.
The supplied brief reviewed `ded4eacbb7c8cd9b868458ceb16654956db20f70`.
Subsequent commits fix prelude-value access/document pattern coordinates and format
Rust; they do not resolve the principal leads below. Line references in this
document identify the inspected HEAD, not the older review.

This is a standalone handoff in `docs/`; no established planning directory was
found. `AGENTS.md` and `docs/composer-friction.md` govern the work. Confirmed
unresolved defects are recorded in that existing friction log, with this task as
origin. This document owns the design and sequencing, not a second friction log.
No pieces, dependencies, external issues, or host implementations were changed.

## 1. Verified baseline

### Environment and scope

Rust `1.98.0 (88d9e12ae 2026-08-18)`, Cargo `1.98.0 (797e8a9bc 2026-08-05)`,
Linux x86-64; existing debug artifacts were available. Installed targets include
`wasm32-unknown-unknown`, `x86_64-unknown-linux-gnu`, and Windows GNU. The default
`desktop` feature enables CLI, PipeWire, plugins, workers, and native control;
`--no-default-features` preserves the library engine. No dependency setup failed.

Read the language/evaluator, musical values, compilation, model/reconciliation,
transaction/engine/scheduling paths, worker, source/asset interfaces, arrangement
library, relevant diagnostics and tests. Read the actual browser consumer at
`/home/aaron/proj/muzpad/wasm/src/lib.rs`, its Cargo manifest and audio handoff,
at clean HEAD `16d5901cc72fe6d53a36f94ac02f1e20ab1a30b8`. Its local instructions
were inspected; no files there were edited. This was not a full plugin, DSP,
security, or composition audit.

### Architecture map: reuse these boundaries

| Stage | Current representations and ownership | Evidence at inspected HEAD |
| --- | --- | --- |
| Load | `SourceLoader` resolves module identities and supplies text/contrib names; each evaluator has fresh module/cache state. Filesystem is one implementation. | `src/lang/loader.rs:7`, `src/lang/eval.rs:392`, `src/compile.rs:150`; `tests/editor_host.rs:34` |
| Parse/evaluate | AST nodes have offsets. Immutable `Value` arrays/records/patterns share storage; numbers distinguish exact rationals, inexact controls, and dimensions. Records retain `Origin`; patterns do not retain source origins. | `src/lang/parser.rs`, `src/lang/eval.rs:16`, `:218`, `:607` |
| Musical source | `music::Pattern` contains exact score positions, notes, controls, raw messages, keys/tags/JSON annotations. `std/arrange` implements passages/occurrences/edits/grouping/late gestures in source. | `src/music.rs:24`, `:75`; `std/arrange.muz:1`; `examples/revision-workflow.muz` |
| Lower/validate | `.muz` records become score plus performed `ImportedMidi`, one-shot transport and graph `Session`. Validation is distributed: pattern checks, field checks, graph checks, asset stamps, playing policy. Public mutable `Session` is not proof of validation. | `src/compile.rs:181`, `:331`, `:816`, `:703`, `:1411` |
| Alternate input | JSON5 project input has separate raw DTOs/validation and MIDI import. It also converges on `model::Session`. | `src/source.rs:50`, `:72`, `:105`; `src/midi.rs:129` |
| Patch description | Nested source graphs become an ordered JSON node description. Sharing uses source `Arc` identity, then deterministic traversal IDs. Equal independent stateful nodes stay separate. | `src/patch_source.rs:94`, `:104`, `:250` |
| Preparation | `AudioEngine::new` creates typed processor operations, decoded samples, timeline/schedules, routing/sidechains, compensation, and automation. Source language/JSON is not interpreted by the audio loop. | `src/audio/engine.rs:120`, `:681`; `src/audio/device/patch.rs:16`, `:28`, `:216` |
| Reconcile | IDs, parent/order, device structural identity and field deltas produce a bounded `ReconcilePlan`. MIDI source equality includes performed arrays. Patch structure already excludes declared controls/default values and selected display fields. | `src/reconcile.rs:308`, `:336`; `src/model.rs:142`, `:245`, `:275` |
| Prepare revision | Value path supports parameters, legacy patterns, transport; top-level preparation constructs trial processors for setters. Other changes construct a full candidate engine, then retention tables. | `src/audio/transaction.rs:52`, `:232`, `:353`, `:468`, `:582` |
| Apply/retire | Callback preflights slots, applies prepared operations or swaps engines/processors/schedulers. Coordinator accepts the model on receipt and drops retired data. Desktop queue permits one in-flight candidate and coalesces subsequent candidates. | `src/audio/engine.rs:237`, `:318`; `src/audio/pipewire.rs:538`, `:764`; `src/live.rs:403`, `:469` |
| Schedule/transport | Arrangement schedules precompute frame positions; bounded active notes retain original release/expression state on ordinary replacement. Seek/discontinuity flushes. Tempo/source transport adoption currently conflates some source edits with discontinuity. | `src/audio/transport.rs:106`, `:424`, `:694`, `:775`, `:784` |
| Inspect/transfer | `Compiled::locations` exposes declarations. Graph JSON omits performed MIDI arrays; explicit performance view includes them. Worker `Input` restores parallel event arrays. Browser host has a separate ID-keyed wrapper. | `src/compile.rs:79`, `:743`; `src/model.rs:167`; `src/inspect.rs:7`; `src/worker.rs:16`; `docs/embedding.md` |

Prepared runtime state, validated musical meaning, source values, and serialized
views are different things today, although several public types blur these
boundaries. Improve the boundaries in this crate; do not replace the engine,
language, arrangement library, source loader, or fixed operation schedule.

### Evidence ledger

“Confirmed” means the stated behavior exists, not that every consequence claimed
by the brief was measured. Proposals appear in later sections.

| Lead | Classification | Current evidence and implication |
| --- | --- | --- |
| A1: clip marker collision/panic | **Confirmed** | `annotate` writes `v.json()` at `src/lang/builtins.rs:1082`; `clip` creates three annotation fields at `:778`; ordinary transforms clone them. `src/compile.rs:332` selects numeric `clock_start`, then indexes/unwraps companions before `Pattern::validate`. Three malformed cases panic in P1. Non-numeric start and orphan duration are currently ordinary data; negative span can be accepted. |
| A2: route `pre` omitted | **Confirmed** | Route branch at `src/reconcile.rs:443` compares only `to`/`gain_db` on `model::Route` (`src/model.rs:427`). `needs_replacement` at `:308` has no compensating route-content check. `same_routes` in transaction preparation compares full routes, but it runs only after a structural plan is selected. P1 produces an empty plan and divergent live/fresh output. |
| A3: collection equality | **Confirmed; related grouping already addressed** | `src/lang/eval.rs:926` normalizes beat/bar scalar operands; `:938` falls back to JSON for other values. `contains` at `src/lang/builtins.rs:1485` does likewise. P1 confirms nested unit loss, opposite beat/bar mismatch, and unrelated functions comparing equal. `data_key` at `:403` already preserves types/dimensions for grouping/random keys; it must not be replaced with inspection JSON. |
| A4: numeric boundaries | **Partially addressed, remaining defects confirmed** | Exact arithmetic and musical-duration conversion are present (`eval.rs:28`, `:154`, `:260`); `amount_ms`, `clock_seconds`, patch-field quantities, several MIDI integer checks and range helpers exist. Generic `.number()` strips units (`:254`), meter casts precede checks (`compile.rs:227`), and list indexing casts at `eval.rs:718`. P2 confirms truncation, saturation and incompatible dimensions. |
| A5: expansion budgets | **Confirmed** | `repeat` at `builtins.rs:845` preflights only notes; `fit` at `:915` repeats the pattern. `seq`/`stack` and `overlay` aggregate before checks. `Pattern::overlay` at `music.rs:107` extends all streams; `validate_local` at `:125` checks counts afterward. P1 materializes 210,000 controls or raw events from small inputs, then explicit validation rejects them. Saturating note-count multiplication avoids overflow for that check, not allocation of other streams. |
| B: preparation classification | **Confirmed and selectively measured** | `is_structural_operation` excludes exactly three variants. `.muz` music becomes MIDI, so note/tempo edits use `ReplaceTrackSource`. Rename/title/tail/automation use structural preparation. Trial processor creation occurs for value setters. P1 counts construction and preparation time; it does not establish production latency or plugin costs. |
| B: retention contract | **Partially addressed** | `ArrangementScheduler::adopt` retains active release/expression state and runtime note IDs. Existing held-note and patch-control tests pass. Newly discovered tempo/discontinuity interaction below violates retention for a compatible tempo edit. Stale revision enforcement at the raw engine API is not demonstrated by existing tests. |
| C: session transfer | **Confirmed, worker workaround works** | Ordinary serialization intentionally skips `imported`. Worker restores `Vec<ImportedMidi>` by `zip` (`worker.rs:25`). P1's real child worker produces sample-identical native output after restoration. This is not a dropped-note report against the normal worker. Missing/extra associations are not validated by the wrapper itself. Browser `Snapshot` requires IDs to exist but does not reject leftover event IDs; map decoding cannot diagnose duplicate JSON keys after collapse. |
| D: loose descriptions/dependency inversion | **Confirmed; typed runtime and control exclusions already exist** | `Device` has independently optional patch/rack/sample/plugin payloads (`model.rs:205`), `Session` references `compile::Extras` (`:32`), music/expression import frontend diagnostics (`music.rs:2`, `expression.rs:2`). Source/runtime repeat JSON field/node interpretation. `Device::patch_structure` already strips controls. Runtime `Op` is typed. |
| E: provenance | **Partially addressed** | Records/declarations retain locations; `Note.key`, tags and annotations survive into `MidiNote` (`compile.rs:899`). `Pattern::shifted` prefixes keys, named arrangement occurrences supply placement keys. `origin_of` only stamps records (`eval.rs:607`); no definition/transform source chain reaches generated notes. Existing arrangement and editor tests cover reuse/navigation ingredients, not event-level navigation. |
| F: host assets/configuration | **Confirmed, source abstraction already delivered** | MIDI/clip builtins canonicalize/read files (`builtins.rs:663`, `:724`); sample zones, sample decoders, patch sample caches, asset stamps and plugin state use native paths. `graph_budget` uses process `OnceLock` (`model.rs:562`); `INTERRUPTED` is global (`lib.rs:37`, `eval.rs:545`). Browser consumer skips asset folders (`muzpad/src/folder.ts:40`) and uses host-owned source files. Portable compilation succeeds, not proof of portable asset support. |
| G1: bars versus measures | **Intentionally supported behavior; naming opportunity** | Literal bars are four quarter beats; `std/arrange.bars(count,meter)` already calculates measures explicitly. `tests/arrangement.rs:38` passes. No ambient meter reinterpretation is warranted. |
| G2: rest/spread | **Confirmed limitation; defer feature** | Parser/function binder support fixed named/default parameters (`parser.rs:389`, `eval.rs:832`). Variadic `merge` is already present and tested (`builtins.rs:1517`, `tests/source_catalogs.rs:210`). Arrangement mostly uses binary merge and list-taking map/fold/overlay; no inspected call site requires general forwarding. |

### New adjacent finding: tempo changes discard a held note

`RuntimeTransport::adopt_position_from` (`transport.rs:424`) increments
discontinuity when its timeline changes. The staged scheduler adopts active
notes, but `schedule` (`:811`) then clears them and emits `Flush`. Because it is
also a reload, the seek catch-up branch (`:867`) does not restart earlier notes.
P3 changes only tempo during a long constant note: output falls from `0.14142136`
to `0`, with one delivered note-on and zero note-offs. This is a distinct defect
discovered while checking B, not evidence that ordinary note replacement fails.

Another adjacent **static concern, not a reproduced defect**: value transport
application calls `RuntimeTransport::update_source` (`:409`), which builds a
`Vec`-backed loop timeline. Existing allocation tests do not exercise that loop
transport edit. Before broadening the value path, move preparation outside apply
and add the missing allocation test. Similarly, fallible setters are applied in
sequence after slot preflight; trial construction alone is not a proof that
dynamic plugins cannot fail or change capabilities at application time.

### Commands and observed outcomes

The following existing tests were actually run, with default features:

| Command | Result |
| --- | --- |
| `cargo test --test numbers --test editor_host --test lifecycle --test patch_controls --test event_transforms --test arrangement --test graph_resources` | 41 passed: 8 + 4 + 6 + 2 + 8 + 10 + 3; zero failures/ignored |
| `cargo test --test sidechain_tap --test source_catalogs --test patch_design` | 34 passed: 1 + 17 + 16; zero failures/ignored |
| `cargo test --lib audio::pipewire::tests::callback_resources_return_to_the_preparation_thread` | 1 passed, 30 filtered out |
| `cargo test --lib -- --list` | Listed 31 unit tests; listing is not execution |
| `cargo check --lib --no-default-features --target wasm32-unknown-unknown` | Passed; three dead-code warnings for desktop-only telemetry/fade methods |

Thus 76 existing tests passed. Especially useful contracts are
`removing_a_sounding_note_keeps_its_original_release_obligation`,
`native_callback_does_not_allocate_or_retire_objects`,
`nested_graphs_share_by_binding_not_by_equal_contents_and_check_units`,
`clock_clip_placement_keeps_local_seconds_and_duration`, and the editor-host tests.
The sidechain test verifies fresh-render tap behavior; it did not cover a live
`pre`-only edit before this investigation.

Disposable Rust integration probes used only public APIs, without production
instrumentation. They were run serially and then removed:

| Probe command | Exact outcome |
| --- | --- |
| P1: `cargo test --test __architecture_probe -- --nocapture --test-threads=1` (one test at that point) | Passed as an investigation test. Caught expected panics at `compile.rs:333/334`; observed behaviors listed below. Passing means observations/assertions completed, not that the defects are acceptable. |
| P2: `cargo test --test __architecture_probe investigate_boundary_followups -- --nocapture --test-threads=1` | One passed, one filtered. Corrected fixture omissions; checked numeric boundaries, sharing and rejected parameter preparation. |
| P3: `cargo test --test __architecture_probe investigate_tempo_hold -- --nocapture --test-threads=1` | One passed, two filtered. Asserted the existing tempo-edit loss of a held constant note. |

Reproduction details sufficient to recreate these probes:

- P1 uses `Evaluator::source(...).__result` and `compile::lower(...,
  Path::new("architecture-probe.muz"), vec![])`. In
  `song({tracks:[track("x",note(60,1b).annotate("all",PAYLOAD),synth("init"))]})`,
  `{clock_start:0}`, `{clock_start:0,clock_duration:1}`, and
  `{clock_start:0,clock_duration:"bad",clock_span:1}` panic under `catch_unwind`.
  `{clock_start:"label"}`, `{clock_duration:1}`, and
  `{clock_start:0,clock_duration:1,clock_span:-2}` are accepted.
- Equality results: `1b==1s` false; `[1b]==[1s]` true;
  `contains([1b],1s)` true; `1bar==4b` true; `[1bar]==[4b]` false;
  `[1ms]==[0.001s]` true; `{a:[1b]}=={a:[1s]}` true;
  `(fn(x)=>x)==(fn(x)=>x+1)` true; `[1]==[cos(0)]` true.
- P1's first meter/tempo/tail fixtures omitted tracks and correctly failed
  `song needs at least one track`; those failures say nothing about numeric
  validation. P2 added `tracks:[track("x",note(60),synth("init"))]`:
  `[3.5,4]` becomes meter `[3,4]`; `[256,4]` becomes `[255,4]` (saturation,
  not wrapping); `[3s,4Hz]` becomes `[3,4]`; `tempo:120Hz` and `tail:1b`
  are accepted. Separately, `note(60Hz,1b)`, `note(60).repeat(2s)` and
  `cc(64Hz,1)` are accepted; `[1,2][0.5]` returns `1`.
- `control(64,0).repeat(21).repeat(10000)` and
  `cc(64,0).repeat(21).repeat(10000)` evaluate to 210,000 events each;
  subsequent `Pattern::validate` rejects both. No huge allocation was attempted.
- P1 routing uses a one-beat constant voice patch: one `param` named `level`,
  value `0.2`, range `0..1`, output `level`, `gain_db:0`; note velocity/gate 1;
  track output gain -12 dB, send gain 0 to an empty bus. Change only send `pre`
  false to true. At 48 kHz, 256-frame stereo blocks, sample 500 of the fourth
  block is `0.07104688` before and after application, versus `0.1769448`
  in a fresh candidate engine. The reconcile operation list is empty.
- P2's shared `[n,n]` noise input creates one noise operation; independent
  `[{op:"noise"},{op:"noise"}]` creates two. Invalid synth sustain `2`
  is rejected by top-level transaction preparation; the accepted processor token
  remains unchanged and the old engine continues rendering.
- P1 serializes the real worker `Input` shape with a session plus performed
  events, invokes `CARGO_BIN_EXE_muz render-worker INPUT REPORT PROGRESS`, and
  compares its native float WAV against `render_with` of the original session.
  All 48,000 float samples match. Ordinary `Session` JSON round-trip alone has
  zero imported notes and tempos. Temporary WAV/JSON files lived in `TempDir`.
- P3 uses the same constant patch with a four-beat note, no sends, tail 0.
  After four blocks, change only its imported tempo from 500,000 to 600,000
  microseconds/quarter; prepare/apply normally and render four more blocks.
  The held-note output becomes zero as described above.

Preparation measurements used P1's single native device, same graph, 48 kHz,
256-frame offline configuration. Seven repetitions per edit, debug build,
timer covering `PreparedTransaction::prepare` only (not reconciliation,
compilation, sentinel creation, application or destruction). Existing
`create_processor` instance tokens bracket preparation: token difference minus
one counts intervening constructions. No concurrent test threads or processors.
All seven construction counts agreed per row.

| Edit | Structural? | Processor constructions | Median microseconds | Range microseconds |
| --- | --- | --- | --- | --- |
| No-op | No | 0 | 69 | 68–149 |
| Track display name | Yes | 1 | 269 | 265–338 |
| Title | Yes | 1 | 270 | 261–313 |
| Performed note key | Yes | 1 | 265 | 261–335 |
| Patch control value | No | 1 | 115 | 113–152 |
| Tail | Yes | 1 | 287 | 263–314 |
| Automation lane | Yes | 1 | 265 | 263–339 |
| Imported tempo | Yes | 1 | 263 | 259–324 |

These demonstrate unnecessary construction, not production speedup estimates.
No plugins were instantiated or auditioned, no live sound device was exercised,
no browser runtime/build was run, and no end-to-end watcher/stale-revision test
was run. Loop transport allocation and arbitrary hostile snapshot validation
remain untested concerns. Do not claim those invariants from these results.

## 2. Design decisions

### A. Correctness boundaries

**Clip timing:** ship a checked conversion first. Under the current numeric
`clock_start` trigger, fetch companions explicitly; require finite nonnegative
start, positive duration, positive span at least duration, and finite sums and
representable converted score time. Reject missing, null, text, collection,
nonfinite host-provided numbers and invalid ranges with track/note key and the
track origin. Keep other metadata keys, including orphan `clock_duration` or
text `clock_start`, untouched in this initial compatibility patch. It closes the
panic without claiming a full namespace solution.

Next introduce `ClockPlacement { offset_seconds, duration_seconds,
span_seconds }` on the musical note's internal timing description, with validated
construction and `Option` distinguishing ordinary score notes. `clip` alone
creates it; cloning/placement preserves the seconds-local payload, and lowering
consumes it once using the final tempo map. After this transition, bare
`clock_*` entries are ordinary metadata and no longer drive timing. Do not ban
keys by prefix. These fields are not documented authoring interfaces; documented
`clip` source stays unchanged. An explicitly versioned legacy pattern decoder,
if retained for host round-trips, may adapt the complete old timing triplet under
the old format, never by guessing in new `annotate` values. Document this narrow
change for direct Rust/serialized-pattern consumers.

Do not silently define new clip stretching/trimming semantics. In the typed
timing slice, allow existing placement, sequence/repeat and metadata/performance
edits; reject `slice`, `fit`, `stretch`, `reverse`, or note patches changing
score duration on a clock clip until they have an explicit clock-domain rule.
Those operations currently manipulate the placeholder score duration and cannot
reliably express a clock trim. Document the rejection and use `clip` trim options
instead. Ordinary score patterns are unaffected. Validated engine-recognized
annotations (`channel`, `sample_zone`, expression, fingering) remain available
through their current interfaces; validate them before lowering/JSON erasure.

**Routes:** use `old != new` in the existing stable-ID route branch. Full derived
semantic equality is sufficient for the current `Route`; there is no display
payload that needs exclusion. Reuse it in fade/route comparisons. Test every
field, not only `pre`; ID changes remain remove/add. Route topology/latency still
uses the full preparation path initially. No need for a new routing abstraction.

**Equality:** one fallible recursive `Value::semantic_eq(&self, &Value)` used by
`==`, `!=` and `contains`. Required rules:

| Values | Semantics |
| --- | --- |
| Numbers | Normalize bars to beats at four beats, as existing scalar operators do. Literal ms/s and kHz/Hz already normalize on parse. Scalar versus dimension remains distinct. No tempo conversion between seconds/beats. Checked normalization overflow is a diagnostic. |
| Exact/inexact | Preserve `Number::compare`: exact/exact compares rationals; a mixed or inexact pair compares finite f64 magnitudes. No epsilon, rounding or conversion of exact score values to inspection JSON. Document mixed precision limits; this relation is not a safe hash-key equivalence near f64 precision limits. |
| Arrays | Equal lengths and recursively equal ordered elements. |
| Records | Equal key sets and recursively equal values, independent of insertion order and `Origin`. |
| Null, bool, string | Same variant and same value; unlike variants are false. |
| Patterns | Compare typed musical fields, including keys/tags/data and clock timing; ignore provenance/location sidecars. Current metadata is already JSON data, so comparing that payload as JSON is intentional, not another erasure of `Value` units. |
| Functions | Same `Rc` function identity only; copied binding equals itself, separately created closures do not. No function equals the string `<function>`. |
| Builtins/bound methods | Same canonical builtin name and both unbound, or recursively equal bound receivers. Builtin and user function are distinct. |
| Invalid numeric value | Propagate its diagnostic instead of treating it as a function/string. |

`contains` searches using precisely this comparison and propagates encountered
errors. Keep `data_key` for `group_by` and `keyed_noise`: it already provides a
typed deterministic key encoding and exactifies inexact keys. Do not change
seeded variation as collateral damage. Document that key grouping is its own
canonical-key equivalence, not a promise to reproduce mixed-float `==` in every
precision edge case. `sort_by` retains quantity ordering. Other JSON consumers
(inspection, string formatting, annotations, patch/worker encoding) are not
substitutes for language equality and should remain independent.

**Conversions:** add small field-aware helpers, not a type checker. Helpers
accept a declared dimension, scalar-default policy, range and number shape;
integrality/bounds are checked on exact values before casts, and on finite
inexact values before conversion. Errors carry field/argument name; frontend
attaches origin. Avoid a single global reinterpretation of `.number()`.

| Boundary family | Preserve convenience | Reject/change | Owners to migrate |
| --- | --- | --- | --- |
| Score positions/durations | Scalar means beats; Beat exact; Bar = four beats; negative local pickups where already allowed | Seconds/Hz/dB/BPM; negative final positions; checked rational overflow | `Value::beats`, pattern operations, compiler section/tempo positions |
| Clock seconds | Scalar means seconds; Seconds accepts ms/s aliases | Beats/bars and unrelated units; nonfinite/invalid duration | clip, song tail, sample offset, patch seconds fields, render recipes |
| Millisecond fields | Scalar means milliseconds; Seconds converted ×1000 | Other units; range violations after conversion | `amount_ms`, offsets, envelope/effect `*_ms` descriptors |
| Frequency | Scalar means Hz; Hz/kHz explicit values normalize | BPM/score/clock/dB | device cutoff/rate controls and patch signal fields |
| Decibels | Scalar means dB; explicit dB accepted | Other dimensions, nonfinite out-of-range gains | track/send/device/zone gains |
| Tempo | Scalar means BPM; BPM accepted; current 20..400 source range | Hz and other dimensions | `tempo_map`, tempo points, source tempo helpers |
| MIDI/pitch | Musical pitch can remain fractional 0..127 for note expression; normalized velocity/release remains scalar 0..1, with existing MIDI quantization | Units on MIDI numbers; fractional channels, bytes, program numbers, discrete sample key-range endpoints | note/refine, channel/raw/CC, sample zones, SMF export |
| Meter/count/index | Existing valid integers and negative list indexing; meter numerator 1..255 and denominator powers of two up to 128 | Fractional/saturated casts, dimensional counts/indices; bounds before cast | compiler meter; evaluator index; repeat/fit/import-track/tonal counts |
| Ratios/shape/phase | Scalar and `%` (already scalar); existing explicit ranges | Dimensional stretch/gate/pan/phase/solver weights | builtins, performance/tonal options, patch inputs |
| Parameters/automation | Field descriptors supply default units; plugin normalized controls use scalar values | Unknown/incompatible dimensions before JSON/f32 erasure | compile device/lane lowering, source catalogs, expression boundary |

Expression phase remains scalar 0..1; expression values keep their documented
normalized units (no inferred dB or Hz conversion). Generic patch `param`
controls with no dimension descriptor accept scalars only; a descriptor can
explicitly assign a dimension later. Existing patch signal connections are not
a static dimensional type system. Legacy JSON DTO numbers have already lost
source dimensions: enforce shape/ranges there, without inventing units.

Use the original `Number::Exact` scalar in stretch/time multiplication;
`builtins.rs:902` currently passes through f64/rational unnecessarily. Compute
`fit` repeat count with checked rational division/ceiling for exact score spans.
Retain f64 for performed seconds, DSP controls and tempo/frame conversion where
inexactness is intentional. Large exact-to-tick conversion must check range and
round once, preserving current PPQ/rounding semantics; do not introduce a new
timing grid. Diagnostic changes for accidental unit acceptance and fractions
require language-reference examples; no automatic source rewriting or warning
period is necessary for this pre-alpha project.

**Expansion:** a small `ExpansionCost` counts notes, controls, raw messages, raw
payload bytes, and variable annotation/tag/key bytes before cloning. Checked
addition/multiplication must precede allocation in repeat, fit, seq/stack/overlay
and callback expansion. Include repeated key-prefix growth; an event count alone
does not bound nested metadata. Start with existing 200,000-per-stream limits,
repeat cap 10,000, and a configurable conservative logical-byte budget (default
256 MiB per resulting pattern, counting fixed per-event storage plus recursively
owned payload, not allocator-exact resident memory). Reject overflow and budget
excess with projected cost and limit. Permit hosts to lower limits; tests use
tiny limits. This budget is separate from imported-file byte/event limits and
`MAX_ACTIVE_NOTES/MAX_EVENTS_PER_BLOCK = 256` scheduling capacity.

For callbacks with unknown output size, check each returned batch before
appending; recursive callback operations themselves obey the same limits.
Preflight the maximum intermediate result for fit; do not allocate an oversized
repeat and rely on later slicing to shrink it. Use `try_reserve` where appropriate
and retain final validation as defense in depth. A compiler-wide hard resident
memory cap, persistent evaluator accounting and OS memory policy are deferred.

### B. Revision effects and atomic application

Classify changed *fields and prepared meaning*, not entire enum variants. A
revision can contain several effects. A conservative resource change dominates
and uses full preparation; narrow changes still form one atomic transaction.

| Effect | Fields/examples | Preparation and initial fallback |
| --- | --- | --- |
| Presentation | Title, display names, source/dependency watch metadata, track groups, sections used for navigation, note tags/keys/user data with no engine interpretation | Validate references and limits; coordinator accepts on receipt of a revision barrier; zero processor creation. Do not reset audio/scheduler state. Section render selection stays host/render metadata. |
| Schedule | Performed note on/off/pitch/velocity/expression/sample selection, CC/channel messages/order; MIDI channel filter; tempo/PPQ/end | Prepare all changed streams, compatibility checks and timeline outside processing. First fast slice requires stable source/channel identity and unchanged tempo/extent; otherwise full path until timeline retention is fixed and tested. |
| Controls | Native descriptor-approved setters and voice-patch declared controls | Validate ranges/units and configuration-dependent constraints without constructing processors. Pre-resolve safe setter slots. Unknown/dynamic plugin/rack controls retain conservative preparation/replacement. |
| Runtime lanes/lifecycle | Automation target/points/shape, tails and completion settings | Not presentation-only. Prepare lane objects/cursors and lifecycle bounds outside processing. Initially full preparation; a later isolated optimization may swap lanes with retained processors once control capabilities are proven. |
| Resources/topology | Devices/ports/order/routing/sidechains, lookahead/latency, voice mode/state layout, sample maps/versions/budget, plugin state/generation | Full candidate engine, existing retention/fades, and off-thread retirement. Do not optimize solely because a field is called a parameter. |

User annotations with recognized meaning participate through validated
performance data; do not classify all annotations as presentation. Tempo arrays
and PPQ/end summaries are authoritative for scheduling; unrelated summary counts
are derived and should be checked/recomputed. Note IDs are composer identities,
not `ArrangementScheduler` runtime voice IDs.

Introduce a prepared schedule payload owning replacement source arrays and
`ArrangementScheduler`, eventually one prepared transport/timeline shared by all
tracks. Split schedule construction from `TrackRuntime::new`; reuse the current
channel selection, expression support and sampler coverage checks using
capability metadata from the accepted processor description. Swapping a schedule
must not require creating a duplicate instrument. It must own retired arrays
until the receipt is drained, even for rejected/stale candidates.

Schedule validation must also account for overlapping active notes and maximum
block event bursts, including releases, expression updates and controller-state
restoration. Use the configured maximum block size and existing fixed capacities;
prepare bounded restoration batches/checkpoints or reject an unsupported schedule
before acceptance. Do not assume the 200,000 composition-event limit proves a
schedule fits 256 events in one callback. This validation belongs to the shared
schedule preparer used by both full and narrow paths.

Keep `ArrangementScheduler::adopt`'s existing rule for ordinary edits: no replay
of new notes whose onset is already before the current playhead; active notes
retain original off time, release velocity, channel, runtime note ID and complete
bounded expression program. Future events use the new schedule. Controllers and
program/bend/pressure restore current state as the existing schedule does; retain
the current policy for a removed lane (do not invent a reset-to-zero). Prepare
bounded checkpoints for restoration if needed so future fast paths do not scan
arbitrarily large histories in the callback. This is not automatic composition.

Resolve tempo interaction before admitting timeline changes to the fast path:
tempo/meter/extent edits are source revisions, not user seeks. Preserve current
beat position, running state, sample counter and audition-loop policy; carry each
held note's remaining wall-clock release/expression obligation into the new
frame origin. Separate source-timeline changes from explicit discontinuities.
User seek/restart/loop-wrap/panic keeps its flush behavior; source removal of a
track or incompatible instrument may still fade/retire under existing rules.
If a shortened piece ends before a held release, processing must continue for
that obligation and then the configured tail. Mode changes remain conservative
structural transitions with explicitly tested flush behavior. This follows the
brief's held-note requirement and fixes P3; it is an audible correction.

Prepare transport timelines and any new storage outside apply, including legacy
loop `UpdateTransport`. At the block boundary, first check the entire candidate:
accepted base revision, expected engine/config generation, device/track identity,
capability epoch and all operation capacities. Only then perform infallible
swaps/setters. A mismatch rejects the entire candidate before mutation. Accepted
model/locations/watch targets advance together on the matching receipt. Keep
single-flight/coalescing for desktop; resubmit queued work against the actual
accepted base. Latest observed-source generation is host/coordinator policy;
do not reject an already valid in-flight revision merely because a later edit
was observed unless the host explicitly superseded it before commit.

The public raw engine API currently does not itself establish revision ownership.
Add a small accepted-revision/config token at its transaction boundary, or an
owning revision controller used by both desktop and embedding. Prefer the engine
boundary so direct consumers cannot accidentally apply a stale prepared object;
do not expose an unchecked “validated” constructor that bypasses the guard.
Receipts retain all old/prepared objects if the return queue is full. No
allocation, final `Arc` release, object destruction, decoding or compilation in
application/processing; extend allocator/deallocator tests to every new path.

Parameter descriptors may remove trial construction only for devices whose
setter behavior is known: names, ranges, units, defaults, and effect flags
(`Control`, `Latency`, `Resource`, `Restart`). Reuse existing
`source::ParameterSpec` and voice-patch param ranges. Sample-rate and device-state
constraints are separate validators. Plugin metadata can be dynamic and require
the plugin's control/preparation thread; snapshot it with an instance/capability
epoch. Unsupported/dynamic changes use a prepared replacement with no retained
processor setter that can fail halfway through the commit. Never assume creating
a second plugin instance proves the first instance accepts a setter. Plugin
zero-construction optimization is deferred until its lifecycle contract is
measured and tested; native gains/patch controls can ship first.

### C. Inspection and playable transfer

Promote the worker's explicit contract into a public library module. Add
`SessionSummary` and `PlayableSnapshotV1`; names may change, their distinction
must not. Illustrative boundary (not production code):

```text
PlayableSnapshotV1 {
    version: 1,
    description: SessionDto,
    performed: [{ track_id, source_id, midi: ImportedMidi }],
    assets: [{ identity, kind, expected_version, reference, metadata }]
}
decode_checked(bytes, limits) -> validated snapshot
restore_checked(snapshot, asset_resolver) -> playable description
SessionSummary::from(accepted_description, revision) -> bounded status data
```

Use a list of ID-keyed associations so duplicates are detectable during decode;
a deserialized `BTreeMap` alone cannot detect repeated JSON keys. Require exactly
one event bundle for each MIDI track, even an empty musical track. Check duplicate
track/source IDs, unknown/non-MIDI associations, missing entries and source-ID
mismatch. Track order may change without changing the association. Pattern-source
notes remain inline in the description. Recompute/check summaries and validate
PPQ, tempos, finite times/values, checked onset-plus-duration, MIDI ranges,
message lengths/status, bounded expression lengths, ordering and limits before
any schedule indexes data. Reuse these checks for direct embedding input, not
only worker JSON. Source-generated and imported MIDI budgets differ: never
apply the 8,192 imported-file-note limit to a compiled 200,000-note score.

Complete playable meaning includes performed note expression/selection, CC and
supported raw channel streams, tempo/PPQ/end, transport, routing/devices,
automation/tail, and every required sample/plugin-state reference/version.
It does not mean bundling sample bytes, plugin binaries, licenses, or guaranteeing
future bit-identical output. Opaque/SysEx data not supported for runtime playback
must be rejected, not silently represented as playable. Existing SMF export
remains a separate lossless structural document facility.

The desktop worker may initially use filesystem references and current
size/mtime stamps. Label these as weak revision tokens, not content hashes.
Missing or mismatched expected assets rejects preparation; do not silently render
changed data under an old accepted snapshot. In-memory hosts supply their own
stable identity/version and preloaded content; this motivates F without blocking
the initial shared wrapper. Plugin installed-version/state compatibility remains
explicit and host dependent.

Require an exact supported top-level snapshot version and reject unsupported
versions before preparation. Keep version 1 strict for meaning-bearing fields;
optional inspection metadata may evolve with documented defaults. No long-term
migration framework. This transfer version is distinct from the legacy JSON5
project schema constant. A same-binary worker can switch atomically to the shared
wrapper. A browser adapter can convert its old wrapper temporarily, then remove
the duplicate once its host upgrades. Never infer a complete snapshot from bare
`Session` JSON with skipped arrays.

Summary contains revision, IDs/display labels, resource/event counts, transport
and bounded diagnostic excerpts; no performed streams, full patch node graphs,
or arbitrary annotations. Default response budget: 1 MiB, at most 1,000 rows,
bounded display strings and explicit `truncated`/cursor metadata. Detailed
performance/patch/automation/graph views remain explicit requests with pagination
or a bounded export path; filter and cap before serialization, not after cloning
the entire session. Preserve the current graph view during consumer migration;
do not silently truncate it into something that appears playable. Status and
summary are not aliases for detailed export. Large transfer decode uses a
separate configured byte/expansion budget; no arbitrary truncation of music.

### D. Validated descriptions within the existing crate

Move description-side types (`Extras`, sections, automation, groups) into a model
module; compiler produces them. Move domain errors and compact source-neutral
location references into neutral modules. Frontends map errors to diagnostic
help/origins; music/expression/runtime must not depend on parser/compiler for
error construction. Keep current public re-exports temporarily to ease consumers.

Introduce a private-construction `ValidatedSession` after pure description/event
validation, and a tagged `DeviceDescription` payload:
`Native { kind, controls }`, `VoicePatch { graph, controls }`,
`Sampler { zones, controls }`, `Rack { branches, controls, modulation }`,
`Plugin { format, identity, state, controls }`. Derive the device kind from its
payload; incompatible optional fields cannot survive validation. Unvalidated
JSON/Rust DTOs remain supported at interchange through checked conversion.

`ValidatedPatch` owns ordered typed nodes with input indices, output channels,
lifetime/voice policy, explicit resource settings and separate control
descriptors/defaults. Source records lower to this description; legacy JSON
patches decode into it using the same validator. Runtime preparation adds
sample-rate coefficients, buffers, decoded assets and mutable voice state.
Structural equality compares the typed structural fields, not a cloned JSON
object with keys deleted. Control values have their own equality/delta. Graph
sharing remains determined by source binding identity during lowering: never
hash-cons equal oscillators/noise nodes. IDs are deterministic traversal labels,
not persistent pointer addresses or numerical-expression common subexpressions.

This removes duplicate field-name/choice/default/node-reference checks from
source lowering and runtime JSON parsing, duplicate native parameter range
tables, and JSON stripping for structure comparisons. Runtime sample-rate,
memory, plugin, sample availability and dynamic capability validation still
exists; it is not redundant with source validation. JSON remains appropriate
for external encodings, inspection, user metadata and plugin-specific opaque
state. Keep descriptors small per device/op; no universal schema system and no
static promise about all dynamically generated plugin parameters.

### E. Bounded provenance and revision explanation

Add optional compile-owned provenance, separate from `MidiNote` runtime fields
and active voices. Intern source references and a compact context table;
events reference definition, placement path and latest relevant transform/edit.
First subset: `.muz` musical notes created by note/phrase/drums, ordinary
placement/repeat, sparse map/refine/select and `std/arrange` named placement.
Imported MIDI events get asset identity plus source track/order, not fabricated
source spans. Controller/raw/generated callback expansion can initially expose
an unavailable-detail reason and later add the same scheme.

Use existing composer identity `(physical track ID, Note.key)` for matching
revisions, including occurrence namespaces. It survives edits that preserve keys,
not arbitrary source rewrites. Keep exact strings/path components; do not parse
display names, periods or source offsets to invent identity. Explicit placement
keys are stable; positional repeat/sequence keys are only stable while their
ordering remains stable. Duplicate keys yield ambiguous identity (or existing
overlay diagnostics); never silently pair the wrong events in a diff. Nested
occurrence paths are interned parent/child context entries, bounded by existing
evaluation depth. Transformations that duplicate notes derive child keys;
filter/removal produces removals, not tombstones in every surviving event.

Capture pattern construction and transformation origins through evaluator call
context into a sidecar carried with pattern values; transformations propagate
that sidecar with keys. Store definition + occurrence context + last relevant
override, not an unbounded transform log. Do not put paths into `Note.data`.
Plain musical equality and runtime signatures exclude this sidecar. The compiler
projects the selected provenance into `Compiled`, keyed to final performed IDs.
No content-based merging of independent musical or signal values.

First navigation guarantee is exact definition and direct source placement/edit
when available. `std/arrange` expansion reliably supplies the occurrence key path,
but evaluation may only retain the `build` caller rather than the occurrence's
declaration span. Return that fallback with its attribution quality; do not claim
an exact occurrence declaration that was lost. Later general value-origin
propagation can improve this without teaching the kernel passage/gesture policy.
This useful subset meets explanation needs without a second arrangement language
or a privileged arrangement builtin.

Bound `explain(event_id)` to the three references and a short reason; paginate
revision diffs using the same 1,000-row/1 MiB response limits. Diff separately
reports musical fields (notes/occurrences/controllers) and prepared consequences
(processors retained/replaced, routes, schedule/timeline and controls). A change
to ordinary tags/location is visible in authoring metadata, not an audio change.
Matching is deterministic; ambiguous identities report remove/add with reason.
Provenance is optional for playback and excluded from playable snapshots by
default; it may be transferred in a separately bounded inspection sidecar.
Read-only navigation/explanation does not authorize source rewriting.

### F. Narrow asset and execution context

Keep `SourceLoader` and synchronous language evaluation. Add a host context with
source loader, `AssetResolver`, explicit `CompileLimits`/`PrepareLimits` and
per-operation cancellation token. Existing entry points construct CLI defaults;
new `*_with_context` entry points allow independent hosts. `graph_budget()` can
remain a CLI-default adapter, but validation/telemetry must use the same captured
per-instance limit instead of consulting process `OnceLock` repeatedly. Default
4096 graph units and existing caps remain. CLI Ctrl-C cancels its own context;
two in-process compilations must not share sticky global interruption state.
Check cancellation in large expansion/preparation loops, never block or allocate
in the audio callback.

Asset service minimum: resolve reference relative to declaring module into an
opaque stable `AssetId`; inspect kind/size/version/audio metadata; open bounded
read/seek content for MIDI, WAV/FLAC and plugin-state blobs. An immutable prepared
handle binds the version to the bytes used, avoiding a separate inspect/read race
as far as the host implementation supports. Desktop stays filesystem-backed;
browser hosts preload asynchronously and expose synchronous reads of that
prepared content. No async function semantics in `.muz`.

Separate resolution and metadata during evaluation/lowering, decoding/validation
during preparation, and immutable prepared PCM/runtime state during processing.
Reuse `smf::decode`/`import_midi(bytes)` and generalize audio decoders over readers.
Use asset IDs plus version and decode settings for sample-cache sharing; preserve
independent sampler/reader playback state even when decoded PCM is shared.
Plugin bundle discovery/installation/native instantiation remains desktop
capability outside the generic byte service; plugin-state bytes may use it.
Missing asset, unsupported kind/codec, stale version and unavailable native
plugin must be distinct source-attributed errors before application.

Initial host slice supports source plus in-memory MIDI; next adds WAV/FLAC sample
preparation. After that slice, browsers still lack native VST3/CLAP, plugin bundle
loading, filesystem watching/control sockets, hardware MIDI/recording and engine-
provided browser output. The browser owns worker scheduling/AudioWorklet output.
Do not move muzpad's UI/storage/session lifecycle into muz to claim portability.

### G. Ergonomics

Add `std/arrange.measures(count, meter)` as the descriptive primary helper;
preserve `bars` as a delegating alias. Keep literal bars fixed at four quarter
beats everywhere. Reuse checked meter rules in source where expressible. Do not
warn merely because a reusable literal eventually appears in a non-4/4 song:
its construction context may be unrelated to the final meter. A future lint may
flag a directly authored, known-context use with a clear location, but no warning
is necessary for the alias delivery.

Defer rest parameters and call spreading. List-taking helpers and
`fold(records, {}, fn(a,b)=>merge(a,b))` cover the inspected library needs.
Keep current variadic `merge` compatibility, including named base/overrides and
last-wins shallow updates. If demonstrated forwarding needs later justify it,
the narrow candidate is one final positional `...rest` parameter and array-only
`...items` expansion at call sites, left-to-right evaluation, existing named
binding/default rules, expansion caps, and explicit duplicate-binding errors.
Record keyword spread, variadic defaults, classes/macros and broad syntax changes
are out of scope. No parser implementation is scheduled by this plan.

## 3. Dependency-ordered implementation slices

Each slice is independently reviewable. “Small/medium/large” describes breadth;
uncertainty reflects evidence gaps, not a calendar estimate. Tests named below
are planned unless listed as executed in section 1. Remove each resolved friction
entry with its implementation, reference documentation and focused verification.

### S1 — Checked legacy clip conversion (small, low uncertainty)

- Purpose/scope: eliminate A1 panics immediately; no general timing redesign.
- Files: `src/compile.rs` clock loop/helper, `tests/diagnostics.rs` or a small
  synthetic clip-boundary test; `docs/language.md`, friction log.
- Prerequisites: none. Use current track `Origin` plus note key.
- Acceptance/tests: missing each companion, wrong types/null, negative/zero
  duration/span, span shorter than duration, extreme values and valid clips
  through placement/tempo all return correct results or attributed diagnostics.
  Use `compile_with_loader` for source attribution. Run existing arrangement clip
  test. A headless accepted → malformed → corrected edit sequence must retain the
  accepted revision/audio after rejection and accept the correction. No panic.
- Compatibility/risk: malformed numeric-marker payloads now error; arbitrary
  unrelated metadata still works. Avoid broad rejection of all `clock_*` keys.
- Stop: checked boundary and focused regression pass; no typed patch migration.

### S2 — Complete route deltas (small, low uncertainty)

- Purpose/scope: compare full stable-ID route semantics, fixing A2.
- Files: `src/reconcile.rs`; new focused reconciliation/route lifecycle tests,
  reusing constant-probe ideas from `tests/sidechain_tap.rs`; friction log.
- Prerequisites: none; may run parallel to S1 without shared production files.
- Acceptance/tests: independently change `pre`, `to`, `gain_db`; exercise track
  and bus sends, outputs, route ID removal/addition, parent/order changes and
  unchanged routes. Pre-only plan contains `UpdateRoute`. Live applied candidate
  matches fresh constant-probe output; compatible instrument token is retained.
  Existing sidechain test continues to pass. No callback allocation/retirement.
- Compatibility/risk: audible bug fix; retain current structural/fade behavior.
- Stop: all current Route fields accounted for, without optimizing routes yet.

### S3 — Recursive semantic equality (small/medium, low uncertainty)

- Purpose/scope: implement the equality table and use it for operators/membership.
- Files: `src/lang/eval.rs`, `builtins.rs`, `tests/numbers.rs`, language guide.
- Prerequisites: none. Coordinate `builtins.rs` with S4/S5.
- Acceptance/tests: P1 cases corrected; nested record/list shapes, null and type
  differences; ms/s, kHz/Hz, beat/bar; exact thirds and mixed finite controls;
  same/different closure identity and function/string distinction; typed patterns
  and `!=` agreement. Deterministically generate small nested data trees and check
  symmetry, operator/membership agreement and incompatible dimensions. Do not
  assert transitivity of mixed f64/exact equality beyond its chosen contract.
- Compatibility/risk: changes accidental function/collection equality; seeded
  `keyed_noise` and canonical grouping remain unchanged and get regression checks.
- Stop: one equality definition, documented precision and non-data semantics.

### S4 — Conversion helpers and migration (medium, medium uncertainty)

- Purpose/scope: introduce helpers and migrate boundary families in small commits:
  meter/count/index first; time/tempo/MIDI next; native parameters/automation last.
- Files: `lang/eval.rs` or a small `lang/convert.rs`, `builtins.rs`, `compile.rs`,
  `patch_source.rs`, source control/parameter descriptors, relevant numeric tests.
- Prerequisites: none for helper/meter slice; share descriptor definitions with S8
  rather than creating parallel tables. Equality normalization can be reused.
- Acceptance/tests: table-driven scalar/default/compatible/incompatible units at
  every inventory family; exact/inexact fractions, boundaries, saturation cases,
  negative indexing and integer overflow. Exact stretch/fit preserves representable
  rationals; performed-seconds precision tests stay green. Preserve fractional
  musical pitch while enforcing discrete MIDI fields. Check std/examples using
  affected interfaces; do not bulk rewrite pieces.
- Compatibility/risk: rejects undocumented accidental inputs; descriptor defaults
  must reflect current units (especially milliseconds versus seconds). Guide
  includes explicit replacements. No epsilon equality/type-system project.
- Stop: each migrated family complete across source and direct DTO validation;
  track remaining call sites in this plan during execution, not a new log.

### S5 — Preflight expansion cost (medium, medium uncertainty)

- Purpose/scope: reusable checked cost and limits for pattern expansion/append;
  no realtime-capacity changes or full evaluator memory allocator.
- Files: `music.rs`, `lang/builtins.rs`, context defaults, numeric/event tests.
- Prerequisites: S4 integer helper useful, not mandatory for repeat's existing
  count check. Host-config API can later carry the same limits.
- Acceptance/tests: tiny configured budgets reject projected note/control/raw
  counts and payload/key bytes before clones/reserves; checked multiplication and
  addition overflow; repeat/fit, seq/stack/overlay, map expansion and zero-count
  cases. P1 raw/control examples now fail during evaluation. Track allocations in
  synthetic rejection if needed to prove no oversized intermediate is built.
- Compatibility/risk: conservative byte accounting may reject very large metadata;
  error exposes projected cost and override route. Never silently drop events.
- Stop: all adjacent expansion entry points share preflight; final checks remain.

### S6 — Typed clip timing (medium, medium uncertainty)

- Purpose/scope: eliminate internal annotation namespace and enforce stated
  transform support; no ambient tempo or new trim composition policy.
- Files: `music.rs`, `lang/builtins.rs`, `compile.rs`, pattern serialization adapter,
  arrangement/event tests and language guide.
- Prerequisites: S1; S4 time helpers preferred; coordinate with S5/E sidecars.
- Acceptance/tests: valid clip placement/repeat agrees with previous timing;
  arbitrary `clock_*` metadata cannot activate timing; unsupported transforms
  diagnose; typed invalid data cannot enter validated compilation. If legacy
  decoder is supplied, old explicitly versioned payload converts and malformed
  payload fails. No implicit legacy parsing in new annotations.
- Compatibility/risk: direct pattern/Rust struct users need adaptation; documented
  clip source stable. Stop after one explicit timing representation and docs.

### S7 — Revision guard and tempo/transport retention (medium, higher uncertainty)

Implemented: transactions check their base revision and prepared audio configuration
before mutation. Transport timelines are prepared off callback. Compatible source
tempo edits retain running state and held-note release/expression obligations;
explicit transport-mode changes still create a discontinuity. Synthetic constant
voice and stale/config-mismatch tests cover the corrected boundary.

- Purpose/scope: fix P3 and make the commit contract explicit before cheaper paths.
- Files: `audio/transaction.rs`, `engine.rs`, `transport.rs`, `live.rs`,
  `audio/pipewire.rs`, lifecycle tests; no generalized incremental compiler.
- Prerequisites: S1/S2 can ship independently; S4 transport validation helps.
- Acceptance/tests: P3 preserves held constant audio and emits exactly its original
  eventual release; changed tempo/extent carries expression/release velocity and
  sample counter; explicit seek/restart/loop/panic still flushes. Stopped/running,
  end-of-piece/tail, mode transitions and tempo changes with queued edits tested.
  All timeline storage prepared outside apply, including legacy loop updates.
  Wrong base/config, duplicate apply and rejected candidate mutate nothing;
  receipt-full retains ownership until coordinator drains it. Extend allocation
  and deallocation checks to each application branch and retirement.
- Compatibility/risk: audible correction; source edits must not masquerade as
  explicit transport commands. Keep structural candidate preparation for now.
- Stop: both narrow and full paths can rely on the same atomic revision rules.

### S8 — Shared descriptors and native control validation (medium, medium uncertainty)

Implemented the initial static subset: poly synth, gain and voice-patch controls
validate without processor construction. Parameter descriptions now live in
`description`; `source` re-exports preserve callers. Other device controls use
conservative structural preparation and are not retained across changed values.

- Purpose/scope: move/extend existing parameter specs; eliminate trial construction
  for proven native controls, not all plugins/racks.
- Files: `source.rs`, a model/device description module, `compile.rs`,
  `audio/device.rs`, native device setters and `audio/transaction.rs`.
- Prerequisites: S4 conversion contract, S7 commit guard for capability epochs.
- Acceptance/tests: source, DTO and setter validators agree for min/default/max
  and invalid values; resource/latency-affecting fields choose full preparation.
  Synth gain and patch param value edits show zero construction, retained instance
  and changed output; parameter removal rejects or resolves to the documented
  default before commit. A mixed invalid change cannot partly mutate controls.
  Simulated changing plugin metadata forces fallback/rejection before commit.
- Compatibility/risk: range-table drift and dynamic metadata. Retain conservative
  plugin construction/replacement and lifecycle threading; no timing gates.
- Stop: useful native subset and shared validation, no universal descriptor schema.

### S9 — Presentation barriers and prepared schedule swaps (medium/large, medium uncertainty)

Implemented metadata-only barriers and compatible native MIDI/schedule swaps.
Automation/tail, source identity, transport mode and dynamic plugin changes stay
structural. The lifecycle test checks zero processor constructions and zero
allocation/deallocation during metadata, control, note and tempo application.

- Purpose/scope: cheap metadata revisions first, then unchanged-timeline MIDI
  schedule edits, then timeline integration after S7. Keep automation/tail on
  full path until separately justified.
- Files: reconcile effects, `audio/transaction.rs`, `engine.rs`, `transport.rs`,
  `live.rs`, lifecycle tests and preparation-counter tests.
- Prerequisites: S7; S8 for mixed control/schedule optimization. Typed patches are
  not required to extract schedule preparation.
- Acceptance/tests: title/name/group edits construct zero processors, preserve
  counters/voices, and commit metadata atomically. Note/CC/expression-only edits
  with compatible source/capability prepare zero processors, retain active release
  obligations and controller behavior, and use new events only from the cutover.
  Tempo changes prepare one coherent timeline, recompile all affected schedules,
  and preserve S7 semantics. Removed/unsupported resources take full fallback.
  Valid → invalid → corrected → superseded edit sequence and config change while
  preparing cannot partly apply or retire on callback. Counter expectations are
  regression gates; timing remains informational.
- Compatibility/risk: extent changes disguised as note edits, metadata annotations
  with runtime meaning, and late capability changes. Stop after each proved
  category; do not classify all `Extras` or all MIDI updates as cheap by name.

### S10 — Shared playable snapshot and bounded summary (medium, medium uncertainty)

- Purpose/scope: public transfer module, same-process-version worker migration,
  validated associations/events, and bounded status. No asset bundler/archive.
- Files: new `snapshot.rs`, `model.rs`, `worker.rs`, `inspect.rs`, `control.rs`,
  embedding docs and transfer tests. Host adapter tracked in its own repo.
- Prerequisites: can proceed independently of S7–S9 using current DTOs; S4/S5
  supply validators/limits. Do not wait for all of D/F.
- Acceptance/tests: real worker transfer preserves controlled native rendered
  meaning including CC, expression, tempo and selected samples; reordered tracks
  restore by IDs. Missing, duplicate, unknown and non-MIDI associations, bad version,
  inconsistent summary/PPQ, invalid message/expression lengths and overflow fail
  before preparation. Decode byte limits reject before unbounded allocation.
  Summary size remains bounded as event/metadata counts grow; detailed views page.
  Plugin probes assert identity/state/parameter/event contract only when fixtures
  exist, not bit-identical output. Keep P1's native actual-worker comparison small.
- Compatibility/risk: existing external `Session` serializers. Retain bounded
  legacy graph view, publish an adapter and update worker before deprecating bare
  transfer. Stop when one tested public wrapper replaces custom worker restoration.

### S11 — Neutral model and validated device/patch descriptions (large, medium uncertainty)

- Purpose/scope: first move neutral types/errors with re-exports, then tagged device
  payloads, then validated typed patches. Three separable commits/slices inside
  one crate; no workspace split.
- Files: model/description modules, `compile.rs`, `music.rs`, `expression.rs`,
  `source.rs`, `patch_source.rs`, runtime patch preparation, snapshot adapters.
- Prerequisites: S8 descriptors and S10 explicit DTO boundary reduce migration
  risk. Neither panic/route fixes nor schedule optimization waits for this slice.
- Acceptance/tests: source and JSON compile to equivalent validated descriptions;
  invalid payload combinations/node references/ranges cannot reach preparation;
  existing patch-control and sharing tests pass, including independent noise and
  explicitly shared nodes. Resource/default changes invalidate correct structural
  identity; control changes do not. Same native render for small representative
  typed patch operations. Portable library still builds.
- Compatibility/risk: public Rust structs and serialized patches. Keep checked DTO
  adapters/re-exports during transition. Stop after each boundary; runtime still
  owns configuration/plugin/asset checks that validation cannot discharge.

### S12 — Host context and assets (medium/large, higher uncertainty)

- Purpose/scope: per-instance limits/cancellation first; asset identities and
  in-memory MIDI next; reader-backed WAV/FLAC preparation last.
- Files: `lang/loader.rs`, evaluator/compile APIs, `assets.rs`, `smf.rs`, `midi.rs`,
  `audio_file.rs`, sample preparation, graph budget/telemetry, embedding docs.
- Prerequisites: reuse S5 limits, S10 asset manifest and S11 neutral interfaces;
  per-instance context can start earlier with no sample API change.
- Acceptance/tests: two contexts with different limits/cancellation do not affect
  one another; CLI environment defaults behave as before. Memory source/MIDI and
  tiny generated WAV/FLAC fixtures prepare with no filesystem reads; same decoded
  asset shares PCM but independent readers retain state. Missing/unsupported/stale
  assets reject before commit with declaring-module location. Compile/check native
  default and portable/WASM library targets; exercise host bridge after its
  separately scoped adaptation. No browser plugin support implied.
- Compatibility/risk: lifetime/Send bounds and decoder seek requirements. Keep
  filesystem adapter and simple CLI constructors. Stop after each asset family;
  no async evaluator or universal storage framework.

### S13 — Provenance explanation and semantic diffs (medium, higher uncertainty)

- Purpose/scope: optional compile sidecar and bounded read-only APIs for the E
  subset. No automatic source edits or universal expansion history.
- Files: evaluator/pattern provenance propagation, `compile.rs`, `inspect.rs`,
  declaration-source helpers; arrangement/revision examples and synthetic tests.
- Prerequisites: S3 ignores sidecars, S6 timing representation is stable, S9 effect
  reporting and S10 bounded views can supply audio consequences. Do not block
  explanation-only delivery on host asset work.
- Acceptance/tests: repeated/nested explicitly named occurrences explain material
  definition, key path and latest attributable change; stable IDs survive unrelated
  occurrence insertion; positional identities advertise weaker stability. Remove,
  expand, rename and ambiguous duplicate cases yield truthful bounded diffs.
  Known origins navigate to correct modules; unavailable occurrence spans report
  fallback quality. No sidecar growth in voices or effect on render/equality.
- Compatibility/risk: source offsets are revision-local; compiler must not claim
  identity stability from them. Stop at useful notes/occurrences and honest fallbacks.

### S14 — Measures alias (small, low uncertainty; optional after correctness)

- Purpose/scope: source-level descriptive alias, no literal semantic change/warning.
- Files: `std/arrange.muz`, language/revision examples, existing arrangement test.
- Prerequisites: S4 meter rules settled; otherwise independent of runtime work.
- Acceptance/tests: measures in 3/4, 6/8, 7/8 and default 4/4; old `bars` helper
  agrees; literal bars remain fixed in nested/reused material. Invalid meter fails.
- Compatibility/risk: additive only. Stop with alias/docs; rest/spread stays deferred.

### Dependencies, safe parallel work and shared files

Start S1 and S2; follow with S3–S5 and the narrow tempo/transport S7.
S6 removes the clip namespace independently of reload work. S8 and S9 then remove
measured unnecessary preparation. S10 can run alongside reload work because its
first wrapper uses the current description and independent event validation.
S11, S12 and S13 are later investments with clear stopping points; S14 is optional.

S1/S2 are good independent assignments. Later, transfer/inspection and
transaction/engine work can proceed separately with an agreed DTO boundary.
Avoid concurrent edits to `builtins.rs` across equality/conversions/budgets/clip
provenance, to `compile.rs` across conversion/timing/model moves, or to
`transaction.rs`/`transport.rs` across S7–S9. Land neutral model moves before
patch migration instead of repeatedly rebasing behavior changes through them.
Coordinate the single friction log at integration; no per-agent logs. This
describes safe future parallel execution; no implementation agents were launched
for this planning task.

## 4. Migration and rollout

1. Ship isolated correctness fixes with diagnostics and synthetic regressions.
   Preserve documented source conveniences and publish rejected accidental forms.
   No piece becomes an engine fixture; any piece adaptation belongs in its own repo.
2. Extract pure validators/descriptors while keeping the full preparation path.
   Add revision guards and fix tempo retention before widening cheap categories.
   Enable each zero-construction category only with lifecycle/allocator coverage;
   unknown capabilities choose the conservative path. No production performance
   threshold until representative timing is stable.
3. Introduce explicit snapshot/summary APIs alongside existing `Session` and
   inspection APIs. Switch the internal worker in the same change. Document how
   hosts translate their wrappers; adapt muzpad separately using its existing
   prepare-worker/latest-generation handoff. Its current full engine replacement
   is host behavior, not evidence it already uses engine live reconciliation.
4. Move domain descriptions and add tagged/validated internal types behind checked
   adapters. Keep JSON source/interchange readable; serde round-trip is not validation.
   Default-field/re-export compatibility is preferable to a simultaneous consumer
   rewrite. Version meaning changes explicitly; reject unsupported snapshots.
5. Add context/asset overloads with filesystem-backed defaults, then opt-in host
   implementations. Preserve native desktop plugin/output behavior. Publish an
   explicit supported-capabilities matrix for portable hosts.
6. Add optional provenance responses and the measures alias. Old source remains
   playable without provenance or new host UI. No source rewriting is required.

Verification stays focused: table-driven and deterministic generated tests are
sufficient with current `tempfile`, serde and standard test facilities. No new
property-testing dependency is needed. A small sequence driver should track only
accepted revision, known active-note obligations and processor identity; it must
not duplicate the full scheduler. Compare route/snapshot meaning using independent
constant signals or simple native cases. Reuse actual worker and lifecycle paths.
Avoid frozen compositions, routine full-song renders, encode/decode suites,
verification-only media copies, or permanent timing gates.

## 5. Owner decisions and deferred work

No owner answer is required to begin S1/S2 or the specified core semantics.
The brief settles Rust/language preservation, explicit musical choices, source
arrangement policy and held-note obligations; repository evidence supports the
defaults above. Recommended byte/response budgets are configurable engineering
defaults, not a reason to block the first fixes.

Before choosing *which external host integration to implement next*, confirm its
required asset families and release cadence. Default is muzpad's in-memory MIDI
then WAV/FLAC and exact-version snapshot adapter; native plugins remain unsupported
there. This is a future prioritization question, not an unresolved dependency for
the plan. No external consumer changes or deployment are authorized by this plan.

Defer general rest/spread, ambient meter warnings, a static unit type system,
language server, macros/classes, alternate arrangement syntax, automatic musical
correction, persistent/incremental evaluator, whole-program dependency graph,
content-hash asset archives, plugin installation snapshots, future bit-identical
render guarantees, universal parameter/storage schemas, multi-crate migration,
browser UI bridges inside the engine, and optimizations for unmeasured plugins.
Do not add speculative architecture or development-process requests to friction.

## 6. First implementation handoff

**First change: S1 checked clip conversion.** At inspected HEAD, lowering reads
numeric `Note.data["clock_start"]` at `src/compile.rs:332`, indexes/unwraps duration
and span at `:333/334`, and validates the pattern only afterward. `annotate` makes
this user-reachable. Implement one checked helper returning a located diagnostic
and use it in that loop. Preserve the current numeric trigger for this patch and
the existing `clock_clip_placement_keeps_local_seconds_and_duration` behavior.
Use the P1 malformed payloads plus valid/invalid bounds and a headless rejected
edit sequence as acceptance. Remove only the resolved panic portion of the
friction entry; its namespace issue remains until S6. Do not start typed patches,
assets, provenance or a general compiler refactor.

**Second change: S2 route equality.** In the route match arm of
`plan_reconciliation`, replace the handpicked destination/gain test with semantic
`Route` equality. P1's pre-only change currently generates no operations and
produces `0.07104688` after live application versus `0.1769448` fresh. Recreate
that constant native probe, assert `UpdateRoute`, matching applied/fresh output,
and retained processor identity; table-test every Route field independently.
Re-run the existing sidechain tap test and the small allocation-guarded application
case. Remove the matching friction entry with the fix. Keep full structural route
preparation; narrower routing optimization is not part of this handoff.
