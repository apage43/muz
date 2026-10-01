# Remaining native SFZ qualification work

The native importer, multilayer processor, DSP, constructor, bounded resources,
linked/embedded state, CLAP/DAWProject integration, exact-history seeks and reusable
loop checkpoints are implemented. Their permanent contracts are in
[Native SFZ instruments](sfz.md), [embedding](embedding.md),
[DAWProject](dawproject.md), and the [instrument guide](instruments.md).
Completed implementation proposals have been removed from this plan.

Full-library qualification is still in progress. Keep this file and its index entry
until every gate below passes; then move the final evidence and accepted dialect
decisions into permanent documentation and delete this file. Do not confuse parsing,
preparation, native execution or a reference comparison with the other stages.

## Scope

Qualify all 588 public roots: 557 Sonatina 4.0, eight Virtuosity Drums 0.925,
one Shinyguitar, eleven Black And Blue Basses, six Standard Guitar and five METAL-GTX.
Include their complete include/macro/sample closure, microphone/articulation,
release/noise and performance branches. Keep the existing curated native presets,
calibration and register choices. No additional sample formats are part of this work.
VSCO's pinned sample source contains no SFZ mapping; its existing native interface
remains supported, and finding a separate SFZ edition is outside this scope.

Pinned sources, complete manifests, named builders, installer profiles, narrow
source overlays and reproducible audit commands are documented in
[contrib/SFZ.md](../contrib/SFZ.md). The Standard Guitar archive has now been
independently downloaded and verified against its immutable SHA-256; that original
provenance blocker is closed. Complete Karoryfer installers preserve the native
subsets and add all original SFZ dependencies separately. No publisher mappings,
recordings, personal pieces or downloaded player binaries belong in git.

## Remaining dependency-ordered gates

| Gate | Required evidence |
| --- | --- |
| Final corpus preparation | Refresh every pack's canonical audit using the current importer/runtime. All 588 roots must have verified dependency hashes, no importer or runtime behavior errors, and successful complete decoding/preparation with declared resource limits. Include `.txt` mapping fragments, control defaults, labels, curves and exact effective opcode values. Restricted reports contain paths/hashes/metadata only. |
| Reachable native behavior | Exercise keys/velocity boundaries, every authored keyswitch and physical controller boundary/midpoint, coupled controls, overlap, sustain and release branches. Record actual selection/voice counters and finite output; retain synthetic tests for the semantic families rather than using copyrighted patches as test fixtures. A accepted opcode spelling alone is insufficient. |
| Resource/performance | Record actual decoded footprints and representative loading/render costs for large roots and layered/long-tail workloads. Prove callback allocation/deallocation is zero, memory/voice/event capacities remain bounded, drops/steals are inspectable and preparation cancellation works. Named builders must provide adequate explicit sample budgets; large graphs retain explicit host budget diagnostics. |
| Reference extensions | sfizz covers common behavior but rejects ARIA variable and cross-LFO routes. Validate those routes and Unreal out-of-range EG behavior against the official native Sforzando reference, recording exact version/build, controls, events and player limitations. Follow the user's approved installation scope and obtain actual-term acceptance before installation or execution where required. No decompilation/disassembly or binary modification. |
| Measured DSP agreement | Complete focused gain/pitch/envelope/filter/EQ measurements and representative dry library articulation comparisons. Classify every difference as a defect to fix, a reference limitation, or a documented intentional dialect decision with its musical effect. Preserve raw metrics; do not hide differences behind fitted gain, alignment or a blanket RMS pass. |
| Final host/regression release | Complete the workspace tests, CLAP SFZ state/CC/note-ID expression and live transport tests, no-default-feature core and WASM compilation, synthetic packaging/license/resource tests, and legacy native regressions. Commit the resulting permanent documentation and final audit evidence, remove this plan/index entry only after all gates close, push main and verify remote/check status. |

## Comparison rules

Use generated synthetic WAV/FLAC and SFZ fixtures for engine regressions. Local
corpus jobs install through authorized sources and verify immutable pins. Reference
jobs record binary hashes and identical dry settings, controls, key/velocity/event
timing, tuning, sample rates and recordings. Compare event/selection traces first;
independent random-player seeds need distribution tests rather than waveform nulls.

Within one build/platform, fixed seeds and event order must produce identical PCM
across block partitions, repeat resets and prepared history/checkpoint restores.
Different engines need calibrated musical tolerances: event/release onset within
one sample for deterministic fixtures; plain playback gain within 0.1 dB and pitch
within one cent; envelope timing within max(1 ms, 1% of duration); relevant filter
passband response within 1 dB, with resonance and phase inspected separately.
These are starting thresholds, not permission to overlook articulation differences.
Explicitly document measured reference scheduling/interpolation deviations.

Already measured sfizz limitations include a first-callback repeated source sample,
a forced minimum 1 ms loop crossfade, and differing sine/envelope approximations.
Keep raw comparison data beside any separately named diagnostic adjustment.
Normative equations and analytic tests for variables/cross-modulation are implemented;
those do not replace the remaining independent empirical extension comparison.

## Completion rule

Release requires every scoped root complete, every dependency verified, no
unclassified sound-affecting behavior/value, no prohibited asset redistribution,
legacy native behavior preserved, bounded/deterministic execution, host/state gates
passed and explicit reference deviations. Continue tested commits directly to main,
removing the corresponding completed planning material each time. Remaining effort
is uncertain and depends on reference differences and corpus findings; the original
9–21 engineer-week planning estimate was not a calendar promise.
