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

### An oversampled patch folds its 24–29 kHz partials back into 19–24 kHz

- **Origin:** std/woodwinds voicing (bass clarinet and clarinet top notes at ff).
- **Observed behavior:** at `oversample: 2` the half-band decimator is about 80
  dB down only from 0.3 of the internal rate (28.8 kHz at 48 kHz output), so a
  beating reed's partials between 24 and 28.8 kHz fold back to 19–24 kHz at about
  40 dB under the fundamental. `oversample: 4` does not remove them: it is the
  last stage's transition band.
- **Affected work:** the top octave of bright reed notes. Inaudible to most
  listeners, but it is energy that is not in the instrument and a measurement of
  aliasing must stop at 16 kHz to ignore it.
- **Workaround:** none needed for listening; tools/woodwinds.py measures
  aliasing below 16 kHz.
- **Desired behavior:** the decimator's stopband starts at the output's Nyquist
  frequency (a steeper or longer half-band filter in the last stage).

### No convolution for a body, a bell or a room

- **Origin:** std/woodwinds voicing against recordings.
- **Observed behavior:** an instrument's body and bell, and a hall, can only be
  approximated with parametric filters and the algorithmic reverb. The oboe's
  and bassoon's formants are two or three bandpass filters; the recordings'
  spectral envelopes have more detail (the remaining 10–13 dB rms error of their
  second and third partials is partly this).
- **Affected work:** matching a recorded instrument's body response, and placing
  an ensemble in a measured room.
- **Workaround:** `std/physical.body` formants; `fx("reverb")`.
- **Desired behavior:** a convolution node or effect taking a short impulse
  response (a body or bell, tens of milliseconds) and a long one (a room).
