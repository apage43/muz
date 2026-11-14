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

### Wind patches cannot run in real time

- **Origin:** Arundo v5 and the 2026-11-13 woodwind realism work.
- **Observed behavior:** `examples/wind-ensemble.muz` (five `std/woodwinds`
  instruments and a hall) misses every callback deadline at 64, 128 and
  256 frames. Its mean processing time is 1.5–2× the deadline on one
  Threadripper 3990X core, varying with machine load. One voice of the
  fixed-bore clarinet prototype takes 0.7–1.1× a 64-frame deadline. The graph
  evaluates every node every sample, including per-hole `shape`, `slew` and
  cutoff chains whose inputs change only at note events. Tracks render on one
  thread. The lattice is close to the 512-node limit.
- **Affected work:** live playback of any wind ensemble, and every finer
  acoustic model (tone-hole lattices, dynamic reeds) that adds nodes.
- **Workaround:** offline rendering.
- **Desired behavior:** a general graph optimisation that evaluates
  note-rate and constant subgraphs once per note or block, plus parallel
  track processing. Target: one lattice voice at 5% or less of a core, and
  five winds with a hall at half or less of a 128-frame deadline. Reproduce
  with `cargo run --release --example woodwind_callback -- examples/wind-ensemble.muz 3`.

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
- **Desired behavior:** an evaluation budget that scales with the music it
  produces, or one a song can raise, and a linear-time scan (or append) for
  sequential note state.

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
