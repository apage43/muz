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

### Sequencing inexact beat offsets can silently move events backwards

- Origin: **The Lights Stay On**, the slower rewrite of the Hedgerow guitar piece.
- Observed: an inexact strum offset becomes a large rational at the raw-event
  boundary. Shifting that event with `seq` can silently produce a wrong onset.
  This reproduction should place its note at approximately 32.052981 beats,
  but `muz eval` reports approximately 0.148464 beats:

  ```muz
  seq([rest(32b), note_on(57, 0.5,
      at = 0.025b + 2 * (0.013b + 0.001b * sin(14)), channel = 2)])
  ```

- Affected work: deterministic variation in brush spacing caused false string
  collisions during the full guitar arrangement's preparation.
- Workaround: use small exact rational beat increments for brush timing;
  retain sine-derived variation only for velocities.
- Desired: pattern shifts preserve the intended finite onset, or report a
  representational overflow before producing an invalid schedule. The language
  documentation already promises explicit overflow for exact dimensional time.
