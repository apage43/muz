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
| `muz play` / `muz stop` | Start or stop playback. |
| `muz restart` | Restart playback from the beginning. |
| `muz seek --section opening` | Seek to a named section; `--beat` and `--tick` also work. |
| `muz audition opening` | Loop a named section and start playback. |
| `muz loop --off` | Disable looping. |
| `muz panic` | Stop transport and reset audio, silencing active voices and effects. |
| `muz shutdown` | Stop the server and cancel its jobs. |

Without `--stopped`, the server starts playing immediately. `--headless` provides
a silent transport clock, session controls, and rendering without PipeWire output.
Source and selected assets are watched. A failed edit leaves the last accepted
session running; fix the reported error and save again. Compatible changes can
retain sounding voices and processors; structural changes require prepared
replacements. See [embedding](embedding.md#live-revisions) for the host contract.

Live seeks and loops restore overlapping notes and prior controllers. They cannot
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
muz render song.muz --start 10 --seconds 5 --tail 2 -o excerpt.wav
```

Disk rendering reads the source on disk. To capture the live server's applied
revision, use `render --socket PATH` without a source argument; that queues a job.

| Option | Meaning |
| --- | --- |
| `--section NAME` | Render the named section with preceding context from song start. |
| `--start SECONDS`, `--seconds LENGTH` | Select an explicit clock-time range. |
| `--tail SECONDS` | Set additional release/effect-tail time. |
| `--solo TRACK` | Audition a physical track through its routing and master. Repeat to select more tracks. |
| `--tap TRACK_OR_BUS` | Capture the specified graph output boundary. |
| `--format float32` / `--format pcm24` | Choose WAV sample format; float32 is default, pcm24 uses TPDF dither. |
| `--sample-rate RATE` | Output sample rate; default 48000 Hz. |
| `--block-size FRAMES` | Processing block size; default 256, maximum 1024. |

Section renders process preceding context and discard that audio, retaining
instrument/effect history. Parallel routes are latency-aligned and export latency
is trimmed. Keep enough tail for the intended releases.

### Track stems and bus taps

```sh
muz stems song.muz -o stems/
muz stems song.muz --wet -o solo-auditions/
muz render song.muz --tap hall -o hall-return.wav
```

Ordinary stems capture each physical track **after inserts and pan, before output
gain, sends, and master**. A shared return can be exported by bus name with `--tap`.
Wet stems are solo auditions through effect returns and the nonlinear master;
they do not sum back to the mix. Solo keeps detector sources processing.
See [signal flow](production.md#signal-flow) for the boundaries.

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
`section`, `start`, `seconds`, `tail`, `solo`, `tap`, `format`, `sample_rate`,
and `block_size`. `start`, `seconds` and `tail` are seconds; section names resolve
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
