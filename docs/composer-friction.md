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
