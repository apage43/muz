# DAWProject compile target and Muz CLAP plugins

Status: implementation and bounded Bitwig probes completed in part, 2026-09-26;
production-graph verification remains open. This is the implementation plan,
not a statement that every capability exists. Maintain the checklist while
working, then delete this file in a later commit only when all completion criteria
are met and enduring contracts have moved into reference documentation.

## Product contract

Compile a muz project into an editable DAWProject handoff with the greatest
practical fidelity. The DAW owns notes, arrangement, mixer structure and exposed
automation. Muz Instrument and Muz FX CLAP plugins preserve native muz DSP using
the same processors as the renderer. Their state must contain usable muz code,
not only an opaque compiled object. Original third-party plugins remain original
plugin instances wherever possible.

Every unsupported, approximated, omitted or unverified semantic must produce a
warning identifying the affected object and actual exported behavior. Successful
file creation never implies a lossless export. A missing mapping must not silently
fall through. Fatal errors prevent publication of the output archive.

Initial interoperability target: Bitwig on Linux, as proposed in the supplied
feasibility report. Keep the archive standards-compliant and distinguish portable
format support from behavior verified in a particular host/version. This is a
one-way handoff; source reconstruction from DAW edits, native DAW substitutes for
muz DSP, and automatic whole-song/stem baking are outside initial scope.

## Starting points and evidence

The supplied `muz-dawproject-feasibility-report.md` motivates the design; its
recommendations are background rather than implementation instructions. Relevant
current code boundaries are:

- `src/compile.rs`: `Compiled` carries the session, score, diagnostics and source
  locations. Export after performance processing and physical track expansion.
- `src/model.rs`, `src/midi.rs`, `src/description.rs`: session graph, performed
  streams, routes, groups, sections, automation and parameter specifications.
  The small `model::Pattern::Note` alone is insufficient for rich performance.
- `src/audio/device.rs`: `DeviceProcessor`, timestamped events, parameter updates,
  transport, detector input, reset, latency and tail information. This is a reuse
  boundary, not yet a complete CLAP implementation.
- `src/audio/automation.rs`, `src/audio/transport.rs`, `src/audio/engine.rs`:
  interpolation, scheduling and routing semantics to preserve.
- `src/patch_source.rs`, `src/patch_description.rs`, `src/assets.rs`: device
  lowering, validation and asset resolution to reuse for code-backed state.
- `src/audio/clap.rs` and `src/audio/vst3*.rs`: existing plugin **hosting** and state
  support. New CLAP plugin entry points are separate from these host adapters.
- `Cargo.toml`: portable core already builds without `desktop`; plugin packaging
  must not pull in PipeWire, file watching or desktop playback by default.

Upstream references checked during planning:
[DAWProject overview](https://github.com/bitwig/dawproject),
[project schema](https://github.com/bitwig/dawproject/blob/main/Project.xsd),
[CLAP API](https://github.com/free-audio/clap),
[state](https://github.com/free-audio/clap/blob/main/include/clap/ext/state.h),
[parameters](https://github.com/free-audio/clap/blob/main/include/clap/ext/params.h),
[audio ports](https://github.com/free-audio/clap/blob/main/include/clap/ext/audio-ports.h).
Implementation baselines (resolved 2026-09-26): DAWProject
`ee4dcdde75940f30e14e55401a26955a58b8322b` (format 1.0) and CLAP
`a47f6badb49d948fd009998f28309cdab78979c9`. Moving `main` links above
are discovery links, not a compatibility baseline. The first host target installed
for verification is Bitwig Studio 6.0.11, revision 160070
(`f2730b10e641fdf2e4ae82140089d5f6550ca3b7`); each tested behavior still
needs a recorded import/edit/reopen observation before it is called verified.

DAWProject provides plugin state, parameter references, notes, expression timelines
and mixer routing. Its channel/send destinations do not establish a portable
connection to an arbitrary plugin auxiliary port. Exposing a CLAP detector port
therefore does not by itself solve imported sidechains. Prove the chosen import
path before claiming support.

## Architecture and interfaces

Use one evaluated compilation as input to a target-specific export planner, then
serialize its validated result. Do not compile via MIDI files or translate source
syntax independently. Keep file writing separate from capability analysis:

```text
source -> compiler/performance -> validated session
                               -> export plan + fidelity report
                               -> XML, plugin states, assets -> .dawproject

Muz Instrument / Muz FX -> shared state loader -> shared muz DSP
```

Start with `src/dawproject/` for the planner/writer and a separate CLAP `cdylib`
crate, with two stable plugin descriptor IDs (Instrument and FX). Share a versioned
state/parameter module between exporter and plugins. Expand core APIs only where
existing data and operations cannot express the requirement; avoid a speculative
universal compiler framework or new composition builtins for export policies.

Proposed CLI:

```sh
muz export song.muz --format dawproject -o song.dawproject
muz export song.muz --format dawproject -o song.dawproject --strict
```

Add a machine-readable report option and explicit destination profile. Default to
preserving DSP and exposing as much editable structure as verified semantics allow.
Strict mode rejects any fidelity warning, including unverified capabilities, before
publishing an archive; retain its diagnostics. Ordinary mode may emit a partial
handoff only with explicit warnings and a report explaining every loss. Missing
required assets, invalid state or an invalid graph are errors, not partial success.

Each report entry needs a stable code, severity, source location where available,
logical and physical object paths, affected feature, intended semantics, exported
representation, fidelity outcome and practical remedy. Outcomes include preserved,
approximated, omitted, manual setup required and unverified. Surface all non-preserved
outcomes as warnings on the CLI and retain human/JSON reports alongside the archive.
Add archive comments or report copies only where permitted by the pinned format;
never rely on a DAW displaying those comments. Include target/profile versions,
required plugin IDs and external dependencies. No blanket warning substitutes for
object-level diagnostics.

## Representation and fidelity decisions

| Feature | Planned representation and required checks |
| --- | --- |
| Performed notes | Editable clips from the delivered performance, preserving offsets, gates, fractional tuning, attack/release intensity, overlap and event order. Never use quantized SMF as an intermediate. |
| Arrangement | Tempo map, meter, section markers, useful clip boundaries and physical track groups. Do not split sustained notes at markers just to create clips. Distinguish authored organization from audible behavior in loss reports. |
| Native instruments | Muz Instrument for presets, voice patches, samplers and sample zones, driven by host notes. Preserve voice limits, stealing, lifetime, one-shot and choke semantics or warn. |
| Native processing | Muz FX for individual processors and coupled racks; preserve internal parallel paths, modulation and latency. No substitution based only on similarly named effect parameters. |
| Mixer | Actual tracks, buses, output routes, sends and master chain. Check pan law, gain units, tap order and compensation; use a small Muz FX stage for incompatible gain/pan behavior when possible. |
| External plugins | Original CLAP/VST3 identity, effective state after overrides, parameter IDs/units and automation. Explicitly report missing plugins and unverified dependencies. |
| Automation | Map target identity and time basis; prove interpolation and value-domain equivalence. Bound approximation error and point count, warn for every sampled approximation. |
| Expression/controllers | Preserve note identity, channel controls, pitch, pedals, pressure and supported expression. Warn on custom controls or messages without an equivalent; never collapse overlapping note expression to a track lane. |
| Assets | Portable, content-identified dependencies with no implicit source-machine paths. External dependencies remain explicit and diagnosable. |
| Sidechains | Expose required CLAP auxiliary inputs, but separately verify each DAWProject connection and exact detector tap/alignment. Otherwise emit precise manual reconnection instructions and warn about the resulting sound until connected. |

Inventory every current `DeviceKind`, event kind, transport mode, route and
automation target. Exhaustive matching or equivalent coverage must ensure newly
added engine features require an export decision. Unknown future fields must not
be accepted as preserved.

Automation needs particular care: muz's smooth curve uses smoothstep and runtime
interpolation occurs in time/value domains that may differ from a DAW's. Matching
the word “linear” is insufficient after nonlinear unit conversion or tempo changes.
Keep audio-rate/internal rack modulation inside DSP. Do not hide ordinary song
automation in plugin state, where DAW edits would stop controlling the arrangement.
Record host automation delivery limitations separately from format expressibility.

Sampler zone pinning and round-robin selection cannot assume the host preserves
muz note IDs. Prototype a portable, editable encoding first; test moving, duplicating
and inserting notes. If unavailable, report the exact loss. A hidden absolute-time
lookup or whole-song sequencer is not a faithful solution to editable notes.
Apply the same rule to custom per-note controls and cross-track kit chokes.

## Code-backed plugin state and runtime

Define an intentional compatibility format, not serialization of arbitrary Rust
structs. It contains schema/engine compatibility versions, role, executable source
modules and entry point, dependency versions/content identities, validated evaluated
device data, parameter identity table/current values, port layout and assets.
Capture required standard-library dependencies as well as project imports.

Provide a minimal device-source entry contract using existing language values and
lowering where possible. Exported state may contain a generated self-contained muz
device module, clearly distinguished from original source retained for provenance.
It must actually reconstruct the device. Source imports, closures and selected
devices must not depend on absolute paths or running a hidden song.

Restore compatible evaluated state deterministically; explicit source replacement
recompiles off the audio thread and updates that state atomically. Define which
representation is authoritative at each step, migrations, unsupported-version
errors, and how current host parameter values survive recompilation. Expose source
replacement initially through documented tooling/state loading; an in-plugin code
editor is not required. Do not silently recompile old state under changed semantics.

Use persistent parameter paths and a stored collision-checked numeric ID table.
IDs must survive unrelated parameter insertion/reordering, save/reopen and instance
duplication. Rename/removal needs an explicit identity/migration policy; never reuse
an ID for a different control. Resolve the CLAP unsigned-ID versus DAWProject
signed parameter-ID representation in the interoperability spike. Reuse native
parameter ranges, units and defaults; structural controls must not masquerade as
realtime automatable parameters.

Implement CLAP lifecycle, state, params, note/audio ports, transport, latency, tail
and offline-render behavior. Handle note identity/wildcards, MIDI fallback,
note-off/choke, overlapping same-key voices and sample-offset parameter events.
Native processors do not all accept queued offsets: split blocks at events or
provide a shared adapter without changing DSP timing. Preserve the expression
control clock across host block boundaries.

Source compilation, asset decoding, processor construction and state I/O stay off
the audio callback. Bound buffers/events, contain failures at the ABI boundary,
handle short state stream reads/writes, and make overflow visible instead of
silently dropping events. State/port/parameter changes use host restart/rescan
contracts. Seek, stop and loop behavior must not invent missing effect history;
document and warn about differences that cannot be reconstructed from host events.

Portable asset packaging is an early acceptance gate: an imported plugin cannot
assume it can resolve a sibling ZIP entry from a raw state stream. Initially embed
required sample bytes in self-contained state with explicit resource bounds, or
prove a supported relocation mechanism before using shared external assets. Test
restore after moving the export and without the source checkout. Third-party state
may retain private file references; do not claim to relocate opaque dependencies.

## Implementation sequence

Each phase ends in a reviewable commit with focused verification. Update this plan
as decisions are established; report actual engine/language friction in
`composer-friction.md` under its existing protocol, not in a parallel issue log.

- [x] **1. Capability and interoperability spike.** Pin format/API revisions;
  inventory source semantics; define report schema and target profile. Use small
  synthetic probes for CLAP state loading, stable automation IDs, auxiliary ports,
  expression and asset restoration in Bitwig. Record exact host version and
  observed support. Resolve state packaging and parameter enumeration on restore
  before building the full exporter. Schema validity alone is not acceptance.
  Completed as a bounded capability spike: revisions, profile, report schema and
  preset framing are established; observations and remaining limitations are
  recorded below rather than treated as full feature verification.
  Bitwig loaded native state and played an imported cutoff automation lane.
  Exact auxiliary routing semantics and expression playback remain open. In final15, the
  relocated combined archive produced a nonzero solo sampled-track offline WAV
  with all six instances loaded and no sample/state errors. This verifies embedded
  sample playback after relocation, not pinned-zone equivalence or DSP parity.
  Native fixtures also cover restoration without the original sample files.
  Final16 showed pitch/timbre data in the glass note inspector and a rising
  internal line; the attempted drag left its start unchanged. Expression playback
  and attachment after moving that note remain unverified.
  Final18 opened the compressor's auxiliary-input panel, selected kick POST and
  ran playback. Final19 paired offline bounces with No input versus kick POST
  changed consistently with sidechain response. The practical host selection path
  and an audio response are established; exact Muz tap/timing and DSP parity are not.
- [x] **2. Shared device state and runtime boundary.** Implement versioned
  source/evaluated state, standalone device loading, parameter identities and
  asset resolver. Add only the core extraction needed for reuse. Verify state
  migration/rejection, source reconstruction and stable IDs with small fixtures.
  Implemented: state version 3, standalone source reconstruction, bounded embedded
  assets, persistent parameter identities and explicit source replacement.
  Focused fixtures cover version/corruption rejection, source replacement,
  retained values/IDs and restoration without the original sample files.
- [x] **3. First editable vertical slice.** Build/install Muz Instrument, add
  compiler-target CLI, planner diagnostics and XML/ZIP writer. Export one native
  instrument and editable performed notes. Open in Bitwig, move a note, duplicate
  the track, save/reopen and bounce; confirm instances are independent. Unsupported
  features already warn rather than disappearing during this limited phase.
  Completed: import, note movement, track duplication, save/reopen and offline
  export succeeded in Bitwig. In final14, the original lead retained its 1800 Hz
  cutoff while the duplicate retained an edited 7494.3 Hz cutoff after save/reopen;
  both plugin instances loaded. This verifies independent parameter state for
  the tested duplicate pair.
- [ ] **4. Native production and routing.** Add Muz FX, native instrument families,
  racks, buses, master, gain/pan and sends. Implement verified auxiliary-port
  behavior and explicit unsupported-routing diagnostics. Verify DSP parity with
  synthetic impulses/notes and a small routing graph, including parallel latency.
  Progress: both CLAP roles, native device state, racks, buses, sends and master
  mappings exist. Native harness probes verify detector-driven gain reduction and
  exact rack impulse parity with 240 samples of parallel-path latency.
  Bitwig loaded all six instances in the combined graph and showed activity
  on four tracks and Master. Final17 showed the room FX track with Muz routed to
  Master and a keys send named room. Exact -12 dB/post-send behavior, audible
  bus/send routing and host parallel timing remain unverified. Final18 selected
  kick POST in the compressor's auxiliary-input panel and ran playback. Final19
  paired Master bounces with No input versus kick POST changed consistently with
  sidechain response; exact Muz detector tap/timing and DSP parity remain unproved.
  Manual reconnection is the diagnosed fallback for the missing archive connection.
  Final20 imported the structure fixture with the keys-to-room send and room FX
  bus. Its enabled-bus Project Master export produced nonzero PCM: 2 seconds,
  48 kHz stereo 24-bit, 96000 frames, peak 464796 and RMS 68750 in signed 24-bit
  units. The isolated Xvfb window was unavailable through the UI controller for
  the second, bus-muted condition. This does not isolate the bus/send contribution
  or establish the -12 dB level or post tap. Real profile/CLAP/Projects manifests
  matched after correcting the comparator.
  Keep this phase open for audible bus/send routing and host parallel timing: these
  are supported mappings awaiting verification, not established format limits with
  a specified fallback.
- [ ] **5. Automation and expressive performance.** Add tempo/meter, groups,
  sections, device/mixer/send automation, controllers, expression and sampler
  selection policies. Establish bounded approximation rules and warning coverage.
  Include the difficult combined probe: sidechain, parallel rack, custom expression,
  pinned sample zones, smooth curves, tempo change and tails.
  Progress: the combined synthetic archive imports and reports its specific
  omissions, approximations and manual setup. Smooth plugin curves have a bounded
  normalized-value approximation. In final13, Bitwig displayed the 120-to-90 BPM
  change during playback, retained a sampled-track gain edit from 0 to 24 dB after
  save/reopen, and completed two offline 24-bit WAV exports. Expression fidelity,
  exact tail handling and sidechain audio equivalence remain open. Final18 later
  established manual source selection; final19 paired bounces then showed a change
  consistent with sidechain response, without proving exact Muz tap/timing or DSP
  parity.
  This probe contained
  no groups, sections, controller lanes, buses or sends. The separate final17
  structure probe showed drums with hat/open_hat children, room FX, a keys-to-room
  send and an End marker. Intro/Turn labels and CC64 lane/playback remain open.
  Final16 showed Pitch 0.06 and Timbre 0.95% on the selected glass note, matching
  XML that contains three pitch and three timbre points within it; a rising
  internal line was visible. The drag did not change Start from 1.2.1.00, so
  expression attachment after movement and expression playback remain open.
- [x] **6. External plugin handoff.** Snapshot effective CLAP/VST3 state after
  overrides using existing adapters, write the format-required state containers,
  and preserve plugin identity and automation units. Verify with a small locally
  available plugin case; missing or untested capabilities remain explicit warnings.
  Mixed native/external racks need a deliberate decomposition or warning, not
  accidental nested hosting in the portable wrappers.
  Implemented: effective state snapshots and DAWProject preset containers, with
  focused restore checks. Bitwig imported ZamEQ2 CLAP with its overridden value.
  VST3 host import remains explicitly unverified; controller-private state and
  unsupported parameter identities are diagnosed. Mixed external racks are
  rejected from native wrapper state and reported.
- [ ] **7. Delivery and contract documentation.** Ship reproducible Linux CLAP
  build/install instructions and exporter reference docs, capability matrix,
  warning examples and troubleshooting. Validate the complete handoff, strict
  behavior and missing-dependency failures. Move durable state/API contracts into
  reference docs. Delete this plan in the completion commit only after every
  required deliverable is implemented or has its specified diagnosed fallback.
  Progress: installation, state/source tooling, capability matrix, warning examples
  and troubleshooting are in `docs/dawproject.md`; strict rejection preserves an
  existing archive. The combined project now has a recorded parameter edit,
  save/reopen and two offline exports. Final14 established independent duplicate
  parameter state across save/reopen. Final15 verified isolated embedded-sample
  playback from the relocated archive: a solo first-bar 24-bit WAV export at
  48 kHz stereo contained 112001 frames and 9592 nonzero PCM samples (peak 917165
  in signed 24-bit units; first nonzero sample index 2). All six instances loaded
  without sample/state errors. Final18 established the manual Bitwig detector
  source-selection procedure, now documented in the reference. Final19 compared
  Project Master bounces over 1.1.1–2.1.1, both 48 kHz stereo 24-bit without dither,
  with 112001 frames each. No input measured peak -6.864 dBFS / RMS -20.633 dBFS;
  kick POST measured peak -8.566 dBFS / RMS -20.918 dBFS. 157902 of 224002 PCM
  samples differed, consistent with sidechain response. Pinned-zone equivalence,
  DSP parity, exact Muz detector tap/timing and remaining
  arrangement/routing semantics are not established.
  Final16/17 add visible note-expression and group/FX/send/End-marker evidence;
  they do not establish expression attachment after movement, controller playback
  or exact send level/tap. Keep
  this plan until those acceptance
  gates are completed or explicitly resolved under the product contract.

## Verification and completion

Use small synthetic engine fixtures, never production pieces as regression tests.
Cover archive schema/references, deterministic IDs and report ordering, warning
coverage, atomic failure, state corruption/version rejection, relocated assets,
source reload and parameter compatibility. Strict mode must reject every degraded
fixture and leave any existing destination intact.

Use a small CLAP host harness for wrapper parity with shared native DSP across
relevant block boundaries, rates, notes/expression, parameter events, restart,
transport, latency and tails. Exact comparison is appropriate for deterministic
identical event streams; set justified tolerances where host scheduling differs.
Check realtime allocation constraints where the new wrapper introduces code.

Perform bounded Bitwig import/edit/save/reopen/offline-bounce checks for the vertical
slice, production graph and difficult probe. Record what was exercised, including
host version and unresolved fidelity outcomes. Do not promise universal DAW support
or sample-identical arbitrary plugin bounces. Follow the repository verification
budget: successful ordinary media operations need no repeated decoding/analysis;
investigate only concrete differences with the smallest relevant check.

Completion requires both working CLAP roles with code-backed portable state,
editable notes, implemented mappings for the current semantic inventory, mandatory
specific warnings for all remaining losses, strict rejection, documented installation
and demonstrated first-target handoff. Known format limits may remain with explicit
fallbacks; missing required plugin/runtime infrastructure is not a completed plan.
