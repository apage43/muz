# Sonatina Symphonic Orchestra 4.0

The complete upstream Sonatina library, played through an SFZ plugin. This keeps
its own instrument programming: velocity layers, round robins, loops, release
samples, envelopes, filters, keyswitches, legato and controller modulation.
The pack installs **all 557 playable SFZ programs**, including 73 keyswitch
programs. No recordings or plugin binaries are committed here.

[All packs](../README.md) · [Complete patch catalog](CATALOG.md) ·
[Plugin hosting](../../docs/instruments.md#state-and-preparation)

## Install the library

The commands below use the `muz-core` checkout. For the installed CLI, run the
utilities from the installed copy instead, for example:

```sh
python3 "$HOME/.local/share/muz/contrib/sonatina/install.py"
```

Substitute `$XDG_DATA_HOME/muz/contrib` if you use an absolute `XDG_DATA_HOME`.
Use that same installed pack path for `make-state.py` and the catalog utilities.
See [pack setup](../README.md#install-and-import). Checkout commands:

```sh
python3 contrib/sonatina/install.py
python3 contrib/sonatina/install.py --check       # verify without downloading
python3 contrib/sonatina/install.py --force       # replace files that differ
python3 contrib/sonatina/install.py --transport files --jobs 8  # individual downloads
```

Python 3 is the only installer dependency. The 3,827 pinned files total
**1,481,120,173 bytes** (1.48 GB / 1.38 GiB). The installer downloads from
GitHub at revision `64a66eda18c5cc1039a56c902d0555df56742300` (tag `v4.0`),
verified by size and SHA-256, and placed atomically under `assets/sso/`.
For a new installation (more than 32 missing files), it streams one compressed
GitHub archive and verifies each file against the manifest before placing it.
No archive is retained. Smaller repairs download individual files in parallel
(default 4 jobs). `--transport archive` or `--transport files` overrides the
automatic choice. An interrupted download keeps files already verified and
placed; rerun to finish. Re-running verifies existing files and fetches missing ones. Differing files
require an explicit `--force`; `--check` exits nonzero for missing or differing
files. The full upstream folder structure and licenses are preserved.

## Player and project setup

Use an SFZ player supported by muz, such as sfizz VST3. Upstream tests its patches
with **sfizz and sforzando**. The pack's helper expects a `sfizz` alias, configured
in your muz `plugins.json`; an explicit plugin path works too. See
[plugin aliases](../../docs/instruments.md) for the configuration location.

A plugin state selects the SFZ file. Keep that state in the project that uses it,
and call `plugin(...)` from the same project's module so relative state paths
resolve correctly. The library and plugin must remain installed at the paths
recorded by the state. The tested player is **sfizz 1.2.3 VST3**, with state format version 5.

Build/use a current muz executable (the integration includes host compatibility
fixes for sfizz). Then find a patch and create project-owned state:

```sh
cargo build
python3 contrib/sonatina/make-state.py --list violins
python3 contrib/sonatina/make-state.py \
  --patch strings-performance-1st-violins-ks \
  --output /path/to/your-project/assets/sonatina/violins.state \
  --plugin /usr/lib/vst3/sfizz.vst3 --muz target/debug/muz
```

Choose your own project directory and installed plugin path. The default
`--muz` is `muz`, and the default `--plugin` is the `sfizz` alias. To use that
alias in project source, add its entry to your existing
`~/.config/muz/plugins.json` (or `$XDG_CONFIG_HOME/muz/plugins.json`):

```json
{
  "sfizz": {"path": "/usr/lib/vst3/sfizz.vst3"}
}
```

`make-state.py` asks the installed plugin for a fresh state, changes only its
SFZ path field, then reloads and saves it through the plugin and verifies the
selected path. It rejects unsupported state versions and writes the final state
atomically. No hand-built defaults or recorded plugin binary are shipped.
The utility is specific to sfizz VST3; sforzando requires its own saved preset
workflow and is not locally validated by this pack.

```muz
use "contrib/sonatina/plugin" as sso;

let violins = plugin(sso.alias, sso.settings("assets/sonatina/violins.state"));
let phrase = seq([note("G4", 1b), note("A4", 1b), note("B4", 2b)]);
// Pass this instrument and pattern to your project's track/song arrangement.
let shaped_phrase = stack([phrase, sso.dynamics(96), sso.vibrato(25)]);
```

With `./install.sh`, installed muz discovers its installed contrib copy without
an export. Download samples and generate state from that copy so the recorded
SFZ path stays valid even if the checkout moves or is deleted. To intentionally
use another contrib tree, set `MUZ_CONTRIB_DIR` to its absolute path. Projects can
live anywhere.

## Select instruments and articulations

[CATALOG.md](CATALOG.md) lists every playable patch with its stable pack ID, SFZ
name, numeric MIDI keyswitches and labeled/initialized controllers.
[catalog.json](catalog.json) contains the same information for tooling; its
`path` values are relative to this pack's `assets/` directory.

| Family | Programs |
| --- | ---: |
| Strings, Notation / Performance | 96 + 96 |
| Woodwinds, Notation / Performance | 96 + 96 |
| Brass, Notation / Performance | 66 + 66 |
| Chorus, Notation / Performance | 2 + 2 |
| Organ | 17 |
| Percussion, including pitched percussion | 14 |
| Harpsichord | 4 |
| Grand Piano | 1 |
| Harp | 1 |

The count includes articulation variants, composites such as All Strings,
looped/unlooped choices, and KS programs. SFZ include fragments are installed
but are not separate playable patches. No playable upstream patch is omitted.

Choose **Notation** for velocity-driven loudness. Its marcato patches use CC1
for attack strength. Choose **Performance** for CC1 dynamics on sustained
articulations; velocity often controls attack speed or marcato accent instead.
Short articulations generally use velocity for loudness. Many strings use CC21
for vibrato, with exceptions where vibrato is recorded in the samples. The
catalog shows controls declared by each SFZ; upstream defaults still apply.

Use separate articulation patches for simple instrument selection. To switch
articulations within one instance, select a `KS` patch and send its documented
keyswitch note on the same track before the corresponding musical attack:

```muz
let switched = stack([
    sso.keyswitch(41),                // First Violins KS: pizzicato (MIDI F2)
    phrase.at(1b / 32),               // leave time for the switch to arrive
    sso.dynamics(96),
]);
```

Use the numeric keys from the catalog: octave names differ across software.
Keyswitch programs differ by instrument; do not assume all use the violin map.
Legato patches are monophonic and need overlapping notes for transitions, as
specified by upstream. Ordinary sustain patches are the appropriate choice for
chords. For controller curves, schedule additional `control(number, value,
at=...)` events or the helper's `dynamics`/`vibrato` calls; values are integers
0–127. These controls follow the track's MIDI channel.

## Reproduce the catalog

After installing the pinned library:

```sh
python3 contrib/sonatina/export-catalog.py --check
python3 contrib/sonatina/export-catalog.py         # regenerate metadata/docs
python3 contrib/sonatina/test-pack.py              # synthetic installer/state checks
```

The exporter verifies SFZ hashes, follows include files and simple defines, and
records patch names, keyswitch labels and declared CC metadata. It is a catalog
tool, not a replacement SFZ interpreter. The plugin reads the original files.

## Verification

The complete installation passed per-file size/SHA-256 verification; catalog
regeneration matches all 557 programs. The state setup helper was exercised with
sfizz 1.2.3 VST3. Representative Grand Piano and First Violins Performance KS
states loaded and rendered successfully through muz, including a keyswitch and
CC1 dynamics. Synthetic checks cover malformed state rejection, preserving
plugin settings, archive pin failures, and archive path validation.

## Limits and upstream details

- sfizz currently prints an `Incomplete SCL file` warning for its empty optional
  Scala tuning path during state loading; the tested patches still load and
  render.
- A compatible SFZ player is required. Playback fidelity depends on its SFZ
  implementation; full catalog coverage does not mean every opcode has been
  individually auditioned or proven identical between players.
- Plugin state and the installed library's paths must be preserved when moving
  a project. Regenerate state after relocating the library. DAW/session sharing
  also needs the player and external recordings.
- Non-looped patches naturally stop at the recording's end. Looping, legato,
  recorded vibrato and articulation availability follow the upstream patches.
- Source recordings have different room sound and sampling detail. Upstream
  recommends added reverb to blend the orchestra; the pack adds no mixing policy.
- VSCO and its import paths remain independently available.

Upstream: [peastman/sso v4.0](https://github.com/peastman/sso/tree/v4.0), originally
created by Mattias Westlund and maintained in this repository by Peter Eastman
and contributors. Sonatina uses the **Creative Commons Sampling Plus 1.0**
license, not CC0. Read the installed `assets/sso/LICENSE` and the source and
licensing history in `assets/sso/README.md` when documenting your project's
chosen assets. The SFZ player has its own license.
