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

### Preset synths reject per-note volume expressions

- **Origin:** The News We Carried, expanded orchestration, September 2026.
- **Observed:** Applying `.express({volume: [[0, 0.58], [0.38, 1], [1, 0.55]]})`
  to a polyphonic `synth("pad")` part fails preflight with “per-note expression
  requires voice_patch or CLAP”. The same independently swelling chords work
  with a native `voice_patch`.
- **Effect/workaround:** Rebuilt the halo instrument as a source voice graph;
  a shared track gain curve cannot represent overlapping chord envelopes.
- **Desired:** Consistent per-note volume/expression support across native
  pitched instruments. Consider implementing preset recipes as source graphs
  over the existing expression-capable voice engine before expanding the kernel.

### Sampler calibration shares gain across overlapping voices

- **Origin:** The News We Carried, expanded orchestral dynamics, September 2026.
- **Observed:** Sampler zones expose pitch/velocity ranges and offsets but no
  amplitude calibration held with the voice. Compensating recording levels with
  `instrument.gain_db` also changes older voices still releasing. When the crest
  changes recorded velocity layers, the viola's measured peak rose by about
  7.6 dB after adding a crest arc with softer closing bars.
- **Effect/workaround:** Prepared constant-gain float WAV derivatives, each
  sustain at -24 dBFS body RMS, and used shared gain only for modest bow/breath
  shaping. Original recordings remain unchanged; this needs extra files and an
  external preparation step.
- **Desired:** A general per-zone amplitude value captured by each sample voice,
  or equivalent per-note amplitude independent of velocity-layer selection.
  Keep calibration measurement and target-level policy in project/source recipes.
