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

### SFZ constructor controller values fail song validation

- **Origin:** Pocket Customs full arrangement; first exposed on `d3cf44ffbd257250fcc715de92085154bf827eaf`, reproduced against current main `8044edfa8b117ad60c3937d26cfd75038fb3e266` (2026-10-01).
- **Observed behavior:** A song using `sfz("program.sfz", {cc1:104})` fails with `x.instrument: unknown parameter 'cc1'` and an empty parameter list, even when the SFZ is only `<region> sample=*silence key=60`. The SFZ-aware device validation accepts physical `cc0` through `cc127`, but the song compiler first searches an empty static parameter table. Current `docs/sfz.md` documents constructor controllers overriding mapping defaults.
- **Affected work:** Initial clarinet dynamics, instrument gain controllers and dry drum microphone settings cannot be declared in the instrument options as documented.
- **Workaround:** The initial piece used explicit score-start `cc()` messages. The user requests removing that workaround after fixing constructor validation; genuine later musical automation is separate.
- **Desired behavior:** Song constructors must use the existing SFZ-aware device validation for physical controller names and finite MIDI-unit values in 0–127, reject invalid fields/ranges, and retain constructor overrides in the device's initial/reset state.

Minimal reproduction:

```sfz
<region> sample=*silence key=60
```

```muz
song({tracks:[track("x",note(60,1b),sfz("program.sfz",{cc1:104}))]})
```
