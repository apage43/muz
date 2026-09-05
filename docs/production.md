# Production and audition

Tracks have an instrument, `chain`, output bus, gain in dB, pan and sends. Buses have a chain, output and sends; `master` is the final chain. Feedback between buses and sidechain dependency cycles are errors. A delay device provides intentional feedback within a bounded processor. EQ, low/highpass, compressor, limiter, delay, reverb, chorus, gate, drive, gain and stereo are native. `synth` presets include pulse-bass, glass-lead, pad, choir, bell, fifths and percussion voices.

Sends follow the source's output fader by default: `sends:{hall:-18}`. `sends:{hall:{gain:-18,pre:true}}` taps before that fader. Both taps follow the inserts/pan. Output-fader automation affects post-fader sends. A solo audition preserves processing of muted sidechain sources.

`rack([[fx(...),fx(...)],[fx(...)]],{id:"parallel",mix:0.4,gain_db:0})` runs serial branches in parallel, aligns their latencies, sums them, and mixes with an equally delayed dry input. An empty branch is a dry path; set branch gains explicitly when summing several full-level branches. A rack has at most eight branches / 32 devices. Define racks with normal functions and parameter defaults. `expose:{tone:"0.0.cutoff_hz"},tone:1200` exposes a branch-index/device-index control as `track.parallel.tone`. Nested rack objects are unnecessary: compose the branch arrays in source.

Rack `modulate` entries name an internal target and resolve `base + depth*sin(2π*rate_hz*t) + follower*envelope`, clamped to explicit `min`/`max`. The envelope uses `attack_ms`/`release_ms` and the rack's input or its external `sidechain` track. This builds tremolo, envelope filters, duckers, gated spaces and band-split racks without Rust. See `std/mix` and `examples/racks.muz`. Exposed setters and modulators cannot compete for the same target.

```
fx("compressor", {id:"duck", sidechain:"drums.kick", threshold_db:-25, ratio:5, attack_ms:3, release_ms:130})
automation("lead.instrument.cutoff_hz", curve([[0b,500],[16b,3500]], "smooth"))
throws("lead", "echo", "lead.send.echo", -12, tail=180ms)
```

Automation targets explicit device IDs and parameter names or route IDs (`lead.out`, `lead.send.echo`). Curves use beat positions or seconds; values use the target parameter's units. Shapes are linear, smooth and step. One lane owns each target. Annotation-driven throws merge overlapping windows and open a send around the performed note, including human timing. This sends the track's complete audio during that interval, including overlapping voices. For isolated echoes, put the tagged material on a dedicated track.

`lfo(period,duration,low=0,high=1,phase=0)` builds a reusable cosine control curve with 64 points per cycle. `curve_at(curve,offset)` places local envelopes. `curve_map(curve,fn(x)=>...)`, `curve_add(a,b)` and `curve_mul(a,b)` explicitly compose controls off-thread; their default sampling resolution is 1/64 beat (or second for clock curves). Supply `resolution` for sharper shapes. Automation is then interpolated at audio sample positions. Lookahead/latency controls require a prepared source edit, not automation.

Plugin parameters are normalized 0..1, addressed by numeric ID or the key shown by `muz devices inspect PATH`. `plugin(PATH,{class:"...",state:"preset.state",parameter_key:0.5})` hosts VST3 instruments and effects. The first audio class is selected when class is absent. `piano()` uses the local Pianoteq 9 installation; override path/class/state explicitly on another machine. `muz devices state PATH -o preset.state` captures component state; `--load` accepts raw component state or a VST3 preset. Loading state and plugin preparation happen outside the callback. Linux stereo plugins are the initial supported layout.

`muz devices convert PATH PARAMETER PLAIN_VALUE` uses the plugin controller's actual plain-to-normalized mapping. State loads first, then source parameter overrides. Host restart notifications request a prepared replacement and recalculate latency on the coordinator; live plugin crashes are not isolated. Inspect/state commands are separate muz invocations, and background render failures cannot publish a partial output. `check` prepares the graph and validates available plugin parameters as well as musical source.

`sample("audio.wav",{root:60,offset:0s,attack_ms:2,release_ms:30})` plays WAV at the note's pitch. Arrays of paths rotate round robin; arrays of records add path, root, keys `[0,127]`, velocity `[0,1]`, offset, loop `[start_seconds,end_seconds]` and one_shot. Sample data is prepared before playback. Mono/stereo integer and float WAV work at different sample rates.

`clip("texture","audio.wav",{at:8s,offset:2s,duration:6s,fade_in:100ms,fade_out:400ms,gain:-12})` creates a track for a clock-timed audio region. It supports trim, fades and sample-rate conversion; it does not time-stretch. Imported asset paths resolve relative to the module that declares them.

The server admits at most two concurrent background render jobs. Use `muz jobs --socket PATH` to see when a slot is available; additional submissions fail without starting another worker.

`muz render source.muz -o master.wav --format pcm24` exports with TPDF dither. float32 is the default. `--section NAME` renders preceding context from song start and discards it, preserving effect and instrument history. `--start SECONDS --seconds LENGTH`, `--tail SECONDS`, `--solo TRACK`, and `--tap TRACK_OR_BUS` refine scope. Latency is aligned through parallel routes and trimmed from exports. Live loops chase overlapping notes and prior controllers; exact history is available through section bounces.

`muz stems source.muz -o stems/` exports each physical track after inserts, before output gain/sends/master. `--wet` exports solo auditions through effects returns and the nonlinear master; these do not sum back to the mix. Shared returns can be exported by bus name with `render --tap`. Solo leaves detector sources running. `analyze` reports integrated LUFS, loudness range, true peak, sample peak, RMS, DC and stereo correlation.

# CLAP, native graphs and assets

`plugin("/path/Instrument.clap",{class:"plugin.id",state:"patch.state",p123:0.4})` uses CLAP. `muz devices list` lists native devices and discovered plugins; `muz devices inspect eq` shows native parameter ranges/defaults. Inspect a plugin path for its stable IDs, parameter ranges and ports. CLAP values use the plugin's plain units (some plugins themselves expose a 0..1 range); VST3 values use normalized 0..1. VST3 and CLAP automation are delivered through preallocated sample-offset parameter queues. Plugin reconstruction of those values is the plugin's responsibility.

CLAP supports main-thread callbacks, parameter/state inspection, mono/stereo ports, native note expression and MIDI channel input. Stereo main output is used; auxiliary audio outputs are currently discarded and auxiliary inputs are silent. Plugin MIDI/event output is not routed. The current locally exercised plugins are Pianoteq 9 VST3, Surge XT VST3/CLAP and Surge XT Effects VST3/CLAP. Surviving those checks is not a promise of arbitrary plugin compatibility. Plugin code runs in the live process; child bounces isolate render failures.

Per-note expression belongs to CLAP native-note ports and `voice_patch`; see `muz docs synthesis`. VST3 receives floating attack intensity and initial note tuning, but subsequent note-expression curves require CLAP/native graphs or an explicitly split layer. Channel pressure, poly pressure, bank/program and bend are also available on capable plugin adapters.

Samples and clips decode mono/stereo WAV or FLAC natively. Samples/preset files are watched along with source imports. File length and modification time trigger fresh preparation when an asset changes; this is a development convenience, not a content identity or reproducibility guarantee. External media stays outside git; each piece documents the exact licensed downloads and folder layout it needs.

Audio blocks are bounded to 1024 frames, with 256 scheduled note/control events per physical track per block. Dense multi-note expression can reach that budget; use a smaller block size or fewer simultaneous controls. Native synth/voice patches have 16 voices and samplers 32; they steal voices when necessary. Piano policy concerns musical playability independently of those sound-engine budgets. Latency-changing controls cannot be hidden behind rack exposure/modulation.

Socket inspection accepts `section` and `track`, for example `muz call '{"command":"inspect","view":"performance","section":"bridge","track":"piano"}'`. Status contains compact transport/revision/device telemetry. Detailed responses are bounded to 16 MiB; filter large inspections. Expression programs serialize only their actual points, while prepared voice state remains fixed-size on the audio thread.
