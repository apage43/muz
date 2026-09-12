# VSCO 2 Community Edition

Sample catalogs for the VSCO 2 Community Edition orchestral library, plus an
installer that fetches the recordings from the pinned upstream revision.

## Upstream

| | |
|---|---|
| Library | VSCO 2 Community Edition (VSCO 2 CE) |
| Publisher | Versilian Studios — recorded by Sam Gossner and Simon Dalzell, sample cutting by Elan Hickler / Soundemote |
| Repository | https://github.com/sgossner/VSCO-2-CE |
| Revision | `440300901dfe9275fd84e0b7763af1f8443ae62e` |
| License | CC0 1.0 Universal (public domain dedication) — https://raw.githubusercontent.com/sgossner/VSCO-2-CE/440300901dfe9275fd84e0b7763af1f8443ae62e/LICENSE |
| Size | 268 files, 462482142 bytes (about 462 MB) |

`manifest.json` pins every file: upstream repository path, destination path,
byte size and SHA-256. No recording is committed to this repository; the
`assets/` directory is gitignored.

## Install

```sh
python3 contrib/vsco-2-ce/install.py            # fetch, verify and place
python3 contrib/vsco-2-ce/install.py --check    # verify only, download nothing
python3 contrib/vsco-2-ce/install.py --force    # replace files that fail their pin
python3 contrib/vsco-2-ce/install.py --jobs 8   # parallel downloads (default 4)
```

Files are downloaded from `raw.githubusercontent.com` at the pinned revision,
checked against their pinned size and SHA-256, and only then moved into place
atomically. Re-running verifies what is already there and installs what is
missing; an existing file that does not match its pin is reported instead of
being overwritten. `--check` exits non-zero when anything is missing or
differing.

### Manual install

Any downloader works as long as the files land at the pinned paths and verify:

```sh
cd contrib/vsco-2-ce
python3 -c "import json;[print(f['path'], f['url'], sep='\t') for f in json.load(open('manifest.json'))['files']]" |
while IFS="$(printf '\t')" read -r path url; do
    mkdir -p "assets/$(dirname "$path")"
    curl -fsSL --retry 3 -o "assets/$path" "$url"
done
python3 install.py --check
```

### Layout produced

Destination paths keep the upstream folder names, including spaces. They are
relative to `assets/`, and the modules below address them as `assets/<path>`.

```
assets/vsco/Strings/Cello Section/susvib/      cello section sustains
assets/vsco/Strings/Viola Section/susvib/      viola section sustains
assets/vsco/Strings/Violin Section/susVib/     violin section sustains
assets/vsco/Strings/Solo Contrabass/SusNV/     contrabass sustains
assets/vsco/Strings/Solo Violin/Arco Vib/      solo violin arco vib
assets/vsco/Strings/Harp/                      harp
assets/vsco/Brass/F Horn/sus/                  horn sustains
assets/vsco/Brass/Trumpet/                     trumpet sustains
assets/vsco/Brass/Tenor Trombone/              trombone sustains
assets/vsco/Woodwinds/Flute/                   flute sustains and staccato
assets/vsco/Woodwinds/Oboe/Vib/                oboe sustains
assets/vsco/Woodwinds/Clarinet/susLong/        clarinet sustains
assets/vsco/Woodwinds/Bassoon/                 bassoon sustains
assets/vsco/Percussion/Timpani/                struck timpani
assets/vsco/VSCO 1 Percussion/varMetal/Cymbals/susp/  suspended cymbal
assets/vsco/LICENSE, assets/vsco/README.md     upstream attribution
assets/shorts/Strings/Cello Section/spic/     cello section spiccato
assets/shorts/Strings/Violin Section/Spic/    violin section spiccato
```

Three pinned files (the upstream LICENSE and README, and one alternate timpani
hit take) are kept alongside the catalogs rather than referenced by them.

## Calibrated sustains

`calibrated.muz` now reads the pinned original recordings and applies the historical
body-RMS corrections as per-zone `gain_db`. Each note owns its correction, including
its release. `install.py` alone is sufficient: the 124 derived float WAVs (about
447 MB) are no longer needed. Existing derivatives are left untouched.

`calibration.json` records the original source hashes, measured mono body RMS over
0.4–2.8 seconds, and gains targeting -24 dBFS. These are the actual derivative
measurements, not estimates from the older pitch/velocity level tables. The small
`calibration-gains.muz` module carries the same values in source:

```sh
python3 contrib/vsco-2-ce/export-calibration.py --check
```

Omit `--check` to regenerate the source table. This validates the measurements against
the pinned manifest. `calibrate.py` remains a legacy derivative reproduction tool;
it is unnecessary for playback and does not update the committed gain tables.
Gain now follows resampling instead of being baked into float samples, so tiny
floating-point differences are possible; recording selection, tuning and onsets remain.

## Modules

Zone tables are tabulated per instrument; order is musically significant
because samplers select round robins by index.

| Module | Contents |
|---|---|
| `strings.muz` | cello, viola and violin section sustains with level tables |
| `winds.muz` | flute sustains with level table |
| `expanded.muz` | horn, oboe, contrabass, harp, struck timpani, timpani roll, suspended cymbal |
| `extra.muz` | clarinet, bassoon, trumpet, trombone (zone gain baked in) |
| `shorts.muz` | cello and violin section spiccato, already built into samplers |
| `alternates.muz` | a second flute sustain mapping, solo violin arco vib, two-take flute staccato |
| `calibrated.muz` | original sustains with per-zone calibration, levels reported as -24 dBFS |

```muz
use "contrib/vsco-2-ce/strings" as strings;
use "contrib/vsco-2-ce/winds" as winds;
use "contrib/vsco-2-ce/calibrated" as calibrated;

let cello = strings.instrument(strings.cello_zones, {attack_ms: 82, release_ms: 290});
let flute = calibrated.instrument(calibrated.flute_zones, {attack_ms: 18, release_ms: 130});
```

Use `instrument(zones, options)` from the module that owns the zones rather
than calling `sample(zones, options)` directly: the engine resolves sample
paths in the module that defines the function, so a call from a piece would
look for the recordings next to the piece. `shorts.muz` already exposes its
finished `cello_short` and `violin_short` samplers.

Level tables (`cello_levels`, `violin_levels`, ..., keyed by pitch and velocity
range) carry the measured level of each zone for pieces that want a
source-derived gain lane; `extra.muz` folds that gain into each zone's
`gain_db` instead, and `calibrated.muz` replaces it with -24 dBFS everywhere.

## Expressive voices and remaining design work

```muz
use "contrib/vsco-2-ce/expressive" as expressive;
let flute = expressive.voice("flute", [71, 74], 6000Hz, {release_ms: 130});
let cello = expressive.strings("cello", [46, 48]);
```

`voice(name, keys, tone, options, sample_budget_frames)` provides a stereo note-local filter; pressure
raises cutoff by one octave. `strings(name, keys, options)` accepts cello, viola,
or violin, retaining round robins separately in the soft and loud layers. Pressure
crossfades the two calibrated timbres from 0 to 1; note velocity still controls
amplitude. These are opt-in sounds with new envelope policies, not replacements
for the existing samplers. The optional `keys` argument restricts loaded zones and coverage; its default
`[0, 127]` keeps the complete map. Both helpers explicitly allow 16,777,216 decoded
frames (128 MiB), sufficient for each full section-string map. `voice` accepts a
`sample_budget_frames` argument; `strings` accepts it in its patch `options`.
Layered strings use a 30 ms attack and 250 ms release; `options` overrides patch
controls such as gain. Both layer maps must cover every performed note.

Research on 2026-09-12 found that the upstream
[CE cello sustain SFZ](https://github.com/sgossner/VSCO-2-CE/blob/6dd651d55dde97fd4028699be9d4481f26917891/CelloEnsSusVib.sfz)
does not specify loop points; the installed C1 v1 recording also has no embedded
`smpl` loop chunk. This does not establish a validated loop set for our maps.
Sustain-loop defaults remain deferred until candidate regions are auditioned for
vibrato continuity, crossfade beating, and release behavior. Do not use the separate
VSCO Pro manual as evidence that CE contains equivalent looping or dynamic controls.

Full section-string maps can now be prepared with the explicit sample budget; range
selection is optional. Simultaneous dynamic layers can also reveal timing or
phase differences between recordings: audition the chosen register before using a
crossfade as a replacement for velocity selection.
