# Karoryfer instrument pack

Native maps for the CC0 Karoryfer Shinyguitar and Black And Blue Basses libraries.
Use them for acoustic/electric guitar and regular bass plucks. Python 3 is needed
for the installer; recordings are downloaded separately into ignored `assets/`.

[All packs](../../contrib/README.md) · [Instrument guide](../../docs/instruments.md)

Run shell commands from the `muz-core` checkout unless stated otherwise.
For the installed CLI, run these utilities from the installed contrib copy
created by `./install.sh`; see the [pack setup guide](../../contrib/README.md#install-and-import).

## Contents

- [Install](#install)
- [Use the instruments](#use-the-instruments)
- [Mapping and options](#mapping-and-options)
- [Performance helpers](#performance-helpers)
- [Asset layout](#asset-layout)
- [Upstream and license](#upstream-and-license)
- [Maintenance and manual installation](#maintenance-and-manual-installation)
- [Known limits](#known-limits)

## Install

From the repository root:

```sh
python3 contrib/karoryfer/install.py
```

The installer verifies pinned sizes and SHA-256 hashes, downloads missing files,
and installs them through staging files. It reports differing files without
replacing them unless `--force` is set. Re-running is idempotent. Options:

- `--from DIR` — take the pinned files from an existing assets directory (one that
  contains `guitar/` and `bass/`) instead of downloading them; files missing there
  are downloaded.
- `--force` — replace files that exist but differ from the pin. Without it the
  installer refuses to overwrite a differing file and exits non-zero, naming the
  expected and found hashes.
- `--jobs N` — parallel transfers (default 6).

## Use the instruments

The following fragment declares three instruments; pass one to `track(...)`.

```muz
use "contrib/karoryfer/guitar" as guitar;
use "contrib/karoryfer/basses" as basses;

let acoustic = guitar.acoustic({attack_ms: 0.5, release_ms: 260, velocity_track: 0.38});
let electric_left = guitar.electric_left({attack_ms: 2.5, release_ms: 150, velocity_track: 0.32});
let bass = basses.bass({attack_ms: 1, release_ms: 65, velocity_track: 0.8});
```

Each map has a thin builder (`acoustic`, `electric`, `electric_left`, `electric_right`,
`electric_release`, `bass`) that forwards to `sample`. A function body evaluates in its
defining module, so the builder stamps this pack directory as the asset root; calling
`sample(guitar.acoustic_zones, {...})` from a piece would instead resolve `assets/...`
against that piece's directory. The builders pass their options through unchanged and
impose no settings of their own, so each piece keeps its own envelope, velocity
tracking and gain policy.

The electric map is the dry pickup signal; there is no amplifier in this pack.

## Mapping and options

Zone boundaries, roots, per-zone offsets and zone order are preserved from the
libraries' own sampled material; order is audible, because round robins are picked
by index.

- The acoustic map is the microphone signal: 17 sampled roots, three velocity layers,
  two round robins each. The C4 root is set explicitly to 60.
- The electric map is the magnetic pickup (dry) signal: four takes per root.
  `electric_left_zones` uses takes 1 and 3, `electric_right_zones` uses takes 2 and 4,
  and `electric_release_zones` holds the two quiet note-ending recordings per root.
  Odd-numbered takes feed one side, even-numbered takes the other.
- The bass map is the darkblack regular plucks: ten sampled roots, two dynamic layers
  and two round robins. The lowest zone (root 35, B1) spans keys 24–36, extending the
  sampled B1 down through the written low register. Small per-zone offsets remove most
  recorded preroll while preserving the pluck attack.

## Performance helpers

```muz
use "contrib/karoryfer/guitar" as guitar;
use "contrib/karoryfer/performance" as performance;
let parts = performance.electric_with_releases("guitar", phrase("C3:q E3:q"),
    release_options = {gain_db: -12}, timing = {tempo: 120});
let bright = performance.expressive(guitar.acoustic_zones, [48, 52], 5000Hz);
// Put parts in song.tracks; bright is an instrument for track(...).
```

`electric_with_releases` returns main and `-release` tracks. It schedules the existing
release recordings at performed note ends, including gate and timing offsets;
pass the song's timing record when it differs from the default. Attack velocity
also drives the release. `options` and `release_options` are sampler options;
release gain is a musical choice, not a new calibration. This helper does not infer
pedal-up, choke, or mono-voice ownership changes from a score.

`expressive(zones, keys, tone, options, sample_budget_frames)` adds independent per-note filtering with
pressure raising cutoff by one octave. Pass this pack's guitar or bass zone tables;
select a register or raise `sample_budget_frames` from its 8,388,608-frame default
when the selected map needs more storage (each frame is eight bytes). It uses the
[sampled-voice envelope policy](../../docs/synthesis.md#amplitude-and-envelopes)
and is optional.

## Asset layout

```
contrib/karoryfer/assets/
  guitar/LICENSE                                   CC0 1.0 legal code
  guitar/readme.txt                                upstream library notes
  guitar/Samples/acoustic/*.wav                    102 files, 2 round robins
  guitar/Samples/electric/*.wav                    238 files, 4 takes + 2 releases
  bass/license                                     CC0 1.0 legal code
  bass/Samples/darkblack/reg/*.wav                 40 files
```

## Upstream and license

| Library | Upstream | Revision | License | Files | Size |
| --- | --- | --- | --- | --- | --- |
| Shinyguitar (acoustic + electric guitar) | [sfzinstruments/karoryfer.shinyguitar](https://github.com/sfzinstruments/karoryfer.shinyguitar) | `57243cca85277dbcc120ce17c6178032f93c80f3` | CC0 1.0 | 342 | 251.7 MB |
| Black And Blue Basses (darkblack) | [sfzinstruments/karoryfer.black-and-blue-basses](https://github.com/sfzinstruments/karoryfer.black-and-blue-basses) | `6e7d674cdb41be7a54dbccb15472401ad01099b9` | CC0 1.0 | 41 | 26.5 MB |
| **Total** | | | | **383** | **278.2 MB** |

Maps are written against the two upstream libraries and their pinned revisions in
the table above; no other source is used. The mapping source is this directory's
own `guitar.muz` and `basses.muz`.

## Maintenance and manual installation

Each record in `manifest.json` carries the pinned `url`, `path`, `bytes` and `sha256`.
Download every `url` — all of them are `raw.githubusercontent.com` files at the
revisions in the table above — and place the bytes at
`contrib/karoryfer/assets/<path>`, then check the sizes and hashes:

```sh
cd contrib/karoryfer
python3 - <<'PY'
import hashlib, json, pathlib
for item in json.loads(pathlib.Path('manifest.json').read_text())['files']:
    data = (pathlib.Path('assets') / item['path']).read_bytes()
    assert len(data) == item['bytes'], item['path']
    assert hashlib.sha256(data).hexdigest() == item['sha256'], item['path']
print('all pinned files match')
PY
```

`python3 contrib/karoryfer/install.py` passes once every pinned file is in place; it
installs the upstream `LICENSE`/`license` and `readme.txt` files as well.

### Pack files

- `guitar.muz` — **Karoryfer Shinyguitar** (archtop guitar by D. Smolken): the
  acoustic microphone map and the electric magnetic-pickup map, including the
  generator tables that split the four electric takes across two sides and the
  release samples.
- `basses.muz` — **Karoryfer Black And Blue Basses**: the darkblack regular-pluck map
  with its per-zone source offsets.
- `install.py` — fetches, verifies and places the pinned recordings.
- `manifest.json` — per-file pin (`path`, `url`, `bytes`, `sha256`) and the upstream
  repository revisions.
- `assets/` — created by `install.py`; ignored by git.

## Known limits

Research on 2026-09-12: the pinned upstream
[Shinyguitar control description](https://github.com/sfzinstruments/karoryfer.shinyguitar/blob/57243cca85277dbcc120ce17c6178032f93c80f3/readme.txt)
supports tone, microphone/pickup blend and release-noise controls. Those are credible
future packaging directions. It does not establish that our mapped takes contain
recorded legato transitions. Continuing a sample with glide therefore remains an
explicit synthesized effect, not a default guitar/bass realism fix. A complete blend
instrument still needs balance and phase auditioning across the two recording sets.
