# SLOP NOTICE

This project is 99.99% LLM-Slop! This disclosure is probably the only human-written text in the whole thing.

# muz

`muz` is a code-driven music production engine. You write a song in `.muz` source, and one Rust program evaluates the musical material, prepares instruments and effects, and renders audio. The same source can drive a live session for audition and revision. There is no graphical editor or recording workflow in this repository.

The language is for both composition and production. A source file can define phrases, chord progressions, drum grids, reusable passages, performance timing, instruments, buses, effects, automation, and delivery variants. Its standard library is readable `.muz` source in [`std/`](std/); the compiler and audio engine are in [`src/`](src/). The [language guide](docs/language.md) explains the source model, and the [production reference](docs/production.md) covers the audio graph and rendering.

## Try it

Linux is the current desktop host platform. Install the CLI on your PATH with `cargo install --path .`, or build it at `target/release/muz` with `cargo build --release`. Offline checks and renders need no audio service; live playback uses PipeWire.

```sh
muz new my-song
muz check my-song/song.muz
muz render my-song/song.muz -o mix.wav
```

`muz new` creates an editable song that uses a native synth and effect, so it needs no downloaded samples or plugins. The generated file shows a reusable passage, two named occurrences, an edit to one occurrence, and a delivery recipe. You can also inspect it with `muz inspect FILE`, format it with `muz fmt FILE`, or read the bundled references with `muz docs language` and `muz docs production`.

This repository contains the engine, standard library, examples, and optional instrument setups. The files in [`examples/`](examples/) demonstrate individual techniques rather than serving as compositions or regression fixtures.

## What it currently does

- **Compose in source.** Build patterns from notes, phrases, harmony, drums, MIDI, and functions; transform and reuse them without flattening the song into a fixed event list. Named passages and occurrences support local revisions. The [language](docs/language.md) and [performance](docs/performance.md) references cover these operations.
- **Shape a performance.** Express velocity, articulation, timing, pedals, note tags, and event edits in source. A bounded piano policy can check hand and finger constraints; it is a compositional aid, not a guarantee of playability.
- **Produce and render.** Use native synths, samplers, voice graphs, effects, sends, buses, sidechains, and automation. External VST3 and CLAP plugins are supported as well. Render a mix, stems, a section, or a named batch; analyze rendered audio and export MIDI. See [production](docs/production.md) and [native sound design](docs/synthesis.md).
- **Audition changes.** `muz serve FILE` watches source and selected assets, plays through PipeWire, and exposes transport, inspection, and render commands over a local Unix socket. A failed edit leaves the last accepted session running. `muz serve FILE --headless` provides the session and render controls without playback.
- **Hand work to a DAW.** `muz export FILE -o song.dawproject` writes an editable DAWProject archive and a fidelity report. This is a one-way handoff with documented limits, not a lossless round trip; read the [DAWProject reference](docs/dawproject.md) before relying on an export.

The default build includes the desktop CLI. The Rust library can also be built without desktop dependencies for an embedding host that supplies source documents, assets, and audio output. That interface, including inspection and playable snapshots, is described in [embedding](docs/embedding.md).

## Working with a live session

```sh
muz serve my-song/song.muz --stopped
muz audition opening
muz render --socket /tmp/muz.sock --section opening -o opening.wav
muz jobs
muz shutdown
```

The server accepts edits to source while it runs. `muz inspect FILE --view performance --track lead` pages the compiled performance; `muz call '{"command":"status"}'` uses the same newline JSON control protocol available to other clients. See [production](docs/production.md) for transport, render jobs, and delivery collections.

## Optional sounds and project status

[`contrib/`](contrib/) contains source mappings and installers for third-party sample libraries and plugins. External content is downloaded into ignored asset directories; it is not shipped with the engine. Each pack documents its upstream, license, and installation requirements. Projects choose and document their own assets.

This is pre-alpha software. Linux is the exercised desktop target; plugin coverage is limited, musical searches and event/voice counts are bounded, and compatibility and future-render reproducibility are not guaranteed. There is no GUI, recording or comping, or time stretching. The [composer friction log](docs/composer-friction.md) tracks open engine and language issues encountered during actual composition.
