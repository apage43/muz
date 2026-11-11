# The muz language

A `.muz` file builds values: notes and patterns, reusable functions, instruments,
and songs. A file used by `muz check` or `muz render` normally ends with a song.
Use `muz eval` to inspect other values, or export a pattern directly as MIDI.
`muz eval FILE --expr EXPR` evaluates `EXPR` in the file's top-level scope, so an
audit needs no scratch module; `-` as FILE reads the module from standard input.
`muz eval` prints JSON. A quantity with a unit prints as a string in source
spelling, so it can be pasted back: `phrase("C4:q D4:e.").notes[1].duration`
prints `"0.75b"`. Units print as `b`, `bar`, `s`, `dB`, `Hz`, and `bpm` (`250ms`
prints `"0.25s"`, `1kHz` prints `"1000Hz"`); a ratio with no finite decimal
prints as `"1b/3"`. Plain scalars stay JSON numbers. Dumping a whole pattern
prints `span`, `notes`, `controls`, and `raw` with the same units and field
names as the `pattern.notes`, `.controls`, and `.raw` accessors; a note in a
clock clip also carries a `clock` record with `offset`, `duration`, and `span`
in seconds.

Start with [getting started](getting-started.md) for CLI setup. This guide teaches
the source model; [performance](performance.md) covers expressive timing and
[production](production.md) covers the audio graph.

[Documentation index](README.md) · Offline: `muz docs language`

## Contents

- [A complete song](#a-complete-song)
- [Values, functions, and modules](#values-functions-and-modules)
- [Units and musical time](#units-and-musical-time)
- [Patterns and transformations](#patterns-and-transformations)
- [Selectors, tags, and note data](#selectors-tags-and-note-data)
- [Harmony and pitch](#harmony-and-pitch)
- [Songs and tracks](#songs-and-tracks)
- [Arranging passages](#arranging-passages)
- [MIDI interchange](#midi-interchange)
- [Diagnostics and limits](#diagnostics-and-limits)

## A complete song

```muz
let theme = phrase("C4:q D4:e E4:e | [F4 A4]:h r:h");
let lead = theme.repeat(2).velocity(0.7);
song({
    title: "First phrase",
    tempo: 120,
    tracks: [track("lead", lead, synth("glass-lead"))],
    tail: 1
})
```

`phrase` creates a pattern. Transformations return new patterns, so `theme` stays
available for another variation. `track` assigns the music to an instrument;
`song` assembles tracks and production settings. The final expression is the
file's result.

## Values, functions, and modules

### Bindings and functions

Use `let` for a binding, arrays for ordered collections, and records for named
fields. Values are immutable. Arrays use zero-based indices; record fields can
be accessed with a dot or an index.

```muz
let pitches = [60, 64, 67];
let settings = {velocity: 0.7, label: "lead"};
fn shifted(notes, semitones = 12) = notes.transpose(semitones);
fn motif(root) {
    let notes = phrase("C4:q E4:q G4:h");
    notes.transpose(root - 60)
}
let answer = shifted(motif(pitches[0]), semitones = 7);
```

Functions use lexical scope and return their last expression. Expression bodies
end with `;`; block bodies can introduce local bindings. Anonymous functions use
`fn(x) => expression`. Defaults and named arguments work with ordinary functions.
An `if` is an expression: `if x > 0 { x } else { 0 }`. Comments use `//` or
`/* ... */`.

A function sees only names bound before its definition, so it cannot call
itself: muz has no recursion, and a self-reference reports
`function 'f' cannot refer to itself`. Iterate with `map`, `fold` or `range`
instead. A definition may still wrap a builtin of the same name, as in
`fn chord(s) = chord(s, octave = 4);`.

Every name must resolve to a local binding, an import, a builtin or a standard
prelude export; an unknown name such as a misspelled `tmepo` is an error at the
point of use. `value.name(...)` calls a builtin or prelude function with `value`
as its first argument; any other dotted name on a non-record value is an error.

Single- and double-quoted strings stay literal even when their contents match
syntax: `let label = "if";`, `["}", "fn", "<eof>"]` and `{"let": "="}` all
work. Quoted record keys are allowed; bindings and function parameter names use
unquoted identifiers. Formatting preserves string contents and quote spelling.

### Imports and asset paths

```muz
use "material.muz" as material;
use "std/music" as music;
use "contrib/virtuosity-drums/kit" as drums_pack;
```

Local modules resolve relative to the importing file. `std/` names bundled source
modules, and `contrib/` names optional instrument setups. The standard prelude
already exposes common functions and data as bare names; import a module when
you need its additional helpers or want an explicit namespace.

An alias is an ordinary binding, so `use "keys.muz" as keys` hides the builtin
`keys()` for the whole module. A `use` whose alias the module also calls as a
function is an error at the `use` line. Calling any other binding that hides a
builtin, such as `let keys = {...}`, reports which name is in the way. Name
such modules after their role (`use "keys.muz" as theo`).

`is_entry()` is true while the module being evaluated is the entry point, the
file given to `muz eval`, `check` or `render`, and false while it is imported.
Use it to keep a part file's self-checks without paying for them in every song
that imports the part:

```muz
let part = /* ... */;
// Runs for `muz eval bass.muz`, not when song.muz imports the bass part.
let checked = if is_entry() { self_test(part) } else { true };
```

Call `is_entry()` at a module's top level. Inside a function, it reflects the
module being evaluated when the function runs.

A function body evaluates in its defining module. Asset paths therefore follow
the module that declares the device, even when another module calls its builder.
See [asset setup](instruments.md#asset-paths-and-optional-packs) for contrib
installation and lookup.

The [standard library](../std/) is readable `.muz` source. Extend it for reusable
musical habits rather than copying large event lists.

### Collections and strings

`map`, `filter`, `fold`, `sort_by`, `range`, `len`, `slice`, `chars`, `record`, `group_by`, `index_by` operate on ordinary values.
Arrays and records are shared immutable values; passing them to callbacks does
not copy their contents.

`sort_by(list,fn(item)=>key)` is stable and accepts numeric or string keys;
numeric keys must have compatible units. `range(end)`, `range(start, end)` and `range(start, end, step)` (half-open, start first like `slice`; named `start`, `end` and `step` work in any order) retain exact
arithmetic and accepts compatible dimensional bounds and steps. An omitted step
is one in the end value's original unit.

`merge(a, b, ...)` shallowly combines any number of records, with later records
winning on duplicate keys. For example, `merge({x:1}, {y:2}, {x:3})` returns
`{x:3, y:2}`. Nested records are replaced as whole values, and inputs remain
unchanged. `merge()` returns `{}`; `merge(record)` returns an equivalent record.
The first two arguments still accept the names `base` and `overrides`; additional
records are positional.

`index_by(list, function)` builds a record keyed by the string returned for each
item. It calls the function once per item in list order, retains the original
values, rejects duplicate or non-string keys, and returns `{}` for empty input.
For example, `index_by(pattern.notes, fn(n) => n.key)[key]` retrieves a note.
Like other collections, input is limited to 200,000 items; construction is
O(n log n), with O(log n) dynamic lookup.

`group_by(list, function)` partitions a list into a list of lists, one per
distinct callback key, in order of first appearance and preserving item order
inside each group. Keys may be any ordinary data value and keep their units. It
is the usual replacement for nested scans and for `fold`s that rebuild a list on
each step, which exhaust the evaluation budget on note-sized inputs:
`group_by(notes, fn(n) => round(n.at / 1b * 4))` groups notes by sixteenth.
Input is limited to 200,000 items.

`chars(text)` returns a list of one-character strings, one per Unicode scalar
value (not per byte or grapheme cluster), so source can decode its own text
notations: `chars("c.X.")` is `["c", ".", "X", "."]`. Non-text input is an error
and `chars("")` is `[]`. Text has no index syntax; use `chars(text)[i]`.

`join(list, separator = "")` is the inverse: it concatenates a list of text items
with the separator between them, so a grid computed from data becomes a lane with
`join(map(chars("X.x."), fn(c) => if c == "x" { "X" } else { c }))`. `join(chars(t)) == t`, and `join([])` is
`""`. Any non-text item is an error naming its index.

`split(text, separator)` (the same name as the pattern form `split(pattern, selector)`, chosen by the first argument) is the inverse of `join`: it returns the pieces of
`text` between occurrences of `separator`, so `join(split(t, s), s) == t`.
`split("u+v", "+")` is `["u", "v"]` and `split("", ",")` is `[""]`. An empty
separator is an error; use `chars` to split into characters.

`slice(list, from, to)` returns the items at indexes `from` up to but excluding
`to`, like `range`'s half-open convention: `slice([a, b, c, d], 1, 3)` is
`[b, c]` and `slice(list, 0, len(list))` is the whole list. Bounds are unitless
integers from 0 to `len(list)`; a bound outside that span, a fractional or
negative bound, or `to` before `from` is an error rather than being clamped
(like out-of-range list indexing). `slice(list, n, n)` is `[]`. When the first
argument is a pattern, `slice` and `.slice` keep their pattern meaning (see
[patterns and transformations](#patterns-and-transformations)).

`record(pairs)` builds a record from a list of `[key, value]` pairs, so tables
can be computed: `record(map(["a", "b"], fn(k) => [k + "2", str(len(k))]))`. Keys must
be text; a pair that is not a two-item list, a non-text key, or a duplicate key
is an error (unlike `merge`, later pairs never silently replace earlier ones).
`record([])` is `{}`. Input is limited to 200,000 pairs. Use `keys` to read keys
back and `index_by` when the keys come from the items themselves.

`format("layer{}_rr{}.flac",[layer,rr])` and `str(value)` help construct asset names
and other strings. They do not search or glob the filesystem.

### Source catalogs

[std/catalogs](../std/catalogs.muz) provides synth presets, kit/choke defaults,
scale modes, Euclidean rhythms, and control-curve recipes. The prelude exposes
these as ordinary values and functions; extending it needs no Rust name registry.
`scale(root,mode="minor",octave=4,modes=catalog.scale_modes)` accepts a replacement
mode table. `drums(lanes,span=4b,voices=...,articulations=...,gate=0.5)` also uses
source tables: the generic `drum_grid` decoder maps arbitrary lane names to
pitches and strike symbols to velocities or null for rests. A lane is text or a
list of one-character symbols (`join` is not needed for that). Each decoded hit
keeps its strike symbol as the tag `sym:X` and as `n.data.symbol`, so
`select(grid, "tag:sym:k")` or a `flat_map_notes` callback testing
`n.data.symbol == "k"` chooses a stroke by name rather than by velocity. These signatures
show optional table arguments; replace the ellipses with real tables.

### Equality

`==`, `!=` and `contains` compare values recursively, preserving dimensions in
lists and records. Beats/bars, ms/s and Hz/kHz normalize consistently; beats and
seconds never compare equal. Exact numbers compare as rationals, while a pair
containing an inexact number uses finite floating-point comparison without an
epsilon. Record source locations do not affect equality. Functions compare by
binding identity: a copied function equals itself, independently created closures
do not. Group/random keys retain their separate deterministic canonical encoding.

## Units and musical time

A beat (`b`) is a quarter note. Use explicit units to distinguish score time from
clock time and other quantities:

| Quantity | Examples |
| --- | --- |
| Score time | `1b`, `1/3b`, `2bars` |
| Clock time | `20ms`, `1s` |
| Gain | `-12dB` |
| Frequency | `500Hz`, `1kHz` |
| Scalar | `0.5`, `50%` |

A literal `1bar` always equals four quarter beats, even in a non-4/4 song.
For meter-dependent measures, import `std/arrange` and use
`a.measures(count,meter)`, or its alias `a.bars(count,meter)`.
For example, `a.measures(2,[3,4])` is `6b`. This distinction also applies to
section and passage durations.

### Mixed meter

`meter` is the meter at the song start. `meters: [[position, [beats, unit]], ...]`
adds later meters; each starts on a bar line of the meter before it, and a
change at `0b` replaces the starting meter. Bar numbers in `muz check`
diagnostics, MIDI time signatures, DAWProject time-signature points, plugin
transport bar lines and `muz analyze --song` follow this map. Notes and spans
stay in quarter beats, so a 7/8 bar is a `3.5b` span either way.

`a.meter_changes(runs)` from `std/arrange` builds the list from runs of bars in
form order:

```muz
use "std/arrange" as a;
// 35 bars of 4/4, one 7/8 bar, 6 bars of 4/4, two 7/8 bars ...
let meters = a.meter_changes([[35, [4, 4]], [1, [7, 8]], [6, [4, 4]], [2, [7, 8]]]);
song({tempo: 152, meters: meters, tracks: [/* ... */]})
```

`a.bar_at(position, meters, meter=[4,4])` gives the bar holding a song position
under such a map as `{bar, start, meter, position}`, with `bar` from 1 and
`position` measured from the bar line, so source recipes can work bar by bar.

Numeric fields preserve their documented plain-number defaults: musical time in
beats, clock time in seconds, `*_ms` in milliseconds, `*_hz` in Hz and gains in
dB. Compatible explicit units are accepted; incompatible units are errors.
Meter, counts, list indices and discrete MIDI fields require integral unitless
values before conversion. Fractional musical pitch remains supported. Exact
stretch factors retain rational score time. Plugin normalized controls are scalars.

### Arithmetic and precision

Source numeric literals and their arithmetic retain exact rational values. Floating
calculations such as `sin`, `cos`, and `pow` produce finite inexact
numbers; arithmetic involving an inexact operand stays inexact. Exact scalar
arithmetic that exceeds the rational representation falls back to floating point;
exact dimensional arithmetic reports overflow rather than silently losing timing
precision. An inexact value used as score time (a length, `at`, an offset, a
slice bound, a stretch factor, a bar count) is rounded at that boundary to the
nearest multiple of 1/20160000 beat. The grid is lcm(960000-tick PPQ, 1..10), so
floats such as `0.25` and `0.375` stay exact and a computed `1/3` or `1/5`
becomes that tuplet fraction. The rounding error is at most 1/40320000 beat
(about 2.5e-8 beat, 12 ns at 120 BPM), well below one export tick. The bounded
denominator keeps later exact arithmetic with tuplet positions from overflowing.
Exact values never pass through this grid. Division by zero and nonfinite
results are errors.
Thus `1b/3+1b/3+1b/3` remains exactly one beat, while
`0.5-0.5*cos(6.28318*i/64)` composes as an ordinary control calculation.

`min` and `max` require compatible units and return the selected quantity without
losing its exactness. `abs`, `floor`, and `round` retain the input unit (`ms` is
normalized to seconds, so rounding acts on seconds). `sin`, `cos`, and `pow`
require scalars. Addition, subtraction, comparisons and remainder require matching
units; beats and bars interoperate at four quarter beats per bar. Multiplication
requires at least one scalar; division accepts a scalar divisor or matching units
(the latter yields a scalar). For example `max(1b,2b)+1b` is `3b`, while
`min(1b,1s)` and `1b*2b` are errors.

Score-time operations report `exact score time overflow` if an onset, duration,
or span cannot fit the exact rational representation. This covers sequencing,
placement, repeat/fit, slicing, stretching, and reversal; where available the
message names the operation and both exact operands. Inexact values cannot
cause it alone, but exact rationals with large unrelated denominators can,
including long decimal literals: `-2.0258913990809204` is exactly
20258913990809204/10^16, so `note(60, 1/6b + -2.0258913990809204 * 128 / 60000 * 1b, at = 301/6b)`
overflows. The same value computed at run time (for example from a performed
`n.offset / 1ms`) is inexact and rounds to the grid. Prefer simple fractions for
exact score offsets, or [displacement](performance.md#coherent-time-displacement)
for performance offsets in seconds.

### Clock clips

Clock clips carry dedicated validated timing. `clock_start`, `clock_duration`
and `clock_span` annotation keys are ordinary metadata and cannot alter playback.
Place or repeat a clip's pattern normally; change its trim through `clip` options.
Score-duration transforms (`slice`, `fit`, `stretch`, `reverse`, or a duration
patch) reject clock clips rather than manipulating a placeholder beat duration.
Direct Rust/serialized musical notes use the optional `clock` payload; legacy
timing annotations are not inferred as clock timing in new pattern values.

`pattern.has_clock_timing` is a read-only boolean indicating private clock-clip
note timing. Public note records retain their existing beat placeholders; this
property does not expose or convert that private payload.

## Patterns and transformations

A pattern has musical notes, controller events, raw events, and a logical span.
The span includes intentional silence and determines how sequencing places the
next pattern. Local patterns can have negative pickup positions; final compiled
events must be at or after zero.

### Write a phrase

```muz
phrase("C4:q D4:e E4:e | [F4 A4]:h r:h")
```

Pitch names specify the note and octave. Square brackets make a chord; `r` makes
a rest. Bar lines are visual separators. An `@name` after a note adds a tag, and several may follow one note:
`C4:q@r@b@v` has the tags `r`, `b` and `v`. A tag cannot contain `@`; `+` is an
ordinary tag character, so `@u+v` is the single tag `u+v`.

| Length | Value in quarter beats |
| --- | --- |
| `w` | 4 (whole note) |
| `h` | 2 (half note) |
| `q` | 1 (quarter note) |
| `e` | 1/2 (eighth note) |
| `s` | 1/4 (sixteenth note) |
| `t` | 1/8 (thirty-second note) |

Dotted and numeric rational lengths also work. Each dot adds half the previous
addition, up to three dots: `h.` is 3 beats and `h..` is 3.5 (a held note filling
a 7/8 bar). Build individual notes with
`note("C4",1b)`, and intentional silence with `rest(duration)`.

### Combine and transform

| Operation | Purpose |
| --- | --- |
| `seq([a,b])` | Place patterns consecutively using their spans. |
| `stack([a,b])` | Combine patterns at their current positions. |
| `.repeat(n)` | Repeat material. |
| `.transpose(semitones)`, `.invert(center)` | Change pitches. |
| `.at(offset)` | Shift events and span by a relative beat offset. |
| `.slice(start,end)` | Extract a window and rebase it to zero. |
| `.stretch(factor)`, `.fit(duration)`, `.reverse()` | Transform score time. |
| `.velocity(value)`, `.gain(factor)` | Set or scale note velocity. |
| `.gate(value)`, `.scale_gate(factor)` | Set or scale the held fraction of written duration. |

These are pattern transformations; audio gain and effects belong to tracks and
devices. See [performance](performance.md) for gate, dynamics, timing, and expression.

### Placement and slicing

Pattern score positions belong to the current pattern value. `.at(offset)` shifts
every event and the pattern span by that relative offset; it is not absolute
placement, so applying `.at(16b)` twice adds 32 beats. Place reusable local
material once for each occurrence rather than placing an already-positioned
pattern again.

`.slice(from,to)` reads the current pattern coordinates, keeps material that
intersects the half-open window `[from,to)`, and subtracts `from` so the result
starts at zero with span `to-from`. Notes crossing a boundary are trimmed;
controls and raw events are retained when their onset is inside the window. A
window with no events returns a valid silent pattern of the requested span. To
keep the selected material at its source position, use `p.slice(a,b).at(a)`;
usually, slice reusable local material before placing the occurrence.

## Selectors, tags, and note data

Tags and annotations keep musical intent attached to notes through reuse:

```muz
let marked = phrase("C4:q E4:q G4:h").tag("last", "echo");
let revised = marked.annotate("tag:echo", {purpose: "answer"})
    .refine("tag:echo", {velocity: 0.8});
```

`select` keeps matching notes; `reject` removes them; `refine` patches them.
Selectors include `all`, `first`, `last`, `tag:name`, `voice:name`, arrays for
unions, records for intersections, and predicates such as `fn(n) => n.pitch > 60`.
A bare tag name is also accepted. Use exact note keys for identity; display labels
are not lookup keys.

`split(pattern,selector)` returns `{selected,remaining}` patterns with the same
span. Put them in separate tracks to isolate a phrase ending or a voice before
shared mixing. Channel controls stay with `remaining`; add the appropriate
controls to the selected layer deliberately.

`pattern.notes` exposes ordinary note records. The [performance reference](performance.md#score-and-performed-time)
lists their fields and shows how to calculate performed attack/release times with
`seconds_at`. Transform and place the material before deriving final automation.
Use `map`, `filter`, and `fold` to calculate values, or the
[event patch functions](performance.md#custom-performance-policies) to preserve
all untouched event data.

`sample_zone` is a reserved annotation that selects an absolute zero-based sampler
zone. See [recording selection](instruments.md#pinning-a-recording-to-a-note) for
validation and a recipe that preserves choices when splitting tracks.
`articulation` is a reserved annotation that chooses an `sfz()` instrument's
[articulation for that note](sfz.md#per-note-articulations).

## Harmony and pitch

```muz
let harmony = chords("F#m D A E", each = 2bars);
let voiced = voicelead(harmony, low = 48, high = 84, center = 64);
let accompaniment = arpeggiate(voiced, [0, 2, 1, 2], 1/2b);
```

`chord("F#m7")` returns pitches; `pitch("F#4")` returns a MIDI pitch.

`chord(symbol, octave=3)` places the root in the given octave and stacks the
remaining tones upward from it in close position: `chord("Em")` is
`[52, 55, 59]`, `chord("C", octave=4)` is `[60, 64, 67]`. Sevenths and
extensions stay above the root rather than wrapping, so `chord("C9")` is
`[48, 52, 55, 58, 62]` and `chord("C13")` is `[48, 52, 55, 58, 62, 69]`.
A slash bass is placed strictly below the root: `chord("C/G")` is
`[43, 48, 52, 55]` and `chord("C/E")` is `[40, 48, 52, 55]`.
The result is always strictly ascending, so its first pitch is the root, or
the bass of a slash chord. `pitch(name, octave=n)` and the root motion that
`reharmonize` scores both read that first pitch.
`chords(symbols, each=4b, octave=3)` applies the same rule to every symbol.

### Chord symbols

A chord symbol must be read completely. Text the parser cannot read is an error
that quotes the unread part, as in `chord 'C7alt': cannot read 'alt'`. A symbol
is written in this order, with no spaces:

1. **Root:** a letter `A`–`G` with an optional `#`, `##`, `b` or `bb`. An
   accidental right after the letter belongs to the root, so `Cb5` is a C-flat
   power chord. Write a flatted fifth on C as `C(b5)`.
2. **Quality:** none (major), `m`, `min` or `-` (minor), `dim`, `°` or `o`
   (diminished), `ø` (half-diminished), or `aug` or `+` (augmented).
3. **Sixth, seventh or extension:** optionally `maj`, `Maj`, `M` or `Δ` to
   mark a major seventh, followed by `5`, `6`, `69`, `6/9`, `7`, `9`, `11`
   or `13`.
4. **Modifiers, in any order:** `sus2`, `sus4` or `sus` (same as `sus4`),
   alterations, and `add` tones. Alterations and added tones may also be
   grouped in parentheses with commas, as in `C7(b9,#11)` or `C(add9)`.
5. **Slash bass:** `/` and a note name with an optional accidental.

| Part | Intervals from the root (semitones) |
| --- | --- |
| major, minor, diminished, augmented triads | `0 4 7`, `0 3 7`, `0 3 6`, `0 4 8` |
| `ø`, `ø7` | `0 3 6 10` (same as `m7b5`) |
| `5` | `0 7` (major quality only, no third) |
| `6` | adds 9; `69` and `6/9` add 9 and 14 |
| `7` | adds 10; 11 after `maj`/`M`/`Δ`; 9 for a diminished seventh |
| `9`, `11`, `13` | add the seventh and 14; `11` also adds 17; `13` adds 21 but omits the 11 |
| `sus2`, `sus4` | replace the third with 2 or 5 (major and dominant chords only) |
| `b5`, `#5` | replace the fifth with 6 or 8 |
| `b9`, `#9` | add 13 or 15 and remove a natural 9 |
| `#11` | adds 18 and removes a natural 11 |
| `b13` | adds 20 and removes a natural 13 |
| `add2`, `add4`, `add6`, `add9`, `add11`, `add13` | add 2, 5, 9, 14, 17 or 21; `#`/`b` after `add` raise or lower that tone |

An alteration adds only its own tone. `C7#11` is `C E G Bb F#` and implies no
natural 9, and `C7b9` contains no natural 9. Inside parentheses, a bare `9`,
`11` or `13` means the same as `add9`, `add11` or `add13`. `Δ` alone and `ø`
alone imply the seventh. `maj` or `M` alone, as in `Cmaj` or `CM`, means a
plain major triad. `maj6` and `M6` mean `6`. A symbol cannot contain
whitespace, because `chords` separates symbols at spaces. Spellings outside this
grammar, such as `7alt`, are rejected. Write the intended tones explicitly
instead, for example `C7(b9,#9,b13)`.

Pitch text accepts `#`, `b`, `##` and `bb` in `pitch`, `note` and `phrase`,
as well as chord roots and slash basses. Double accidentals shift the named
natural by two semitones, including across octave boundaries: `C##5` and `Ebb5`
both sound MIDI 74, `B##4` sounds 73, and `Cbb5` sounds 70. For example,
`phrase("[A#4 C##5 E#5 G#5]:q")` spells A-sharp dominant seventh directly.
The existing numeric pitch representation and MIDI range checks still apply;
pitch display uses its canonical spelling rather than retaining the input text.

`scale("E","harmonic_minor",octave=3)` returns pitches. Modes include major/minor,
church modes, melodic/harmonic minor, pentatonics, and whole tone.

| Operation | Result |
| --- | --- |
| `degree(key,1)` | The tonic; degree numbers continue across octaves. |
| `diatonic_chord(key,5,voices=4)` | A chord built by stacking thirds. |
| `pattern.diatonic_transpose(key,steps)` | Transposition by scale steps, retaining chromatic inflections. `steps` is an integer in -200000..=200000; negative steps move down and `0` leaves pitches unchanged. |

[std/tonal](../std/tonal.muz) supplies melody and progression helpers based on
scale degrees; [std/piano](../std/piano.muz) supplies accompaniment and performance
functions.

### Voice leading

`voicelead` searches the whole supplied phrase while preserving chord pitch
classes. It considers inversions, octave placement, common tones, register, smooth
movement, and a penalty for parallel fifths/octaves. The default search retains
64 candidates per chord, with at most eight voices and 512 chord attacks per call.
Its result is the best path within that bounded set.

Tag a note `fixed` or annotate `{anchor:true}` to pin its pitch. Keep roles and
tags on individual notes to inspect their resulting placement.

The `std/tonal.tonal_scoring` record and search wrappers expose register,
candidate-count, and ranking preferences in source. Candidate generation and
search remain bounded native operations.

### Reharmonization

`reharmonize(harmony,melody,["Em","Cmaj7","Am","B7"])` chooses chords for
existing harmonic slots. Held melody duration, metrical position, and root
continuity affect its choice. A `fixed` harmony slot keeps its chord symbol;
the melody stays unchanged.

`reharmonizations(harmony,melody,palette,count=3)` returns up to five ranked
`{harmony,score}` alternatives. It varies the first unanchored slot and optimizes
the rest. Bind one result's `.harmony` in source and audition it; selection is
explicit and the ranking is not a stylistic guarantee.

Use `muz eval choices.muz` to inspect these values without preparing audio.
See [tonal-assistance.muz](../examples/tonal-assistance.muz).

## Songs and tracks

`song` contains `title`, `tempo`, `meter`, `meters`, `sections`, `tracks`, `buses`,
`master`, `automation`, `checks` ([cross-track checks](performance.md#playability-checks)),
and `tail`. A track connects a pattern to an instrument and accepts
production options such as gain, pan, inserts, and sends; a bus accepts the same
output, gain, pan and sends. Unknown song, track and bus fields are errors. See [production](production.md#signal-flow) for a complete routed example.

`section("chorus",16bars)` labels the next span in the section list. It does not
place notes. Place patterns explicitly or use the arrangement functions below.

## Arranging passages

### Define and place reusable material

`use "std/arrange" as a;` provides ordinary source records and functions.
`a.passage(span, parts, gestures=[], tempos=[])` holds local material;
`a.part("lead", pattern)` assigns it to a continuing song track.
`a.sequence([a.occurrence("verse", verse), a.occurrence("chorus", chorus)])`
places each occurrence using its logical span. `a.group(sequence)` nests an
arrangement. Pickups and note/effect tails do not change the sequencing span.

The following complete example places a passage twice and changes the second use:

```muz
use "std/arrange" as a;
let theme = phrase("C4:q E4:q G4:h");
let verse = a.passage(4b, [a.part("lead", theme)]);
let form = a.sequence([
    a.occurrence("opening", verse),
    a.occurrence("answer", a.edit(verse, "lead", fn(p) => p.transpose(7)))
]);
a.build(form, {
    tempo: 120,
    tracks: [track("lead", rest(0b), synth("glass-lead"))]
})
```

`a.edit(passage,"lead",fn(p)=>...)` changes that value's part, leaving the shared
input available for other occurrences. `a.require(p,selector,count=1)` diagnoses
a pinned selection that no longer matches; a rule such as `"last"` intentionally
follows whichever note is last. `a.build(form, settings, gestures=[])` fills the patterns of
the declared tracks and generates section labels, tempo points and automation.
The same track/instrument/effect graph continues across passage boundaries.
The optional third argument supplies whole-arrangement gesture callbacks, useful
for policies such as unioning send windows across passage boundaries.

### Optional span and containment checks

Passages remain permissive. Opt into logical contracts with
`a.require_span(pattern, expected)` (exact equality, including trailing rests)
or `a.require_fit(pattern, limit)` (logical span at most the limit). Both return
the unchanged pattern and require a nonnegative beat duration.

`a.require_contained(passage, pickups=false, tails=false)` adds a deferred
performed-time contract, checked by `build` before generating gestures with the
complete tempo map and final edited part layers. Attacks and control/raw starts
must precede the passage end; starts before its beginning require `pickups=true`.
Key releases may equal the end, or exceed it with `tails=true`. Tails never permit
late attacks or controls. Releases mean scheduled key releases, not acoustic
decay. Containment does not imply logical fit: a long written note with a short
gate can fit in performed time. Empty layers pass. Each duplicate-ID layer and
nested child is checked independently. Clock-timed clip parts are explicitly
unsupported by containment; logical contracts still work on them.

For example, `a.require_contained(a.passage(4b,
[a.part("lead", a.require_span(phrase("C4:w"), 4b))]))` checks both contracts.

### Derive gestures after placement

Gesture functions receive `{name,start,span,parts,timing}` after placement and
after assembling the full tempo map. `a.material(context,"lead")` supplies the
placed pattern, including its namespaced keys. Derive any automation from those
notes. After editing a group, child gestures see their own final edited layers,
including changed timing, articulation and deleted or replaced notes. This holds
through multiple levels of nesting; siblings and other uses of the original
passage remain independent. A gesture must handle an empty selection explicitly,
or use `a.require` when removing its target should fail.

For a previously defined `chorus` passage:

```muz
let grouped = a.group(a.sequence([a.occurrence("inner", chorus)]));
let revised = a.edit(grouped, "lead",
    fn(p) => p.map_notes(fn(n) => {at: n.at + 1b, velocity: 0.5}));
```

Here the chorus's note-derived gestures follow the shifted, quieter notes when
`revised` is placed. Edits to a grouped part use times relative to the group and
run separately on each matching layer. `edit` preserves the part list's slots,
which bind layers to child gestures; use it for pattern revisions rather than
manually reordering or replacing a group's `parts` list. A replacement pattern
uses the same group-relative coordinates as any other edit.

`a.local(target,points,shape="linear")` moves beat-relative points;
`a.clock(...)` moves a seconds-relative gesture without stretching its offsets.
`a.join` combines same-target gestures in chronological order, requiring the
same shape and no overlap. Continuous endpoints must agree; conflicting curves
need an explicit source combination or transition. There is no implicit winner.

See [revision-workflow.muz](../examples/revision-workflow.muz) for meter-aware
arrangement, a sparse final occurrence, note-derived gestures, and named render
collections. The starter from `muz new` uses the same arrangement functions.
For gestures spanning passage boundaries, see
[automation scope](production.md#passage-and-arrangement-gestures).

## MIDI interchange

### Import musical material

`midi("part.mid",track=1)` imports a musical sequence as a pattern, including
channel data, controllers, and ordinary channel events. Without a track selection,
synchronous format-0/1 tracks combine. Format-2 sequences require explicit
selection; place independent patterns yourself.

`midi_tempos(path,track=...)` supplies beat/BPM points for the song's `tempos`.
`notes_only(pattern)` strips other event families when reusing a melody on a
different destination.

### Export and inspect files

```sh
muz midi export song.muz -o song.mid
muz midi inspect part.mid -o part.json
muz midi encode part.json -o part.mid
```

`export` writes multitrack musical MIDI. `inspect` and `encode` expose structural
formats 0/1/2, PPQN/SMPTE division, delta times, and valid opaque meta/SysEx payloads.
Structural conversion does not promise byte-identical encoding. SMPTE files retain
clock structure here; beat-based composition needs an explicit musical tempo
interpretation.

### Explicit channel events

The following calls return composable event patterns:

| Call | Value convention |
| --- | --- |
| `note_on("C4",velocity=0.7,at=0b,channel=0)` | Velocity 0–1; channel 0–15. |
| `note_off("C4",at=2b)` | Explicit matching release. |
| `cc(64,90,at=1b)` | Integer controller value 0–127. |
| `bank(0)`, `program(12)` | Bank 0–16383; program 0–127. |
| `bend(-0.2)` | −1 to 1 of the destination's configured bend range. |
| `pressure(0.4)`, `poly_pressure(60,0.4)` | Pressure 0–1. |

Every explicit channel event, `raw_midi` included, also takes `offset`: clock time
after `at`, such as `note_off("C4", at = 2b, offset = 25ms)`. The offset resolves
against the song's tempo map at compile time, so a millisecond window (a slur's
overlap, a keyswitch around a note-on, a strum) keeps its length under any tempo
or ritardando. Negative offsets are allowed, but the final event cannot come
before the song's start. `map_raw` reads and patches the same `offset`.

`.channel(2)` moves notes and channel controls together. Conflicting simultaneous
CC values and ambiguous overlapping raw note-ons for the same key are errors;
use `controls_over(base, layer)` ([performance](performance.md#controllers-and-raw-events))
to let a layer deliberately replace equal-time controls.
Raw notes need matching releases and bypass piano allocation.

`sysex([126,127,9,1])` adds F0/F7 framing around a 7-bit payload.
`meta(type,[bytes])` and `opaque([bytes])` provide structural escape hatches.
Runtime stereo plugin adapters accept ordinary channel events and reject
SysEx/meta/opaque events they cannot consume.

A file ending in a pattern can be passed directly to `muz midi export`, including
these events, without an instrument. Direct pattern exports use 960 pulses per
quarter note (PPQ) and interpret millisecond offsets at 120 BPM. Structural JSON
conversion preserves supported SMF timing/division. See
[midi-messages.muz](../examples/midi-messages.muz).

## Diagnostics and limits

### Source errors

Evaluation errors lead with the offending file, one-based line/column and message,
then display the source line with the expression underlined. Columns count Unicode
characters rather than UTF-8 bytes. Nested calls add a compact trace in innermost
caller order, collapsing repeated calls on the same source line. Imported and
bundled standard modules retain their own source locations, including function
defaults. Long traces abbreviate middle caller lines while retaining module
transitions. For example, an invalid `note` call in a helper reports its definition's
line first, followed by the callers that reached it.

Every later stage names a location too: parse failures point at the offending
token (with the closing `quote`, `*/` or matching bracket spelled out in a hint),
and production failures (lowering, graph validation, automation targets) point at
the `song`, `track`, `fx`, `sample` or `automation` call that declared the value.
Where a name is recognizable, the message adds `= help:` lines listing the accepted
unit, field, parameter, effect, module or function names, plus `did you mean ...?`
for near misses. Unknown identifiers and methods fail where they are looked up
rather than later when the value is used. Hints are mechanical only: they list what the language accepts and
never choose musical material for you. `muz check --json` carries the same failures
for scripts.

### Evaluation and expansion budgets

Evaluation has bounded steps, call depth, and event counts. A failed evaluation
leaves the live session intact. One evaluation's 5,000,000 operations are shared
by the entry file and every module it imports, since an imported module's
top-level `let`s and assertions run on import. When the budget runs out, the
error lists the operations each module's top level spent, largest first.

Pattern expansion preflights each event stream and owned payload before repeat,
fit or aggregation. Defaults are 200,000 notes/controllers/raw events each and
256 MiB of logical event/payload storage, including generated identity strings.
Hosts can lower these through `Evaluator::expansion_limits`. Oversized results
produce diagnostics, never truncated music. These composition limits are separate
from realtime simultaneous-voice and per-block event capacities.

## Next steps

Use [performance](performance.md) to shape notes, [instruments](instruments.md)
to choose their sounds, and [workflow](workflow.md) to inspect or render the result.
