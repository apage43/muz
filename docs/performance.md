# Musical performance

Patterns carry exact beat positions, duration, gate, velocity, release velocity, voice, hand, tags and arbitrary annotation data. Composition transformations preserve metadata and give repeated occurrences distinct local keys. `inspect --view score` pages this writing; `--view performance --track ID --start-tick A --end-tick B` pages scheduled notes/controls and their retained annotations. Both return `{revision, view, rows, total, next}`; use `--offset NEXT` to continue. `tag("last","echo")` and `annotate(selector,{...})` address musical events before production lowering.

`policy:"piano"` assigns unmarked hands with a bounded attack-group search, using register as a soft preference and keeping explicit `.hand("left")` / `.hand("right")` choices. Stack all lanes for one player into the same logical track. The checker aggregates held notes across those lanes, checking acoustic range, key count, span, repeated depressions and movement against `reach` and `movement` assumptions. `strict:true` rejects violations. `.hands(reach=12)` exposes assignment as an authoring transform. These checks are a model of feasibility, not a fingering proof. Shorten finger-held durations under pedal, redistribute, roll, or reduce density when flagged; checking never deletes notes.

`gate` controls key release separately from sustain. `.gate(value)` sets an absolute fraction of written duration; `.scale_gate(factor)` changes existing gates proportionally, preserving varied articulation. `pedal(harmony,depth=0.65)` returns controls: combine them with notes using `stack([notes,pedal(changes)])`. Its default 30ms catch repedals on harmonic changes, reduces depth in dense/low registers, and releases at phrase end. Override `catch`, `aware=false` or `controller=66` (sostenuto) / `67` (una corda). Use `cc(64,0,at=4b)` for explicit local releases, or write all desired catches using CC patterns. Half-pedal sound depends on the destination; Pianoteq is the configured continuous-pedal instrument.

`dynamics(start,end)` shapes musical intensity; `.voice("melody")` and selectors let melodic and accompaniment parts receive different treatment. `.humanize(timing=4ms,velocity=0.025,seed=1)` shares most variation within simultaneous groups, then adds a smaller note component. The `fixed` tag protects timing. `.rubato(25ms)` adds an anchored phrase displacement to attacks, releases and controls, stealing and returning time inside the phrase; it does not change the ensemble tempo map. `.swing(0.58)` changes subdivision placement. Song `tempos:[[position,bpm],...]` changes the global clock; keep an ensemble on that shared clock when it should breathe together.

Drum grids use named voices (`kick`, `snare`, `hat`, `open_hat`, `crash`, `ride`, tom variants) and `X` accents, `x` normal hits, `g` ghosts, `1`–`9` explicit intensities, `.` rests. `drums({kick:"X...X...",snare:"....X..."},span=1bars)` is a pattern; repetition, placement, dynamics and refinement work normally. `euclidean(hits,steps,pitch=42,span=4b)` supplies a source rhythm recipe. A kit maps each voice to a synth or sample instrument.

For useful defaults, import `std/music`, `std/grooves` and `std/mix`. Functions and source modules are the abstraction system; a full arrangement need not flatten reusable themes into notes.

Piano policy now runs a bounded hand/finger search before checking the performed timing. Each held note keeps its finger; other fingers can articulate moving inner voices and repeated attacks. Explicit hands and `annotate(selector,{finger:3})` are anchors. The search retains eight alternatives, prioritizing distinct held-note obligations, and stops at 400,000 transitions. `inspect --view score` shows hands and finger annotations. A search with no surviving allocation and a search that hit its budget have different messages. These are results under a deliberately small reach/continuity model, not a general physical proof. Use `fingering:false` for the earlier span/count/movement checks alone.

`groove({snare:9ms,hat:-2ms},accents=[1,0.92,1,0.9],grid=1/2b)` applies repeatable pocket and metrical intensity. `drum_feel(3ms,variation=0.045,seed=1)` adds correlated bar/hit/recovery variation while retaining ghost/accent relationships. `flam("snare",spread=24ms,grace=0.5)` adds a softer preparatory hit. `roll("snare",step=1/8b,to=0.9)` subdivides selected note extents with a dynamic rise and alternating stick annotations. Tag/refine selected hits to change articulations such as `rimshot`. `std/grooves` provides reusable acoustic/electronic starting points.

Kits accept a `chokes:[["hat","open_hat","pedal_hat"]]` definition; that hat group is the default. A hit releases older samples in its group through a short ramp, including one-shots. The prepared sample map chooses velocity zones and cycles through matching round robins. Lower velocity bounds are inclusive; upper bounds are exclusive except 1.0. `velocity_track:0.2` reduces double attenuation when the recording already supplies dynamic layers. Physical kit voices can specify `{instrument:sample(...),chain:[fx(...)],gain:3,pan:0.1,sends:{hall:-20}}`; gain is relative to the kit track, and the voice's chain precedes the track's common chain.

## Source performance policies

`std/performance` owns `scale_gate`, `dynamics`, `humanize`, `groove`,
`drum_feel`, `rubato`, `flam`, `roll`, and `pedal`. These names remain available
as global functions and pattern methods through the source standard library.
Import the module to compose or adapt its helpers explicitly:

```muz
use "std/performance" as feel;
let shaped = phrase("C4:q E4:q").map_notes(fn(n) => {
    duration: n.duration * 3/4,
    release_offset: n.release_offset + 2ms
});
let main = feel.displace(shaped, fn(at) => sin(at / 1b) * 5ms);
```

Performance durations use units (`4ms`, `25ms`). Numeric offsets in sparse event
patches retain the low-level millisecond convention, but source performance
recipes take explicit time quantities. Humanization is deterministic for its
key, seed and stream. Its source-defined timing and velocity weights can be
changed without changing the engine. `fixed` notes keep their authored attack
timing under humanize, groove and drum feel. Velocity still varies. Deterministic
noise values differ from the earlier native policies; repeated evaluation of
the same source remains reproducible.

### Lossless transformations

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
`refine(selector, callback)` now accepts sparse callback patches with the same
fields, alongside its original constant record form.

Single-patch transformations preserve the original key. Expansion automatically
uses `original/expand0`, `original/expand1`, etc. unless the patch supplies an
explicit `key`. For an ornament that retains its principal note, return its
original key explicitly. `overlay(patterns)` combines already namespaced
patterns without positional key prefixes and rejects duplicate note identities;
use `.at(position, key="occurrence")` to namespace occurrences before overlay.

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

`keyed_noise(key, seed=0, stream=0)` returns deterministic noise in `[-1,1]`.
Keys and streams may be strings or compound source values. Distinct streams
allow independent velocity and timing variation without positional randomness.
`group_by(list, callback)` groups equal callback keys, preserving first-group and
within-group order. `keys(record)` exposes record field names. Pedal policy uses
one grouping pass instead of scanning the harmony once per chord; its register,
density and catch choices are ordinary source expressions. Piano hand/reach and
voice-leading solvers remain efficient kernel machinery.

`std/performance.piano_preferences` owns the hand centers, initial finger positions,
search weights and per-finger pitch-class costs. Import it as `perf` and pass
`{playing:merge(perf.piano_preferences,{hand_centers:[50,74]})}` in track options,
or pass `preferences` to `.hands()`. The source `track` helper supplies defaults;
manual piano track records must supply `playing`. Costs must be scalar values
from 0 to 1,000,000; initial positions must be MIDI pitches 0..127. Default thumb
costs cover all five black-key pitch classes. Preferences rank allocations;
explicit hand/finger anchors, held-note constraints and bounded search remain
native. Existing reach and movement checks still report infeasible results.

Rubato's forward-time guard compares the dimensionless displacement-to-time ratio
against phrase length. This keeps long phrases with decimal timing amounts (for
example `note(60,48b).rubato(38ms)`) within ordinary numeric arithmetic instead of
overflowing an exact seconds fraction; it preserves the same tempo safety bound.
