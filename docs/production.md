# Production: routing, effects, and automation

Connect instruments to effects, buses, and the master output. This guide assumes
you can build a [song and tracks](language.md#songs-and-tracks). For instrument
setup, see [instruments](instruments.md); for audio exports, see
[workflow](workflow.md#rendering-and-delivery).

[Documentation index](README.md) · Offline: `muz docs production`

## Contents

- [Signal flow](#signal-flow)
- [Native effects](#native-effects)
- [Sidechains](#sidechains)
- [Parallel racks and modulation](#parallel-racks-and-modulation)
- [Automation](#automation)
- [Idle chains](#idle-chains)
- [Resource limits](#resource-limits)

## Signal flow

A track connects a pattern to an instrument. Its `chain` processes the instrument
output in order; pan follows the inserts. The output fader (`gain`, in dB) feeds
the track's output bus. Buses also have a chain, output, and sends. `master` is the
final chain.

```text
instrument → inserts → pan ─┬→ output fader ─┬→ output bus → master
                           │                └→ post-fader sends
                           ├→ pre-fader sends
                           └→ track tap / sidechain detector
```

Here is a complete native-instrument example:

```muz
song({
    tempo: 120,
    tracks: [track("lead", phrase("C4:q E4:q G4:h"), synth("glass-lead"), {
        gain: -3,
        pan: 0.1,
        chain: [fx("eq", {frequency_hz: 2400, gain_db: 2, q: 0.7})],
        sends: {hall: -15}
    })],
    buses: [bus("hall", [fx("reverb", {mix: 1})])],
    tail: 3
})
```

`bus(name, chain, options)` takes `output`, `gain`, `pan` and `sends`, with the
same meaning as on a track: `bus("gtr_l", chain, {pan: -0.8})` places a stereo
pan device after the bus chain. Unknown bus fields are errors.

Sends follow the output fader by default: `sends:{hall:-18}`. Use
`sends:{hall:{gain:-18,pre:true}}` to tap before that fader. Both taps include
inserts and pan. Output-fader automation therefore changes post-fader sends.
A solo audition keeps muted sidechain sources processing.

Bus feedback loops and sidechain dependency cycles are errors. For intentional
feedback, use a delay processor's bounded internal feedback.

## Native effects

The engine supplies EQ, lowpass, highpass, compressor, limiter, delay, reverb,
chorus, gate, bitcrusher, drive, gain, and stereo effects. Inspect a device for
its current controls and ranges:

```sh
muz devices list
muz devices inspect eq
```

Parameters that choose among integer options list a `values` array of
`{value, name}` pairs in the inspect output. Sessions and source still use the
number; the names only document it.

### EQ band shape

The `eq` device is one band with `frequency_hz`, `gain_db`, `q` and `mode`:

| `mode` | Name | Behavior |
| --- | --- | --- |
| 0 (default) | `peak` | Bell centered on `frequency_hz`; `q` sets its width |
| 1 | `low_shelf` | Boosts or cuts everything below `frequency_hz` |
| 2 | `high_shelf` | Boosts or cuts everything above `frequency_hz` |

For shelves `gain_db` is the shelf height and `frequency_hz` the midpoint of the
transition; `q` sets how steep the transition is.
Chain several `eq` inserts for several bands:

```muz
// Add air and take some weight off the low end of a bus.
let tone = [
  fx("eq", {frequency_hz: 9000, gain_db: 2, q: 0.7, mode: 2}),
  fx("eq", {frequency_hz: 120, gain_db: -1.5, q: 0.7, mode: 1}),
];
```

### Limiter ceiling and true peak

`limiter` has `ceiling_db` (−24–0, default −1), `lookahead_ms` (0–20, default
0) and `release_ms` (1–2000, default 100). By default it holds sample peaks:
every output sample stays at or under the ceiling, through gain reduction plus a
final clamp. Peaks between samples are not detected, and the clamp itself makes
new ones, so a limited master with high-frequency content can measure above the
ceiling in dBTP. With `ceiling_db:-3`, a master with a bright ride measured
−3.00 dBFS sample peak and −1.9 dBTP.

`true_peak:true` limits the estimated true peak instead:

```muz
fx("limiter", {ceiling_db: -2.5, lookahead_ms: 5, true_peak: true})
```

The switch changes the detector, not the gain envelope. With lookahead, the
default mode holds the gain each peak needs for `lookahead_ms`, then releases
it exponentially with `release_ms`; the applied gain follows that target with a
0.5 ms attack and a `release_ms` release, and a final per-sample limit catches
any peak the envelope has not fully met. True-peak mode uses the same hold,
release, attack, and time constants, and differs only in three ways:

- The detector oversamples 4x (three interpolated points per sample, as in
  ITU-R BS.1770-4 Annex 2), and each sample's peak covers the intervals on both
  of its sides.
- Every peak in the hold window holds the gain, not only a new lowest one, so
  a slightly lesser peak late in a hold is met by the envelope rather than by
  a one-sample dip at the final limit, which would create new inter-sample
  peaks.
- The final limit uses the 4x peak at the output sample.

The true-peak gain is therefore never higher than the default gain at the same
settings. Switching the mode on can only make a master quieter. Where peaks
between samples barely exceed the sample peaks, the two modes differ by under
0.005 dB RMS, so the switch then only adds latency. In a synthetic test with
dense, bright material (detuned saws and noise hits driven about 6 dB over a
−3 dB ceiling), true-peak mode was 0.6–0.9 dB lower in RMS. That test used
`lookahead_ms` 2–5 and `release_ms` 50–200. The amount depends on how much
of the material's peak energy falls between samples.

True-peak mode looks ahead at least 1 ms, even with `lookahead_ms:0`, because
gain that steps down on the peak sample itself creates new inter-sample peaks.
It then behaves like the default mode at `lookahead_ms:1`, not like the
default mode's no-lookahead limiter, which drops gain instantly. The
interpolator adds 8 samples, so latency is `max(lookahead, 1 ms)` plus 8
samples. The engine compensates latency like any other device's. `true_peak`
is a structural switch (0 or 1 in sessions and `muz devices inspect limiter`),
not an automatable control. It is off by default, and renders without it are
unchanged.

What is assured: the limiter's own 4x estimate of its output stays at or under
`ceiling_db`. `muz analyze` reports the same estimate per window. Its
whole-file `true_peak_dbtp` comes from a separate BS.1770 meter, which agreed
with the estimate to within 0.1 dB on tones up to 19 kHz at 48 kHz; nearer
Nyquist the two interpolation filters disagree by up to about 0.7 dB in either
direction. Other meters, such as ffmpeg's `ebur128=peak=true`, use their own
filters and have not been compared. The true continuous peak can be higher
than any 4x reading, because a peak can fall between the four points: by up to
about 0.2 dB for content at a quarter of the sample rate, 0.5 dB at 20 kHz and
0.7 dB at Nyquist. In synthetic tests with dense 11–22 kHz bursts and a −3 dB
ceiling, the default mode measured −2.5 dBTP (4x) while the true-peak mode
measured −3.00 dBTP (4x) and −2.57 dB against a 32x reference. To meet a
delivery limit with margin, set the ceiling 0.5 dB below it, or less when the
master has little energy above 16 kHz.

### Delay timing

Native `delay` accepts `time_ms` for fixed clock timing, for example
`fx("delay", {time_ms:11.7, feedback:0.2, mix:0.15})`. Positive values up to
48000 ms select clock timing and remain fixed across tempo changes. Durations
round to the nearest sample, with a minimum of one sample. `time_ms:0` (the
default) selects `time_beats`, whose range is 0.03125–16 beats and default
is 0.5. A positive `time_ms` takes precedence when both controls are present;
setting it back to zero also switches live playback/automation back to beat timing.
Both modes share a 48-second buffer; changing modes allocates no memory
in the audio callback. Short body resonances and doubling can be composed with
ordinary inserts, racks or sends:

```muz
fn body(ms) = fx("delay", {time_ms:ms, feedback:0.15, mix:0.12});
let resonances = [body(11.7), body(17.3)];
// Use {chain:resonances} on a track, regardless of the song's tempo.
```

### Reverb rooms

Native `reverb` is a mono-in, stereo-out room: an optional set of early
reflections, then a diffuse tail. `predelay_ms` (0–200, default 20) delays both.
The tail has `decay_s` (0.1–15, default 2), `damping_hz` (500–20000, default
6000, a lowpass in the feedback that also darkens the reflections) and `mix`
(0–1, default 0.25, the wet share). The input is highpassed at 180 Hz.

`early` (0–1, default 0) is the level of six discrete reflections a side,
falling in level, at different times on the left and right. `early_ms` (5–100,
default 40) is when the last one arrives, counted from the pre-delay, so it sets
the apparent size: 10–25 ms is a small room, 60–100 ms a hall. At `early: 1` the
reflections together carry about the input's energy. The default `early: 0`
leaves the room as it was before these controls existed. A small, close, damp
room:

```muz
fx("reverb", {predelay_ms: 18, early: 0.8, early_ms: 18, decay_s: 1.2, damping_hz: 4000, mix: 0.25})
```

### Amplitude quantization and sample-rate reduction

`fx("bitcrusher",{bits:10,rate_hz:16000,mix:0.5})` combines two independent
operations. `bits` ranges from 0 to 24 (default 0, quantization off); positive
values round to amplitude steps of `2^(1-bits)`, referenced to full scale ±1.
Fractional values allow gradual resolution changes. Quantization does not clip
signals above full scale or add dither. `rate_hz` ranges from 0 to 192000
(default 0, sample-and-hold off); rates at or above the engine rate also capture
each sample. Lower rates capture both stereo channels on one shared clock and
hold their individual values until the next capture. Noninteger ratios alternate
capture intervals deterministically, independent of render block size.

`mix` is 0–1 (default 1); 0 is exact dry bypass while the internal clock continues.
Parameter updates preserve hold state and normalized clock position; transport
reset clears it and captures the first new sample immediately. This zero-latency
effect intentionally adds quantization noise and aliasing, with no implicit
filtering or console emulation. Put an explicit lowpass before/after it to shape
the result; choose resolution and filtering per sound in source.

## Sidechains

A sidechain uses one track's audio to control another processor. For example,
this insert fragment uses the physical kick lane as a compressor detector:

```muz
fx("compressor", {
    id: "duck", sidechain: "drums.kick", threshold_db: -25,
    ratio: 5, attack_ms: 3, release_ms: 130
})
```

An external `sidechain:"kick"` reads the source track **after all inserts and
track pan, before its output fader, sends, and downstream bus/master processing**.
This is the same signal boundary as `render --tap kick` or a dry track stem;
"dry" here still includes inserts and pan. The detector is latency-aligned with
the receiving processor. A sidechain names a physical track (`kit.voice` for a
kit lane), and the same tap feeds compressors and rack followers on tracks,
buses, or master. A kit expands only the voices its part plays, except that a
declared voice named as a sidechain source always exists: while the part rests
or does not play it, `drums.kick` is a silent track and the duck reads silence.

Changing `kick.out` (the track's `gain`, or a lane that
[replaces it](#a-lane-replaces-the-static-value)) changes its audible
output and post-fader sends, but leaves the detector unchanged. Instrument gain,
insert effects/gain, and pan affect both the detector and routed audio. The
compressor and rack follower detect `max(abs(left), abs(right))`, then apply
their attack/release smoothing: pan can therefore change detector level even
when stereo power is preserved. Soloing the receiving track keeps the sidechain
source processing while muting that source's output and sends.

Use an ordinary gain insert to set detector level, and the output fader to set
audible level. For example:

```muz
track("kick", phrase("C2:q").repeat(12), synth("kick"), {
    chain: [fx("gain", {id:"key_level", gain_db:0})],
    gain: -12
})
```

Lowering `kick.out` to −30 dB makes the kick quieter with the same ducking.
Setting `kick.key_level.gain_db` to −12 dB and `kick.out` to 0 dB instead keeps
the original audible kick level while reducing detector level by 12 dB. This
compensation assumes no other gain-dependent processing between the insert and
output; sends still obey their own pre/post-fader rules. The runnable
[sidechain level example](../examples/sidechain-levels.muz) automates these three
states over a held bass. Render `--tap kick` to measure the detector source,
`--tap bass` to measure ducking, or `--solo bass` to hear it through production.

## Parallel racks and modulation

A rack runs serial effect branches in parallel, aligns their latencies, sums them,
and mixes the result with an equally delayed dry input. This fragment uses two
branches:

```muz
rack([
    [fx("lowpass", {cutoff_hz: 800}), fx("gain", {gain_db: -6})],
    [fx("highpass", {cutoff_hz: 800}), fx("gain", {gain_db: -6})]
], {id: "parallel", mix: 0.4, gain_db: 0})
```

An empty branch is a dry path. Set branch gains explicitly when summing several
full-level branches. A rack has at most eight branches and 32 devices. Build
reusable racks with ordinary functions and compose branch arrays in source.

`expose:{tone:"0.0.cutoff_hz"},tone:1200` exposes a control by branch index,
device index, and parameter name. On a track named `lead` with rack ID `parallel`,
that control is addressed as `lead.parallel.tone`.

An automation lane can also name a device inside a branch by its id, as it names
a device in the track's own chain. With `fx("chorus", {id: "mid_chorus"})` in a
rack branch on track `bass`, the lane target `bass.mid_chorus.mix` automates that
chorus. The full device id, `<rack id>.<branch>.<id>`, such as
`bass.fx1.1.mid_chorus.mix`, also works and is required when another device of
the track shares the id (unnamed devices are `fxN` in every branch). Compile
lowers such a lane to a rack exposure named `<branch>.<id>.<parameter>`. Only
realtime controls of native effects qualify, and a control a modulator drives
cannot also take a lane. An unknown target's help lists these names.

Rack `modulate` entries name an internal target and calculate:

```text
base + depth*sin(2π*rate_hz*t) + follower*envelope
```

The result is clamped to explicit `min`/`max`. The envelope follows the rack input
or its external `sidechain` track, using `attack_ms`/`release_ms`. Exposed setters
and modulators cannot compete for one target. Latency-changing controls cannot
be exposed or modulated this way.

See [std/mix](../std/mix.muz) and the [rack example](../examples/racks.muz)
for tremolo, envelope filters, ducking, gated spaces, and split-band processing.

## Automation

An automation lane connects a target to a curve. Device targets use explicit IDs
and parameter names, such as `lead.instrument.cutoff_hz`. Route targets include
`lead.out` and `lead.send.echo`. Put lanes in the song's `automation` list:

```muz
automation("lead.instrument.cutoff_hz", curve([[0b, 500], [16b, 3500]], "smooth"))
```

Curve positions use beats or seconds; values use the target parameter's units.
A lane on a plugin parameter sets its global value. To shape a parameter per
note, so overlapping notes move independently, give a CLAP instrument
[note controls](performance.md#per-note-plugin-parameters) instead.
Shapes are `linear`, `smooth`, and `step`. Points must be nonnegative and strictly
increasing. One lane owns each target. Lookahead and other latency controls
require a prepared source edit.

### A lane replaces the static value

A lane sets its target for the whole song: before its first point it holds the
first value, and after its last point it holds the last value. The static value
of the same target never sounds and is not an offset. This applies to every
target: a track's, kit voice's or bus's `gain` under its `.out` lane, a send's
level under its `.send.NAME` lane, and a device parameter such as `cutoff_hz`
under its lane.

When a track, kit voice or bus writes `gain` explicitly and a lane targets its
`.out`, `muz check` warns with `automation.replaces_gain`, naming the dead value.
Leaving `gain` out does not warn. A kit voice's dead value is the kit track's
`gain` plus the voice's own. Send levels and device parameters under a lane do
not warn. A send must be declared with a level to exist, so a parked level such
as `sends:{echo:-120}` under a throw lane is the ordinary idiom. Likewise, a
device's static parameter next to its lane is a common way to write the
device's resting setting.

To combine a trim with a ride, as on a console, add the trim to the ride's
values in source and drop `gain`:

```muz
let trim = -6.5;
let ride = [[0b, 0], [32b, -3], [48b, 2]];
automation("guitar.out", curve(ride.map(fn(p) => [p[0], p[1] + trim]), "step"))
```

Mapping the points keeps the ride's knots and shape. `curve_map(curve, fn(x) =>
x + trim)` also works on a curve value, but it resamples the curve as described
below.

Curve sampling preserves value dimensions: interpolation between `1kHz` and
`2000Hz` returns Hz. All values must have compatible dimensions; beat/bar values
normalize to beats. Native parameter descriptors declare units and whether a
change updates a control or requires structural preparation.

### Curves and combinations

`lfo(period,duration,low=0,high=1,phase=0)` constructs a cosine curve with 64 points
per cycle. `curve_at(curve,offset)` places a local envelope. `curve_map(curve,fn(x)=>...)`,
`curve_add(a,b)`, and `curve_mul(a,b)` combine curves before audio processing.
Their default sampling resolution is 1/64 beat, or 1/64 second for clock curves;
supply `resolution` for sharper shapes. Playback interpolates the prepared
points at audio sample positions.

`curve_value(curve,position)` evaluates one position or a list of positions.
`unit(quantity)` returns one in its dimension. Source curve-combination helpers
retain original knots and guard points at step edges. Their sampling policy can
be changed in source without changing the interpolation kernel.

### Kit insert automation

An insert in a kit track's `chain` can use the logical track ID. Compilation
broadcasts its lane to every voice with hits in that track:

```muz
song({
    tracks: [track("drums", drums({kick:"X...", hat:"x.x."}), kit(), {
        chain: [fx("lowpass", {id:"tone", cutoff_hz:16000})]
    })],
    automation: [automation("drums.tone.cutoff_hz", curve([[0b,500],[4b,16000]]))]
})
```

Each voice keeps its own insert, so the sweep also affects its sends. This does
not create a shared bus processor. Voice-specific inserts stay independently
addressable, for example `drums.kick.tone.cutoff_hz`. An unnamed track insert uses
its logical chain index (`drums.fx0.cutoff_hz`), even when voice-specific inserts
precede its physical copy. `muz inspect song.muz --view automation` shows bounded
summaries of the expanded physical lanes; `--view automation_points --track TARGET`
pages one lane's points. A logical lane and a physical lane cannot own the same
target; merge the curves or use separate physical lanes. A logical insert lane
on a kit with no hits is an error.

### Deriving automation from notes

Composers derive automation from musical data using ordinary source functions.
`pattern.select("tag:answer").notes` yields note records that can be filtered,
sorted and folded into a `curve`, then passed to `automation(target,curve)`.
The target can be send gain, a bus level, filter cutoff, effect mix, or any other
supported parameter. Tags supply a selection; the function decides the shape,
values and overlap policy. For example, this custom function raises a send from
−120 dB to a velocity-dependent amount at each tagged attack:

```muz
fn accents(material, timing) {
    let notes = sort_by(material.select("answer").notes, fn(n) => n.at);
    let points = fold(notes, [[0s, -120]], fn(ps, n) => ps + [
        [seconds_at(n.at, timing) + n.offset, -24 + n.velocity * 12],
        [seconds_at(n.at, timing) + n.offset + 100ms, -120]
    ]);
    automation("lead.send.echo", curve(points, "step"))
}
```

This particular recipe assumes attacks after zero with more than 100 ms between
them. For overlapping gestures, choose how to combine them in source before
submitting one lane per target. Points must be nonnegative and strictly increasing.
Use beats for score-aligned curves or `seconds_at` and note offsets for performed
timing; share the song's tempo settings as described in
[score and performed time](performance.md#score-and-performed-time).
Generated curves remain visible with `muz inspect song.muz --view automation_points --track TARGET`.

`std/mix.muz` includes editable examples: `note_start`/`note_end` calculate performed
times, `gate_windows` merges constant-level windows, and `mix.throws(material,
selector,target,level=-12,tail=120ms,timing={})` returns an ordinary automation lane
using those helpers. Put it in the song's `automation` list. Its send-volume recipe
returns to −120 dB after key release plus tail, and empty selections leave the
send closed. Its policy is editable in `.muz`.
The automated send carries the track's sounding audio; the receiving effect keeps
processing its tails after the send closes. [The example](../examples/tagged-send.muz)
combines this recipe with a composer-written velocity-shaped filter gesture.

### Passage and arrangement gestures

`std/arrange` evaluates passage gestures after final placement with the full tempo
map. Its `build(form,settings,gestures=[])` also accepts functions over the whole
arrangement context. Use that wider scope when a policy spans passage boundaries:
for example, derive send windows from all placed lead notes and union them once
with `std/mix.throws`. A tail may then cross into the next passage without being
clipped or competing with a second lane. Local and whole-arrangement callbacks
receive the same fields: `name`, `start`, `span`, `parts`, and `timing`.

`mix.throws(...,start=0s)` and `mix.gate_windows(...,start=0s)` accept an explicit
start for a local automation fragment. Joining fragments requires compatible
nonoverlapping curves; overlapping policies belong in an explicit source
combination.

## Idle chains

Every device of a track processes every block of the song, whether or not the
track is playing. Two things can stop that.

**Plugins that say they are done.** A CLAP plugin reports after each block
whether it needs to keep running. muz honours that report: a plugin that
returns "sleep", returns "continue if not quiet" with an all-zero block, or
returns "tail" and has run out its declared tail length, is not processed again
until a note, controller, parameter change, transport start or stop, or nonzero
input sample reaches it. Its output is silence meanwhile. This needs no source
and does not change what a well-behaved plugin produces. The native devices and
VST3 plugins always process.

**A bound you declare.** Amp simulators and similar plugins usually keep a
noise floor running and never report that they are done. `idle_after` on a
track gives its chain a bound:

```muz
track("guitar_heavy", riff, plugin("di-guitar"), {
    chain: [plugin("amp"), plugin("cab")],
    idle_after: 2s
})
```

Each device of the chain, the instrument and every insert, then stops once its
own input has been digitally silent for that long with no event and no held
note, and outputs silence until a note or a nonzero input sample arrives. The
instrument stops two seconds after its last note-off; each insert stops two
seconds after the device before it went silent, so a reverb at the end still
gets that long to ring.

This changes the audio, which is why it is never the default: a noise floor
disappears and any release or effect tail longer than the bound is cut. Choose
a bound longer than the tails you want to keep. A stopped device is frozen
rather than reset. It resumes from the state it had, as if the skipped time had
not passed: a free-running LFO continues from where it stopped, and a tail that
was cut can sound when the device wakes. Idleness is decided per processing
block (`--block-size`) from the device's input alone, so a render is identical
for every `--threads` value. An external sidechain key does not keep a device
awake, and bus inserts are not affected.

## Resource limits

Audio blocks are bounded to 1024 frames, with 256 scheduled note/control events
per physical track per block. Dense expression can reach that event budget; use
a smaller block size or fewer simultaneous controls. Native synths and voice
patches have 16 voices, and samplers have 32; they steal voices when necessary.
Piano feasibility checks are independent of these playback limits.

### Graph preparation budget

`muz check` reports expanded physical tracks, devices, buses (including master),
routes, sample zones, total resource units and the five largest contributing lanes/buses. JSON
output exposes this under `graph` and `graph_budget`. Kit expansion is included.
There are no separate 32-track, 128-device, 15-bus, 128-route or 128-zone limits.

The process-wide `MUZ_GRAPH_BUDGET` defaults to 4096 units. Each graph device, bus,
route or sample zone costs one unit; a track contributes its instrument, inserts,
routes and zones. Zone preparation is counted across the expanded graph, including
every kit voice and reused instrument instance. One sampler with 200 zones on a
single track contributes 202 units (instrument, output route, zones), plus one
for the master bus. Its zones can be generated with ordinary source operations:

```muz
use "std/arrange" as arrange;
let zones = arrange.flatten(map(range(25), fn(i) => map(range(8), fn(take) => {
    path:format("recordings/root{}_take{}.wav", [48+i, take]),
    root:48+i, keys:[48+i,48+i]
})));
let guitar = sample(zones, {attack_ms:2, release_ms:35});
```

Maps must contain at least one zone. Preparation fails with the required and
allowed total plus contributing lanes, before sample file metadata or audio is read.
Set the budget before starting muz, for example `MUZ_GRAPH_BUDGET=8192 muz check song.muz`.
It accepts integers from 1 through 65536 and stays fixed for the process lifetime.
The ceiling is deliberate: the budget value itself, whatever the graph uses of it,
sizes each live stream's per-device telemetry tables and bounds reader zone counts.
A failed preflight reports required/allowed units, expanded counts and contributors.
This counts graph and zone preparation, not CPU or decoded-sample memory. The
sampler's independent cap of 64 × 1024 × 1024 decoded stereo frames
per instrument remains; plugin, rack and per-block event budgets also still apply.

Engine and transaction vectors are sized from the graph during preparation. Live
telemetry and its scratch space are allocated from the process budget before
playback, so edits can grow the graph within that budget without allocating in the
audio callback. A larger budget permits larger preparations; shared bus processing
remains useful for reducing actual work.

## Next steps

Use [workflow](workflow.md) to inspect automation, render taps, or compare mixes.
See [instruments](instruments.md) for samples and plugins, and
[synthesis](synthesis.md) for native voice graphs.
