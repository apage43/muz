# Native SFZ support for the current contrib libraries

Status: engineering plan only; no engine behavior is implemented by this document.
Audit date: 2026-10-01. Repository baseline: `abb02405cf4015322d3dcf3d7b916b1e03e1ce96`.

[Documentation index](README.md) · [Instrument guide](instruments.md) ·
[Contrib inventory](../contrib/README.md) · [Embedding](embedding.md)

## Goal and scope

Play the original SFZ programs of every SFZ-based sample library already represented
in contrib using muz's own engine, without requiring an SFZ plugin. Cover the full
pinned libraries, including their articulation, microphone, release, noise, control,
and performance programs, rather than reproducing only today's handwritten native
presets. Preserve those presets and their deliberate register, gain, and voicing
choices. Expose new original-program builders alongside them before considering any
change of defaults.

The scope is Sonatina 4.0, Virtuosity Drums 0.925, Karoryfer Shinyguitar and Black
And Blue Basses, Unreal Standard Guitar, and Unreal METAL-GTX. VSCO 2 CE is a real
exception: its currently pinned sample repository has no SFZ files. Keep its existing
native maps working; do not fabricate an upstream SFZ provenance or make finding a
new mapping repository a prerequisite for these six SFZ libraries. Supporting an
independently sourced VSCO SFZ edition requires a separately reviewed, pinned mapping
source with verified sample compatibility. SF2, Kontakt, DecentSampler, proprietary
bank GUI execution, and universal SFZ-spec completeness are outside this project.
Plugin recipes in `contrib/plugins` are not additional SFZ sample libraries.

Recommend a Rust SFZ importer and dedicated multilayer processor sharing muz's asset
and sample DSP infrastructure. Do not flatten SFZ into `SampleZone`, or execute an
external player as the native implementation. A Rust implementation fits the existing
Rust core, memory-backed AssetResolver and WASM build; wrapping a C++ player would
introduce a separate hosting/build/state layer that would still need these integrations. The importer handles authoring syntax;
the processor handles musical state. This separation gives native, embedded/WASM,
and CLAP hosts the same semantics without putting file parsing on the audio thread.

## Verified inventory and provenance

Evidence comes from current tracked manifests and installers, local installed asset
bytes, and read-only GitHub tree/content requests at the pinned revisions below.
External assets were not copied into git. The personal checkout's tracked manifest
files were verified by size/hash content checks; the current main's engine and pack
changes were inspected in an isolated checkout. GitHub trees were not truncated.

| Library | Immutable source | Mapping corpus and intended public coverage | Current dependency situation |
| --- | --- | --- | --- |
| Sonatina | `peastman/sso`, v4.0, `64a66eda18c5cc1039a56c902d0555df56742300` | 747 SFZ files: 557 cataloged playable programs and 190 include fragments; preserve all catalog IDs, including KS and legato/performance variants | All 3,827 manifest records present and SHA-256 verified. Install mappings and their sample closure together. |
| Virtuosity | `Virtuosity_Drums_v0.925.zip`, archive SHA-256 `c6c5d0fe11a394e94be3146a950c3377ec102cb57d189d5a23cec26183d1963a` | 419 SFZ files, eight top-level entry programs; mapping/control fragments remain dependencies | Archive matches pin. Manifest extraction declares 4,858 samples, 419 Programs files; local counts agree. Include `*silence` as an internal silent source, not a missing recording. |
| Shinyguitar | `sfzinstruments/karoryfer.shinyguitar`, `57243cca85277dbcc120ce17c6178032f93c80f3` | 16 SFZ files, `Programs/main.sfz` is bank entry; the other files provide layers/maps | Tree has 846 WAV files, all 846 named in raw sample references. Current manifest installs 340 recordings and metadata; 506 referenced recordings are outside that subset. |
| Black And Blue Basses | `sfzinstruments/karoryfer.black-and-blue-basses`, `6e7d674cdb41be7a54dbccb15472401ad01099b9` | 64 SFZ files, 11 numbered top-level programs; controls/maps include fragments | Tree has 2,208 WAV files, all named in raw sample references. Current manifest has 40 darkblack regular recordings; 2,168 referenced recordings are outside it. |
| Standard Guitar | Publisher RAR, 750,502,921 bytes, SHA-256 `0777b8cbbfe2fe79e843fa6c2c24ab300c2d5a538d46f4d970f057979f87fee0` | 612 installed SFZ files; six top-level programs plus their individual patch graph | Installer extracts the archive, but only 400 recordings are individually pinned by current manifest. All 400 verified. Local archive cache unavailable for an independent archive-hash recheck; SFZ file count is an installed-corpus observation, not a per-file provenance guarantee. |
| METAL-GTX | Publisher RAR, 1,343,285,011 bytes, SHA-256 `c5756f95fcc30ac680f6034bb2e54c636e67fd0b53b1d40babada7257e782e73` | 578 installed SFZ files; five top-level programs plus individual patches | Cached archive hash matches. Manifest describes 360 source recordings and 720 derived mono files; all 720 derived records verified. Full SFZ programs need original stereo recordings and additional releases/noises, not just derived mono takes. |
| VSCO 2 CE | `sgossner/VSCO-2-CE`, `440300901dfe9275fd84e0b7763af1f8443ae62e` | Zero SFZ files in pinned upstream tree; existing `.muz` maps remain native | Tree has 3,168 WAV files; contrib installs 266 recordings plus two metadata files (268 records), all verified. No corresponding SFZ mapping source is pinned here. |

The 383-record Karoryfer manifest includes metadata as well as samples; all records
verified. Two locally present Shinyguitar map fragments do not constitute the complete
upstream program. Read the upstream tree and dependency graph, not local presence,
to decide whether a pack is installed completely.

Source locations are preserved in each pack's `manifest.json` and README. Pinned
primary trees: [Sonatina](https://github.com/peastman/sso/tree/64a66eda18c5cc1039a56c902d0555df56742300),
[Shinyguitar](https://github.com/sfzinstruments/karoryfer.shinyguitar/tree/57243cca85277dbcc120ce17c6178032f93c80f3),
[Black And Blue](https://github.com/sfzinstruments/karoryfer.black-and-blue-basses/tree/6e7d674cdb41be7a54dbccb15472401ad01099b9),
and [VSCO](https://github.com/sgossner/VSCO-2-CE/tree/440300901dfe9275fd84e0b7763af1f8443ae62e).
The [Virtuosity archive](https://versilian-studios.com/Distro/Virtuosity_Drums_v0.925.zip)
and publisher pages for [Standard Guitar](https://unreal-instruments.wixsite.com/unreal-instruments/standard-guitar)
and [METAL-GTX](https://unreal-instruments.wixsite.com/unreal-instruments/metal-gtx)
are download provenance, not substitutes for the immutable hashes above. Sonatina's
`catalog.json` is useful stable program metadata; `export-catalog.py` explicitly is
not a runtime parser. No established separate design-plan series was found in docs;
this plan belongs beside the engineering guides and is linked from their index.
The listening and visualizer skills under `.agents/skills` are unrelated to this
planning-only change; use the listening skill later if a listening comparison is run.

### Public entrypoints and sample dependency audit

Virtuosity's entries are `01-basic-kit`, `02-full-kit`, `03-kick-mic`, `04-snare-mic`,
`05-oh-mic`, `06-mid-mic`, `07-room-mic`, and `08-vintage-mic` under Programs.
Standard Guitar's six are KSOP, KSOP XTracking, VSOP XTracking, VSOP, Guitar Chord
Central, and Simple Arpeggio (use exact numbered filenames from the archive).
METAL-GTX's five are Full, Lite, XTracking, Guitar Chord Central, Simple Arpeggio.
Black And Blue's eleven are darkblack keysw/keysw warm, babyblue all/warm, darkblack
pluck/pluck warm, ghost/ghost warm, stac, btb, and btb open. Catalog every program
explicitly; offer individual patch fragments as diagnostic/import entrypoints where
meaningful, but do not count an inherited fragment as an independently configured
instrument. Full support includes their behavior in the parent program.

Shinyguitar's `main.sfz` uses `$sample_dir` without defining it internally. Its pinned
`Shinyguitar.bank.xml` supplies `$sample_dir=../Samples`. Capture that one declarative
binding in the pack descriptor, with the XML source hash; do not build an ARIA GUI or
silently assume undefined macros mean an empty string. Standalone fragments require
explicit root/default bindings or a clear missing-context diagnostic.

The lexical survey traversed every available SFZ file, stripping comments and
collecting opcode spellings, headers and sample-reference text. It is more extensive
than the earlier representative research, but is **not an exhaustive semantic audit**:
it does not prove expanded inheritance, all numeric domains, player compatibility,
or complete dependency resolution. Installed Unreal trees contain extra and legacy
assets; raw sample-reference counts are not reliable installed-sample requirements.
Phase 0 must close these gaps before a program is advertised as supported.

For each public root, produce a deterministic, machine-readable inventory with:

1. Root identity, source version, root and fragment SHA-256, include edges, macro
   bindings, encoding, source locations, and a stable program ID.
2. Expanded regions, inherited values and defaults, supported opcode/value variants,
   modulation edges, key/velocity/controller ranges and maximum simultaneous layers.
3. Unique sample identities, hashes, bytes, format/rate/channels/frames and loop
   metadata; distinguish recording-backed sources from `*silence`.
4. Relative sample path, effective `default_path`, root-relative resolution, existence,
   and provenance. Show missing/ambiguous paths with include stack and region location.
5. Program completeness against installed assets and required downloaded content;
   classify unsupported behavior, upstream typo, frontend-only metadata, or missing
   context separately. Never delete unreachable-looking branches solely because they
   are silent at the default controllers.

Karoryfer's raw references normalize exactly to the pinned tree sample paths
(Shinyguitar beneath Samples; Black And Blue strips `../`). That establishes the
subset gaps above, but program-specific closures still require expansion. For
Virtuosity, hash extracted Programs/Samples from the verified archive. For both
Unreal packs, re-extract into a temporary audit directory from the verified RAR,
compare all installed SFZ dependencies, and generate hashes for the complete graph.
Do not trust old unmanifested files or count `Legacy_Sounds.rar` as part of a public
root without an actual dependency. Install originals for SFZ fidelity; retain current
mono-derived METAL-GTX files only for old native builders.

## Behavior coverage matrix

Every row below is required when reachable in a scoped public program. The examples
are observed spellings/families, not a declaration that every variation of the SFZ
spec must be implemented. Phase 0 emits exact per-program membership and test links;
the matrix becomes a tracked release gate, with no unclassified sound-affecting tokens.

| Behavior | Corpus evidence | Required implementation and gate |
| --- | --- | --- |
| Authoring syntax and hierarchy | `control`, `global`, `master`, `group`, `region`, `curve`; includes in SSO, Virtuosity, bass and Unreal; Shiny bank macro | Tokenizer with comments, BOM/CRLF, path spaces/backslashes, adjacent headers/opcodes, source maps; bounded include expansion, ordered macros, scope resets/inheritance/defaults. Test repeated includes under different parents. |
| Basic sample playback | `sample`, `key`, `lokey/hikey`, `lovel/hivel`, `pitch_keycenter`, `pitch_keytrack`, transpose/tune, offset/end/loops | Inclusive SFZ integer key/velocity endpoints; frame-domain offsets/endpoints; sample metadata and loop modes. Avoid reusing native half-open velocity matching. |
| Multilayer and selection | Mic/pickup layers; `seq_length/seq_position`, `lorand/hirand`, CC gates and key switches | Start all eligible regions; explicit sequence and shared random-event state with tested boundary/reset rules. Ordered selection trace must match reference. |
| Articulation and note history | `sw_lokey/sw_hikey/sw_last/sw_default`, `trigger`; SSO legato, Unreal pseudo-legato/hammer/pull | Keep held-key counts and keyswitch state per channel/device, preserve attacks vs legato and release semantics. Keys used for switching must not accidentally sound. |
| Release/sustain/choke | `rt_decay`, `sustain_cc`, `group/off_by/off_mode/off_time`, `polyphony`, `note_polyphony` | Separate key release, pedal hold, voice release and choke; owner-note duration/velocity capture; explicit choke ordering and release-tail policy. Silent regions can still control choke. |
| Gain and fades | volume/amplitude, velocity/key tracking, velocity curves, `xfin/xfout` key/velocity/CC families and `xf_cccurve` | Preserve units, curve domain and equal-power/linear laws according to evidence; simultaneous layers are summed, never round-robin alternatives. |
| Envelopes | `ampeg_*`, velocity-to-time, dynamic/shape and CC curves; Unreal `eg01` and `eg1..eg32` | Bounded compiled envelope segments with correct attack/hold/decay/sustain/release and dynamic-vs-latched CC behavior; numbered multi-stage envelopes and pitch routes are required. |
| Filters and EQ | cutoff/resonance/fil tracking, `fileg_*`, `fil2_type/cutoff2`, `eq1..eq3_*` | Per-layer filter state, actual encountered types, second filter and EQ; audit defaults and modulation units. Cannot replace with one track-wide filter. |
| Modulation | SSO `lfo01_*`, `amplfo_*`; bass `lfo02/03`, `var01/02_*`; Unreal pitch LFO/EG and CC140 | Compile sparse routes including LFO-to-LFO frequency, variables and extended CC sources; preserve alias distinctions until documented equivalent. No blanket 0–127 controller truncation. |
| Stereo and source variation | `pan`, `width`, width/pan CC routes, random pitch/gain/delay/offset | Stereo width semantics, correlated choice across layers, repeatable seeded variation and frame-exact delayed starts; default takes retain source channels. |
| Initialization and metadata | `set_cc*`, `set_hdcc*`, `label_cc*`, `group_label`, `sw_label`, curve values `v000..v127` | Initialize defaults once at program load/reset; expose labels/controls in inspection/catalog. Curves are indexed tables with tested interpolation, not ignored metadata. |
| Compatibility anomalies | Unreal `Key`, `Ampeg_release`, `bend_Down`; bass `lfo03_freq_lfo2_oncc117`, curve `v77` | Resolve each against pinned reference players and source intent. Record explicit corpus-specific aliases or actionable upstream errors; do not lowercase everything or silently "fix" source. |

Lexical counts of distinct opcode spellings were 167 for SSO, 125 Virtuosity, 84
Shinyguitar, 121 Black And Blue, 504 Standard Guitar, and 501 METAL-GTX. Numbered
EG/controller spellings heavily inflate Unreal counts; counts include potential
source anomalies and are not implemented-feature counts. The survey also observed
`group_volume/group_tune/master_volume`, `delay_random`, `offset_random`, pitch-bend
ranges and CC aliases (`*_ccN`, `*ccN`, `*_onccN`); include them in normalization and
the exact audit rather than restricting implementation to the table's short examples.

## Existing engine and integration points

`src/audio/device/sampler.rs` currently chooses **one** matching zone per NoteOn,
using a global rotating index unless an event pins `sample_zone`. It has a fixed
32-voice array, simple attack/release, shared loading, interpolation and basic loops,
and note-ID volume/pan/tuning/expression updates. It does not supply the general
SFZ selection, hierarchy, release-trigger or modulation state machine. `src/model.rs::SampleZone` stores
only path, static gain/root, key/velocity intervals, offset, loop and one-shot state.
That representation cannot encode overlapping mic layers or inherited SFZ behavior.
`patch_sample.rs` offers reusable readers, sample-budget checks and loop interpolation;
retain DSP helpers without adopting its graph as an SFZ intermediate representation.

`src/assets.rs` already supplies bounded versioned immutable reads and memory assets,
and enumerates sampler/patch/rack dependencies. Extend it to SFZ roots, include files,
and samples, with deterministic dependency ordering and cancellation. Keep stale-asset
and size-limit failures during preparation. `src/lang/eval.rs` on the audited main
now discovers contrib from override, checkout or installed user-data roots; new pack
builders must use that discovery and module-owned resolution rather than embedding
maintainer paths. `install.sh` installs contrib source separately from recordings.

Read `src/audio/device.rs`, `src/audio/engine.rs`, event scheduling and transport,
`src/model.rs`, `src/host.rs`, `src/compile.rs`, `src/device_state.rs`,
`src/dawproject/mod.rs`, and `crates/muz-clap/src/lib.rs` before decomposition.
Reconfirm these integration points and serialization boundaries against the then-current tree. Resource
accounting must include expanded regions, modulation storage, samples and voices,
not merely one root file. Coverage validation must understand stateful SFZ gates;
absence of a region at one controller setting is not a static missing-key error.

## Proposed representation and importer

Add a general source-level `sfz(path, options)` instrument constructor, following
existing module-relative sample/patch resolution. Pack functions select stable catalog
IDs and provide context bindings; they do not generate thousands of duplicated zones.
Options expose control initialization, deterministic seed, resource limits and thin
musical overrides. Keep the API small; musical presets remain in contrib/std.

Phase 0 should deliver `contrib/<pack>/sfz-catalog.json` (public root IDs and source
paths) and `sfz-audit.json` (schema/version, source fingerprints, per-root dependency
closure and exact behavior requirements). Include a reproducible audit command and
report format; the runtime parser then becomes its source of truth. Restricted-source
reports contain paths/hashes/metadata, not copied publisher patch text or recordings.

Use three layers:

- Source document/preprocessor: ordered tokens, includes/macros and precise source
  spans. Parse paths without naïve whitespace splitting; reject undefined macros,
  cyclic includes, excessive depth/expanded bytes and paths escaping allowed asset
  roots. `../Samples` within a pack is legitimate; unrestricted host traversal is not.
- Normalized immutable program: stable region IDs, typed predicates, sample/curve
  handles, envelope/filter definitions, sparse modulation routes, source diagnostics,
  program metadata and dependency fingerprints. Keep source provenance after flattening
  inheritance. Represent absent values until the opcode registry applies documented
  defaults; don't merge zero with unspecified. Normalize note names to MIDI keys.
- Prepared program: shared decoded sample buffers, precomputed curves, key-indexed
  candidate lists and compact region parameters. Allocate voices/modulator state and
  bounded event queues here. Cache by source closure, parser schema, overrides and
  preparation configuration; voices and RNG state are never shared between devices.

Build an opcode registry defining legal scope, units, ranges, default, inheritance,
modulation aliases, update timing and supported values. Compile modulation graphs
with explicit evaluation order; classify cross-modulation and feedback from observed
player behavior rather than arbitrarily rejecting or evaluating cycles recursively. Normalize aliases only when
validated; preserve unknown raw tokens in diagnostics. Global/master/group defaults
are copied/reset according to the SFZ hierarchy, while last assignment within a
scope wins. Preserve path case and Unicode; normalize separator syntax only. Prefer
exact path resolution. If a pinned corpus relies on case-insensitive filesystem
behavior, generate an explicit, hash-verified compatibility mapping after checking
for collisions, with a diagnostic; never silently lowercase filenames. Include lookup
is relative to the including file, while sample/default-path bases follow the player
semantics established for the program root, not an assumed fragment directory. Add
tests with case collisions, spaces, non-ASCII paths and legitimate parent segments.
Includes behave as textual expansion, so declarations inside them can
change the surrounding parse context. A later global/group header must not inherit
stale sibling state accidentally. Add golden normalized-region tests for each rule.

Default import is strict for unknown or unsupported sound-affecting behavior in any
reachable program branch. Collect diagnostics rather than aborting at the first token;
report root, include chain, source line, opcode/value and affected regions. Harmless
label/UI metadata can be retained without playback support. An explicit exploratory
partial mode may allow playback but must mark the program partial in inspect/export;
it cannot satisfy pack acceptance. No silent missing-sample substitutions except
explicit SFZ `*silence`.

## Runtime, expression and precedence

Implement one dedicated SFZ device using the normalized program. A logical note owns
zero or many region voices. Separate note identity and held-key/controller history
from physical voice slots. Capture onset key, velocity, random draw, sequence position,
keyswitch selection and owner timing; update only modulation defined as continuous.
NoteOff, NoteChoke and NoteExpression address all voices belonging to the note ID,
including delayed/release layers as defined. Overlapping same-key notes remain distinct.
Release regions need the original attack state and elapsed duration for `rt_decay`,
not a reconstructed NoteOn at note-off time. Determine whether release velocity and
controller predicates are latched or evaluated at release from reference evidence.

Maintain controller, pitch-bend, sustain and keyswitch state per device/channel;
preserve distinctions for mono vs poly history. Accept scheduled CC changes and host
MIDI through the common device event path. For extended controller numbers, define
typed internal sources rather than treating every value as a physical MIDI CC:
CC131/133/135/140 in these corpora require reference verification of their velocity,
random, bend or other virtual-source meanings. MIDI itself still has 128 CC numbers.

Region selection has deterministic stages: apply state-changing events in stable
frame/order; update key history/keyswitch; compute event-level random and sequence
inputs; evaluate all candidates; compute choke/polyphony effects; instantiate layers
in source order. Validate the exact ordering, sequence advancement domain, random
sharing, endpoint inclusion and same-frame events against reference fixtures. Do not
reuse the current sampler's global `next % matches` policy. Bound note and region
voice capacities separately; prefer stealing released/quiet tails then oldest notes,
with stable tie-breaking and short ramps. Apply SFZ group and note limits before the
global resource limit. Expose steals and dropped regions in inspection counters.

Recommended precedence:

1. Registry defaults, then SFZ declarations following inheritance and textual order.
2. Named pack context (external defines) during preprocessing; no accidental global
   defines shared across loaded programs.
3. Explicit initial controller overrides after SFZ `set_cc` defaults at load/reset.
4. Authored/host controller events in the shared event order during playback.
5. Note-ID expression as independent per-note gain, pan and tuning factors after
   SFZ region processing, then normal instrument/track mix processing.

Do not automatically map note expression to CC1, CC7 or CC11: doing so can double
apply dynamics and alter region selection. An opt-in source-level mapping can do it.
Retain existing supported expression kinds; add any further kinds only with a clear
host API contract. Document gain/pan composition and clamp domains. Note tuning
combines with region tuning/pitch modulation without modifying the selection key.
Thin native preset overrides may select program, initialized controls, track gain,
register constraints or mixer treatment. They must not silently overwrite every
region's envelopes or transpose keyswitches along with playable notes.

## Realtime safety, resources and deterministic rendering

Preparation performs parsing, version checks, decoding, allocations and expensive
index construction off the callback. Playback performs bounded candidate evaluation,
voice DSP and preallocated event handling with no filesystem access, logging, locks,
allocation or unbounded include/region scans. Reuse immutable sample buffers across
mic/round-robin references; budget decoded bytes and frames explicitly. Full SSO and
Unreal programs can exceed today's sampler limit. Recommend eager preparation of the
selected program only with a configurable hard budget and a detailed failure report;
streaming is a later project unless measured full-program footprints make eager
loading impractical. Do not reduce fidelity silently to meet the budget.

Use a specified PRNG with program/track seed and event identity; random draws must be
independent of audio block size and voice-stealing iteration. Sequence state is
explicit and serializable. Seek/restart reconstructs controllers, held-note history,
keyswitches, sequence and release state by deterministic event replay or validated
checkpoints; starting a mid-note SFZ at an elapsed sample offset alone is insufficient
for envelopes, LFOs, delayed layers and history. Live edits prepare a replacement
transactionally and keep the accepted program sounding if new assets fail.

Benchmark basic and full Virtuosity kits, dense SSO ensembles and the largest Unreal
roots at 44.1/48/96 kHz, 64/256/1024-frame blocks. Record candidate count, active region
voices, peak decoded memory, prepare time and callback p50/p99/max on identified
hardware. Set measured budgets before release; as an initial gate, p99 processing
should stay below half the callback duration on the declared reference workload, with
no allocation or overruns. This is a measurement target, not a cross-hardware promise.

## Hosts, state and export

The core implementation must compile without desktop features and use AssetResolver
for both text and recordings. WASM/embedding accepts memory-backed dependencies and
identical normalized semantics; browser memory budgets may reject huge roots with
explicit diagnostics. Do not introduce process execution, native plugin loading or
OS path assumptions into the SFZ device. Check the existing no-default-features WASM
build documented in `embedding.md`.

Version SFZ device state and the normalized schema; persist program identity/source
closure, overrides, seed and required controller state. Rebuild prepared buffers on
state load off-thread. Extend native CLAP preset/state validation and dependency
enumeration rather than smuggling absolute source paths into opaque strings. Test
host note-ID expression, CC, pitch bend, reset, offline render and state round trips.

Current `src/device_state.rs` packages referenced native assets into bounded,
hash-addressed `/muz-assets` storage; new SFZ source/dependency serialization must
extend that process and its validation. Prefer serializing validated normalized
program state plus rehomed sample handles; preserve original source hashes and
metadata without requiring callback-time reparsing. Test text/include resolution
through memory assets for direct imports as well.

DAWProject should use the existing muz CLAP wrapper for the new native device,
subject to its current asset packaging rules. Carry a complete versioned dependency
manifest and report nonportable or prohibited assets. Standard Guitar and METAL-GTX
terms prohibit redistribution; never embed their recordings or source data into
redistributable presets/archives by default. A linked-local-assets state/export mode
would be new work, requiring validation and an explicit portability warning; until that mode exists, refuse restricted-asset
embedding and offer an authorized rendered-audio workflow;
respect any current export refusal rather than claiming portable embedding works.
Test moved projects, missing roots, sample hashes, controller automation and unsupported
partial programs. MIDI export alone cannot preserve SFZ audio/control semantics;
report keyswitch/controller/per-note-expression loss accurately. WAV/stem render
uses the same processor, seed and state reconstruction as live/CLAP playback.

## Catalog, installers, licenses and backwards compatibility

Extend installers to offer `native-subset` (current behavior) and `sfz-complete`
profiles with explicit size estimates. Full profile installs original roots/fragments,
context metadata and every dependency at pinned versions. For Karoryfer expand the
per-file manifest at the existing revisions rather than following moving branches.
For archive packs preserve verified archives as provenance and generate reproducible
per-file dependency manifests from clean extraction. Never overwrite differing user
files silently; retain existing verification/replacement conventions and path checks.

Keep recordings, generated caches and restricted SFZ source out of git. Commit
catalogs, descriptors and provenance metadata only when licenses permit. CC0 covers
Virtuosity, Karoryfer and VSCO; Sonatina uses CC Sampling Plus 1.0, not CC0. Retain its
attribution/license and do not assert blanket redistribution permissions. Unreal has
publisher-specific terms, so retain the notice and original download path and perform
local import/cache only. Pack inspection must show source, installed profile, license,
complete/partial status and missing content. Avoid automatic multi-gigabyte downloads
just to inspect a catalog or compile an unrelated piece.

Introduce native builders for all stable Sonatina catalog IDs alongside `plugin.muz`;
keep plugin state helpers for existing projects and as a comparison path. Publish
complete Virtuosity, Shinyguitar, bass and Unreal catalogs. Context includes external
macro binding and any separately evidenced compatibility corrections, with hashes.
Do not change old `sample()`/patch selection, `sample_zone` annotation indices or
serialization. Existing guitar/bass/VSCO calibration and register recipes remain
unchanged. SFZ uses stable region IDs for its own debug pinning; current numeric
sample-zone annotations must fail clearly if applied to an SFZ device, unless a
separate explicit compatibility mapping is supplied. Add independent examples, never
maintainer compositions or their assets as fixtures.

## Dependency-ordered implementation milestones

Effort bands below are uncertain engineer-weeks for one experienced engineer, not
calendar commitments. The bands sum to roughly 9–21 engineer-weeks for this broader
full-library scope; some integration work can overlap, but runtime dependencies limit
parallelization. Corpus anomalies and reference-player gaps may dominate.
Decompose milestones into independently reviewable tasks after Phase 0.

| Phase | Work and dependencies | Acceptance gate | Rough effort |
| --- | --- | --- | --- |
| 0: freeze evidence | Clean source/archive verification; root catalogs; full include/macro/sample closure; exact opcode/value matrix; reference versions | Every scoped root classified; no invented VSCO source; complete Karoryfer gaps enumerated; Unreal source/provenance reconciled | 1–2 weeks |
| 1: syntax and program model | Tokenizer/preprocessor, registry defaults/inheritance, diagnostics, immutable IR, asset fingerprints; follows 0 | Synthetic grammar/path/scoping tests and golden normalized corpus summaries; all roots parse or have precise unresolved blockers | 1–3 weeks |
| 2: selection and lifecycle | Multilayer voices, controller/history state, sequence/random, switches, release/sustain/choke and limits; follows 1 | Exact selection/event traces for representative roots and boundary fixtures; deterministic block-size/seek tests | 2–4 weeks |
| 3: DSP and modulation | Envelopes/curves/fades/stereo, filters/EQ, numbered EG/LFO/variables/virtual controllers; follows typed IR and lifecycle | Every matrix sound-affecting opcode/value reachable in all roots implemented and tested; no "basic native" shortcuts called complete | 3–6 weeks |
| 4: host/state/resource integration | Native constructor, inspection, budgets, checkpoints, embedding/WASM, CLAP and DAWProject policy; begins after 1, finishes after 3 | Host/state/asset tests, measured realtime budgets, loss/portability diagnostics | 1–3 weeks |
| 5: pack migration and release | Complete manifests/profiles/catalogs, thin builders/docs; corpus comparisons and compatibility tests; follows 0–4 | All public roots prepare with complete installs, no unresolved behavior diagnostics, acceptance comparisons pass; legacy presets unchanged | 1–3 weeks |

Do not release an entire pack as complete because a simple root passes. During
implementation, report per-program status. Small milestones can deliver correctly
supported programs while the remaining scope stays explicit. Re-estimate after exact
opcode expansion and reference compatibility results, especially Unreal EGs and bass
cross-modulation. Shared runtime work should precede pack-specific workarounds.

## Tests and release acceptance

Use tiny synthetic WAV/FLAC and SFZ fixtures for automated engine tests. Generate
samples in tests or use redistributable fixtures; no copyrighted/restricted upstream
assets or personal pieces in tests. Public CI runs these synthetic cases; opt-in local
corpus jobs install assets through authorized sources, verify pins and emit summaries.

- Parser: CRLF/BOM/comments, unquoted paths with spaces, UTF-8/Japanese paths, macros
  in paths/includes, undefined symbols, repeated includes, cycles and traversal bounds;
  every scope reset and default; duplicate assignments and malformed values.
- Selection: exact inclusive key/velocity boundaries including 0/127, layered microphones,
  one shared random draw, sequence state across unrelated notes, keyswitch reset and
  held-note history, controller gate/fade transitions, silent choke regions.
- Lifecycle: overlapping same-key note IDs, pedal down/up, one-shot vs sustained loops,
  delayed voices canceled/released correctly, release trigger and duration decay,
  choke/group/note/global polyphony, release layers and expression after NoteOff.
- DSP: frame offset/end/loop endpoints and embedded loops, sample rates and channels,
  amplitude/velocity/curve units, dynamic envelopes, filter/EQ responses, width/pan,
  delayed/faded LFOs, numbered EG segments and variable/cross-mod routes.
- Integration: direct constructor, contrib discovery (checkout/user data/override),
  stale include/sample rejection, preparation cancellation and failed live edits,
  state schema migration, CLAP event/expression round trips and DAWProject asset policy.
- Determinism/performance: identical seeds and event logs across render block sizes,
  restarts/checkpoints/seeks; bounded memory and allocation instrumentation. Exact
  sample equality is expected within the same build/platform when event ordering
  and seed are fixed; document cross-platform floating-point differences.

Run full root preparation and graph checks for 557 SSO entries, eight Virtuosity,
one Shiny bank root, eleven bass roots, six Standard Guitar and five METAL-GTX roots.
Enumerate individual fragments in the dependency coverage, not as missing programs.
For every sound-affecting opcode/value combination, have at least one focused fixture
and a corpus trace where it is reachable. Exercise every keyswitch and controller
branch with boundary and midpoint values, plus coupled controls and note overlap.
Make tests prove behavior, not just that the parser accepts a spelling.

Use pinned sfizz as the first reference because muz already uses its plugin path;
record exact player version/build and supported features. Add ARIA/Sforzando or the
publisher's documented player for extensions sfizz does not implement, subject to
availability. Neither player is an unquestionable oracle: maintain a disagreement
ledger with source evidence and an explicit muz decision. Failure to reproduce an
extended behavior in sfizz is not permission to omit it. No required semantics can
remain unresolved at the full-support gate.

Compare identical dry settings, controls, key/velocity/event timing, tuning, sample
rate and samples; disable host gain/effects and align only known latency. For random
programs first compare selected sample/region traces or controlled deterministic
fixtures, then distributions; two unrelated player seeds will not null. Compare event
and release onset within one sample for deterministic fixtures; delays from coarse
reference scheduling require documented measured tolerance. For plain unmodulated
playback target gain within 0.1 dB and pitch within 1 cent. For envelope timing use
max(1 ms, 1% of segment duration); for filter sweeps target response within 1 dB over
the relevant passband, with separate resonance/phase inspection. These are starting
engineering thresholds to calibrate against known interpolation/filter differences,
not blanket corpus pass criteria. Long modulated, random and crossfade renders use
waveform/envelope/spectrum and listening comparisons; do not demand bitwise identity
between different DSP engines or accept a gross articulation mismatch behind an RMS
metric. Version accepted deviations and explain their musical effect.

Release requires all scoped roots complete, every dependency verified, no unclassified
sound-affecting token/value, no prohibited asset redistribution, legacy native behavior
unchanged, deterministic rendering evidence, host/state/resource gates passed, and
documented reference deviations. Documentation-only delivery of this plan requires
only diff review under AGENTS.md; no engine tests are implied by this commit.

## Risks and decisions to close with evidence

The largest risks are player dialects/virtual controllers, Unreal multistage envelopes,
bass cross-modulation, source typos and memory footprint. Resolve each in Phase 0/3
with minimal source cases, not patch-wide approximations. The recommended architecture
remains a dedicated native processor even if some extensions require new modulation
primitives. Evaluate an existing Rust parser only after it proves source maps, textual
includes, defaults and scoped corpus coverage; it must not dictate weaker semantics.

Known open evidence items are Standard Guitar archive revalidation, program-specific
sample closures and exact opcode values, compatibility spelling/virtual-source rules,
and the installed availability of a second reference player. These are explicit
milestone blockers to complete support, not blockers to committing this plan. VSCO's
lack of SFZ is a scope exception, not unresolved implementation work. If the user
later wants all VSCO upstream recordings exposed, plan that separately around its
existing native maps or a specifically approved SFZ source.
