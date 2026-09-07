# Selecting a native music reviewer

Use this reference for music critique through OpenRouter. These choices reflect
the first piece listening tests from September 2026, not a general ranking. Check the
[current catalog](https://openrouter.ai/api/v1/models) and provider support when
availability changes. Native audio capability does not establish reliable mix
judgment; test observations against known audio and preserve uncertainty.

| Model | Practical use and limits |
| --- | --- |
| `google/gemini-3.1-pro-preview` | Provisional capability choice. Most consistent on obvious EQ/unchanged-note controls, but missed silence and invented differences between exact duplicates in subtle mix comparisons. |
| `google/gemini-3.8-flash` | Useful second opinion with the same important grounding limits. |
| `google/gemini-3.5-flash-lite` | Inexpensive helper for broad comparisons. Lower cost was demonstrated; superiority over 3.8 was not. |
| `xiaomi/mimo-v2.5` | Useful coarse event descriptions within one file. Unreliable cross-attachment comparisons; can confuse processing changes with changed notes. Reasoning-heavy requests can be slow or exhaust output without a final answer. |
| `thinkingmachines/inkling` and `thinkingmachines/inkling-small` | Full Inkling showed no clear advantage over the Geminis for music. Both sizes gave inconsistent event and comparison descriptions. |

Prefer the capable model's cheap route when available; larger models remain
eligible. In the small tests, full Inkling and Pro reviews each cost only cents.
Respect the session's data-use preferences rather than imposing ZDR or excluding
free/contributor tiers automatically.

## Routing details worth checking

- Google's native audio routes worked through Google and Google AI Studio.
- Inkling accepted audio through Together. DeepInfra rejected Small's audio
  schema. Free Inkling required a recognized agentic harness and rejected the
  direct caller; a paid route avoided the access gate. Its
  [model card](https://huggingface.co/thinkingmachines/Inkling-Small) recommends
  16 kHz WAV.
- MiMo through DeepInfra accepted at most two audio attachments. A single file
  containing separated excerpts can avoid attachment limitations, but does not
  guarantee correct comparisons.
- Muse Spark 1.3/1.2 Contributor and Nemotron 3 Nano Omni free did not provide
  usable listening on the tested routes. Recheck actual audio delivery before
  spending effort on critiques. [Muse's route](https://openrouter.ai/meta/muse-spark-1.3)
  also advertised incomplete audio support at the time.
- Do not spend music-review calls on speech/voice-first services without evidence
  of relevant general sound understanding. Skip automatic routers and duplicate
  aliases when attributing capability to a model.

Use the [native audio request format](https://openrouter.ai/docs/guides/overview/multimodal/audio).
Reported audio-token counts vary by provider: zero alone does not establish that
audio was absent. Verify localized observations, duplicate controls and final
content. Agreement on generic production advice is weaker evidence than grounded
comparisons; no model listed here reliably settles subtle mix decisions.
