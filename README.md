# muz

A headless music production studio in one Rust binary. **v1 / 0.1.0 pre-alpha is implemented**, with five finished, freely editable pieces. See the bundled [language guide](docs/language.md) and the [production](docs/production.md) reference.

Write reusable musical material, shape piano/drum/synth performance, connect instruments and effects, automate the mix, live-reload source, and bounce audio. No Python, ffmpeg, GUI or separate music compiler is required to write or render. The source language and standard library ship inside `muz`.

Build with `cargo build --release`, or `cargo install --path .` to put `muz` on PATH. Linux is the current host platform. Playback uses PipeWire; `serve --headless` and offline work need no running audio service. External sample libraries and plugins are optional inputs selected by each project.

```sh
muz new my-song
muz check my-song/song.muz
muz render my-song/song.muz -o mix.wav
muz analyze mix.wav
muz batch my-song/song.muz delivery -o out/delivery
muz serve my-song/song.muz --stopped
muz audition opening
muz render --socket /tmp/muz.sock --section opening -o audition.wav
muz jobs
muz shutdown
```

Run `muz fmt song.muz material.muz` to format source, or add `--check` to check without writing. The formatter uses four-space indentation, consistent spacing, and a 100-column target, with long calls, collections, and method chains split across lines. Multichannel `drums` literals use one lane per row with aligned pattern strings:

```muz
let beat = drums({
    kick:     "X...X...X...X...",
    snare:    "....X.......X...",
    open_hat: "......x.......x."
});
```

Lane order, step counts, string contents, and comments are preserved. An indivisible
token (such as a long string or identifier) or comment may exceed the width target.
Blank lines between statements and between collection item groups are retained,
with consecutive blank lines reduced to one. Short scalar arrays stay compact and
fill rows when long; expanded arrays of records, calls, or curve points use one
item per row. Pattern characters represent matching times across lanes when their
step counts match; the formatter never pads or resamples a pattern. Spaces and
`|` inside drum strings can be added manually as visual separators and are
preserved as written.

Long expressions wrap at definition boundaries and between operators, keeping
higher-precedence terms together where they fit:

```muz
fn at_chorus(at) =
    (at >= 96b && at < 160b)
    || (at >= 256b && at < 320b)
    || (at >= 384b && at < 464b);
```

Calls stay inline when they fit. A single expanding collection or callback body
can stay attached to its call, with short trailing arguments on its closing line
(for example, `], "smooth")` for a curve). More complex calls put each argument
on its own line and align the closing parenthesis with the call. Callback
introductions stay together when they fit:

```muz
fn shape(p) = p.map_notes(fn(n) => {
    gate: if n.duration < 1b { 0.85 } else { 0.98 },
    velocity: clamp(n.velocity + phrase_level(n.at))
});
```

Short conditional blocks have spaces inside their braces. When a conditional's
branches expand, both branches use multiline braces. Longer method chains put
each method on a continuation line; a short suffix can stay attached to a
multiline receiver. Formatting preserves punctuation, including existing trailing
commas, and a second formatting pass produces the same text.

Start with phrases, chords, grids and synth presets. Add functions/imports, voice leading, piano fingering, grooves/pedals and note tags as needed. Production has native EQ/space/dynamics, real sidechains, latency-compensated routes/racks, samples, VST3/CLAP and note-addressed native voice graphs. `muz devices list`, `muz devices inspect eq`, `muz docs synthesis` and the small files in [examples/](examples/) disclose the deeper controls.

`serve` watches source, imports and selected assets. Invalid saves retain the accepted session; compatible devices and held-note obligations survive normal edits. Background bounces use another `muz` process and record the accepted source/revision. `muz call '{"command":"status"}'` exposes the same newline JSON protocol used by agents. Unix socket permissions are 0600. See [production](docs/production.md) and `muz docs workflow` for transport/render details.

