---
name: muz-ab
description: Run blind audio comparisons with selectable loops, synchronized A/B/X switching and repeated-trial statistics. Use when testing audible production differences or collecting blinded listening preferences.
---

# Muz blind comparator

Use the bundled local comparator to collect the listener's evidence before choosing
production settings. ABX asks whether the listener can identify the files;
preference asks which file they want. Keep these conclusions separate.

## Prepare a controlled pair

1. Choose one production variable. Keep the performance, tempo, source assets,
   render context and processing outside that variable fixed. Render bounded
   excerpts with a named `muz batch` recipe and `--match-levels`; the public
   [production reference](../../../docs/production.md) describes recipe scopes.
   Section rendering preserves preceding instrument/effect context. Completion:
   two successful, aligned bounces with finite integrated-LUFS analysis in
   `renders.json` and an explicit description of what differs.
2. Use lossless bounces, not independently encoded lossy copies. The comparator
   attenuates the louder selected file to the quieter one, removes container
   metadata from its listening copies, and leaves originals unchanged. It rejects
   mismatched rates/channels or duration differences beyond one sample; matching
   dimensions do not prove musical alignment. Stochastic instrument rerenders can
   introduce additional differences: ABX concerns the concrete files, not proof
   that the intended parameter caused every audible difference.

## Launch and hand over

Run from the muz-core root. Python's standard library, `ffmpeg` and `ffprobe` are
required; there is no package install or frontend build. Discover flags with
`--help`. A manifest with exactly two successful outputs is selected automatically;
use `--a` and `--b` when it contains more. Give the private source labels descriptive
names, rather than making the eventual report say only “A” and “B.”

```sh
python .agents/skills/muz-ab/scripts/serve.py \
  --manifest /path/to/comparison/renders.json \
  --output /path/to/listening-session \
  --label-a "Current mix" --label-b "Room return -6 dB" \
  --title "Room comparison" --port 8765
```

Launch this long-running command through the process supervisor and observe its
`READY` URL. It binds only to loopback. Keep generated media, private state and
results in ignored `out/` or the user's delivery directory. Source material remains
in its piece's sibling `muz-projects` directory.

Open the URL and verify the actual interface: select a region, start playback,
switch candidates, and observe looping and pause/resume. Use a separate disposable
output directory for implementation smoke trials; the listener's session starts
fresh. Hand over the URL with the controls, not a suggested answer.

## Collect without peeking

- The listener chooses ABX or preference and the planned trial count before
  starting. Twenty trials is a practical starting point, not a power guarantee.
- Drag on the shared waveform to select a loop; drag its boundaries or use the
  numeric fields to refine it. Click to seek. Space plays/pauses; 1/2/3 select
  A/B/X; Home restarts. A common volume control affects every candidate.
- All candidates share an audio clock, loop bounds and playhead. Switching uses
  a short linear crossfade. Native looping has a common edge taper; a background
  timer freeze longer than its lookahead can leave a boundary untapered, without
  breaking synchronization. Seek and region edits restart at the selected point.
- Source positions randomize per trial; ABX's X is independently assigned.
  Labels, answers and running scores stay hidden until completion. Each recorded
  response advances the trial and is immutable. Pausing or reloading preserves
  recorded responses and the pending challenge; restart the same server command
  to recover after a process restart.
- Let the user listen and respond. Keep private state files and source assignments
  out of the listening conversation until the session ends. The tool blinds its
  interface; deliberate inspection of files or audio can defeat that blinding.

Completion: the predeclared trial count is reached, or the listener explicitly ends
early. An early stop reveals descriptive counts, without a fixed-length p-value or
confidence interval. Do not manufacture responses or pool smoke trials with human
trials.

## Read the finished evidence

Completed JSON and CSV reports are saved under the output's `results/` directory
and downloadable from the page. They include source provenance, assignments,
responses, loop bounds and optional notes. Read finished reports, not private state,
to discuss the user's result.

ABX reports an exact one-sided binomial chance tail and a Wilson accuracy interval.
Preference reports underlying-candidate counts, ties separately, and a two-sided
exact binomial test on decisive choices. These are nominal within-session results
for one listener and pair. Multiple pairs/sessions, learning and selective reporting
limit broader claims. A nonsignificant result is not evidence of equivalence; an
ABX success is not a preference judgment. Confirm consequential findings with a
fresh predeclared session rather than extending a run until its score looks good.

Apply only the production choice supported by the listener's decision. Preserve
canonical masters until that decision; document the selected setting and finished
report alongside the piece. Follow the repo's normal rendering verification budget.
