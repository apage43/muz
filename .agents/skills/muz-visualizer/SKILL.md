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

Run from the repository root. `uv run` provisions the script's Python dependencies;
`ffmpeg`, `ffprobe`, the built muz binary, and the piece's local assets are required.
Discover flags with `--help`. A piece can own a stable track-ID-to-name/color mapping
in its own `visualizer.json`; otherwise start with readable track IDs.

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
unique temporary MP4, verifies a complete decode and media properties, then
atomically replaces the requested destination. Use a new versioned filename when
updating a piece so earlier deliveries remain available. Inspect a frame extracted
from the encoded video before delivery, and report its path, duration, resolution,
and frame rate. `verification.json` records the media properties and input hashes.

Keep generated exports, previews, logs, and videos in ignored `out/` or the user's
delivery folder. Keep reusable code in this skill and piece-specific styling in
the project. Document any newly chosen external visual assets in the project.
