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

### Formatter layouts obscure expression and argument structure

- **Origin:** formatter improvement proposal task, inspecting `std/performance.muz`
  and `projects/what-the-wind-keeps/production-performance.muz`.
- **Observed:** `muz fmt` has no wrapping points at binary operators or after a
  definition's `=`. Long bodies can therefore split short inner calls or the
  function parameter list while still exceeding the 100-column target. For
  example, this source formats with a 108-column final line, after expanding the
  parameter list:

  ```muz
  fn choose_level(velocity, phrase, section, articulation) = velocity + phrase + section + articulation + velocity * phrase + section * articulation;
  ```

  Multiline callback arguments introduce separate lines and indentation for the
  receiver, method, lambda and body. Calls with an expanded collection followed
  by a scalar put the scalar below the collection's closing delimiter at a deeper
  indent (`curve([...], "smooth")`). Conditional bodies collapse to `{value}`;
  one branch can remain inline while its sibling expands. Blank lines separating
  groups inside ordinary arrays disappear.
- **Affected work:** scanning performance expressions, callback transformations
  and automation data; formatter output obscures the relationships between
  clauses, arguments and intentional musical groups.
- **Workaround:** shorter expressions and extracted helpers can reduce wrapping;
  manual whitespace changes are overwritten by the next formatting pass.
- **Desired behavior:** use syntactic expression boundaries for wrapping,
  consistent layouts for multiline calls and conditionals, compact callback
  delimiters, and preservation of deliberate blank-line groups in collections.
  Retain token/string/comment fidelity and idempotence. Verify with small
  synthetic layout cases rather than fixing a composition's text as a fixture.

### Nested evaluation errors obscure the source location

- **Origin:** composing *What the Wind Keeps* (`projects/what-the-wind-keeps/`).
- **Observed:** an invalid `offset` argument to `note` inside the accompaniment's
  nested `map`/`stack` helpers produced a single diagnostic with many repeated
  absolute module paths and raw byte offsets before `unexpected arguments:
  offset`. Even this small reproduction emits eight repeated path/byte contexts:

  ```muz
  fn make_note() = note("C4", 1b, offset=2ms);
  fn phrase_layer() = stack(map(range(2), fn(i) => make_note()));
  fn passage_layer() = stack(map(range(2), fn(i) => phrase_layer()));
  passage_layer()
  ```

- **Affected work:** locating the invalid call among nested accompaniment helpers
  required inspecting source separately; the error gives neither a line/column
  nor a displayed source span. The rejected argument itself is correctly rejected.
- **Workaround:** locate the innermost call and use the existing note transform
  to set its performed `offset`. The composition now checks and renders.
- **Desired behavior:** show the offending source line/span and line/column first,
  with a compact, deduplicated caller trace when useful. Preserve meaningful
  module boundaries without repeating an absolute path at every expression.
