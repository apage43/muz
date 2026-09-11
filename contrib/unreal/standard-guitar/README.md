# Unreal Instruments Standard Guitar

Native `sustain` and `mute` map for the Unreal Instruments Standard Guitar
library, plus an installer that restores the publisher's recordings into this
directory. No recordings ship with the repository: the license permits
commercial music but forbids redistribution.

## Upstream and license

| | |
| --- | --- |
| Library | Unreal Instruments Standard Guitar |
| Publisher page | <https://unreal-instruments.wixsite.com/unreal-instruments/standard-guitar> |
| License terms | <https://unreal-instruments.wixsite.com/unreal-instruments/about> |
| Download page | <https://drive.google.com/uc?export=download&id=1uoV7icZV1_IjiOGKM7Wm5_K5UkF41Fm3> |
| Archive | `standard-guitar.rar`, 750,502,921 bytes (716 MiB) |
| Archive SHA-256 | `0777b8cbbfe2fe79e843fa6c2c24ab300c2d5a538d46f4d970f057979f87fee0` |
| Extracted library | 761,487,033 bytes (726 MiB) under `assets/standard-guitar/` |
| Mapped recordings | 400 FLAC files, 115,800,959 bytes (110 MiB) |

License summary: commercial music made with the library is permitted and no
credit is required; unauthorized alteration and redistribution of the library's
data are prohibited. Because redistribution is prohibited, the recordings stay
out of git — `install.py` fetches them from the publisher and `**/assets` is
ignored by the repository. `manifest.json` carries the pins.

## Install

Requires Python 3.11 or newer (standard library only) and the `unrar` command on
PATH (Arch: `unrar`, Debian/Ubuntu: `unrar-free`, macOS: `brew install unrar`).

```sh
python3 contrib/unreal/standard-guitar/install.py
```

The installer:

1. verifies all 400 mapped recordings and exits immediately when they are
   already intact — re-running is cheap and idempotent;
2. downloads the publisher archive to `assets/.downloads/standard-guitar.rar`,
   keeps it as a cache, and checks it against the pinned SHA-256;
3. rejects archive members with absolute or `..` paths before extraction;
4. extracts into `assets/standard-guitar/` with `unrar x -idq -o- -p-`;
5. verifies every mapped recording by size and SHA-256 and reports the total.

A file that differs from its pin aborts the run with the offending path: the
supplied recordings are never overwritten, and neither is a cached archive that
fails the pin. Nothing in the library is renamed, re-encoded or altered.

### Manual steps

If the automatic download is blocked, fetch the archive from the publisher page
and place it at `assets/.downloads/standard-guitar.rar`, then re-run the
installer. If `unrar` cannot be installed, extract the archive yourself with any
RAR tool into `assets/standard-guitar/` (the archive's `standard-guitar/`
directory must end up there) and re-run the installer to verify the result.
When the extracted library already exists elsewhere on this machine, moving its
`standard-guitar/` directory to `assets/standard-guitar/` avoids the 716 MiB
download.

## Layout produced

```
contrib/unreal/standard-guitar/
  standard-guitar.muz   this pack's map
  install.py            restore and verify the recordings
  manifest.json         archive and recording pins
  README.md             this file
  assets/               gitignored, produced by install.py
    .downloads/standard-guitar.rar
    standard-guitar/UI_Standard_Guitar/Samples/Sus_Down/*.flac
    standard-guitar/UI_Standard_Guitar/Samples/Mute_Down/*.flac
```

The archive holds the whole publisher library; this pack maps only the two
`*_Down` articulations.

## What the map owns

- 25 chromatic roots, E2–E4 (MIDI 40–64), eight recorded takes per root and
  articulation: 200 `sustain` zones and 200 `mute` zones.
- Outer key bounds 35–39 and 65 keep nearest-root coverage from the preceding
  map; the played roots and takes are unchanged.
- Zones are emitted root by root, take 1–8 in recording order. The sampler
  chooses round robins by index, so this order is musically significant and must
  not be sorted or reordered.
- Sample paths use the template
  `assets/standard-guitar/UI_Standard_Guitar/Samples/<articulation>/<root>_<articulation><take>.flac`,
  resolved relative to `standard-guitar.muz`.
- `instrument` is the ready-made `kit()` with the pack's level settings.
- `voice(art, options)` builds a voice from the same zones; use it instead of
  calling `sample()` on these zones yourself, because `sample()` stamps the
  directory of the module that evaluates the call as the asset root.

Known issue, left in place for the owner to decide: neither voice sets
`one_shot:false`, so `kit()` supplies the sample default `one_shot:true`. The
sampler then ignores note-off, and the written durations, gates and the
configured 95/55 ms releases never stop the recordings: the release tails ring
through written rests and percussion breaks. `voice("Sus_Down", {one_shot: false})`
(or the option on the kit voices) is the correction; the map does not apply it
silently.

## Verification

`manifest.json` pins the archive URL, byte count and SHA-256, and the path, byte
count and SHA-256 of each of the 400 mapped recordings, all copied from the
publisher archive as extracted. The installer re-verifies those pins on every
run, so a damaged or replaced file is reported rather than used.
