# SFZ reference verification

The reference harness uses synthetic samples, explicit MIDI events and existing
local players. It is diagnostic evidence for individual behaviors, not proof
that every patch in the supported libraries sounds identical. Generated WAVs,
player binaries and third-party sample libraries are not repository assets.

## Reproduce

```sh
python3 tools/test-sfz-reference.py
python3 tools/sfz-reference.py \
  --player /usr/bin/sfizz_render --library /usr/lib/libsfizz.so.1.2.3 \
  --muz target/debug/muz --output /tmp/muz-sfz-reference
python3 tools/sfz-reference-frequency.py \
  --library /usr/lib/libsfizz.so.1.2.3 --muz target/debug/muz \
  --output /tmp/muz-sfz-frequency
```

Select trusted, installed executables explicitly. The scripts do not install or
download players. `report.json` is the canonical output: the reference library
also writes parser diagnostics to stdout/stderr. Reports identify binary hashes,
settings, unknown opcodes, exit status and measured output. A successful load or
an empty unknown-opcode list does not establish audible equivalence.

The measured reference in this work was Arch's sfizz **1.2.3-15**, using
`libsfizz.so.1.2.3`. API renders use 48 kHz, 48-frame callbacks, note-on at frame
6000, note-off at frame 30000, explicit master gain 0 dB and CC7/CC11=127.
Native renders use the same sample, note, velocity, sample rate and unity track
gain. sfizz's default host volume is -7.35 dB; implicit CC7 defaults to 100 and
uses a squared gain. Comparing uncalibrated hosts gives a misleading gain error.

## Interpret comparisons

Use gain within 0.1 dB, pitch within one cent, envelope event timing within the
larger of 1 ms or 1% of the segment duration, and dry filter response within 1 dB
as separate gates. Measure resonance and dynamic modulation separately. Compare
random selection through traces and distributions with a fixed native seed;
different players' random generators need not produce identical PCM.

The harness retains unaligned, gain-preserved differences. Its explicitly named
`sfizz_first_callback_repeat_diagnostic` removes a single observed repeated
sample at the first callback boundary. This helps identify a reference artifact;
it is not a universal alignment rule and is unsuitable for arbitrary modulated
pitch. Never normalize away an unexplained amplitude or timing difference.

sfizz inserts at least 1 ms loop crossfade even when no crossfade was requested.
Native inclusive hard loop wrapping therefore need not null against sfizz.
Compare intro samples, inclusive endpoints and authored crossfade separately.
See pinned [Defaults.cpp](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/Defaults.cpp)
and [Voice.cpp](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/Voice.cpp).
The [published loop-crossfade opcode](https://sfzformat.com/opcodes/loop_crossfade/)
does not establish that reference minimum as an authored default.

## Behavior evidence and dialects

| Behavior | Evidence and interpretation |
| --- | --- |
| Includes and sample path base | Synthetic conflicting nested targets select the root SFZ directory in sfizz. See [modular instrument documentation](https://sfzformat.com/tutorials/modular_instruments/). |
| Lone slash, `volume-1`, `volume-2` | sfizz reports malformed tokens, loads successfully and produces exactly baseline PCM. This supports narrowly pinned token removal, not inventing a missing equals sign or changing gain. |
| Case-insensitive sample fallback | Exact synthetic case variants resolve in sfizz. Native ambiguity rejection must remain explicit. |
| Amplitude routes | Base amplitude zero remains silent with a CC route. Base 100 multiplied by normalized CC produces linear gain; multiple routes multiply. Explicit CC7 amplitude routing replaces its implicit gain. |
| Note polyphony | sfizz counts group/key voices, clamps zero to one and releases sibling rings together. Limits 0/1/2/3/4 with two layers produce counts 1/1/2/2/4 on repeated hits. Default self-mask can protect a louder voice while allowing a quieter new voice beyond the soft limit. See [VoiceManager.cpp](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/VoiceManager.cpp). |
| Virtual controllers | The MIDI event state stores shared snapshots, but audible CC131/133/135/140 modulation uses per-voice trigger velocity/key and captured random/key delta. CC131 is zero for note-off trigger voices in sfizz. See [Controller.cpp](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/modulations/sources/Controller.cpp). See [extension documentation](https://sfzformat.com/extensions/midi_ccs/) and [MidiState.cpp](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/MidiState.cpp). |
| Curves | sfizz predefined curves are 0–6; curve4 is squared, curve5 square root, curve6 square root of one minus input. Corpus curves7/8/11/12 are authored custom curves, including point77. See [Curve.cpp](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/Curve.cpp). |
| Numbered EG out-of-range levels | sfizz renders -100 and -1 identically by clamping. Approved Sforzando also renders -100 and -1 with identical PCM, confirming the clamp interpretation for the tested numbered EG. |
| ARIA variables | sfizz rejects `var01_*`; native analytic tests establish the published multiplication/addition equations. Sforzando full/half CC92 multiplication fixtures audibly change cutoff; dry-normalized native response differs by -0.891/-0.646 dB, within the 1 dB filter gate for those two points. See [variable operators](https://sfzformat.com/opcodes/varNN_mod/) and [variable routing](https://sfzformat.com/opcodes/varNN_/). |
| Cross-LFO routing | Short `lfo03_freq_lfo2_oncc117` is documented in [vibrato examples](https://sfzformat.com/tutorials/vibrato/), not a typo to silently correct. sfizz rejects it and the full/base variants. Sforzando short/full forms produce identical PCM. The direction is source N to target X: `lfo03_freq_lfo2` sends LFO3 to LFO2's frequency. Isolated DC volume clocks confirm unit-wave additive Hz. Triangle/sine sources, target base rates 2/4 Hz and depths 0/0.01/1 agree with analytic cycle times within 0.154 ms. Earlier apparent source-depth dependence came from reversing this direction and listening to the actually modulated oscillator's pitch depth. Native corresponding source2/target1 fixtures pass within 0.032 ms. Large depth 100 signed-rate probes remain diagnostics outside this monotonic gate. |
| LFO waveforms | Triangle starts at zero, then positive peak. Native exact sine differs from sfizz's parabolic sine approximation; use modulation depth/frequency/phase metrics, not a global PCM error gate. See [LFO.cpp](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/LFO.cpp) and [wave indices](https://sfzformat.com/opcodes/lfoN_wave/). |
| Envelope curves | sfizz legacy decay follows exponential coefficient9 and stops at sustain. Published ARIA release-shape defaults differ; the measured Sforzando/native thresholds below pass the2ms gate. |

The frequency script samples 100, 250, 500, 1000, 2000 and 4000 Hz for a dry
baseline, LP2, HP1, two filters and one +6 dB EQ band. It normalizes each engine's
filter response to that engine's dry output. The initial measured HP1 and EQ
responses agreed within 0.001 dB. LP2 and two-filter differences reached 3.010 dB
at cutoff and exceeded 1 dB at 500/2000 Hz; that initial run failed the empirical gate,
not permission to choose whichever filter sounds preferable. Re-run after any
filter policy change; the report's binary hash identifies the tested build.
After changing LP2 resonance zero to unity Q, the repeated 30-case sweep passed
with maximum absolute response difference 0.003831 dB.
This six-frequency sweep does not validate resonance, modulation or all filters.

A DC fixture measured identical 100 ms attack-to-90% and 200 ms decay-to-50%
thresholds. For a 200 ms release, native reached 10% 6.75 ms earlier and 1%
14.208 ms earlier than sfizz, exceeding the 2 ms gate. The chosen published
ARIA shape versus sfizz coefficient9 explains a hypothesis, not a passed gate.
The approved Sforzando DC test reached 10% at 0.669458333 s and 1% at
0.713750 s. Native reached those thresholds at 0.669395833/0.7133125 s:
differences 0.0625/0.4375 ms, passing the 2 ms gate and supporting the published
ARIA release shape. This resolves the sfizz release discrepancy for this fixture.

## Additional reference player

Plogue's [official downloads](https://www.plogue.com/downloads.html) offer native
Linux Sforzando **1.982**, dated April 14, 2026, including CLAP/VST3 and standalone
x86_64/aarch64 builds. The x86_64 build requires SSE4.2 and kernel 5.4 or later;
its packages list GTK3/gtkmm, GLib, Pango, Cairo, curl and libc dependencies.
Wine is unnecessary. No ARIA/Sforzando installation was initially found in the
scoped personal plugin locations.

The official x86_64 archive was downloaded with explicit user authorization and
inspected without running its installer or binaries. SHA256:
`ee6b354fd375ff9d0ce30819683f44d3b712a1fe4ff53c77e44df036da82a900`.
The archive contains a proprietary installation-consent agreement in
`opt/Plogue/sforzando/Licence.rtf`. The user separately accepted the actual agreement. Normal CLAP initialization
and audio rendering succeeded in a narrow temporary sandbox containing runtime
libraries, the product, synthetic fixtures and isolated HOME/cache. The plugin
self-reported version **2.1.2.4**. Its public `clap.preset-load/2` interface
rejected direct SFZ and ARIA paths. Normal GUI Import worked once the host
implemented public timer/file-descriptor callbacks. The public state extension
then saved an opaque state and reloaded it without decoding or modifying it.
Updating our synthetic SFZ at that state’s known source path allowed reproducible
case replay. No proprietary binaries or opaque state are repository deliverables.

`tools/sfz-reference-clap.py` provides the public CLAP host, embedded X11 GUI,
opaque state save/load and deterministic MIDI/audio callbacks.
`tools/sfz-reference-aria.py` replays a named case JSON through an approved saved
state. `tools/sfz-reference-aria-cases.json` contains synthetic probes. Use a clean
synthetic fixture path, import it through the normal GUI, then save with
`--save-state`; pass the same path to the suite’s `--fixture`. The suite reports
plugin/state hashes, exact fixture text, load/render failures and audio metrics.
Never use a third-party library file as the mutable fixture.

Reference SFZ fixtures must avoid duplicate opcodes when comparing players:
Sforzando selected the first `sample` in an initial duplicate-sample diagnostic,
while sfizz/native selected the later value. The reported DC and focused ARIA
measurements use one sample opcode per region. The suite preserves only the
final intended synthetic value before writing each fixture.

Prioritize Sforzando black-box probes for numbered EG -100/-200 units, variable
multiplication/custom curves, cross-LFO rate routes, dynamic envelope modulation
and release timing. Analytic tests plus published documentation are useful but
must be reported separately from original-player comparisons. Any reference
unavailable or license-pending gate remains explicitly open; a successful
syntax/import audit alone cannot close it.

## Additional reproducible gates

`tools/sfz-reference-clock-cases.json` and `tools/sfz-reference-clock.py`
measure DC loop volume clocks for 2.5-second held notes. Generate reference
audio with `sfz-reference-aria.py --seconds 3 --note-off 2.625`, then run the
clock analyzer against that output directory. It compares cycle crossing times
with the integrated additive-Hz model and reports interval mean frequencies.
It does not infer clock rates from the zero crossings of a pitched sample.

`tools/sfz-reference-sequence.py` records four-hit sfizz voice traces for omitted,
three and four sequence lengths and positions 1–8. Omitted length keeps the
counter at 1: only position 1 plays. With length 3, positions 4–8 remain unreachable.
This confirms that the audited Virtuosity/Unreal out-of-cycle regions should
produce an authored-unreachable diagnostic, not an invented longer cycle.

Use `sfz-reference-clap.py --stereo` to preserve per-channel measurements.
Raw per-channel calibration covers mono and identical-channel stereo samples
at pan -100, 0 and +100. Native and sfizz at explicit master 0 dB agree in total stereo RMS within
0.02 dB (the centered channel asymmetry from integer CC10 stays below 0.07 dB); ARIA is consistently 3.0103 dB lower for both source types and all three
pan positions. This is a reference-engine normalization difference, not evidence
for a mono-specific sampler correction. ARIA center RMS is 0.122066 per channel
and hard-pan RMS 0.172633; native values are 0.172633 and 0.244140 respectively.
Keep this explicit gain ledger when comparing ARIA, rather than silently fitting
each result. Dry-normalized filter comparisons remain valid. The sfizz oracle
passes the 0.1 dB plain-gain gate; ARIA absolute gain remains uncalibrated until
its independent engine/host normalization is identified.

The reference plugin exposes zero parameters through `clap.params` in this
instance, so that API did not reveal a separate master setting to explain its
constant normalization. This remains an explicit oracle difference.

Actual-library black-box trials cover one held articulation from each of the
six SFZ library families. Both outputs preserve two channels; comparing a stereo
RMS with a mono downmix would confuse width/cancellation with gain. All six
native and ARIA runs loaded audibly, returned success, and produced finite PCM.
These are representative smoke tests and waveform diagnostics, not an assertion
that every patch has passed a sound-equivalence gate.

The native reference binary was the release build with SHA-256
`146be03843455d9cde434ec1a846df686cd5d88f5b8eea07a8fc3093cb6e602d`.
The accepted official Sforzando archive and plugin descriptor versions are
recorded above. Runs used 48 kHz, a three-second render, note-on at 0.125 seconds,
note-off at 2.625 seconds, attack velocity 100, and stereo RMS measured from
0.25–0.5 seconds. Native seed was 7, voice limit 256, and graph budget 32768.
Both hosts received the root's authored `set_cc` values rounded to 7-bit MIDI,
then CC7/11=127, CC10=64, and CC117=0. Fractional SFZ controller defaults remain
covered separately by analytic/native tests; these MIDI trials do not verify
fractional-controller equivalence. Standard and METAL received explicit switch
notes 12 and 17 before their musical notes. Temporary mapping copies rebased
sample paths to the approved installed assets and expanded Shinyguitar's
`sample_dir=../Samples`; original mapping/sample bytes were unchanged.

<!-- Generated by tools/sfz-reference-report.py from the final report. -->
| Family | Original entry program | Musical key | Historical aggregate residual dB | Classification |
|---|---|---:|---:|---|
| Sonatina | `Trumpet Solo Sustain (looped).sfz` | 60 | +0.1034 | source gain passes; residual attributed to filter path |
| Virtuosity | `01-basic-kit.sfz` | 36 | -0.2451 | source gain passes; measured ARIA curve-1 pitch dialect |
| Shinyguitar | `main.sfz` | 60 | -0.2494 | forced-take gain passes; random selection/gain qualified |
| Black and Blue bass | `03-babyblue_all.sfz` | 40 | +0.0124 | source gain passes; independently qualified variable/filter path |
| Standard Guitar | `05-Guitar Chord Central.sfz` | 24 | +0.7479 | source gain passes; authored random gain/offset diagnostic |
| METAL-GTX | `01-METAL-GTX Full.sfz` | 60 | -1.9294 | source gain and measured filter bands pass; random-gain diagnostic |

The largest valid absolute residual is 1.9294 dB (METAL-GTX).
The explicit ARIA ledger is 3.0102999566 dB; the isolated plain-gain bound is 0.1000 dB and does not classify whole patches.
This whole-patch table does not establish isolated gain, selected-sample parity, pitch,
envelope, release, filter or stochastic-distribution conformance. Differences require
source/selection attribution; an unexplained difference is an open gate, not a pass.

Generate this table and its maximum from the same report with
`tools/sfz-reference-report.py --report docs/sfz-reference-evidence.json --ledger-db 3.010299956639812`.
The 1.9294 dB METAL result is the final baseline artifact; the earlier -0.2623 dB
trial used a different controller setup and must not replace it. These rows used
the baseline binary above; current-native and explicitly labeled random-neutral
isolation measurements are classified by the followups below. The fixed-window helper
field called `release_rms` is not a release measurement for these longer notes;
no release-conformance claim derives from it.

Original entry-file SHA-256 values (copied mapping-subtree inventories were also
recorded; full sample dependency closure belongs to the separate catalog audit).
No third-party mapping/audio/state data is committed:

- Sonatina: `6cb051a4ae1da3084fdfe8e0107d7dc544cac739aa9773ef680d9cc63abeccce`.
- Virtuosity: `a04683d87ad820bcb6045b8accf199fbcf69cb7b151674a7fd6c3df5ec526906`.
- Shinyguitar: `e78ba4363619ac242ab43adf646c558b33379c3c0199d77a72dec4752c062d1f`.
- Black and Blue bass: `fd750c8f2204e70897ce446be021b3d6d0584412f1fd2eee9793b6b3ca2d2d6f`.
- Standard Guitar: `97c833d0150a5bc72e8b3777953e86b8ab7ba8e8e86d7865c3065157eeb03183`.
- METAL-GTX: `a7661be6657064013297bb77a0a8827279195914d27e1e2338c23f75ca7b112e`.

`tools/sfz-reference-cc-trigger.py` supplies an additional deterministic CC
trigger probe. Installed sfizz 1.2.3-15 receives values 0, 64, 70, 70, 0, 70 for
`on_locc1=50 on_hicc1=100` and produces active-voice counts 0, 1, 2, 2, 2, 3.
An in-range value change retriggers; an identical repeated value does not in
this reference. Requiring the previous value to lie outside the gate would miss
the second voice. The [opcode description](https://sfzformat.com/opcodes/on_loccN/)
requires an incoming in-range message; [sfizz Layer.cpp](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/Layer.cpp#L147)
adds its observed unchanged-value suppression. Record that distinction rather
than claiming the documentation alone defines suppression.

The CC probe also uses sequence length 2 with distinct DC samples panned to
opposite sides. Values 64, 64, 70 start positions 1, none, 1; active-voice counts
are 1, 1, 2 and only the first sample's side accumulates audio. Thus the identical
in-range message advances the sequence even though its voice is suppressed in
sfizz; advancing only when a voice starts would incorrectly select position 2.

Independent one-voice amplitude and pitch probes establish the CC trigger's
voice context. With `amp_veltrack=100`, CC64/CC100 amplitude ratio is exactly 1,
while ordinary note velocities 64/100 produce ratio 0.409568, approximately
`(64/100)^2`. CC-triggered 440 Hz samples stay at 440 Hz with no explicit key,
`key=64`, and `key=64 pitch_keycenter=60`; an ordinary note 60 at root 60 also
plays at 440 Hz. The trigger key is the sample's `pitch_keycenter`, not the
region's low key, and ordinary note-velocity gain is bypassed. The controller
value still feeds velocity-sensitive filter/envelope/pitch behavior.
[sfizz Voice.cpp](https://github.com/sfztools/sfizz/blob/1.2.3/src/sfizz/Voice.cpp#L380)
implements that key substitution and gain bypass, with a separate
`velocityOverride=previous` exception. These observations cover the tested
sfizz reference; they do not turn unspecified behavior into an inferred
all-player requirement.

### Current METAL controlled attribution

The current release binary SHA-256 is
`69fd14a6e9835f22696b91de9e253c8ab8bb9bc82f874796206e3f493c1bb215`.
A repeat of the original METAL case produced exactly the baseline native PCM
hash, while ARIA's newly drawn random gain changed its residual to -2.0290 dB.
A paired diagnostic neutralized only `amp_random` in temporary mappings in both
hosts: native/ARIA held RMS was 0.1641395106/0.1184062702 and the ledger residual
was -0.1735 dB. These are separate experiments, not replacements for the baseline.

Six fresh ARIA processes per setting with an otherwise constant DC sample proved
negative `amp_random=-3` applies attenuation, rather than clamping to zero or
using the absolute value. Observed attenuation ranged from -2.9501 to -1.4776 dB;
zero produced identical PCM and positive 3 produced +0.0637 to +1.5816 dB.
Some processes repeat the same draw, so these are not six independent seeds and
are not a distribution-coverage claim. The original source uses negative random
gain; a single whole-patch gain residual cannot qualify its stochastic behavior.
No compensating gain or source correction was applied.

A second paired diagnostic kept random gain at zero and removed only cutoff,
resonance, `fil*`, and `eq1`–`eq3` opcodes from temporary mappings. With identical
controls, switch, notes and original samples, native/ARIA RMS was
0.1825739903/0.1288217926, residual +0.0187 dB. Thus the source/selection plain-gain
comparison passes 0.1 dB, and the remaining -0.1922 dB change is attributable to
the active filter/EQ path, not unexplained master gain.

The four outputs were also compared using summed-stereo power spectra of the
0.25–0.5-second window: 2048-frame Hann segments, 50% overlap and segment-mean
removal. Each host's filtered/unfiltered transfer was compared, without gain
fitting. Difference by band was -0.0094 dB (160–320 Hz), -0.0352 (320–640),
-0.1333 (640–1280), -0.3620 (1280–2560), -0.5858 (2560–5120), and -0.1305
(5120–10240): within the 1 dB component gate. The 80–160 Hz band differed by
-4.0941 dB, but contained only 0.000003806 of unfiltered power (-54.2 dB of total),
where finite-window leakage and PCM16 precision make the actual-window ratio
unreliable. The energetic 100 Hz component check below resolves the low-frequency
filter/EQ conformance gate without suppressing this historical diagnostic.
This qualifies the measured energetic bands; it does not establish an unrestricted
full-spectrum result or qualify every METAL articulation.

Local evidence is `metal-isolation-current/report.json`,
`metal-filter-isolation/report.json`, `metal-filter-isolation/spectral-report.json`,
and `amp-random-reference/report.json` under the isolated reference workspace.
Reports contain original source hashes, controller/switch recipes, public host
output, binary hash, and WAV hashes. Third-party assets and opaque player state
remain outside Git. An initial native diagnostic omitted the explicit graph
budget and was rejected; the successful retry used the unchanged source and
established budget 32768. Subsequent paired diagnostics below provide the remaining family attribution;
the standalone synthetic component gates above remain independent evidence.

### Five-family controlled diagnostics

A second current-binary series used the same authored controllers, switches,
notes, paths and samples. Temporary copies removed only random amplitude, pitch,
offset, delay and pan parameters and filter/EQ opcodes. Random region selection
was retained at this stage. Residuals were Sonatina +0.0105 dB, Virtuosity
-0.2451 dB, Shinyguitar +0.7046 dB, bass +0.0105 dB and Standard +0.0184 dB.
Sonatina, bass and Standard pass the isolated source-gain bound in these copies;
this attributes their original aggregate differences to removed processing,
without establishing all processing details from RMS alone. Sonatina's active
filter contribution was +0.0928 dB; bass's was +0.0018 dB. Standard has authored
negative random gain and random offsets, so its original single draw remains a
stochastic diagnostic rather than deterministic gain evidence.

The actual include graphs contained 2, 123, 15, 25 and 110 files respectively,
with no missing includes. Virtuosity's unchanged intermediate result isolated its tune path for the next
probe; Shinyguitar's intermediate result retained different random takes. These intermediate residuals are not conformance passes; the subsequent
single-cause diagnostics below resolve their attribution. Raw source inventories, measurements and host diagnostics
are recorded in the local `five-family-neutral/report.json` artifact.

Subsequent single-cause diagnostics resolved the remaining source-gain paths:
forcing Shinyguitar's first random interval in both temporary mapping copies gave
+0.0003 dB, while the native PCM stayed unchanged. The prior +0.7046 dB therefore
compared different random takes. Removing only Virtuosity's tune and tune-controller
parameters gave +0.0106 dB, with microphone layers and sample selection preserved.

A synthetic 440 Hz tone isolated the latter difference. Two 1200-cent routes with
built-in curve 1, CC72=64 and CC90=64, produced 444.829187 Hz in native muz and
444.828940 Hz in sfizz (difference approximately 0.001 cent), but 440.000041 Hz in
ARIA. Native/sfizz center normalized bipolar curve 1 at 63.5; ARIA centers integer
64. This is an observed controller-dialect difference, not a decoder or gain bug.
The actual library authors `set_cc72=63.5` and `set_cc90=63.5`; preserving those
fractional defaults in native import retains neutral tuning. The common rounded
MIDI recipe intentionally turns both into 64 and therefore produces the explicitly documented 18.9-cent ARIA dialect difference. The common
sfizz/native pitch gate passes at 0.000962 cent; this does not relabel the ARIA
rounded-controller result as a pitch pass. Do not adjust master gain or pretend
that passing source-gain diagnostics establishes universal player equivalence.

The additional artifacts are `selection-tune-isolation/report.json` and
`tune-probe-report.json`. The latter records the exact synthetic SFZ. An initial
scratch sfizz call omitted synth cleanup and tripped its leak assertion; the
reported retry called `sfizz_free` and exited successfully with zero buffers.
`tools/sfz-reference-spectrum.py` reproduces paired transfer diagnostics from four
PCM16 WAVs and explicitly marks low-energy bands; its energy label is diagnostic,
not an automatic exemption from a declared conformance gate.

### Continuous-instance stochastic gates

`tools/sfz-reference-stochastic.py` uses 1000 isolated 15 ms notes at 20 ms spacing
within each continuous player instance. A constant DC source separates gain from
sample timbre. Two half-probability regions panned left/right yielded ARIA counts
521/479 and native counts 467/533, both within the predeclared four-sigma binomial
bound of 63.25 around 500. The positive 3 dB unipolar random-gain mean/variance was
1.508215/0.767898 in ARIA and 1.538262/0.736892 in native; negative 3 yielded
-1.499136/0.729153 and -1.538265/0.736892 respectively. All measured ranges stayed
inside the signed 0–3 dB interval with 0.005 dB PCM16 allowance, means within
0.109545 dB of ±1.5, and variances within 0.084853 dB² of 0.75. The bounds use
uniform/binomial theory with four sigma to avoid frequent stochastic-test false
failures; no seed matching or gain fitting is required. Native seed was 7.
These pass the tested selection/gain distribution gates, not every upstream
random-layer/controller cross-product. Reports retain each fixture and public
host diagnostic; repeated notes avoid the coarse process-seed correlation in the
earlier six-process probe.

The low-energy METAL actual band was separately checked with an energetic 100 Hz
sine at 44.1 kHz source/output and note-on zero, using the actual CC-driven LP2
and three-EQ opcode forms. Native filter/EQ transfer was -6.303073 dB, ARIA
-6.301600 dB: difference -0.001474 dB, passing 1 dB without an energy-floor
exemption. The original low-energy actual-window discrepancy remains recorded;
it does not describe this nondegenerate low-frequency component behavior.
The host's `--sample-rate 44100`, `--note-on 0`, and `--midi-events` paths were
executed successfully through the accepted public plugin interface.

### Final evidence and scope

[Metadata-only final evidence](sfz-reference-evidence.json) merges the original
six baseline metrics unchanged with controlled followups, source/WAV hashes,
player versions/hashes, MIDI/controller recipes, distribution gates and explicit
ARIA dialect classification. No audio, library mapping text, plugin binaries or
opaque player state is included. The baseline table and its maximum are generated
from this one artifact; its classifications reference the subsequent experiments.
Rebuild it from the local measured reports with
`tools/build-sfz-reference-evidence.py --workspace REFERENCE_WORKSPACE --output docs/sfz-reference-evidence.json`,
then render with
`tools/sfz-reference-report.py --report docs/sfz-reference-evidence.json --ledger-db 3.010299956639812`.

All six isolated source-gain paths pass 0.1 dB; common native/sfizz pitch, tested
filter/EQ, envelope/release and cross-LFO components, and continuous-instance
selection/gain distributions pass their declared gates. The ARIA integer-center
curve 1 difference remains an intentional measured dialect policy. Qualification
covers those exact representative events and isolated families, not waveform-null
equivalence across all 588 programs or every controller/articulation cross-product.
No remaining sound blocker was found within that declared evidence scope;
performance qualification is tracked independently and must retain its actual
failed/pending results.

Scientific reference helpers (`sfz-reference-spectrum.py`,
`sfz-reference-stochastic.py`, and their spectrum tests) require NumPy. The
verified interpreter on this host is `/usr/bin/python` (Python 3.14, NumPy 2.5.3 at
`/usr/lib/python3.14/site-packages/numpy`). The default `python3` resolves to
`/home/linuxbrew/.linuxbrew/bin/python3`, which lacks NumPy here; use the verified
interpreter explicitly, for example
`/usr/bin/python tools/test-sfz-reference-spectrum.py` and
`/usr/bin/python tools/sfz-reference-spectrum.py --help`.
This dependency belongs to offline scientific reference tooling, not native muz.
No additional package installation is required on this host.
