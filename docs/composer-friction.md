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

### A lattice voice peaks at its budget while its holes move

- **Origin:** Arundo v5 and the 2026-11-13 woodwind realism work.
- **Observed behavior:** Costs are user-mode megacycles per second of audio on
  the audio thread (`examples/woodwind_callback`, 128 frames, 48 kHz, one
  thread; a 4 GHz core supplies 4,000). Voice patches are
  [compiled](synthesis.md#compiled-patches), with output identical to the
  interpreter's, and repeated bore sections run four to a vector. With
  `renders` (the fourth argument) above 1, each block counts its least
  cycles over that many renders, which leaves out most of the machine's
  load. One voice of the round-7 tone-hole lattice prototype
  (`examples/woodwind-prototypes/lattice-slurs.muz`: 642 nodes, with
  second-order hole shunts, the lip-loaded reed, the tongue and the
  player's tract) costs about 122 over four seconds (3% of a core) and
  193–196 at the 99th percentile (4.9%), least of three renders. Its 99th
  percentile is the plateau of the few dozen milliseconds after each note
  change, while the seated keys' slews settle alone (eight of them a
  frame) and the register hole's jet moves its filter. A passage of fast
  slurs across the break,
  `examples/benchmarks/lattice-runs.muz` (ten notes a second), costs 177
  and 245–270 at the 99th percentile (6–6.7%); its peaks reach 390 in the
  frames where nine keys travel at once. There the settling tier runs each
  moving key's cluster alone: its slew, three shape lookups (cutoff, q and
  depth of the hole's filter) and the filter's coefficients, about 200
  instructions a key a frame at about one per cycle. The tangent in those
  coefficients was about a fifth of that; muz's own tangent, inline, and
  the shapes' tables side by side took the benchmark's 99th percentile
  from 260–280 to 245–260 and left the slurs' unchanged.
- **Affected work:** denser physics: tone holes with the whole lower bore
  attached (about twice the nodes), a finer bore flare, a mass-spring reed
  (which may want 4x oversampling), and ensembles of lattice-class voices.
- **Workaround:** one lattice voice plays live; offline rendering for denser
  models.
- **Desired behavior:** one lattice voice at 5% of a core or less at the
  99th percentile, robustly, on both commands below, without changing
  output. Candidates: running up to four key clusters of one shape in
  vector lanes (slew, lookups and coefficients; a resting or seated lane
  may run with them, as it would compute what it holds), and fewer
  instructions for a seated key's slew. Packing the coefficient refills
  alone gains little: of the 20 holes, a frame moves nine at most, and the
  tangent is no longer a call. Reproduce with
  `cargo run --release --example woodwind_callback -- examples/woodwind-prototypes/lattice-slurs.muz 4 1 3`
  and the same for `examples/benchmarks/lattice-runs.muz`.

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
  pitch flexibility from the player. The tone-hole lattice prototype in
  `examples/woodwind-prototypes` now does this, holds 4 cents across its
  range, and slurs both ways across the break without a chirp or a crack
  and without passing through the low register. An ascent across the break
  still settles in up to 35 ms and dips up to 3.3 dB (as its test measures
  them, over 24 breath seeds; one ascent in thirteen past 30 ms or 3 dB),
  like a short new attack, against 10 ms or less elsewhere: the
  throat note's resonance slides toward the long tube's first mode as the
  fingers seal, so the clarion grows anew (see the prototype's README). Its cost (1.8 times the std clarinet compiled)
  keeps it out of `std`.


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
