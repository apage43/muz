---
name: muz-visualizer
description: Create or update instrument visualizer videos for muz pieces, synchronized to their performed notes and finished audio masters.
---

# Muz visualizer

Create a video synchronized to the selected arrangement and its delivery master.
The bundled [renderer](scripts/render.py) draws one luminous lane per physical
track. Use it for the established instrument-lane style; adapt the artwork when
requested.

## Select inputs and prepare

Use the piece's current committed arrangement and matching master unless another
version is requested. Read its README and recent commit first: older masters and
performance exports may remain in `out/`. If provenance is unclear, render a
fresh master from the selected source.

Resolve project paths from the task. The maintainer's sibling `muz-projects`
checkout is a personal convention; the renderer accepts other project locations.
Run from the muz-core root. `uv run` provisions the Python dependencies;
`ffmpeg`, `ffprobe`, the built muz binary, and the piece's assets are required.
Use `--help` for flags and `--muz PATH` for a binary outside `target/release/muz`.

## Configure the visualizer

Defaults are 1920×1080 at 60 fps, instrument names as the only text, and the full
master with its natural tail. Brightness reflects performed note velocity;
filaments are procedural animation, not measured stem waveforms. The layout
supports up to 24 tracks; larger ensembles need a deliberate layout change.

A project can supply a `visualizer.json` mapping stable track IDs to names and
colors. Otherwise, start with readable track IDs. Set `"lead_in": 1.0` for a
one-second silent approach to the first attack; the default is zero. Audio and
animation shift together without changing the source audio. Preview times are
video seconds, including the lead-in. `--font` selects a local font if Noto Sans
Light is unavailable.

The renderer exports compact graph, performance, and patch metadata directly
from muz, following each view's pagination, and integrates the tempo map.
Graph rows supply source PPQ and instrument controls; performed events and
patch details come from their own bounded views rather than a full-session dump.
Pitch traces include native legato/retrigger glides
from the prepared `glide_ms` control and per-note tuning expression. Overlapping
mono notes transfer visual ownership; interrupted glides continue from their
current pitch. These are constant-control glide paths and note tuning, without
inference from tags or evaluation of arbitrary oscillator modulation and automated
glide-time changes.

## Preview and render

Replace the example input/output paths with those selected for the task:

```sh
uv run .agents/skills/muz-visualizer/scripts/render.py \
  --source /path/to/project/song.muz \
  --audio /path/to/project/master.wav \
  --output /path/to/delivery/visualizer-1080p.mp4 \
  --work-dir out/visualizer \
  --style /path/to/project/visualizer.json \
  --preview 0 30 60 90
```

Choose preview times from the actual revision: opening, dense crest, sparse
passage, and ending. Inspect the PNGs for readable names, pitch separation,
clipping, and glow that preserves detail. Omit `--style` if no style file is
needed. Remove `--preview` to encode the full video.

The renderer probes NVIDIA encoding and falls back to software H.264. It writes
a unique temporary MP4 and atomically replaces the requested destination after
encoding succeeds.

## Deliver

Follow the [verification budget](../../../AGENTS.md#verification-budget): deliver
after a successful encode. Extract encoded frames or run more media checks only
for a specific suspected defect or a user request. Report the output path and
configured duration, resolution, and frame rate. `verification.json` records
encoding completion and input hashes; it does not certify a decoded output.

Keep generated exports, previews, logs, and videos in ignored `out/` or the user's
delivery folder. Keep reusable code in this skill and piece-specific styling in
the project. Document newly chosen external visual assets in the project.

## Maintain the drawing code

Retain subpixel coordinates through OpenCV's fixed-point drawing helpers and
composite note ink over the background. Integer snapping causes uneven scrolling;
painting faint ink directly onto the background creates dark trails.
Focused synthetic motion cases live in [test_motion.py](scripts/test_motion.py);
pitch-continuation and tuning cases live in [test_pitch.py](scripts/test_pitch.py).
