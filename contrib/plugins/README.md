# Shared plugin recipes

Reusable settings for VST3/CLAP instruments and effects, plus helpers for JUCE
preset containers. Install plugins from their publishers and configure local
aliases before using these modules. This pack ships no plugin binaries or preset
state dumps; project-specific state belongs with the project that owns the sound.

[All packs](../../contrib/README.md) · [Instrument guide](../../docs/instruments.md)

Run shell commands from the `muz-core` checkout unless stated otherwise.
For a CLI installed outside this checkout, set `MUZ_CONTRIB_DIR` to this
checkout's absolute `contrib` path; see the [pack setup guide](../../contrib/README.md#install-and-import).

## Contents

- [Install and configure aliases](#install-and-configure-aliases)
- [Use the modules](#use-the-modules)
- [Create or inspect state files](#create-or-inspect-state-files)
- [OrbitCab](#orbitcab)
- [CHOWTapeModel](#chowtapemodel)
- [Surge XT](#surge-xt)
- [Pianoteq](#pianoteq)
- [Pack maintenance](#pack-maintenance)

## Install and configure aliases

Install the plugin you need; versions, formats, and licenses are listed below.
OrbitCab has a pinned installer:

```sh
python3 contrib/plugins/orbitcab-install.py
```

The command verifies the download and extracted binary and refuses to replace a
different build. Add `--dest DIR` for another plugin directory or `--check` to
verify an existing install. Other plugins are installed separately.

Machine paths and plugin class identifiers live in
`$XDG_CONFIG_HOME/muz/plugins.json` (normally `~/.config/muz/plugins.json`);
`MUZ_PLUGIN_CONFIG` selects another file. These are example paths; replace them
with your installed binaries and inspected class IDs. Use only the aliases you need:

```json
{
  "default": {"path": "/opt/pianoteq/Pianoteq 9.vst3", "class": "the-class-from-muz-devices-inspect"},
  "orbitcab": {"path": "/usr/lib/clap/OrbitCab.clap", "class": "com.darwinscat.orbitcab"},
  "chowtape": {"path": "/usr/lib/clap/CHOWTapeModel.clap", "class": "org.chowdsp.CHOWTapeModel"},
  "surge": {"path": "/usr/lib/clap/Surge XT.clap", "class": "org.surge-synth-team.surge-xt"}
}
```

To add an alias: run `muz devices inspect <installed plugin path>`, copy its
`id` into `class`, point `path` at the installed binary, and give it the alias
name a module expects. Relative `path` values resolve against the configuration
file's directory; an absolute path is the usual choice. A `plugin()` or `piano()`
that names an alias missing from this file fails with the alias named. An alias
supplies `path` and, when the source sets no `class`, its `class`; other keys are
ignored, and an explicit source field overrides the alias.

## Use the modules

```muz
use "contrib/plugins/chowtape" as tape;
use "contrib/plugins/orbitcab" as amp;
use "contrib/plugins/surge-xt" as surge;
use "contrib/plugins/pianoteq" as ptq;
```

Each module is also importable on its own; the pack has no shared index.

### Why the device call stays in your module

A device's `state:` path resolves against the `.muz` file that declares it, and a
function body evaluates in its defining module. A wrapper defined here would
therefore resolve a project-relative state path against `contrib/plugins/`. The
modules above consequently export the *settings record* and document the call
site; the `plugin()`/`piano()` call, and the path, stay in your own source:

```muz
let guitar = plugin("orbitcab", amp.settings("assets/amp/sheriff-greenback.state"));
let bass = plugin("surge", surge.settings("assets/surge/piston-bass.state"));
let keys = piano("default", ptq.settings("assets/piano/bechstein-warm.state"));
```

`chowtape.muz` is the exception: its recipe carries no file paths, so
`tape.tape(mix)` builds the whole insert. For example, after configuring `chowtape`,
this complete song processes a native synth through it:

```muz
use "contrib/plugins/chowtape" as tape;
song({tracks: [track("lead", phrase("C4:q E4:q G4:h"), synth("glass-lead"), {
    chain: [tape.tape(0.32)]
})]})
```

## Create or inspect state files

`.state` files belong to the composer's project; this library never ships them.
Build one from an editable XML payload with the helper in this directory:

```sh
python3 contrib/plugins/juce_state.py --wrap preset.xml --state preset.state
python3 contrib/plugins/juce_state.py --check preset.state --xml preset.xml
python3 contrib/plugins/juce_state.py --unwrap preset.state --payload preset.xml
```

`--check` decodes a stored state, re-encodes it and reports whether the bytes
match, so a generated container can be proven byte-identical before it is
committed to a project. `--template` reuses a stored container's header and
trailing block when a plugin's state is more than payload plus wrapper.

## OrbitCab

- Upstream: OrbitCab by Darwin's Cat - <https://github.com/darwinscat/orbitcab>
  (homepage <https://darwinscat.com/orbitcab>), version 2.5.0.
- Format: CLAP (upstream also builds VST3 and AU). Installed as
  `OrbitCab.clap` on a CLAP search path such as `~/.clap` or `/usr/lib/clap`.
- License: AGPL-3.0. The plugin's bundled preamp captures and cabinet impulse
  responses are content owned by its publisher; we redistribute none of them.
- Alias this pack expects: `orbitcab`, class id `com.darwinscat.orbitcab`.
- Install the pinned build with `python3 contrib/plugins/orbitcab-install.py`
  (add `--dest <dir>` for another plugin directory, `--check` to verify only).
  It refuses to replace a different build and verifies the release archive and
  the extracted binary against `orbitcab-manifest.json`.
- State convention: the plugin's tone - preamp capture, power stage, cabinet IR -
  lives in a JUCE `VC2!` container. Keep the XML source for it in your project
  and rebuild the container with `juce_state.py`; do not copy a state file out of
  another project and do not commit one here.
- Module: `orbitcab.muz` exports `settings(state, overrides = {})`. Two
  independent mono instances panned after the plugin give the doubled width.

## CHOWTapeModel

- Upstream: CHOWTapeModel by Chowdhury DSP (Jatin Chowdhury) -
  <https://github.com/jatinchowdhury18/AnalogTapeModel>, version 2.11.4.
- Format: CLAP. Installed as `CHOWTapeModel.clap` on a CLAP search path.
- License: GPL-3.0.
- Alias this pack expects: `chowtape`, class id `org.chowdsp.CHOWTapeModel`.
- State convention: none. The module is a pure parameter recipe, so it has no
  preset state to keep; every setting it writes is named and commented in
  `chowtape.muz`.
- Module: `chowtape.muz` exports `tape(mix, drive, saturation)` (and the harder
  `driven(mix)`). Both use 4x oversampling on the live and render paths with tone
  processing, head-loss coloration and wow/flutter switched off.

## Surge XT

- Upstream: Surge XT by the Surge Synth Team -
  <https://github.com/surge-synthesizer/surge> (downloads at
  <https://surge-synthesizer.github.io>), version 1.3.4.
- Format: CLAP (upstream also builds VST3 and AU/standalone).
- License: GPL-3.0. Factory patches are content of that project; we
  redistribute none of them.
- Alias this pack expects: `surge`, class id `org.surge-synth-team.surge-xt`.
- State convention: patches are `.state` files authored from an editable XML with
  `juce_state.py`. Surge's container is the 32-byte `sub3` form, which also
  carries a trailing block after the XML payload; build it with
  `--magic sub3 --template original.state` so the header and trailing block are
  preserved. Your project keeps the XML and the `.state` it builds.
- Module: `surge-xt.muz` exports `settings(state, overrides = {})`.

## Pianoteq

- Upstream: Pianoteq by Modartt - <https://www.modartt.com/pianoteq>, version 9
  (the composer's installed build).
- Format: VST3 (also AU/AAX/standalone). The builtin `piano()` resolves the
  `default` alias unless the source names another plugin.
- License: proprietary, commercial. Editions and instrument packs are licensed
  per user; the instrument a state refers to must be owned by whoever renders it.
- Alias this pack expects: `default`.
- State convention: whatever the composer saves for the instrument and preset.
  The instrument, its license and the state dump belong to the composer, so this
  library ships none of them and `pianoteq.muz` pins no engine parameters.
- Module: `pianoteq.muz` exports `settings(state, overrides = {})`.
- MIDI mapping: `pianoteq/classical-guitar.ptm` is a six-string guitar workflow
  mapping - channels 0-5 carry the strings, channel 15 carries technique keys,
  and controllers 64/66/67/69 drive the instrument's `Pdl[4]/[3]/[1]/[2]`
  pedals. Install it as `~/.local/share/Modartt/Pianoteq/MidiMappings/<name>.ptm`
  on Linux and select it in the instrument's MIDI settings; a preset that refers
  to a mapping by name mis-routes silently while that mapping is absent.

## Pack maintenance

| File | What it owns |
| --- | --- |
| `orbitcab.muz` | alias + settings record for the OrbitCab amp/cabinet insert |
| `orbitcab-manifest.json` | pinned OrbitCab release: URL, archive and binary SHA-256 |
| `orbitcab-install.py` | installs/verifies that build in the user plugin directory |
| `chowtape.muz` | tape insert recipe (parameter ids, oversampling policy, switches) |
| `surge-xt.muz` | alias + settings record for a Surge XT instrument instance |
| `pianoteq.muz` | alias + settings record over the builtin `piano("default", …)` convention |
| `pianoteq/classical-guitar.ptm` | Pianoteq MIDI mapping for a six-string guitar workflow |
| `juce_state.py` | wrap/unwrap the `VC2!`, `VstW`/`sub3` and raw-XML state forms |

The following checks are useful when editing pack source or the state helper:

```sh
./target/release/muz fmt --check contrib/plugins/*.muz
python3 -m py_compile contrib/plugins/juce_state.py
```
