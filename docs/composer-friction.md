# Composer friction

This is the canonical protocol and live log for unresolved engine and language
issues that can be fixed in muz. Read it before composing, revising a piece, or
changing the engine. Repository boundaries and the verification budget are in
[AGENTS.md](../AGENTS.md).

[Documentation index](README.md)

## What belongs here

Record defects and limitations in language semantics, synthesis, rendering,
diagnostics, and exposed primitives. Include issues that distort musical choices
or force repetitive editing, even when a workaround lets the piece proceed.
A valid workaround does not close a report.

External model quality, provider availability, skill tooling, and development
process reports do not belong here.

## Recording a report

Commit a report in the same commit as the piece or revision that exposed it.
For work without a piece, name the task and commit the report with that work.
Each entry includes:

- **Origin:** the piece, revision, or task that exposed the issue.
- **Observed behavior:** what muz does and the relevant conditions.
- **Affected work:** the musical decision or work the issue obstructs.
- **Workaround:** how the work proceeded, if possible.
- **Desired behavior:** the capability or correction needed.

Include a small reproduction when useful. Do not freeze a whole composition as
a test. A piece commit with no new friction needs no report; a problem found and
fixed within the same commit needs no artificial live entry.

## Choosing a resolution

Seek a general solution that expands the builtins/kernel as little as necessary
while fully resolving the reported problem. First ask whether existing language
operations and exposed data can express the solution clearly. Put reusable
policies and recipes in `std/`, particular musical choices in project source,
and demonstrations of useful techniques in examples.

When source cannot express the solution well, identify the missing general
capability. Prefer a small, composable primitive or better access to musical data
that enables a family of solutions over a builtin for one special case. Keep
shapes, selection rules, and overlap policies in source wherever practical.

Kernel changes remain appropriate for engine defects, runtime guarantees, and
capabilities that require them. Minimizing the kernel must not retain awkward
workarounds or move complexity into every composition.

In the fixing commit, explain why existing facilities suffice or why the new
primitive belongs in the kernel. Demonstrate the specific resolution in source.
For example, tag-derived automation uses exposed note data and timing operations
so composers can write their own automation recipes. This principle applies to
all friction items.

## Closing a report

Verify the reported use case, document usage in the relevant reference, and add
focused regression tests. Remove the entry in the same commit that resolves the
engine or language issue. If only part is fixed, keep the remaining problem
explicit.

Git history retains reports and fixes. This file has no resolved section or
remedy archive; do not maintain parallel per-piece friction logs.

## Open reports

### Arbitrary CLAP parameter lanes cannot carry per-note modulation scope

- **Origin:** PALCO modulation reopen v017, M03 feasibility investigation (2026-11-03); PM-serialized from the accepted research report.
- **Observed behavior:** Inspected current `src/audio/clap.rs` authored parameter lanes encode global CLAP `PARAM_VALUE` with note_id/port/channel/key=-1; no authored `PARAM_MOD` encoding is exposed. Surge XT 1.3.4 Pitch and Unison Detune descriptors advertise per-note modulation (flags 31776), but arbitrary parameter lanes cannot independently address overlapping notes. Standard per-note expression covers seven fixed controls and does not supply this mapping. Installed CLI probes establish global target validation, not per-note DSP behavior.
- **Affected work:** Exact reconstruction of graphs that require independent per-voice arbitrary plugin modulation is obstructed. Applicability to saved PALCO remains unresolved: its two recovered host modulators have polyphonic_mode=false, while full scope/source semantics still need calibration. This is not a claim that PALCO requires an engine change.
- **Workaround:** For proven global graphs, derive one prepared curve per parameter from final performed note data with explicit overlap/reset policy. Splitting notes into separate plugin instances changes shared voice/effect/state behavior and is not an established equivalent for per-note modulation.
- **Desired behavior:** Expose a small general scoped parameter-event mechanism with stable parameter identity, note identity, sample-offset timing, value-versus-modulation semantics and capability validation. Keep EG/waveform/overlap policies in source. No special PALCO builtin is requested; no engine implementation is authorized by this report.

### A controller ramp cannot end where the next controller point begins

- **Origin:** Glass Hearts (electronic/glass-hearts), 2026-11-08; CC1 dynamics for Sonatina sustain programs in sfizz.
- **Observed behavior:** Stacking a source-built CC1 ramp (`control(1, v, at = start + dur * i / steps)`) whose final point lands at `110bars` with a section step `control(1, 96, at = 110bars)` fails compilation: `conflicting CC 1 values on channel 0 at beat 440`. There is no ordering rule for simultaneous controller events in one stack.
- **Affected work:** Section-boundary dynamics are naturally written as "ramp into the boundary, new level at the boundary". Every such join needs a manual sub-beat offset (`110bars + 1/8b`), or the ramp must be shortened by hand, which is repetitive and easy to get wrong across many sections.
- **Workaround:** Offset the boundary point by `1/8b`.
- **Desired behavior:** A documented, deterministic resolution for equal-time controller values inside one stacked pattern (for example, later stack elements win), or a source helper in `std/` that joins ramp and step controller fragments while retaining the current error for genuinely ambiguous conflicts.

### `chord()` register is undocumented

- **Origin:** Glass Hearts (electronic/glass-hearts), 2026-11-08.
- **Observed behavior:** `chord("Em")` returns `[52, 55, 59]` (octave 3). The language guide only says `chord("F#m7")` "returns pitches"; the register and the octave convention for symbols with slash basses or extensions are not stated anywhere in the docs.
- **Affected work:** Register-dependent writing (piano voicings, bass roots that a patch sub-octaves) needed a `muz eval` probe before the first pattern was correct; an off-by-one octave sent the synth bass's sub-oscillator to C1.
- **Workaround:** Probe with `muz eval`, then offset in source.
- **Desired behavior:** Document the returned register in [language.md](language.md#harmony-and-pitch) (and ideally give `chord` an `octave` parameter like `scale`).

### Offline render is single-threaded

- **Origin:** Glass Hearts revision (electronic/glass-hearts), 2026-11-08; render and perf sample of `muz render`.
- **Observed behavior:** A 276 s song (34 expanded tracks, ~130 devices, 8 buses, Pianoteq, 8 sfizz instances, native voice_patch synths) renders on one thread at ~98% CPU while a 128-core machine is ~56% idle, and takes on the order of ten minutes. An 8 s cpu-clock sample showed `VoicePatch::process` 14%, libm `powf`/`expf`/`tanf`/`sinf`/`fmod` ~14% combined, `mix_into_bus` 3.7%, `Chorus::process` 3.4%, `Automation::value_at` 2.3%, sfizz ~10%. Code reading (not measured): `render_slice` walks tracks then buses serially (`src/audio/engine.rs`, ~996 and ~1075); no rayon or render-path threads exist; `mix_into_bus` calls `powf` and an automation lookup per sample (~1666); automated native devices that do not accept parameter offsets fall back to a per-sample `frames=1` loop (~1757); `VoicePatch` recomputes pitch, detune, envelope, velocity and pan math per voice per sample.
- **Affected work:** Every revision-and-listen cycle on a full arrangement; iteration on mix and arrangement decisions is limited by render time, and section/stem auditioning does not remove the cost for whole-piece checks.
- **Workaround:** Render sections or stems; wait.
- **Desired behavior:** First, hoist per-block-constant math in `mix_into_bus` and `VoicePatch` and avoid the per-sample fallback for automated native devices (bit-exact where only hoisted). Then an offline-only mode that renders independent tracks concurrently into per-track buffers and sums into buses in fixed track order (bit-identical output), respecting sidechain dependencies and keeping CLAP main-thread servicing on the main thread, with a thread-count option. Live playback unchanged. These are engine/runtime changes; no language surface is requested and no engine implementation is authorized by this report.
