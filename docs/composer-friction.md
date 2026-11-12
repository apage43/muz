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

### The jet's `offset` has an unstated safe range, and the low flute lacks a second partial

- **Origin:** Arundo v2 flute voicing
  (`muz-projects/piano-and-chamber/arundo/winds.muz`).
- **Observed behavior:** `std/physical.flue` with `offset: 0.8` does not speak
  at pitch from C4 to F#5: it sounds about 1890 cents low, with odd partials
  above the fundamental. `offset: 0.5` is fine. At 0.5 the low register's
  second partial is about 12 dB under the fundamental, where recorded flutes
  measure -4 to +6 dB from C4 to A4.
- **Affected work:** the flute's low register, whose character is a strong
  second partial.
- **Workaround:** `offset: 0.5` and an output boost above the fundamental, by
  register.
- **Desired behavior:** the docs give the range of `offset` in which the jet
  speaks at pitch, and the model reaches a recorded flute's second partial at
  the bottom of its range from inside the loop.
- **Repro:** `ph.flue(0.92, f, {offset: 0.8})` at MIDI 60 to 78.

### A modeled flute is loudest at the bottom, and its low notes barely brighten

- **Origin:** Arundo v2 flute voicing.
- **Observed behavior:** for the same mouth pressure, `std/physical.flue` is
  about 14 dB louder at C4 than at F#6; a flute is quieter at the bottom. At C4
  the spectral centroid moves only from 275 to 345 Hz from pianissimo to
  fortissimo.
- **Affected work:** balancing a flute line across registers, and dynamics in
  the low register, which change level but hardly colour.
- **Workaround:** a gain table over `log2` of the frequency, and an output
  lowpass that opens with the dynamic.
- **Desired behavior:** the docs say how the jet's level and brightness depend
  on register, and the model or the `flute` recipe gives a flute's balance
  across its range without a table in every piece.

