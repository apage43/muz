# SLOP NOTICE

This project is 99.99% LLM-Slop! This disclosure is probably the only human-written text in the whole thing.

# muz

A headless music production studio in one Rust binary. See the bundled [language guide](docs/language.md) and the [production](docs/production.md) reference.

Write reusable musical material, shape piano/drum/synth performance, connect instruments and effects, automate the mix, live-reload source, and bounce audio. No Python, ffmpeg, GUI or separate music compiler is required to write or render. The source language and standard library ship inside `muz`.

Build with `cargo build --release`, or `cargo install --path .` to put `muz` on PATH. Linux is the current host platform. Playback uses PipeWire; `serve --headless` and offline work need no running audio service. External sample libraries and plugins are optional inputs selected by each project.

Library consumers can build the native synthesis/compiler without desktop
dependencies and provide unsaved source documents through a loader. See the
[embedding reference](docs/embedding.md) for WASM builds and editor interfaces.

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

`muz inspect song.muz` returns a bounded summary. Detailed views are revision-aware
pages; for example, use `muz inspect song.muz --view performance --track lead --offset 0 --limit 100`.
Each page reports `total` and `next`, is limited to 1,000
rows and 1 MiB of row payload, and offers separate patch-node, automation-point,
location and dense-performance overview views. See the
[embedding reference](docs/embedding.md#editor-inspection) for the page contract
and control-socket fields.

Pieces live in a sibling `muz-projects` checkout, not in this repository: this is the engine alone, so it can be published without anyone's music. Start one with `muz new ../muz-projects/my-song` and render it with `muz render ../muz-projects/my-song/song.muz -o mix.wav`.

## Native sound design

`std/signal` builds reusable nested voice graphs; `std/synthesis` supplies editable
stereo, legato, sampled-layer, resonant and evolving instrument recipes. Patches
support explicit tails, multistage envelopes, per-note modulation, zone readers,
loop crossfades/reverse regions, and antialiased waveshaping. See the
[synthesis reference](docs/synthesis.md) and the small
[instrument-design example](examples/instrument-design.muz).

## Instrument library

`contrib/` holds ready-to-use setups for third-party sample libraries and
plugins: a native zone mapping in source, an installer that downloads and
verifies the content it needs, and notes on upstream, version and license.
Content is never committed; it lands in the ignored `contrib/<pack>/assets/`.
Import a pack like any other module:

```muz
use "contrib/virtuosity-drums/kit" as vd;
```

`use "contrib/<pack>/<module>"` resolves inside the engine checkout's `contrib/`
directory, or in `$MUZ_CONTRIB_DIR` when set. Each pack's README names its
upstream, license and size; `python3 contrib/<pack>/install.py` installs or
verifies its content. A pack's functions evaluate in the pack's own directory, so
its relative sample paths resolve to its installed assets.

Generated audio is ignored by git. Each piece documents its render commands and required assets, and downloaded libraries or recordings stay out of git. Pieces are not test fixtures. Small synthetic tests protect the tricky invariants, with optional real-plugin/PipeWire workflow checks under the isolated `tools` uv project.

Report composer friction in the [canonical live log](docs/composer-friction.md).
Reports accompany the piece commit that exposes them and are removed in the
commit that fixes them; the log contains unresolved problems only.

This is pre-alpha software: Linux and a small exercised plugin set, bounded musical searches and voice/event budgets, a modest piano model, and no compatibility or future-render reproducibility guarantees. No GUI, recording/comping or time stretching is included.

Before the next piece, use reusable passages, occurrence-specific edits and
named comparison bounces. [The revision example](examples/revision-workflow.muz)
shows the complete workflow; `muz batch ... --match-levels` generates a local
listening page without changing production audio.
