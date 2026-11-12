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

