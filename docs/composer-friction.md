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

### A track's `gain` is silently ignored under an `.out` automation lane

- **Origin:** Glass Hearts v14, integration (A2).
- **Observed behavior:** `track(..., {gain: -6.5})` together with
  `automation("guitar.out", curve(...))` plays at the lane's values alone: the
  lane owns the fader, and the static `gain` has no effect anywhere in the
  render. `muz check` evaluates and renders without any diagnostic. The docs say
  `out` is "the track's `gain`, including automation", but nothing says that a
  lane replaces rather than offsets the static value, and the piece had several
  tracks (`guitar`, `guitar_heavy`, `lead`, `piano`, `bass`) carrying dead
  `gain` values next to their lanes.
- **Affected work:** the v14 foundation step re-levelled Jun's guitar by moving
  its `gain` from −11.5 to −6.5 and computing the `guitar.out` rides as offsets
  from it. Every section of that track came out 5 dB low; it took a full render,
  a soloed section render of v13 and v14 and a test song to find why.
- **Workaround:** put the whole fader value in the lane and drop `gain` from
  tracks that have an `.out` lane.
- **Desired behavior:** either a lane on `<track>.out` offsets the static `gain`
  (a trim plus a ride, as on a console), or setting both is a check warning
  naming the dead value. Document whichever rule holds next to `.out`.
