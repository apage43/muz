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


### A sampled guitar has no slur: legato must be faked as one note's tuning curve

- **Origin:** Glass Hearts v15 (G), Jun's rewritten solo (bars 73–80): hammer-on
  and pull-off runs, slides that shift position, bends that release into
  pull-offs.
- **Observed behavior:** the Karoryfer guitar is a `sample()` instrument, which
  only plays each note as a new attack. Shinyguitar has no recorded legato,
  slide or bend articulations. `std/instrument.mono(..., glide)` continues a
  voice without re-attacking, but only for a programmable `voice_patch`. To use
  it the track's instrument has to be rebuilt as `instrument.sampled(...)`,
  which changes every other bar the track plays, and a lone `sample()` track has
  no legato voice mode. So the piece merges each slurred group into one picked
  note whose `tuning` curve steps (hammer, pull) or ramps (slide, bend, release)
  through the written pitches, with `volume` steps for the hammers. That works
  and sounds like one string, but the hammered notes stop being notes: MIDI
  export, `inspect` and every check see only the picked notes (51 of the
  solo's 83 written notes are slurred, so the export has 32 of them), and the
  32-point budget limits how long a slurred run plus vibrato can be.
- **Affected work:** any idiomatic guitar or bass lead (legato runs, slides,
  bend-and-release phrases) on a sampled instrument.
- **Workaround:** a source recipe in `guitar.muz` (`slurs`, `slur_group`) that
  groups notes by tag and writes the tuning and volume curves, asserting the
  point budget.
- **Desired behavior:** a way to slur written notes on a sampler: for example a
  legato voice mode with glide time on `sample()` instruments, chosen per note
  (a tag or note field) so picked and slurred notes share one track. The
  slurred notes should stay notes in the score and the export.


### The limiter holds sample peaks only, so true peak can't be set

- **Origin:** Glass Hearts v15 (A2), the master's true peak after the jitter
  re-roll.
- **Observed behavior:** `limiter` keeps every sample under `ceiling_db` (its
  gain plus a final clamp) but doesn't look between samples. With
  `ceiling_db: -3` the master's sample peak is −3.00 dBFS, while ffmpeg's true
  peak is −1.9 dBTP. The overshoot comes from the ride recording's 16–24 kHz
  energy at the clamp. Trimming faders barely moves it: cutting the post-chorus
  master level by 0.6 dB or the drum bus by 1.2 dB changed the worst bar by
  0.0–0.1 dB, because the limiter still clamps the same transient.
- **Affected work:** keeping a −2.0 dBTP limit. Every re-roll or level change
  needs a full render and a hunt for whichever source has the ultrasonic
  content.
- **Workaround:** low-pass the ride at 16 kHz (`sounds.muz`), found by tapping
  each bus and kit piece around the over. A lower ceiling was ruled out because
  it drives the limiter harder.
- **Desired behavior:** a true-peak option on `limiter` (oversampled
  detection, for example `true_peak: true`), or a true-peak figure per bar or
  section in `muz analyze` so the source can be found without custom scripts.
