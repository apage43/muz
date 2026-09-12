# Native voice patches

Start with `synth("pulse-bass")`, `synth("bell")` or another preset. Use a `voice_patch` when its actual signal flow matters. Patches are ordinary values, so functions, imports and arrays provide abstraction. See `examples/voice-expression.muz`.

```
voice_patch("simple", {
    gain_db:-16,
    nodes:[
        {id:"cutoff",op:"param",value:2400,min:80,max:10000},
        {id:"osc",op:"osc",wave:"saw",detune:3},
        {id:"filter",op:"filter",input:"osc",cutoff:"cutoff",q:0.7},
        {id:"env",op:"adsr",attack:0.01,decay:0.2,sustain:0.6,release:0.3},
        {id:"out",op:"mul",inputs:["filter","env"]}
    ],output:"out"
})
```

Nodes appear after their dependencies. A signal input is a number or an earlier node ID. Every internal signal runs at audio rate. `param` nodes cross the device boundary as named controls; `automation("lead.instrument.cutoff",curve(...))` drives them. Note expressions are a separate control input, addressed by sounding-note identity and sampled every 128 frames. Plain parameters are held between changes; expression points interpolate on that fixed clock. No per-sample language closures or runtime compilation occur.

| Operation | Inputs |
| --- | --- |
| `param` | `value`, `min`, `max`; node ID becomes the parameter name |
| `frequency`, `velocity` | Current voice's frequency in Hz / attack intensity |
| `expression` | `kind`: volume, pan, tuning, vibrato, expression, brightness, pressure |
| `osc` | `wave`: sine/saw/pulse/triangle; `ratio`, `detune` in cents, optional `hz`, `fm` in Hz, pulse `width` |
| `noise` | Deterministic bipolar white noise |
| `adsr` | `attack`, `decay`, `release` in seconds; `sustain` 0..1; optional `one_shot:true` |
| `sum`, `mul` | `inputs` array of signals |
| `drive` | `input`, linear drive `amount`; tanh saturation |
| `filter` | `input`, `cutoff` Hz, `q`; `mode`: lowpass/highpass |
| `delay` | `input`, `seconds`, `feedback`; fixed `max_seconds` allocation |
| `sample` | WAV/FLAC `path`, `root` pitch, optional `loop:true`; mono voice reader |

Each patch has 16 voices, at most 64 nodes, at most 16 inputs to a sum/product, and at most two seconds of delay memory per voice. Each delay allows up to one second, with feedback clamped inside ±0.98. Sample readers share at most eight million decoded frames per patch. Dynamic frequency/filter/envelope inputs have bounded ranges. The output must be named explicitly. A graph is acyclic; feedback lives inside delay nodes. Cycles/forward references and unknown fields fail during preparation.

Volume, expression, pan and tuning apply to the voice automatically. Brightness, vibrato and pressure are available to wire into the desired graph inputs. A patch containing ADSR nodes retires released voices when all its envelopes finish; when all ADSRs use `one_shot:true`, it also retires held voices after all envelopes finish; otherwise a short default release applies. Put delays before the final envelope when you want their sound gated with the voice; use track effects for tails that should outlive voice retirement. Voice stealing uses the quietest current voice.

Fractional note pitch and attack intensity stay precise through native rendering. CLAP native-note ports receive tuning/expression; MIDI export quantizes notes and does not encode these per-note curves. Native sample resampling and nonlinear saturation are intentionally modest first implementations, not oversampled mastering processors.

Voice-graph connections retain their units: a `frequency` node or a `param` carrying 2400 Hz is not an audio amplitude. Each consuming oscillator/filter/envelope bounds its own controls; non-finite intermediate values are replaced by zero. Earlier builds incorrectly clipped every connection to ±100, corrupting frequency and cutoff signals. The focused regression compares literal and connected Hz controls.

The preset synth also exposes `mode` (`"saw"`, `"pulse"`, `"fm"`, `"kick"`, `"snare"`, `"cymbal"`, `"fm_percussion"`, or `"noise"`), `filter_env` in octaves, `resonance`, `sub`, `unison` (1–5), `detune_cents`, `width`, `fm_ratio`, `fm_index`, `drive_db`, and delayed `vibrato_cents`/`vibrato_hz`. Override these on `synth("init", {...})` or an existing preset. `muz devices inspect studio_synth` lists parameter ranges. Pitched/noise modes use gate-controlled envelopes; percussion modes finish their decays after note-off and respond to choke.

Preset synths accept per-note `volume` (0–4), `expression` (0–1), `pan`
(0–1, center 0.5), and `tuning` (semitones, −120–120), using the same
`.express({...})` curves and 128-frame control clock as voice patches. Volume
and expression multiply each voice after synthesis; tuning affects its pitched
oscillators, including the sub oscillator. Pan balances the existing stereo
voice with square-root gains, preserving its sound at center. The last control
value before note-off remains in force during release. Instrument gain and
CC 11 remain shared controls. Brightness, pressure and vibrato expression need
an explicit voice graph mapping or CLAP; presets reject them instead of ignoring
them. See [independent preset swells](../examples/preset-expression.muz).

These controls reuse native note events and voice state. Translating all pitched
presets into current mono voice graphs would change their stereo unison,
oscillator phases and documented envelope response; native expression preserves
those existing instruments without adding a graph operation or musical policy.

Samplers accept the same per-note volume, expression, pan and tuning controls,
including instruments built from zone lists. Volume and expression multiply each
sample voice independently; pan balances its existing stereo channels with the
same square-root gains as presets, preserving the recording at center. Tuning
changes playback speed relative to the selected zone's calibrated root, without
selecting a different zone or restarting playback. Curves use the existing
128-frame clock and retain their last scheduled value through release or the
remainder of a one-shot sample. Brightness, pressure and vibrato require a voice
patch mapping or CLAP; unsupported sampler expression reports the track and
instrument kind. Instrument gain remains shared.

With a local recording at `tone.wav`, independent bow/breath swells use ordinary
source values, even when a new note overlaps the previous release tail:

```muz
let swell = {volume:[[0,0.7],[0.3,1],[1,0.6]]};
song({tracks:[track("strings",stack([
    phrase("C4:q").gate(1).express(swell),
    phrase("E4:q").gate(1).at(1b).express(swell)
]),sample([{path:"tone.wav",root:60}],{release_ms:300}))],tail:0.3})
```

This reuses note-addressed events and sampler voice state. Envelope shapes and
musical choices remain in source; no sampler-specific expression builtin is needed.

In the preset synth, `width` controls **stereo spread of unison voices**; its pulse
oscillator has a fixed 50% duty cycle. In a voice-patch `osc` node, `width` instead
controls **pulse duty cycle**. Use an explicit pulse node for 25% or 12.5% shapes.

## Envelope timing

Graph `adsr` and the preset synth use different response conventions. For fixed
parameters, let `t` be seconds since note-on, `A` attack time, `D` decay time,
`S` sustain level, and `R` release time. Graph times are already in seconds;
divide the preset's `attack_ms`, `decay_ms` and `release_ms` by 1000 first.
These formulas describe the envelope amplitude before velocity, gain, and other
signal processing:

| Stage | Graph `adsr` | Preset `saw`, `pulse`, `fm`, `noise` |
| --- | --- | --- |
| Held, `t < A` | `t / A` | `t / max(A, 1/sample_rate)` |
| Held, `t >= A` | `S + (1-S) * exp(-5*(t-A)/D)` | `S + (1-S) * exp(-(t-A)/D)` |
| Released, after `m` samples | `E0 * exp(-9.21*m/(R*sample_rate))` | `E0 * exp(-6.9078*m/(R*sample_rate))` |

Attack is linear; zero attack starts at 1 on the first sample. After one decay
time **following the attack**, the graph retains about 0.674% of the distance
from peak to sustain (−43.43 dB), while the preset retains about 36.8%
(−8.69 dB). Those dB values describe the residual above sustain, not the total
level when `S > 0`. Sustain is an amplitude level, approached asymptotically
while the key is held; decay has no hard endpoint. With `S = 0`, a held note
keeps decaying toward silence.

Note-off, including a short `.gate(...)`, starts release from the **last computed
envelope level** `E0`, even during attack or decay. The note-off sample is the
first multiplication (`m = 1`). One release time reduces that level by about
80 dB in the graph and 60 dB in the preset; it is not a fixed time to silence.
A released graph voice retires once **all** its ADSR levels are below `0.00001`;
a released preset voice retires below the same envelope threshold (−100 dB
relative to unity). From `E0 = 1`, this takes about `1.25*R` and `1.67*R`
respectively. Held graph and pitched/noise preset voices do not retire solely
because their envelope has decayed below that threshold. Rendering also needs
enough song `tail` to include the desired release.

Graph attack is clamped to 0–30 seconds, decay to 0.0001–60 seconds, release to 0.0001–30 seconds,
and sustain to 0–1. Signal-connected parameters are read each sample: held
attack/decay is recomputed from note age, while release multiplies the previous
level using the current release input. The formulas above assume those inputs
stay constant. Preset ranges are listed by `muz devices inspect studio_synth`.

To translate a pitched/noise preset envelope into a graph envelope, keep attack
and sustain, use `graph_decay = 5 * preset_decay_seconds`, and use
`graph_release = (9.21 / 6.9078) * preset_release_seconds` (about 4/3).
For example, a 200 ms preset decay matches a 1 second graph decay; a 300 ms
preset release matches about 0.4 seconds in the graph. This matches envelope
rates within supported ranges and sample precision; timbre depends on the rest
of each instrument. [The source example](../examples/envelope-timing.muz)
implements the conversion with an ordinary function.

`synth` resolves `kick`, `snare` and `cymbal` to source voice patches in
`std/catalogs.muz` (including the kick, snare, hat and crash presets). Their
oscillators, pitch sweeps, noise mix and saturation are editable source recipes.
`decay_ms` remains an automatable patch parameter, 1–10000 ms; `gain_db` remains
a device control. Drive is selected when building the patch. These recipes ignore
`attack_ms`, `sustain`, `release_ms` and ordinary note-off, as before. Numeric
native modes 3–5 now fail with a migration diagnostic: use `synth` with the named
mode instead. Saw, pulse, FM, noise and FM-percussion remain native modes.

A graph ADSR with `one_shot:true` follows its held attack/decay/sustain even after
note-off. Use sustain zero for a finite envelope. CC 120 chokes graph envelopes
with an 8 ms release (80 dB reduction); CC 123 supplies ordinary note-off.
A patch whose ADSRs are all one-shot retires once all levels fall below 0.00001,
after at least two samples. Mixed patches retire after note-off once every ADSR
finishes. The 60 second graph decay bound accommodates slow source envelopes,
including conversion of the full preset decay range; no extra buffers are needed.

The source percussion amplitude uses a 0.7 ms linear attack followed by
`exp(-(t-0.0007)/D)`, with `graph_decay = 5*D`. Native FM-percussion retains
`exp(-t/D) * min(t/0.0007,1)`. Patches preserve the gate/choke contract but are
not sample-identical to the former native recipes: the general highpass replaces
the old noise filter, the snare transient decays smoothly instead of in fixed
steps, and the patch pans its mono output at center. See
[the source example](../examples/percussion-patches.muz) for stock voices and a
custom one-shot resonator. No percussion-specific DSP operation is needed.

Additional pitch/noise/FM/filter motion can change audible tail length. In the
native pitched/noise preset, the filter offset is `filter_env * exp(-t/D)` octaves,
starting at note-on independently of amplitude sustain/note-off.

## Source catalogs and policies

`std/catalogs` contains synth presets, kit voice/choke defaults, scale modes,
Euclidean rhythms, LFO construction and curve sampling recipes. The standard
prelude makes the familiar calls available without imports. Adding a helper to
the source prelude requires no Rust function-name registry.

`synth(name,params={},presets=catalog.synth_presets)` and
`scale(root,mode="minor",octave=4,modes=catalog.scale_modes)` accept alternative
source tables. An ordinary `{type:"synth",name:"my-patch",...}` record can describe
a custom device directly. `drums(lanes,span=4b,voices=...,articulations=...,gate=0.5)`
uses source tables. The generic `drum_grid` decoder accepts arbitrary lane names
mapped to pitches and arbitrary strike symbols mapped to velocities (or `null`
for rests); the kit supplies the actual sample or instrument for each voice.

`curve_value(curve,position)` evaluates a curve; a list of positions evaluates it
once for the whole batch. `unit(quantity)` returns one in its dimension. Source
`curve_map`, `curve_add`, and `curve_mul` choose sampling resolution and retain
original knots and step-edge guard points. Composers can supply another sampling
policy without altering the interpolation kernel.

`std/tonal` exports the source `tonal_scoring` record and search wrappers. Their
register, candidate count and ranking weights are ordinary source choices; native
solvers retain bounded candidate generation and search. Source performance and
pattern recipes similarly sit above generic lossless event operations.

Synth presets and overrides use named modes, for example
`synth("init", {mode:"pulse", cutoff_hz:700})`. Unknown names and numeric mode
overrides are rejected by the source helper. This is a checked choice in stdlib,
not a first-class language enum. The single `synth_mode_codes` table translates
names to integer device codes; raw device records and numeric parameter inspection
remain the lower-level engine interface. The audio algorithms are unchanged.

## Editing patch controls

Inline `param.value` defaults, top-level exposed overrides, and `gain_db` are
mutable controls, including in legacy flat patches and serialized sessions.
A top-level override takes precedence over the node default; the device parameter
map takes precedence over embedded values in imported sessions. Value-only source
edits preserve the prepared instrument through parameter updates. Changing a node,
connection, parameter range, or asset still prepares a replacement. Automation
continues to address the same parameter names.
