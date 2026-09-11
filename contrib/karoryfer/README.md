# Karoryfer instrument pack

Native muz maps for two CC0 [Karoryfer](https://github.com/sfzinstruments) libraries,
plus a pinned installer for their recordings.

The recordings are third-party content and are never committed. `install.py` places
them in the git-ignored `assets/` directory next to this README; the `.muz` files
reference them as `assets/...`, which resolves inside this directory.

## Contents

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

## Install

From the repository root:

```sh
python3 contrib/karoryfer/install.py
```

The installer verifies every pinned size and SHA-256, downloads only what is missing
or wrong, writes through a staging file and renames it into place, and prints what it
verified or installed. Re-running it is idempotent. Options:

- `--from DIR` — take the pinned files from an existing assets directory (one that
  contains `guitar/` and `bass/`) instead of downloading them; files missing there
  are downloaded.
- `--force` — replace files that exist but differ from the pin. Without it the
  installer refuses to overwrite a differing file and exits non-zero, naming the
  expected and found hashes.
- `--jobs N` — parallel transfers (default 6).

## Layout produced

```
contrib/karoryfer/assets/
  guitar/LICENSE                                   CC0 1.0 legal code
  guitar/readme.txt                                upstream library notes
  guitar/Samples/acoustic/*.wav                    102 files, 2 round robins
  guitar/Samples/electric/*.wav                    238 files, 4 takes + 2 releases
  bass/license                                     CC0 1.0 legal code
  bass/Samples/darkblack/reg/*.wav                 40 files
```

## Upstreams

| Library | Upstream | Revision | License | Files | Size |
| --- | --- | --- | --- | --- | --- |
| Shinyguitar (acoustic + electric guitar) | [sfzinstruments/karoryfer.shinyguitar](https://github.com/sfzinstruments/karoryfer.shinyguitar) | `57243cca85277dbcc120ce17c6178032f93c80f3` | CC0 1.0 | 342 | 251.7 MB |
| Black And Blue Basses (darkblack) | [sfzinstruments/karoryfer.black-and-blue-basses](https://github.com/sfzinstruments/karoryfer.black-and-blue-basses) | `6e7d674cdb41be7a54dbccb15472401ad01099b9` | CC0 1.0 | 41 | 26.5 MB |
| **Total** | | | | **383** | **278.2 MB** |

## Mapping notes

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

## Using the pack

```muz
// Import paths are relative to the file that writes them.
use "contrib/karoryfer/guitar.muz" as guitar;
use "contrib/karoryfer/basses.muz" as basses;

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

## Manual install

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

## Provenance

Maps are written against the two upstream libraries and their pinned revisions in
the table above; no other source is used. The mapping source is this directory's
own `guitar.muz` and `basses.muz`.
