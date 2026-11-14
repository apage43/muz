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
  interpreter's, and repeated bore sections run four to a vector. One voice
  of the tone-hole lattice prototype
  (`examples/woodwind-prototypes/lattice-slurs.muz`: 563 nodes, with the
  stepped and flared bore, the lip-loaded reed and the player's tract)
  costs about 105 over the four seconds of the command below (2.6% of a
  core) and 150 while it sounds (3.7%). Its 99th percentile is 185–215
  (4.6–5.4%; machine load moves it), and the cylindrical 523-node bore of
  the same engine reaches 162–168. The peaks are the first few blocks after
  each note change, while keys travel and the reed and tract coefficients
  follow the breath (`tan` per frame), and the attack's plateau. A passage
  with more note changes per second than this one would raise the 99th
  percentile further. The round-7 lattice (642 nodes: second-order hole
  shunts, the little finger's hold, the tongue and the tract's controls)
  costs 122–125 at 64 frames and 195–237 at the 99th percentile, against
  107–108 and 187–214 for the 563-node lattice, measured alternately.
- **Affected work:** denser physics: tone holes with the whole lower bore
  attached (about twice the nodes), a finer bore flare, a mass-spring reed
  (which may want 4x oversampling), and ensembles of lattice-class voices.
- **Workaround:** one lattice voice plays live; offline rendering for denser
  models.
- **Desired behavior:** one lattice voice at 5% of a core or less at the
  99th percentile, robustly, without changing output. Candidates: packing
  the settling key clusters' coefficient refills together, or a
  project-owned correctly rounded `tan` shared by the interpreter and the
  compiled code. Reproduce with
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
