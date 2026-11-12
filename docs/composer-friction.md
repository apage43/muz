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

### Song MIDI export cannot carry markers

- **Origin:** Starshot (muz-projects `games/starshot`), v1 process probe for a
  game studio's MIDI hand-off.
- **Observed behavior:** `muz midi export song.muz` writes tempo, meter, track
  names and notes, and nothing else in the tempo track. Song `sections` are
  not written. A `meta(6, [...])` event stacked into a track's pattern stops
  the export with `track Pulse1: device adapter cannot consume this raw
  message; structural SMF still preserves it`, because the track has an
  instrument. A pattern-root export keeps the meta event but has no tracks,
  names or tempo.
- **Affected work:** the studio wants `loop start` and `loop end` markers in
  each cue's file. The loop bounds are already in the source as the `intro`
  and `loop` sections.
- **Workaround:** `games/starshot/tools/finish_midi.py` reads the sections with
  `muz inspect --view sections` and adds the markers with mido.
- **Desired behavior:** a way for a song export to carry markers: sections
  written as marker events, or meta events on a song track passed through to
  the file and ignored by the instrument.

Reproduction: `games/starshot/probe/probe-meta.muz`.

### A kit track exports as one MIDI track per voice

- **Origin:** Starshot, the same probe.
- **Observed behavior:** `track("Noise", drums({...}), kit(...))` is one
  logical track, and its `checks` see all voices together. `muz midi export`
  writes the physical tracks instead: `Noise.hat`, `Noise.kick`,
  `Noise.snare`.
- **Affected work:** the hand-off needs one track per NES channel, named
  exactly `Noise`, with General MIDI drum notes on it. Writing the drums as
  plain numbered notes on a single patch exports one track, but gives up the
  `drums()` lanes and a sound per drum.
- **Workaround:** `finish_midi.py` merges every `Noise.*` track into `Noise`.
- **Desired behavior:** export a kit as its logical track, by default or by an
  option, with each voice on its drum note.

Reproduction: `games/starshot/probe/probe-kit.muz`.

### `format` has no positional placeholders

- **Origin:** Starshot v1 (`muz-projects/games/starshot`), `cues/hyperdrive.muz`
  and `cues/liftoff.muz`.
- **Observed behavior:** `format("{0}:s {1}:s {0}:s", [lo, hi])` fails with
  "format placeholder/value counts differ". Only `{}` in order is accepted, and
  the diagnostic does not say numbered placeholders are unsupported.
- **Affected work:** writing a bar's rhythm once as phrase text and filling in
  pitches that repeat (per-chord arpeggio or octave-bass cells, repeated
  countdown beeps on one note).
- **Workaround:** repeat the value once per placeholder, or build the bar from
  `note(pitch, duration)` calls, which makes the rhythm harder to read.
- **Desired behavior:** numbered or named placeholders in `format`, or a phrase
  template with pitch variables; at least a diagnostic naming `{0}` as
  unsupported.

### `drums()` lane symbols cannot carry a project's velocity scale

- **Origin:** Starshot v1, `cues/title.muz` and `cues/orbit.muz`.
- **Observed behavior:** in a `drums()` lane, `1`–`9` export as tenths of full
  velocity (13, 25, 38 … 114) and `x`, `X`, `g` as 91, 121 and 43. `.velocity()`
  on the result sets every lane to one value.
- **Affected work:** the target (NES noise channel) maps velocity to 16 volume
  levels, so each drum and each accent or ghost needs an exact level.
- **Workaround:** one `drums()` call per lane, each with `.velocity(level)`,
  stacked; an accent inside a lane needs a further split.
- **Desired behavior:** per-symbol or per-lane velocity in `drums()`, for
  example `drums(lanes, levels = {x: 7/15, X: 10/15})`.

Reproduction: `drums({hat: "123456789xXg...."}, span = 1bars)`, then
`muz midi export`.

### A top-level function used above its definition is "unknown"

- **Origin:** Starshot v1, `cues/arrival.muz` and `material.muz`.
- **Observed behavior:** a top-level `fn` defined below the `let` or `fn` that
  calls it reports "unknown function". Module functions are not hoisted.
- **Affected work:** ordering helpers in a module; a check run lost each time.
- **Workaround:** define helpers above their first use.
- **Desired behavior:** hoist top-level `fn` definitions, or say in the error
  that the function is defined later in the file, at line N.
