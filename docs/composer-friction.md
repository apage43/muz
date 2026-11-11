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

### A slurred note's expression does not reach a mono-legato CLAP synth

- **Origin:** The Long Debt v1, stream F (Nadia's Surge XT lead, `synth.muz`).
- **Observed behavior:** on Surge XT 1.3.4 in a mono legato mode (`Mono ST`),
  when a note overlaps the next (gate above 1), the second note's per-note
  expression never sounds, and the first note's last expression values carry on
  through it. Measured at 60 BPM: `A4:h` (gate 1.05) into `C5:h` with
  `tuning: 2` sounds C5, not D5 (attacked, the same note sounds D5); `A4:h` with
  `volume` falling to 0.2, slurred into an unexpressed `C5:h`, leaves the C5
  12 dB down. The voice keeps the first note's ID, so curves sent to the new ID
  address no voice. Nothing reports it.
- **Affected work:** phrasing slurred groups on a mono lead: a swell, scoop or
  vibrato on a slurred-into note is silently dropped, and a vibrato or bend cut
  at the slur leaves the rest of the group detuned.
- **Workaround:** slur targets carry no expression; every curve on a note that
  slurs ends back at pitch before its release (checked in the piece).
- **Desired behavior:** a way to phrase a legato group on such an instrument:
  for example, send a legato note's expression to the voice that sounds it
  (opt-in per track), or at least a check or diagnostic that flags expression
  on notes that will not be heard.

### A slur needs overlapping notes, which the monophony checks reject

- **Origin:** The Long Debt v1, stream E (Ilse's cello lament on Sonatina Cello
  Solo KS, native `sfz()`).
- **Observed behavior:** a slur is expressed by overlap. Sonatina's legato
  regions (`trigger=legato`) play their transition only when the next note
  starts while the previous key is still held, and `sample(...)` with
  `voice_mode: "legato"` uses the same rule. `checks.monophonic()` (default
  tolerance `0b`) and a piece's own one-player checks read score time
  (`at + duration * gate`), so a slurred line written with `gate > 1` fails
  them. Neither the score, `inspect` nor the exports can tell a slur from an
  accidental overlap.
- **Affected work:** every slur in a bowed or sung line on a monophonic
  instrument with a checked range and monophony: the lament's slurs and bow
  changes, about 60 notes in this piece.
- **Workaround:** slurred notes keep `gate: 1` and get a positive
  `release_offset` (25 ms) so the overlap exists only in performed time; bow
  changes get `-30 ms`. The checks pass, but the slur lives in an unnamed
  performance offset rather than in the score.
- **Desired behavior:** a per-note slur marking (an annotation like
  `articulation`, e.g. `data.slur` or a `slur` tag) that the sampler and SFZ
  legato logic honour as "the previous note is still held", and that
  `monophonic()` and the exports read as one legato line rather than an
  overlap.

