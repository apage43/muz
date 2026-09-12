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

### Full contrib sample maps exceed programmable-reader preparation capacity

- **Origin:** contrib instrument follow-up audit, 2026-09-12, after native sound-design work.
- **Observed:** programmable patches cap all distinct decoded assets at 8,388,608
  frames. The pinned VSCO cello, viola and violin maps contain 12,097,215,
  11,950,272 and 11,490,344 frames respectively (from calibration provenance).
  METAL-GTX's 84 original sustain-down files contain 25,930,800 frames (FLAC metadata).
  Sharing left/right readers avoids duplicate storage but cannot fit these maps.
- **Affected work:** full-register sampled voices with independent filtering/layers,
  and replacing METAL-GTX's mono derivatives with original stereo channel readers.
- **Workaround:** source filters zones to a selected register; retain the standalone
  samplers and existing METAL-GTX derived files for full maps.
- **Desired behavior:** a general, explicit preparation/resource policy that admits
  practical multisample maps with actionable required/allowed-memory diagnostics,
  while preserving bounded preparation and allocation-free audio processing. Evaluate
  configurable decoded-asset budgets and shared storage before expanding the kernel;
  this does not by itself require disk streaming.
