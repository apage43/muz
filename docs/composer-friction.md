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

### Songs have one meter, so mixed-meter forms lose their bar numbers

- **Origin:** The Long Debt v1 (muz-projects `bands-and-guitars/the-long-debt`), A1 foundation.
- **Observed behavior:** `song.meter` is a single `[n, d]` value; there is no meter
  map or per-section meter. A 7/8 bar has to be written as a 3.5-beat span in a
  4/4 song. Every tool that prints bar numbers then counts 4/4 bars: in the
  reduction `muz check` reported `piano bar 133 beat 1.5 (528.5b)` for what is
  bar 137, sixteenth 14 of the form (44 of its 158 bars are 7/8). MIDI and
  DAWProject exports carry the same 4/4 grid.
- **Affected work:** every player checks and translates diagnostics by hand after
  the first 7/8 bar (bar 36); DAW handoffs show the wrong bar lines; `--bpm`
  per-bar peak analysis cannot follow the form.
- **Workaround:** the piece's `material.muz` keeps a bar-length table and a
  `where(beat)` helper that prints "bar N, sixteenth K".
- **Desired behavior:** a song-level meter map (or meters on sections) that
  diagnostics, `analyze --bpm`, MIDI time signatures and DAWProject export follow.

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

### Devices inside a rack branch cannot be automated

- **Origin:** The Long Debt v1, Step 0 palette (bass candidate B2).
- **Observed behavior:** a chorus with `id: "mid_chorus"` inside one branch of a
  `rack(...)` on track `bass` has no automation target: `bass.mid_chorus.mix` is
  unknown, and the help lists only the rack itself (`bass.fx1.gain_db`,
  `bass.fx1.mix`) and the devices outside it.
- **Affected work:** a split-band bass (lows clean, mids driven) whose mid-band
  chorus is on in the verses and off in the choruses.
- **Workaround:** two bass tracks with copies of the chain, one with the chorus and
  one without, and the part split between them.
- **Desired behavior:** ids inside rack branches are addressable (for example
  `bass.mid_chorus.mix`), like ids in a track's own chain.

### `batch --match-levels` cannot level a candidate inside a mix

- **Origin:** The Long Debt v1, Step 0 palette.
- **Observed behavior:** `--match-levels` applies a listening gain to whole outputs
  in `listen.html` and leaves the WAVs as rendered. A decision page needs each
  candidate heard alone and over a fixed backing with only the candidate's fader
  moved, and needs files that are already matched.
- **Affected work:** every loudness-matched audition (Glass Hearts' and this one).
- **Workaround:** render once, read `integrated_lufs` from `renders.json`, write the
  per-candidate fader offsets into a source file, render again, and confirm with
  ffmpeg `ebur128`.
- **Desired behavior:** a way for a collection to name the track whose fader
  matching may move (for example `options.match: "synth"`), so one batch writes
  matched solo and in-context files and reports the offsets it applied.

### `bus()` accepts unknown options and drops them silently

- **Origin:** The Long Debt v1, Step 0 heavy-riff tone study (calibration clip).
- **Observed behavior:** `bus("gtr_l", chain, {pan: -0.8})` evaluates, passes
  `muz check` and renders, but the bus is not panned: its left and right levels
  match to 0.1 dB. `bus("b", [], {pan: -1, bogus_key: 3})` is accepted the same
  way. The builtin merges whatever record it is given into the bus
  (`src/lang/builtins.rs`, `"bus"`), and nothing downstream rejects unknown keys.
  Tracks take `pan`, so the mistake is natural.
- **Affected work:** a double-tracked guitar panned on its buses came out with
  both takes centred (equal left and right levels on each bus). Only a
  measurement caught it.
- **Workaround:** `fx("stereo", {pan: -0.8})` at the end of the bus chain.
- **Desired behavior:** unknown bus options are an error (with suggestions), as
  unknown kit voices are, or buses accept `pan` like tracks.
