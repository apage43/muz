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

### Bounded history rejects some piano passages as out of time order

- **Origin:** Glass Hearts v14 plan, step A0 (tone auditions), at 9aaec64.
- **Observed behavior:** `muz render song.muz --section solos --history 8`
  fails with `invalid static audio graph: performed events must be time
  ordered`. Full history renders. The failure depends on the bound: 6, 7, 8,
  8.5, 9, 10 and 12 s fail; 4, 5, 7.5, 11 and 30 s render. `muz stems --track`
  isolates it to the `piano` track (Pianoteq 9 VST3, `policy: "piano"`,
  `strict: false`, step automation on `piano.out` and a highpass cutoff). It
  persists with `--threads 1`, with `--solo piano`, with `--start 135 --seconds 4`
  instead of `--section`, and with the pedal controllers removed. It still fails
  when the track keeps only the notes of bars 67–68, sliced into place. Those
  notes are three-note chords with equal lengths and starts spread by under
  1 ms (`humanize`), so their ends are a few ticks apart, and their performed
  start order differs from source order. Each failing
  bound starts while one of these chords is sounding, but so does the passing
  11 s bound, so the trigger is not simply a held chord. A standalone file with
  the same notes (exact performed ticks, or grid plus `offset_ms`), the same
  plugin, policy and automation shapes did not reproduce it.
- **Affected work:** quick section renders of the solos for listening and
  measurement. `--history` is documented as composing with any section, so a
  failure that depends on the bound is unexpected.
- **Workaround:** pick a bound that happens to render (7.5 s here) or use full
  history.
- **Desired behavior:** any bound renders. Retriggered notes and their ends are
  ordered by performed time, or the bound reports which events conflict.
- **Reproduction:** at muz-projects `electronic/glass-hearts` (v13 source),
  `muz stems song.muz --section solos --history 10 --track piano -o stems/`.

### Overlap checks name notes by source path, not by pitch or bar

- **Origin:** Glass Hearts v14, step A1 (track checks), at 9aaec64.
- **Observed behavior:** `std/checks.monophonic()` reports an overlap as
  `check.monophonic.overlap: bass beat 207: seq7/seq1/stack6/note starts while
  seq7/seq1/stack5/note still sounds`. The two notes are named by their
  pattern-construction paths (`n.key`), and the position is a score beat. The
  message does not give either pitch, the bar, or by how much the notes
  overlap. Here the overlap was 0.02 beats (an eighth at gate 0.54 on sixteenth
  11 reaching into a run's first sixteenth), which took a separate `muz eval`
  over `part.notes` to find.
- **Affected work:** triaging a check that fires on existing material: deciding
  whether it is a real double-stop, a slip, or a few milliseconds of release
  that a player would never notice.
- **Workaround:** filter `part.notes` around the reported beat with `muz eval`
  and read pitches, starts and gates by hand.
- **Desired behavior:** check diagnostics locate notes musically: pitch names,
  a bar and beat position, and for overlaps the overlap length. Possibly a
  tolerance argument (`monophonic(tolerance = 1/32b)`) so legato that only
  touches is not reported as two voices.

### `muz eval` reads only a file, so auditing a piece's part needs a scratch module

- **Origin:** Glass Hearts v14, workstream R (drum limb check, bass overlap
  fix), at 9aaec64.
- **Observed behavior:** `muz eval <SOURCE>` takes a path and nothing else: no
  expression argument and no source on standard input. Running a track check
  over a part (`d.limbs()(d.part.notes)`, `checks.monophonic()(b.part.notes)`)
  outside `muz check` meant writing a module in a scratch directory whose `use`
  lines give the piece's files by absolute path, since relative paths resolve
  against the scratch file. The v14 planner, which may not create files, could
  not evaluate anything at all for the same reason.
- **Affected work:** quick audits of one part against a check or a rule before
  wiring it into `song.muz`; inspecting note records around a reported beat.
- **Workaround:** a scratch `.muz` file with absolute `use` paths per question.
- **Desired behavior:** `muz eval` accepts an expression (or `-` for standard
  input) evaluated relative to a given module, for example
  `muz eval drums.muz --expr 'limbs()(part.notes)'`.

### Track checks cannot state a rule that spans two tracks

- **Origin:** Glass Hearts v14, workstream V (violin), at 9aaec64.
- **Observed behavior:** a track's `checks` receive only that track's performed
  notes. Ilse is one violinist on two tracks (legato and spiccato Sonatina
  programs, which need separate instruments), and the rule that matters, that
  the two bows never sound together, spans both. No `song`-level or
  multi-track check exists, so it cannot be a check.
- **Affected work:** any one-player-several-programs part (bow strokes,
  keyswitch-free articulation tracks, a guitarist split across amp channels),
  and piece-wide rules such as two players never doubling a note.
- **Workaround:** a source `assert` in `violin.muz`:
  `len(checks.monophonic()(legato_notes.notes + spiccato.notes)) == 0`. It
  reads source notes, not performed ones (timing offsets and track-level
  performance are not seen), and a failure aborts compilation with one message
  instead of reporting located diagnostics alongside the other checks.
- **Desired behavior:** a check that receives several tracks' performed notes,
  for example `song(..., checks: [{tracks: ["violin", "violin_spiccato"],
  check: checks.monophonic()}])`, reported by `muz check` like track checks.

### Humanize jitter in untouched notes moves when other notes are added

- **Origin:** Glass Hearts v14, workstream K (keys), at 9aaec64.
- **Observed behavior:** `std/performance` `humanize` takes 15 % of each note's
  timing and 35 % of its velocity variation from `keyed_noise(n.key, …)`, and
  `n.key` is the construction path (`…/stack6/note`). A passage built as one
  `stack` of a flat hit list (here the piano's chorus `grid`, a `stack` over
  `m.cat` of every bar's hits) numbers its children by list position, so
  adding or removing a hit in one bar renumbers every later hit in the passage.
  Changing only the open bars of a chorus (bars 2, 3, 6 and 7) moved the
  untouched lock bars 4, 5 and 8 by ±1 MIDI velocity and ±1 tick.
- **Affected work:** a revision's regression check that the note diff is
  confined to the edited bars. The exported MIDI can't show that, so each
  difference in an untouched bar must be read by hand to tell jitter from a
  real change. It also means an edit can't be heard in isolation: neighbouring
  bars are re-performed slightly differently.
- **Workaround:** accept the drift, and compare bars by hand.
- **Desired behavior:** per-note variation keyed by something stable under
  unrelated edits (for example onset, pitch and voice, with the path only to
  break ties), or a documented way for source to give notes stable keys.

### `drum_grid` hits lose their strike symbol, so a strum's stroke kind is read from velocity

- **Origin:** Glass Hearts v14, workstream G (guitar chorus strum), at 9aaec64.
- **Observed behavior:** `drum_grid(lanes, voices, articulations, gate, span)`
  decodes each symbol to a velocity (or a rest) and returns notes that carry only
  pitch, time and velocity. A strum written as a grid (`X` ringing accent on four
  strings, `x` short chord, `k` chuck on three strings, `c` the verse's two-string
  chuck) has to be expanded in `flat_map_notes`, and the callback can tell the
  stroke kinds apart only by velocity bands (`< 0.4`, `< 0.55`, `> 0.8`). Adding
  the chorus chuck meant giving it a velocity no other symbol uses and re-banding
  every existing symbol; scaling the part's dynamics has to happen after the
  callback, or the bands misread the strokes. Building a grid from the piece's
  rhythm contract (accents on `m.kick_a` or `m.back` over a fill) also means
  folding `chars()` back into text with `fold(…, "", fn(a, b) => a + b)`, since
  `drum_grid` takes only text and there is no `join`.
- **Affected work:** any instrument whose grid symbols mean articulations rather
  than loudness (strums, chucks, ghost strokes, mutes), and grids computed from
  shared rhythm data.
- **Workaround:** unique velocities per symbol and threshold tests on the raw
  hit velocity in the callback; `fold` to rebuild the text.
- **Desired behavior:** decoded hits keep the strike symbol (for example as a
  tag or `n.data.symbol`), so a callback or selector (`tag:k`) chooses the
  stroke by name; and a list-of-symbols form of the grid (or a text `join`) for
  grids built from data.
