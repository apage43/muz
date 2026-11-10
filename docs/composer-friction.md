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

### Arbitrary CLAP parameter lanes cannot carry per-note modulation scope

- **Origin:** PALCO modulation reopen v017, M03 feasibility investigation (2026-11-03); PM-serialized from the accepted research report.
- **Observed behavior:** Inspected current `src/audio/clap.rs` authored parameter lanes encode global CLAP `PARAM_VALUE` with note_id/port/channel/key=-1; no authored `PARAM_MOD` encoding is exposed. Surge XT 1.3.4 Pitch and Unison Detune descriptors advertise per-note modulation (flags 31776), but arbitrary parameter lanes cannot independently address overlapping notes. Standard per-note expression covers seven fixed controls and does not supply this mapping. Installed CLI probes establish global target validation, not per-note DSP behavior.
- **Affected work:** Exact reconstruction of graphs that require independent per-voice arbitrary plugin modulation is obstructed. Applicability to saved PALCO remains unresolved: its two recovered host modulators have polyphonic_mode=false, while full scope/source semantics still need calibration. This is not a claim that PALCO requires an engine change.
- **Workaround:** For proven global graphs, derive one prepared curve per parameter from final performed note data with explicit overlap/reset policy. Splitting notes into separate plugin instances changes shared voice/effect/state behavior and is not an established equivalent for per-note modulation.
- **Desired behavior:** Expose a small general scoped parameter-event mechanism with stable parameter identity, note identity, sample-offset timing, value-versus-modulation semantics and capability validation. Keep EG/waveform/overlap policies in source. No special PALCO builtin is requested; no engine implementation is authorized by this report.


### Playability checks exist only for piano

- **Origin:** Glass Hearts v7 band rewrite (electronic/glass-hearts), 2026-11-09.
- **Observed behavior:** `policy: "piano"` reports reach, key count and movement problems in `muz check`. No other instrument has an equivalent, and there's no hook through which source can report its own playability warnings. A drum part needing three hands at once, overlapping notes in a monophonic bass line, a guitar grip outside the fretboard, and a violin sounding its legato and spiccato programs at once all check clean.
- **Affected work:** A brief that requires every part to be physically playable by its player can only be verified by hand. v7 needed a separate `muz eval` audit script (limb count per sixteenth, monophonic overlaps, ranges, articulation exclusivity) that `muz check` never runs.
- **Workaround:** A scratch `.muz` audit evaluated with `muz eval`, importing each player's module and filtering `pattern.notes` for violations.
- **Desired behavior:** A small general mechanism through which a track can name a source-level check function that receives the track's performed notes and returns diagnostics. `muz check` would report these alongside `piano.movement`. Specific instrument rules (limbs, strings, ranges) stay in `std/` or the piece.


### Records cannot be built with computed keys

- **Origin:** Glass Hearts v13 chord table (electronic/glass-hearts, `material.muz`), 2026-11-10.
- **Observed behavior:** There is no `{[key]: value}` syntax and no builtin that builds a record from key/value pairs or maps over a record's values. `index_by(list, f)` computes keys but keeps the list items as the values.
- **Affected work:** The F#-minor entries of the chord table are the E-minor entries moved up a whole step, keyed by their transposed spellings. They could not be derived; ten entries were written out by hand next to an `assert` that recomputes each one.
- **Workaround:** A literal record plus a checking `map` over `keys(...)` with `assert`.
- **Desired behavior:** A small general constructor such as `record(pairs)` (or `index_by` with a value function), so derived lookup tables can be computed in source.


### Pattern dumps in `muz eval` print note times as bare numbers

- **Origin:** Glass Hearts v13 lead phrasing (electronic/glass-hearts, `synth.muz` `sing`), 2026-11-10.
- **Observed behavior:** `muz eval` prints quantities reached through `.notes`, records, and lists with their unit (`"1b"`, `"0.75b"`). Evaluating a whole pattern (`muz eval` of `phrase("C4:q D4:e.")`) still prints its serialized form, where `at`, `duration`, and `span` are bare beat numbers.
- **Affected work:** Reading metric values off a pattern dump suggests scalar arithmetic that fails in callbacks.
- **Workaround:** Inspect `pattern.notes` rather than the pattern itself; documented in the language guide.
- **Desired behavior:** A pattern dump prints note times with units, without changing the serialization used elsewhere.

### No recipe for a rolled (spread) piano chord

- **Origin:** Glass Hearts v13 keys (electronic/glass-hearts, `keys.muz`), 2026-11-10.
- **Observed behavior:** The piano-constraint docs suggest "roll" for a flagged chord, but the prelude's `roll(pattern, selector, step, to)` is a drum roll that subdivides a note into repeated hits. Nothing spreads the attacks of a chord's simultaneous notes from the bottom up. A `map_notes` callback sees one note at a time, so it cannot rank a note within its chord.
- **Affected work:** A pianist's rolled left hand, here 12–18 ms per note in all the exposed piano, and a slow roll on the final chord. A large part of what makes a written piano part sound played rather than sequenced.
- **Workaround:** A `voiced(pitches, at, dur, vel, top, roll, hand)` helper builds each chord note by note, sorted, and patches `offset_ms` by rank. It also adds the top-voice accent. Every chord has to be constructed through it rather than written as phrase text.
- **Desired behavior:** A source recipe in `std/performance` (for example `spread(pattern, step = 15ms, direction = "up", selector)`) that groups notes by onset and hand and offsets them by pitch rank, alongside a matching top-voice emphasis helper.

### Per-note expression curves have only phase positions

- **Origin:** Glass Hearts v13 guitar solo (electronic/glass-hearts, `guitar.muz`), 2026-11-10.
- **Observed behavior:** `.express` curve positions are phases (0–1) of the note's performed gate, and there is no way to give a point in time (milliseconds or beats from the attack) or a rate. One bend-and-vibrato curve therefore bends twice as slowly and wobbles at half the rate on a half note as on a quarter note, and at a quarter of the rate on a whole note. The sampled guitar has no vibrato control of its own, so finger vibrato is the `tuning` curve itself.
- **Affected work:** Guitar bends and finger vibrato (and any idiomatic per-note gesture with a physical speed: scoops, falls, string vibrato) on notes of different lengths. A guitar part needs one shape per note length, which the composer has to re-derive by hand.
- **Workaround:** Separate tags and curves per length (`vib`/`vibh`, `bendv`/`bendvh` in `guitar.muz`) with the points recomputed so the bend arrives in about 150 ms and the vibrato stays near 5.5–6 Hz; within the 32-point budget.
- **Desired behavior:** Curve points that can be given in time from the attack as well as in phase (for example `[[0ms, -2], [150ms, 0], [1, 0]]`), or a source-level helper that builds a time-based curve from a note's duration and tempo, so one gesture has the same speed on any note.

### The collections reference omits `group_by`, and the budget error doesn't point to it

- **Origin:** Glass Hearts v13 rhythm section audit (electronic/glass-hearts, `drums.muz`/`bass.muz` playability harness), 2026-11-10.
- **Observed behavior:** `language.md` "Collections and strings" lists `map`, `filter`, `fold`, `sort_by`, `range` and `len`; `group_by` and `index_by` are only mentioned in `performance.md` and in `std/` source. Grouping 1,750 drum hits by onset with `fold` (rebuilding the list on each append) and matching 160 kicks against 860 bass notes with nested `filter` both stopped with `evaluation budget exceeded (5000000 operations)`, whose help says only "simplify the expression, or split the work into smaller definitions".
- **Affected work:** A per-sixteenth playability audit (limbs per step, kick/bass lock) over a whole 138-bar part, which is exactly the source-level check the piano-only policy leaves to the composer. The audit was first moved to Python before `group_by` was found.
- **Workaround:** `group_by(notes, fn(n) => round(n.at / 1b * 4))` for the limb count and `index_by(...)` on the bass onsets as a lookup; the whole audit then runs well within budget.
- **Desired behavior:** List `group_by` and `index_by` with the other collection functions in `language.md`, and let the budget diagnostic name the usual remedies (grouping or indexing instead of nested scans over note lists).
