# Production and audition

Tracks have an instrument, `chain`, output bus, gain in dB, pan and sends. Buses have a chain, output and sends; `master` is the final chain. Feedback between buses and sidechain dependency cycles are errors. A delay device provides intentional feedback within a bounded processor. EQ, low/highpass, compressor, limiter, delay, reverb, chorus, gate, bitcrusher, drive, gain and stereo are native. `synth` presets include pulse-bass, glass-lead, pad, choir, bell, fifths and percussion voices.

Sends follow the source's output fader by default: `sends:{hall:-18}`. `sends:{hall:{gain:-18,pre:true}}` taps before that fader. Both taps follow the inserts/pan. Output-fader automation affects post-fader sends. A solo audition preserves processing of muted sidechain sources.

`rack([[fx(...),fx(...)],[fx(...)]],{id:"parallel",mix:0.4,gain_db:0})` runs serial branches in parallel, aligns their latencies, sums them, and mixes with an equally delayed dry input. An empty branch is a dry path; set branch gains explicitly when summing several full-level branches. A rack has at most eight branches / 32 devices. Define racks with normal functions and parameter defaults. `expose:{tone:"0.0.cutoff_hz"},tone:1200` exposes a branch-index/device-index control as `track.parallel.tone`. Nested rack objects are unnecessary: compose the branch arrays in source.

Rack `modulate` entries name an internal target and resolve `base + depth*sin(2π*rate_hz*t) + follower*envelope`, clamped to explicit `min`/`max`. The envelope uses `attack_ms`/`release_ms` and the rack's input or its external `sidechain` track. This builds tremolo, envelope filters, duckers, gated spaces and band-split racks without Rust. See `std/mix` and `examples/racks.muz`. Exposed setters and modulators cannot compete for the same target.

```
fx("compressor", {id:"duck", sidechain:"drums.kick", threshold_db:-25, ratio:5, attack_ms:3, release_ms:130})
automation("lead.instrument.cutoff_hz", curve([[0b,500],[16b,3500]], "smooth"))
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

Automation targets explicit device IDs and parameter names or route IDs (`lead.out`, `lead.send.echo`). Curves use beat positions or seconds; values use the target parameter's units. Shapes are linear, smooth and step. One lane owns each target.

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
timing; share the song's tempo settings as described in `muz docs language`.
Generated curves remain visible with `muz inspect song.muz --view automation`.

`std/mix.muz` includes editable examples: `note_start`/`note_end` calculate performed
times, `gate_windows` merges constant-level windows, and `mix.throws(material,
selector,target,level=-12,tail=120ms,timing={})` returns an ordinary automation lane
using those helpers. Put it in the song's `automation` list. Its send-volume recipe
returns to −120 dB after key release plus tail, and empty selections leave the
send closed. It is implemented entirely in `.muz`, with no special compiler path.
The automated send carries the track's sounding audio; the receiving effect keeps
processing its tails after the send closes. [The example](../examples/tagged-send.muz)
combines this recipe with a composer-written velocity-shaped filter gesture.

`lfo(period,duration,low=0,high=1,phase=0)` builds a reusable cosine control curve with 64 points per cycle. `curve_at(curve,offset)` places local envelopes. `curve_map(curve,fn(x)=>...)`, `curve_add(a,b)` and `curve_mul(a,b)` explicitly compose controls off-thread; their default sampling resolution is 1/64 beat (or second for clock curves). Supply `resolution` for sharper shapes. Automation is then interpolated at audio sample positions. Lookahead/latency controls require a prepared source edit, not automation.

Plugin parameters are normalized 0..1, addressed by numeric ID or the key shown by `muz devices inspect PATH`. `plugin(PATH,{class:"...",state:"preset.state",parameter_key:0.5})` hosts VST3 instruments and effects. The first audio class is selected when class is absent. `piano()` uses the local Pianoteq 9 installation; override path/class/state explicitly on another machine. `muz devices state PATH -o preset.state` captures component state; `--load` accepts raw component state or a VST3 preset. Loading state and plugin preparation happen outside the callback. Linux stereo plugins are the initial supported layout.

`muz devices convert PATH PARAMETER PLAIN_VALUE` uses the plugin controller's actual plain-to-normalized mapping. State loads first, then source parameter overrides. Host restart notifications request a prepared replacement and recalculate latency on the coordinator; live plugin crashes are not isolated. Inspect/state commands are separate muz invocations, and background render failures cannot publish a partial output. `check` prepares the graph and validates available plugin parameters as well as musical source.

`sample("audio.wav",{root:60,offset:0s,attack_ms:2,release_ms:30})` plays WAV at the note's pitch. Arrays of paths rotate round robin; arrays of records add path, root, keys `[0,127]`, velocity `[0,1]`, offset, loop `[start_seconds,end_seconds]` and one_shot. Sample data is prepared before playback. Mono/stereo integer and float WAV work at different sample rates.

`root` is the recording's MIDI pitch in `[0,127]`, including fractional values
for fine tuning. For a sample with measured fundamental `f` Hz, calculate its
MIDI root as 69 + 12 log₂(f/440). For a waveform repeated every `n` frames at sample
rate `r`, `f=r/n`. For example, 32 frames at 8,363 Hz need
`root:59.981341609272455`. The sampler preserves that value through compilation,
session serialization and playback; rounding or truncating it detunes every note.
Zone `root` values override the parent sample root; integer roots remain valid.

Graph preparation (including `muz check`, render and live reload) rejects performed
sampler notes with no matching zone before loading instruments. The diagnostic
names each affected physical track, its missing-note count, and an example pitch,
velocity and source key. Keys use the same rounded MIDI key as playback; fractional
pitch remains available for tuning. Key bounds are inclusive. Velocity layers are
`[low,high)`, except an upper bound of 1 includes full velocity. Checking uses the
performed float velocity, not its MIDI-export quantization. Overlapping matching
zones still rotate round robin; this check never remaps notes.

`clip("texture","audio.wav",{at:8s,offset:2s,duration:6s,fade_in:100ms,fade_out:400ms,gain:-12})` creates a track for a clock-timed audio region. It supports trim, fades and sample-rate conversion; it does not time-stretch. Imported asset paths resolve relative to the module that declares them.

The server runs two background renders concurrently and queues up to 32 more in submission order. `muz render --socket PATH -o audition.wav --section chorus` returns a job immediately; `muz jobs --socket PATH` shows `queued`, `running`, `finished`, `failed` or `cancelled`, together with the source and accepted revision captured at submission. Later source edits do not change queued musical/graph data; external asset files remain ordinary live files. `muz cancel ID --socket PATH` cancels either a waiting or running job, preserving an existing destination file. Failed/cancelled workers release their slots automatically. Duplicate active output paths and a full waiting queue are rejected. Shutdown cancels workers and discards waiting jobs; the queue is in memory and does not survive a server restart.

`muz render source.muz -o master.wav --format pcm24` exports with TPDF dither. float32 is the default. `--section NAME` renders preceding context from song start and discards it, preserving effect and instrument history. `--start SECONDS --seconds LENGTH`, `--tail SECONDS`, `--solo TRACK`, and `--tap TRACK_OR_BUS` refine scope. Latency is aligned through parallel routes and trimmed from exports. Live loops chase overlapping notes and prior controllers; exact history is available through section bounces.

`muz stems source.muz -o stems/` exports each physical track after inserts and track pan, before output gain/sends/master. `--wet` exports solo auditions through effects returns and the nonlinear master; these do not sum back to the mix. Shared returns can be exported by bus name with `render --tap`. Solo leaves detector sources running. `analyze` reports integrated LUFS, loudness range, true peak, sample peak, RMS, DC and stereo correlation.

# CLAP, native graphs and assets

`plugin("/path/Instrument.clap",{class:"plugin.id",state:"patch.state",p123:0.4})` uses CLAP. `muz devices list` lists native devices and discovered plugins; `muz devices inspect eq` shows native parameter ranges/defaults. Inspect a plugin path for its stable IDs, parameter ranges and ports. CLAP values use the plugin's plain units (some plugins themselves expose a 0..1 range); VST3 values use normalized 0..1. VST3 and CLAP automation are delivered through preallocated sample-offset parameter queues. Plugin reconstruction of those values is the plugin's responsibility.

CLAP supports main-thread callbacks, parameter/state inspection, mono/stereo ports, native note expression and MIDI channel input. Stereo main output is used; auxiliary audio outputs are currently discarded and auxiliary inputs are silent. Plugin MIDI/event output is not routed. The current locally exercised plugins are Pianoteq 9 VST3, Surge XT VST3/CLAP and Surge XT Effects VST3/CLAP. Surviving those checks is not a promise of arbitrary plugin compatibility. Plugin code runs in the live process; child bounces isolate render failures.

Per-note expression belongs to CLAP native-note ports and `voice_patch`; see `muz docs synthesis`. VST3 receives floating attack intensity and initial note tuning, but subsequent note-expression curves require CLAP/native graphs or an explicitly split layer. Channel pressure, poly pressure, bank/program and bend are also available on capable plugin adapters.

Samples and clips decode mono/stereo WAV or FLAC natively. Samples/preset files are watched along with source imports. File length and modification time trigger fresh preparation when an asset changes; this is a development convenience, not a content identity or reproducibility guarantee. External media stays outside git; each piece documents the exact licensed downloads and folder layout it needs.

Audio blocks are bounded to 1024 frames, with 256 scheduled note/control events per physical track per block. Dense multi-note expression can reach that budget; use a smaller block size or fewer simultaneous controls. Native synth/voice patches have 16 voices and samplers 32; they steal voices when necessary. Piano policy concerns musical playability independently of those sound-engine budgets. Latency-changing controls cannot be hidden behind rack exposure/modulation.

Socket inspection accepts `section` and `track`, for example `muz call '{"command":"inspect","view":"performance","section":"bridge","track":"piano"}'`. Status contains compact transport/revision/device telemetry. Detailed responses are bounded to 16 MiB; filter large inspections. Expression programs serialize only their actual points, while prepared voice state remains fixed-size on the audio thread.

## Named comparisons and delivery collections

A source module can export named lists of `{name, song, options}` records. `song`
is an ordinary evaluated song, so candidates can come from different arguments
to the same composer function. For example:

```muz
let compare = [
    {name:"held", song:version(0), options:{section:"final-chorus",tail:2}},
    {name:"lifted", song:version(7), options:{section:"final-chorus",tail:2}}
];
let delivery = [
    {name:"master",song:chosen,options:{format:"pcm24",tail:3}},
    {name:"lead-tap",song:chosen,options:{tap:"lead",tail:3}},
    {name:"hall-return",song:chosen,options:{tap:"hall",tail:3}}
];
```

Run `muz batch revisions.muz compare -o out/compare --match-levels` or
`muz batch revisions.muz delivery -o out/delivery`. Names use letters, numbers,
hyphens and underscores. Options are the existing renderer options:
`section`, `start`, `seconds`, `tail`, `solo`, `tap`, `format`, `sample_rate`,
and `block_size`. `start`, `seconds` and `tail` are seconds; section names resolve
against each candidate. Omitted options retain ordinary render defaults.

The module and all candidate songs are evaluated before starting the collection.
The existing two-worker queue runs up to 34 captured outputs in isolated child
processes. Ctrl-C cancels outstanding work; failed outputs preserve existing files.
`renders.json` records each result or failure and its exact render scope. Batch
renders are disk-source captures, so they have no live-server revision number.

`listen.html` plays the results and switches candidates at the same playback
position. With `--match-levels`, measured integrated loudness determines listening
attenuation to the quietest measurable candidate. The WAVs and their production
processing are unchanged. Very short or silent candidates without measurable
integrated loudness remain unmatched. Keep arrangement, preceding context, render
scope and controllable randomness consistent when comparing one musical choice.

Track taps remain post-insert (including track pan) and before output
gain/sends/master. Return taps contain the shared return. Wet solo auditions pass through nonlinear production
and do not sum to the full mix. Name those boundaries explicitly in delivery
recipes instead of treating every output as a summable stem.

See `examples/revision-workflow.muz` for a complete source example.

## Gesture scope

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
combination. Neither scope introduces a special engine path for tags or sends.

## Local plugin aliases

Machine paths and plugin class identifiers live in
`$XDG_CONFIG_HOME/muz/plugins.json` (normally `~/.config/muz/plugins.json`).
`MUZ_PLUGIN_CONFIG` selects another file. It contains an object of named aliases:

```json
{
  "default": {
    "path": "/chosen/location/Pianoteq.vst3",
    "class": "the-class-id-from-muz-devices-inspect"
  },
  "my-synth": {"path": "/chosen/location/instrument.clap"}
}
```

`piano()` uses the `default` alias; named plugin references can resolve an alias.
Explicit source fields override the alias. Paths relative to the configuration
file resolve against its directory. Choose and inspect the actual installed
plugin; there is no built-in versioned Pianoteq path. Project state and source
parameter overrides remain project inputs.


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

## External native-audio listening reviews

`tools/listening_review.py` is an optional developer aid using Python's standard
library and an `OPENROUTER_API_KEY` environment variable. It sends the actual
base64 audio as OpenRouter `input_audio`, not a transcript or a spectrogram.
The default model is `google/gemini-3.8-flash`; `--model` selects another compatible
model. Running it sends the named audio to OpenRouter and incurs API charges.

For model selection, controlled comparisons and revision decisions, explicitly
invoke the repo-local [$external-critique skill](../.agents/skills/external-critique/SKILL.md).

```sh
python tools/listening_review.py --audio out/excerpt.wav \
  --prompt prompts/first-listen.txt \
  --output out/listening/first
python tools/listening_review.py --audio out/a.wav --audio out/b.wav \
  --audio out/c.wav --audio out/d.wav \
  --prompt prompts/blind-comparison.txt \
  --output out/listening/compare
```

Audio inputs are labelled A, B, etc. without revealing filenames to the model.
Save a local mapping of each source, crop bounds, and listening gain. For a mix
comparison, export equivalent musical passages, measure them with `muz analyze`,
and apply constant listening gain to a shared LUFS level; retain the original
production files. Do not use dynamic normalization that changes the mix being
judged. Short stereo PCM WAV excerpts work; FLAC is also
accepted. Supported formats depend on the selected provider.

The output directory retains the prompt, source hashes, returned model identity,
usage/cost, raw response and readable review. It does not retain credentials or
base64 payloads. Check `prompt_tokens_details.audio_tokens` where reported, and
verify basic audible event claims against the known excerpt. The model can
confidently invent timestamps, instruments and absent processing despite accepting
audio. A blind preference is useful subjective evidence, not proof of release
quality. Use focused excerpts and independent peak/tail/mono checks before choosing
a revision. Keep generated media and review output under ignored `out/`.

The caller prefers cheaper provider routes without requiring zero data retention.
Use `--provider` to pin a known working provider, `--reasoning` and `--max-tokens`
to control reasoning/output, and `--temperature` when comparing sampling settings.
Provider routing and generation settings are recorded in the request metadata.
Long base64-like strings in responses/errors are redacted before storage; HTTP
errors print a compact diagnostic pointing to the saved sanitized body. A review
that ends at the output-token limit is saved but exits unsuccessfully, so a
truncated response is not silently accepted as complete.

## Graph resource preparation

`muz check` reports expanded physical tracks, devices, buses (including master),
routes, total resource units and the five largest contributing lanes/buses. JSON
output exposes this under `graph` and `graph_budget`. Kit expansion is included.
There are no separate 32-track, 128-device, 15-bus or 128-route limits.

The process-wide `MUZ_GRAPH_BUDGET` defaults to 4096 units. Each graph device, bus
or route costs one unit; a track contributes its instrument, inserts and routes.
Set it before starting muz, for example `MUZ_GRAPH_BUDGET=8192 muz check song.muz`.
It accepts integers from 1 through 65536 and stays fixed for the process lifetime.
A failed preflight reports required/allowed units, expanded counts and contributors.
This is a topology preparation budget, not a CPU or decoded-sample memory estimate;
plugin, rack, sample and per-block event budgets still apply independently.

Engine and transaction vectors are sized from the graph during preparation. Live
telemetry and its scratch space are allocated from the process budget before
playback, so edits can grow the graph within that budget without allocating in the
audio callback. A larger budget permits larger preparations; shared bus processing
remains useful for reducing actual work.
