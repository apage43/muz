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

### SFZ articulations can only be selected by writing keyswitch notes

- **Origin:** Glass Hearts, evaluating Unreal Instruments Standard Guitar (KSOP
  program) for Jun's lead and strum (scratch A/B, no piece change).
- **Observed behavior:** `sfz()` honours `sw_last` keyswitches, but the only way
  to choose an articulation for a note is to write an extra note at the switch
  key (C1 = 24, F#1 = 30, G#6 = 92, …) a few ms before it. Those switch notes
  are ordinary notes everywhere else: `checks.in_range(40, 84)` reports each one
  as outside the guitar's range, `checks.monophonic()` reports the switch and
  its note as starting together, `muz check` counts them, and MIDI export
  carries them. When two simultaneous notes need different articulations (a
  double stop with one string bent), the switches and notes have to be
  staggered by hand with negative `offset_ms` so each switch lands before its
  own note and after the previous one; a song can't start with a switch at
  beat 0 without a lead-in.
- **Affected work:** any keyswitched SFZ part (Standard Guitar, METAL-GTX,
  Darkblack's keyswitch program): per-note articulation from tags, and keeping
  a guitar part's range and monophony checks meaningful.
- **Workaround:** a source converter that maps technique tags to switch keys,
  inserts a switch note 4 ms ahead of every note (6 ms more for a double-stop
  string), and keeps those notes out of checks by hand; CC changes for slide
  intervals and vibrato are written the same way.
- **Desired behavior:** a per-note articulation choice for SFZ instruments (for
  example a note field or tag mapped to a keyswitch in the track or instrument
  options) that the engine sends to the instrument just before the note, so it
  is not a score note for checks, counts and export, and ordering against
  simultaneous notes is handled.

### A large SFZ program is prepared and budgeted once per track

- **Origin:** Glass Hearts, evaluating Standard Guitar's KSOP program for Jun's
  five guitar tracks (lead, two strum takes, heavy, echo).
- **Observed behavior:** one KSOP instance costs 20,150 graph units (12,427
  regions, 2,278 samples, 173,861 modulation terms) and about 2.35 GB resident;
  a song with two tracks using it costs 40,339 units and 4.2 GB, so each track
  prepares and budgets its own copy. `MUZ_GRAPH_BUDGET` accepts at most 65,536,
  so at most three tracks can use the program, fewer alongside the rest of a
  band. The default budget (4,096) rejects even one instance; the error names
  `MUZ_GRAPH_BUDGET` but not its maximum.
- **Affected work:** using one full sampled library for a player's several
  tracks (double-tracked strums, a separate amp channel), and render times
  (about 25 s of preparation per instance).
- **Workaround:** two instances at most, set `MUZ_GRAPH_BUDGET=65536`; the
  untested alternative is a project-local SFZ that includes only the needed
  individual patches.
- **Desired behavior:** tracks that use the same program and options share one
  prepared program (regions, decoded samples) and are budgeted for it once,
  with only their voices and controller state per track; or a documented way
  to set a budget above 65,536 when the host can afford it.

### A float beat value overflows exact score time at a tuplet position

- **Origin:** Glass Hearts v16, Jun's lead on Standard Guitar (`guitar.muz`,
  `lead`): each hammered or pulled note is its own voice, and the note before it
  is cut to end where the hammered note is played, after humanizing.
- **Observed behavior:** a beat quantity computed from a float stays a float
  (`1/6b + off * 128 / 60000 * 1b` with `off = -2.0258913990809204`, a humanized
  `n.offset / 1ms`, is `0.16234476501529405b`). As a note's length at a tuplet
  position, `note(60, len, at = 301/6b)` fails with `exact score time overflow`;
  the same length at an eighth position works. The diagnostic points at the
  `note` call but names neither value nor the float as the cause. A float with
  fewer digits (`2.7182818284`) works, so the failure depends on the jitter's
  digits.
- **Affected work:** ending a note exactly where its humanized neighbour starts
  (clean legato in the solo's sextuplet and quintuplet runs), and any score
  arithmetic that turns performed clock time into beats near tuplets.
- **Workaround:** round the clock difference before converting it
  (`ms(round(10 * d) / 10)`, 0.1 ms).
- **Desired behavior:** a float beat value converts to exact score time with a
  bounded denominator (or rounds to the engine's time resolution) instead of
  overflowing; failing that, the diagnostic names the operands and the float.
  A note end given as a `[beats, clock time]` position, as expression points
  allow, would avoid the conversion entirely.

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
