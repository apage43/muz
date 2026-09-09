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

### Fixed sampler zone limit constrains multi-articulation recording maps

- **Origin:** revising guitar production for *What the Wind Keeps*.
- **Observed:** mapping 25 sampled roots with eight recorded takes each from
  Standard Guitar into one sustain instrument fails with
  `sample needs 1..128 zones`. Each articulation has 200 ordinary sample zones;
  splitting sustain and mute into separate kit voices does not avoid the
  per-instrument limit.
- **Affected work:** retaining both the library's chromatic root coverage and
  all eight takes in the working register requires pruning or further splitting
  an otherwise ordinary sample map.
- **Workaround:** use 15 sampled roots with eight takes (120 zones), retaining
  exact roots for every currently played pitch and nearest-root coverage between
  them. The source remains editable, but later register changes may require
  revisiting this tradeoff.
- **Desired behavior:** account for sample-zone preparation in a general resource
  budget with a useful size diagnostic, rather than a small fixed limit that
  requires instrument-specific map partitioning. Verify any expansion with
  small synthetic zone maps rather than this piece or its external recordings.

### Quoted keywords can be interpreted as syntax

- **Origin:** formatter layout implementation task, testing preservation of
  punctuation and keywords inside strings.
- **Observed:** both the previous binary and the updated formatter reject
  `let x = "if";` with `expected expression, got ';'`. The parser's syntax checks
  use a token's decoded text without consistently distinguishing string tokens.
  Formatting cannot proceed because the input fails the initial parse.
- **Affected work:** writing ordinary text values whose contents match language
  syntax, and verifying formatter fidelity for those values.
- **Workaround:** `let x = "i" + "f";` parses and formats successfully.
- **Desired behavior:** interpret keywords and punctuation as syntax only for
  non-string tokens. Keep quoted values literal in all parser lookahead and
  consumption paths, with small parsing and formatting regressions.

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
