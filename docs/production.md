# Production and audition

Tracks have an instrument, `chain`, output bus, gain in dB, pan and sends. Buses have a chain, output and sends; `master` is the final chain. Feedback between buses and sidechain dependency cycles are errors. A delay device provides intentional feedback within a bounded processor. EQ, low/highpass, compressor, limiter, delay, reverb, chorus, gate, drive, gain and stereo are native. `synth` presets include pulse-bass, glass-lead, pad, choir, bell, fifths and percussion voices.

```
fx("compressor", {id:"duck", sidechain:"drums.kick", threshold_db:-25, ratio:5, attack_ms:3, release_ms:130})
automation("lead.instrument.cutoff_hz", curve([[0b,500],[16b,3500]], "smooth"))
throws("lead", "echo", "lead.send.echo", -12, tail=180ms)
```

Automation targets explicit device IDs and parameter names or route IDs (`lead.out`, `lead.send.echo`). Curves use beat positions or seconds; values use the target parameter's units. Shapes are linear, smooth and step. One lane owns each target. Annotation-driven throws merge overlapping windows and open a send around the performed note, including human timing. This sends the track's complete audio during that interval, including overlapping voices. For isolated echoes, put the tagged material on a dedicated track.

Plugin parameters are normalized 0..1, addressed by numeric ID or the key shown by `muz devices inspect PATH`. `plugin(PATH,{class:"...",state:"preset.state",parameter_key:0.5})` hosts VST3 instruments and effects. The first audio class is selected when class is absent. `piano()` uses the local Pianoteq 9 installation; override path/class/state explicitly on another machine. `muz devices state PATH -o preset.state` captures component state; `--load` accepts raw component state or a VST3 preset. Loading state and plugin preparation happen outside the callback. Linux stereo plugins are the initial supported layout.

`sample("audio.wav",{root:60,offset:0s,attack_ms:2,release_ms:30})` plays WAV at the note's pitch. Arrays of paths rotate round robin; arrays of records add path, root, keys `[0,127]`, velocity `[0,1]`, offset, loop `[start_seconds,end_seconds]` and one_shot. Sample data is prepared before playback. Mono/stereo integer and float WAV work at different sample rates.

`muz render source.muz -o master.wav --format pcm24` exports with TPDF dither. float32 is the default. `--section NAME` renders preceding context from song start and discards it, preserving effect and instrument history. `--start SECONDS --seconds LENGTH`, `--tail SECONDS`, `--solo TRACK`, and `--tap TRACK_OR_BUS` refine scope. Latency is aligned through parallel routes and trimmed from exports. Live loops chase overlapping notes and prior controllers; exact history is available through section bounces.

`muz stems source.muz -o stems/` exports each physical track after inserts, before output gain/sends/master. `--wet` exports solo auditions through effects returns and the nonlinear master; these do not sum back to the mix. Shared returns can be exported by bus name with `render --tap`. Solo leaves detector sources running. `analyze` reports integrated LUFS, loudness range, true peak, sample peak, RMS, DC and stereo correlation.
