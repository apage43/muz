---
name: external-critique
description: Seek and evaluate native multimodal critique from external models, then audition justified revisions of music or other creative artifacts.
disable-model-invocation: true
---

# External multimodal critique

Use external perception as evidence for a concrete creative decision, not as an
authority or a quality certificate. The calling agent retains responsibility for the result; reading a
model's audio description does not give the agent auditory perception.

Paths below are relative to the repository root, three levels above this folder.
Before revising music, read `docs/composer-friction.md` and the piece's production
notes. Record newly encountered friction there, not in a parallel skill log.

## Select a listener and a bounded task

- Establish the artifact, intended audience/style, preserved identity, and the
  decision to improve. Use the session's existing authorization to send named
  material and spend modest API charges; do not ask again for an already approved
  review. Activation does not authorize unrelated uploads, publishing, or messages
  to people. If external submission is not authorized, prepare local review
  material first and ask only for the missing authorization.
- Read `docs/audio-listening-models.md` for the dated music pilot and route
  limitations. For music, its provisional capability choice is
  `google/gemini-3.1-pro-preview`, with `google/gemini-3.8-flash` as a second
  opinion. `google/gemini-3.5-flash-lite` is a cheaper helper, not a demonstrated
  better listener. Recheck live availability when stale or failing; do not repeat
  the entire model survey on every revision.
- Prefer cheap routes for the desired model, including free/contributor routes
  where the user's data-use preferences permit. Do not substitute a smaller model
  merely to minimize cents when capability is the objective. Do not impose ZDR
  without a user requirement. For music, skip transcription/voice-first services
  without evidence of useful general sound understanding.
- Confirm native support for the actual modality at the selected provider. For
  audio send the waveform, for images the actual image, and for temporal video
  judgments use native video or explicitly identify frame-sampling limits. A
  transcript, spectrogram, screenshot or caption is supplementary evidence, not
  equivalent access to another modality. Separate each supported modality when a
  route cannot receive them together; do not pretend joint understanding.
- Plan one focused revision round: usually one or two reviewers, a few relevant
  excerpts, and at most two intervention hypotheses. Retry a failed route only
  when there is a concrete format/provider/settings correction. Stop when the
  evidence supports a choice or is too inconsistent to justify another change.

## Prepare honest evidence

Preserve the baseline source/render under ignored `out/`. Record source hashes,
crop boundaries, format, listening gain, and the concealed label mapping locally.
Do not put generated media, external assets, credentials or base64 payloads in git.

For music use short stereo PCM WAV passages with enough preceding context for
envelopes and effects to settle. Render equivalent scopes from the same source
state and controllable randomness. Follow `docs/production.md` for render/batch,
tap and solo semantics: wet solos are not summable stems, and kit-sidechain taps
are before output faders. Match subjective comparison excerpts using constant
gain to a common measured LUFS level. Do not dynamically normalize away the
change being evaluated. Measure before submitting.

Use a concealed identical pair or a known signal change when selecting an
unproven route or when a result seems suspicious. Allow ties. A zero reported
audio-token count alone does not establish absent audio; successful HTTP and
eloquent prose do not establish delivered audio either. Verify basic events.
Provider attachment limits and single-file versus separate-file presentation can
change results. A failed control lowers confidence; a later pass does not erase it.
A perceptual tie on a small change can be a valid outcome; it is not proof of
identical waveform data. Claims of differences between exact duplicates are a
stronger grounding failure. Never count ties alone as evidence for a revision.

For image/video comparisons, likewise hold crop, resolution, framing, timing and
display conditions constant where they are not the intended change. Identify
what a still frame cannot establish, such as movement or audio synchronization.

## Ask questions that can change a decision

Use fresh contexts and neutral labels. Do not tell reviewers which file is newer,
which hypothesis you favor, another model's answer, or the control's ground truth.
Give intended genre/function when helpful, but avoid supplying source details that
let the model manufacture the requested perception.

An initial audio prompt can be adapted from:

> Listen directly to these excerpts of a [style/intention] piece. Describe the
> salient sources and audible contrast briefly. Identify at most two problems
> worth auditioning, with local audible evidence. Distinguish observation,
> uncertain explanation and taste. Preserve [identity/constraints]. Do not infer
> absent processors or exact measurements from sound. Say if no change is needed
> or if the audio is inaccessible. Give a feasible experiment and its success
> criterion rather than a generic mastering checklist.

A comparison prompt can be adapted from:

> These anonymous excerpts may be identical. Compare [specific perceptual aim]
> while preserving [musical/visual priority]. Identify audible/visible differences
> before stating a preference. Allow ties and low confidence. Does a benefit cost
> anything elsewhere? Do not infer version chronology, changed notes, processors,
> or source settings without evidence. Limit the answer to supported distinctions.

## Call and retain provenance

For native audio, use the existing standard-library caller from the repo root:

```sh
python tools/listening_review.py \
  --model google/gemini-3.1-pro-preview \
  --audio out/critique/A.wav --audio out/critique/B.wav \
  --prompt out/critique/compare.txt --output out/critique/pro-review
```

`OPENROUTER_API_KEY` must be in the process environment. Never put keys in command
arguments, prompts, logs, tracked files or tool output. If the session authorizes
loading a local credential file, extract the named variable privately and pass it
in memory; do not print/source an entire credential collection for inspection.
The helper accepts audio only. For other modalities use the provider's documented
native request format; preserve the same provenance without logging media payloads.

The caller saves request metadata, hashes, model identity, usage and response.
Inspect provider identity, finish reason and usable final content. Keep fallback
disabled when attributing capability to a particular model; pin a working provider
if routing matters. Raw upstream errors can echo submitted payloads: sanitize
them before saving or printing, and report a compact error rather than dumping
the provider body. Track reported cost, distinguishing unknown failed-call cost
from zero. Do not expose private reasoning in user-facing reports; summarize
supported final observations and outcomes.

## Decide, revise and verify

Separate grounded observations from suggested causes and preferences. Check claims
against source/measurements where possible. “Add sidechain,” “missing outro,” or
“clipping” is not established by confident wording, and existing processing may
already address the claimed issue. Model agreement on generic advice is weak
evidence; multiple independent successes on relevant controls are stronger.

Translate the best supported critique into small editable source alternatives.
Audition changes independently where practical; retain the baseline as an option.
Use blind, level-matched comparisons and a second perspective on a consequential
choice. If responses contradict known facts or each other, narrow the question or
retain the baseline rather than revising until the models praise it.

Deliver the chosen full artifact, not only excerpt experiments. For music check
source validity, intended duration, sample/true peaks, mono behavior and ending;
verify encoded copies if delivering them. These checks constrain technical risk,
not artistic value. Use source-level solutions before expanding the engine.

Document what was accepted, rejected and uncertain, the exact final source/render
relationship, reviewer identities, reported cost, and where local evidence lives.
Commit the skill/source/production notes and any new canonical friction together
with the task; keep media ignored. Clearly distinguish actual perception tests,
model opinions, measured facts and the agent's own inference in the final report.
