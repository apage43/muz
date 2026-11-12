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

### A kit's MIDI channel changes what its noise voices render

- **Origin:** Deadline at Dry Gulch v1, plan probes
  (`muz-projects/games/dry-gulch/probe/probe-channels.muz`).
- **Observed behavior:** adding `.channel(9)` to the pattern on a native kit
  track whose voices use `s.noise()` changes the render: the result is no
  longer bit-identical, and the difference between the two renders peaks at
  −15.8 dBFS (the noise is a different realisation). `.channel(1)` or
  `.channel(2)` on `std/instrument.mono` pulse and triangle tracks leaves the
  render bit-identical.
- **Affected work:** a hand-off that wants the drum track on MIDI channel 10
  cannot set it in source without changing the reference audio, so a "the
  notes did not change" proof by nulling fails.
- **Workaround:** set channels after export (`tools/finish_midi.py`).
- **Desired behavior:** a note's MIDI channel does not reseed a voice's noise,
  or the docs say that channel is part of the noise seed and how to pin it.

### `1bars` ignores the song's meter, and `require_span` doesn't say what it got

- **Origin:** Deadline at Dry Gulch v1, `muz-projects/games/dry-gulch/cues/crows-roost.muz`
  (12/8).
- **Observed behavior:** `drums({...}, span = 1bars)` in a cue whose song is
  in 12/8 makes a pattern four quarter-beats long: `1bars` is a fixed 4/4 bar
  where the pattern is built. The 24-step lane is squeezed into four beats.
  The only report comes later from the cue builder:
  `<std/arrange>:101:5: pattern does not have the required span`, with no
  actual span, no expected span and no caller.
- **Affected work:** every drum lane and whole-bar rest in a compound-meter
  piece.
- **Workaround:** `span = 12 * 1/2b`, or `a.bars(1, meter)` from
  `std/arrange`.
- **Desired behavior:** `require_span`'s failure names the actual and expected
  spans; and the `bars` unit is documented as a 4/4 bar beside a pointer to
  `std/arrange.bars(n, meter)`, or warns in a song that is not in 4/4.

### The modeled flute has almost no even harmonics

- **Origin:** Arundo v1 (`muz-projects/piano-and-chamber/arundo/winds.muz`),
  and `std/physical.flute` measured for the woodwind modeling task.
- **Observed behavior:** `std/physical.flue` sounds a second harmonic 17 to 35
  dB under the fundamental from C4 to C7 (a flute's is about 5 to 12 dB under).
  The jet path is half a period long and inverting, so at twice the note the
  loop is negative feedback, and the saturated jet settles into a half-wave
  symmetric pulse. An offset on the jet large enough to break the symmetry
  stops the model from starting. Mixing the jet's own flow into the output
  does not help (it is as symmetric as the bore), and a shorter jet followed
  by a lag did not speak at pitch.
- **Affected work:** the flute's low register sounds hollow, closer to a
  stopped pipe than a flute.
- **Workaround:** a stage after the bore, `x + 1.2 * x^2` and a highpass, which
  the piece labels as not part of the waveguide.
- **Desired behavior:** a jet model in `std/physical` whose flow pulse is
  lopsided at playing pressures and that still starts from breath noise.
  This needs no kernel change that is known: it is a modeling problem in
  source.

### A mixed time and phase expression curve fails on short notes

- **Origin:** Arundo v1, `muz-projects/piano-and-chamber/arundo/playing.muz`
  (one breath curve rule for every note length).
- **Observed behavior:** `[[0ms, a], [20ms, b], [70ms, c], [0.5, d], [1, e]]`
  is an error on any note shorter than 140 ms, where 70 ms falls after phase
  0.5 ("falls at phase ..., not after point ..."). The rule is documented and
  the diagnostic is exact, but the curve cannot be written once for a score
  with 90 ms staccato notes and 12 s held ones.
- **Affected work:** tonguing shapes: a physical 20 to 70 ms onset, then the
  phrase's dynamic by phase.
- **Workaround:** the recipe computes each note's performed length with
  `seconds_at` and writes phases only on notes under 260 ms.
- **Desired behavior:** a way to say which point gives way: a phase point
  overtaken by an earlier time point is dropped, or a position such as "70 ms
  or phase 0.3, whichever comes first".

### A voice patch's internal signals cannot be rendered

- **Origin:** Arundo v1, `muz-projects/piano-and-chamber/arundo/probe/`
  (calibrating speaking thresholds, registers and loop gain).
- **Observed behavior:** only a patch's `output` reaches a render. Measuring
  the mouthpiece pressure or the reed's flow of a waveguide means building the
  patch again with a different `output` for each signal.
- **Affected work:** calibration and diagnosis of any feedback patch, where
  the output alone does not say why a note failed to speak.
- **Workaround:** a mono patch can carry one probe as the other channel of a
  stereo `output: {left: sound, right: probe}` and read it from a stem; that
  is one signal at a time and changes the patch.
- **Desired behavior:** named probe signals on a `voice_patch` that a render
  or stem can capture without changing what the track sounds like.

### A modeled reed's second register is an accident, not a fingering

- **Origin:** Arundo v1 (`muz-projects/piano-and-chamber/arundo/winds.muz`).
- **Observed behavior:** `std/physical` has no register vent. A cone's attack
  lands in the octave once the mouth pressure passes a limit (about 0.62 of the
  closing pressure from a tongued start at `apex` 0.27), and a cylinder never
  reaches its twelfth on the long tube: every clarion note is a short tube.
- **Affected work:** the clarinet's break and its clarion colour, a slurred
  twelfth on one fingering, an oboe's octave key; and the top of a cone's
  dynamic range, which has to be capped to stay in the low register.
- **Workaround:** cap the pressure, and play upper notes as shorter tubes.
- **Desired behavior:** a vented bore in `std/physical`: a third line and a
  junction part-way along the tube (for a clarinet at a third of it, a one-pole
  highpass in transmission and a negative one-pole lowpass in reflection), opened
  by a per-note control. The existing `feedback` lines can carry it; this is
  a recipe, not a kernel change.

