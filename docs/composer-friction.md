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

### A limiter's true-peak mode changes its gain reduction, not only its detector

- **Origin:** Glass Hearts v16, bringing the master's true peak under −2 dBTP
  without v15's 16 kHz low-pass on the ride.
- **Observed behavior:** switching the master limiter
  (`ceiling_db: -3, lookahead_ms: 5, release_ms: 90`) to `true_peak: true`, with
  nothing else changed, made the mix louder, not quieter: the choruses rose
  0.8–0.9 LU, the pre-choruses, interlude and build 0.4–0.7 LU, and the loudness
  range went from 5.9 to 6.8 LU. True peak became −3.0 dBTP in every section, as
  documented. A stricter detector at the same ceiling should reduce gain at
  least as much as the sample-peak mode, so the gain envelope itself must differ
  between the modes (the documented ramp over the lookahead window, against the
  default mode's attack and release). `production.md` describes the true-peak
  mode's ramp and latency but not how its gain reduction compares with the
  default mode's, so the switch reads as a drop-in fix and is not one.
- **Affected work:** meeting a true-peak limit on a finished master. Turning the
  mode on re-masters the song: every section's balance against the earlier
  revision moves, so the change cannot be judged as a peak fix alone.
- **Workaround:** the original limiter is unchanged and a second, true-peak
  limiter follows it 0.7 dB higher
  (`ceiling_db: -2.3, lookahead_ms: 2, release_ms: 50, true_peak: true`), so it
  catches only inter-sample overs. Sections stay within 0.1 LU of v15.
- **Desired behavior:** `true_peak: true` changes only what is detected (4x
  oversampled peaks instead of samples) and keeps the default mode's gain
  envelope for the same `lookahead_ms` and `release_ms`; or, if the envelopes
  must differ, `production.md` states how and by how much, so a composer knows
  the switch will move loudness.
