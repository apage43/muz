# Native SFZ instruments

[Instrument guide](instruments.md) · [Complete contrib assets](../contrib/SFZ.md) ·
[Embedding](embedding.md) · [DAWProject](dawproject.md)

muz imports SFZ into its Rust engine. `sfz(path, options)` is an instrument;
original mappings choose all matching regions instead of choosing a single native
sample zone. Existing curated sample-map presets remain available. The supported
contrib corpus consists of Sonatina 4.0, Virtuosity Drums 0.925, Shinyguitar, Black
And Blue Basses, Standard Guitar and METAL-GTX. Their 588 public roots are listed
in per-pack `sfz-catalog.json` files. VSCO's pinned repository contains no SFZ
mappings, so its existing native builders remain its interface.

```muz
let instrument = sfz("Programs/main.sfz", {
  defines: {sample_dir: "../Samples"},
  cc1: 104,
  cc7: 127,
  cc11: 127,
  seed: 7,
  max_voices: 256,
  sample_budget_frames: 268435456
});
```

Paths are relative to the module declaring the instrument. The `defines` record
supplies declarative bank context without running an upstream bank GUI. Named
contrib builders provide their required bindings and provenance overlays. Physical
controllers use `cc0` through `cc127`, with values in MIDI units 0–127; constructor
control values override mapping defaults and are restored on instrument reset.
Names outside `cc0`–`cc127`, nonfinite values, and values outside 0–127 are rejected
during song validation, before audio preparation. Fractional values such as
`cc90: 63.5` are preserved. Use constructor controls for initial instrument setup
and the existing parameter automation or MIDI CC events for later musical changes.
MIDI bend and note-ID expression use the same native performance paths. Numeric
`sample_zone` annotations are rejected for SFZ instruments because they bypass authored selection semantics.

## Import and preparation

`src/sfz` preprocesses ordered source text and produces a serializable `Program`.
It expands includes/macros, flattens global/master/group/region inheritance, keeps
opcode source locations and resolved dependency identities, and
retains control defaults, labels and curves. Last assignment in a scope wins.
Original opcode values remain available; typed compilation converts note names.
Headers reset their subordinate scopes. Includes first resolve relative to the
program root, as measured in sfizz; fragment-relative fallback reports a diagnostic.
Sample paths follow the root/default-path mapping context. Both includes and samples
use exact filenames first, then SFZ-specific case-insensitive fallback; ambiguous
fallbacks fail. Other native asset formats retain their existing path semantics.

Undefined macros, include cycles, size/depth limits, malformed numeric values and
unrecognized opcodes produce errors. Recognizing source syntax does not
prove playback support: preparation separately compiles every effective region's
selection and DSP behavior, rejecting unsupported behavior. Source errors retain
path and line information. `Program::validate` also checks deserialized structure and value semantics. Import
limits are 16 MiB cumulative source bytes, 32 include levels and 100,000 regions;
these source limits are distinct from saved-state and embedded-asset limits.

An opt-in `SourceOverlay` records a relative mapping path, original SHA-256, exact
single-occurrence token removals and a reason. The original installed source is
never rewritten. The importer rejects changed hashes, ambiguous removals and unused
descriptors. Sonatina builders use these descriptors only for malformed tokens
measured to be ignored by sfizz; they never invent a missing volume value.

Prepared instruments decode each unique WAV/FLAC recording once into stereo frame
buffers, compile region predicates/DSP and preallocate bounded voice/note state.
`*silence` is an explicit internal silent source. Missing samples fail preparation.
Parsing, hashing, decoding and allocation happen before callback processing.
Cancellation is checked during preparation. Independent instruments own their
controller, performance-history, random and voice state.

## Selection and expression

The dedicated processor evaluates layered key/velocity/controller/bend gates,
keyswitches, first/legato triggers, sequence position and one shared random draw
per triggering event. Release and release-key layers retain their originating note
identity. Sustain, one-shot and sustain/continuous loops, delayed attacks, group
chokes and note polyphony have separate state; choking audio does not erase the
held-key history used by legato or keyswitch selection.

For `off_mode=time`, muz deliberately follows sfizz 1.2.3's timed amplitude
release: the current envelope level decays by `exp(-9 * elapsed / off_time)`,
then finishes with a 50 ms linear fade below an envelope amplitude of `1e-4`.
Positive times shorter than 6 ms use 6 ms; zero enters the final linear fade
directly. The coefficient is prepared off-thread. Choking during attack stops
the attack at its current level; repeated group chokes and subsequent key
releases do not restart the timed fade. Pitch/filter envelopes enter their
ordinary release stages.

This is a compatibility policy, not a universal SFZ conformance requirement.
[`off_mode=time`](https://sfzformat.com/opcodes/off_mode/) and
[`off_time`](https://sfzformat.com/opcodes/off_time/) are ARIA extensions; the
documented default [`off_curve=10`](https://sfzformat.com/opcodes/off_curve/) is
vendor-specific, while `off_curve=-1` explicitly denotes linear fading.
Explicit `off_curve`/`off_shape` opcodes remain unsupported and are rejected
rather than silently substituted. The reference algorithm is sfizz 1.2.3's
[`ADSREnvelope`](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/ADSREnvelope.cpp)
and [voice off handling](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/Voice.cpp).


Controller-trigger regions (`on_loccN`/`on_hiccN`) respond to changed in-range
messages. Repeated identical messages advance sequence state without starting a
voice, matching the measured sfizz behavior. These voices use the region pitch
center and retain controller velocity for tracking, while bypassing ordinary
attack-velocity amplitude gain. Each trigger owns independent layers. The present
contrib corpus has no controller-trigger regions; synthetic probes cover this
additional recognized behavior.

Physical CCs are channel-scoped. Virtual modulation inputs include velocity, key,
random and previous-key distance and follow their documented note-time lifetimes.
CC131/133/135/140 modulation is captured per voice, so a later note cannot change
an earlier voice's velocity, key, random value or key distance. CC131 is zero for
release-trigger voices. Event-time envelope depths and LFO phases latch before
later events at the same sample offset; live controller routes remain live.
Note-ID volume, expression, pan and tuning affect every layer owned by that note,
including releasable layers. Authored SFZ gain and tuning combine with those values.
Implicit CC7/11 gain applies only where the mapping does not explicitly author the
corresponding amplitude routes. Controllers initialize from mapping defaults, then
constructor overrides; resets restore those values and the configured seed.

`note_polyphony=0` is retained in source diagnostics and played as a minimum of one,
matching the measured sfizz behavior. Polyphony counts audible region voices and
chokes their sisters. Velocity self-masking protects louder earlier voices.
Selection and voice stealing are deterministic for a fixed seed and event order.
The last effective `sw_default` initializes switch state. Only explicitly named
`sw_last` keys are registered switches; broad listener ranges do not consume
ordinary musical keys. Registered switches can also select playable regions.
Switch history and previous musical-key distance are independent. An authored
sequence position beyond its sequence length stays unreachable and inspectable.
Import diagnostics identify its effective length and inherited source. Reference
tests confirm that an absent `seq_length` defaults to one; muz does not infer a
larger cycle from other regions' positions.

## Per-note articulations

A keyswitched program chooses regions by its switch state. Instead of writing a
note at the switch key before each note, declare the instrument's articulations
and let each note name its own:

```muz
let guitar = sfz("Programs/guitar-ksop.sfz", {
  articulations: {
    open: 24,
    mute: 30,
    harmonic: 92,
    slide_up: {key: 31, cc21: 64}
  }
});

// Which tags mean which articulation is a source decision.
let techniques = {pm: "mute", harm: "harmonic", sl: "slide_up"};
fn articulate(p) = p.map_notes(fn(n) => {
    let named = filter(n.tags, fn(tag) => contains(keys(techniques), tag));
    if len(named) > 0 { {data: merge(n.data, {articulation: techniques[named[0]]})} } else { {} }
});

let lead = articulate(phrase("E3:e@pm E3:e@pm G3:q B3:q@harm D4:q@sl"));
```

`articulation` is a reserved note annotation: a declared name, or a raw switch
key 0–127 (`p.annotate("tag:pm", {articulation: 30})`). An entry of
`articulations` is a switch key or a record with an optional `key` and up to
eight controller values `cc0`–`cc127` in MIDI units; an entry may set
controllers alone for programs that select by controller.

The engine applies the choice to that note alone, at its note-on, as if the key
had been pressed and the controllers moved immediately before it:

- The note's regions, including its release-trigger layers, are selected with
  the entry's `key` as the `sw_last` state.
- The entry's controllers hold their values while the note's regions are chosen
  and started, so `locc`/`hicc` gates and values latched at note start see them.
  They then return to the channel's values: modulation that follows a controller
  continuously keeps following the channel controller, and release layers are
  gated by the channel's controllers at the release. Automate those with
  ordinary CC events, which already precede a note at the same instant.
- Nothing is left behind. The channel's switch state, controllers, legato and
  `sw_previous` history are unchanged, so simultaneous notes each play their own
  articulation without staggering and a song can start with one at beat 0.

A note that names no articulation uses `default_articulation: "open"` when the
instrument declares one. Otherwise it follows the program's switch state: its
`sw_default`, or the last keyswitch note written in the score. Keyswitch notes
written as notes keep working and are not changed by per-note articulations.
Because a choice never depends on earlier notes, section renders and seeks need
no history for it.

There is no switch note: checks, note counts, `inspect` and exports see only
the musical notes. Compilation rejects an unknown name (listing the declared
ones), a `key` that no region names in `sw_last` (listing the program's switch
keys), and `articulation` on an instrument other than `sfz()`. MIDI file export
writes the musical notes only and does not encode the choice; DAWProject export
reports each such note, and a declared default, as `ARTICULATION_OMITTED`.

## DSP model and reference evidence

Prepared DSP covers gains/velocity curves/key tracking, fades, pan/width,
AHDSR envelopes, numbered EGs, legacy and numbered LFOs, dual filters and three EQ
bands, variables and acyclic cross-LFO modulation. Sparse compiled routes avoid
source string lookups in the sample loop. Voice DSP uses fixed arrays; cyclic
modulation fails compilation instead of recursing in the callback. Runtime and DSP
unit tests specify equations, update timing and release behavior.
Cross-LFO `lfoN_freq_lfoX` routes source N into target X's frequency as additive
Hz from the source oscillator. Asymmetric DC probes against Sforzando verify the
direction and measured clock. Per-voice controllers are read through a borrowed
view, avoiding a controller-array copy per voice per sample. Channel generation
tokens invalidate physical-controller coefficient caches only when normalized
values change. Waveforms, phases and envelopes still advance sample by sample;
cache/bypass regressions preserve exact PCM through controller changes, release
and checkpoints.

`tools/sfz-reference.py` generates synthetic recordings and events, runs the native
engine and a local sfizz reference, and saves measured comparisons plus exact
binary hashes. Its report distinguishes measured behavior, normative documentation
and unsupported reference opcodes. Reference renders are external evidence, not
redistributed upstream assets. Different interpolation/envelope/filter equations
are compared with relevant gain, pitch, timing and response metrics rather than a
blanket waveform equality claim.

Known reference dialect differences include sfizz's forced minimum 1 ms loop
crossfade despite the SFZ opcode documentation giving no such default. muz uses
explicit authored loop smoothing and otherwise wraps at the inclusive loop endpoint.
The harness also records a reproducible first-callback repeated sample in the tested
sfizz build; raw measurements remain present beside a separately named diagnostic.
ARIA-specific variables and cross-LFO routes have normative and synthetic validation;
sfizz does not implement those routes, so its renders cannot establish their empirical
agreement. Full corpus/reference qualification is tracked in the remaining plan.

## Resource limits and inspection

`max_voices` defaults to 256 and accepts 1–4096. Note ownership is independently
bounded. `sample_budget_frames` defaults to 64 Mi stereo frames (512 MiB decoded),
with a maximum of 1024 Mi frames (8 GiB decoded). Unique sample metadata is counted
before decoding; an oversized program fails with its required footprint. Large
original programs deliberately need an explicit larger budget: Shinyguitar needs
about 1.17 GiB decoded, Darkblack keyswitch about 1.75 GiB, and METAL Full about
3.64 GiB; Virtuosity's full kit needs about 4.9 GiB. Those estimates describe stereo
runtime buffers, not download size. Choose a budget the host can actually allocate;
the hard ceiling does not promise that every platform can hold a full library.
Likewise, voice capacity is a memory/selection limit, not a realtime guarantee.
An earlier fixed 256-layer synthetic release benchmark took about 0.704 s for
0.501 s of audio on the qualification host. Coefficient, gain and pitch caches and
an absolute note-age clock preserve exact PCM, but that result does not qualify arbitrary 256-layer realtime playback. The
[final declared-workload grid](sfz-performance.md) passes all 72 timing cases with
zero overruns/drops and at least 13.1% headroom below the half-callback target.
Measure the actual instrument and host before choosing polyphony.

The existing graph budget additionally accounts for exact SFZ region/sample counts,
modulation declarations and voice slots. Region/sample costs are one unit each;
modulation and voice costs group 32 declarations/slots per unit. Large mappings can
exceed the default 4096 graph units; choose a sufficient explicit host graph budget
or `MUZ_GRAPH_BUDGET` (at most 65536) after inspecting the diagnostic. Counts remain
exposed rather than silently weakening validation. See [embedding](embedding.md).

### One program on several tracks

Instruments whose normalized programs are identical share one prepared program:
compiled regions, modulation tables and decoded recordings are built once and read
by every such instrument, also from parallel render threads. The same file with the
same `defines` and `source_overlays` yields an identical program, so five tracks
calling `sfz("guitar.sfz")` prepare it once. Each instrument keeps its own voices,
controller values, keyswitch and round-robin state and random sequence, so
`max_voices`, `seed`, `ccN` values and `sample_budget_frames` may differ between
tracks without preparing another copy. Different `defines` or overlays, a changed
program or recording, or another sample rate give a separate program.

The graph budget follows the same rule. A program's regions, recordings and
modulation terms are charged once, to the first instrument that uses it. Every
instrument pays for its voice slots, and each instrument after the first also pays
one unit per 32 regions for its own region counters and its own search of the
region table at note-on. A program with 12,427 regions, 2,278 recordings and
173,861 modulation terms costs 20,139 units once and 389 units plus voice slots
for each further track. `muz check --json` reports `sfz_programs`,
`sfz_instruments` and the cost of the largest programs under
`graph.sfz_program_costs`; an over-budget diagnostic names the largest program,
its one-time cost and its cost per further instrument.

Sharing lasts as long as an instrument uses the program. The host context that
prepares the session holds only a weak index of it, keyed by the normalized
program, the sample rate and the version (for files, size and modification time)
of every recording. A host that keeps its context, as live reload does, therefore
reuses the program that is already playing; an edited file is prepared afresh,
and memory is released with the last instrument.
`SfzStatistics::program_instances` reports how many instruments currently use an
instrument's program.

`AudioEngine::device_sfz_statistics()` returns per-device decoded frames, configured
limits, active notes/voices, matched/started regions, stolen voices/notes and dropped
regions. Stolen-note counters record note-owner capacity pressure; stolen-voice
counters record voice/polyphony pressure. It allocates an inspection result and
belongs outside the callback.
Exact effective opcode maps share immutable compiled DSP programs during
preparation. Statistics expose their unique count and shallow struct bytes; the
shallow count excludes separately allocated curve/route tables and is not total
resident memory. Region identity and selection remain independent of DSP sharing.
Processor counters and audio processing use preallocated storage. The callback
allocation regression exercises layered attacks, releases and loops and verifies
identical PCM under different block partitions.

## Hosts, assets and state

SFZ is part of the native model and instrument state, including normalized programs,
source overlays, seed, limits and controls. Version 4 linked wire state stores a
compact descriptor and normalized-program fingerprint instead of duplicating all
regions. Restoration verifies the complete dependency SHA-256 closure before
reparsing offthread; version 3 states remain readable. Native restoration scopes
access to the existing trusted contrib discovery and its five SFZ asset directories.
Custom libraries require `MUZ_SFZ_ASSET_ROOTS` or a host-supplied authorized resolver;
descriptor paths never grant access. Missing, modified or unlisted dependencies
fail restoration. Files must remain stable during restoration/preparation: native
size/mtime fences and canonical path checks are not an OS sandbox against hostile
concurrent filesystem mutation. Byte-identical
reinstallations remain valid when timestamps change. Opt-in embedded state preserves
its own bounded asset package; the instrument-state ceiling is 64 MiB and embedded
assets are limited to 32 MiB. Full libraries ordinarily use linked installation. SFZ reports an unknown/infinite
CLAP tail conservatively; offline rendering still uses the configured session tail.
Memory-backed `AssetResolver` supplies the same paths and bytes to embedded/WASM
hosts. Compile the core with `--no-default-features` for those hosts.

CLAP uses the instrument state and common MIDI/expression paths. DAWProject packages
actual SFZ mapping/sample dependencies according to its existing asset policy.
Effect racks reject SFZ instrument state before asset reads. Unreal's restricted
contrib builders reject `embed_assets: true`; installing a library does not grant
permission to redistribute its recordings or publisher mappings. Generic embedding
remains available for a caller's appropriately licensed material.

For corpus reproduction, build `cargo build --bin sfz-audit` and run the commands in
[contrib/SFZ.md](../contrib/SFZ.md). Normalization, dependency verification, runtime
preparation and audible comparison are distinct stages in the reports. A successful
parse alone does not qualify a program as fully supported.
