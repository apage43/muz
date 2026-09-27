# DAWProject export (initial implementation)

`muz export` creates an editable DAWProject archive and a fidelity report from one
compiled muz song. The report is part of the handoff: read its warnings before
using the project for production. The current target profile is Bitwig Studio
6.0.11 on Linux. Format baseline: DAWProject 1.0 at revision
`ee4dcdde75940f30e14e55401a26955a58b8322b`; CLAP API baseline:
`a47f6badb49d948fd009998f28309cdab78979c9`.

```sh
cargo build --release --package muz-clap --no-default-features
mkdir -p ~/.clap
cp target/release/libmuz_clap.so ~/.clap/Muz.clap
cargo run -- export song.muz --format dawproject -o song.dawproject
```

The CLAP bundle exposes `com.muz.instrument` and `com.muz.fx`. Restart or rescan
the DAW after installation. Keep the same plugin version available when reopening
an export. `--report PATH` selects the JSON report path; otherwise the exporter
writes a `.dawproject.report.json` file beside the archive. `--strict` rejects an
export with any fidelity warning and leaves an existing archive untouched. The
report remains available on strict rejection.

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

## Current capability boundary

| Feature | Current handoff | Bitwig 6.0.11 observation |
| --- | --- | --- |
| Notes and clips | Performed notes remain editable; pitch, pressure and timbre use note-contained curves | Three-note clip imported; moving G3, duplicating the track, saving/reopening and bouncing succeeded; distinct original/duplicate cutoff values survived reopen |
| Channel controls | CC lanes, including pedals, use channel and controller identity | Import and native playback remain open |
| Native DSP | Muz Instrument and Muz FX carry validated code-backed state, including native serial/parallel FX racks | All six instances loaded in the combined probe; four track meters and Master responded; a sampled-track gain edit survived save/reopen and two offline WAV exports completed; DSP parity and isolated sample playback remain open |
| Arrangement | Tempo points, meter, groups and section markers | The combined probe showed 120→90 BPM during playback; groups and sections were not exercised |
| Mixer | Tracks, buses, master, sends, output routes and volume automation | Simple lead-to-master playback succeeded; buses, sends and automation remain open |
| External plugins | Original CLAP/VST3 identity, effective public values and state where available | ZamEQ2 CLAP loaded with an overridden parameter; VST3 import remains open |
| Sidechains | Muz FX exposes a detector input for supported devices | DAWProject connection, detector tap and timing remain open |

Warnings are attached to affected objects. For example,
`SAMPLE_ZONE_OMITTED` means a pinned sampler zone cannot follow an edited note;
the note remains editable, but exact pinned selection requires playback in native
Muz. Re-exporting does not restore the missing pin in DAWProject.
`SIDECHAIN_MANUAL_SETUP` means the Muz FX detector port exists but the archive
does not connect the declared source to it automatically. `--strict` rejects
these outcomes.

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

Original third-party CLAP and VST3 instances are exported with their identities,
effective parameter values and saved state after source overrides. VST3 state
currently includes the component chunk; controller-private state is specifically
reported as omitted.

The report names every device or feature that this implementation omits or cannot
yet verify. Nested external/sampled racks, exact sidechain connections, some expression slots,
sampler zone pins and raw MIDI messages remain under development. Native FX sound parity,
complex routing, expression and controller playback, and VST3 import remain unverified in Bitwig. A schema-valid
archive alone does not establish audible equivalence. The live
[implementation plan](dawproject-plan.md) records the remaining acceptance gates.

The Bitwig profile is the tested target. On its first preset load, Muz suppresses
CLAP parameter rescans because Bitwig reports an initialization restart error
when a rescan is sent. Bitwig 6.0.11 enumerated the imported controls and played
their automation in the synthetic probe. Other hosts may cache the default
controls before loading state; parameter discovery and automation in those hosts
remain unverified.

Muz Instrument accepts CLAP notes and MIDI note events. MIDI note-on, note-off,
note-on with zero velocity, and channel CC messages become native device events
at their sample offsets. Overlapping MIDI notes on the same channel and key
release one voice per note-off, oldest first. Other MIDI messages remain raw;
native Muz instruments do not interpret them.

If a DAW reports a missing Muz plugin, install the `.clap` library shown above,
rescan CLAP plugins and reopen the project. If notes appear but sound is absent,
check the instance state and the report's `required_plugin_ids` and
`external_dependencies`. For sidechain devices, connect the reported source to
the Muz FX Detector input in the DAW and compare the detector tap and timing with
the original Muz render. Keep the report with the archive when transferring it.

The report's `entries` are sorted by physical path, code and feature. Each entry
contains a stable code, severity, source location when available, logical and
physical paths, affected feature, intended sound or structure, exported behavior,
fidelity outcome and a practical remedy. `required_plugin_ids` identifies the
plugin roles needed to play the handoff. `external_dependencies` names any source
dependencies that remain outside the archive.

## Bitwig probe, 2026-09-26

Bitwig Studio 6.0.11 (revision 160070,
`f2730b10e641fdf2e4ae82140089d5f6550ca3b7`) imported a three-note
synthetic archive whose XML passed the pinned XSDs. The notes appeared at the
intended MIDI keys and beats; Bitwig labels MIDI key 60 as C3. Muz Instrument
restored its state and parameters. At 48 kHz, the lead and master meters moved
during playback. G3 was edited from start `1.3.1.00` to `1.2.1.00`, the track
was duplicated, and the edited project saved and reopened with both changes.
Bitwig also completed an offline WAV export.

A second synthetic archive imported Muz Instrument with cutoff automation and
a ZamEQ2 CLAP insert. Bitwig displayed the overridden ZamEQ2 value `3.0`, the
Muz cutoff lane and its `800 Hz` initial value. During playback the cutoff
read `1030.4 Hz` at 0.127 s, `3003.2 Hz` at 1.237 s and `1200 Hz` at 2.389 s,
following the rising then falling lane. Other device families and routing
cases remain bounded by their report warnings.

A combined synthetic archive contained four note tracks, a native parallel
rack, a sidechain compressor, embedded sampler assets, note expression, smooth
automation, a tempo change and an effect tail. In the final13 run, all four
Muz Instrument and two Muz FX instances loaded. Playback activated meters on
all four tracks and Master, and the displayed tempo changed from 120 to 90 BPM.
The sampled track's `gain_db` was changed from `0` to `24`; that value survived
saving and reopening the combined project. Two offline 24-bit WAV exports
completed.

The archive was opened from a relocated path and contained embedded sample
state, but isolated playback of the sampled track was not established. A later
Solo/Play screenshot showed no meter movement in that frame and does not confirm
sampled isolation. The combined run did not reconnect the sidechain. It contained
no groups, sections, controller lanes, buses or sends,
so their representation was not exercised. The report still names manual
sidechain setup and unverified rack, expression and tail behavior; successful
playback and export do not establish DSP parity or exact tail handling.

In final14, the duplicated lead's Muz Instrument cutoff was changed to
`7494.3 Hz` while the original remained at `1800 Hz`. Saving and reopening
retained both distinct values, and both plugin instances loaded successfully.
This establishes independent parameter state for that duplicate pair.

All three probe projects and their follow-up runs used a private Bitwig profile
and temporary activation copies in
an isolated Linux sandbox. The copies were removed, and the real Bitwig profile,
CLAP directory and Projects tree had no mtime-manifest changes after the runs.
