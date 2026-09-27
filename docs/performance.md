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
deterministic for its key, seed, and stream. Notes tagged `fixed` keep authored
attack timing under humanize, groove, and drum feel; velocity still varies.
Recipes and their timing/velocity weights are editable in [std/performance](../std/performance.muz).
The source recipes use different noise values from the former native policies;
repeated evaluation of the same source remains reproducible.

## Drums and kits

`drums({kick:"X...X...",snare:"....X..."},span=1bars)` returns a pattern.
Named voices include `kick`, `snare`, `hat`, `open_hat`, `crash`, `ride`, and tom
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

`flam("snare",spread=24ms,grace=0.5)` adds a softer preparatory hit.
`roll("snare",step=1/8b,to=0.9)` subdivides selected extents with a dynamic rise
and alternating stick annotations. Tag or refine hits to choose articulations
such as `rimshot`. [std/grooves](../std/grooves.muz) supplies acoustic and
electronic starting points.

### Chokes and voice processing

Kits accept `chokes:[["hat","open_hat","pedal_hat"]]`; that hat group is the
default. A hit releases older samples in its group through a short ramp,
including one-shots. Matching velocity zones rotate round robin. See
[sample coverage](instruments.md#key-and-velocity-coverage) for exact layer bounds.
`velocity_track:0.2` reduces double attenuation when recordings already contain
dynamic layers.

A kit voice can carry `{instrument:sample(...),chain:[fx(...)],gain:3,pan:0.1,sends:{hall:-20}}`.
Its gain is relative to the kit track; its chain precedes the track's common
chain. [Kit insert automation](production.md#kit-insert-automation) can control
those common inserts through the logical track ID.

## Per-note expression

Expression attaches controls to a sounding note, independently of overlapping
notes and release tails. Positions are phases from 0 to 1 of its performed gate.
A constant or a curve works:

```muz
let swelling = phrase("C4:q E4:q").express({
    volume: [[0, 0.7], [0.3, 1], [1, 0.6]],
    tuning: [[0, 0], [0.8, 0], [1, 1]]
}, selector = "last");
```

| Control | Range |
| --- | --- |
| `volume` | 0–4 |
| `tuning` | −120–120 semitones |
| `pan` | 0–1; center 0.5 |
| `vibrato`, `expression`, `brightness`, `pressure` | 0–1 |
| Declared custom native controls | 0–1 |

| Destination | Support |
| --- | --- |
| Preset synth or sampler | Volume, expression, pan, and tuning. |
| Native `voice_patch` | All standard controls; brightness, vibrato, and pressure need explicit graph wiring. Up to 25 declared custom controls. |
| CLAP native-note instrument | The seven standard expression kinds, subject to plugin support. |
| VST3 | Floating attack intensity and initial tuning; subsequent per-note curves need another destination or a split layer. |

Curves survive repeat, transpose, and placement, and remain with already-sounding
notes through compatible reloads. Each note allows at most 32 points across all
controls. `.express` checks values and curves immediately; destination support and
custom declaration membership are checked during compilation, including writes
made through note `data`. Unsupported destinations report the track and instrument
kind.

Native expression uses a 128-frame control clock and retains the last delivered
value through release. MIDI file export quantizes notes and does not encode these
curves or invent custom-control messages. Explicit raw bend and pressure messages
keep their independent bytes and timing. See [synthesis](synthesis.md#expression)
for native mapping and custom lanes.

## Piano constraints

Opt in with `track(...,{policy:"piano",reach:12,strict:true})`; the policy works
with any sound source. Tracks without it are unconstrained. Stack all lanes for
one player into the same logical track so the checker sees the combined demands.

The bounded hand search uses register as a soft preference and preserves explicit
`.hand("left")` / `.hand("right")` choices. It checks held-note range, key count,
span, repeated depressions, and movement against `reach` and `movement` assumptions.
`strict:true` rejects violations. `.hands(reach=12)` exposes assignment as an
authoring transformation.

The finger search retains eight alternatives and stops at 400,000 transitions.
Held notes retain fingers; explicit hands and `annotate(selector,{finger:3})` are
anchors. `inspect --view score` shows the assignments. Failure to find an allocation
and exhaustion of the search budget have different diagnostics. Use `fingering:false`
for span/count/movement checks without finger allocation.

These checks model feasibility under bounded assumptions. They do not prove a
fingering is playable and never delete notes. When flagged, shorten finger-held
durations under pedal, redistribute, roll, or reduce density.

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

## Custom performance policies

`std/performance` owns `scale_gate`, `dynamics`, `humanize`, `groove`, `drum_feel`,
`rubato`, `flam`, `roll`, and `pedal`. The source prelude exposes these as global
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

Callbacks execute under the evaluator's existing step/depth bounds. Results are
validated, and each event family is limited to 200,000 events. Invalid fields,
MIDI values, or nonpositive note durations fail at the transformation. Reusable
fragments may have negative local onset coordinates for pickups; final compiled
patterns must have nonnegative score coordinates.

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
