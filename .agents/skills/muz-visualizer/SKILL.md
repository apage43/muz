---
name: muz-visualizer
description: Create or update instrument visualizer videos for muz pieces, synchronized to their performed notes and finished audio masters.
---

# Muz visualizer

Use the piece's current committed arrangement and matching delivery master unless
another version is requested. Read its README and recent commit first: older
masters and performance exports remain in `out/` and can look like current inputs.
If provenance is unclear, render a fresh master from the selected source.

## Build and review

The bundled [renderer](scripts/render.py) exports performance directly from muz,
integrates the tempo map, and draws one luminous lane per physical track. Use it
for the established instrument-lane style; adapt the artwork when requested.
Defaults are 1920×1080 at 60 fps, instrument names as the only text, and the full
master with its natural tail. Brightness reflects performed note velocity;
filaments are procedural note animation, not measured stem waveforms.

Pitch traces include native patch legato/retrigger glides from the prepared
`glide_ms` control and per-note tuning expression. Overlapping mono notes transfer
visual ownership, and interrupted glides continue from their current pitch.
The renderer exports both performance and patch metadata; it does not infer bends
from note tags. These are constant-control glide paths and note tuning, not an
evaluation of arbitrary oscillator modulation or automated glide-time changes.

Run from the repository root. `uv run` provisions the script's Python dependencies;
`ffmpeg`, `ffprobe`, the built muz binary, and the piece's local assets are required.
Discover flags with `--help`. A piece can own a stable track-ID-to-name/color mapping
in its own `visualizer.json`; otherwise start with readable track IDs.
Set `"lead_in": 1.0` in that style for a one-second silent approach to the first
attack (default zero). Audio and animation shift together; source audio stays
untouched. Preview times are video seconds, including the lead-in.

```sh
uv run .agents/skills/muz-visualizer/scripts/render.py \
  --source ../muz-projects/<piece>/song.muz \
  --audio out/<piece>/master.wav \
  --output ~/Documents/<piece>-visualizer-1080p.mp4 \
  --work-dir out/<piece>/visualizer \
  --style ../muz-projects/<piece>/visualizer.json \
  --preview 0 30 60 90
```

Choose preview times from the selected revision: opening, dense crest, sparse
passage, and ending. Inspect the PNGs for readable names, pitch separation within
lanes, clipping, and glow that preserves detail. Remove `--preview` to encode the
full video. The layout supports up to 24 tracks; larger ensembles need a deliberate
layout change. `--font` selects a local font if Noto Sans Light is unavailable.

The renderer probes NVIDIA encoding and falls back to software H.264. It writes a
unique temporary MP4 and atomically replaces the requested destination after
encoding succeeds. Follow the repo's verification budget: deliver after a
successful encode. Extract encoded frames or run additional media checks only
for a specific suspected defect or a user request. Report the output path and
configured duration, resolution, and frame rate. `verification.json` records
encoding completion and input hashes; it does not certify a decoded output.

Keep generated exports, previews, logs, and videos in ignored `out/` or the user's
delivery folder. Keep reusable code in this skill and piece-specific styling in
the project. Document any newly chosen external visual assets in the project.

When changing drawing code, retain subpixel coordinates through OpenCV's
fixed-point drawing helpers and composite note ink over the background.
Integer snapping causes uneven scrolling; painting faint ink directly onto the
background creates dark trails. Focused synthetic cases live in
`scripts/test_motion.py`.
Pitch-continuation and tuning cases live in `scripts/test_pitch.py`.
