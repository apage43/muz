# muz documentation

muz turns source into a musical performance and an audio graph. Start with a
native instrument to learn the workflow, then add the composition and production
features your piece needs. These guides describe the current checkout.

Offline: `muz docs` or `muz docs index`. Use `muz COMMAND --help` for command flags
and `muz devices inspect DEVICE` for device parameters and ranges.

## Start here

1. [Getting started](getting-started.md) (`muz docs getting-started`): install,
   create a project, check it, render audio, and audition changes.
2. [Language](language.md) (`muz docs language`): build musical material and
   arrange it into a song.
3. [Performance](performance.md) (`muz docs performance`): control articulation,
   dynamics, timing, pedals, and expression.
4. [Instruments](instruments.md) (`muz docs instruments`) and
   [production](production.md) (`muz docs production`): choose sounds and connect
   them with effects, routing, and automation.
5. [Workflow](workflow.md) (`muz docs workflow`): inspect, revise, compare, and
   deliver a finished piece.

## Find a task

| I want to… | Read |
| --- | --- |
| Understand syntax, units, patterns, or imports | [Language](language.md) |
| Reuse a passage and edit one occurrence | [Arranging passages](language.md#arranging-passages) |
| Import or export MIDI | [MIDI interchange](language.md#midi-interchange) |
| Humanize notes or check piano constraints | [Performance](performance.md) |
| Install samples or configure a plugin | [Instruments](instruments.md), [contrib packs](../contrib/README.md) |
| Build a native instrument graph | [Synthesis](synthesis.md) (`muz docs synthesis`) |
| Set up sends, sidechains, racks, or automation | [Production](production.md) |
| Control a live session or page through events | [Workflow](workflow.md) |
| Render a section, stems, or comparison variants | [Rendering and delivery](workflow.md#rendering-and-delivery) |
| Move a song into a DAW | [DAWProject export](dawproject.md) (`muz docs dawproject`) |
| Understand the evidence behind DAW compatibility claims | [DAWProject validation](dawproject-validation.md) (`muz docs dawproject-validation`) |
| Build an editor or another host around muz | [Embedding](embedding.md) (`muz docs embedding`) |
| Import and play original SFZ programs | [Native SFZ instruments](sfz.md) |
| Inspect native SFZ realtime measurements | [SFZ performance qualification](sfz-performance.md) |
| Finish native SFZ corpus qualification | [Remaining SFZ qualification gates](native-sfz-plan.md) |
| Report an engine or language obstacle | [Composer friction](composer-friction.md) |

## Terms used in the guides

| Term | Meaning |
| --- | --- |
| Pattern | Musical events plus a logical span, before instrument assignment. |
| Score time | Exact beat positions and written durations. A literal bar is always four quarter beats. |
| Performed time | Scheduled attacks, releases, and controls after tempo and performance offsets are applied. |
| Track | A pattern connected to an instrument, with inserts and output routing. |
| Logical / physical track | The authored track / a compiled audio lane. A kit expands into physical tracks for its played voices. |
| Bus | A shared processing and routing destination; `master` is the final output chain. |
| Passage / occurrence | Reusable local material / one named placement of that material in an arrangement. |
| Accepted revision | The source version retained by the live session after successful preparation. Failed edits leave the last accepted session running. |
| Tap | Audio taken at a defined point in the graph, such as after a track's inserts and pan. |
| Tail | Extra render time for releases and effect decay after the musical material ends. |

## For contributors and agents

Read [AGENTS.md](../AGENTS.md) for repository boundaries and the verification
budget. Before composing, revising a piece, or changing the engine, read the
[friction protocol and live log](composer-friction.md). It is the canonical place
for unresolved engine and language issues.

| Location | Contents |
| --- | --- |
| `src/` | Rust language implementation, compiler, audio engine, CLI, and host APIs |
| `std/` | Editable `.muz` functions, instrument recipes, and musical policies |
| `examples/` | Independent demonstrations of individual techniques |
| `templates/song.muz` | Source copied by `muz new` |
| `tests/` | Focused engine and language tests using synthetic material |
| `crates/muz-clap/` | Native instrument/effect wrapper used by DAWProject exports |
| `contrib/` | Third-party instrument mappings and installers; content is downloaded separately |
| `out/` | Ignored local renders and other generated artifacts |

In the maintainer's workspace, pieces happen to live in the sibling
`muz-projects` checkout. That is a local convention for agents working here,
not a required layout for muz users.

The [blind comparison skill](../.agents/skills/muz-ab/SKILL.md) runs listening
trials; the [visualizer skill](../.agents/skills/muz-visualizer/SKILL.md) makes
videos synchronized to a piece's performed notes and master. Their instructions
apply when using those workflows.
