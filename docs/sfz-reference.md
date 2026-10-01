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

## Behavior evidence and unresolved gates

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
| Envelope curves | sfizz legacy decay follows exponential coefficient9 and stops at sustain. Published ARIA release-shape defaults can differ; measure threshold times directly before claiming the timing gate passes. |

The frequency script samples 100, 250, 500, 1000, 2000 and 4000 Hz for a dry
baseline, LP2, HP1, two filters and one +6 dB EQ band. It normalizes each engine's
filter response to that engine's dry output. The initial measured HP1 and EQ
responses agreed within 0.001 dB. LP2 and two-filter differences reached 3.010 dB
at cutoff and exceeded 1 dB at 500/2000 Hz; this is a failed empirical gate,
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

Plain mono sample level requires a separate gate: the calibrated ARIA centered
output is 3.0103 dB below the native mono-to-stereo path in the tested dry fixture.
Filter comparisons normalized to each engine's dry signal remain valid, but
that normalization cannot establish gain fidelity. Resolve the mono pan-law
policy and rerun the 0.1 dB gain gate before claiming it passes.
