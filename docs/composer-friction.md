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

### The `is_entry()` self-test pattern and `muz eval FILE --expr` run it twice

- **Origin:** Main theme from KITE, v1 foundation, `material.muz`'s self-test.
- **Observed behavior:** `docs/language.md` recommends `let checked = if
  is_entry() { self_test(part) } else { true };`, while the projects' helpers
  (`untitled-ruins/gtr.muz`, citing the evaluation budget) use a `fn
  self_test()` run as `muz eval FILE --expr 'self_test()'`. With both, `--expr` evaluates FILE as the entry point, so
  the top-level check runs and then the expression runs it again. Worse, any
  `--expr` used to inspect a failing contract (`--expr 'bass_roots'`) fails
  with the self-test's assertion first, so the data can't be read until the
  top-level check is removed.
- **Affected work:** contract files whose self-test should run on a plain
  `muz eval` and still be debuggable.
- **Workaround:** `material.muz` keeps only `fn self_test()` and doesn't call
  it under `is_entry()`.
- **Desired behavior:** a way to tell "evaluated for `--expr`" from "evaluated
  as the program" (or `is_entry()` false under `--expr`), or docs that pick one
  pattern.

### The native reverb has no early-reflection or room-size control

- **Origin:** Main theme from KITE, v1 palette, room candidates R1–R3.
- **Observed behavior:** `muz devices inspect reverb` lists `decay_s`,
  `damping_hz`, `mix` and `predelay_ms` only.
- **Affected work:** the plan's room is "a small damp room: short,
  early-reflection heavy, 15–25 ms pre-delay, tail about 1.2 s". The early
  reflections that make a room sound small and close can't be set; the
  candidates differ only in tail length and damping.
- **Workaround:** short decay and high damping; Pianoteq's own room as a third
  candidate.
- **Desired behavior:** early-reflection level and pattern (or room size) on
  the native reverb.

### No `ceil`, and no way to read a digit as a number

- **Origin:** Main theme from KITE, v1 foundation (`material.muz`, the palette).
- **Observed behavior:** `ceil` and `number`/`parse` are unknown functions;
  `floor` and `round` exist. Reading a guitar grip such as `"xx3231"` into
  frets needs a lookup (`filter(range(10), fn(i) => str(i) == c)[0]`).
- **Affected work:** grip tables written as strings, clip lengths rounded up
  to whole bars.
- **Workaround:** `floor(x) + 1` (wrong when x is whole) and the digit lookup.
- **Desired behavior:** `ceil`, and a string-to-number conversion.
