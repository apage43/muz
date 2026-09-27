# DAWProject validation evidence

These are observations from the Bitwig Studio 6.0.11 Linux probes begun on
2026-09-26 and their follow-ups. They support the capability statements in
[DAWProject export](dawproject.md); they are historical evidence, not mandatory
delivery checks or a promise about other hosts and versions.

Labels such as `final13` identify successive probe runs. Observations below are
grouped by feature; later runs sometimes resolve a narrower question left open
by an earlier one. Audible response, displayed data, and exact DSP equivalence
are separate claims.

[Documentation index](README.md) · Offline: `muz docs dawproject-validation`

## Contents

- [Basic import and parameter persistence](#basic-import-and-parameter-persistence)
- [Embedded samples](#embedded-samples)
- [Note expression](#note-expression)
- [Sidechains](#sidechains)
- [Groups, sends, buses, and markers](#groups-sends-buses-and-markers)
- [Controller lanes](#controller-lanes)
- [Probe isolation](#probe-isolation)

## Basic import and parameter persistence

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

The combined run did not reconnect the sidechain. It contained
no groups, sections, controller lanes, buses or sends,
so their representation was not exercised. The report still names manual
sidechain setup, unverified rack/expression fidelity and manual tail setup; successful
playback and export do not establish DSP parity or exact tail handling.

In final14, the duplicated lead's Muz Instrument cutoff was changed to
`7494.3 Hz` while the original remained at `1800 Hz`. Saving and reopening
retained both distinct values, and both plugin instances loaded successfully.
This establishes independent parameter state for that duplicate pair.

## Embedded samples

In final15, the combined archive was copied into the private sandbox's home
directory. The sampled track was soloed and the first bar exported offline to
a 24-bit WAV. The result was 48 kHz stereo with 112001 frames, 9592 nonzero PCM
samples, a peak of 917165 in signed 24-bit units and its first nonzero sample at
index 2. Bitwig loaded all six instances without sample or state errors. This
confirms audio from the embedded sample after relocation; it does not establish
pinned-zone equivalence or DSP parity. It supersedes the earlier inconclusive
Solo/Play screenshot, which showed no meter movement in that frame.

## Note expression

In final16, the selected glass note's inspector showed Pitch `0.06` and Timbre
`0.95%`, with a rising line inside the note. The exported XML contained three
pitch and three timbre points within that note. The attempted drag did not move
it: Start remained `1.2.1.00`. The observation establishes visible expression
data, but not expression playback or attachment after a note move.

Final25 moved the glass B3 note with plain Right Arrow from Start `1.2.1` to
`1.2.2`, preserving its key and length. The rising internal line remained visible.
After movement, Inspector showed Pitch `0.00%`, Timbre `0.95%` and Pressure
`0.00%`. This establishes note movement with visible expression retained; it does
not establish unchanged values for every curve point. The archive contains three
pitch and three timbre points, but point-by-point host readback remains
unverified. The later expression A/B below tests an audible effect separately.

A later glass solo comparison tested imported expression against a no-expression
variant. After excluding the first 100 ms, their difference had a peak of 1600481
and RMS of 314919.55 in raw signed 24-bit PCM units. A repeated no-expression
bounce was bit-identical over that same window. This supports an audible effect
from the imported expression, alongside the note-edit observation. It does not
prove each pitch/timbre point was unchanged or establish sample-exact native Muz
expression parity. The CC11 limitation above remains a separate diagnosed outcome.

## Sidechains

In the final18 follow-up, the compressor's header sidechain icon opened the
`Select sidechain input` auxiliary-input panel. Its kick source offered PRE,
POST and Muz Out choices; kick POST was selected and playback was active.
No dedicated Detector meter was observed, and there was no exact detector-tap,
timing or sound comparison. Separately, the native harness verifies detector-driven
gain reduction and exact rack impulse parity with 240 samples of parallel-path
latency. Those harness results do not establish the imported host connection's
audio behavior.

Final19 compared paired Project Master bounces over `1.1.1`–`2.1.1`, both
48 kHz stereo 24-bit with no dither and 112001 frames. With `No input`, peak
was -6.864 dBFS and RMS was -20.633 dBFS; with kick POST selected, peak was
-8.566 dBFS and RMS was -20.918 dBFS. Of 224002 interleaved PCM samples,
157902 differed. This is consistent with a sidechain response through the
selected auxiliary input. It does not establish the exact original Muz detector
tap, timing or DSP parity.

## Groups, sends, buses, and markers

In final17, the structure probe showed a `drums` group with `hat` and `open_hat`
children, a `room` FX track containing Muz and routed to Master, a `keys` send
named `room`, and an `End` marker. That run did not verify the exact send level,
post tap, `Intro`/`Turn` labels or CC64 lane representation/playback.

Final20 imported the structure fixture with the keys-to-room send and room FX
bus, then exported Project Master with the bus enabled. The 2-second WAV was
48 kHz stereo 24-bit with 96000 frames and nonzero PCM (peak 464796 and RMS
68750 in signed 24-bit units). The paired bus-muted condition could not be run:
the isolated Xvfb window was unavailable through the UI controller. The enabled
bounce establishes project audio output, but does not isolate the send/bus
contribution or verify the -12 dB send level or post tap. The real profile,
CLAP directory and Projects manifests matched after correcting the comparator.

Final22 showed green meters on both keys and the room FX track during playback
at `1.2.1.77` / `0:00.597`. Room had no clip, and its idle meter was dark. This
supports audio reaching room through the keys send. It does not measure the
send's exact -12 dB level or establish its post tap.

Final23 showed the keys-to-room send hover in Bitwig's Mixer explicitly reading
`SEND -12.0 dB` and `ENABLE On`. `Intro`, `Turn` and `End` markers were visible.
This verifies the imported send's displayed level and enabled state and the
section labels. The archive encodes the send as `type="post"`; Bitwig's actual
post-fader behavior was not independently verified in that run. The CC64 lane
was not inspected during final23.

Final24 compared a schema-valid variant that changed only keys Volume from `1`
to `0.0001` (-80 dB), leaving the `type="post"` send unchanged. The baseline
showed green keys and room meters at `0:00.597`; the variant showed keys at
-80 dB and a dark room meter at `0:00.575`. This supports post-fader send
response in Bitwig. The meter comparison does not establish sample-exact tap
parity with native Muz.

## Controller lanes

Final26 inspected the existing keys automation lane `Ch. 1 Sustain Pedal (#64)`.
It showed a high hold from the start of bar 1 followed by a step down, matching
the fixture's CC64 values of 127 at beat 0 and 0 at beat 2. No lane was created
and no points were drawn. This verifies imported controller-lane representation;
the audible sustain-pedal effect was not tested.

A subsequent CC11 probe compared two Bitwig bounces differing only in a normalized
`channelController` 11 point, changed from `1` to `0`. The 96000-frame bounces
were byte-identical. A focused CLAP harness test independently delivered MIDI
CC11 high/low events and matched native DSP output sample-for-sample; after
smoothing, low-level energy was below 1% of high-level energy. Together these
results indicate that the imported lane did not produce the expected controller
effect in this Bitwig/Muz path. There was no host callback trace, so this does
not establish that all CC events are absent or that every controller is affected.
Use native Muz playback for guaranteed original controller behavior; imported
lane visibility alone is insufficient evidence of playback fidelity.

## Probe isolation

The probe projects and their follow-up runs used a private Bitwig profile
and temporary activation copies in
an isolated Linux sandbox. The copies were removed, and the real Bitwig profile,
CLAP directory and Projects tree had no mtime-manifest changes after the runs.

Return to the [export guide](dawproject.md) for setup, report remedies, and the
current handoff contract.
