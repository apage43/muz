# DAWProject export

`muz export` creates an editable DAWProject archive and a fidelity report from
one compiled song. Use it for a one-way handoff to a DAW: notes, arrangement,
and exposed automation can be edited there, but those edits cannot be converted
back into authored muz source.

The tested target profile is Bitwig Studio 6.0.11 on Linux. Read the report before
using an export for production; a successful export does not imply lossless
conversion. See the [dated validation evidence](dawproject-validation.md) for what
has been observed in that host.

[Documentation index](README.md) · Offline: `muz docs dawproject`

## Contents

- [Install the native wrapper and export](#install-the-native-wrapper-and-export)
- [Read the fidelity report](#read-the-fidelity-report)
- [Supported data and limitations](#supported-data-and-limitations)
- [DAW setup and troubleshooting](#daw-setup-and-troubleshooting)
- [Replace native device source](#replace-native-device-source)
- [State and runtime contract](#state-and-runtime-contract)

## Install the native wrapper and export

Native muz instruments and effects are carried by the Muz CLAP wrapper. Build
and install it from the engine checkout:

```sh
cargo build --release --package muz-clap --no-default-features
mkdir -p ~/.clap
cp target/release/libmuz_clap.so ~/.clap/Muz.clap
```

Restart or rescan the DAW after installation. The bundle exposes
`com.muz.instrument` and `com.muz.fx`. Keep the same plugin version available
when reopening an export. The recipient must also install any third-party
plugins used by the song.

With the muz CLI installed, run from your project directory:

```sh
muz export song.muz -o song.dawproject
```

`--format dawproject` and `--profile bitwig-linux` are the defaults. Without an
installed CLI, use `cargo run -- export /path/to/song.muz -o /path/to/song.dawproject`
from the engine checkout.

## Read the fidelity report

For `song.dawproject`, the default reports are `song.dawproject.report.json`
and a readable `song.dawproject.report.txt` beside the archive. Their names replace
the output extension with `dawproject.report.json` / `dawproject.report.txt`.
`--report PATH` selects a different JSON report path. Keep the report with the
archive when transferring the project.

`--strict` rejects any export with a fidelity warning and leaves an existing
archive untouched. The report remains available after strict rejection. Missing
required assets/plugins, invalid state, and invalid graphs fail export.

The report's `entries` are sorted by physical path, code and feature. Each entry
contains a stable code, severity, source location when available, logical and
physical paths, affected feature, intended sound or structure, exported behavior,
fidelity outcome and a practical remedy. `required_plugin_ids` identifies the
plugin roles needed to play the handoff. `external_dependencies` names any source
dependencies that remain outside the archive.

Common actions include:

| Report code | Meaning and action |
| --- | --- |
| `SAMPLE_ZONE_OMITTED` | The note is editable, but its pinned sampler recording is omitted. Use native muz for exact pinned selection; re-exporting does not restore it. |
| `SIDECHAIN_MANUAL_SETUP` | The declared detector source is disconnected. Connect and verify the plugin's Detector input in the DAW if available. |
| `SONG_TAIL_MANUAL_SETUP` | Extend the DAW export range by the reported tail duration; the archive does not set that range. |

If the host cannot expose the required Detector input, use native muz playback
or recreate the routing and processing externally. `--strict` rejects these
warning outcomes.

## Supported data and limitations

| Feature | Exported representation | Compatibility boundary |
| --- | --- | --- |
| Notes and clips | Editable performed notes | Editing, duplication, save/reopen, and bounce were observed in Bitwig. |
| Note expression | Pitch, pressure, and timbre timelines inside notes | Visible expression survived a note move and had an audible effect; exact curve preservation and native parity remain unverified. |
| Channel controls | Lanes identified by channel and controller, including pedals | CC64's displayed lane was observed. Imported CC11 high/low bounces were identical despite native-wrapper response; use native muz for guaranteed controller behavior. |
| Native DSP | Muz Instrument/Muz FX with code-backed state, including serial/parallel racks | State persistence and relocated embedded-sample audio were observed; exact DSP and pinned-zone parity remain open. |
| Arrangement | Tempo points, meter, authored groups, and section markers | Tempo changes, kit groups, and section labels were observed. |
| Mixer | Tracks, buses, master, output routes, sends, and volume automation | Send level/enabled state and response consistent with a post-fader send were observed; exact tap parity remains unverified. |
| External plugins | CLAP/VST3 identities, effective public values, and available state | ZamEQ2 CLAP loaded with an override; VST3 import remains unverified. |
| Sidechains | Muz FX detector input | Manual connection produced a repeatable audio change; exact source tap, timing, and DSP parity remain unverified. |

### Native devices and assets

The exporter writes editable performed notes, track and bus channels, output
destinations, sends, tempo points, meter, section markers and authored groups.
StudioSynth, samplers, voice patches and simple built-in effects can be carried
as Muz CLAP instances when their standalone source reconstructs the evaluated
device. Required WAV/FLAC samples are embedded by SHA-256 identity, subject to
a 32 MiB asset and 64 MiB state limit. State restoration validates the source,
asset digests and device structure. Persistent parameter IDs use DAWProject's
signed 32-bit range and include default controls. Simple native control
automation is exported as points; smooth curves are approximated within a
0.001 normalized-value bound and reported as such.

Native racks retain serial and parallel DSP, exposed realtime controls,
modulation, mix and latency inside Muz FX. Nested external plugins and sampled
racks are currently rejected from standalone rack state and diagnosed in the
report. Rack structural settings remain in code-backed state rather than host
automation.

### External plugins and automation

Original third-party CLAP and VST3 instances are exported with their identities,
effective parameter values and saved state after source overrides. VST3 state
currently includes the component chunk; controller-private state is specifically
reported as omitted.

Plugin parameter declarations and points use normalized 0–1 values derived from
the plugin range; typed mixer/send Volume uses linear amplitude. Approximation
warnings state their value domain and error bound. Structural rack controls are
not exposed as realtime automation. Embedded native assets are portable;
third-party opaque state may retain external file dependencies.

The report names every device or feature that this implementation omits or cannot
yet verify. Nested external/sampled racks, exact sidechain connections, some expression slots,
sampler zone pins and uninterpreted raw MIDI messages have explicit omissions or
manual fallbacks in the report. Native FX sound parity,
complex routing, exact expression fidelity, controller behavior beyond the diagnosed
CC11 case, and VST3 import remain unverified in Bitwig. A schema-valid archive alone
does not establish audible equivalence. These limitations remain part of the
initial handoff contract, even where bounded probes establish an audible response.

## DAW setup and troubleshooting

### Missing plugin or silent instrument

If a DAW reports a missing Muz plugin, install the `.clap` library shown above,
rescan CLAP plugins and reopen the project. If notes appear but sound is absent,
check the instance state and the report's `required_plugin_ids` and
`external_dependencies`.

### Connect a sidechain

In Bitwig 6.0.11, select the Muz FX compressor in the device panel and click its
sidechain icon in the plug-in header: the small downward-arrow/branching-box
symbol between the parameter knob button and the search field. This opens
`Select sidechain input`. Choose the source track and the required tap; the probe
offered `kick PRE`, `kick POST` and `kick Muz Out` and selected `kick POST`.
This is the plug-in's auxiliary-input panel, not the track-input chooser.
Compare detector level, timing and sound with the original Muz render before
relying on the connection; that comparison remains unverified.

In other hosts, first check whether the plug-in Detector input is exposed. If
it is, connect the reported source and verify its tap, timing and sound. If it
is not, use native Muz playback or recreate the routing and processing externally.
Keep the report with the archive when transferring it.

### Parameter discovery in other hosts

The Bitwig profile is the tested target. On its first preset load, Muz suppresses
CLAP parameter rescans because Bitwig reports an initialization restart error
when a rescan is sent. Bitwig 6.0.11 enumerated the imported controls and played
their automation in the synthetic probe. Other hosts may cache the default
controls before loading state; parameter discovery and automation in those hosts
remain unverified.

## Replace native device source

To revise a native plugin's code, extract its `plugins/*.clap-preset` state from
the archive, write a standalone Muz device expression, then run:

```sh
muz devices replace-source old.clap-preset revised-device.muz -o revised.clap-preset
```

The command accepts the DAWProject CLAP preset container and preserves its
plug-in ID and framing. It also accepts an older raw Muz JSON state and writes
raw JSON in that case. Replace the same `plugins/*.clap-preset` entry in a copy
of the archive, or load the resulting state through the host's preset/state
command. The replacement keeps parameter IDs and current host values for matching
parameter paths. It rejects incompatible source or state before replacing the
output file. Existing embedded assets remain available; adding new sample files
requires a fresh export from the Muz project.

## State and runtime contract

These details matter when maintaining integrations or reopening and revising
exported devices. Native internals stay inside Muz Instrument/Muz FX; the export
does not bake the whole song or substitute native DAW devices. Unsupported or
unverified semantics receive object-specific report entries.

### State identity and compatibility

Native plugin state version 3 stores a standalone generated Muz device expression,
the evaluated device, role, parameter identities/current values, and embedded
sample bytes identified by SHA-256. The generated expression is authoritative
for source replacement; load checks that it reconstructs the stored device.
Unrecognized versions and mismatched source or asset digests are rejected.
Parameter IDs are stable for each persistent path and checked for collisions;
matching paths retain their host values when source is replaced. Renaming a path
creates a new control identity. Revisions to the plugin state format require an
explicit compatibility update; old state is never silently recompiled under a
new engine version.

Plugin state entry names include the full SHA-256 digest of the plugin identity
and state bytes, plus an instance number. Re-exporting changed state therefore
uses a different archive path, avoiding stale state cached by DAWs under an
earlier export's path.

### Processing and MIDI input

The CLAP wrapper reuses native DSP, applies events at sample offsets, and exposes
latency and tail information. State compilation, asset decoding and processor
construction occur outside processing. Invalid processing inputs and capacity
failures report processing errors; Rust panics are contained at exported ABI
callbacks. Muz FX has fixed Main and Detector stereo inputs; an unused Detector
is ignored. Host transport cannot reconstruct arbitrary prior effect history.
Transport/fidelity diagnostics therefore remain relevant when seeking or looping.
`SONG_TAIL_MANUAL_SETUP` requires extending the DAW export range by the reported
tail duration; the archive does not set that export range.

Muz Instrument accepts CLAP notes and MIDI note events. MIDI note-on, note-off,
note-on with zero velocity, and channel CC messages become native device events
at their sample offsets. Overlapping MIDI notes on the same channel and key
release one voice per note-off, oldest first. Other MIDI messages remain raw;
native Muz instruments do not interpret them.

### Format baselines

The format baseline is DAWProject 1.0 at revision
`ee4dcdde75940f30e14e55401a26955a58b8322b`; the CLAP API baseline is
`a47f6badb49d948fd009998f28309cdab78979c9`.

## Next steps

Read [validation evidence](dawproject-validation.md) to assess the tested host
behavior. Use [native renders](workflow.md#rendering-and-delivery) when the report
identifies a handoff limitation that matters to your piece.
