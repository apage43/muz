# Composer friction

This is the canonical log of live, unresolved composer-facing friction across muz.
Read it before composition and engine work. Record concrete problems as they are
encountered: distorted musical choices, repetitive editing, unclear semantics or
diagnostics, missing abstractions, and workflow failures. A valid workaround does
not close a report.

Commit a report in the same commit as the piece or revision that exposed it. For
work without a piece, name the task and commit the report with that work. Each
entry names its origin, observed behavior, affected decision/work, workaround,
and desired behavior. Include a small reproduction when useful; do not freeze a
whole composition as a test.

Remove an entry in the same commit that fixes the language, engine, documentation,
or workflow responsible, after verifying the reported use case. If only part is
fixed, keep the remaining problem explicit. Put usage in reference docs and
regressions in focused tests. Git history retains reports and their fixes; this
file has no resolved section, remedy archive, or parallel per-piece log. A piece
commit that encounters no new friction needs no ceremonial report. Fixes found
and completed within the same commit need no artificial live entry.

## Choosing a resolution

For every friction item, seek a general solution that expands the builtins/kernel
as little as necessary while fully resolving the reported problem. Start by
asking whether existing language operations and exposed data can express the
solution clearly. Put reusable policies and recipes in `std/`; keep particular
musical choices in project source and demonstrate useful techniques in examples.

When source cannot express the solution well, identify the missing general
capability. Prefer a small, composable primitive or better access to musical data
that enables a family of source-level solutions over a builtin for the reported
special case. Keep policy choices such as shapes, selection rules and overlap
behavior in source wherever practical. Kernel changes remain appropriate for
engine defects, runtime guarantees, or capabilities that genuinely require them;
minimizing the kernel must not mean retaining awkward workarounds or merely moving
complexity into every composition.

In the fixing commit, explain why existing facilities suffice or why the added
primitive belongs in the kernel, and demonstrate the specific resolution in
source. Tag-derived automation is one example of this general rule: expose note
data and timing operations, then let composers write the automation recipes.

## Open reports

- **Synth preset review — percussion recipes remain in the native synth:**
  Inspecting the preset mode selector exposed dedicated kick, snare and cymbal
  branches with fixed pitch envelopes, noise mixtures and timing choices in
  `src/audio/device/studio.rs`. Moving preset settings into stdlib did not move
  these synthesis recipes. Workaround: use a source `voice_patch` for a custom
  percussion design. Desired: assess expressing the stock percussion recipes as
  source patches over general DSP primitives, retaining native processing where
  it is needed for sound quality or runtime guarantees. The current requested
  fix addresses named mode ergonomics; recipe ownership remains open.

- **Afterimage native-audio production review — external listening descriptions
  can contradict the audio's known structure:** Gemini 3.8 Flash through
  OpenRouter accepted native audio (the response reported audio input tokens),
  but described the 66.75-second baseline as losing its drums around 49 seconds;
  the rendered groove continues to 60 seconds. A later review also requested an
  eight-bar subtractive outro that the 154-second candidate already contained.
  This affected which arrangement/mix advice could safely guide the revision.
  Workaround: shorter WAV excerpts, concealed comparison order, constant-gain
  loudness matching, and cross-checking claims against source and measured audio;
  treat preferences as subjective and discard contradicted event/processor
  claims. Even excerpt descriptions remain imperfect. Desired: a listening
  workflow with dependable localized observations and explicit uncertainty, so
  composers can distinguish heard defects from plausible invented explanations.
  The subsequent OpenRouter model survey reproduced the problem with controlled
  excerpts: Gemini 3.5 Flash-Lite and 3.8 Flash denied a two-second digital-silence
  interval; MiMo V2.5 and Inkling Small missed a large bass reduction across
  attachments. MiMo detected that reduction within one file but invented changes
  in notes. Full Inkling and 3.8 Flash failed one single-file bass comparison but
  passed another presentation; 3.1 Pro passed both and still missed the silent
  gap. See `docs/audio-listening-models.md` for the controls and model choices.

- **OpenRouter audio-model survey — advertised audio inputs do not ensure a
  usable listening route:** Muse Spark 1.3 Contributor and a Nemotron 3 Nano Omni
  retry returned text saying they could not hear the supplied audio. Inkling
  Small's DeepInfra route rejected the audio schema, while Together accepted it;
  free Inkling routes rejected the direct API caller as an unrecognized harness.
  MiMo's DeepInfra route rejected three attachments. This prevents treating the
  audio-input catalog as an interchangeable reviewer pool. Workaround: probe
  known audio events, retain provider identity, use accepted formats and fewer
  attachments, and select a working route. Desired: capability metadata and
  diagnostics that accurately establish audio delivery and attachment limits
  before a production review is trusted. Origin: the model-selection survey in
  `docs/audio-listening-models.md`; local request evidence is under ignored
  `out/audio-model-review/`.
