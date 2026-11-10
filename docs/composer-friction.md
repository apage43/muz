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

### Pattern dumps in `muz eval` print note times as bare numbers

- **Origin:** Glass Hearts v13 lead phrasing (electronic/glass-hearts, `synth.muz` `sing`), 2026-11-10.
- **Observed behavior:** `muz eval` prints quantities reached through `.notes`, records, and lists with their unit (`"1b"`, `"0.75b"`). Evaluating a whole pattern (`muz eval` of `phrase("C4:q D4:e.")`) still prints its serialized form, where `at`, `duration`, and `span` are bare beat numbers.
- **Affected work:** Reading metric values off a pattern dump suggests scalar arithmetic that fails in callbacks.
- **Workaround:** Inspect `pattern.notes` rather than the pattern itself; documented in the language guide.
- **Desired behavior:** A pattern dump prints note times with units, without changing the serialization used elsewhere.

### Measuring one section or a few tracks costs a whole-song render

- **Origin:** Glass Hearts v13 mix pass (electronic/glass-hearts, A2 integration), 2026-11-10.
- **Observed behavior:** `muz render --section solos` processes every track from the song start to keep instrument and effect history, so on this 17-track, 138-bar piece one 30 s section took about 7.5 min, the same as the full master. `--tap` cannot be repeated (`the argument '--tap <TAP>' cannot be used multiple times`), and `muz stems` rejects `--solo` ("stems chooses its own taps"), so measuring three guitar tracks in one section means either three section renders or dry stems of all 17 tracks.
- **Affected work:** Level iteration against per-section loudness targets and pre-fader tone checks on a few tracks. Each check costs as much as a full render, so the mix converged in fewer, coarser steps than it needed.
- **Workaround:** Full renders measured per section with ffmpeg `ebur128`, and one `muz stems --section` pass for all tracks when several pre-fader taps were needed.
- **Desired behavior:** Repeatable `--tap` (several taps from one pass), `muz stems` limited to named tracks, and a way to shorten section history (for example, history only for tracks with long-lived state, or a bounded `--history SECONDS`).

### Plugin-chain tracks render serially and process through silence

- **Origin:** Glass Hearts v13 final render (electronic/glass-hearts), 2026-11-10.
- **Observed behavior:** `muz render --threads 32` on this 33-track graph ran on one core. After 12.5 min the render thread had 12:30 of CPU and the workers had almost none. perf put about 60% of the time in the five guitar tracks' NAM and OrbitCab CLAP chains: guitar, guitar_left, guitar_right, guitar_heavy and guitar_echo. This is documented behavior: tracks containing a CLAP plugin are pinned to the render thread (`pinned_to_render_thread`, docs/workflow.md "Parallel rendering"), and buses are summed serially. These chains also process the whole song, including tracks that play only a few bars; guitar_heavy is silent outside the bridge and build.
- **Affected work:** Every mix iteration on a guitar piece costs a full serial pass through all the amp sims, about 12–15 min per render here. Together with full-history section renders (see the previous report), that makes level and tone iteration slow enough to cut short.
- **Workaround:** Fewer, coarser mix iterations.
- **Desired behavior:** Run independent CLAP tracks in parallel, keeping main-thread callbacks on the render thread rather than the audio processing itself. Or skip or bypass a plugin chain while its input has been silent past a declared tail. Alternatively, a supported way to freeze an unchanged track's output between renders.
