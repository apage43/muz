# The muz language

A file evaluates to `song({...})`. `let`, lexical functions, defaults, named arguments, arrays, records, module imports and higher-order functions provide reuse. `use "material.muz" as m;` imports a local module; `use "std/music" as m;` imports the bundled library. Functions return their last expression. Records use `{key: value}`; functions use `fn name(arg, optional = value) = expression;` or `fn(x) => expression`.

`phrase("C4:q D4:e E4:e | [F4 A4]:h r:h")` makes material. Note lengths: w/h/q/e/s/t; dotted or numeric rational lengths also work. Bar lines are visual separators. `@name` after a note tags it. `1/3b` is exact musical time; `2bars`, `20ms`, `1s`, `-12dB`, `500Hz`, `1kHz`, `50%` carry units. Bars in generic pattern operations mean four quarter beats; section durations use the declared song meter. Timeline positions start at zero.

Patterns support `.repeat(n)`, `.transpose(semitones)`, `.gate(factor)`, `.velocity(value)`, `.gain(factor)`, `.at(beat)`, `.slice(start,end)`, `.stretch(factor)`, `.fit(duration)`, `.reverse()`, `.invert(center)`. `seq([a,b])` and `stack([a,b])` compose them. `rest(duration)` keeps intentional silence. `map`, `filter`, `fold`, `range`, `len`, `merge` work on ordinary values.

`chords("F#m D A E", each=2bars)` makes harmony. `voicelead(harmony, low=48, high=84, center=64)` chooses registers. `arpeggiate(harmony, [0,2,1,2], 1/2b)` makes accompaniment. `chord("F#m7")` returns pitches. `pitch("F#4")` returns a MIDI pitch. `drums({kick:"X...X...X...X...",snare:"....X.......X...",hat:"x.x.x.x.x.x.x.x."})` makes semantic hits in one bar; X is accented, x ordinary, g ghost. Drum tracks use `kit()` and can override voices with `kit("default", {snare:sample("snare.wav",{one_shot:true})})`.

`.hand("left")`, `.voice("melody")`, `.dynamics(from,to)`, `.humanize(4ms,0.02,seed=1)`, `.swing(0.57)` and `.rubato(20ms)` express performance. `pedal(harmony, depth=0.65)` returns a control pattern; combine it with notes using `stack([notes,pedal(harmony)])`. The piano policy checks all voices together. `track(...,{policy:"piano",reach:12,strict:true})` opts into constraints with any sound source. Synth tracks are unconstrained.

`.tag("last","echo")`, `.annotate("tag:echo",{purpose:"answer"})`, `.select(...)`, `.reject(...)`, `.refine(...,{velocity:0.8})` preserve annotations through reuse. Selectors include all/first/last, `tag:name`, `voice:name`, arrays (union), records (intersection), and `fn(n) => ...` predicates. Score inspection retains tags and data; performance inspection shows the timed events.

`track("lead", material, synth("glass-lead"), {gain:-3,pan:0.1,chain:[fx("eq",{frequency_hz:2400,gain_db:2,q:0.7})],sends:{hall:-15}})` connects music to sound. `song` contains title, tempo, meter, sections, tracks, buses, master, automation, throws and tail. Unknown song/track fields are errors. `section("chorus",16bars)` names the next span; it does not implicitly place notes.

The standard library is readable source in `std/`. Extend it for musical habits instead of copying large blocks of event data. Evaluation has bounded steps, call depth and event counts; failures leave the live session intact.
