# Native synthesis and voice patches

Use a preset for a ready-made sound or a `voice_patch` to define its signal flow.
A patch is an ordinary source value: functions, imports, arrays, and records
provide reuse. Read [instruments](instruments.md) for track setup and
[per-note expression](performance.md#per-note-expression) for the note-side syntax.

[Documentation index](README.md) · Offline: `muz docs synthesis`

## Contents

- [Preset controls](#preset-controls)
- [A minimal voice patch](#a-minimal-voice-patch)
- [Constructing signals in source](#constructing-signals-in-source)
- [Editing and inspecting controls](#editing-and-inspecting-controls)
- [Expression](#expression)
- [Envelopes and voice lifetime](#envelopes-and-voice-lifetime)
- [Stereo output](#stereo-output)
- [Continuing a mono voice](#continuing-a-mono-voice)
- [Zone readers and sampled instruments](#zone-readers-and-sampled-instruments)
- [Node reference and limits](#node-reference-and-limits)
- [Feedback loops and waveguides](#feedback-loops-and-waveguides)
- [Blown instruments](#blown-instruments)
- [Reusable instruments](#reusable-instruments)

## Preset controls

Start with `synth("pulse-bass")`, `synth("bell")`, or another entry in
[std/catalogs](../std/catalogs.muz). Override controls with the second argument:

```muz
synth("init", {mode: "pulse", cutoff_hz: 700})
```

| Controls | Purpose |
| --- | --- |
| `mode` | `saw`, `pulse`, `fm`, `kick`, `snare`, `cymbal`, `fm_percussion`, or `noise` |
| `filter_env`, `resonance` | Filter-envelope depth in octaves and resonance |
| `sub`, `unison`, `detune_cents`, `width` | Sub oscillator, 1–5 unison voices, detuning, and stereo spread |
| `fm_ratio`, `fm_index` | FM oscillator settings |
| `drive_db` | Saturation drive |
| `vibrato_cents`, `vibrato_hz` | Delayed vibrato |

`muz devices inspect studio_synth` lists ranges and defaults. Pitched/noise modes
use gate-controlled envelopes. Percussion modes finish their decays after note-off
and respond to choke; the named kick/snare/cymbal recipes are source voice patches.

In the preset synth, `width` controls **stereo spread of unison voices**; its pulse
oscillator has a fixed 50% duty cycle. In a voice-patch `osc` node, `width` instead
controls **pulse duty cycle**. Use an explicit pulse node for 25% or 12.5% shapes.

## A minimal voice patch

A voice is one sounding note and its processing state. This complete song uses
an oscillator, filter, and amplitude envelope for each voice:

```muz
let simple = voice_patch("simple", {
    gain_db: -16,
    nodes: [
        {id: "cutoff", op: "param", value: 2400, min: 80, max: 10000},
        {id: "osc", op: "osc", wave: "saw", detune: 3},
        {id: "filter", op: "filter", input: "osc", cutoff: "cutoff", q: 0.7},
        {id: "env", op: "adsr", attack: 0.01, decay: 0.2, sustain: 0.6, release: 0.3},
        {id: "out", op: "mul", inputs: ["filter", "env"]}
    ],
    output: "out"
});
song({tracks: [track("lead", phrase("C4:q E4:q G4:h"), simple)], tail: 1})
```

Nodes appear after their dependencies. A signal input is a number or an earlier
node ID. Internal signals run at audio rate. `param` nodes expose named device
controls; for example, `lead.instrument.cutoff` targets the parameter above.
The `adsr` node is an attack/decay/sustain/release envelope in seconds.

Connections preserve signal magnitudes and units: frequency and cutoff signals
are not audio amplitudes. Each consuming node bounds its controls; nonfinite
intermediate values become zero. Explicit dimensional literals are checked on
node controls, but connected signals do not undergo full dimensional inference.

## Constructing signals in source

`use "std/signal" as s;` supplies ordinary record constructors. A patch may omit
`nodes` and use nested signals directly:

```muz
use "std/signal" as s;
let e = s.adsr(20ms, 200ms, 0.5, 300ms);
let o = s.saw();
voice_patch("nested", {output:s.mul([o,e]), lifetime:{envelope:e,tail:0s}})
```

Reusing a bound record shares one node's state. Calling a constructor twice makes
two independent nodes even when their fields are equal. Record merges create a
new outer node while retaining any shared child records. Public parameter IDs
are explicit; internal IDs are generated deterministically. Dependencies are
ordered during compilation, with the 512-node limit and bounded nesting.
Flat patches remain supported. Explicit dimensional literals are checked on node
controls (seconds for times, Hz for frequency controls); bare scalar values remain
accepted. Full dimensional inference across connected signals is not performed.

## Editing and inspecting controls

Inline `param.value` defaults, top-level exposed overrides, and `gain_db` are
mutable controls, including in legacy flat patches and serialized sessions.
A top-level override takes precedence over the node default; the device parameter
map takes precedence over embedded values in imported sessions. Value-only source
edits preserve the prepared instrument through parameter updates. Changing a node,
connection, parameter range, or asset still prepares a replacement. Automation
continues to address the same parameter names.

`muz inspect song.muz --view patches --track lead` shows a bounded patch summary
with effective controls, node counts and asset paths. Request `--view patch_nodes`
for paged lowered nodes and `--view patch_detail` for non-node configuration.
See [nested-patch.muz](../examples/nested-patch.muz). Constructors have no runtime language cost.

Plain parameters hold their value between changes. Note expressions use a separate
128-frame control clock, addressed by sounding-note identity. No per-sample language
callbacks or runtime compilation occur. For an expression example, see
[voice-expression.muz](../examples/voice-expression.muz).

## Expression

Native patches apply volume, expression, pan, and tuning automatically. Wire
brightness, vibrato, and pressure into graph inputs where you want them to act.
Fractional pitch and attack intensity stay precise through native rendering.
MIDI file export quantizes notes and does not encode per-note curves.

### Presets and standalone samplers

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

A zone without `loop` that is not `one_shot` plays to the end of its recording
and is then silent while the note is still held. `muz check` warns with
`sample.overrun`, once per track, when any held note outlasts its recording. It
counts those notes and names the one with the longest silence, at its bar and
beat. A recording lasts `(length - offset) / 2^((pitch - root) / 12)` seconds at
a note's performed pitch, so a higher note or a slower tempo runs out sooner.
The check uses each note's attack pitch, without tuning curves, and treats a
slurred note as a fresh attack; see the slur caveat below.

With a local recording at `tone.wav`, independent bow/breath swells use ordinary
source values, even when a new note overlaps the previous release tail:

```muz
let swell = {volume:[[0,0.7],[0.3,1],[1,0.6]]};
song({tracks:[track("strings",stack([
    phrase("C4:q").gate(1).express(swell),
    phrase("E4:q").gate(1).at(1b).express(swell)
]),sample([{path:"tone.wav",root:60}],{release_ms:300}))],tail:0.3})
```

### Custom patch controls

Native patches declare custom per-note lanes with `note_controls`:

```muz
use "std/signal" as s;
let pluck = voice_patch("pluck", {
    note_controls: {mute: 0, pick_position: 0.5},
    output: s.mul([s.saw(), s.expression("pick_position")])
});
let notes = phrase("C4:q E4:q").express({
    mute: 1, pick_position: [[0, 0.2], [1, 0.8]]
});
```

Names must be nonempty and cannot reuse standard expression names. Defaults and
values are finite scalars in 0..1; graph arithmetic supplies musical mapping.
At most 25 custom lanes share the existing aggregate 32-point note budget with
the seven standard expressions. Custom slots are assigned by sorted name,
independently of declaration order. Graph readers and note writes must refer to
the destination patch's declarations. Patch parameter IDs remain separate.
Defaults initialize fresh voices and reset on mono ownership transfer; overlapping
notes retain independent values, including through release. Curves use the first
point before its phase and hold their last delivered value through release, on
the existing 128-frame expression clock. Changing names or
defaults changes patch structure and replaces the device on reload.

### Stable variation

`std/instrument.variation(pattern,seed,stream)` uses existing source `keyed_noise`
and writes a stable `variation` value per note identity (`note_identity`: voice,
onset, pitch and occurrence, or a pinned `data.identity`; see
[performance](performance.md#dynamics-and-timing)); `stream` distinguishes
seed streams, not expression destinations. Existing other expression fields,
including actual pressure, remain. Declare `note_controls: {variation: 0}` on the
native patch and read `s.expression("variation")`. Pin variation (or set
`data.identity`) before transformations that move notes in time or pitch if
those transformations should retain a chosen realization.
Variation is computed in source before audio processing.

## Envelopes and voice lifetime

An amplitude envelope shapes a note; voice lifetime determines when its processing
state can be reclaimed. By default, a patch with ADSRs retires released voices
when all envelopes finish. If every ADSR is one-shot, it can also retire held
voices after their envelopes finish. Patches without an explicit lifetime or
ADSR use a short default release. Voice stealing chooses the quietest current voice.

Put delays before the final envelope to gate them with the note. Use a track effect
for tails that must outlive the voice, or declare an explicit patch lifetime.

### ADSR response

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

Additional pitch/noise/FM/filter motion can change audible tail length. In the
native pitched/noise preset, the filter offset is `filter_env * exp(-t/D)` octaves,
starting at note-on independently of amplitude sustain/note-off.

### One-shot envelopes and percussion

A graph ADSR with `one_shot:true` follows its held attack/decay/sustain even after
note-off. Use sustain zero for a finite envelope. CC 120 chokes graph envelopes
with an 8 ms release (80 dB reduction); CC 123 supplies ordinary note-off.
A patch whose ADSRs are all one-shot retires once all levels fall below 0.00001,
after at least two samples. Mixed patches retire after note-off once every ADSR
finishes. The 60 second graph decay bound accommodates slow source envelopes,
including conversion of the full preset decay range.

`synth` resolves `kick`, `snare` and `cymbal` to source voice patches in
`std/catalogs.muz` (including the kick, snare, hat and crash presets). Their
oscillators, pitch sweeps, noise mix and saturation are editable source recipes.
`decay_ms` remains an automatable patch parameter, 1–10000 ms; `gain_db` remains
a device control. Drive is selected when building the patch. These recipes ignore
`attack_ms`, `sustain`, `release_ms`, and ordinary note-off. Numeric
native modes 3–5 fail with a migration diagnostic: use `synth` with the named
mode instead. Saw, pulse, FM, noise and FM-percussion remain native modes.

The source percussion amplitude uses a 0.7 ms linear attack followed by
`exp(-(t-0.0007)/D)`, with `graph_decay = 5*D`. Native FM-percussion retains
`exp(-t/D) * min(t/0.0007,1)`. Patches preserve the gate/choke contract but are
not sample-identical to the former native recipes: the general highpass replaces
the old noise filter, the snare transient decays smoothly instead of in fixed
steps, and the patch pans its mono output at center. See
[the source example](../examples/percussion-patches.muz) for stock voices and a
custom one-shot resonator.

### Explicit lifetime

`lifetime:{envelope:"exciter",tail:0.4}` selects an ADSR whose completion owns the
voice, followed by a fixed tail allowance in seconds (0–60). Other envelopes no
longer decide retirement. A one-shot envelope must finish its attack before it can
complete; an ordinary ADSR completes after note-off. The tail starts once the
selected envelope falls below 0.00001. Choke fades the complete output by 80 dB in
8 ms and reclaims the voice after 10 ms. Legacy patches without `lifetime` retain
their previous envelope-based retirement. This explicit contract lets a delay
continue after its excitation without a dummy envelope or shared track effect.

### Multisegment envelopes

`mseg` accepts 1–16 `attack` segments and 1–8 `release` segments. Each is
`{time:seconds,to:value,curve:"linear"|"smooth"|"exp"}`. Times are 0..60 seconds;
targets are -100..100. Exponential interpolation is normalized
`(1-exp(-5*p))/(1-exp(-5))`, with exact endpoints. `sustain` optionally names a
zero-based attack endpoint to hold. Release starts from the current value and
must end at zero. `one_shot:true` ignores note-off and requires an attack sequence
ending at zero. Segment completion works with explicit `lifetime`; initial zero
is not completion. A segment advances on each sample, including its first.

## Stereo output

`output:{left:"l",right:"r"}` retains two scalar graph outputs. Mono output keeps
its existing equal-power pan. Stereo uses gains `sqrt(2*(1-pan))` and
`sqrt(2*pan)`: center preserves both channels and either endpoint boosts its own
channel by sqrt(2), silencing the other. Graph sample nodes accept
`channel:"left"`, `"right"`, or the legacy `"mono"` downmix. Readers of the same
canonical file share decoded stereo storage within the patch, with independent
playheads. Equal playback settings retain stereo synchronization.

## Continuing a mono voice

`std/instrument.mono(patch,glide=0ms,retrigger=false)` configures the
`voice_mode` of a programmable patch or a `sample(...)` instrument as `legato`
or `retrigger`. The default is `poly`. In mono
modes the last overlapping attack takes ownership of the current held voice;
earlier note-offs and expression events cannot release or modify its new owner.
Legato retains oscillator and envelope state. Retrigger resets the graph on the
new attack. `s.note_age()` restarts in both, so a legato patch can still shape
something at the start of each slurred note (a finger transient, a pitch scoop, a
breath pulse): map it through `shape`, or `s.mapping(s.mul([s.note_age(), -40]),
"exp2")` for a decaying pulse. Both can glide linearly in semitones over mutable `glide_ms` (0–10000).
The first note starts at its own pitch. Source score notes remain separate.

### Slurs: abutting versus overlapping notes

A mono voice continues only while it is still held when the next attack arrives.
When a note's gated release and the next note's attack fall on the same output
frame, the engine delivers the note-off first (events at one frame sort as
flush, note-off/choke, controllers, note-on, expression). The voice is then
already released, so the new note takes a fresh voice, starts at its own pitch
without gliding, and resets the envelope in both `legato` and `retrigger`
modes; the earlier voice finishes its release tail beside it. This is
deterministic: it does not depend on note order in source or on the notes' ids.

So `gate(1)` on a note followed directly by the next one is a re-attack, not a
slur. For a slur, make the release land at least one frame after the next
attack, as `.gate(1.05)` does for ordinary note lengths; a gate that is only
barely above 1 on a very short note can still round to the same frame. The
earlier note-off then no longer releases the voice, which belongs to the newer
note. Humanizing offsets that move one note's release or attack relative to the
other change which case applies. `inspect --view performance` shows note
times, not the delivered order, so confirm borderline slurs by rendering.

This policy does not return to an older held key when the newest key releases;
source can schedule an explicit return attack where desired. New notes following
note-off start new voices while old release tails can finish. Initial expression
values reset on ownership transfer and subsequent expression addresses the new
note. Choke still terminates the voice. A seek reconstructs overlapping attacks
in scheduled order; exact oscillator/filter history requires a contextual bounce.
See [modulated-lead.muz](../examples/modulated-lead.muz) for source variation, an MSEG, drift and glide.

### Slurs on a sampler

`sample(...)` takes the same two options, so `mono(sample(...), glide)` or
`sample(path, {voice_mode: "legato", glide_ms: 30})` gives a sampled line
hammer-ons, pull-offs and slides. The overlap rule above is the per-note choice:
a note that starts while the previous one is still held is slurred, and any
other note is picked. Both stay ordinary notes in the score, MIDI export,
`inspect` and checks.

A slurred note does not start a recording. The sounding voice keeps its
recording and playhead and changes playback speed to the new pitch, measured
from its own zone's root, exactly as `tuning` repitches it. So:

- No zone is selected, the round robin does not advance, and a pinned
  `sample_zone` on the slurred note is unused. The note must still lie inside
  the map's key and velocity coverage, which preparation checks for every note.
- The slurred note's velocity does not change the level; the voice keeps the
  level of the picked note, as a string keeps ringing. Shape a hammer with the
  slurred note's `volume` curve.
- `volume`, `expression`, `pan` and `tuning` reset to their defaults at the slur
  and then follow the slurred note's own curves. `tuning` is relative to the
  slurred note's pitch.
- The pitch steps at once with `glide_ms: 0`, or moves linearly in semitones
  over `glide_ms` (0–10000, automatable) for every slur. For a slide on one
  note, leave `glide_ms` at zero and give that note a tuning ramp from the
  earlier pitch, as `slur(..., slide = 80ms)` below does.
- How far a recording can be repitched before it sounds wrong is for the
  composer to judge; nothing crossfades to the new note's zone.
- The voice releases at the end of the last slurred note. If an unlooped
  recording runs out during a slur, the rest of the group is silent and the next
  note is picked, so loop the zones or keep groups shorter than the recording.
  A `one_shot` zone can be slurred while its note is held and then plays out.
- `retrigger` starts the new note's recording, fades the held voice over 8 ms as
  a choke does, and glides from the old pitch when `glide_ms` is positive.

Mono is one voice per track, as for patches: a chord on a mono track leaves only
its last note sounding. Put each string that slurs on its own track, or keep
chords on a polyphonic one. A seek into a slurred group rebuilds the held notes
in scheduled order, as for patches, so the playhead position is an estimate.

`std/instrument.slur(pattern, tag = "slur", slide = 0ms, overlap = 10ms)` writes
the overlap from tags. For each tagged note it holds the note before it (in
order of `at`) to the tagged note's written start and releases it `overlap`
after the tagged note's performed attack, timing offsets included. The gate ends
where the next note begins, so `checks.monophonic()` still passes. A positive
`slide` adds the tuning ramp from the earlier pitch, unless the note already has
a tuning curve. Which tags mean a hammer or a slide is the piece's choice:

```muz
use "std/instrument" as i;
let guitar = i.mono(sample([{path: "e-string.wav", root: 64, loop: [1.2s, 2.4s]}],
    {release_ms: 95}));
// Pick E, hammer F# and G, pull off to F#, pick A and slide up to B.
let run = phrase("E4:e F#4:e@h G4:e@h F#4:e@h A4:q B4:q@s");
let lead = i.slur(i.slur(run, "h"), "s", slide = 80ms);
song({tracks: [track("lead", lead, guitar)], tail: 0.5})
```

The tag is the score's slur marking for any monophonic line, not only on
`sample(...)`. The overlap exists only in performed time, so the score, `inspect`
and checks see notes that meet end to start (`checks.monophonic()` and
end-versus-start checks of one player pass), while the instrument receives a note
that starts while the previous key is held. An `sfz(...)` program then plays its
`trigger=legato` regions and keeps its own release and legato behaviour as its
SFZ file describes. A mono-legato plugin continues its voice. MIDI and DAWProject
exports carry the overlap, the usual legato spelling there. Don't write slurs as
`gate > 1`: that overlap is in score time, and checks report it as two notes
sounding at once.

```muz
use "std/instrument" as i;
// Bow C, slur to D, then a new bow on E, on a native SFZ cello.
let line = i.slur(phrase("C3:q D3:q@slur E3:q"), "slur");
```

## Zone readers and sampled instruments

A reader places a [sample map](instruments.md#samples-and-zones) inside a voice
graph so that you can add per-note filters, modulation, and custom envelopes.
The examples in this section require local recordings.

### Maps, selection, and storage

`s.reader(sample(zones,options),channel="mono",speed=1,options={})` places an existing
recording map inside a programmable voice. It preserves roots, key/velocity ranges,
zone gain, offset, loops, and note-addressed recording choices. Selection happens at
attack; tuning and filtering do not reselect. Every reader rotates its own alternates
in attack order, and paired left/right readers with the same map stay aligned.
`sample_zone` explicitly selects that index in every reader; it must be valid in each
map. Graph preparation checks every map's coverage, and graph budgets count its zones.
Decoded files share one per-patch budget across legacy `sample` and zone `reader`
nodes. Canonical paths are counted once, including separate left/right readers.
`sample_budget_frames` is an optional unitless integer on `voice_patch`: default
8,388,608 frames (64 MiB), configurable from 1 through 134,217,728 frames (1 GiB).
Each decoded frame stores two float32 channels, even for mono recordings. This is a
ceiling on retained sample storage, not preallocation, a process-wide memory limit,
or a limit on decoder scratch space, delay buffers, or other active patches.

```muz
use "std/signal" as s;
let recording = sample("strings.wav");
let envelope = s.adsr();
let strings = voice_patch("strings", {
    sample_budget_frames: 33554432, // allow up to 256 MiB of decoded recordings
    output: s.stereo_mul(s.stereo(s.reader(recording, "left"),
                                s.reader(recording, "right")), envelope),
    lifetime: {envelope: envelope, tail: 0s}
});
```

Preparation sums unique file frame counts from metadata before decoding and reports
required/allowed frames and bytes if they exceed the budget. Files whose metadata
omits a frame count are still bounded during decoding; a failure identifies the
file, total allowance and already-decoded frames. No decoding, allocation or budget
checks are added to the audio callback. Raising the budget admits larger recording
maps without changing resampling, selection or DSP behavior. Changing this setting
is structural and prepares a replacement; it is not an automatable patch control.
`inspect --view patch_detail` includes the effective frame budget.

### Amplitude and envelopes

Readers output zone-calibrated recordings. They do not inherit the sampler device's
amplitude envelope, shared gain, or velocity curve. The patch applies note expression
once, with mutable `velocity_track` (0–2, default 1) controlling its velocity exponent.
Set it to zero when the graph itself implements velocity amplitude. The source helper
`std/instrument.sampled(source,tone,options)` explicitly builds a stereo filtered
instrument using the source's attack/release times, gain and velocity response. Its
linear segment envelope is a source policy; it does not promise identical sustain-loop
or one-shot behavior to every standalone sampler configuration.

### Regions, looping, and playback speed

Reader options add `offset` (region start in seconds, added to each zone's initial playback offset), `end` (absolute file
seconds), and `loop_crossfade` (seconds, less than half each loop). Signal `speed` ranges
from -64 to 64; negative initial speed starts at the region's last frame and reverses.
Zero holds the playhead. Direction changes preserve its position; pitch and speed both
change playback duration. Interpolation taps wrap inside active loops. Crossfades
consume the overlapped beginning/end, shortening the repeated period by their length;
they are for clean sustained recordings, not pitch calibration of single-cycle waves.
A zone's initial playback offset may lie inside its loop; the loop may wrap before
that initial position, while remaining inside the reader region. Note-off exits a
non-one-shot loop and continues toward the region end. One-shot zones
keep looping until the patch's own lifetime/choke contract completes.

A reader can itself be the explicit `lifetime.envelope`: it completes at the region
end. An indefinitely looped reader needs a separate envelope-based completion contract.
Use `s.reader` signals with `std/instrument.crossfade(a,b,control)` for independently
controlled dynamic layers; alternates inside each layer retain round-robin semantics.
`std/instrument.releases(pattern,duration,timing)` builds a separate release-recording
lane at performed note ends, including gate and release offsets. Keep recording
selection for that release lane independent of the attack map.

## Node reference and limits

| Operation | Inputs |
| --- | --- |
| `param` | `value`, `min`, `max`; node ID becomes the parameter name |
| `frequency`, `velocity` | Current voice's frequency in Hz / attack intensity |
| `target_frequency` | Destination note frequency in Hz, before glide and tuning expression |
| `note_age` | Seconds since the sounding note began; restarts when a mono voice passes to a new note |
| `expression` | `kind`: volume, pan, tuning, vibrato, expression, brightness, pressure, or a declared custom note control; a note without a curve reads the [neutral value](performance.md#oscillating-curves) (brightness 0.5, vibrato and pressure 0) or the declared default |
| `osc` | `wave`: sine/saw/pulse/triangle; `ratio`, `detune` in cents, optional `hz`, `fm` in Hz, pulse `width`, `phase` in cycles |
| `noise` | Deterministic bipolar white noise, seeded from the device: two tracks playing one patch draw different noise, and a render repeats exactly |
| `adsr` | `attack`, `decay`, `release` in seconds; `sustain` 0..1; optional `one_shot:true` |
| `sum`, `mul` | `inputs` array of signals |
| `drive` | `input`, linear drive `amount`; tanh saturation |
| `filter` | `input`, `cutoff` Hz, `q`; `mode`: lowpass/highpass/bandpass/notch, or one-pole lowpass1/highpass1/allpass1 |
| `delay` | `input` (any node, so it can [close a loop](#feedback-loops-and-waveguides)), `seconds`, `feedback`; fixed `max_seconds` allocation; optional feedback `damping` and `max_feedback`; `interpolation`: linear/cubic |
| `mseg` | Bounded attack/release segment arrays; optional sustain endpoint and one-shot |
| `map` | `input`, `kind`: clamp/abs/reciprocal/exp2/log2/sqrt; clamp `min`/`max` |
| `hold`, `slew` | `input`; hold `rate_hz`, or slew `rise`/`fall` seconds |
| `shape` | `input`, static `points`, `quality`: adaa/raw |
| `resonator` | `input`, `frequency` Hz, `decay` amplitude T60 seconds |
| `reader` | Sample `source` (lowered to `zones`), channel, speed, region/loop options |
| `sample` | WAV/FLAC `path`, `root` pitch, optional `loop:true`; `channel`: mono/left/right |

A patch has 16 voices, at most 512 nodes, and at most 16 inputs to a sum/product.
A voice's storage and work grow with the nodes its patch uses.

A node runs only as often as its value can change, and the result is the same,
bit for bit, as running every node every frame:

- **Event rate.** `param`, `frequency`, `target_frequency`, `velocity` and
  `expression` change only at events: a note's start or legato transfer, a new
  expression value, a parameter change, or a frame of glide. A `sum`, `mul`,
  `map`, `drive` or raw `shape` of these and constants runs only when one of
  its own sources changes; a pressure curve's update does not rerun nodes that
  follow only the note's frequency.
- **Settling.** A `slew`, `filter` or antialiased `shape` of event-rate signals,
  and pure arithmetic of those, runs every frame until a frame leaves every
  output and state exactly as it was, then rests until one of its sources
  changes. A smoother that never reaches an exact fixed point keeps running.
- **Audio rate.** Everything else, and anything reading it, runs every frame
  at the oversampled rate. In a blown instrument that is most of the graph:
  the bore and reed loops, the noise, and whatever follows the breath
  envelope.

Measure a patch's cost with [`woodwind_callback`](#measuring-processing-cost).
Delay memory is limited to two seconds per voice, with at most one second per
delay. Feedback defaults to a bound of ±0.98; `max_feedback` can explicitly raise
that bound as described below. Sample storage defaults to 8,388,608 decoded stereo
frames and is configurable through `sample_budget_frames`.

An explicit scalar or stereo-pair output is required. Nodes read earlier nodes,
with one exception: a delay's `input` may be any node, which is how a graph
[closes a loop](#feedback-loops-and-waveguides). Any other forward reference
and unknown fields fail preparation. Dynamic frequency, filter, and envelope inputs have
bounded ranges. Native sample resampling and nonlinear saturation are not
oversampled mastering processors.

### Compiled patches

Preparing a patch also compiles its schedule to native code with
[Cranelift](https://cranelift.dev), on the thread that prepares the revision,
never the audio thread; the 443-node lattice prototype takes tens of
milliseconds. The compiled code computes what the interpreter
computes, in the same order and with the same floating-point operations and
libm functions, so a render is identical bit for bit either way, and the
interpreter remains the reference. It removes the interpreter's dispatch for
each node and keeps a frame's signals in registers. That matters most for the
audio-rate nodes of a physical model: the tone-hole lattice prototype costs
about 40% of its interpreted cycles, and a `std/woodwinds` instrument about
60%. Coefficients that follow a moving signal, such as a filter cutoff
following a slew, are still recomputed every frame, and those `tan` and `exp`
calls cost the same either way.

If code generation fails, or the process may not map executable memory, as
under a plugin host's hardened runtime or a W^X policy, patches run
interpreted and muz prints the reason once. To interpret every patch prepared
afterward, set `MUZ_JIT=off` (or `0`) in the environment; an embedding host
calls `muz::audio::set_patch_jit(false)`. A build without the `jit` Cargo
feature, which is on by default and enabled by the CLAP plugin, interprets
every patch and does not link Cranelift.

### Oversampling

`oversample: 2` or `4` on a `voice_patch` runs the whole voice graph at that
multiple of the output rate and brings the sum of its voices back down through
lowpass filters. The last stage is flat to 20 kHz (at 48 kHz output) and about
85 dB down from the output's Nyquist frequency, so nothing between 24 kHz and
the graph's own Nyquist frequency folds back; at `oversample: 4` a 47-tap
half-band stage comes first. Partials above the graph's own Nyquist frequency
still fold inside the graph, which is what a higher setting reduces. Use it where a
nonlinearity makes partials above half the sample rate, which fold back as
inharmonic tones: `drive`, a raw `shape`, a clamp, and above all a nonlinearity
inside a [feedback loop](#feedback-loops-and-waveguides), where ADAA does not
apply. The graph's times and frequencies mean the same at any setting, so a patch
keeps its pitch, envelopes and filter cutoffs; the cost is the factor in CPU and
delay memory, and under a millisecond of latency (33.5 output frames at 2,
39.25 at 4). Filters may open up to 0.45 of the raised rate, `noise`
is white up to half of it (so less of it lands in the audible band), and a
loop's shortest delay is shorter. It is structural: changing it replaces the
device. `merge(patch, {oversample: 2})` applies it to a patch built elsewhere.

### Mapping and smoothing

`map` consumes `input` and a `kind`: `clamp` (with signal `min`/`max`), `abs`,
`reciprocal`, `exp2`, `log2`, or `sqrt`. Reciprocal returns zero within ±1e-20 of zero;
exp2 bounds its exponent to ±100; log2 floors its argument at 1e-20; sqrt of a
negative input is zero. These explicit
operations retain tuning precision without baking pitch policy into each processor.
`std/signal.octaves(base,amount)` and `period(frequency)` compose them.

`hold` captures its input immediately, then at `rate_hz` (bounded 0..sample rate).
`slew` starts at its first input and smooths with separate `rise`/`fall` time
constants in seconds (0..60; zero is immediate). After one time constant the
remaining difference is about 36.8%. `s.drift()` composes noise, hold and slew.
These states reset per voice; shared score motion uses ordinary automation.
Noise uses a patch-wide deterministic stream, so changing graph/voice
evaluation order may change a drift realization.

### Filters and oscillator phase

Graph filters expose `bandpass` (the SVF band state, gain Q at its center) and
`notch` (input minus the damped band), alongside lowpass and highpass modes.
`lowpass1`, `highpass1` and `allpass1` are one-pole (6 dB per octave) filters
that ignore `q`; `std/signal` names them `lowpass1(input, cutoff)` and so on.
They are trapezoidal, so a `lowpass1` delays a low partial by `1/(2*pi*cutoff)`
seconds at any sample rate, which is what a tuned loop needs to know.
Oscillator `phase` is an offset in cycles. Sine accepts a signal for phase modulation;
other waves currently accept a constant phase only. This is neither hard sync nor a
promise of alias-free arbitrary modulation. In particular, changing a discontinuous
wave's phase requires more than its ordinary base-frequency BLEP correction.

### Transfer curves

`shape` prepares 2–64 increasing `[input,output]` points into a 2049-entry linear
transfer table. Values outside its domain hold the endpoint output. Default
`quality:"adaa"` evaluates the exact integral of that prepared interpolant between
successive inputs (first-order antiderivative antialiasing). `quality:"raw"` performs
ordinary lookup, useful for control mappings or deliberate nonlinear artifacts.
ADAA reduces aliasing but has a small averaging/phase effect and does not eliminate
all aliases; raw and ADAA need not null.

### Damped delays

Delays optionally take signal `damping` in Hz for a one-pole feedback lowpass, and
static `max_feedback` (0–0.99999). The default is 0.98 with no damping.
`s.damped_delay` opts into the expanded bound. Seconds remain actual delay time;
`s.period(frequency)` produces a nominal comb period. Loop-filter phase and fractional
interpolation alter ringing pitch and decay, so this is not a calibrated string model.

`interpolation: "cubic"` reads the line with a third-order Lagrange interpolator
instead of the default two-point one. A two-point read is a lowpass whose depth
depends on the fractional part of the delay, so in a loop the brightness and decay
of a note vary with its pitch; the cubic read keeps them even and holds tuning. It
needs a delay of at least two samples (linear needs one), and costs four reads.

## Feedback loops and waveguides

A delay only ever reads what was written on earlier frames, so its `input` may be
a node that comes later in the graph, including one computed from the delay's own
output. That one edge is how a graph feeds back. Everything else in the loop is
ordinary nodes, so a filter, a `shape`, a clamp or a product can sit inside it.
The engine writes every delay line after the frame's nodes have run; a loop
through a delay of `seconds` is exactly that long, with no extra sample.

In a flat patch, name the later node:

```muz
{id: "bore", op: "delay", input: "into", seconds: 0.002, interpolation: "cubic"},
{id: "back", op: "mul", inputs: ["bore", -0.95]},
{id: "into", op: "sum", inputs: ["breath", "back"]}
```

A nested record cannot contain itself, so nested signals close a loop by naming
the line and its input in the patch's `feedback` list. `s.line(seconds)` makes a
delay with no input yet (cubic, 50 ms of capacity unless `options` say otherwise):

```muz
use "std/signal" as s;
let bore = s.line(s.mul([s.period(s.frequency()), 0.5]));
let back = s.mul([s.lowpass1(bore, 2500Hz), -0.95]);
let burst = s.mul([s.noise(), s.adsr(0s, 10ms, 0, 10ms, true)]);
let hold = s.adsr(0s, 10ms, 1, 300ms);
voice_patch("tube", {
    output: s.mul([bore, hold]),
    feedback: [[bore, s.add([burst, back])]],
    lifetime: {envelope: hold, tail: 0s}
})
```

That is a tube closed at one end: the wave returns inverted after half a period,
so it rings at the note and its odd harmonics. Several lines can feed each other;
each `feedback` entry is `[line, signal]`, and a line may appear once.

Three things are the patch's responsibility:

- **Stability.** Nothing bounds a loop's gain. Keep the round trip below 1, or
  put a clamp or `shape` inside it. A value that stops being finite becomes zero.
- **Tuning.** A loop sounds at the reciprocal of its whole delay: the line plus
  the phase delay of every filter inside it. Shorten the line by what the filters
  add; `std/physical.lowpass1_delay(frequency, cutoff)` and
  `highpass1_advance(frequency, cutoff)` are those times as signals.
- **Budget.** Each line counts toward the two seconds of delay memory per voice.
  The pitch is read every sample, so tuning curves, vibrato and glide all move
  the line smoothly.

### Probes

A loop's output alone rarely says why a note failed to speak. `probes` names
inner signals that a render can capture:

```muz
voice_patch("tube", {
    output: s.mul([bore, hold]),
    feedback: [[bore, s.add([burst, back])]],
    probes: {bore: bore, back: back, level: s.slew(s.mapping(bore, "abs"), 1ms, 50ms)},
    lifetime: {envelope: hold, tail: 0s}
})
```

`muz render song.muz --tap clarinet@bore -o bore.wav` writes the `bore` signal
of the track `clarinet` (a kit piece is `drums.kick@name`); repeat `--tap` for
several probes, tracks or buses from one pass, and `--list-taps` lists every
probe. A probe is heard raw: the signal itself summed over the track's voices,
the same on both channels, before the patch's gain, velocity, `volume`, pan and
the track's inserts, fader and sends. It has no `:post` point.

Declaring probes does not change what the track sounds like. Nodes used only by
probes come after every sounding node and run only while a render listens to
one; a `noise` among them draws from its own generator. A probe tap renders a
muted copy of the track with the same notes, controllers and instrument
automation, so its voices start, steal and end exactly as the track's do.
Probe nodes count toward the 512-node limit.

## Blown instruments

`std/woodwinds` is a family of five modeled winds, each a waveguide from
`std/physical` with a reed or an air jet inside its loop, calibrated across its
range and voiced against recorded instruments (Sonatina and VSCO 2). Write notes
and marks; `play` blows them as a player would:

```muz
use "std/woodwinds" as w;
let timing = {tempo: 66};
let line = phrase("D4:h@dal_niente A4:q@slur D5:q@slur E5:e@staccato F5:e@staccato G5:h@niente").gain(0.5);
song({tempo: 66, tracks: [track("cl", w.play(line, w.specs.clarinet, {timing: timing}), w.clarinet())]})
```

[wind-solo.muz](../examples/wind-solo.muz) is a solo line to start from,
[wind-ensemble.muz](../examples/wind-ensemble.muz) five winds seated in a hall,
and [wind-phrasing.muz](../examples/wind-phrasing.muz) one line played first
by the note rules alone (`perform`) and then by the player (`play`).

| Instrument | Model | Range | Registers |
| --- | --- | --- | --- |
| `flute()` | `flue`: an open pipe and an air jet | C4 to C7 (60–96) | overblows by the jet; `lip` |
| `oboe()` | `cone`: a narrow cone, a stiff double reed, two formants | B♭3 to G6 (58–91) | octave vent from E5 (76); C♯6 up on its own tube |
| `clarinet()` | `cylinder`: a closed cylinder and a single reed | D3 to G6 (50–91) | register vent from A4 (69): the clarion is the third mode |
| `bass_clarinet()` | `cylinder` | B♭1 to G5 (34–79) | register vent from A3 (57) |
| `bassoon()` | `cone`, three formants | B♭1 to E♭5 (34–75) | one tube |

Each returns a monophonic instrument (`std/instrument.mono`): a note that
overlaps the one before continues the voice, which is a slur.

### Register transitions

`clarinet({register_blend: true})` selects a transition variant with two
independently ringing register loops whose outputs crossfade at the break.
Neither delay is swept to a new length during a slur. It is a stopgap: two
instruments blended, not a model of the register vent, and it costs a second
bore loop. The default clarinet remains the single variable-length loop,
whose register vent closes in 0.4 ms on a descent to limit the chirp as the
bore shortens.

A fixed-bore clarinet, with one reed and fixed segments whose tone holes and
register vents open and close, is a prototype in
[examples/woodwind-prototypes](../examples/woodwind-prototypes/README.md).
It is not part of `std` until it keeps the lower bore attached below an open
hole and honors the player's tuning.

### Experimental headjoint load

`flute({headjoint: {frequency: 3800Hz, q: 0.9, strength: 0.08, radiation: 0.15}})`
adds a damped local return at the embouchure and radiates a share of that
component. These are effective parameters for a lumped side branch.
Measured input admittance is needed to fit a particular headjoint.

### Experimental double-reed staple recovery

`oboe({staple_recovery: 0.4})` and `bassoon({staple_recovery: 0.4})`
model some of the pressure regained as forward flow expands through the staple.
The coefficient is a normalized fitting parameter from 0 to 1, with zero as
the default. The recovery term is proportional to squared forward flow and
is folded into the prepared reed tables. Reverse flow uses the base law.
Use flow and pressure measurements to fit a particular reed and staple.
[Almeida, Vergez and Caussé, equation 17](https://arxiv.org/abs/physics/0607011)

### Measuring processing cost

To measure processing cost, run
`cargo run --release --example woodwind_callback -- SONG.muz 10` with a score
that keeps the intended voices active. This measures the engine's real-time
processing path at 48 kHz over 64, 128 and 256 frame blocks. It reports the
audio thread's user-mode cycles per block as megacycles per second of audio
(mean and 99th percentile, and the mean as a share of a 4 GHz core), then
wall-clock percentiles and deadline overruns. Cycles stop counting while the
thread is descheduled, so they stay comparable on a loaded machine; wall-clock
time does not. A third argument sets the threads that process tracks (1 is
serial; by default the live engine [shares tracks among
workers](workflow.md#parallel-rendering)). Audio-driver and scheduling latencies
require a device-level measurement. Prefix `MUZ_JIT=off` to measure the
interpreter instead of [compiled patches](#compiled-patches). Cycles still
vary by a few percent with the machine's load, since a busy sibling hardware
thread slows the measured one: compare runs made close together.
`examples/benchmarks` holds a sustained note for each `std/woodwinds`
instrument.

### What a note asks of the player

- **Dynamic.** Velocity is the written dynamic: 0.2 ppp, 0.5 mf, 1 ff, and about
  0.1 air without tone. It becomes a mouth pressure through the instrument's
  measured speaking threshold, so mf is the same distance above threshold at
  every pitch, and harder blowing changes the colour as well as the level. The
  `pressure` expression adds to it; a note's `velocity_breath` control (1 by
  default) weights the velocity, and `perform` sets it to 0 and writes the
  whole breath as `pressure`.
- **`tongue`** (0..1, 0.4 by default): the burst of pressure and air that starts
  a fresh voice. A slurred note continues the voice and has none.
- **`noise`** (0.5 neutral): the breath turbulence. It is never white: its
  spectrum falls away above a corner that rises with the dynamic and the
  register (`std/physical.turbulence`), and it grows from p to f.
- **`flutter`** (0..1): flutter-tonguing, the breath chopped at about 24 Hz.
- **`vibrato`** (0..1): the depth of a breath pulse and pitch swing at the
  instrument's rate (flute 5 Hz, 11 cents and a 10 % breath pulse at full depth;
  oboe 5 Hz, 7 cents; bassoon 4.1 Hz, 4 cents; the clarinets have none).
  **`vibrato_rate`** (0.5 neutral) scales that rate from 0.81 (at 0) to 1.23
  times (at 1).
- **`brightness`** (0.5 neutral): the embouchure. Reeds loosen or tighten the lip
  on the reed; the flute aims its jet further off or nearer the edge.
- **`lip`** (flute, 0.5 at rest): the jet's length. Lower sharpens, and near 0.25
  the note overblows the octave.
- **`vent`** (reeds, 0..1): opens the register vent on the note's own fingering.
  A curve that opens it during a note is a slur up a register. The clarinet
  family closes its automatic register vent quickly on a descent across the
  break to limit the chirp as the bore shortens.

Every player's pitch wanders a few cents and the fingers add a little air at
each change of note, slurred or not (`note_age`). Two tracks playing the same
instrument breathe and wander independently: a voice patch's noise is seeded
from its track.

### Playing rules

`perform(pattern, spec, options)` turns each written note into curves of
breath, tongue, air, vibrato, flutter and level, the same rules for every
instrument with the numbers of its spec's `style`. Marks are tags:

| Tag | Played as |
| --- | --- |
| `slur` | continues from the note before: no tongue, the breath carried over, the notes overlapped |
| `staccato` | half the length, a lighter, livelier tongue |
| `accent`, `hard` | a pressure kick at the start; `hard` tongues it hardest |
| `soft` | a legato tongue: the breath swells into the note |
| `tenuto` | held to the next note, no taper |
| `niente`, `dal_niente` | the level falls away to nothing, or grows from air; the breath stays above the speaking threshold |
| `flutter`, `breathy` | flutter-tongued; an airy tone |

A phrase's last note (a rest or nothing after it) tapers, and notes on
instruments with vibrato that last more than 0.6 s get one that starts late and
grows, narrower when soft. Options:

- `timing`: the song's tempo record, for the notes' lengths in seconds.
- `dynamics`: `[[position, dynamic], ...]` in score time, for hairpins through
  notes, or a function of score position; without it each note holds its
  velocity, moving toward the next one's when it slurs into it. A point every
  bar of a long part is cheap: `play` evaluates the points as one curve for the
  whole line, and `perform` searches them (`w.interp`). Prefer points to a
  function, which is called several times a note. In a piece with meter
  changes, `w.in_bars(points, meters, meter)` takes `[[bar, dynamic], ...]` with
  bars counted from 1 as the song numbers them (fractional within a bar) and the
  song's `meters`, and returns the points in score time, straight in bars (with
  a point at each meter change between them):

  ```muz
  use "std/arrange" as arr;
  let meters = arr.meter_changes([[8, [4, 4]], [1, [7, 8]], [8, [4, 4]]]);
  w.perform(part, w.specs.oboe, {timing: timing, dynamics: w.in_bars([[1, 0.3], [9.5, 0.8], [18, 0.4]], meters)})
  ```
- `tags`: renames marks, such as `{slur: "s", staccato: "st"}`.
- `soft`: `true` for legato tonguing throughout, or a function of the note
  (`fn(n) => n.at >= 32b`).
- `level`: scales each note's `volume` curve: a record of tags as written on
  the notes to factors (`{last_chord: 0.5}`, multiplied when several apply), or
  a function of the note.
- `humanize` (5 ms) and `seed`: each part's attacks wander a few milliseconds of
  their own; slurs overlap by more, so they survive.
- `rule`: replaces the per-note rule. It is `sounding(note, context)` by default,
  ordinary source to copy and change. Its context has `has(note, mark)` and
  `marks`, the note's own marks as booleans (`w.marks_of`), worked out once.

Breath is not a fader. A beating reed saturates: once it speaks, more pressure
changes its spectrum far more than its level, so each spec adds the rest of a
dynamic as gain (`level_db`, from p to f). And a note ends above its threshold:
lowering the breath through it stops a reed within a few tenths of a second,
which sounds like a cut. The rules hold the breath above it and fade the level;
a part written by hand should do the same with `volume`.

### The player

`play(pattern, spec, options)` is the recommended way to perform a wind part. It
plays the same marks with `perform`'s articulation rules, but as a player who
has read the whole line first. It reads one monophonic line and works in two
passes. Its output is only ordinary note data: attack offsets, releases, and
per-note `pressure`, `tongue`, `noise`, `tuning`, `brightness`, `vibrato` and
`vibrato_rate` curves. No phrase logic runs in the audio graph.

**The phrase plan.** Each spec has a `player` profile with an air budget: how
many seconds of mezzo-forte playing a full breath lasts (flute 9, clarinet 18,
bass clarinet 13, bassoon 16), how fast a louder note spends it (`flow`), and,
for the oboe and bassoon, how long air can be held before it is stale (oboe
15 s, bassoon 25 s; their breath lets the old air out before taking new). Air
is spent only while a note sounds: a staccato note spends half its length.
The player breathes:

- at every rest of at least `rest_breath` (0.3 s), and at a `breath` mark;
- otherwise at the best place before the air runs out. Places rank as a
  listener groups a line: a `phrase` mark, a long note before shorter ones, a
  note of at least a beat on a strong beat, a leap. A later place wins a tie,
  so the player uses the air it has. A slur is broken only when nothing else
  is left, and `no_breath` (or `data.play.breath: false`) forbids a place.

A breath's time is the silence after the note (a rest, or the silent half of
a staccato), up to `take` (0.35) of the note's sounding length, and the next
entry's lateness: `late` (15 ms), or up to `late_max` (70 ms) when the note is
too short to give the time. A full breath lets out the air that has gone stale
and fills what is spent: it takes `exhale` times the part of `stale` used plus
`inhale` times the part of the lungs then empty, so a player who has spent
little air tops up quickly. A shorter breath is the same breath scaled down, an
exhale and a quick sip, and renews that part of both the air and the stale air
(its `refill`). An oboe's full exchange (0.25 s out, 0.3 s in) takes about half
a second, so the 0.3 s breath pauses measured in running reed playing renew
about half of it, and a quarter-note join at crotchet = 72 about three
quarters. When a breath is for stale air and an exhale alone would renew more of
it, the player only exhales, keeping at least `exhale_keep` (0.5) of the air.
A silence that is no breath (an articulation gap, a rest too short to breathe
in) gives no air, but the player lets a little out: it counts `relief` (0.5)
less toward stale air. Near the end of a breath the tone gets more air, more
support and a slight waver.

**Phrases** are where a listener hears them, not where the player happens to
breathe. Each join of two notes gets a boundary strength (`data.plan.boundary`
on the note before it), the sum of these cues, weighted by `phrasing`:

| Cue | Weight | Counts |
| --- | --- | --- |
| `rest` | 1 | a rest, in beats, times how far it differs from the rests at the joins either side (so an ostinato's regular rests count nothing), at most 4 |
| `ioi` | 0.5 | a time from attack to attack longer than its neighbours', in beats, the same way |
| `leap` | 0.25 | a leap of a fourth or more, in octaves, among smaller intervals |
| `cadence` | 0.75 | a note of at least a beat on the downbeat or mid-bar, at least 1.5 times as long as the one before |
| `dynamics` | 0.75 | a step in the written dynamic between the note's middle and the next attack (full at 0.15) |
| `articulation` | 0.25 | the end of a slur, or staccato to legato or back |
| `meter`, `tempo` | 1, 1 | at a bar line, a new meter, or a tempo 5 % or more away: a new section |
| `hyper` | 0.6 | a bar line that starts a group of four bars, counted from the last change of meter |
| `mark` | 1 | a written `breath` mark |
| `breath` | 0.5 | a breath the player plans there |

Beats are the meter's counted beats (a dotted quarter in 6/8, 9/8 and 12/8), so
a jig's dotted-quarter cadence rest ends a phrase at any tempo. A join inside a
slur scores -10 and is never cut; a `phrase` mark, or a rest of `rest_beats` (2)
beats or more, is decisive and always cut. Among the joins of at least 0.5 that
are stronger than both neighbours, the player chooses the set of cuts that
earns most: each cut earns its strength less `threshold` (1.25), and a phrase
costs nothing from `min` (2.5 s) to `max` (16 s) and more the further it falls
outside (a phrase under `min` only after a decisive cut). A breath alone ends
no phrase; a long phrase is split at its best weaker boundary. Change any of
these with `phrasing: {...}`.

Each phrase is given an arch of breath around the written dynamic: `arc` (10 %)
under it at the start, `arc` over it at the peak, and 1.3 times `arc` under it
at the end. The peak is the highest, longest, loudest note on a strong beat, or
a `peak` mark. Each slope stays within `arc_reach` (4 s) of the phrase's start,
peak or end, so a long phrase plays at its written dynamic in between rather
than sinking along a slow fall; the written dynamics shape the larger span.

**The note rules** see `{prev, next, interval, interval_in, crosses_break,
breaks_in, phrase_pos, phrase_first, phrase_last, metric_weight, note_rate,
air_left, breath_before, breath_after, intensity, run, repeat, tuning,
slurs_on}` beside
`sounding`'s context. `phrased(note, context)` is the default rule:

| Rule | Played as |
| --- | --- |
| Tonguing | firmer at a phrase's start and on strong beats; lighter on a repeated note and as notes pass `tongue_fast` per second. A flute double-tongues runs of `double_rate` tongued notes per second: each ku is weaker, darker and 3 ms late |
| Leaps | the breath rises into an upward leap of a fourth or more and supports its landing. A leap up across a register break (`breaks`) or wider than a fifth starts a little brighter. A slurred octave leap lands `leap_late` (8 ms) late, half that tongued or downward. A tongued leap is separated by `leap_gap` more |
| Intonation | each note starts at the profile's tendency: `register` cents by pitch, `dynamic` cents per unit of dynamic from mf, and `scatter` cents of its own. After `correct` (120 ms) the lip pulls it in over `settle` (150 ms), leaving `keep` (0.3) of it |
| Vibrato | depth, onset and rate follow the phrase's intensity, and the rate quickens through a long note. A note needs at least `vibrato_min` (0.45 s, longer when the phrase is calm) to carry it, and it dies away on a phrase's last note |
| Timing | a slow drift shared by the whole ensemble (`ensemble: {seed, drift: 7ms, scale: 3}`), the chair's own (`drift` 4 ms over `drift_scale` 1.5 s), 1 ms of motor `jitter`, and the chair's `lead`. A phrase slows into its end: its last note comes `final` of `final_span` late, about 48 ms. Before a rest its last note is held `hold` (12 %) longer, at most `hold_max` (0.3 s) and half the rest, never past the next bar line or the part's next entry, and `hold_section` (half) as long before a rest into a new section (the part waits past the bar line, or stops) |

The timing is correlated noise, not `humanize`'s independent jitter. Parts that
share `ensemble.seed` (0 by default) move together. Give each part its own
`seed`, and a lead voice a small negative `lead`. In an ensemble, write a shared
ritardando in the song's tempo map; set `final: 0` where parts should not slow
on their own.

Options are `perform`'s (`timing`, `dynamics`, `tags`, `soft`, `level`, `seed`)
plus the ones above, `meters` and `meter` for beat weights, `player` (changes
to the spec's profile, such as `{air: 6}`), and `rule`. A note's own
`data.expression` still wins over the player's curves. A note's `data.play`
record overrides the player for that note: `breath` (true forces a breath
after it, false forbids one), `tongue` (a factor), `tuning` (cents, instead of
the tendency), `vibrato` (a depth factor; 0 for none) and `shift` (extra
attack offset).

```muz
let parts = {fl: phrase("A5:h G5:q@slur F5:q@slur E5:h@phrase G5:h A5:w"), ob: phrase("F5:h E5:q@slur D5:q@slur C#5:h E5:h F5:w")};
let fl = w.play(parts.fl, w.specs.flute, {timing: timing, seed: 1, lead: 0ms - 4ms});
let ob = w.play(parts.ob, w.specs.oboe, {timing: timing, seed: 2, player: {intonation: {dynamic: 0}}});
```

**What cannot be played.** The player refuses material that no player could
play as written. Its limits are the profile's `limits` and the spec's `range`:

| Code | Found when | Limit (std profiles) |
| --- | --- | --- |
| `range` | a note is outside the spec's `range` | the instruments table above |
| `air` | a single note, or a stretch with nowhere to breathe, outlasts a breath | `air`, `flow` |
| `slur_breath` | the only breath the line allows breaks a slur, or a `no_breath` passage | `air`, `flow` |
| `breath_time` | a needed breath renews less than `refill` (0.25) of what it has to | `inhale`, `exhale`, `take`, `late_max` |
| `tonguing` | `run` (4) or more tongued notes come faster than the player can tongue | `tongue` 8 a second (bass clarinet 7); `double` flute 13, bassoon 11, oboe 10 |
| `slurred` | `run` or more slurred notes come faster than fingers can move | `slurred` 14–20 a second |
| `crossing` | two or more slurs in a row cross a register break too fast | `crossing`: clarinet 7, bass clarinet 5, others 10–14 a second |
| `slur_leap` | a slur is wider than `slur_leap` semitones, or wider than `fast_leap` (12) with under `leap_time` (0.08 s) before it | `slur_leap`: oboe 19, flute 26, clarinet 27, others 24 |

The limits are pedagogical estimates for a strong professional, not
measurements. Tune them per piece with `player: {limits: {...}}`. By default
(`strict: true`) the part fails evaluation. The error lists every finding with
the part (`part`, the spec's name by default), the bars and beats, the notes,
the limit and what was asked:

```text
unplayable as written; pass `accept` (or tag the notes `superhuman`) to play it anyway, or `strict: false` to warn:
  flute, bar 1 beat 1 to bar 5 beat 1: the slur from D5 to A5 lasts 10 s and needs a breath after 6 s, between F5 and G5; a breath lasts about 6.4 s at this dynamic
  clarinet, bar 1 beat 1 to bar 1 beat 4.75: 16 tongued notes at 9.3 a second; single tonguing reaches about 8
```

To keep such a passage, a deliberately superhuman one say, accept it:
`accept: true` for the whole part, `accept: ["tonguing", "range"]` for some
codes, a `superhuman` tag on the notes (a finding touching a tagged note is
accepted), or `data.play.accept` (true or a list of codes) on a note. With
`strict: false` the player warns instead and plays its best plan. Add the track
check `w.playable()` (`level` "warning" or "error") to report the findings in
`muz check` at their bar and beat. `w.unplayable(played)` lists them,
accepted ones included.

Each played note keeps its plan in `data.plan`: `phrase`, `boundary` (the
strength of the join after it, 1000 if decisive, null on the last note), `air` (the part of
the breath left at its end), `peak` and `breath` (the breath after it, or
null: `kind` "rest", "mark", "planned", "slur" or "exhale" (an exhale alone),
`why` (the limit it was for: "air", "stale", "rest" or "mark"), `refill` and
`seconds`). A finding names the limit that binds: how long a breath lasts at
the dynamic, how long air may be held before it is stale, or the time a useful
breath needs, with as many decimals as show the shortfall.
The player leaves the time for each breath but does not voice it.

The profiles' numbers are estimates, so tune them per piece. The rules draw on
performance research and pedagogy:

- Breath groups and their ranking, and the phrase boundaries' cues: the
  grouping preference rules of Lerdahl and Jackendoff, *A Generative Theory of
  Tonal Music* (1983).
- Phrase boundaries: Cambouropoulos, "The Local Boundary Detection Model
  (LBDM) and its application in the study of expressive timing", *ICMC* (2001),
  for each interval weighed by its size and its change from its neighbours;
  Temperley, *The Cognition of Basic Musical Structures* (2001), ch. 3, for
  choosing the best set of boundaries by dynamic programming with a preferred
  phrase length, and for metrical parallelism.
- Air: Fuks and Sundberg, "Blowing pressures in bassoon, clarinet, oboe and
  saxophone", *Acta Acustica* 85 (1999), for the oboe's small flow, and
  Fletcher, "Acoustical correlates of flute performance technique", *JASA* 57
  (1975), for the flute's.
- Breathing: Fuks and Sundberg, "Respiratory inductive plethysmography
  measurements on professional reed woodwind instrument players", *KTH
  TMH-QPSR* 39 (1998): breath groups end at 14–52 % of vital capacity, oboists
  rarely below the resting volume, and breath pauses in running music last
  about 0.3 s. Fuks, *From Air to Music* (KTH, 1998), on exhaled CO2 rising the
  longer air is held. Carr, "Exercises for oboists", *The Instrumentalist*
  (2012): oboists usually exhale before inhaling, and mark where to exhale and
  inhale, only inhale, or only exhale.
- The phrase-final hold: the KTH Final-ritard and Punctuation rules put a
  phrase's lengthening mostly in its timing, and Punctuation leaves a note
  before a rest alone; Penel and Drake, "Sources of timing variations in music
  performance", *Psychological Research* 61 (1998), measure phrase-final
  lengthening as later onsets. Band pedagogy releases a note into the rest
  that follows it, not over the next entry.
- The phrase arch and final slowing: the KTH rule system (Friberg, Bresin and
  Sundberg, "Overview of the KTH rule system for musical performance",
  *Advances in Cognitive Psychology* 2, 2006), whose Phrase-arch rule works on
  phrases and subphrases as a deviation from the nominal level (Friberg,
  "Matching the rule parameters of Phrase arch to performances of
  Träumerei", *KTH STL-QPSR* 36, 1995), and Todd, "The dynamics of dynamics:
  a model of musical expression", *JASA* 91 (1992). The final ritard follows
  Friberg and Sundberg, "Does music performance allude to locomotion?", *JASA*
  105 (1999).
- Correlated timing: Hennig et al., "The nature and perception of fluctuations
  in human musical rhythms", *PLoS ONE* 6 (2011), and Wing and Kristofferson's
  timekeeper and motor delay (*Perception & Psychophysics* 14, 1973).
  Ensemble asynchrony: Rasch, "Synchronization in performed ensemble music",
  *Acustica* 43 (1979).
- Intonation: a flute's pitch rises with its jet's speed (Coltman, "Resonance
  and sounding frequencies of the flute", *JASA* 40, 1966). A pitch correction
  comes about 100–150 ms after the error is heard (Burnett et al., "Voice F0
  responses to manipulations in pitch feedback", *JASA* 103, 1998).
- Vibrato: Gärtner, *The Vibrato* (1981), on flute vibrato, and Prame,
  "Measurements of the vibrato rate of ten singers", *JASA* 96 (1994), on a
  rate that rises through a tone.
- Double tonguing: Taffanel and Gaubert, *Méthode complète de flûte* (1923).

The player costs about 2,000 evaluation steps a note with the song's `meters`
and a dynamics curve of points (on a long line with marks on most notes; about
2,200 on a lyrical line with vibrato, a `level` record, a `soft` function and a
phrase every few notes), about two and a half times `perform`. One
evaluation's 5,000,000 steps hold about 2,300 played notes. Searches and noise
that apply to the whole line (the meters, the dynamics points, the timing
drift, the phrase arches) are each one `curve_value` call, so their cost no
longer grows with every note. Finding the phrases costs about 100 steps a note:
the boundary cues for each join outside a slur, and the choice of cuts, which
visits only the candidate joins. A dynamics
function is still called three times a note: prefer points (`w.in_bars` for
bars across meter changes).

### Specs, derived instruments and calibration

An instrument is a spec record in `w.specs`: its model and bore (`kind`,
`apex`, `lattice_hz`, `track` or `track_curve`, `loss`, `formants`, the
registers' `clarion`, `octave_key` and `key_top`), its reed (`tight`, `loose`,
`embouchure_dynamic`, `gamma_max`) or jet (`offset`, `register`, `stiff`,
`pressure_rise`, `vortex`), its breath (`noise`, `noise_hz`, `noise_brighten`,
`noise_dynamic`, `hiss`, `finger`, `drift_cents`), its vibrato and playing
`style`, its `range`, its `calibration` and, where the model's pitch moves with
the dynamic, `cents_per_dynamic` or a `dynamic_cents` curve. `patch(spec, overrides)` is its voice
patch and `instrument(spec, overrides, glide)` the monophonic instrument.

Another instrument is a changed spec: `derive(spec, changes)`. An English horn is
an oboe a fifth lower with a darker bulb; an alto flute is a flute a fourth
lower; a contrabassoon a bassoon an octave down. Once its
bore or range changes, recalibrate it. `tools/woodwinds.py` renders the model
note by note, measures where it starts to speak over its range and its tuning,
and writes the tables as a `.muz` file to pass as the spec's `calibration`:

```sh
uv run tools/woodwinds.py calibrate english_horn --use my/winds.muz:my \
    --spec my.english_horn --threshold cone --cal-out my/english_horn_cal.muz
uv run tools/woodwinds.py compare english_horn --use my/winds.muz:my \
    --spec my.english_horn --recordings "path/to/Cor Anglais"
```

A spec may carry `body_eq`, the spectral envelope its model lacks against the
recordings (`[[hz, dB], ...]` by third octave, measured by `tools/woodwinds.py
body NAME`). `w.body(spec)` turns it into a track chain, a 255-tap linear-phase
convolution (`std/physical.response_ir`, 2.6 ms of delay), or nothing:

```muz
track("bsn", line, w.bassoon(), {chain: w.body(w.specs.bassoon)})
```

The bassoon's is std's: it takes its brightness error from 0.23 to 0.21 octaves
and its second and third partials' from 11.9 to 11.2 dB. The oboe's measured
envelope improved its partials but not its brightness, so it has none.

`check` tabulates a model across its range and dynamics (pitch error, partials,
spectral centroid over the fundamental, the breath floor between partials by
band, rise and release); `compare` sets it beside recordings at their pitches and
reports the brightness error in octaves, the partials' error and the floor's
level and slope; `summary` does all five; `vibrato` plays long notes through the
rules. `reference` measures the recordings once (contrib's Sonatina and VSCO 2,
or `--recordings DIR`). std's own tables are in `std/woodwind_calibration.muz`,
written by `calibrate NAME` with no `--cal-out`.

Measured against the recordings at their pitches and dynamics (rms over notes;
the recordings themselves disagree by about 0.3 octaves of brightness between
libraries):

| | Flute | Oboe | Clarinet | Bass clarinet | Bassoon |
| --- | --- | --- | --- | --- | --- |
| Brightness error, octaves | 0.32 | 0.26 | 0.48 | 0.39 | 0.23 |
| H2/H3 error, dB | 7.9 | 9.8 | 12.8 | 12.2 | 11.9 |
| Breath floor 1–2 kHz, 8–16 kHz, dB | −8, −9 | −4, −2 | −5, −12 | −4, −16 | −2, −7 |
| Floor slope error, dB | −1 | +2 | −7 | −12 | −4 |
| Worst tuning at mf across the range, cents | 2.5 | 1.6 | 1.8 | 1.6 | 1.6 |
| Worst tuning at ff across the range, cents | | | 6 (G♯4; others 3) | 3 | |
| Level, bottom to top at mf, dB | +12 | +7 | +8 | +10 | +7 |
| Lowest note, p to f: brightening (octaves), level (dB) | 0.4, 10 | 0.3, 10 | 1.8, 18 | 3.0, 25 | 0.5, 12 |

The breath is voiced close and dry: between the partials it sits several dB
under the recordings, whose floor also holds a room and distant microphones (a
clarinet's floor at 1–2 kHz is the room's), and at their level a dry
instrument's air reads as a blanket of noise. `w.far(spec)` puts back the
recordings' brighter air (each spec's `far` record), for an instrument heard
from a distance or a comparison with them.

A model costs about a fifth of real time per voice at the default
`oversample: 2`. Below 16 kHz nothing folds back by more than −60 dB except the
clarinets' top notes at ff (−50 to −53 dB): partials of the beating reed above
48 kHz fold inside the 96 kHz loop. `oversample: 4` takes them under −62.

### Parts

`std/physical` holds the parts, so a piece can build its own instrument from
them (see [woodwind-models.muz](../examples/woodwind-models.muz)):

- **`bernoulli(h, embouchure, tight, loose)`**: the exact quasi-static reed. With x
  the pressure drop over the reed's closing pressure, the flow is u = ζ(1 − x)√x,
  zero once the reed shuts; the waveguide gives h = x + u(x), so the pairs
  `bernoulli_points(zeta)` = [x + u, u] map h to u and a `shape` interpolates them.
  The threshold, the beating reed and extinction are all in that curve.
  `embouchure` crossfades a tight and a loose reed opening.
- **`open_end(frequency, floor, track)`**: the bell and tone-hole lattice as two
  one-pole lowpasses at a cutoff that follows the note but never falls under
  `floor`, with their phase `delay` to take off the line.
- **`pipe(seconds, frequency, options)`**: one round trip from the driver to an open
  end (`poles: 2` for `open_end`'s), with an optional register `vent`: three
  lines joined at a lossy hole that passes the mode with a node there and damps
  the others.
- **`cylinder(mouth, frequency, options)`** and **`cone(mouth, frequency, options)`**:
  the single-reed cylinder and the double-reed cone (a long pipe and a short one
  standing for the missing tip), each with `vent`, returning `inside` (the
  mouthpiece pressure), `radiated` (the bell's), `flow` and `feedback`.
- **`flue(mouth, frequency, options)`**: the jet-drive flue pipe. The pipe's flow
  deflects the jet (its displacement is that flow integrated) and the jet's flow
  past the edge (a tanh aimed `offset` off it) drives the pipe by its rate of
  change. `offset` is clamped to ±0.6, the furthest the jet speaks at pitch at
  every breath: 0 is hollow, 0.4 a flute's, 0.6 reedy. `register` (0 by default,
  1.3 in the flute) scales the jet as on the instrument: its deflection falls with
  the pitch while its drive rises, so the low notes saturate against the edge
  and are rich (the second partial within a few dB of the first), the top ones
  are nearly sine and louder. `stiff` lets a harder-blown jet deflect less,
  `bias` makes the vortex loss lopsided, `aim` moves the jet as a signal.
- **`turbulence(level, frequency, options)`**: breath noise with a corner at
  `cutoff` that moves `brighten` octaves with the level and `follow` octaves per
  octave of pitch, falling 6 dB per octave above it and 12 from `slope` times it.
  Use it, never raw `s.noise()`, in a loop.
- **`wind_player(options)`**: the player all five share: `envelope`, `dynamic`,
  `noise`, `fingers`, `cents` (drift and vibrato) and the note `controls` to
  declare.
- `reed`, `jet`, `edge_points`, `player`, `breath`, `dynamics` and `body` are the
  simpler parts of the first models, kept for pieces built on them.

A model is not a recording. Its pitch is within a few cents across its range at
playing dynamics, and moves a few cents with the dynamic as a real one does;
every model speaks only above a threshold that rises with pitch and chokes when
the reed is held shut.

### Resonators

`resonator` accepts `input`, `frequency` in Hz and amplitude-T60 `decay` in seconds.
It is a damped quadrature oscillator driven on its real component. Frequency is
bounded to 1..0.45*sample_rate and decay to 0.001..60 seconds. Pole magnitude is
`exp(-3*ln(10)/(decay*sample_rate))`; constant controls prepare coefficients once.
Dynamic controls rotate and shrink the same two-state vector, avoiding unstable
coefficient interpolation. Output is the real component, with no hidden gain
normalization. Source chooses modal gains and bank size.

Constant oscillator detune, filter coefficients, and resonator coefficients are
prepared once when their operands are literals. Signal-connected controls retain
sample-rate evaluation; this optimization does not introduce a separate control
clock or alter arithmetic ordering for the existing oscillator/filter operations.

## Reusable instruments

`std/synthesis` supplies `pad`, `lead`, `struck`, `pluck`, `texture`, and
`layered(soft,loud)` as ordinary editable instrument recipes;
[`std/woodwinds`](#blown-instruments) supplies blown ones. Their suggested ranges,
controls and gain choices are documented next to their source definitions. The legacy
`choir` preset remains an unchanged alias of `pad`; the recipes use distinct
signal structures rather than silently changing those presets.

`std/instrument.sound` packages an instrument, insert chain, suggested range and
source macro mappings. `play` constructs its track; `automate` turns a normalized
macro curve into ordinary target automation lanes. Existing one-owner-per-target
rules still apply. [instrument-design.muz](../examples/instrument-design.muz) exercises the combined workflow.
Sampled layers accept existing sample devices, for example:

```muz
use "std/synthesis" as native;
let strings = native.layered(sample("soft.wav"), sample("loud.wav"));
// Pressure expression now controls the crossfade independently for each note.
```

### Preset catalogs

`synth(name,params={},presets=catalog.synth_presets)` accepts an alternative
source preset table. A raw `{type:"synth",name:"my-patch",...}` device record can
also describe a custom device; fill in its actual fields before use.

Synth overrides use checked names for modes. Unknown names and numeric mode
overrides are rejected by the source helper. `synth_mode_codes` translates names
to integer device codes; raw device records and numeric parameter inspection are
the lower-level engine interface.

The current palette has no wavetable morphing, granular synthesis, convolution,
or time stretching. Feedback runs through delay lines, so a loop is at least one
sample long (two with cubic interpolation).

## Next steps

Route the instrument through [production effects](production.md), shape its notes
with [performance](performance.md), and [audition or render](workflow.md) the result.
