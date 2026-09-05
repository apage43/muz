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
| `adsr` | `attack`, `decay`, `release` in seconds; `sustain` 0..1 |
| `sum`, `mul` | `inputs` array of signals |
| `drive` | `input`, linear drive `amount`; tanh saturation |
| `filter` | `input`, `cutoff` Hz, `q`; `mode`: lowpass/highpass |
| `delay` | `input`, `seconds`, `feedback`; fixed `max_seconds` allocation |
| `sample` | WAV/FLAC `path`, `root` pitch, optional `loop:true`; mono voice reader |

Each patch has 16 voices, at most 64 nodes, at most 16 inputs to a sum/product, and at most two seconds of delay memory per voice. Each delay allows up to one second, with feedback clamped inside ±0.98. Sample readers share at most eight million decoded frames per patch. Dynamic frequency/filter/envelope inputs have bounded ranges. The output must be named explicitly. A graph is acyclic; feedback lives inside delay nodes. Cycles/forward references and unknown fields fail during preparation.

Volume, expression, pan and tuning apply to the voice automatically. Brightness, vibrato and pressure are available to wire into the desired graph inputs. A patch containing ADSR nodes retires released voices when all its envelopes finish; otherwise a short default release applies. Put delays before the final envelope when you want their sound gated with the voice; use track effects for tails that should outlive voice retirement. Voice stealing uses the quietest current voice.

Fractional note pitch and attack intensity stay precise through native rendering. CLAP native-note ports receive tuning/expression; MIDI export quantizes notes and does not encode these per-note curves. Native sample resampling and nonlinear saturation are intentionally modest first implementations, not oversampled mastering processors.

Voice-graph connections retain their units: a `frequency` node or a `param` carrying 2400 Hz is not an audio amplitude. Each consuming oscillator/filter/envelope bounds its own controls; non-finite intermediate values are replaced by zero. Earlier builds incorrectly clipped every connection to ±100, corrupting frequency and cutoff signals. The focused regression compares literal and connected Hz controls.

The preset synth also exposes `mode` (0 saw, 1 pulse, 2 FM, 3 kick, 4 snare, 5 cymbal, 6 metallic percussion, 7 noise), `filter_env` in octaves, `resonance`, `sub`, `unison` (1–5), `detune_cents`, `width`, `fm_ratio`, `fm_index`, `drive_db`, and delayed `vibrato_cents`/`vibrato_hz`. Override these on `synth("init", {...})` or an existing preset. `muz devices inspect studio_synth` lists parameter ranges. Pitched/noise modes use gate-controlled envelopes; percussion modes finish their decays after note-off and respond to choke. `../muz-projects/<piece>/sounds.muz` illustrates short bass/stab envelopes and independent short-room/long-hall production in its song.

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
