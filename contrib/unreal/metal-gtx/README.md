# Unreal Instruments METAL-GTX

Guitar maps for picked sustains, palm mutes, and recorded releases from the
METAL-GTX sforzando library. The pack supplies articulation tables, a mono-channel
derivation, and an installer. The publisher's recordings are downloaded separately
and must not be redistributed.

[All packs](../../../contrib/README.md) · [Instrument guide](../../../docs/instruments.md)

Run shell commands from the `muz-core` checkout unless stated otherwise.
For the installed CLI, run these utilities from the installed contrib copy
created by `./install.sh`; see the [pack setup guide](../../../contrib/README.md#install-and-import).

## Contents

- [Install](#install)
- [Use the instruments](#use-the-instruments)
- [Mapping and options](#mapping-and-options)
- [Release helpers](#release-helpers)
- [Asset layout](#asset-layout)
- [Upstream and license](#upstream-and-license)
- [Maintenance](#maintenance)
- [Known limits](#known-limits)

## Install

Requires `python3` (standard library only), plus `unrar` and `ffmpeg` on `PATH`.

```sh
python3 contrib/unreal/metal-gtx/install.py
```

The installer prints what it verifies or installs, and re-running it is safe:

1. downloads the archive to `assets/metal-gtx-download.rar` when it is missing — an
   unfinished `.partial` download and an existing archive are never overwritten — and
   verifies the pinned size and sha256
2. rejects any archive member whose path would land outside `assets/metal-gtx/`
3. unpacks with `unrar x -o-`, so existing files are kept
4. verifies the 360 bank recordings the module plays against their pinned bytes and sha256
5. runs `derive.py`, which extracts the 720 mono takes and checks `metal-gtx.muz` against them

A take that already exists but differs from its pin stops the run naming that file;
nothing is overwritten, and deleting the file derives it again.

## Use the instruments

Pieces import the pack through the engine's contrib library root — `contrib/` in the
checkout that holds the running `muz`, the installed user data tree, or whatever
`$MUZ_CONTRIB_DIR` explicitly names:

```muz
use "contrib/unreal/metal-gtx/metal-gtx" as gtx;
let riff = stack([note("F#1", 1/4b), note("A1", 1/4b).at(1/4b), note("E2", 1/4b).at(3/4b)]);
song({
    tempo: 140,
    tracks: [
        track("open-left", riff, gtx.sustain_down_0),
        track("palm-right", riff, gtx.instrument(gtx.palm_down_1_zones, {gain_db: -6}))
    ]
})
```

Zone paths resolve against the pack's own `assets/` directory, so a piece anywhere can
play these samples as long as `contrib/unreal/metal-gtx/assets/` is installed.

## Mapping and options

`metal-gtx.muz` owns the articulation table. Zone order is the round-robin order within
each articulation, so takes stay in recording order, and every articulation has one
sample per recorded side:

| Articulation | Bank folder | Roots | Takes | Sides | Settings |
| --- | --- | --- | --- | --- | --- |
| `sustain_down` | `Samples/Sus_Down` | 30–57 (28) | 3 | 0, 1 | attack 0.4 ms, release 38 ms, velocity 0.45, −10 dB |
| `sustain_up` | `Samples/Sus_Up` | 30–57 (28) | 3 | 0, 1 | attack 0.4 ms, release 38 ms, velocity 0.45, −10 dB |
| `palm_down` | `Samples/Mute_Down` | 30–57 (28) | 3 | 0, 1 | attack 0.4 ms, release 42 ms, velocity 0.65, −7.5 dB |
| `palm_up` | `Samples/Mute_Up` | 30–57 (28) | 3 | 0, 1 | attack 0.4 ms, release 42 ms, velocity 0.65, −7.5 dB |
| `release` | `Samples/Release1` | 32–55 (6) | 4 | 0, 1 | attack 0.3 ms, release 55 ms, velocity 1, −28 dB, one shot |

720 zones in total: 84 takes per sustain and palm-mute side, 24 per release side. The
bank's remaining articulations (harmonics, slides, fretted mutes, brushes, chromatic
runs, keyswitches) are not mapped here.

The ten zone tables are exported as `sustain_down_0_zones` … `release_1_zones`, and the
preconfigured samples as `sustain_down_0` … `release_1`. `instrument(zones, options)`
builds those tables inside this module, which keeps every zone path resolving against the
pack's own `assets/` directory: a `sample()` call resolves relative to the module that
declares the path, so a caller in another directory building these zones itself would
look for them beside its own file.

## Release helpers

```muz
use "contrib/unreal/metal-gtx/metal-gtx" as gtx;
use "contrib/unreal/metal-gtx/performance" as performance;
let parts = performance.with_releases("guitar", phrase("F#1:q A1:q"),
    voice = gtx.sustain_down_1, side = 1, timing = {tempo: 120});
// Use parts as song.tracks. Side must match the chosen attack recording side.
```

The helper adds a `-release` track at performed note ends and retains the existing
release articulation's gain/envelope settings. `release_options` can override those
sampler settings. Pass the song timing record; this is source note-end scheduling,
not emulation of pedal, choke, or all upstream articulation rules. The helper lives
outside the generated mapping so `derive.py` does not overwrite it.

## Asset layout

| Path | Contents |
| --- | --- |
| `assets/metal-gtx-download.rar` | the publisher's archive, 1.34 GB, verified against the pin |
| `assets/metal-gtx/UI_METAL-GTX/` | the unpacked bank: `Samples/` (2739 FLAC), `Programs/`, `GUI/`, `Sample_MIDI_Files/` and the publisher's notice |
| `assets/gtx-mono/` | the 720 derived mono takes, 248,249,697 bytes |

Every source recording is a stereo double track, so it is split losslessly into its two
recorded sides with `ffmpeg -af pan=mono|c0=c0` / `pan=mono|c0=c1` and written as
`assets/gtx-mono/{articulation}-{root}-{take}-{side}.flac`. No pitch, timing, filtering or
level change; nothing is resampled or re-encoded beyond the channel split.

## Upstream and license

| | |
| --- | --- |
| Library | METAL-GTX (Multi Articulation Electric Guitar), sforzando soundfont |
| Publisher and download page | <https://unreal-instruments.wixsite.com/unreal-instruments/metal-gtx> |
| Revision | Published 2018-06-30, last updated 2019-07-07, verified against sforzando v1.916 |
| Recorded content | 44.1 kHz/24-bit FLAC, DI, humbucker guitar; F#1–E5 stereo double-tracked and F5–E6 mono, up to 18 round robins, 2600+ samples, keyswitched articulations |
| Archive | <https://drive.usercontent.google.com/download?id=1FurY3_x_tog_56irX1VDNyRCUt5JD7bO&export=download&confirm=t> |
| Archive size | 1,343,285,011 bytes (1.34 GB) |
| Archive sha256 | `c5756f95fcc30ac680f6034bb2e54c636e67fd0b53b1d40babada7257e782e73` |
| License | Custom, free to use; the publisher's notice ships inside the bank |
| Not in git | the archive, the unpacked bank and the derived takes (the repository ignores `**/assets`) |

### License terms

The notice inside the bank, `UI_METAL-GTX/見てね♡/説明および注意事項.txt`, states:

> ＊ライセンスについて
> ・ライセンスフリーです
> ・クレジット表記は不要です

The library is licence-free and needs no credit. The same notice disclaims liability for
any damage caused by using it (「ライブラリを使用したことによって生じたすべての障害・損害・不具合等に関して、
当方は一切の責任を負いません。」), and neither the notice nor the download page grants
redistribution. This pack therefore ships no recordings: `install.py` fetches the archive
from the publisher's own link, and everything it downloads stays in the gitignored
`assets/` directory. Do not redistribute the recordings.

## Maintenance

Manual route, if the bank is already unpacked at `assets/metal-gtx/UI_METAL-GTX/`:

    python3 contrib/unreal/metal-gtx/derive.py            # derive missing takes, verify pins and module
    python3 contrib/unreal/metal-gtx/derive.py --write-muz # accept a regenerated articulation table

`derive.py` needs `ffmpeg` only for takes that are missing, regenerates `manifest.json`
from what is on disk, and never changes `metal-gtx.muz` on its own.

### Pack files and pins

| File | Role |
| --- | --- |
| `metal-gtx.muz` | the articulation table, zone-order and settings included; generated by `derive.py` |
| `install.py` | stdlib installer: download, verify, member-path validation, unpacking |
| `derive.py` | stdlib derivation: mono extraction, pin regeneration, module check |
| `manifest.json` | pins: archive url/bytes/sha256, the 360 bank recordings, the 720 derived takes |

`manifest.json` records `sources` — the bank recordings the module plays — and `files` —
the derived takes — each with its path, byte size and sha256, together with the
articulation, root, take and channel of every derived file. For example:

```json
{
  "articulation": "sustain_down",
  "source": "assets/metal-gtx/UI_METAL-GTX/Samples/Sus_Down/a#1_Sus_Down1.flac",
  "path": "assets/gtx-mono/sustain_down-34-1-0.flac",
  "bytes": 423418,
  "sha256": "63f2e709635e3b44b054c427fb6a574f9aa811164217a0d47e8c57521a1c5f6a",
  "root": 34,
  "take": 1,
  "source_channel": 0
}
```

The archive pin is the strongest check: `install.py` verifies the whole archive's size and
sha256 before unpacking anything, and rejects member paths that would escape
`assets/metal-gtx/`.

## Known limits

Research on 2026-09-12 against the installed, pinned bank found release-triggered
one-shot regions in `Programs/Individual Patchs/METAL-GTX_XTracking/Release1_Sus_Down.sfz`.
`Release5_Pseudo_Legato.sfz` has a separate Hammer/Pull selection and release behavior.
Mapping that articulation requires its recordings and selection policy; native glide
alone would not reproduce it. Keep that work separate from these simple release helpers.

Direct channel readers can prepare the complete original sustain-down map with
`sample_budget_frames: 33554432` (256 MiB allowance). The 84 originals total
25,930,800 decoded frames and stereo channel readers share their storage. Replacing the generated mono map and installer would be a separate migration.
It must preserve side/take ordering and existing sound settings.

## Original SFZ programs

See [complete SFZ assets and catalogs](../../SFZ.md) for the `sfz-complete`
installation profile. The existing native presets remain available.
