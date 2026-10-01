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
  seed: 7,
  max_voices: 256,
  sample_budget_frames: 268435456
});
```

Paths are relative to the module declaring the instrument. The `defines` record
supplies declarative bank context without running an upstream bank GUI. Named
contrib builders provide their required bindings and provenance overlays. Physical
controllers use `cc0` through `cc127`, with values in MIDI units 0–127; constructor
control values override mapping defaults. Reuse the existing parameter automation,
MIDI bend and note-ID expression paths. Numeric `sample_zone` annotations are
rejected for SFZ instruments because they bypass authored selection semantics.

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
or `MUZ_GRAPH_BUDGET` after inspecting the diagnostic. Counts remain exposed rather
than silently weakening validation. See [embedding](embedding.md).

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
