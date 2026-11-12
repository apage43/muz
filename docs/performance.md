# Musical performance

Performance controls how written notes become timed attacks, releases, and
expression. Read the [pattern and selector basics](language.md#patterns-and-transformations)
first. The transformations here return patterns that you can reuse and assign to
tracks as usual.

[Documentation index](README.md) · Offline: `muz docs performance`

## Contents

- [Score and performed time](#score-and-performed-time)
- [Gate and pedal](#gate-and-pedal)
- [Dynamics and timing](#dynamics-and-timing)
- [Drums and kits](#drums-and-kits)
- [Per-note expression](#per-note-expression)
- [Piano constraints](#piano-constraints)
- [Playability checks](#playability-checks)
- [Custom performance policies](#custom-performance-policies)

## Score and performed time

Patterns carry exact score positions and written durations, plus performance
metadata. Transformations preserve metadata and give repeated occurrences distinct
local keys. `muz inspect song.muz --view score` shows the writing;
`--view performance --track ID` shows scheduled events. Both use the
[paged inspection interface](workflow.md#inspect-a-song).

`pattern.notes` is an array of records:

| Fields | Meaning |
| --- | --- |
| `at`, `duration` | Score position and written duration, in beats. |
| `pitch`, `velocity`, `release`, `gate` | Pitch, attack intensity, release velocity, and held fraction of duration. |
| `offset`, `release_offset` | Performance displacements, exposed as seconds quantities. |
| `hand`, `voice` | Hand assignment (or null) and musical voice label. |
| `key`, `tags`, `data` | Exact identity, tags, and annotation record. |

`seconds_at(position,timing={})` converts beats/bars using the renderer's tempo
interpretation; seconds pass through unchanged. Share one timing record with the
song, for example `{tempo:120,tempos:[[8b,90]]}`. Defaults are 120 BPM and no
changes. For an exposed note `n`:

```text
attack  = seconds_at(n.at, timing) + n.offset
release = seconds_at(n.at + n.duration*n.gate, timing) + n.offset + n.release_offset
```

Converted times and performed offsets are finite inexact seconds, preserving the
renderer's floating-point precision when combined. Source seconds literals stay
exact, and passing seconds through `seconds_at` preserves their exactness.
Transform and place notes before deriving final automation from these records.

## Gate and pedal

`.gate(0.6)` holds each key for 60% of its written duration. `.scale_gate(0.5)`
halves existing gates, preserving articulation differences: gates 0.4 and 0.8
become 0.2 and 0.4. Both require a positive finite value and preserve placement,
written duration, pedal, and release offsets. Use `.refine(selector,{gate:0.6})`
to set only selected notes.

`pedal(harmony,depth=0.65)` returns a control pattern. Combine it with notes:

```muz
let changes = chords("C F", each = 4b);
let held = stack([changes.gate(0.7), pedal(changes, depth = 0.65)]);
```

The default 30 ms catch repedals at harmonic changes, reduces depth in dense or
low registers, and releases at phrase end. Override `catch`, set `aware=false`,
or choose controller 66 (sostenuto) or 67 (una corda). Use `cc(64,0,at=4b)` for
an explicit local release, or write all catches as controller patterns.
Half-pedal sound depends on the destination; a configured Pianoteq instrument
supports continuous pedal values.

## Dynamics and timing

| Transformation | Effect |
| --- | --- |
| `.dynamics(start,end)` | Shape musical intensity across the pattern. |
| `.humanize(timing=4ms,velocity=0.025,seed=1)` | Share most variation within simultaneous groups, with smaller per-note variation. |
| `.rubato(25ms)` | Displace attacks, releases, and controls inside a phrase while anchoring its ends. |
| `.swing(0.58)` | Change subdivision placement. |
| `.voice("melody")` | Mark a musical role so selectors can shape it independently. |

Use the song's `tempos:[[position,bpm],...]` for a shared ensemble clock.
Rubato changes a pattern's performance offsets, without changing that tempo map.
It guards forward time using a displacement-to-time ratio relative to phrase
length, including for long phrases with decimal amounts such as
`note(60,48b).rubato(38ms)`.

Performance recipes take time quantities such as `4ms`. Humanization is
deterministic for each note's identity, seed, and stream. `humanize`, `drum_feel`
and `std/instrument.variation` key per-note noise by `note_identity(pattern)`:
the note's voice, score onset (`at`) and pitch, plus its occurrence index among
the notes that share all three, in pattern order. Construction paths (`n.key`)
are not used, so adding or removing a hit in a `stack` or `cat` leaves every other
note's variation unchanged. Because `at` is the onset within the pattern being
performed, apply these recipes to the assembled passage: a repeated bar then
varies, while a bar humanized before `repeat` is copied with its realization.
Offsets from earlier performance steps do not change identities, so recipes
compose in any order. To pin identities through edits that move notes in time
or pitch, set `data.identity` (any value) on them, for example
`p.map_notes(fn(n) => {data: merge(n.data, {identity: ["riff", 3]})})`; it
replaces voice, onset and pitch, and notes that share it are again separated by
occurrence. Humanize's shared group noise (voice and onset) still follows the
onset. This identity replaced path keys in November 2026, so existing pieces get
different (equally reproducible) jitter than before. Notes tagged `fixed` keep authored
attack timing under humanize, groove, and drum feel; velocity still varies.
Recipes and their timing/velocity weights are editable in [std/performance](../std/performance.muz).
The source recipes use different noise values from the former native policies;
repeated evaluation of the same source remains reproducible.

## Drums and kits

`drums({kick:"X...X...",snare:"....X..."},span=1bars)` returns a pattern.
Named voices include `kick`, `snare`, `hat`, `open_hat`, `crash`, `ride`, `ride_bell`, and tom
variants. Grid symbols are:

| Symbol | Hit |
| --- | --- |
| `X` | Accent |
| `x` | Ordinary |
| `g` | Ghost |
| `1`–`9` | Explicit intensity |
| `.` | Rest |

Repeat, place, and refine drum patterns like any other material.
`euclidean(hits,steps,pitch=42,span=4b)` creates a Euclidean rhythm. A kit maps
semantic voices to synth or sample instruments.

### Drum feel and articulations

`groove({snare:9ms,hat:-2ms},accents=[1,0.92,1,0.9],grid=1/2b)` applies repeatable
timing and metrical intensity. `drum_feel(3ms,variation=0.045,seed=1)` adds
correlated bar/hit/recovery variation while keeping ghost/accent relationships.
Both read note positions from beat 0 and assume 4b bars. In a mixed-meter song,
pass the song's meter map as `meters` and apply them to the part at its song
positions: `groove(...)` then counts accents from each bar line and
`drum_feel(...)` shares its bar variation within each bar of the map, for
example `.groove({snare: 9ms}, accents = lean, meters = meters)`. The map
includes the starting meter as a change at `0b`, as `a.meter_changes` writes it.

`flam("snare",spread=24ms,grace=0.5)` adds a softer preparatory hit.
`roll("snare",step=1/8b,to=0.9)` subdivides selected extents with a dynamic rise
and alternating stick annotations. Tag or refine hits to choose articulations
such as `rimshot`. [std/grooves](../std/grooves.muz) supplies acoustic and
electronic starting points.

### Rolled chords

`spread(pattern, step=15ms, direction="up", selector="all")` rolls simultaneous
notes the way a pianist does. Selected notes are grouped by exact onset and by
`hand`, so each hand rolls separately and notes without a hand share one group.
Each group is ranked by pitch (equal pitches keep pattern order) and the note of
rank `r` gets `offset + r * step`. With `"up"` the lowest note keeps its attack and
higher notes arrive later; `"down"` starts from the highest. Single-note groups are
unchanged, as are unselected notes. The first note stays on the beat; for a roll
that lands on the beat, pass a negative `step` (e.g. `-15ms`). It adds to any
existing offset, so `humanize` composes before or after it. This is distinct from
`roll`, which subdivides a drum hit.

`voice_top(pattern, amount=0.1, selector="all")` raises the velocity of the highest
note in each onset-and-hand group by `amount` (negative lowers), clamped to
0.001..1. A lone note is its own top voice. Both helpers group once, so cost
stays linear in the number of notes.

### Chokes and voice processing

Kits accept `chokes:[["hat","open_hat","pedal_hat"]]`; that hat group is the
default. A hit releases older samples in its group through a short ramp,
including one-shots. The choke travels on the MIDI channel of the voice it
cuts, so `.channel(9)` on a kit's pattern renders exactly as channel 0 does and
the export carries the chokes on channel 10. Matching velocity zones rotate round robin. See
[sample coverage](instruments.md#key-and-velocity-coverage) for exact layer bounds.
`velocity_track:0.2` reduces double attenuation when recordings already contain
dynamic layers.

A kit voice can carry `{instrument:sample(...),chain:[fx(...)],gain:3,pan:0.1,sends:{hall:-20}}`.
Its gain is relative to the kit track; its chain precedes the track's common
chain. [Kit insert automation](production.md#kit-insert-automation) can control
those common inserts through the logical track ID.

## Per-note expression

Expression attaches controls to a sounding note, independently of overlapping
notes and release tails. A constant or a curve of `[position, value]` points
works. A plain-number position is a phase from 0 to 1 of the note's performed
gate, from attack to release:

```muz
let swelling = phrase("C4:q E4:q").express({
    volume: [[0, 0.7], [0.3, 1], [1, 0.6]],
    tuning: [[0, 0], [0.8, 0], [1, 1]]
}, selector = "last");
```

A phase stretches with the note, so the same curve moves twice as slowly on a
half note as on a quarter. For a gesture with a physical speed, give a position
as time from the performed attack instead: clock time (`150ms`, `0.2s`) or beats
(`1/2b`). Phases and times can be mixed in one curve:

```muz
// The bend arrives 150 ms after the attack on every note length.
let bent = phrase("E4:q G4:h").express({tuning: [[0ms, -2], [150ms, 0], [1, 0]]});
```

A pair `[beats, clock time]` is a clock offset after a beat offset, such as
`[1/4b, 140ms]`: 140 ms after the score beat that falls a quarter beat from the
attack. Use it for a gesture at a written note inside one sounding note (a
slurred group or a tie) that keeps a physical speed. The clock part may be
negative, so `[1/2b, -30ms]` is 30 ms before that beat. (`1/4b + 140ms` is not
a quantity: beats and clock time only combine as a position.)

```muz
// Scoop into the second written note of a slurred half: dip from 30 ms before
// it and arrive 140 ms after it, at any tempo.
let slurred = note(64, 2b).express({
    tuning: [[0, 0], [[1b, -30ms], 0], [[1b, 0ms], -1], [[1b, 140ms], 0]]
});
```

Compilation resolves time positions against each performed note, after timing
offsets, gate, and tempo are known, and stores phases. A time position is a
phase of `time / performed gate`. Beat positions count score beats from the
performed attack through the song's tempo map, so a tempo change inside the note
is honoured. A `[beats, clock time]` pair first locates its beat that way, then
adds the clock time in seconds. Bars are rejected because their length depends
on the meter. The rules are:

- Negative times are an error, except the clock part of a pair, which must
  still resolve at or after the attack. Times of the same kind (clock or beats)
  and phases must each increase in written order; `.express` checks that at
  once. Pairs are ordered only after resolution.
- Points are taken in written order. The first point at or past the note's
  release ends the curve: it is placed at phase 1 with the value interpolated
  from the previous point (or its own value if it is the first), and the points
  after it are dropped. One tagged gesture therefore fits short notes too.
- After resolution the kept points must still increase. A mixed curve such as
  `[[0, 0], [400ms, 1], [0.5, 0]]` is valid on a half note at 120 BPM (400 ms is
  phase 0.4) but an error on a quarter (phase 0.8), and the diagnostic names the
  track, note, both points, and their phases. Likewise
  `[[1/2b, 0], [[1/2b, 300ms], 1], [1b, 2]]` is valid at 60 BPM but not at
  120, where beat 1 comes 50 ms before the pair. Write the later points as times, or end a mixed
  curve with phase 1, which always follows the kept points.

Note `data` keeps time positions as written, so `map_notes` callbacks read
`n.data.expression` with units and can merge it back. Exports and the audio
engine receive only the resolved phases.

[std/gestures](../std/gestures.muz) builds time-positioned curves as source
recipes:

| Recipe | Result |
| --- | --- |
| `bend(from, over = 150ms, to = 0)` | `[[0ms, from], [over, to]]` |
| `vibrato(rate = 5.5Hz, depth = 0.3, delay = 0ms, center = 0, fade = 0ms, steps = 2, points = 30)` | A start point at `delay`, `points - 2` samples of a sine `steps` per cycle with linear segments, growing to `depth` over `fade`, and a return to `center`. |
| `vibrato_lfo(rate = 5.5Hz, depth = 0.3, delay = 0ms, fade = 0ms, curve = 0)` | `{curve: curve, oscillate: {rate: rate, depth: ...}}`: a sine around `curve` evaluated on the control clock, with depth rising from 0 at `delay` to `depth` over `fade`. |
| `follow(curve, next)` | `curve + next`, dropping `next`'s first point if it repeats `curve`'s last position. |
| `at(offset, curve)` | `curve` started `offset` beats after the attack: clock points `t` become `[offset, t]`, beat points `offset + t`. Phases are rejected. |

```muz
use "std/gestures" as g;
let sung = phrase("E4:q G4:w").express({
    tuning: g.follow(g.bend(-2, 150ms), g.vibrato(5.5Hz, 0.3, delay = 150ms))
});
// A slurred group performed as one note: bend in at the attack, then start
// vibrato at the note written one beat in, at its clock rate.
let slur = note(64, 2b).express({
    tuning: g.follow(g.bend(-2, 150ms), g.at(1b, g.vibrato(5.5Hz, 0.3)))
});
```

Points interpolate linearly, so a curve glides from one gesture's last point to
the next one's first; add a hold point (for example `[[1b, -1ms], 0]`) before
a gesture that should start abruptly.

The vibrato needs one point per sample, so it covers a fixed time and then holds
`center`: `delay + ((points - 3) / steps + 1/2)` cycles. The defaults cover 14
cycles, about 2.5 s at 5.5 Hz after the delay; with the two-point bend the curve
uses 31 of the note's 32 points. `steps = 2` is a triangle through the peaks;
larger values are rounder but cover proportionally less time. On a shorter note
the curve ends at the release, so the budget is only reached on long notes.

### Oscillating curves

A control's curve may instead be a record
`{curve: base, oscillate: {rate: 5.2Hz, depth: depth_curve}}`. The value is
`base + depth * sin(2π · rate · t)`, with `t` the performed time from the attack,
and the engine evaluates it on the control clock. `curve` and `depth` are ordinary
values or curves with phase and time positions. `curve` defaults to the control's
neutral value, and depth is at most the control's span. The oscillation costs its
base and depth points plus one point for the rate, whatever the note length:

```muz
use "std/gestures" as g;
// A 3.2 s held note: vibrato grows from 0.8 s, with room left for swells.
let held = note("A4", 16/5b).express({
    tuning: g.vibrato_lfo(5.2Hz, 0.35, delay = 800ms, fade = 1.5s),
    volume: [[0, 0.8], [1, 1.1]],
    brightness: [[0, 0.4], [2s, 0.7], [1, 0.8]]
});
```

The rate is fixed for the note, and the sine starts at the attack. DAWProject
exports an oscillating curve as straight segments, eight per cycle.

A note that writes no curve for a control gets its neutral value on native
destinations, and an oscillating curve without `curve` oscillates around it. A
CLAP instrument is sent only the controls a note writes, so an unwritten control
keeps the plugin's own per-voice default:

| Control | Range | Neutral |
| --- | --- | --- |
| `volume` | 0–4 | 1 |
| `tuning` | −120–120 semitones | 0 |
| `pan` | 0–1; center 0.5 | 0.5 |
| `vibrato` | 0–1 | 0 |
| `expression` | 0–1 | 1 |
| `brightness` | 0–1 | 0.5 |
| `pressure` | 0–1 | 0 |
| Declared custom native controls | 0–1 | the patch's declared default |
| CLAP note controls | `value`: the parameter's plain range; `mod`: an offset within ± its span | nothing sent: the parameter's own value |

So a native patch that maps `brightness` onto a cutoff sounds as voiced at 0.5.

| Destination | Support |
| --- | --- |
| Preset synth or sampler | Volume, expression, pan, and tuning. |
| Native `voice_patch` | All standard controls; brightness, vibrato, and pressure need explicit graph wiring. Up to 25 declared custom controls. |
| CLAP native-note instrument | The seven standard expression kinds, subject to plugin support, and up to 25 [note controls](#per-note-plugin-parameters) on parameters the plugin addresses per note ID. |
| VST3 | Floating attack intensity and initial tuning; subsequent per-note curves need another destination or a split layer. |

Curves survive repeat, transpose, and placement, and remain with already-sounding
notes through compatible reloads. Each note allows at most 32 points across all
controls, counted after time positions are resolved and cut at the release.
`.express` checks standard values and curve shapes immediately; destination
support, custom declaration membership, and custom value ranges are checked during
compilation, including writes made through note `data`. CLAP note-control ranges
come from the plugin and are checked when the graph is prepared (`muz check`,
render, or playback). Unsupported destinations report the track and instrument
kind.

Native expression uses a 128-frame control clock and retains the last delivered
value through release. MIDI file export quantizes notes and does not encode these
curves or invent custom-control messages. Explicit raw bend and pressure messages
keep their independent bytes and timing. See [synthesis](synthesis.md#expression)
for native mapping and custom lanes.

### Per-note plugin parameters

A CLAP instrument can expose plugin parameters as custom note controls. Each
control names a parameter, by the same identity automation lanes use (the
inspected key such as `p40`, the numeric ID, or the name), and a mode:

- `mode: "mod"` sends CLAP parameter modulation (`PARAM_MOD`) to the note: an
  offset in plain units added on top of the parameter's current value, without
  moving that value or its automation. Offsets may span the parameter's range in
  either direction (`-(max - min)` to `max - min`).
- `mode: "value"` sends a per-note parameter value (`PARAM_VALUE`) in plain units
  within the parameter's `min..max`. It replaces the parameter's value for that
  note only; stepped parameters are rounded.

Notes then write the control like any other expression, so phases, time
positions, and the 32-point budget all apply. Envelopes, LFO shapes, and
overlap policy are ordinary source recipes:

```muz
// A per-note envelope written in source: rise to `peak`, then fall back to 0.
fn swell(peak, attack = 40ms, decay = 400ms) {
    [[0ms, 0], [attack, peak], [attack + decay, 0]]
};
let synth = plugin("my-synth", {note_controls: {
    detune: {param: "p40", mode: "mod"},
    cutoff: {param: "Filter Cutoff", mode: "value"}
}});
let line = phrase("C4:h C4:q").express({detune: swell(0.25)});
```

Preparation binds each control to the plugin and rejects, naming the control,
parameter, and the scopes the plugin advertises:

- a plugin without a CLAP-dialect note port, since such a plugin receives MIDI
  notes and has no note IDs;
- an unknown or read-only parameter;
- `value` on a parameter without `CLAP_PARAM_IS_AUTOMATABLE_PER_NOTE_ID`, or `mod`
  on one without `CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID`. When the other mode is
  available the diagnostic suggests it. Surge XT, for example, advertises
  per-note modulation, but not per-note values.

muz never falls back to key, channel, or port addressing. A per-key event reaches
every voice of that pitch, so it cannot separate two overlapping notes of the
same pitch; a plugin that only advertises per-key scopes is rejected rather than
written to globally. `muz devices inspect` lists each parameter's `scopes` for
`value` and `mod` (`global`, `note_id`, `key`, `channel`, `port`).

Each event carries the note's ID, port 0, channel, and key, as its note-on did.
Note IDs come from the track's scheduler: unique per sounding note, never reused
within a run, and identical across block sizes and thread counts. An event at the
attack frame follows that note's note-on. A constant control is sent once at the
attack. A changing curve is stepped every 128 frames from the attack, the same
control clock as other expression, until the note-off. Nothing is sent at or
after the note-off: the last value or offset stays with the voice through its
release, and the plugin ends per-note state with the voice. Because note IDs are
not reused, a later note never inherits it. Automation lanes remain global
`PARAM_VALUE` writes; there is no global modulation lane.

Native devices and VST3 instruments have no per-note parameter addressing.
`note_controls` on a VST3 plugin or on an effect is a compile error, and an
undeclared control on a native instrument is reported as an unknown note
expression. DAWProject export reports each note's curve as
`NOTE_PARAMETER_OMITTED` (strict export fails); MIDI file export does not encode
expression curves.

## Piano constraints

Opt in with `track(...,{policy:"piano",reach:12,strict:true})`; the policy works
with any sound source. Tracks without it are unconstrained. Stack all lanes for
one player into the same logical track so the checker sees the combined demands.

The bounded hand search uses register as a soft preference and preserves explicit
`.hand("left")` / `.hand("right")` choices. It checks held-note range, key count,
span, repeated depressions, and movement against `reach` and `movement` assumptions.
A hand may move `reach` semitones plus `movement` semitones per second
between successive notes; a `piano.movement` diagnostic names the hand, the
departing and arriving pitches, the interval, the time between them and that
limit, for example `left hand moves Bb5 to Ab2 (38 semitones) in 0.099 s;
allowed 16.737 (reach 12 + movement 48/s x 0.099 s)`.
`strict:true` rejects violations. `.hands(reach=12)` exposes assignment as an
authoring transformation.

The finger search retains eight alternatives and stops at 400,000 transitions.
Held notes retain fingers; explicit hands and `annotate(selector,{finger:3})` are
anchors. `inspect --view score` shows the assignments. Failure to find an allocation
and exhaustion of the search budget have different diagnostics. Use `fingering:false`
for span/count/movement checks without finger allocation.

These checks model feasibility under bounded assumptions. They do not prove a
fingering is playable and never delete notes. When flagged, shorten finger-held
durations under pedal, redistribute, [spread](#rolled-chords) the chord, or reduce density.

### Piano preferences

`std/performance.piano_preferences` owns the hand centers, initial finger positions,
search weights and per-finger pitch-class costs. Import it as `perf` and pass
`{playing:merge(perf.piano_preferences,{hand_centers:[50,74]})}` in track options,
or pass `preferences` to `.hands()`. The source `track` helper supplies defaults;
manual piano track records must supply `playing`. Costs must be scalar values
from 0 to 1,000,000; initial positions must be MIDI pitches 0..127. Default thumb
costs cover all five black-key pitch classes. Preferences rank allocations;
explicit hand/finger anchors, held-note constraints and bounded search remain
native. Existing reach and movement checks still report infeasible results.

## Playability checks

Any track can run its own source checks: `checks: [f, ...]` in the track options
names functions called as `f(notes)` during compilation. `notes` is the logical
track's performed note list, the same records `pattern.notes` returns: every kit
voice together (with `voice` and `key`), after `.at`/clock placement and, for
`policy:"piano"`, after hand and finger allocation (`hand`, `data.finger`).
Each record also carries `track`, the logical track id. Positions are score time: `at` and `duration` are beats, `offset` and
`release_offset` are seconds. Controls, raw events, kit voice splitting, chokes,
and anything the instrument or plugin does later are not visible.

A check returns a list of `{code, message, at?, severity?}` records. `code` is a
text of letters, digits, `.`, `_` or `-`; it is reported as `check.<code>`.
`at` is a musical time such as `3b` or `2bar` (default `0b`). `severity` is
`"warning"` (default) or `"error"`, which fails compilation, as a strict piano
violation does. Anything else, including an unknown field, is a compile error that
names the check and track. `muz check` prints each warning alongside the piano
diagnostics, and `--json` lists them. The text form locates every diagnostic as
a one-based bar and beat in the song's `meter` (beats in the meter's unit, so
eighths in 7/8), followed by the score beat that `at`, `pattern.notes` and the
JSON `beat` field use:

```text
check.max_simultaneous.exceeded: drums bar 2 beat 1 (4b): 3 onsets on one grid step (kick snare hat), more than 2
check.monophonic.overlap: bass bar 52 beat 4.25 (207.25b): D3 starts while C3 from 0.25b earlier still sounds; overlap 0.02b
```

When errors stop `muz check`, `muz render` or `muz midi export`, the failure
names each failing check by its code and track, then its errors one per line in
the same form. Strict policy errors are listed separately under
`playing policy failed on <tracks>`:

```text
song.muz: check check.monophonic.overlap failed on violin+violin_spiccato:
  check.monophonic.overlap: violin+violin_spiccato bar 1 beat 2 (1b): D5 (violin_spiccato) starts while G4 (violin) from 1b earlier still sounds; overlap 0.45b
```

[std/checks](../std/checks.muz) supplies `monophonic(tolerance = 0b)` (a note
starting while an earlier onset still sounds by more than `tolerance`, using
`duration * gate`; the message names both pitches, how far apart they start, and
the overlap), `in_range(low, high)` (MIDI pitches outside the inclusive range),
and `max_simultaneous(count, grid)` (more than `count` onsets on one `grid` step
across all voices). `monophonic(tolerance = 1/32b)` accepts legato whose release
only touches the next note. The std checks report warnings; to make a rule
fail compilation, wrap it: `checks.as_error(checks.monophonic())`, or generally
`checks.severity(f, level)` with `"warning"` or `"error"`, which sets every
diagnostic of any check `f` to that level. A wrapped check goes wherever a check
does, in a track's `checks` list or a song check's `check`.
`checks.pitch_name(61)` is `"C#4"`, for messages in your own checks. Rules for a particular instrument or player belong in the
piece, with the same shape:

```muz
use "std/checks" as checks;
fn no_mixed_bows(notes) = map(
    filter(group_by(notes, fn(n) => n.at), fn(onset) =>
        len(group_by(onset, fn(n) => n.voice)) > 1),
    fn(onset) => {code: "violin.bows", message: "legato and spiccato at once", at: onset[0].at}
);
let options = {checks: [checks.monophonic(), checks.in_range(55, 103), no_mixed_bows]};
```

A rule that spans tracks, such as one player split across two programs, is a
song check: `song({..., checks: [{tracks: [...], check: f}]})`. `f` is the same
kind of function as a track check. It receives the performed notes of the named
tracks (as each track's own checks see them) merged in onset order, with
simultaneous onsets in song track order, and each note's `track`
tells them apart. Diagnostics report under the names joined by `+`. An unknown
track name, an empty `tracks` list, a `check` that is not a function, or another
field is a compile error.

```muz
song({
    tracks: [track("violin", legato, bows.legato), track("violin_spiccato", spiccato, bows.spiccato)],
    checks: [{tracks: ["violin", "violin_spiccato"], check: checks.monophonic()}]
})
```

```text
check.monophonic.overlap: violin+violin_spiccato bar 1 beat 2 (1b): D5 (violin_spiccato) starts while G4 (violin) from 1b earlier still sounds; overlap 0.45b
```

`monophonic` names a note's track only when the two notes come from different
tracks.

Checks receive values and return values, so they cannot change the music. All
checks in a song share one evaluation budget of the usual size, separate from
the one that built the song; running out names the check (`'name'` for a
named `fn`, otherwise `checks[i]`) and its track.

## Custom performance policies

`std/performance` owns `scale_gate`, `dynamics`, `humanize`, `groove`, `drum_feel`,
`rubato`, `flam`, `roll`, `spread`, `voice_top`, `pedal`, and `note_identity`. The source prelude exposes these as global
functions and pattern methods. Import the module to use or adapt its helpers:

```muz
use "std/performance" as feel;
let shaped = phrase("C4:q E4:q").map_notes(fn(n) => {
    duration: n.duration * 3/4,
    release_offset: n.release_offset + 2ms
});
let main = feel.displace(shaped, fn(at) => sin(at / 1b) * 5ms);
```

### Sparse event patches

A pattern exposes `.notes`, `.controls`, `.raw` and `.span`.
`map_notes(pattern, callback, selector="all")` invokes the callback once per
selected note and applies the returned sparse record as a patch. Omitted fields
remain exactly as authored. Returning `null` drops a note.
`filter_notes(pattern, callback)` keeps notes for which the callback is true.
`flat_map_notes(pattern, callback, selector="all")` expects a list of patches;
an empty list drops a note and multiple patches create multiple notes. An
unselected note is unchanged. Selectors accept the same tags, voices, records,
unions and predicates as `select`.

Note fields are `at`, `duration`, `pitch`, `velocity`, `release`, `gate`, `offset`,
`release_offset`, `hand`, `voice`, `key`, `tags`, and `data`. The two offsets are
seconds quantities when read, and patches also accept scalar milliseconds.
`offset_ms` and `release_offset_ms` are scalar patch aliases. A patch to `data`
or `tags` replaces that field; use `merge(n.data, {...})` or `n.tags + ["tag"]`
to retain existing contents. `hand: null` removes a hand assignment.
`refine(selector, callback)` accepts sparse callback patches with the same
fields, alongside its original constant record form.

### Identities and expansion

Single-patch transformations preserve the original key. Expansion automatically
uses `original/expand0`, `original/expand1`, etc. unless the patch supplies an
explicit `key`. For an ornament that retains its principal note, return its
original key explicitly. `overlay(patterns)` combines already namespaced
patterns without positional key prefixes and rejects duplicate note identities;
use `.at(position, key="occurrence")` to namespace occurrences before overlay.

### Controllers and raw events

`map_controls` / `flat_map_controls` apply patches to `at`, `offset`, `controller`
and `value`. `map_raw` / `flat_map_raw` apply patches to `at`, `offset` and `bytes`.
All map functions accept `null` to remove an event; flat maps accept empty arrays.
They preserve other event families and the authored span, including trailing
silence. `control(controller, value, at=0b, offset=0ms)` constructs a channel-relative
CC event (including pedal); `cc` remains an explicitly channel-addressed raw event.
Both take a clock-time `offset`, as do the other
[explicit channel events](language.md#explicit-channel-events).

Callbacks execute under the evaluator's existing step/depth bounds. Results are
validated, and each event family is limited to 200,000 events. Invalid fields,
MIDI values, or nonpositive note durations fail at the transformation. Reusable
fragments may have negative local onset coordinates for pickups; final compiled
patterns must have nonnegative score coordinates.

`controls_over(base, layer)` drops each control in `base` whose controller, `at`
and `offset` match a control in `layer`, then stacks `layer` on top. Use it
when a ramp ends exactly where the next level begins; plain `stack` still
rejects accidental equal-time CC conflicts:

```muz
let ramp = stack(map(range(5), fn(i) => control(1, 40 + 14 * i, at = i * 1b)));
let dynamics = controls_over(ramp, control(1, 96, at = 4b));
```

### Coherent time displacement

`displace(pattern, function)` takes a function from score position to a duration
in seconds. It evaluates the displacement at note-on and gated note-off separately,
and at every control/raw event. The release correction accounts for the onset
shift already applied. This keeps matching note releases and pedal releases
aligned. `rubato` is a sinusoidal displacement recipe with a forward-time bound;
custom displacement functions must preserve the intended event ordering.

### Deterministic noise and grouping

`keyed_noise(key, seed=0, stream=0)` returns deterministic noise in `[-1,1]`.
Keys and streams may be strings or compound source values. Distinct streams
allow independent velocity and timing variation without positional randomness.
`group_by(list, callback)` groups equal callback keys, preserving first-group and
within-group order. `keys(record)` exposes record field names. Pedal policy uses
one grouping pass instead of scanning the harmony once per chord; its register,
density and catch choices are ordinary source expressions. Piano hand/reach and
voice-leading solvers remain efficient kernel machinery.

### Neighbor-aware note policies

`std/patterns.map_note_runs(pattern, function, run=fn(n) => n.voice)` supplies
context for source performance rules. The callback receives
`{note, previous, next, run, index, count}` and returns a sparse note patch or
`null` to drop, as with `map_notes`. Boundaries are `null`. Runs are visited in
first-occurrence order, with stable score-onset sorting within each run. Equal
onsets retain original order; second-valued offsets do not change adjacency.
Unvoiced polyphony forms one run with deterministic neighbors, not inferred
melodic strands. Assign voices or supply a custom grouping function (including
compound keys); a constant key requests a global run.

The grouping callback runs once per original note. All contexts come from the
immutable input, so earlier onset, voice or key edits and drops cannot change
later neighbors. Results are applied in original native order through
`map_notes`, preserving omitted fields, controls, raw events, trailing silence
and hidden clip payloads. Empty material invokes neither callback. Collection
work is O(n log n), excluding callback cost, with O(n) intermediate storage.
For example:

```muz
use "std/patterns" as p;
let linked = p.map_note_runs(phrase("C4:q D4:q E4:q"), fn(c) => {
    gate: if c.next == null { 0.8 } else { 1 }
});
```

## Next steps

Derive [production automation](production.md#deriving-automation-from-notes) from
performed notes, or [inspect and audition](workflow.md) the result. General source
helpers also live in [std/music](../std/music.muz) and [std/patterns](../std/patterns.muz).
