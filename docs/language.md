# The muz language

A file evaluates to `song({...})`. `let`, lexical functions, defaults, named arguments, arrays, records, module imports and higher-order functions provide reuse. `use "material.muz" as m;` imports a local module; `use "std/music" as m;` imports the bundled library. Functions return their last expression. Records use `{key: value}`; functions use `fn name(arg, optional = value) = expression;` or `fn(x) => expression`.

`phrase("C4:q D4:e E4:e | [F4 A4]:h r:h")` makes material. Note lengths: w/h/q/e/s/t; dotted or numeric rational lengths also work. Bar lines are visual separators. `@name` after a note tags it. `1/3b` is exact musical time; `2bars`, `20ms`, `1s`, `-12dB`, `500Hz`, `1kHz`, `50%` carry units. Bars in generic pattern operations mean four quarter beats; section durations use the declared song meter. Timeline positions start at zero.

Patterns support `.repeat(n)`, `.transpose(semitones)`, `.gate(value)`, `.scale_gate(factor)`, `.velocity(value)`, `.gain(factor)`, `.at(beat)`, `.slice(start,end)`, `.stretch(factor)`, `.fit(duration)`, `.reverse()`, `.invert(center)`. `seq([a,b])` and `stack([a,b])` compose them. `rest(duration)` keeps intentional silence. `map`, `filter`, `fold`, `range`, `len`, `merge` work on ordinary values.

`.gate(0.6)` sets every note's key-hold duration to 60% of its written duration. `.scale_gate(0.5)` halves each existing gate, preserving articulation differences: gates 0.4 and 0.8 become 0.2 and 0.4. Both require a positive finite value; neither changes note placement, written duration, pedal, or release offsets. Use `.refine(selector,{gate:0.6})` to set selected notes.

`chords("F#m D A E", each=2bars)` makes harmony. `voicelead(harmony, low=48, high=84, center=64)` chooses registers. `arpeggiate(harmony, [0,2,1,2], 1/2b)` makes accompaniment. `chord("F#m7")` returns pitches. `pitch("F#4")` returns a MIDI pitch. `drums({kick:"X...X...X...X...",snare:"....X.......X...",hat:"x.x.x.x.x.x.x.x."})` makes semantic hits in one bar; X is accented, x ordinary, g ghost. Drum tracks use `kit()` and can override voices with `kit("default", {snare:sample("snare.wav",{one_shot:true})})`.

`.hand("left")`, `.voice("melody")`, `.dynamics(from,to)`, `.humanize(4ms,0.02,seed=1)`, `.swing(0.57)` and `.rubato(20ms)` express performance. `pedal(harmony, depth=0.65)` returns a control pattern; combine it with notes using `stack([notes,pedal(harmony)])`. The piano policy checks all voices together. `track(...,{policy:"piano",reach:12,strict:true})` opts into constraints with any sound source. Synth tracks are unconstrained.

`.tag("last","echo")`, `.annotate("tag:echo",{purpose:"answer"})`, `.select(...)`, `.reject(...)`, `.refine(...,{velocity:0.8})` preserve annotations through reuse. Selectors include all/first/last, `tag:name`, `voice:name`, arrays (union), records (intersection), and `fn(n) => ...` predicates. Score inspection retains tags and data; performance inspection shows the timed events.

`track("lead", material, synth("glass-lead"), {gain:-3,pan:0.1,chain:[fx("eq",{frequency_hz:2400,gain_db:2,q:0.7})],sends:{hall:-15}})` connects music to sound. `song` contains title, tempo, meter, sections, tracks, buses, master, automation, throws and tail. Unknown song/track fields are errors. `section("chorus",16bars)` names the next span; it does not implicitly place notes.

The standard library is readable source in `std/`. Extend it for musical habits instead of copying large blocks of event data. Evaluation has bounded steps, call depth and event counts; failures leave the live session intact.
# MIDI interchange

`midi("part.mid",track=1)` imports a musical sequence as a pattern with channel data, controllers and ordinary channel events. Without a track selection, synchronous format-0/1 tracks combine. Format-2 sequences require explicit selection; place independent patterns yourself. `midi_tempos(path,track=...)` supplies beat/BPM points for the song's `tempos`. `notes_only(pattern)` intentionally strips other events when reusing a melody on a different destination.

`muz midi export song.muz -o song.mid` exports a multitrack musical MIDI file. `muz midi inspect part.mid -o part.json` and `muz midi encode part.json -o part.mid` expose structural formats 0/1/2, PPQN/SMPTE division, delta times and opaque valid meta/SysEx payloads. Structural conversion does not promise byte-identical encoding. SMPTE files retain clock structure here; converting them to beat-based composition requires an explicit musical tempo interpretation.

# Deeper composition

`scale("E","harmonic_minor",octave=3)` returns pitches. Modes include major/minor, church modes, melodic/harmonic minor, pentatonics and whole tone. `degree(key,1)` is the tonic; degree numbers continue across octaves. `diatonic_chord(key,5,voices=4)` stacks thirds. `pattern.diatonic_transpose(key,steps)` moves by scale steps while retaining chromatic inflections. `std/tonal` supplies concise degree-based melody/progression helpers; `std/piano` supplies accompaniment and performance functions.

`voicelead` searches across the whole supplied phrase, preserving chord pitch classes. It considers inversions/octave placements, common tones, register, smooth movement and a penalty for parallel fifths/octaves. It retains 64 candidates per chord, with at most eight voices and 512 chord attacks per call; the result is the best path within that bounded candidate set. Tag a note `fixed` or annotate `{anchor:true}` to pin its pitch. Keep roles/tags on individual notes to inspect their resulting placement.

`reharmonize(harmony,melody,["Em","Cmaj7","Am","B7"])` explicitly chooses chords for existing harmonic slots. Held melody durations, metrical position and root continuity affect the choice. A `fixed` harmony slot retains its chord symbol. Melody is unchanged. `reharmonizations(harmony,melody,palette,count=3)` returns up to five ranked `{harmony,score}` alternatives by varying the first unanchored slot and optimizing the rest. Bind a chosen `.harmony` in source and audition it; there is no automatic acceptance. `muz eval choices.muz` inspects plain musical values without preparing audio. This is assistance, not a stylistic guarantee. `examples/tonal-assistance.muz` is a small starting point.

`split(pattern,selector)` returns `{selected,remaining}` patterns with the same span. Put those in separate tracks to isolate a phrase ending or a voice before shared audio mixing. Channel controls stay with `remaining`; add appropriate controls to the new layer deliberately.

`pattern.express({tuning:[[0,0],[0.8,0],[1,1]],brightness:[[0,0.3],[1,0.8]]},selector="last")` attaches note-specific expression. Positions are phases from 0 to 1 of each performed gate. A constant value also works. Curves survive repeat/transpose/placement and are carried with already-sounding notes through compatible reloads. Use CLAP native-note instruments or native voice patches. `volume` is 0..4, `tuning` is semitones ±120, and pan/vibrato/expression/brightness/pressure use 0..1. Pan 0.5 is center. Each note has at most 32 points across these controls. Unsupported destinations fail explicitly.

`format("layer{}_rr{}.flac",[layer,rr])` and `str(value)` make compact kit/asset declarations possible. These are string helpers, not file-system globbing. Asset paths are relative to the module declaring them.

# Explicit MIDI values

`note_on("C4",velocity=0.7,at=0b,channel=0)`, `note_off("C4",at=2b)`, `cc(64,90,at=1b)`, `bank(0)`, `program(12)`, `bend(-0.2)`, `pressure(0.4)` and `poly_pressure(60,0.4)` return composable event patterns. Channels are zero-based 0..15; program values 0..127, bank 0..16383, CC values are integer 0..127. Bend is -1..1 of the destination's configured bend range; pressure/velocities are 0..1. `.channel(2)` explicitly moves musical notes and channel controls together. Conflicting simultaneous CC values and ambiguous overlapping raw same-key note-ons are errors. Raw notes must have matching releases and bypass piano allocation.

`sysex([126,127,9,1])` adds framing F0/F7 around a 7-bit payload. `meta(type,[bytes])` and `opaque([bytes])` provide structural escape hatches. A file whose final value is a pattern can be passed directly to `muz midi export`, including these events; no instrument is required. Such direct pattern exports use 960 PPQ and a 120 BPM interpretation for millisecond offsets. Structural JSON conversion preserves arbitrary supported SMF timing/division. Runtime stereo plugin adapters accept ordinary channel events; they reject SysEx/meta/opaque events they cannot consume. See `examples/midi-messages.muz`.
