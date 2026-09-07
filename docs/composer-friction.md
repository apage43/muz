# Composer friction

This is the canonical log of live, unresolved engine and language issues that we
can fix in muz. Record defects and limitations in language semantics, synthesis,
rendering, diagnostics and exposed primitives, including cases that distort
musical choices or force repetitive editing. A valid workaround does not close a
report.

External model quality, provider availability, skill tooling and development
process reports do not belong here.

Commit a report in the same commit as the piece or revision that exposed it. For
work without a piece, name the task and commit the report with that work. Each
entry names its origin, observed behavior, affected decision/work, workaround,
and desired behavior. Include a small reproduction when useful; do not freeze a
whole composition as a test.

Remove an entry in the same commit that resolves the engine or language issue,
after verifying the reported use case. If only part is fixed, keep the remaining
problem explicit. Put usage in reference docs and
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

- **The Clockwork Hart — sampler coverage gaps pass checking and render silently:**
  While mapping the new guitar/flute assets, `muz check` accepted a song with
  performed notes outside every sample zone's key/velocity range. The sampler
  simply returned without starting a voice. This concealed 310 omitted guitar
  notes and one high flute note in early renders. Workaround: inspect the score,
  compare every sampled note against the project zones, complete the missing
  C4 guitar map (the upstream SFZ relies on the default root of 60), and revise
  the flute occurrence into its mapped register. Final coverage is complete.
  Desired: graph preparation/checking should diagnose performed sampler notes
  with no matching zone, naming the track, pitch/velocity and an example source
  key. Consider the available note/zone data first; this needs no new musical
  builtin or automatic pitch remapping. A focused reproduction is
  `song({tempo:120,tracks:[track("gap",note("D4",1b),sample([{path:"tone.wav",root:60,keys:[60,60]}]))],tail:0.1})`
  with any short valid tone WAV: check reports one track/one note, while the
  rendered peak is negative infinity. The task-local synthetic reproduction and
  final coverage audit are under ignored `out/the-clockwork-hart/`.
