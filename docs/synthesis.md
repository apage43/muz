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
ordered during compilation, with the 256-node limit and bounded nesting.
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
| `note_age` | Seconds since the sounding note began; restarts when a mono voice passes to a new note |
| `expression` | `kind`: volume, pan, tuning, vibrato, expression, brightness, pressure, or a declared custom note control; a note without a curve reads the [neutral value](performance.md#oscillating-curves) (brightness 0.5, vibrato and pressure 0) or the declared default |
| `osc` | `wave`: sine/saw/pulse/triangle; `ratio`, `detune` in cents, optional `hz`, `fm` in Hz, pulse `width`, `phase` in cycles |
| `noise` | Deterministic bipolar white noise |
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

A patch has 16 voices, at most 256 nodes, and at most 16 inputs to a sum/product.
A voice's storage and work grow with the nodes its patch uses.
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

### Oversampling

`oversample: 2` or `4` on a `voice_patch` runs the whole voice graph at that
multiple of the output rate and brings the sum of its voices back down through
half-band filters (47 taps a stage, about 80 dB down from 0.3 of each stage's
input rate: at 48 kHz and `oversample: 2`, everything above 29 kHz is removed
and 24 to 29 kHz folds into 19 to 24 kHz). Use it where a
nonlinearity makes partials above half the sample rate, which fold back as
inharmonic tones: `drive`, a raw `shape`, a clamp, and above all a nonlinearity
inside a [feedback loop](#feedback-loops-and-waveguides), where ADAA does not
apply. The graph's times and frequencies mean the same at any setting, so a patch
keeps its pitch, envelopes and filter cutoffs; the cost is the factor in CPU and
delay memory, and about a quarter of a millisecond of latency (11.5 output
frames at 2, 17.25 at 4). Filters may open up to 0.45 of the raised rate, `noise`
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
Probe nodes count toward the 256-node limit.

## Blown instruments

`std/physical` builds woodwinds from those loops. The bore is a line, and the
reed or air jet is the nonlinear part inside it, driven by a breath pressure. The
tone then follows the breath as it does on the instrument: a note starts when
the pressure crosses a threshold, harder blowing changes the spectrum as well as
the level, and a slur moves the pitch of a bore that is already sounding.

```muz
use "std/physical" as p;
use "std/instrument" as i;
let line = i.slur(phrase("D4:q F4:e@slur A4:e@slur G4:h"), "slur")
    .express({pressure: [[0, 0], [0.6, 0.35], [1, 0.1]], vibrato: [[0.5, 0], [1, 0.6]]});
song({tracks: [track("clarinet", line, i.mono(p.clarinet(), 25ms))], tail: 1})
```

| Recipe | Model | Range |
| --- | --- | --- |
| `clarinet(options)` | `cylinder`: a closed pipe and a single reed, odd harmonics | D3 to about G6 |
| `oboe(options)` | `cone`: a conical bore and a stiff reed, every harmonic, two formants | B-flat3 to about G6 |
| `flute(options)` | `flue`: an open pipe and an air jet | C4 to about C7 |

Each recipe reads the same things from a note:

- **Breath.** The level is velocity plus the note's `pressure` curve, clamped to
  0..1, so velocity is the dynamic at the attack and `pressure` swells above it.
  `velocity_track` is 0: the patch maps the level onto the pressures at which
  the model speaks and onto its loudness and brightness (`dynamics`).
- **`brightness`** opens or darkens the bore's cutoff by up to an octave.
- **`vibrato`** is the depth (0..1) of a pitch and breath vibrato at
  `vibrato_hz`; `options.vibrato_cents` sets its width.
- **`jet`** (flute only, 0.5 at rest) is the lip. Lower values sharpen, by about
  6 cents per 0.01, and near 0.25 the note overblows to the octave.
- **`vent`** (clarinet and oboe, 0..1, 0 at rest) opens the register vent on the
  note's own fingering: a clarinet sounds the twelfth, an oboe the octave. A
  curve that opens it during a slurred note is a lip-slur to the upper register
  on one fingering.

Notes above the break choose their register by fingering, as a player does. The
clarinet plays notes from `options.clarion` (MIDI 69, A4) up as the fingering a
twelfth lower with its register vent open, so the clarion is the long tube's
third mode; the oboe plays notes from `options.octave_key` (76, E5) up an octave
lower with its octave vent open. Set either to 128 to play every note on its own
tube. A glide switches register half a semitone below the break.

```muz
// A slurred twelfth on the F3 fingering, then the same F3 tongued.
let twelfth = note(53, 2b).express({vent: [[0, 0], [0.5, 0], [0.55, 1], [1, 1]]});
song({tracks: [track("clarinet", seq([twelfth, note(53, 1b)]), i.mono(p.clarinet()))]})
```

Options are the recipe's voicing: `tone`, `air` (breath noise), `attack`,
`release`, `gain_db`, the flute's `chiff` and `offset`, the oboe's `apex` and
`formants`, and the breaks above. Wrap a recipe in `std/instrument.mono` for
slurs; without it every note is tongued and the patch is polyphonic.

The models take a mouth pressure and a frequency and return `{inside, radiated,
feedback}`: the pressure at the mouthpiece, the sound leaving the open end, and
the entries for the patch's `feedback`. `reed`, `jet`, `edge_points`, `pipe`,
`breath`, `player`, `dynamics` and `body` are the parts the recipes are made of.
A different instrument is a new recipe around the same model, as the stopped
pipe in [woodwind-models.muz](../examples/woodwind-models.muz) shows; a bassoon
or a saxophone is `cone` with its own formants, `apex` and range.

**Vents.** `cylinder` and `cone` take `vent`, a signal from 0 (shut) to 1. The
bore is then `pipe`'s three lines: down to the hole, back up from it, and the
rest of the bore and back. At the hole, the pressure of the arriving waves is
released through a one-pole lowpass of depth `vent_depth` (0.5 by default):
the hole passes the wave on as a highpass and reflects an inverted, lowpassed
share, as a narrow, resistive register hole does. A cylinder's hole sits a
third of the way down (`vent_at`), at the twelfth's pressure node; a cone's sits
on its long pipe at half the whole cone, the octave's node. The mode with a
node at the hole passes it untouched, the others lose energy there, and the
reed moves to that mode. A lossless hole (`vent_depth: 1`) does not: it drags
the lower mode sharp instead. Without `vent` a model is one line, as before;
`pipe(seconds, frequency, options)` is the part to build other vented bores
from, and its result says how to close its loops.

**The jet.** `flue` follows the jet-drive model of a flue pipe. The pipe's flow
at the mouth deflects the jet, so the jet's displacement is that flow
integrated; the jet's flow past the edge (`edge_points`, a tanh) drives the pipe
by its rate of change. `offset` aims the jet off the edge, so its two sides
saturate at different distances and the flow pulse is lopsided: that is where a
flute's second partial comes from, about 9 to 12 dB under the fundamental at
mezzo-forte and stronger as the breath rises (`offset: 0` leaves almost none).
What limits the level is the pipe's own loss growing with its amplitude
(`vortex`, the vortices shed at the mouth), not the jet saturating; a jet that
saturated on both sides would make a symmetric pulse again. The recipe takes
its sound mostly from `radiated`, the pipe's flow leaving it.

Two things about breath follow from the models and are worth knowing before
writing a part:

- **Breath is not a fader.** A beating reed saturates: once a reed model
  speaks, more pressure changes its spectrum far more than its level (a cone
  moves about 1 dB from mezzo-forte to fortissimo). The recipes add the level
  with `dynamics`; a recipe of your own needs that stage, or a `volume` curve
  per note, for a crescendo to be heard as one.
- **A note ends above its threshold.** Lowering the breath through the
  speaking threshold stops a reed within a few tenths of a second (a clarinet
  falls about 40 dB in 0.3 s), which sounds like a cut, not a fade. For an al
  niente, hold the pressure just above the threshold and fade the note's
  `volume` or `expression`; let the breath go only at the release.

A model is not a recording. Its pitch is within a few cents across the stated
range at playing pressures, and drifts where a filter's cutoff approaches the
note; measure a new voicing across its range. Each model speaks only above a
threshold pressure that rises with pitch, and chokes when the reed is held shut.
The reed and jet fold partials back above half the sample rate on bright high
notes. The flute recipe runs at `oversample: 2` for that reason and the reeds at
1; every recipe takes an `oversample` option.

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
[`std/physical`](#blown-instruments) supplies blown ones. Their suggested ranges,
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
