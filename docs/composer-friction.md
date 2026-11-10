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

### `eq` shelf modes are undocumented

- **Origin:** Glass Hearts mix revision (electronic/glass-hearts), 2026-11-08.
- **Observed behavior:** `muz devices inspect eq` lists `mode` with range 0–2 and no value names, and no guide mentions shelves. Only reading `src/audio/device/advanced.rs` shows that 0 is a peak, 1 a low shelf and 2 a high shelf.
- **Affected work:** Broad tonal moves on a bus or master (air, low-end weight) look impossible from the docs, which invites stacking wide peaks instead.
- **Workaround:** Read the engine source, then use `fx("eq", {frequency_hz: 9000, gain_db: 2, q: 0.7, mode: 2})`.
- **Desired behavior:** Document the mode values in [production.md](production.md#native-effects) and name them in `muz devices inspect` output.

### Strings cannot be indexed or split into characters

- **Origin:** Glass Hearts v7 band rewrite (electronic/glass-hearts), 2026-11-09.
- **Observed behavior:** `"c.X."[2]` fails with `indexing needs list or record`, and no builtin or `std/` helper splits text into characters. `len` works on text, but nothing reads one character of it.
- **Affected work:** A guitarist's sixteenth-note strum grid (`"c.Xc.cXc.cXc.cXc"`: accent, chuck, rest) is the natural way to write rhythm-guitar comping, the same way `drums()` lanes are written. Without character access, a piece can't decode its own grid notation for anything except drums.
- **Workaround:** Decode the grid with `drum_grid({g: pat}, {g: 60}, {"X": 0.84, "x": 0.64, "c": 0.3, ".": null}, 0.5, 1bars)`, then classify each decoded note by its velocity and expand it with `flat_map_notes` (`guitar.muz`, `comp`). This misuses velocity as a symbol channel and depends on drum-specific gate behavior.
- **Desired behavior:** Character access for text (indexing, or `chars(text)` returning a list of one-character strings), so source can write its own grid decoders; `drum_grid` could then be source in `std/`.

### Playability checks exist only for piano

- **Origin:** Glass Hearts v7 band rewrite (electronic/glass-hearts), 2026-11-09.
- **Observed behavior:** `policy: "piano"` reports reach, key count and movement problems in `muz check`. No other instrument has an equivalent, and there's no hook through which source can report its own playability warnings. A drum part needing three hands at once, overlapping notes in a monophonic bass line, a guitar grip outside the fretboard, and a violin sounding its legato and spiccato programs at once all check clean.
- **Affected work:** A brief that requires every part to be physically playable by its player can only be verified by hand. v7 needed a separate `muz eval` audit script (limb count per sixteenth, monophonic overlaps, ranges, articulation exclusivity) that `muz check` never runs.
- **Workaround:** A scratch `.muz` audit evaluated with `muz eval`, importing each player's module and filtering `pattern.notes` for violations.
- **Desired behavior:** A small general mechanism through which a track can name a source-level check function that receives the track's performed notes and returns diagnostics. `muz check` would report these alongside `piano.movement`. Specific instrument rules (limbs, strings, ranges) stay in `std/` or the piece.

### `chord()` silently misreads `7sus4`, `#9`, `#11` and `69`

- **Origin:** Glass Hearts v13 harmony contract (electronic/glass-hearts, `material.muz`), 2026-11-10.
- **Observed behavior:** `src/music.rs` `chord()` matches qualities by substring and never rejects unknown text. `chord("B7sus4") == chord("B7")` (the `sus4` is only seen at the start of the quality, so the 3rd stays and the 4th is dropped). `chord("E7#9")` is `[52,56,59,62,66]`, a natural 9. `chord("C7#11")` is `[48,52,55,58,62,65]`, natural 9 and 11 instead of a ♯11. `chord("C69")` is `[48,52,55,58,57,62]`: a 6/9 gains a ♭7, and the result is unsorted. None of these warns.
- **Affected work:** Fusion harmony is written in exactly these symbols (sus dominants, altered 9ths, Lydian ♯11, 6/9 chords). A misspelled chord feeds `chords()`, the pedal's harmony and any part that voices `chord(sym)`, and the wrong notes are only found by printing every pitch set.
- **Workaround:** Avoid the four spellings: sus dominants as slash chords (`A/B`, `C/D`), no altered 9ths or ♯11 chords, add9 instead of 6/9. Every symbol in the form was checked with `muz eval` against its intended pitch set.
- **Desired behavior:** Parse these qualities correctly (at least `7sus4`, `9sus4`, `#9`, `b13`, `#11`, `69`/`6/9`), return pitches in ascending order, and reject a quality the parser does not fully consume with a diagnostic naming the unread part.

### `kit()` silently fills unmapped standard voices with synth presets

- **Origin:** Glass Hearts v13 drum lanes (electronic/glass-hearts, `sounds.muz` `kit_band`), 2026-11-10.
- **Observed behavior:** `kit(name, params)` is `merge(kit_defaults, …)`, so a sample kit that maps only some voices still answers `rimshot`, `pedal_hat`, `closed_hat` and `tom` with `synth("snare")`, `synth("hat")` and `synth("tom")`. `kit_band` (all Virtuosity recordings) had no rimshot or pedal hat, yet a `rimshot:` or `pedal_hat:` lane would have checked clean and played a synthesized drum in the middle of an acoustic kit. Conversely, the Virtuosity pack records a ride bell, but the standard `drum_voices` table has no `ride_bell`, so that lane needs a custom `voices` map on every `drums()` call.
- **Affected work:** Adding lanes to an acoustic kit: nothing reports that a lane is not backed by the kit's own instruments, and the mistake is only audible.
- **Workaround:** Map every lane the part uses explicitly in the kit, and export `merge(drum_voices, {ride_bell: 53})` from `sounds.muz` for `drums(…, voices = s.band_voices)`.
- **Desired behavior:** A way to build a kit without the synth defaults (or a `muz check` note when a kit voice used by a part comes from the defaults), and a `ride_bell` entry in the standard voice table.

### Records cannot be built with computed keys

- **Origin:** Glass Hearts v13 chord table (electronic/glass-hearts, `material.muz`), 2026-11-10.
- **Observed behavior:** There is no `{[key]: value}` syntax and no builtin that builds a record from key/value pairs or maps over a record's values. `index_by(list, f)` computes keys but keeps the list items as the values.
- **Affected work:** The F#-minor entries of the chord table are the E-minor entries moved up a whole step, keyed by their transposed spellings. They could not be derived; ten entries were written out by hand next to an `assert` that recomputes each one.
- **Workaround:** A literal record plus a checking `map` over `keys(...)` with `assert`.
- **Desired behavior:** A small general constructor such as `record(pairs)` (or `index_by` with a value function), so derived lookup tables can be computed in source.

### Mono legato does not say whether abutting notes glide

- **Origin:** Glass Hearts v13 lead phrasing (electronic/glass-hearts, `synth.muz` `sing`), 2026-11-10.
- **Observed behavior:** [Continuing a mono voice](synthesis.md#continuing-a-mono-voice) says the last *overlapping* attack takes over the held voice. It does not say what happens when a note's gated release falls exactly on the next attack (`gate: 1`), which is how a written slur is normally entered. Whether that pair glides depends on how equal-time note-off and note-on are ordered, and neither the docs nor `inspect --view performance` state the order.
- **Affected work:** Slurring pairs in a sung lead. A slur that silently re-attacks instead of gliding is only audible, so every slur needs a render to confirm, or a guess.
- **Workaround:** Slurred notes use `gate: 1.05`, a clear overlap, which the mono policy documents as safe (the earlier note-off cannot release the new owner).
- **Desired behavior:** Document the ordering of a note-off and a note-on at the same instant on one track, and whether a mono voice treats them as legato.

### Note times read as plain numbers in `muz eval` but are beat quantities in callbacks

- **Origin:** Glass Hearts v13 lead phrasing (electronic/glass-hearts, `synth.muz` `sing`), 2026-11-10.
- **Observed behavior:** `muz eval` prints `phrase("C4:q D4:e.").notes[1].at` and `.duration` as `1` and `0.75`, with no unit. Inside a `map_notes` or `map_note_runs` callback the same fields are beat quantities, so `n.duration < 3` fails with `incompatible units Beat and Scalar`, and `n.at % 4` has to be written `n.at % 4b`.
- **Affected work:** Writing metric performance rules (position in the bar, note length thresholds) from what `muz eval` shows. The printed value suggests scalar arithmetic that then fails.
- **Workaround:** Convert once with `fn beats(q) = q / 1b` and do the rule arithmetic on scalars.
- **Desired behavior:** `muz eval` prints quantities with their unit (`1b`, `0.75b`), as source writes them.

### No recipe for a rolled (spread) piano chord

- **Origin:** Glass Hearts v13 keys (electronic/glass-hearts, `keys.muz`), 2026-11-10.
- **Observed behavior:** The piano-constraint docs suggest "roll" for a flagged chord, but the prelude's `roll(pattern, selector, step, to)` is a drum roll that subdivides a note into repeated hits. Nothing spreads the attacks of a chord's simultaneous notes from the bottom up. A `map_notes` callback sees one note at a time, so it cannot rank a note within its chord.
- **Affected work:** A pianist's rolled left hand, here 12–18 ms per note in all the exposed piano, and a slow roll on the final chord. A large part of what makes a written piano part sound played rather than sequenced.
- **Workaround:** A `voiced(pitches, at, dur, vel, top, roll, hand)` helper builds each chord note by note, sorted, and patches `offset_ms` by rank. It also adds the top-voice accent. Every chord has to be constructed through it rather than written as phrase text.
- **Desired behavior:** A source recipe in `std/performance` (for example `spread(pattern, step = 15ms, direction = "up", selector)`) that groups notes by onset and hand and offsets them by pitch rank, alongside a matching top-voice emphasis helper.

### Lists cannot be sliced

- **Origin:** Glass Hearts v13 keys (electronic/glass-hearts, `keys.muz`), 2026-11-10.
- **Observed behavior:** Patterns have `.slice(start, end)`, but lists have no slice syntax (`list[0:4]`) and no `take`/`drop`/`slice` builtin. Taking the first four bars of a symbol list, or dropping the lowest note of a voicing, needs `map(range(to, from), fn(i) => list[i])`.
- **Affected work:** Working with per-bar harmony lists (the first half of a pre-chorus, all but the last bar) and with voicings (rotating a chord's lowest note up an octave to fit one hand).
- **Workaround:** A local `part_of(list, from, to)` built on `map` and `range`.
- **Desired behavior:** A small list slice, as syntax or as a builtin `slice(list, from, to)` with the same half-open convention as `range`.

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
