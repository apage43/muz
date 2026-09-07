# Audio model selection for production feedback

Surveyed 2026-09-06 (Pacific), using the first piece excerpts and OpenRouter native
`input_audio`. This is a small practical pilot of particular hosted routes, not a
general model ranking or a music benchmark. The survey includes full Inkling and
Gemini 3.1 Pro; cheap routes were preferred without excluding larger models or
requiring zero data retention. No music was changed on the strength of these
survey responses.

## Recommendation

For capability, **Gemini 3.1 Pro Preview is the provisional first choice**. It
correctly identified the bass reduction and unchanged notes in both single-file
comparison controls, whereas 3.8 Flash and full Inkling failed the first and
passed the second. Pro still missed the silent gap and gave incorrect timestamps;
it has not earned authority over arrangement or technical fault diagnosis.

**Gemini 3.8 Flash remains a useful second opinion**, and **3.5 Flash-Lite is a
cheap comparison helper**. The initial matched results established better value
for Lite, not superior capability over 3.8. **MiMo V2.5 is useful for coarse
events within one file**. Full Inkling did not emerge as a stronger music listener
than the Geminis. Its inconsistent control results weaken trust in its otherwise
plausible production prose.

| Model and working provider | What the pilot established | Decision |
| --- | --- | --- |
| `google/gemini-3.1-pro-preview`, Google | Correct EQ/identity/unchanged-note answers on both single-file comparison controls. Missed the two-second silent gap and shifted later event times earlier. Production critique identified the busy hook versus spacious answer, with concrete balance auditions. | Provisional capability choice; small pilot, fallible event descriptions. |
| `thinkingmachines/inkling`, Together | Missed silence, bass reduction and unchanged notes on initial controls; passed all three identity/EQ/note questions on the repeated-original control. Production prose was plausible but asserted the same lead patch in both musical excerpts. | Available at modest cost, but no demonstrated advantage for this job. |
| `google/gemini-3.5-flash-lite`, Google AI Studio | Correctly identified identical A/B files and a heavily bass-reduced C; preserved the distinction between EQ and changed notes. Missed an exact two-second silent gap. Production advice included unsupported certainty about identical synth patches and possible clipping/aliasing. | Best cheap comparison helper tested; verify factual claims. |
| `xiaomi/mimo-v2.5`, DeepInfra | Correctly found the silent gap, muffling, restoration and sparse ending, with some timing error. Called a heavily bass-reduced pair effectively identical across attachments. Putting the pair into one WAV recovered the bass difference but elicited invented changes in melody and rhythm. | Useful for coarse event descriptions in one clip; unreliable for A/B mix decisions and note claims. |
| `thinkingmachines/inkling-small`, Together | Missed the silence and major bass reduction, including in a single-file comparison. Preferred-format mono 16 kHz WAV did not rescue event descriptions. Correct identical-pair answers were uninformative because altered pairs also received ties. | Deprioritize for this task, despite encouraging general audio benchmarks. |
| `google/gemini-2.5-flash-lite`, Google | Failed the basic chronology/filtering probe, including the silent gap and ending. | Its lower price did not justify using it here. |
| `google/gemini-3.8-flash`, Google AI Studio | Correct multi-attachment identical/EQ comparison; missed the silent gap. Failed the two-part single-file bass comparison, then passed the three-part repeat control. Useful production critique, but some unsupported source claims. | Useful reviewer; less consistent than Pro on the two matched single-file controls. |

The two matched probe/comparison calls cost $0.006519 for 3.5 Flash-Lite versus
$0.017250 for 3.8 Flash, about 2.6 times less. These are reported request charges,
including the actual generated output, not normalized model pricing. The first three
3.5 Flash-Lite requests took approximately 8–13 seconds each. MiMo's completed
reasoning-enabled replies took 20–89 seconds; its longer production review
exhausted 8,192 output tokens after 208 seconds without final text. More reasoning
is not automatically a better listening workflow. A MiMo retry with reasoning
disabled returned a provider empty-response error; no usable ordinary production
review was obtained.

The four full Inkling requests cost $0.029593; four Pro requests cost $0.082112.
These were ordinary paid routes with high reasoning, including actual production
feedback. Pro took 10–27 seconds per request; full Inkling took 6–31 seconds.
The cost difference is too small here to justify choosing a weaker listener when
capability is the priority. The full survey made 37 requests with **$0.185076 in
reported charges**; failures without reported usage are not counted as known cost.

## Selection and unavailable routes

The [live OpenRouter catalog](https://openrouter.ai/api/v1/models) contained 46
audio-input entries. Screening removed 13 batch variants, two moving Google
aliases, two automatic routers, and two duplicate free Inkling variants, leaving
27 named candidates before family-level pruning. This was a catalog survey, not
46 listening tests. Free/contributor routes were eligible, including tiers that
may train on submitted data, as explicitly requested.

| Family | Selection judgment |
| --- | --- |
| Gemini | Tested 3.1 Pro Preview, current inexpensive 3.5 Flash-Lite, legacy price-floor 2.5 Flash-Lite, and requested 3.8 Flash reference. 3.1 Pro was the latest named Pro in the catalog. Skipped intermediate Flash versions, older Lite/preview variants, older 2.5 Pro and the 3.1 Pro custom-tools variant. No evidence here ranks those untested models. |
| Inkling | Tried Small free and full free: both returned HTTP 403 requiring a recognized agentic harness. The direct API caller was not accepted. Tested paid Small and full through Together. DeepInfra rejected Small's audio schema; Together accepted it. |
| Muse Spark | Tested latest 1.3 Contributor, then older 1.2 Contributor to check for a working previous-version route. Both reported no usable audio. [OpenRouter also flags degraded/incomplete 1.3 audio support](https://openrouter.ai/meta/muse-spark-1.3). This is an unavailable listening route, not evidence that its underlying model cannot understand music. |
| Nemotron 3 Nano Omni free | Included because its [model card](https://huggingface.co/nvidia/Nemotron-3-Nano-Omni-30B-A3B-Reasoning-FP8) documents general audio understanding. First request failed with resource exhaustion; a retry said it could not hear the file. No usable listening result on this route. |
| MiMo V2.5 | Included: its [model card](https://huggingface.co/XiaomiMiMo/MiMo-V2.5) describes a native multimodal model with an audio encoder, not merely an ASR service. |
| GPT Audio / Audio Mini; Voxtral Small | Omitted as speech/voice-first offerings under the requested scope. This selection does not claim their training literally excluded music. |

[Inkling Small's card](https://huggingface.co/thinkingmachines/Inkling-Small)
documents native audio, recommends 16 kHz WAV, and reports general audio benchmark
performance close to full Inkling. That made Small a reasonable cheap candidate;
those benchmarks did not predict reliable production observations in this pilot.

## Controls and limits

The event probe was 24 seconds of audio, with no speech:

| Time | Known content |
| --- | --- |
| 0–6 s | Full the first piece hook, source 22.5–28.5 s |
| 6–8 s | Exact digital silence in both channels |
| 8–14 s | Same hook through two cascaded 700 Hz lowpasses |
| 14–20 s | Exact opening PCM repeated, full bandwidth restored |
| 20–24 s | Quiet bell/pad passage, source 69.5–73.5 s; no new kick/bass notes |

Models were asked for vocals, chronological events, silence, tonal changes, and
ending texture without being told the answers. MiMo was the only working model
in this pilot to locate the exact silent gap. All four tested Geminis and both Inkling sizes denied
complete silence. Short clips alone therefore do not guarantee grounded
event descriptions.

The comparison used 12 seconds from source 97.5–109.5 s. A and B were identical;
C passed through two cascaded 450 Hz highpasses. Measured power below 180 Hz fell
44.45 dB. C was also 6.14 dB quieter overall: this was an obvious signal-change
control, **not a level-matched preference test** or proof of subtle EQ expertise.
The models were explicitly allowed to say files were identical. MiMo's provider
rejected three attachments, so it and Inkling Small received separate identical and
altered pair tests. A further single-file control placed the bass-reduced excerpt
first, then two seconds of silence, then the original. The notes were unchanged.

A second single-file comparison used original (0–12 s), bass-reduced
(14–26 s), and exact original again (28–40 s), with two-second pauses. Pro, 3.8
Flash, 3.5 Flash-Lite and full Inkling all correctly identified the identical pair,
middle bass reduction and unchanged notes. This reverses the first single-file
failure for 3.8 and full Inkling, showing sensitivity to presentation or sampling;
it does not erase their earlier confident errors. Pro passed both formats.

An ordinary production prompt also used unmodified 15-second hook and answering
sections. Flash-Lite offered plausible lead/snare separation and kick/bass
balancing experiments, but confidently asserted identical patches despite the
source using different lead designs. Suggestions remain audition candidates;
their eloquence does not establish that the diagnosed problem exists.
Pro and 3.8 also suggested auditioning lead/backbeat balance and kick/bass
separation; full Inkling emphasized lead/bass masking and additional contrast.
These are subjective proposals, not validated improvements. Agreement on generic
balance advice is much weaker evidence than successful concealed controls.

There is one composition, mostly one response per condition, unequal diagnostic
follow-ups, no human panel, and no sensitivity/specificity estimate. Provider
format handling and multiple-attachment behavior can contribute to failures.
These results justify narrow workflow choices, not a universal ranking of ears.

## Reusing the findings

Use the [native-audio review workflow](production.md#external-native-audio-listening-reviews)
with `--model google/gemini-3.1-pro-preview` for the provisional strongest reviewer,
or `--model google/gemini-3.5-flash-lite` for inexpensive comparisons. Give the
reviewer the same musical passage, conceal the version identity, match loudness
for subjective preferences, and ask one concrete question. Request audible
evidence and allow “no change.” Check claims about notes, timestamps and defects
against source, measurements and listening before revising music.

For MiMo, prefer one short file rather than assuming cross-attachment comparison
works. DeepInfra accepted at most two audio attachments in this survey. For
Inkling diagnostics, Together accepted 16 kHz WAV while the cheaper DeepInfra
route returned a schema error. Do not treat a zero `audio_tokens` usage field as
proof of absent audio: MiMo reported zero even when it correctly identified the
concealed silence/filter sequence. Conversely, an HTTP-successful textual reply
is not proof that the provider delivered audio to the model.

The local, ignored `out/audio-model-review/` directory retains the catalog and
endpoint snapshots, exact prompts, source hashes, probe construction scripts,
measurements, provider identity, responses and per-request costs. Audio stays out
of git. Native requests followed the [OpenRouter audio format](https://openrouter.ai/docs/guides/overview/multimodal/audio).
