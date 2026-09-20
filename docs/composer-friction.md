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

### Source-level span contracts are repeated composition boilerplate

**Origin:** External composer handoff covering `neon-on-repeat.muz`,
`know-better-v3.muz`, `the-name-it-could-not-swallow.muz`,
`borrowed-morning.muz`, and `until-the-street-runs-out-v2.muz`.

**Observed behavior:** `a.passage(span, ...)` requires a positive logical span
and `a.sequence` advances by that span, but no opt-in contract checks whether
local material fits within it. Muz also has no reusable exact-span assertion for
ordinary patterns. A passage declared as `4b` with an `8b` part is accepted and
its material overlaps the next occurrence. Calling `a.require_span(...)` fails
because the helper does not exist. The existing permissive passage behavior is
valid for pickups and release tails and should remain available.

**Affected decision/work:** Form-driven pieces repeatedly define local helpers
that assert every part fits its passage and that phrases, bars, couplets, or
whole sections have the expected span. Without those guards, a phrase edit can
shift later material or bleed into the next occurrence instead of failing near
the authored source.

**Workaround:** Each score defines assertion wrappers around `pattern.span`,
usually once for exact phrase spans and again while constructing strict
passages.

**Desired behavior:** Provide reusable, opt-in source-level span contracts for
exact span and containment. Containment must be able to distinguish attacks,
release tails, and pickups so strict form checks do not replace the existing
permissive semantics.

### Native voice patches cannot name custom per-note controls

**Origin:** External composer handoff from `borrowed-morning.muz` and
`until-the-street-runs-out-v2.muz`.

**Observed behavior:** Note expression accepts only `volume`, `pan`, `tuning`,
`vibrato`, `expression`, `brightness`, and `pressure`. Both
`s.expression("mute")` and `.express({mute: 1})` are rejected as unknown note
expressions, even for native voice patches.

**Affected decision/work:** Source-defined instruments with note-local
dimensions such as open/muted state, pick position, vowel, or articulation must
mislabel one as a fixed MPE-style expression. The affected ukulele patches use
`pressure` to mean open versus muted, consuming that lane and making the source
misleading. `std/instrument.variation` likewise occupies `pressure` for a stable
variation stream.

**Workaround:** Repurpose a semantically unrelated expression lane, split the
material across instruments or tracks, or omit the expressive dimension.

**Desired behavior:** Let native voice patches declare and consume named
per-note controls while retaining the standard expression names and their
external CLAP/MPE semantics. Custom controls do not need implicit export to
formats that cannot represent them.

### Contextual note transforms require repeated whole-pattern searches

**Origin:** External composer handoff from
`until-the-street-runs-out-v2.muz` and `still-saving-your-place-v3.muz`.

**Observed behavior:** `pattern.notes` exposes note records and `map_notes`
rewrites them, but its callback receives only the current note. A transform that
needs a predecessor or successor must capture the full note list, establish an
ordering, recover the current note's position, and then select adjacent notes.
The shorter common workaround filters the entire list by end time for every
note and treats the first coincident result as the successor.

**Affected decision/work:** Selective legato, breaths, slurs, rearticulation,
and interval-sensitive performance rules duplicate verbose searches. Repeated
filtering is potentially quadratic, and choosing the first coincident note is
ambiguous once material is polyphonic.

**Workaround:** Sort and index `pattern.notes` in score source, or repeatedly
filter the captured list and rely on a monophonic-score assumption.

**Desired behavior:** Provide source-level traversal that supplies stable,
explicit predecessor/successor context and can preserve separate voice runs,
without adding a singing-specific policy.
