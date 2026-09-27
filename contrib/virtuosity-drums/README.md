# Virtuosity Drums v0.925

Native drum maps for Virtuosity Drums v0.925 by
[Versilian Studios](https://versilian-studios.com/virtuosity-drums/) with
Karoryfer, performed by Austin McMahon. The recordings are CC0 and play directly,
without an SFZ player or conversion. The Python 3 installer downloads the
publisher's roughly 1.2 GB ZIP (1,227,151,376 bytes) into ignored `assets/`.

[All packs](../../contrib/README.md) · [Instrument guide](../../docs/instruments.md)

Run shell commands from the `muz-core` checkout unless stated otherwise.
For a CLI installed outside this checkout, set `MUZ_CONTRIB_DIR` to this
checkout's absolute `contrib` path; see the [pack setup guide](../../contrib/README.md#install-and-import).

## Contents

- [Install](#install)
- [Use the kit](#use-the-kit)
- [Mappings and mix options](#mappings-and-mix-options)
- [Shared insert automation](#shared-insert-automation)
- [Asset layout](#asset-layout)
- [Upstream and license](#upstream-and-license)
- [Pins and manual installation](#pins-and-manual-installation)

## Install

```sh
python3 contrib/virtuosity-drums/install.py
```

The installer downloads the publisher's ZIP into `assets/`, checks its pinned
size and sha256, and extracts `Samples/`, `Programs/`, `LICENSE` and
`notes.txt` into `assets/`. It is idempotent: a complete install is verified
and left untouched, missing files are filled in, files that already match are
not rewritten, and anything that differs from the pinned release stops the run
instead of being overwritten.

```sh
python3 contrib/virtuosity-drums/install.py --check     # verify only, no download
python3 contrib/virtuosity-drums/install.py --source /path/to/Virtuosity_Drums_v0.925.zip
```

`--source` verifies an already-downloaded copy and extracts from it in place
without copying another 1.2 GB into `assets/`.

## Use the kit

After installing the recordings, save this as a song in your project:

```muz
use "contrib/virtuosity-drums/kit" as vd;
song({
    tracks: [track("drums", drums({kick: "X...", snare: "..X.", hat: "x.x."}), vd.acoustic)],
    buses: [bus("room", [fx("reverb", {mix: 1})]), bus("crushed", [])]
})
```

The acoustic kit sends to `room` and `crushed`; declare both buses even if you
start with an empty chain. Choose their processing to suit your song.

## Mappings and mix options

Zone paths are written relative to `kit.muz`, so
`assets/Samples/{mic}/{drum}/{mic}_{drum}_{art}_vl{n}[_rr{j}].flac` resolves
inside this pack. Velocity boundaries follow the publisher's SFZ maps; zone
order is significant because samplers pick round robins by index.

| export | what it is |
| --- | --- |
| `zones(mic, drum, art, layers, rr)` | zone list for one articulation, velocity split from the SFZ map |
| `hit(mic, drum, art, layers, key, rr, gain)` | one-shot sample over those zones |
| `acoustic` | close kit: kick mic, snare mic and mid mics with per-voice processing, chokes on `hat`/`open_hat` |
| `room` | shells on the room mic, kick and rimshot trimmed back |
| `room_light` | the same room recordings with shallower trims, un-gained tom bodies and no rim voice |
| `overheads` | cymbals only, from the overhead pair |
| `full_overheads` | the overhead pair carrying every voice, shells included |
| `closed_hat`, `closed_zones` | closed hat with the divergent fourth layer replaced by layer three plus a small accent |
| `center_snare`, `cymbal`, `tom`, `path` | helpers selecting four-layer snares, overhead cymbal round robins and four-layer toms |
| `snare_room`, `kick_attack`, `metal` | the snares on the room mic, the mid-mic kick beater attack, and a pitched-up crossstick voice |

Per-voice trims, pans, chokes and effect chains are part of each table's mix
policy; a caller that wants different processing merges over these tables
rather than editing them.

## Shared insert automation

Logical kit-track insert automation broadcasts to each played voice, including
voices with their own existing EQ or compression chains:

```muz
use "contrib/virtuosity-drums/kit" as vd;
song({
    tracks: [track("drums", drums({kick: "X...", snare: "..X.", hat: "x.x."}), vd.acoustic, {
        chain: [fx("lowpass", {id: "tone", cutoff_hz: 16000})]
    })],
    buses: [bus("room", [fx("reverb", {mix: 1})]), bus("crushed", [])],
    automation: [automation("drums.tone.cutoff_hz", curve([[0b, 800], [4b, 16000]]))]
})
```

Each voice retains its own filter instance, also affecting its sends; this is not a
shared bus filter. The room/overhead kits remain separate tracks and need their own
lanes when they should follow the same motion. The closed-hat top-layer substitution
remains intentional: the fourth layer has a splashier sound than the chosen alternative.

## Asset layout

```
contrib/virtuosity-drums/
  README.md
  install.py
  manifest.json
  kit.muz
  assets/                                  # git-ignored, created by install.py
    virtuosity.zip                         # verified archive, outside the extracted tree
    Samples/{mic}/{drum}/{mic}_{drum}_{art}_vl{n}[_rr{j}].flac
    Programs/                              # publisher's SFZ programs and keymaps
    LICENSE                                # CC0 1.0
    notes.txt                              # publisher's version notes
```

`assets/Samples/` holds one folder per microphone position. The module uses
`kickmic`, `snaremic`, `mid`, `oh` and `room`; `lofi` and `perc` are installed
too. Each drum folder holds `kick`, `snare`, `snareoff`, `hh`, `crash`, `ride`,
`flatride`, `htom` and `ltom`, with the velocity layers and round robins the
zones above expect.

## Upstream and license

The recordings use [CC0 1.0 Universal](https://github.com/sfzinstruments/virtuosity_drums/blob/master/LICENSE),
a public-domain dedication allowing redistribution. This repository still stores
only mappings and installers, with audio under ignored `assets/`.

The ZIP also contains `GUI/`, `Virtuosity Drums.bank.xml`,
`VirtuosityDrums_Keymap.pdf` and `VirtuosityDrumsManual.pdf` — the sample
player's own interface definitions and its documentation. The installer
deliberately leaves those in the archive; they are not needed to render this
mapping. The library's own `notes.txt` (version notes through Beta 0.925) and
its CC0 `LICENSE` are extracted next to the samples.

## Pins and manual installation

| archive member | files | bytes | installed at |
| --- | ---: | ---: | --- |
| `Samples/` | 4858 | 1468997687 | `assets/Samples/` |
| `Programs/` | 419 | 663828 | `assets/Programs/` |
| `LICENSE` | 1 | 7048 | `assets/LICENSE` |
| `notes.txt` | 1 | 5997 | `assets/notes.txt` |
| All archive members (uncompressed) | 5357 | 1488795308 | includes content left in the ZIP |
| Compressed ZIP | 1 | 1227151376 | `assets/virtuosity.zip` |

`manifest.json` carries the same pins: URL, archive bytes, sha256 and the
extraction layout. The verifier counts files and adds bytes per member, so a
truncated or partly deleted install is reported rather than silently reused.

### Manual installation

1. Download <https://versilian-studios.com/Distro/Virtuosity_Drums_v0.925.zip>.
2. Confirm size `1227151376` bytes and sha256
   `c6c5d0fe11a394e94be3146a950c3377ec102cb57d189d5a23cec26183d1963a`.
3. Place the archive at `contrib/virtuosity-drums/assets/virtuosity.zip`.
4. Extract `Samples/`, `Programs/`, `LICENSE` and `notes.txt` from its root
   into `contrib/virtuosity-drums/assets/`, then run the installer with
   `--check`.
