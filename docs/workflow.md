# Checking, auditioning, and delivering a piece

Use this guide after [getting started](getting-started.md). Unless shown otherwise,
commands use a `song.muz` in the current directory. Substitute your project path
when working from the engine checkout. Output paths are relative to the command's
working directory; source asset paths follow their declaring modules.

[Documentation index](README.md) · Offline: `muz docs workflow`

## Contents

- [Evaluate, format, and check](#evaluate-format-and-check)
- [Inspect a song](#inspect-a-song)
- [Live sessions](#live-sessions)
- [Rendering and delivery](#rendering-and-delivery)
- [Named comparisons and delivery collections](#named-comparisons-and-delivery-collections)

## Evaluate, format, and check

| Command | Purpose |
| --- | --- |
| `muz eval material.muz` | Evaluate an ordinary source value without preparing audio. Useful for inspecting generated patterns or alternatives. |
| `muz fmt song.muz` | Format source in place. |
| `muz fmt --check song.muz` | Check formatting without rewriting. |
| `muz check song.muz` | Validate source, musical constraints, available assets/plugins, and the prepared graph. |
| `muz check song.muz --json` | Return machine-readable check results and diagnostics. |

A successful check reports note/track counts, expanded graph resources, and
remaining policy diagnostics. Error messages identify the declaring source;
see [diagnostics](language.md#diagnostics-and-limits). Plugin checks require the
actual plugin and any needed license/assets on this machine.

## Inspect a song

Inspection reads structure and events without playing audio:

```sh
muz inspect song.muz
muz inspect song.muz --view sections
muz inspect song.muz --view score --track lead --limit 100
muz inspect song.muz --view performance --track lead --section opening
muz inspect song.muz --view automation
```

The default view is a bounded summary. `score` shows authored notes and metadata;
`performance` shows scheduled notes and controls. A note key is its exact identity;
a display label is not a lookup key. Kits expose physical tracks such as `drums.kick`.

Most views return `{revision, view, rows, total, next}`. Pass `next` as
`--offset NEXT` to continue. Pages allow 1–1,000 rows and at most 1 MiB of serialized
row data. `--start-tick`/`--end-tick` select a score range; `--section` resolves the
section to that range. For dense material, use `performance_overview` before
requesting detailed ranges. The [embedding reference](embedding.md#paged-inspection)
lists views, filter semantics, and the legacy unfiltered `graph` exception.

To inspect an automation lane's points, use
`--view automation_points --track TARGET`. Native patches have separate
`patches`, `patch_nodes`, and `patch_detail` views.

## Live sessions

### Start and control playback

In one terminal:

```sh
muz serve song.muz --stopped
```

Leave it running. In another terminal, use:

| Command | Action |
| --- | --- |
| `muz status` | Read transport, revision, and device telemetry. |
| `muz play` / `muz stop` | Start or pause playback without rewinding. |
| `muz restart` | Seek to the audition-loop start or beginning, preserving playing state. |
| `muz seek --section opening` | Seek to a named section; `--beat` and `--tick` also work. |
| `muz audition opening` | Loop a named section and start playback. |
| `muz loop --off` | Disable looping. |
| `muz panic` | Silence active voices and effects without treating it as a rewind. |
| `muz shutdown` | Stop the server and cancel its jobs. |

Without `--stopped`, the server starts playing immediately. `--headless` provides
a silent transport clock, session controls, and rendering without PipeWire output.
Source and selected assets are watched. A failed edit leaves the last accepted
session running; fix the reported error and save again. Compatible changes can
retain sounding voices and processors; structural changes require prepared
replacements. See [embedding](embedding.md#live-revisions) for the host contract.

The applied revision starts at zero and advances only after the audio callback
accepts an edit. Source/watch generations track observations separately; a plugin
restart can advance revision without a source edit. While one revision is pending,
only the latest queued source candidate is retained. A rejected candidate does not
replace accepted source or processors, and a later receipt for an older valid edit
does not erase a newer source error.

Native output, plugin main-thread servicing/restarts, SFZ replay, and processor
retirement share the [native coordinator](embedding.md#native-output-coordinator).
Plugin restart requests rebuild accepted state without rereading an invalid file.
Normal startup reports unavailable audio instead of silently choosing headless.
Use `--headless` explicitly when a silent clock is intended.

Live seeks and loops restore overlapping notes and prior controllers. SFZ
transport additionally prepares bounded history/checkpoints off the audio thread.
An audition range is half-open; enabling it seeks its start, disabling preserves
position, and seeking outside it returns to its start. Non-SFZ transport cannot
recreate arbitrary prior oscillator or effect history; use a section bounce when
that history matters.

### Socket clients

The default Unix socket is `/tmp/muz.sock`. For multiple sessions, give each
server a distinct `--socket PATH` and pass the same path to its client commands.
`muz call` sends a newline JSON request through this interface:

```sh
muz call '{"command":"status"}'
muz call '{"command":"inspect","view":"performance","track":"lead","revision":7,"offset":0,"limit":100}'
```

Replace `7` with the revision you intend to inspect. Supplying a revision prevents
accidentally paging across edits; a mismatched revision fails. Successful responses
wrap the result as `{"ok":true,"result":...}`. Use the returned page's `next` for
continuation. [src/control.rs](../src/control.rs) defines the request variants.

Transport success keeps the existing `{"ok":true,"result":{"queued":true}}`
shape: it acknowledges admission, not callback application. Read status to observe
the applied transport; async callback rejection appears in `last_error` and the
server's rejected-event diagnostics. Output health fields report real backend
counters. `runtime_mapping_pending` suppresses device IDs while a newer callback
revision awaits coordinator promotion.

### Queued renders

```sh
muz render --socket /tmp/muz.sock --section opening -o opening.wav
muz jobs
muz cancel 1
```

Use the job ID returned by the submission instead of the illustrative `1` above.

Submission returns a job immediately. The server runs two renders concurrently
and queues up to 32 more in submission order. `muz jobs` reports `queued`,
`running`, `finished`, `failed`, or `cancelled`, with the source and accepted
revision captured at submission.

Later source edits do not change a queued job's musical/graph data. External
assets remain ordinary files, so changing their contents can still affect a job.
Duplicate active output paths and a full waiting queue are rejected.

Cancellation works for waiting and running jobs and preserves an existing output
file. Failed or cancelled workers release their slots. Shutdown cancels workers
and discards waiting jobs; the in-memory queue does not survive a server restart.

## Rendering and delivery

### Choose a render scope

```sh
muz render song.muz -o master.wav --format pcm24
muz render song.muz --section opening -o opening.wav
muz render song.muz --start 10 --seconds 5 -o excerpt.wav
```

Disk rendering reads the source on disk. To capture the live server's applied
revision, use `render --socket PATH` without a source argument; that queues a job.

| Option | Meaning |
| --- | --- |
| `--section NAME` | Render the named section with preceding context from song start (unless `--history` bounds it). |
| `--start SECONDS` | Set the clock-time output start; overrides the selected section's start. |
| `--seconds LENGTH` | Set the entire output duration, including any release audio; `--tail` does not extend it. |
| `--tail SECONDS` | Set release/effect time after the score or section end when `--seconds` is omitted; defaults to the song's `tail`. |
| `--history SECONDS` | Process only this much context before the output start instead of everything from the song start. Faster, but approximate; see [bounded history](#bounded-history). |
| `--solo TRACK` | Audition a physical track through its routing and master. Repeat to select more tracks. |
| `--tap TRACK_OR_BUS` | Capture the specified graph output boundary instead of the master. Repeat to capture several boundaries from one pass; `-o` then names a directory that receives `TAP.wav` per tap. |
| `--format float32` / `--format pcm24` | Choose WAV sample format; float32 is default, pcm24 uses TPDF dither. |
| `--sample-rate RATE` | Output sample rate; default 48000 Hz. |
| `--block-size FRAMES` | Processing block size; default 256, maximum 1024. |
| `--threads N` | Process independent tracks on up to N threads (1..256); default 1. |
| `--serial-plugins` | With `--threads`, keep every track that hosts a plugin on the rendering thread. |

Section renders and dry stems process preceding context from song start (or the
`--history` bound) and discard that audio, retaining instrument/effect history.
Score timestamps are
not rebased: tempo changes through the section end (including a change exactly
at its end) remain active, while later changes and material are excluded.
Parallel routes are latency-aligned and export latency is trimmed. Keep enough
tail for the intended releases.

Without `--seconds`, output ends at the score or selected section end plus the
tail, measured from the selected start. With `--seconds`, the output length is
exactly that many seconds: `--start 10 --seconds 5 --tail 2` still writes five
seconds, not seven. Include any desired release audio within that length. A
clock-time window alone does not cut musical events at an earlier excerpt
boundary to create a separate release segment. These scope rules also apply to
stems, queued renders, and render-recipe options.

### Bounded history

Full preceding context makes a short section cost as much as a render of the
song up to its end. For quick measurements, `--history SECONDS` starts
processing that many seconds before the output start (the section start or
`--start`) instead of at the song start:

```sh
muz render song.muz --section solos --history 8 -o solos.wav
muz stems song.muz --section solos --history 8 --track guitar -o stems/
```

The output has the same start and length as with full history, but the audio
is an approximation. The bound is the only change: omitting it keeps full
history, and a bound reaching back to the song start renders the identical
file. Processing starts on a block boundary at or before the requested time,
so the history can be up to one block longer than requested. Within the bound:

- Notes that start before the history start and are still sounding there are
  retriggered at the history start with their remaining duration: their attack,
  sample start and per-note expression restart there. Notes that ended earlier
  are not played.
- Controllers, pedals, automation, and tempo keep their full timeline; their
  values at the history start are restored as in a live seek.
- Instruments and effects start from a reset state. Reverb and delay tails,
  compressor and limiter gain, and other effect states from before the history
  start are missing. Instrument state carried across notes (round robins,
  free-running modulation, noise streams) also starts fresh.

A track whose voices start fresh on every note and whose effects have settled
by the section start renders exactly as with full history. Choose a bound
longer than the longest effect tail or held note that matters to the
measurement, and use full history for delivery. The bound composes with taps,
stems, `--threads`, queued renders, and render recipes (`history` in seconds).

### Parallel rendering

`--threads N` speeds up disk renders, stems, and queued bounces of arrangements
with many tracks:

```sh
muz render song.muz -o master.wav --threads 8
```

Each track's instrument and inserts render into that track's own buffer. Tracks
run in waves: a track with an external sidechain waits for the wave that
renders its source. Sends, outputs, and buses are then summed on one thread in
the same fixed track order as a serial render. **The output is bit-identical for
every thread count**, including automation, sidechains, and render meters.
Buses and the master run serially, so a mix dominated by one heavy track or bus
gains little. Live playback always uses one audio thread.

Tracks that host CLAP or VST3 plugins run on worker threads like any other
track, so several amp-sim or instrument-plugin tracks render side by side. The
rendering thread stays each plugin's main thread: it makes every main-thread
call and services the main-thread callbacks plugins request, between blocks, in
the same order as a serial render. A worker holds the audio-thread role only
while it processes a plugin, one thread per plugin instance at a time, and the
host's thread check answers accordingly. Two instances of one plugin may process
at the same time, as the CLAP and VST3 threading rules allow.

A plugin that breaks those rules (state shared between its instances without
locking, or audio processing that expects the thread that created it) can be
kept on the rendering thread:

```muz
plugin("/path/to/Amp.clap", {class: "amp.id", render_thread: true})
```

That pins the whole track holding the plugin; `--serial-plugins` pins every
plugin-hosting track for one render. Neither changes the output. If a plugin
waits inside its audio processing for its main thread, a wave with workers is
serviced after two seconds instead of hanging, at the cost of that wait and of
a result that can depend on timing; such a plugin cannot render serially at all.

### Track stems and bus taps

```sh
muz stems song.muz -o stems/
muz stems song.muz --wet -o solo-auditions/
muz stems song.muz --section solos --track guitar --track bass -o stems/
muz render song.muz --tap hall -o hall-return.wav
muz render song.muz --section solos --tap guitar --tap bass --tap hall -o taps/
```

Ordinary stems capture each physical track **after inserts and pan, before output
gain, sends, and master**. A shared return can be exported by bus name with `--tap`.
A track tap is the same boundary as that track's stem; `--tap master` is the mix.

Repeat `--tap` to measure several boundaries from one render pass. With one
`--tap`, `-o` is the output file. With two or more, `-o` is a directory and each
tap is written as `DIR/TAP.wav`, the naming `muz stems` uses; a `.wav` path is
rejected. Each file is identical to the single-tap render of that boundary. A tap
must name a physical track or a bus and may appear once. Queued (`--socket`) and
batch renders write one file per job, so they take a single tap.
Wet stems are solo auditions through effect returns and the nonlinear master;
they do not sum back to the mix. Solo keeps detector sources processing.
See [signal flow](production.md#signal-flow) for the boundaries.

`--track NAME` limits stems to the named physical tracks; repeat it for more.
Like `--solo`, a kit name selects its expanded `NAME.*` tracks. Unlike `--solo`,
it changes which files are written, not what is audible. Dry stems process only
what the selected tracks read: the tracks themselves and the external sidechain
sources of their inserts. Other tracks, bus and master effects, and their
automation are skipped. A stem is taken before output gain and sends, so nothing
skipped can reach it. Selected stems are identical to the same files from a full
stem export. With `--wet`, `--track` limits which solo auditions are rendered;
each audition still renders the whole graph.

### Plugin render behavior

Disk exports, including stems and queued bounces, initialize VST3 processors in
offline mode and use that mode for every audio block. Live playback uses realtime
mode. This lets plugins complete streaming or other required work when rendering
faster than wall clock; plugins may also choose different offline quality, so live
and exported audio need not null exactly.

### Analyze a rendered file

```sh
muz analyze master.wav
```

Analysis reports integrated LUFS, loudness range, true peak, sample peak, RMS,
DC, and stereo correlation. Use it when a delivery requirement or a listening
question needs measurements. Routine renders need no extra analysis after a
successful command exit; see the [verification budget](../AGENTS.md#verification-budget).

### MIDI and DAW handoff

Use [MIDI interchange](language.md#midi-interchange) for musical MIDI files, or
[DAWProject export](dawproject.md) for an editable DAW archive. Read the fidelity
report before relying on a DAWProject handoff.

## Named comparisons and delivery collections

A source module can export named lists of `{name, song, options}` records. `song`
is an ordinary evaluated song, so candidates can come from different arguments
to the same composer function. The following fragment assumes `version` and `chosen` are already defined:

```muz
let compare = [
    {name:"held", song:version(0), options:{section:"final-chorus",tail:2}},
    {name:"lifted", song:version(7), options:{section:"final-chorus",tail:2}}
];
let delivery = [
    {name:"master",song:chosen,options:{format:"pcm24",tail:3}},
    {name:"lead-tap",song:chosen,options:{tap:"lead",tail:3}},
    {name:"hall-return",song:chosen,options:{tap:"hall",tail:3}}
];
```

Run `muz batch revisions.muz compare -o out/compare --match-levels` or
`muz batch revisions.muz delivery -o out/delivery`. Names use letters, numbers,
hyphens and underscores. Options use the renderer fields:
`section`, `start`, `seconds`, `tail`, `solo`, `tap` (one track or bus name),
`format`, `sample_rate`, `block_size`, and `history`. `start`, `seconds`, `tail`
and `history` are seconds; section names resolve
against each candidate. Omitted options retain ordinary render defaults.

The module and all candidate songs are evaluated before starting the collection.
The two-worker queue accepts up to 34 captured outputs, which run in isolated
child processes. Ctrl-C cancels outstanding work; failed outputs preserve existing files.
`renders.json` records each result or failure and its exact render scope. Batch
renders are disk-source captures, so they have no live-server revision number.

`listen.html` plays the results and switches candidates at the same playback
position. With `--match-levels`, measured integrated loudness determines listening
attenuation to the quietest measurable candidate. The WAVs and their production
processing are unchanged. Very short or silent candidates without measurable
integrated loudness remain unmatched. Keep arrangement, preceding context, render
scope and controllable randomness consistent when comparing one musical choice.

For draggable loop selection, synchronized A/B/X switching, or repeated blind
preference trials with deferred statistical reports, use the
[muz-ab comparator skill](../.agents/skills/muz-ab/SKILL.md). Its local player
consumes the matched batch manifest directly and preserves the original bounces.

Name output boundaries explicitly in delivery recipes: a track tap, shared
return, and wet solo audition have different uses. See
[stems and taps](#track-stems-and-bus-taps) before combining exported layers.

See [revision-workflow.muz](../examples/revision-workflow.muz) for a complete source example.

## Next steps

Revise musical material with the [language guide](language.md), adjust the graph
with [production](production.md), or use the
[blind comparator](../.agents/skills/muz-ab/SKILL.md) for controlled listening trials.
