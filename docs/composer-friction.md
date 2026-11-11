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

### Native SFZ rejects whole programs over EQ and flex-EG opcodes

- **Origin:** The Long Debt v1, Step 0 palette (candidate C2, Karoryfer x bigcat cello, CC BY 4.0).
- **Observed behavior:** `sfz()` fails to load `Programs/02- Bowed (mod wheel).sfz`
  with `unsupported opcode eq1_gain`; after removing those it fails on
  `lfo2_pitch_oncc118`, `lfo2_freq_lfo1_oncc119` and the flex EG `eg06_*` opcodes
  (including `eg06_time1_oncc109` inside the included region maps). Loading
  stops at the first one, so finding them all takes one load per opcode.
- **Affected work:** the library's mod-wheel program uses EQ bands to colour its
  CC1 dynamics and a pitch EG for legato slides; neither can be used, and the
  program cannot be loaded as published.
- **Workaround:** the piece's `prepare.py` writes wrapper copies of the program and
  its maps without those opcodes, leaving the originals untouched.
- **Desired behavior:** support `eqN_*` and flex-EG pitch targets, or at least
  list every unsupported opcode in one diagnostic (and offer a lenient mode that
  ignores them with a warning, as sfizz does).
