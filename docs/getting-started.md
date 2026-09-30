# Getting started

Create and render your first muz project with native instruments. You need no
samples or plugins for this guide. Commands below run from the `muz-core`
checkout unless a different terminal is specified.

[Documentation index](README.md) · Offline: `muz docs getting-started`

## Contents

- [Install the CLI](#install-the-cli)
- [Create a project](#create-a-project)
- [Understand the generated song](#understand-the-generated-song)
- [Check, inspect, and render](#check-inspect-and-render)
- [Audition edits live](#audition-edits-live)
- [Next steps](#next-steps)

## Install the CLI

The desktop CLI is exercised on Linux. Building it requires:

- Rust/Cargo and a native C/C++ build toolchain.
- `pkg-config`, with the ALSA and PipeWire development libraries and headers.
- Clang/libclang for the PipeWire/SPA bindings generated during the build.

Package names vary by distribution. A running PipeWire service is needed for
live playback, but not for offline checks or renders.

Install the desktop CLI, CLAP plugin, and contrib pack files. Cargo's binary
directory must be on your `PATH`:

```sh
./install.sh
muz --help
```

`install.sh` copies contrib packs to `$XDG_DATA_HOME/muz/contrib` (default
`~/.local/share/muz/contrib`), including any existing checkout assets. It retains
assets already downloaded into that destination on reinstall. It downloads no
sample libraries; follow [pack setup](../contrib/README.md#install-and-import).
`CLAP_DIR` overrides the plugin destination. Plain `cargo install --path .`
installs only the CLI; it can use an existing installed contrib tree, or an
explicit `MUZ_CONTRIB_DIR`.

Alternatively, build in the checkout:

```sh
cargo build --release
./target/release/muz --help
```

If you choose the second option, replace `muz` in this guide with
`./target/release/muz`. A missing ALSA/PipeWire metadata error points to the
development packages or their `pkg-config` search path. A missing libclang error
points to the Clang shared library needed during binding generation.
For a host without desktop dependencies, see [embedding](embedding.md).

## Create a project

Choose a directory for your project. This example uses `~/music/my-song`; muz
does not require a particular parent directory, repository, or folder layout.

```sh
muz new ~/music/my-song
```

This creates `song.muz` and prints its path. Choose an unused project directory:
`muz new` refuses to replace an existing `song.muz`. The project can hold source
modules and any notes about asset requirements.
The render commands below use this checkout's ignored `out/` directory; choose
another output location if you prefer.

## Understand the generated song

Open `~/music/my-song/song.muz`. It contains:

| Binding | Role |
| --- | --- |
| `theme` | A short phrase, with its last note tagged `echo`. |
| `echo` | A function that derives delay-send automation from those tagged notes. |
| `phrase_section` | A reusable passage with the lead part and its automation gesture. |
| `form` | Two named occurrences, `opening` and `return`; the return edits the tagged note. |
| `main` | The built song, with a native synth, delay bus, master processing, and tail. |
| `delivery` | A named render collection containing a 24-bit master. |

The file's final expression is `main`, so ordinary check/render commands use that
song. `muz batch` can select the named `delivery` collection instead. Functions
and passages are explained in the [language guide](language.md); you can begin by
changing pitches in `theme` or the song's tempo.

## Check, inspect, and render

```sh
muz check ~/music/my-song/song.muz
muz inspect ~/music/my-song/song.muz --view sections
muz render ~/music/my-song/song.muz -o out/my-song/mix.wav
```

`check` evaluates the source, validates the performance, and prepares the audio
graph. Errors identify the relevant source location. `inspect` shows the named
sections without playing them. `render` writes a stereo WAV and prints a render
report; its default output format is 48 kHz float32. Open the WAV in your audio
player to hear it. A successful render is sufficient routine verification.

To render just the opening or use the generated delivery recipe:

```sh
muz render ~/music/my-song/song.muz --section opening -o out/my-song/opening.wav
muz batch ~/music/my-song/song.muz delivery -o out/my-song/delivery
```

A section render preserves preceding instrument and effect history. The
[workflow guide](workflow.md#rendering-and-delivery) explains render scopes,
tails, stems, and queued jobs.

## Audition edits live

Start the server in one terminal and leave it running:

```sh
muz serve ~/music/my-song/song.muz --stopped
```

In another terminal:

```sh
muz audition opening
muz status
muz stop
```

`audition` loops the named section and starts playback through PipeWire. Edit and
save the source while the server runs. It watches the source and prepares
revisions; a failed edit leaves the last accepted session running. Read its
error, correct the source, and save again.

When finished:

```sh
muz shutdown
```

Use `serve --headless` if you need session inspection and queued rendering without
audio playback. Multiple servers need distinct `--socket` paths, also supplied
to their client commands; see [live sessions](workflow.md#live-sessions).

## Next steps

- [Language](language.md): notes, functions, transformations, and arrangements.
- [Workflow](workflow.md): inspection, transport, comparisons, and delivery.
- [Instruments](instruments.md): samples, clips, plugins, and asset setup.
- [Production](production.md): effects, routing, sidechains, and automation.

Run `muz docs` to find the same guides offline.
