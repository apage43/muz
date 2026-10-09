# Instruments, samples, and plugins

Choose a sound source for a track, then configure any recordings or plugins it
needs. This guide assumes the [language basics](language.md). Native synthesis
needs no external assets; sample and plugin examples require the named local files.

[Documentation index](README.md) · Offline: `muz docs instruments`

## Contents

- [Choose an instrument](#choose-an-instrument)
- [Native presets and kits](#native-presets-and-kits)
- [Samples and zones](#samples-and-zones)
- [Clock-timed clips](#clock-timed-clips)
- [External plugins](#external-plugins)
- [Asset paths and optional packs](#asset-paths-and-optional-packs)

## Choose an instrument

| Source | Use it for | External requirements |
| --- | --- | --- |
| `synth("glass-lead")` | A ready-to-play native sound | None |
| `voice_patch(...)` | A programmable native instrument | Recordings only if the patch reads samples |
| `sample(...)` | Pitched recordings or velocity/round-robin maps | WAV or FLAC files |
| `kit(...)` | Named drum or articulation voices | Depends on the voices |
| `plugin(...)` / `piano(...)` | VST3 or CLAP instruments | Installed plugin, any license and preset assets |
| `clip(...)` | A recorded region placed in seconds | WAV or FLAC file; creates its own track |

## Native presets and kits

```muz
let lead = synth("glass-lead", {cutoff_hz: 2800});
let notes = phrase("C4:q E4:q G4:h");
song({tracks: [track("lead", notes, lead)]})
```

Presets include `pulse-bass`, `glass-lead`, `pad`, `choir`, `bell`, `fifths`, and
percussion voices. Their definitions are readable in [std/catalogs](../std/catalogs.muz).
Use `muz devices inspect studio_synth` for native synth controls; see
[synthesis](synthesis.md) for envelope conventions, expression, and custom patches.

A kit maps semantic voices such as `kick`, `snare`, and `hat` to instruments.
For example, `kit("default", {snare:sample("snare.wav",{one_shot:true})})`
overrides one voice. Kits expand into physical tracks named from the logical
track and voice, such as `drums.kick`. Read [drum performance](performance.md#drums-and-kits)
for choke groups and voice processing.

## Samples and zones

`sample("audio.wav",{root:60,offset:0s,attack_ms:2,release_ms:30})` plays a recording
at the note's pitch. Samples decode mono/stereo WAV or FLAC before playback.
Integer and float WAV files and different sample rates are supported.

An array of paths rotates between takes on successive attacks (round robin).
An array of zone records additionally selects recordings by pitch and velocity:

| Zone field | Meaning |
| --- | --- |
| `path` | Recording path relative to the declaring module. |
| `root` | The recording's MIDI pitch, including fractional tuning. |
| `keys` | Inclusive MIDI key range; default `[0,127]`. |
| `velocity` | Velocity layer; default `[0,1]`. |
| `offset` | Initial playback offset. |
| `loop` | `[start_seconds,end_seconds]` within the recording. |
| `one_shot` | Whether playback continues after note-off. |
| `gain_db` | Static calibration for this recording. |

### Note-off and one-shot playback

A standalone sample follows note-off by default. Inside `kit()`, sample voices
instead default to `one_shot:true`, so the recording continues after the written
note ends and `release_ms` does not stop it. When using kit voices for pitched
articulations, explicitly set `one_shot:false` in each sample's options, for
example `sample("guitar.wav",{root:40,one_shot:false,release_ms:95})`. Explicit
zone settings still take precedence over instrument defaults.

### Recording calibration

Zone records also accept `gain_db` (−120–120, default 0): a static recording
calibration captured by each sample voice at note-on. It multiplies the voice's
velocity response without changing velocity-layer selection, round robins, or
older releases. `gain_db` in the instrument options remains a shared automatable
control and is **not** inherited as zone gain. Old serialized zones default to
0 dB. Changing zone calibration prepares a replacement instrument, like changing
a zone's root or sample path.

Calibration measurement and target level belong in source/project recipes:

```muz
fn calibrated(zones, measured_db, target_db) =
    map(range(len(zones)), fn(i) => merge(zones[i], {
        gain_db: target_db - measured_db[i]
    }));
// Original files stay in use; each recording has its own compensation.
let strings = sample(calibrated([
    {path:"soft.wav",velocity:[0,0.5]},
    {path:"loud.wav",velocity:[0.5,1]}
], [-30,-18], -24), {root:60,velocity_track:0.8});
```

The target level and measurement method are project choices. Zone gain applies
the compensation during playback, so this recipe needs no derived audio files.

### Root pitch and fine tuning

`root` is the recording's MIDI pitch in `[0,127]`, including fractional values
for fine tuning. For a sample with measured fundamental `f` Hz, calculate its
MIDI root as 69 + 12 log₂(f/440). For a waveform repeated every `n` frames at sample
rate `r`, `f=r/n`. For example, 32 frames at 8,363 Hz need
`root:59.981341609272455`. The sampler preserves that value through compilation,
session serialization and playback; rounding or truncating it detunes every note.
Zone `root` values override the parent sample root; integer roots remain valid.

### Key and velocity coverage

Graph preparation (including `muz check`, render and live reload) rejects performed
sampler notes with no matching zone before loading instruments. The diagnostic
names each affected physical track, its missing-note count, and an example pitch,
velocity and source key. Keys use the same rounded MIDI key as playback; fractional
pitch remains available for tuning. Key bounds are inclusive. Velocity layers are
`[low,high)`, except an upper bound of 1 includes full velocity. Checking uses the
performed float velocity, not its MIDI-export quantization. Overlapping matching
zones still rotate round robin; this check never remaps notes.

### Pinning a recording to a note

To fix a recording choice to a note, use
`pattern.annotate("last", {sample_zone:2})`. `sample_zone` is a zero-based index
into the instrument's complete zone list, not the list of matching alternates.
The selected zone must match the performed key and velocity; invalid indices,
wrong layers and non-sampler destinations fail during compilation. Graph
preparation also validates imported annotations before playback. The annotation
survives selection, placement, repeat and lane edits. Keep the zone list order
stable, and reassign choices if transposition or velocity changes invalidate them.
This is an attack choice, not a continuous expression control; an already sounding
voice keeps its recording. MIDI export does not encode recording choices.

Unannotated notes retain the original sampler policy: one counter per instrument,
starting at zero, advances on every matching attack (including single-zone and
explicitly selected attacks). It selects counter modulo matching-zone count.
Pinning one note therefore leaves subsequent unannotated choices unchanged on the
same lane. Lane splits can still change unpinned choices; pin the full reference
before splitting when all recordings must remain stable.

The source recipe `std/sampler.pin_recordings(pattern,zones,clock={})` assigns
round-robin choices in performed attack order. Supply the complete original
pattern, explicit zone records and the song timing record; then split the result:

```muz
use "std/sampler" as sampler;
let pinned = sampler.pin_recordings(original, zones, {tempo:120});
let main = pinned.reject("tag:solo");
let solo = pinned.select("tag:solo");
// Give both lanes sample(zones, options); expression can now differ by lane.
```

This recipe is a source policy for a fresh instrument, not a capture of live
sampler state. It requires unique note keys and explicit zone ranges when parent
options would supply them. It sorts by source performed time, retaining pattern
order for ties; attacks separated only below the compiler's timing precision or
velocities at floating-point layer boundaries may need explicit choices to
reproduce a prior render. Existing hand-selected choices can be authored with
`map_notes(fn(n)=>{data:merge(n.data,{sample_zone:choice})})` instead. Selection
policy stays in source; only delivery of the chosen zone to the note-on belongs
to the engine.

## Clock-timed clips

This fragment creates a track containing a recorded region:

```muz
clip("texture", "audio.wav", {
    at: 8s, offset: 2s, duration: 6s,
    fade_in: 100ms, fade_out: 400ms, gain: -12
})
```

`at` places it in clock time; `offset` trims the start of the recording. Clips
support trimming, fades, and sample-rate conversion, but no time stretching.
Change the trim through `clip` options. See the
[language timing rules](language.md#clock-clips) for restrictions on transforming
patterns that contain clips.

## External plugins

### Discover and inspect

```sh
muz devices list
muz devices inspect /path/to/Instrument.clap
muz devices inspect /path/to/Instrument.vst3
```

Inspect the installed plugin for class IDs, parameter IDs/keys, ranges, and ports.
Then declare it in source; these fragments require your own paths and IDs:

```muz
plugin("/path/to/Instrument.clap", {class: "plugin.id", state: "patch.state", p123: 0.4})
plugin("/path/to/Instrument.vst3", {class: "class-id", state: "patch.state", parameter_key: 0.5})
```

`plugin` can describe an instrument or an effect. If `class` is omitted, the first
audio class is selected. Linux stereo plugins are the exercised layout.

| Format | Parameter values in muz source |
| --- | --- |
| VST3 | Normalized 0–1, addressed by numeric ID or the inspected key. |
| CLAP | The plugin's plain units; some plugins themselves expose a 0–1 range. |

For VST3, `muz devices convert PATH PARAMETER PLAIN_VALUE` uses the controller's
actual plain-to-normalized mapping. CLAP already uses plain values. Automation
uses sample-offset parameter queues; interpolation within the plugin is the
plugin's responsibility.

### Local aliases

Machine paths and plugin class identifiers live in
`$XDG_CONFIG_HOME/muz/plugins.json` (normally `~/.config/muz/plugins.json`).
`MUZ_PLUGIN_CONFIG` selects another file. It contains an object of named aliases:

```json
{
  "default": {
    "path": "/chosen/location/Pianoteq.vst3",
    "class": "the-class-id-from-muz-devices-inspect"
  },
  "my-synth": {"path": "/chosen/location/instrument.clap"}
}
```

`piano()` uses the `default` alias; named plugin references can resolve an alias.
Explicit source fields override the alias. Paths relative to the configuration
file resolve against its directory. Choose and inspect the actual installed
plugin; there is no built-in versioned Pianoteq path. Project state and source
parameter overrides remain project inputs.

An alias must point to an installed plugin. `piano()` resolves the `default`
alias; it does not discover a particular Pianoteq version automatically. The
[shared plugin pack](../contrib/plugins/README.md) documents reusable settings
for specific plugins. Device CLI commands also accept alias names.

### State and preparation

```sh
muz devices state /path/to/Instrument.vst3 -o preset.state
```

`--load PATH` loads state before saving. VST3 accepts raw component state or a
VST3 preset. Source instruments load state first and then apply parameter
overrides. State loading, parameter inspection, and plugin preparation happen
outside the audio callback. `muz check` prepares the graph and validates available
plugin parameters as well as musical source.

For exact named Pianoteq factory presets on Linux 9.2.4, the
[shared Pianoteq capture helper](../contrib/plugins/README.md#headless-factory-preset-capture-linux-924)
exports native VST3 presets using an isolated copy of existing activation
preferences, verifies the requested identity, and loads/resaves through the
plugin before atomically publishing project-owned component state:

```sh
python3 contrib/plugins/pianoteq-state.py \
  --preset 'C. Bechstein DG Warm' -o state/bechstein-dg-warm.state
```

It retains the selected alias's class (`--class` overrides it); `--plugin`,
`--pianoteq`, `--prefs`, and `--muz` select local inputs. The instrument pack must
be licensed. Factory settings remain unchanged; author musical overrides in
source after state loading. Other Pianoteq versions/platforms are unsupported.

Host restart notifications request a prepared replacement and recalculate latency
on the coordinator. Inspect/state commands run in separate muz invocations.
Live plugins run in the host process, so their crashes are not isolated;
background bounces run in child processes and cannot publish partial outputs.

### Ports and expression

CLAP supports main-thread callbacks, parameter/state inspection, mono/stereo ports,
native note expression, and MIDI channel input. The stereo main output is used;
auxiliary outputs are discarded and auxiliary inputs are silent. Plugin MIDI/event
output is not routed.

VST3 receives floating attack intensity and initial tuning. Subsequent per-note
curves need a capable native instrument, CLAP native-note port, or a separately
split layer. Capable adapters also support channel pressure, poly pressure,
bank/program, and bend. See [per-note expression](performance.md#per-note-expression)
for controls and destination support.

The locally exercised plugins are Pianoteq 9 VST3, Surge XT VST3/CLAP, Surge
XT Effects VST3/CLAP, and sfizz 1.2.3 VST3. This coverage does not guarantee arbitrary plugin compatibility.
Offline exports use the plugin's offline processing mode; see
[render behavior](workflow.md#plugin-render-behavior).

## Asset paths and optional packs

Asset paths are relative to the module declaring them. A function body evaluates
in its defining module, so a pack's instrument builder resolves its own samples.
Keep a `plugin(...,{state:...})` call in the module that owns that state path.

Samples and preset files are watched along with source imports. File length and
modification time trigger fresh preparation after edits. These are change detectors,
not content identities or guarantees of reproducible future rendering.

Each [contrib pack](../contrib/README.md) supplies source mappings, setup instructions,
and licensing/provenance notes. Sample installers place external content in ignored
asset directories. Document the licensed downloads and folder layout in your project.

`use "contrib/<pack>/<module>"` selects `MUZ_CONTRIB_DIR` when set, otherwise
the executable's checkout `contrib/`, otherwise `$XDG_DATA_HOME/muz/contrib`
(default `~/.local/share/muz/contrib`). An explicit override must exist and never
silently falls back. `./install.sh` installs the shared tree; no export is needed.
Plain `cargo install --path .` installs only the binary. See
[pack setup](../contrib/README.md#install-and-import) for installed sample downloads
or choosing a different library. Standard `std/` modules are bundled into the
binary and need no such configuration.

## Next steps

Use [production](production.md) to route and process the instrument, or
[synthesis](synthesis.md) to build a voice graph. The [workflow guide](workflow.md)
covers checking, live reloads, and exports.

## Native SFZ programs

`sfz("relative/program.sfz", {max_voices: 256, seed: 0})` prepares an SFZ
instrument relative to the declaring module. The second argument can provide
`defines: {sample_dir: "../samples"}` for mappings requiring externally supplied
macros. Preparation reads and normalizes the mapping and includes off the audio
thread; malformed or unsupported mappings report diagnostics rather than silently
substituting a preset. This constructor is distinct from numeric `sample_zone`
selection: SFZ chooses every matching region through its own mapping semantics.

Physical controllers are exposed as `cc0` through `cc127` in MIDI units 0–127.
Defaults come from the mapping; authored options such as `cc1: 100` override them.
The existing MIDI CC/bend and note-ID expression event paths drive the processor.
SFZ virtual modulation sources above 127 are internal sources, not physical MIDI
controllers. Full corpus qualification and reference-player agreement remain
subject to the acceptance gates in the implementation plan.

For verified upstream syntax defects, `source_overlays` can supply narrowly scoped
removals before parsing. Each descriptor records its mapping `path`, original
`sha256`, exact `removals`, and explanatory `reason`. The importer requires the
pinned original bytes and a unique occurrence of each removal. Pack builders use
these only for malformed tokens a reference player demonstrably ignores; overlays
do not infer missing opcode values or modify recordings. Saved state preserves
both these provenance descriptors and the resulting normalized program.
