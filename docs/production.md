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
buses, or master.

Changing `kick.out` (the track's `gain`, including automation) changes its audible
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
Shapes are `linear`, `smooth`, and `step`. Points must be nonnegative and strictly
increasing. One lane owns each target. Lookahead and other latency controls
require a prepared source edit.

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
