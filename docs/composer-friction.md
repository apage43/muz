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

### Dense wind physics cannot run in real time

- **Origin:** Arundo v5 and the 2026-11-13 woodwind realism work.
- **Observed behavior:** Costs are user-mode megacycles per second of audio on
  the audio thread (`examples/woodwind_callback`, 128 frames, 48 kHz; a 4 GHz
  core supplies 4,000). The engine runs event-rate nodes only when their
  sources change, rests settled smoothers, and shares a live block's tracks
  among worker threads. `examples/wind-ensemble.muz` (five `std/woodwinds`
  winds and a hall) costs 1,110 on one thread (28% of a core) and 300 on the
  audio thread with workers; its wall-clock p99 is 0.45 ms of a 2.67 ms
  deadline. A held `std/woodwinds` note costs 185 (clarinet), 200 (bassoon),
  215 (oboe) and 220 (flute): the flute and oboe sit just over 5% of a core.
  One voice of the fixed-bore lattice prototype
  (`examples/woodwind-prototypes/lattice-slurs.muz`) costs 390 on average and
  940 at the 99th percentile, 10% and 24% of a core. About two thirds of a wind
  graph is audio rate (bore and reed loops, breath noise, and everything after
  the breath envelope), at 96 kHz, and each audio-rate node costs the
  interpreter about 13 cycles where its arithmetic needs one to four.
- **Affected work:** every finer acoustic model that adds audio-rate nodes:
  tone holes with the lower bore attached, a mass-spring reed (which may want
  4x oversampling), and ensembles of such voices.
- **Workaround:** offline rendering for lattice-class models; the std winds
  play live.
- **Desired behavior:** one lattice voice at 5% of a core or less, p99
  included, without changing output. The interpreter's per-node cost is the
  multiplier on all of these models. Reproduce with
  `cargo run --release --example woodwind_callback -- examples/woodwind-prototypes/lattice-slurs.muz 4 1`.

### The clarinet chirps on a descending slur across the break

- **Origin:** Arundo v4, clarinet at 3:32.48 (written B4 to open G4).
- **Observed behavior:** `std/woodwinds` ties the register vent and the bore
  length to one control. On a descending slur the delay line shortens while
  the old third mode is still ringing, so the read point sweeps through the
  stored wave and emits a brief burst near 1 kHz before the fundamental
  speaks.
- **Affected work:** every fast slur across the clarinet and bass clarinet
  break.
- **Workaround:** the register vent closes in 0.4 ms on a descent, and
  `register_blend: true` crossfades two independent register loops. The
  blend sounds clean but is not a model of the instrument.
- **Desired behavior:** a fixed-geometry bore in `std` whose tone holes and
  register vents open and close, keeping the lower bore attached and taking
  pitch flexibility from the player. The prototype in
  `examples/woodwind-prototypes` does not yet meet this.

### A played wind part costs most of the evaluation budget

- **Origin:** the 2026-11-13 woodwind player (`std/woodwinds.play`).
- **Observed behavior:** `play` plans breaths and phrases over a whole line,
  then plays each note with that context. It spends about 3,300 evaluation
  steps per note, against about 800 for `perform`. One evaluation shares
  5,000,000 steps across the entry file and every import, so a song holds about
  1,500 played wind notes. Most of the cost is ordinary arithmetic, record
  construction and closure calls per note. Sequential state also costs more
  than it should: a `fold` that appends to a list copies it at every step, so
  `play` cuts the line into segments and joins their results.
- **Affected work:** any orchestral or long chamber piece played with `play`;
  further player rules make it worse.
- **Workaround:** split a long piece into separately rendered sections, or use
  `perform` for less exposed parts.
- **Arundo v6 (925 notes, five parts):** with the song's `meters` and a
  breakpoint `dynamics` list, `play` cost 5,400 to 6,200 steps per note, not
  3,300, and the song exceeded the budget. `meters` adds about 1,000 steps per note
  (`metric_weight` calls `arr.bar_at` twice per note, each a fold over the
  meter list). The `dynamics` list adds about 700 because `interp` runs its search
  several times per note. The piece fits only with a table-backed dynamics function
  (`playing.muz` `dynamic_curve`), and then with about 1% of the budget left. Any
  further rule or part would break it.
- **Desired behavior:** an evaluation budget that scales with the music it
  produces, or one a song can raise, and a linear-time scan (or append) for
  sequential note state.

### The wind player counts staccato at its written length

- **Origin:** Arundo v6, bassoon bars 29–40 and 57–63 (the jig's engine).
- **Observed behavior:** `std/woodwinds.play` spends air for a note's whole
  written duration and measures breath time from written gaps. A staccato note
  sounds for half its length, but the plan still charges the full length and
  finds no time in the silent half. Twelve bars of staccato eighths at 162 bpm
  therefore read as 13 s of continuous playing. The plan reports `breath_time`
  findings that a bassoonist would breathe through in the gaps.
- **Affected work:** any staccato accompaniment figure (engines, repeated-note
  basses) played strict.
- **Workaround:** Arundo writes a rest before the bassoon takes the tune (bar 40)
  and accepts the other finding on one note (`data.play.accept`).
- **Desired behavior:** the air plan charges the performed (gated) length, and
  the silence a staccato leaves counts toward breath time.

### The oboe's breath model leaves no time to breathe at ordinary joins

- **Origin:** Arundo v6, oboe solo (bars 77–80) and chorale (85–98).
- **Observed behavior:** an oboe breath needs `exhale` 0.3 s plus `inhale`
  0.35 s. In a running line it may take at most `take` 0.35 of the note
  before it, plus `late_max`. A note shorter than about 0.9 s gives no breath
  that counts (`refill` under 0.25). A full breath needs a note of about 1.7 s,
  and a quarter note at crotchet = 72 lasts 0.83 s. A partial breath renews
  stale air only by its `refill`. An oboist's quick
  exhale, which renews stale air without an inhale, has no counterpart. A
  15 s solo phrase with a comma before its last bar, and a chorale inner voice
  joined by quarter notes, are both reported as unplayable (`slur_breath`,
  `breath_time`).
- **Affected work:** oboe lines in moderate tempi without written rests.
- **Workaround:** Arundo writes eighth rests at four phrase joins.
- **Desired behavior:** profile numbers checked against oboists' practice,
  and a breath kind that only lets stale air out.

### Unplayable reports name the wrong limit

- **Origin:** Arundo v6, oboe and bassoon.
- **Observed behavior:** when stale air forces the oboe to breathe, the
  `slur_breath` message still says "a breath lasts about 45.5 s at this
  dynamic", which is its air budget, not the limit that applies. `breath_time`
  rounds both values to a tenth. A bassoon report therefore reads "a breath with
  only 0.2 s of time ... a useful breath needs 0.2 s".
- **Affected work:** deciding, from the report, whether to fix the part or
  accept the finding.
- **Workaround:** reading the plan (`data.plan.breath`, `refill`) note by note.
- **Desired behavior:** the message names the limit that applies (air or stale
  air) and gives enough precision to show the shortfall.

### DAWProject export drops most wind expression

- **Origin:** the 2026-11-13 woodwind player.
- **Observed behavior:** the player writes per-note `pressure`, `tuning`,
  `brightness`, `volume`, `vibrato`, `vibrato_rate`, `tongue` and `noise`
  curves. DAWProject export carries only `tuning` (transpose), `brightness`
  (timbre) and `pressure`. It reports the rest as `NOTE_EXPRESSION_OMITTED`,
  so a handed-off wind part loses its vibrato, level tapers and articulation.
- **Affected work:** DAW handoff of any `std/woodwinds` part.
- **Workaround:** none in the DAW. Keep the muz source, or bounce stems.
- **Desired behavior:** a per-note parameter timeline for every standard
  and declared custom expression that the carried Muz device reads.

### Wind models have no voicing or articulation-shape control

- **Origin:** the 2026-11-13 woodwind player.
- **Observed behavior:** the player supports a leap across a register break
  and darkens a double-tongued "ku" through `brightness`. On the models that
  control is the embouchure (reeds) or the jet's aim (flute), not the player's
  vocal tract. The tongue's burst length is a patch option (`burst`), so light
  fast tonguing and a heavy accent differ only in strength.
- **Affected work:** register slurs, double tonguing and articulation variety.
- **Workaround:** the `brightness` and `tongue` strength approximations in
  `std/woodwinds.phrased`.
- **Desired behavior:** note controls in `std/physical.wind_player` for vocal
  tract resonance (voicing) and the tongue's release shape and length.

### MAYBE TODO: audible breaths

- **Origin:** the woodwind player, 2026-11-13. Low priority: nice if good, not needed.
- **Observed behavior:** `play` plans each breath and leaves its time in the line,
  but nothing voices it. A first `inhale()` (band-passed noise under an ADSR) was
  removed because it sounded very unrealistic.
- **Affected work:** close-miked solo winds, where a listener expects to hear the
  player breathe.
- **Workaround:** none needed; breaths are silent.
- **Desired behavior:** a breath a listener believes, or none. Candidates: turbulent
  noise at the real air inlet (mouth corners, lips), shaped by changing vocal-tract
  resonances, a flow envelope from the planned breath's length and refill, and lip
  or mouthpiece transients. Recorded samples are another option. The plan data is
  already on each note (`data.plan.breath`).
