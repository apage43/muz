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

### An oscillating expression curve fails on a CLAP instrument

- **Origin:** The Long Debt v1, stream F after plan revision 4 (adopting
  `std/gestures.vibrato_lfo` from a88cbc7 for the dropout's held A on Surge XT).
- **Observed behavior:** any note whose curve is an oscillating record, on a
  CLAP plugin track, makes `muz check` fail while the graph is prepared:
  `note expression slot 66 value 14.21 is outside 0..1` for
  `note("A5", 8b).express({tuning: {curve: 0, oscillate: {rate: 5Hz, depth: 0.2}}})`
  at 152 BPM (slot 64 for `volume`). The value is the rate times the note's
  performed length (5 Hz × 2.84 s), so a cycle count is validated as a
  normalized expression value.
- **Affected work:** the held notes that need a vibrato longer than the
  sampled-curve budget allows (the dropout's A, tag-1's 11.5-beat D5).
- **Workaround:** the sampled `vibrato` stays; the dropout's still starts at
  0.8 s and the hang's stops about 0.6 s before its release.
- **Desired behavior:** oscillating curves reach a CLAP instrument as
  documented, with a test on a plugin destination.
