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
  load. One voice of the round-8 tone-hole lattice prototype
  (`examples/woodwind-prototypes/lattice-slurs.muz`: 646 nodes, with
  second-order hole shunts, the lip-loaded reed, the tongue and the
  player's tract) costs about 96 over four seconds (2.4% of a core) and
  159–162 at the 99th percentile (4%), least of three renders. A held A4
  on it costs about 130 (99th percentile 153): the audio-rate tier's
  floor, about 3,000 instructions a frame at 2x oversampling, at about
  2.3 a cycle. Two fifths of them are packs of the bore's sections; the
  rest are the reed, tract and radiation filters and delays one by one,
  the lines' writes (two stores a sample, the line and its copy), the
  vibrato's and flutter's sines (libm calls, about 5% of the voice), and
  the frame's guards and bookkeeping. The slurs' 99th percentile is the
  plateau of the few dozen milliseconds after each note change, while the
  seated keys' slews settle alone (eight of them a frame) and the register
  hole's jet moves its filter. A passage of fast slurs across the break,
  `examples/benchmarks/lattice-runs.muz` (ten notes a second), costs 138
  and 203–205 at the 99th percentile (5.1%); its peaks are the frames
  where several keys travel at once. There the settling tier runs each
  moving key's cluster alone: its slew, three shape lookups (cutoff, q and
  depth of the hole's filter) and the filter's coefficients, about 200
  instructions a key a frame at about one per cycle. Packs that keep their
  lanes' state side by side, and cheaper checks of the bounded copy's
  inputs, guards and resting packs, took these from ef9f675's 115 and 189
  (slurs), 167 and 231–250 (runs) and 158 and 180 (held); earlier rounds'
  steps are in the history of this file. The holes' open-path filters cost
  about a tenth of the held note. Round 10 of the lattice (623 nodes: the
  physical bell, round 9's seated open path, no external interaction)
  costs 96–97 and 160–161 on lattice-slurs against main's round-8 model's
  98 and 161–163, alternately, at 64 frames. It drives a closed hole's filter by
  its depth, which is zero once the key is seated, but keeps a leak of
  10^-12 there: driven by an exact zero, those filters cost 1–2 Mc/s more
  on the mean and 5–7 at the 99th percentile (99/166 against 97/161).
  Their states decay toward zero, and an f32 state whose pole factor is
  above one half reaches the smallest subnormal and rounds back to it, so
  it stays subnormal (the engine does not flush them), costs subnormal
  arithmetic every sample, and never lets a pack of such filters rest.
  A step in the bore at a hole also costs 3–4 Mc/s on a held note
  (round 10's first bell stepped at the E/B hole): that junction then
  differs from its neighbours.
- **Tried and set aside:** running up to four key clusters of one shape
  together (slews and refills in vector lanes, each lane's lookups on its
  own, a resting or seated lane running along) raised lattice-slurs' 99th
  percentile from 195 to 212–267 and lattice-runs' from 246 to 256–277:
  holes move a few at a time, so most of a group's work is a lane that
  needed none, and gathering the lanes costs what the vectors save. A closed hole's filter
  cannot rest exactly while its depth is zero: its input (the bore's
  pressure there) keeps moving, so the interpreter's state keeps changing
  and reaches no fixed point, and catching it up when the hole opens means
  running every frame it skipped (nothing shows that an f32 lowpass forgets its
  history exactly). Its product with an exact zero is also a signed zero
  that follows the filter's sign, which a sum of zeros can show. Keeping
  the output requires the model to stop driving a closed hole's filter. When it
  does (its input gated by its key), the filter's state reaches +0 in the
  engine's tests, and a pack of such filters then skips its steps; the
  test costs well under 1 Mc/s on the round-8 graph, where no hole is gated
  yet. In the audio-rate tier, each measured against the same build: a
  line's write index kept in its pack of writes rather than its delay's
  (their lanes rarely match: a junction reads its own lines and writes its
  neighbours') saved 3% of the instructions and cost 1.5% more cycles; a
  delay pack's feedback stored lane by lane for writes that read it so
  cost more of both; walking two of the order's chains at once, for more
  independent work, cost 4–6% and took the runs' 99th percentile past 300;
  an ADSR that skips its divisions once its decay has run out cost 1–2%;
  a moving cubic delay's four weights in one vector (one division instead
  of four) saved 2% on the std oboe and cost the lattice 1.5%.
  Inline sines and exp2 of muz's own would save the libm calls, but glibc's
  `sinf` and `exp2f` are not correctly rounded (about a million sines of
  the oscillator's angles, in [0, 2π), differ from the rounded f64 sine),
  so a project-owned function, like the tangent, would change renders, and
  copying glibc's algorithm exactly would tie the reference to one libm and
  its choice of fused multiply-adds.
- **Affected work:** denser physics: tone holes with the whole lower bore
  attached (about twice the nodes), a finer bore flare, a mass-spring reed
  (which may want 4x oversampling), and ensembles of lattice-class voices.
- **Workaround:** one lattice voice plays live; offline rendering for denser
  models.
- **Desired behavior:** one lattice voice at 5% of a core or less at the
  99th percentile, robustly, on both commands below, without changing
  output. Candidates: the settling tier's work for each moving key, which
  sets the runs' peaks; the audio-rate tier, which sets the held note's
  cost; subnormal states flushed to zero (or a state snapped to zero below
  a threshold), so the lattice's gated filters, now driven by an exact
  zero but for a −240 dB leak, could rest; on the model's side, a vibrato
  or flutter whose depth is zero computing no sine. Reproduce with
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
  settles in 10–35 ms (mean 21) and dips up to 4.0 dB over 24 breath seeds
  and four phases of the old note, a short new attack, as players' legato
  between registers is, and within what players' finger timing gives (the
  prototype's README has the sources and the test the gate). But half its
  slurs down from the clarion to the chalumeau (380 of 756, from written
  B4–C6 to C4–A#4; every one to D4) land on the new fingering's twelfth:
  its second register with the register hole shut is too stable (README,
  `tools/clarinet-lattice.py descents`). That, and its cost (about 1.7
  times the std clarinet compiled), keep it out of `std`.


### A closed vent changes a reed bore's tone with its place

- **Origin:** the oboe's descending-slur fix, 2026-11-14 (Arundo v6h's D5 at
  bar 51).
- **Observed behavior:** `std/physical.pipe` with a `vent` signal is three
  lines joined at the hole; closed (the signal at 0) it should be the one
  line it replaces. Fed a click, passive or through a linear reed, it is:
  the responses match to 0.1 dB wherever the hole is. Driven by the oboe's
  beating reed it is not: a steady tongued D5 (mf) has its second partial
  10.1 dB over its first on a single line, 9.8 with the closed hole at 0.1
  of the long pipe, 8.6 at the octave node std uses (0.29), 5.6 at 0.57 and
  4.0 at 0.7, with the pitch unchanged (within 1 cent), and the same at
  C4-G4 and with the breath noise, drift and vocal tract off. So the oboe's
  closed-key notes were voiced, and their mode balance set, on a split that
  should not matter. Cause not found: the lines' cubic interpolation of the
  reed's full-band flow (two interpolations on the way out instead of one)
  is the suspect.
- **Affected work:** the oboe's lower register tone and how readily it falls
  to its octave; any model that moves a closed hole (the fix keeps the hole in
  place through a slur and then returns it to its usual node, so the tone
  of a slurred-to note drifts by about 3 dB in its second partial over
  `hole_settle`).
- **Workaround:** the closed hole sits where it was voiced except for the
  150 ms after a slur down out of the octave register.
- **Desired behavior:** a closed vent that is transparent with any driver,
  or the reason it is not, and the oboe revoiced on whichever is right.
  Reproduce: `w.patch(w.specs.oboe)` with the cone's `vent_at` held at 0.1,
  0.29 and 0.7 against a `vent` of 0, tongued D5 at 0.6.

### A fast slur down out of the oboe's octave register can still ring its octave

- **Origin:** the oboe's descending-slur fix, 2026-11-14.
- **Observed behavior:** with the octave hole kept in place as the key
  closes, a slur down out of the register usually crosses in a few tens of
  ms, but now and then the octave above rings on: over 1,536 such slurs
  (`tests/cone_descents.rs`'s figures, sixteen seeds, three tempi, four
  dynamics) 27 rang it for 60 ms or more in a row, and 2 slurred sixteenths
  at crotchet = 160 (94 ms) for over half their length (the tests' sweep
  finds one of 72, a semitone from E5 to E♭5 for 70 ms). Before the fix
  the counts were 1,024 and 165.
- **Affected work:** fast oboe runs down through E5; slower slurs (eighths at
  108 and longer) do not lock.
- **Workaround:** none needed for Arundo (its slurs are eighths at 108).
- **Desired behavior:** none for over half a note at any tempo. The model
  plays each note below the key on its own short tube; a real oboist plays
  C♯5 to E♭5 on the long tube with the half hole, so a descent there changes
  no register. Candidates: a half-hole register (the key from C♯5, with the
  calibration redone), or the fixed-geometry bore of the clarinet prototype.

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

### Pianoteq 9.2.4 exports every factory preset as a lean reference

- **Origin:** Kingfisher Line v1 Step 0 (muz-projects, 2026-11-14): capturing
  the electric piano `MKII 70s Fusion`.
- **Observed behavior:** on Pianoteq 9.2.4 build 20260825, `--export-vst3-presets
  DIR --export-presets-filter builtin` writes every factory preset as a
  `LEAN-PRESET` file of about 510 bytes (all of `Factory/Electric`, and also
  `C. Bechstein DG Warm`, which The Long Debt captured on 2026-11-11 before the
  lean check existed). `contrib/plugins/pianoteq-state.py --preset NAME` therefore
  refuses every factory preset. Separately, `select_factory` calls `reject_lean`
  on every identity match before the exact-stem choice, so `--preset "Classical
  Guitar"` fails on `Classical Guitar 12 Strings.vstpreset` even where only the
  exact file mattered.
- **Affected work:** every Pianoteq sound a piece auditions or plays (the EP, the
  nylon guitars, the Bechstein).
- **Workaround:** Kingfisher Line's `prepare.py` runs the standalone headless
  with `--serve` on a private copy of the preferences, calls `loadPreset` and
  `savePreset` (into a temporary user bank, removed afterwards), and captures the
  saved full `.fxp` with `pianoteq-state.py --fxp`. The states load and play.
- **Desired behavior:** headless capture of a factory preset that works on this
  build: for example the helper driving the JSON-RPC `loadPreset`/`savePreset`
  route itself, or reading the full program another way; and the lean check
  applied only to the file the helper finally selects.

### One evaluation budget limits a batch collection of played winds

- **Origin:** Kingfisher Line v1 Step 0: the palette's audition clips.
- **Observed behavior:** a module and all its imports share one 5,000,000-step
  evaluation, and `muz batch` evaluates every top-level binding of the module,
  not just the named collection. A palette module holding the flute's fifteen
  played clips, three reed ensembles, two clarinet lines and two four-part
  `w.ensemble` runs failed with "evaluation budget exceeded" (inside
  `std/physical.response_ir`, called by `w.body` for each bassoon track); any one
  collection alone fits.
- **Affected work:** decision pages and A/B collections of played wind parts,
  which naturally live in one module with shared passages.
- **Workaround:** the palette is split into one module per collection group
  (`clips.muz` shared, `voicing.muz`, `reeds.muz`, `reedtaps.muz`, `band.muz`),
  with every played part behind a function in the shared module.
- **Desired behavior:** a batch that evaluates only the collection it renders
  (or per-output budgets). (`w.body`'s response is now memoized, and
  `MUZ_EVAL_BUDGET` raises the shared budget, but a batch still evaluates every
  collection of the module.)

### A batch output cannot match a kit or a section

- **Origin:** Kingfisher Line v1 Step 0.
- **Observed behavior:** an output's `match` names one track; naming a kit
  (`match: "drums"`) fails with "match names no track 'drums'" and lists the kit's
  voice tracks, and there is no way to match a group of tracks (a three-reed
  section on three tracks) as one candidate.
- **Affected work:** loudness-matched A/B clips of a drum kit or an ensemble,
  which the palette rules ask for ("loudness-matched per role").
- **Workaround:** the drum and reed clips are rendered unmatched and matched by
  hand when encoded.
- **Desired behavior:** `match` accepting a group or kit name (moving the
  members' faders together), or a list of tracks.

### A revision during an SFZ audition loop replays the whole engine

- **Origin:** task "fast plugin servicing", Glass Hearts headless; narrowed by
  task "fast SFZ transport", which made seeks, restarts and loops replay only
  the SFZ instruments.
- **Observed behavior:** while an audition loop is active in a session that
  uses SFZ, every revision (value-only ones included) prepares a separate
  engine and replays the whole graph from zero to the loop start
  (`PreparedTransaction::prepare_runtime_policy`). On Glass Hearts that replay
  costs about 0.35 s per second of timeline before the loop start, on top of
  the revision's own preparation. Transport in sessions with an SFZ
  instrument inside a rack or as an insert still takes the same whole-engine
  path.
- **Affected work:** editing while auditioning a loop late in a piece; each
  edit is applied tens of seconds later.
- **Workaround:** disable the loop while editing, or loop early sections.
- **Desired behavior:** the revision path replays only the SFZ instruments, as
  transport does ([SFZ transport](embedding.md#sfz-transport)), duplicating the
  candidate engine's pristine instruments.

### A control sent right after an outcome can see the state before it

- **Origin:** task "control sequencing", muzdesk's transport check on an SFZ
  piece (set an audition loop, wait for its `ControlApplied`, then restart).
- **Observed behavior:** the callback releases a control's receipt before
  that callback publishes its telemetry. For about one callback after
  `poll` returns `ControlApplied` or `RevisionApplied`, `status()` and every
  `TransportReader` still show the earlier loop, position, running state and
  generation. `restart` and `seek_ticks` clamp to the loop they read there,
  so a restart sent at once goes to zero, and with SFZ its prepared replay
  also clears the new loop. Waiting for the reader's generation to change
  does not help when the baseline was itself read during an earlier
  control's lag: the earlier control's publication satisfies the wait.
- **Affected work:** host transport actions that chain controls, such as
  "loop this section and play it from its start".
- **Workaround:** sleep a few hundred milliseconds between controls.
- **Desired behavior:** once `poll` returns a terminal outcome, every later
  control and every later reading sees that operation's effect.
