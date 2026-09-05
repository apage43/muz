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
