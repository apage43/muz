# SLOP NOTICE

This project is 99.99% LLM-Slop! This disclosure is probably the only human-written text in the whole thing.

# muz

muz is a code-driven music production engine. Write musical material and production
settings in `.muz` source, then check, audition, and render them with one Rust
program. The same source defines notes, performance timing, instruments, effects,
automation, and delivery variants.

Start with the [getting-started guide](docs/getting-started.md), or use the
[documentation index](docs/README.md) to find a task or reference.

## Install and try it

Linux is the exercised desktop platform. From this checkout, with the
[build prerequisites](docs/getting-started.md#install-the-cli) installed:

```sh
./install.sh
muz new ~/music/my-song
muz check ~/music/my-song/song.muz
muz render ~/music/my-song/song.muz -o out/my-song/mix.wav
```

The installer puts `muz` in Cargo's default binary directory and installs the
Muz CLAP plugin in the current user's default CLAP directory. Set `CLAP_DIR` to
choose a different plugin directory.

The generated song uses native instruments and effects; it needs no downloaded
samples or plugins. Offline checks and renders need no running audio service.
Live playback uses PipeWire:

```sh
muz serve ~/music/my-song/song.muz --stopped
```

Leave that process running and use `muz audition opening` in another terminal.
Read [the workflow guide](docs/workflow.md) for transport, live edits, inspection,
and render jobs. `muz docs` displays the documentation index offline.

## What you can build

| Task | Capabilities | Guide |
| --- | --- | --- |
| Compose | Notes, harmony, drum grids, MIDI, reusable passages, and local revisions | [Language](docs/language.md) |
| Shape a performance | Articulation, timing, pedals, note expression, and bounded piano checks | [Performance](docs/performance.md) |
| Choose or design sounds | Synths, samplers, programmable voice graphs, VST3 and CLAP plugins | [Instruments](docs/instruments.md), [synthesis](docs/synthesis.md) |
| Mix and deliver | Buses, effects, sidechains, automation, mixes, stems, and named batches | [Production](docs/production.md), [workflow](docs/workflow.md) |
| Hand off to a DAW | Editable DAWProject export with a fidelity report | [DAWProject](docs/dawproject.md) |
| Embed the engine | Rust library with host-supplied source, assets, and audio output | [Embedding](docs/embedding.md) |

## Repository layout

This checkout holds the engine in [`src/`](src/), readable source libraries in
[`std/`](std/), and small technique demonstrations in [`examples/`](examples/).
Optional third-party instrument setups live in [`contrib/`](contrib/README.md).
Their recordings and plugin binaries are installed separately.

Projects can live wherever you keep your music; `~/music/my-song` above is only
an example. This engine repository does not store personal pieces, generated
media, or external recordings. Contributors and agents working in this checkout
should read [AGENTS.md](AGENTS.md) and the
[composer friction protocol](docs/composer-friction.md) for its local conventions.

## Project status

muz is pre-alpha. Linux is the exercised desktop target, plugin coverage is
limited, and musical searches and runtime resources are bounded. Compatibility
and future-render reproducibility are not guaranteed. There is no graphical
editor, recording or comping workflow, or time stretching. The
[friction log](docs/composer-friction.md#open-reports) tracks unresolved engine
and language issues encountered during composition.
