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

### `devices state --load` returns the plugin's initial state for a file it did not load

- **Origin:** The Long Debt v2, Step 0 palette (the hook-voice candidate S3).
- **Observed behavior:** `muz devices state surge --load "/usr/share/surge-xt/patches_factory/Leads/Formant Pulse.fxp" -o fp.state`
  exits 0 and writes a state whose patch is Surge's "Init Saw", not Formant Pulse.
  Nothing says the file was not applied (Surge's own factory `.fxp` is not a
  format the CLAP host's `--load` path hands to the plugin).
- **Affected work:** starting a synth candidate from a factory preset; a silently
  wrong state would be auditioned as if it were the preset.
- **Workaround:** the piece's `prepare.py` cuts the `sub3` chunk out of the `.fxp`
  (it is the same container the CLAP state uses) and writes it as the state.
- **Desired behavior:** `--load` fails with a clear error when the plugin did not
  accept the file (or the loaded state is unchanged), and the docs say which preset
  formats it passes to CLAP and VST3 plugins.

### A raw event offset before the song's start is clamped silently, and a note that lands wholly before it fails as "needs positive duration"

- **Origin:** Main theme from KITE (`bands-and-guitars/untitled-ruins`), v1
  stream B (Inés's lead guitar, `guitar1.muz`), on muz `9e37f2ea…`.
- **Observed behavior:** a `note_on` at `0b` with `offset = -10ms` renders, but
  the onset is moved to 0 without a word. A note whose on and off both fall
  before 0 through their offsets (`note_on(60, 0.5, at = 0b, offset = -10ms),
  note_off(60, at = 0b, offset = -5ms)`, 5 ms long in real time) fails the whole
  render with `raw note needs positive duration`, reported at the `track(...)`
  call, not at the note. The same pair placed at `1b` renders. So the message
  describes the clamped result, not what was written, and nothing says which of
  thousands of events it is.
- **Affected work:** E2's performance layer moves soft notes earlier by their
  pluck latency (8 ms), and a thumb-tone keyswitch is a ±2 ms window around its
  note. On the piece's first downbeat both land before 0. The error pointed at the
  track holding 1,068 events, and the first guess (the key's zero score
  duration, which is in fact allowed) cost a render and a workaround that wasn't
  needed.
- **Workaround:** the part keeps every onset on the first downbeat at +3 ms or
  later, and its self-test asserts that no event starts before 0.
- **Desired behavior:** an event whose real time falls before the song's start
  is either an error naming the event (time, channel, pitch, the offset), or a
  documented clamp. If it is clamped, a note shortened to nothing by the clamp
  says so, at the note's source location.

### A mix can only be measured pre-fader: taps have no post-fader point and renders cannot mute

- **Origin:** The Long Debt v2, the mix (A2): setting faders and rides from one full render.
- **Observed behavior:** `--tap` writes a track at its stem boundary (after inserts
  and pan, before output gain, sends and master). There is no post-fader tap, so a
  track's level in the mix (its static `gain` or its `.out` lane) has to be
  reconstructed outside muz. The docs do not say whether a bus tap (`--tap rhythm`)
  is taken before or after the bus's own `.out` lane. `render` has `--solo` but no
  `--mute`, so the loudness a part contributes to the mix ("muting the heavy pair
  costs ≤ 1.0 LU") can only be measured by copying the song with a −120 dB ride for
  each variant and rendering each copy.
- **Affected work:** every balance target in the plan (the tune under the band,
  mute costs, a lead's margin over the next stem) is a post-fader comparison; the
  integrator re-implements the fader and automation curves (from `muz eval song.muz
  --expr` on `tracks` and `automation`) to multiply taps, and keeps scratch copies of
  `song.muz` beside the real one for the mute renders.
- **Workaround:** `review/measure.py balance` in the piece rebuilds post-fader
  elements from the pre-fader taps and the evaluated lanes; mute costs were checked
  with two scratch songs and `--section … --history 6` renders.
- **Desired behavior:** a post-fader tap point (for example `--tap NAME:post`, after
  output gain and before sends), a documented boundary for bus taps, and a
  repeatable `--mute TRACK_OR_BUS` on `render` and `stems`, the counterpart of
  `--solo`.
- **Also hit by:** Main theme from KITE v1, stream D (the flute). The plan's
  "the partner at least 6 dB under the guitar's F at bar 75" is post-fader; the
  check added the guitar's +3 dB fader to its tap by hand.
- **Also hit by:** Main theme from KITE v1, stream C (the 12-string). The plan's
  "guitar 1 over guitar 2 by at least 4 dB, stem times fader" and the 250–1000 Hz
  report were built by adding the check song's +3 and −1 dB faders to the taps by
  hand. How much of the 1 LU lift at bar 57 is the second guitar's could not be
  measured: there is no `--mute guitar2`, and a scratch copy of the check song
  was not worth it for an instrument that renders differently every pass.
- **Also hit by:** Main theme from KITE v1, integration (A2). Every R4.6 mute cost
  (drum ≤ 0.7 LU, room ≤ 1.0 LU) needs a part's whole contribution, including its
  share of a shared reverb, which no tap gives. A second render can't stand in,
  because the 12-string differs every pass. Workaround: `song.muz` gives each track
  its own copy of the room bus, so a part's contribution is its tap times its fader
  plus its own `room_<track>` tap. The split is linear, so it sounds the same: it
  nulls against one shared room at −145 dB on the non-Pianoteq tracks, and the
  master equals the sum of the parts to −143 dB. It costs one reverb per track and
  hand-rebuilt faders.

### A batch job takes one tap, so a measured mix can't share a batch with its clips

- **Origin:** Main theme from KITE v1, the mix (A2).
- **Observed behavior:** `muz batch` writes one file per job, and an output's
  `tap` option names one track or bus (`workflow.md`: "batch renders write one file
  per job, so they take a single tap"). A mix measured from its taps (every track,
  every room and the master, from one pass) therefore has to be a separate
  `muz render` with repeated `--tap`. Rendering each tap as its own batch job
  doesn't work either: each job is a separate render, and a Pianoteq 12-string
  never strums the same way twice, so the taps would not add up to the master.
- **Affected work:** the plan asked for "one batch": the mix, the glide version
  of the same mix, and a flute-alone pair, rendered together so that they share
  sources and history. The mix and its comparison clips came out of two
  invocations, and the integrator had to check by hand that the sources and the
  binary were unchanged between them.
- **Workaround:** `render.py` runs the mix with every `--tap` in one pass and
  checks the source and binary hashes before and after. `muz batch song.muz clips`
  runs beside it from the same sources.
- **Desired behavior:** a batch output that writes several taps from one pass,
  for example `taps: ["guitar1", …, "master"]` into a directory per job, as
  `render --tap` does.

### Nothing reports a sampler note that outlasts its unlooped recording

- **Origin:** Main theme from KITE v1, stream D (the flute's held F5, 4.3 s).
- **Observed behavior:** a `sample(...)` zone with no `loop` and `one_shot: false`
  plays to the end of its recording and is then silent while the note is still
  held. Graph preparation checks key and velocity coverage but not whether a
  note's performed length (through the tempo map, at its playback speed from the
  zone root, after the zone `offset`) fits the recording. `synthesis.md` mentions
  the silence only for slurred groups. Whether a long note sustains has to be
  measured outside muz, by reading the WAV lengths and doing the arithmetic.
- **Affected work:** every held note on an unlooped library (VSCO 2 CE has no loop
  points): the plan's "if the sample won't sustain that long, say so" check, and
  any held note that changes length when the tempo map changes.
- **Workaround:** measured the recordings with a Python script: the F5 uses
  4.6 s of the 8.1 s E5 recording, so it fits. Nothing protects it if the clock
  slows further.
- **Desired behavior:** a `muz check` warning (or an exposed per-note "recording
  remaining at release" value a self-test can read) naming the track, note and
  how far it overruns its recording.
