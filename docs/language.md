# The muz language

A file evaluates to `song({...})`. `let`, lexical functions, defaults, named arguments, arrays, records, module imports and higher-order functions provide reuse. `use "material.muz" as m;` imports a local module; `use "std/music" as m;` imports the bundled library; `use "contrib/virtuosity-drums/kit" as d;` imports a bundled instrument setup from the `contrib/` library. Functions return their last expression. Use `fn name(args) { let x = ...; result }` when a function needs local bindings; expression bodies use `fn name(args) = expression;`. Records use `{key: value}`; functions use `fn name(arg, optional = value) = expression;` or `fn(x) => expression`.

Single- and double-quoted strings stay literal even when their contents match
syntax: `let label = "if";`, `["}", "fn", "<eof>"]` and `{"let": "="}` all
work. Quoted record keys are allowed; bindings and function parameter names use
unquoted identifiers. Formatting preserves string contents and quote spelling.

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
for near misses. Hints are mechanical only: they list what the language accepts and
never choose musical material for you. `muz check --json` carries the same failures
for scripts.

For `project.json5` sessions, semantic validation reports the file, line and
column of the failing value, together with its field path. For example,
`params:{cutof_hz:500}` on the third track's instrument reports
`field: tracks[2].instrument.params.cutof_hz`. Array indices are zero-based;
keys containing punctuation use quoted brackets, such as `params["bad.key"]`.
Duplicate global IDs point at the duplicate declaration and name the first
declaration's field path. A missing field points at its containing record;
aggregate graph failures point at the relevant collection or project root.
Parsing source text through `parse_session_with_root` uses `<project>` as the
filename. Comments, escaped keys and repeated values do not change field
identity. JSON5 syntax errors retain their parser-reported locations.

`phrase("C4:q D4:e E4:e | [F4 A4]:h r:h")` makes material. Note lengths: w/h/q/e/s/t; dotted or numeric rational lengths also work. Bar lines are visual separators. `@name` after a note tags it. `1/3b` is exact musical time; `2bars`, `20ms`, `1s`, `-12dB`, `500Hz`, `1kHz`, `50%` carry units. Bare `bars` always means four quarter beats, including section durations. For actual measures in another meter, use `a.bars(count, meter)` from `std/arrange` (for example `a.bars(2,[3,4])` is `6b`). Local patterns may have negative pickup positions; final song events must be at or after zero.

Source numeric literals and their arithmetic retain exact rational values. Floating
calculations such as `sin`, `cos`, and `pow` produce finite inexact
numbers; arithmetic involving an inexact operand stays inexact. Exact scalar
arithmetic that exceeds the rational representation falls back to floating point;
exact dimensional arithmetic reports overflow rather than silently losing timing
precision. An inexact value used as a musical duration is approximated as a
rational at that boundary. Division by zero and nonfinite results are errors.
Thus `1b/3+1b/3+1b/3` remains exactly one beat, while
`0.5-0.5*cos(6.28318*i/64)` composes as an ordinary control calculation.

Pattern construction and transforms also check rational time arithmetic. If an
onset, duration or span cannot fit, evaluation reports `exact score time overflow`
with a source location. This includes `seq`, placement, repeat/fit, slicing,
stretching and reversal. Inexact offsets can acquire large denominators when
converted to score time: for example, sequencing a 32-beat rest before
`note_on(57,0.5,at=0.025b+2*(0.013b+0.001b*sin(14)))` now reports overflow
instead of moving the note backwards. Use representable rational offsets such as
`seq([rest(32b),note_on(57,0.5,at=53b/1000)])`, or `.displace(...)` for
performance timing in seconds.

`==`, `!=` and `contains` compare values recursively, preserving dimensions in
lists and records. Beats/bars, ms/s and Hz/kHz normalize consistently; beats and
seconds never compare equal. Exact numbers compare as rationals, while a pair
containing an inexact number uses finite floating-point comparison without an
epsilon. Record source locations do not affect equality. Functions compare by
binding identity: a copied function equals itself, independently created closures
do not. Group/random keys retain their separate deterministic canonical encoding.

Numeric fields preserve their documented plain-number defaults: musical time in
beats, clock time in seconds, `*_ms` in milliseconds, `*_hz` in Hz and gains in
dB. Compatible explicit units are accepted; incompatible units are errors.
Meter, counts, list indices and discrete MIDI fields require integral unitless
values before conversion. Fractional musical pitch remains supported. Exact
stretch factors retain rational score time. Plugin normalized controls are scalars.

Pattern expansion preflights each event stream and owned payload before repeat,
fit or aggregation. Defaults are 200,000 notes/controllers/raw events each and
256 MiB of logical event/payload storage, including generated identity strings.
Hosts can lower these through `Evaluator::expansion_limits`. Oversized results
produce diagnostics, never truncated music. These composition limits are separate
from realtime simultaneous-voice and per-block event capacities.

`min` and `max` require compatible units and return the selected quantity without
losing its exactness. `abs`, `floor`, and `round` retain the input unit (`ms` is
normalized to seconds, so rounding acts on seconds). `sin`, `cos`, and `pow`
require scalars. Addition, subtraction, comparisons and remainder require matching
units; beats and bars interoperate at four quarter beats per bar. Multiplication
requires at least one scalar; division accepts a scalar divisor or matching units
(the latter yields a scalar). For example `max(1b,2b)+1b` is `3b`, while
`min(1b,1s)` and `1b*2b` are errors.


Patterns support `.repeat(n)`, `.transpose(semitones)`, `.gate(value)`, `.scale_gate(factor)`, `.velocity(value)`, `.gain(factor)`, `.at(beat)`, `.slice(start,end)`, `.stretch(factor)`, `.fit(duration)`, `.reverse()`, `.invert(center)`. `seq([a,b])` and `stack([a,b])` compose them. `rest(duration)` keeps intentional silence. `map`, `filter`, `fold`, `sort_by`, `range`, `len`, `merge` work on ordinary values. `sort_by(list,fn(item)=>key)` sorts stably by numeric or string keys; numeric keys must have compatible units. `range(end,start=0,step=1)` retains exact arithmetic and accepts compatible dimensional bounds/steps. Its omitted stride is one in the end value's original unit. Arrays and records are immutable shared values, so passing collections into callbacks does not copy their contents.

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

`merge(a, b, ...)` shallowly combines any number of records, with later records
winning on duplicate keys. For example, `merge({x:1}, {y:2}, {x:3})` returns
`{x:3, y:2}`. Nested records are replaced as whole values, and inputs remain
unchanged. `merge()` returns `{}`; `merge(record)` returns an equivalent record.
The first two arguments still accept the names `base` and `overrides`; additional
records are positional.

`.gate(0.6)` sets every note's key-hold duration to 60% of its written duration. `.scale_gate(0.5)` halves each existing gate, preserving articulation differences: gates 0.4 and 0.8 become 0.2 and 0.4. Both require a positive finite value; neither changes note placement, written duration, pedal, or release offsets. Use `.refine(selector,{gate:0.6})` to set selected notes.

`chords("F#m D A E", each=2bars)` makes harmony. `voicelead(harmony, low=48, high=84, center=64)` chooses registers. `arpeggiate(harmony, [0,2,1,2], 1/2b)` makes accompaniment. `chord("F#m7")` returns pitches. `pitch("F#4")` returns a MIDI pitch. `drums({kick:"X...X...X...X...",snare:"....X.......X...",hat:"x.x.x.x.x.x.x.x."})` makes semantic hits in one bar; X is accented, x ordinary, g ghost. Drum tracks use `kit()` and can override voices with `kit("default", {snare:sample("snare.wav",{one_shot:true})})`.

Pitch text accepts `#`, `b`, `##` and `bb` in `pitch`, `note` and `phrase`,
as well as chord roots and slash basses. Double accidentals shift the named
natural by two semitones, including across octave boundaries: `C##5` and `Ebb5`
both sound MIDI 74, `B##4` sounds 73, and `Cbb5` sounds 70. For example,
`phrase("[A#4 C##5 E#5 G#5]:q")` spells A-sharp dominant seventh directly.
The existing numeric pitch representation and MIDI range checks still apply;
pitch display uses its canonical spelling rather than retaining the input text.

`.hand("left")`, `.voice("melody")`, `.dynamics(from,to)`, `.humanize(4ms,0.02,seed=1)`, `.swing(0.57)` and `.rubato(20ms)` express performance. `pedal(harmony, depth=0.65)` returns a control pattern; combine it with notes using `stack([notes,pedal(harmony)])`. The piano policy checks all voices together. `track(...,{policy:"piano",reach:12,strict:true})` opts into constraints with any sound source. Synth tracks are unconstrained.

`.tag("last","echo")`, `.annotate("tag:echo",{purpose:"answer"})`, `.select(...)`, `.reject(...)`, `.refine(...,{velocity:0.8})` preserve annotations through reuse. Selectors include all/first/last, `tag:name`, `voice:name`, arrays (union), records (intersection), and `fn(n) => ...` predicates. `sample_zone` is a reserved sampler attack annotation selecting an absolute zero-based zone index; see `muz docs production` for validation and the source pinning recipe. Score inspection retains tags and data; performance inspection shows the timed events.

`pattern.notes` is an array of ordinary records with `at`, `duration` (beats),
`pitch`, `velocity`, `gate`, `release`, `hand` (or null), `voice`, `key`, `tags`,
`data`, and `offset`/`release_offset` (seconds). Transform the material first, then
select its notes and use `map`/`filter`/`fold` to derive any desired points or values.
`seconds_at(position,timing={})` resolves a beat/bar position to seconds (seconds pass through unchanged) using the
same tempo interpretation as rendering. Share a record such as
`let timing = {tempo:120,tempos:[[8b,90]]};` with the song's `tempo` and `tempos`
fields. Defaults are 120 BPM and no changes. Converted seconds and exposed performed
offsets are finite inexact values, retaining the renderer’s floating-point precision
when combined with other times. Source seconds literals remain exact, and a seconds
argument to `seconds_at` passes through with its exactness unchanged. A performed attack is
`seconds_at(n.at,timing)+n.offset`; a key release is
`seconds_at(n.at+n.duration*n.gate,timing)+n.offset+n.release_offset`.


`track("lead", material, synth("glass-lead"), {gain:-3,pan:0.1,chain:[fx("eq",{frequency_hz:2400,gain_db:2,q:0.7})],sends:{hall:-15}})` connects music to sound. `song` contains title, tempo, meter, sections, tracks, buses, master, automation and tail. Composer functions can derive ordinary automation curves from note records; see `muz docs production`. Unknown song/track fields are errors. `section("chorus",16bars)` names the next span; it does not implicitly place notes.

The standard library is readable source in `std/`. The prelude exposes both
functions and data as ordinary bare-name values, so its records and arrays can be
passed, indexed and merged without an explicit import. Extend it for musical
habits instead of copying large blocks of event data. Evaluation has bounded
steps, call depth and event counts; failures leave the live session intact.
# MIDI interchange

`midi("part.mid",track=1)` imports a musical sequence as a pattern with channel data, controllers and ordinary channel events. Without a track selection, synchronous format-0/1 tracks combine. Format-2 sequences require explicit selection; place independent patterns yourself. `midi_tempos(path,track=...)` supplies beat/BPM points for the song's `tempos`. `notes_only(pattern)` intentionally strips other events when reusing a melody on a different destination.

`muz midi export song.muz -o song.mid` exports a multitrack musical MIDI file. `muz midi inspect part.mid -o part.json` and `muz midi encode part.json -o part.mid` expose structural formats 0/1/2, PPQN/SMPTE division, delta times and opaque valid meta/SysEx payloads. Structural conversion does not promise byte-identical encoding. SMPTE files retain clock structure here; converting them to beat-based composition requires an explicit musical tempo interpretation.

# Deeper composition

`scale("E","harmonic_minor",octave=3)` returns pitches. Modes include major/minor, church modes, melodic/harmonic minor, pentatonics and whole tone. `degree(key,1)` is the tonic; degree numbers continue across octaves. `diatonic_chord(key,5,voices=4)` stacks thirds. `pattern.diatonic_transpose(key,steps)` moves by scale steps while retaining chromatic inflections. `std/tonal` supplies concise degree-based melody/progression helpers; `std/piano` supplies accompaniment and performance functions.

`voicelead` searches across the whole supplied phrase, preserving chord pitch classes. It considers inversions/octave placements, common tones, register, smooth movement and a penalty for parallel fifths/octaves. It retains 64 candidates per chord, with at most eight voices and 512 chord attacks per call; the result is the best path within that bounded candidate set. Tag a note `fixed` or annotate `{anchor:true}` to pin its pitch. Keep roles/tags on individual notes to inspect their resulting placement.

`reharmonize(harmony,melody,["Em","Cmaj7","Am","B7"])` explicitly chooses chords for existing harmonic slots. Held melody durations, metrical position and root continuity affect the choice. A `fixed` harmony slot retains its chord symbol. Melody is unchanged. `reharmonizations(harmony,melody,palette,count=3)` returns up to five ranked `{harmony,score}` alternatives by varying the first unanchored slot and optimizing the rest. Bind a chosen `.harmony` in source and audition it; there is no automatic acceptance. `muz eval choices.muz` inspects plain musical values without preparing audio. This is assistance, not a stylistic guarantee. `examples/tonal-assistance.muz` is a small starting point.

`split(pattern,selector)` returns `{selected,remaining}` patterns with the same span. Put those in separate tracks to isolate a phrase ending or a voice before shared audio mixing. Channel controls stay with `remaining`; add appropriate controls to the new layer deliberately.

`pattern.express({tuning:[[0,0],[0.8,0],[1,1]],brightness:[[0,0.3],[1,0.8]]},selector="last")` attaches note-specific expression. Positions are phases from 0 to 1 of each performed gate. A constant value also works. Curves survive repeat/transpose/placement and are carried with already-sounding notes through compatible reloads. Use CLAP native-note instruments or native voice patches for all expression kinds; preset synths and samplers support volume, expression, pan and tuning. `volume` is 0..4, `tuning` is semitones ±120, and pan/vibrato/expression/brightness/pressure use 0..1. Pan 0.5 is center. Each note has at most 32 points across these controls. Unsupported destinations fail explicitly with the track and instrument kind.

`format("layer{}_rr{}.flac",[layer,rr])` and `str(value)` make compact kit/asset declarations possible. These are string helpers, not file-system globbing. Asset paths are relative to the module declaring them, and a function body evaluates in its defining module, so `contrib/` packs resolve against their own installed assets.

# Explicit MIDI values

`note_on("C4",velocity=0.7,at=0b,channel=0)`, `note_off("C4",at=2b)`, `cc(64,90,at=1b)`, `bank(0)`, `program(12)`, `bend(-0.2)`, `pressure(0.4)` and `poly_pressure(60,0.4)` return composable event patterns. Channels are zero-based 0..15; program values 0..127, bank 0..16383, CC values are integer 0..127. Bend is -1..1 of the destination's configured bend range; pressure/velocities are 0..1. `.channel(2)` explicitly moves musical notes and channel controls together. Conflicting simultaneous CC values and ambiguous overlapping raw same-key note-ons are errors. Raw notes must have matching releases and bypass piano allocation.

`sysex([126,127,9,1])` adds framing F0/F7 around a 7-bit payload. `meta(type,[bytes])` and `opaque([bytes])` provide structural escape hatches. A file whose final value is a pattern can be passed directly to `muz midi export`, including these events; no instrument is required. Such direct pattern exports use 960 PPQ and a 120 BPM interpretation for millisecond offsets. Structural JSON conversion preserves arbitrary supported SMF timing/division. Runtime stereo plugin adapters accept ordinary channel events; they reject SysEx/meta/opaque events they cannot consume. See `examples/midi-messages.muz`.

## Arranging passages

`use "std/arrange" as a;` provides ordinary source records and functions.
`a.passage(span, parts, gestures=[], tempos=[])` holds local material;
`a.part("lead", pattern)` assigns it to a continuing song track.
`a.sequence([a.occurrence("verse", verse), a.occurrence("chorus", chorus)])`
places each occurrence using its logical span. `a.group(sequence)` nests an
arrangement. Pickups and note/effect tails do not change the sequencing span.

`a.edit(passage,"lead",fn(p)=>...)` changes that value's part, leaving the shared
input available for other occurrences. `a.require(p,selector,count=1)` diagnoses
a pinned selection that no longer matches; a rule such as `"last"` intentionally
follows whichever note is last. `a.build(form, settings, gestures=[])` fills the patterns of
the declared tracks and generates section labels, tempo points and automation.
The same track/instrument/effect graph continues across passage boundaries.
The optional third argument supplies whole-arrangement gesture callbacks, useful
for policies such as unioning send windows across passage boundaries.

Gesture functions receive `{name,start,span,parts,timing}` after placement and
after assembling the full tempo map. `a.material(context,"lead")` supplies the
placed pattern, including its namespaced keys. Derive any automation from those
notes. After editing a group, child gestures see their own final edited layers,
including changed timing, articulation and deleted or replaced notes. This holds
through multiple levels of nesting; siblings and other uses of the original
passage remain independent. A gesture must handle an empty selection explicitly,
or use `a.require` when removing its target should fail.

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

See `examples/revision-workflow.muz` for reusable material, a meter-aware
arrangement, a sparse final occurrence, two tag-derived parameter gestures and
named comparison/delivery collections. The starter from `muz new` uses the same
arrangement functions.
