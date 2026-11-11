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

### Static overrides of a plugin's MIDI-mapped pedal parameters are silently discarded

- **Origin:** Untitled ruins theme (KITE), step 0 guitar probe
  (`muz-projects/bands-and-guitars/untitled-ruins/probe/`), muz sha256 `944309c6…`.
- **Observed behavior:** on Pianoteq 9 VST3 (Classical Guitar, advanced mode),
  `piano("default", {state: …, buff_stop_pedal: 1})` renders bit-identically to the
  same song without the override; so do `glissando_pedal: 1` and `sustain_pedal: 1`.
  `pinch_harmonic_pedal: 1` works. An automation lane on the same parameters works,
  mid-song too, and so does `cc(66, 127, at = 0b)`. `muz devices inspect` lists
  `pedal_parameter_ids` `0x6d636d40/42/43` (the CC64/66/67 proxies), and the
  render-pair trace in guitar-planner showed the host queueing zero points for
  exactly those IDs at offset 0 of the first process call. The guitar's MIDI
  mapping routes CC64/66/67 to Pianoteq's pedal slots (sustain, buff stop,
  glissando), so the reset overwrites the static values after they are applied.
  CC69 (pinch) is not reset, which is why its override survives. Nothing warns.
- **Affected work:** a part that wants palm-mute amount or sustain as a fixed
  instrument setting gets the plain instrument with no error. Found only by
  comparing renders.
- **Workaround:** a controller at `0b` (`cc(66, v, at = 0b)`) or a one-point lane.
  The piece's `gtr.muz` documents it.
- **Desired behavior:** apply source overrides after the host's controller reset,
  or skip the reset for parameters the source sets; at least a `muz check`
  warning when an override targets a parameter that the controller reset owns.

### Raw channel events take score time only, so millisecond windows depend on the tempo

- **Origin:** Untitled ruins theme (KITE), step 0 guitar probe and `gtr.muz`.
- **Observed behavior:** `note_on`, `note_off` and `bend` accept `at` in beats and
  reject `offset = 20ms` ("unexpected arguments: offset"); `1b + 20ms` is a unit
  error. Driving Pianoteq's guitar is mostly millisecond windows: a slur needs the
  old key held 25 ms into the new one, the thumb key must surround a note-on by
  a couple of milliseconds, a glissando source is released 40 ms after its target,
  a strum spreads strings by 10–30 ms.
- **Affected work:** every raw-event helper takes the song's BPM and converts
  (`x * bpm / 60000 * 1b`). The windows are then wrong under a tempo map or a
  ritardando, and a helper can't be reused at another tempo without threading the
  tempo through every call.
- **Workaround:** `gtr.ms(x, bpm)` in the piece; the piece has one tempo.
- **Desired behavior:** a clock-time `offset` on raw channel events (as `control()`
  already has), or score-plus-clock time arithmetic that resolves against the
  song's tempo map.

### `pianoteq-state.py --preset "Classical Guitar"` is ambiguous though the name is exact

- **Origin:** Untitled ruins theme (KITE), step 0 second-guitar candidates.
- **Observed behavior:** `contrib/plugins/pianoteq-state.py --preset 'Classical
  Guitar'` fails with "ambiguous factory preset" and lists all 19 Classical Guitar
  presets. `select_factory` matches the length-prefixed identity anywhere in each
  preset's payload, and every guitar preset carries the instrument name
  "Classical Guitar" as well as its own preset name. "Classical Guitar 12 Strings",
  "… Warm" and "… Gentle" captured fine.
- **Affected work:** the plain factory preset can't be captured, so it can't be an
  audition candidate; the same will hold for any pack whose base preset shares the
  instrument's name.
- **Workaround:** none needed for this piece (the plain preset wasn't a candidate).
- **Desired behavior:** match the preset-name field itself, or prefer an exact
  file-stem match (`Classical Guitar.vstpreset`) when several payloads match.
