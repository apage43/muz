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

### An SFZ instrument's random draws depend on every earlier note event

- **Origin:** Glass Hearts v17 (muz-projects), moving Jun's Standard Guitar lead
  from written keyswitch notes to per-note `articulation` data.
- **Observed behavior:** an `sfz()` instrument draws its random value
  (`offset_random`, `amp_random`, `delay_random`, `pitch_random`, `lorand`/`hirand`)
  from one per-instance sequence, advanced once at every note-on and once at every
  release, whether or not any region matches. A keyswitch note consumes two draws.
  Removing the 121 switch notes left every sounding note's MIDI and its sample
  choice unchanged, but each note after the first switch received a different
  draw: the lead stem nulls against v16 only to about −19 dB from bar 8 on. With
  the program's random opcodes set to zero (and its release layers removed, which
  per-note articulations select by design) the two renders are bit-identical.
- **Affected work:** a refactor that does not change a played note cannot be
  proved by a null test, and any added or removed note (a switch, a ghost, a rake)
  re-rolls the random variation of every later note on that track, so an edit in
  bar 8 changes how bar 129 sounds. The same kind of whole-track re-roll as the
  v15 humanize re-roll, but inside the instrument.
- **Workaround:** proved equivalence on a scratch copy of the program with the
  random opcodes zeroed; accepted the re-roll in the piece.
- **Desired behavior:** random draws that are stable under edits elsewhere, for
  example derived from the instrument seed and the note's own identity (onset
  time, key, voice index) rather than from a running count of events, or at least
  no draws for events that trigger no region.
