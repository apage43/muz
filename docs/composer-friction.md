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

### Prelude-merged record bindings are not usable as values

Origin: High Lonesome Static (piece), which builds its hand-percussion voice map
as `merge(catalogs.drum_voices, {clap: 39, …})`.

Observed: reading a `let` binding that arrives through the standard prelude
yields a function value, so any operation that expects the record fails:

```muz
merge(drum_voices, {clap: 39})
// error: expected a record, got function
```

The same binding through an explicit import is a record and works:

```muz
use "std/catalogs" as catalogs;
merge(catalogs.drum_voices, {clap: 39})   // fine
```

`muz eval` confirms the difference for every prelude-merged name tried
(`drum_voices`, `drum_articulations`, `kit_defaults`, `synth_presets`,
`scale_modes`, `piano_preferences`, `zero_pitch_costs`), and calling one
(`drum_voices()`) reports "record is not callable", so the value is a record for
every reader except the one that receives it as an argument. `drums()`'s default
argument still works, which hides the problem until a composer wants to extend a
catalog table.

Affected decision: extending a catalog table (drum voice names for custom
percussion lanes) required an explicit `use "std/catalogs"`, which reads as
ceremony for a name the prelude claims to provide.

Desired behavior: a prelude-merged binding behaves like the record it is, or the
prelude entry point wraps values so that field access and argument passing agree.

### `pattern.slice` re-bases its window to zero and can return nothing

Origin: High Lonesome Static (piece), reusing verse patterns for a second verse
and re-cutting a bass line for a second instrument.

Observed: `slice(start, end)` selects the window but subtracts `start`, so a
pattern already placed at bar 16 sliced with `(0b, 32b)` returns an empty pattern
instead of the pattern's first eight bars, and slicing `(64b, 96b)` returns notes
at 0..32 rather than 64..96. Both outcomes are silent: an empty section renders as
silence, and a mis-placed repeat renders several minutes late, with no
diagnostic. The transformation list in `docs/language.md` names `.slice(start,end)`
without stating either the re-basing or that the result is silent when the window
lies outside the pattern.

Affected decision: reusing a written section for a later occurrence — the
intended authoring pattern — needed a helper that expresses the window in source
coordinates and re-places the result (`slice(a, b).at(a)`), and a mis-placed
repeat was audible only as an eight-minute render.

Desired behavior: document the re-basing in the language reference and in
`muz docs` output for the transformation (the existing placement/`seq` rules
already explain the rest), so a composer can predict where the sliced notes land.


### Placing an already-placed pattern shifts it again

Origin: High Lonesome Static (piece), doubling a section's hook with another
instrument.

Observed: `.at()` adds its offset, so wrapping a pattern that already carries
absolute placement moves it a second time:

```muz
let line = phrase("B4:w").at(416b);          // last section
let doubled = at(416, line);                 // 832b, not 416b
```

The render is then eight minutes long instead of four and a half, with no
diagnostic naming the pattern or the extra offset; the symptom is silence for
half the file. The mistake is easy to make while adding a part to a score whose
sections are already placed with `.at`, and hard to see because the intended and
actual offsets are equal in source.

Affected decision: adding a doubling part to the final chorus and outro of an
already-placed score.

Desired behavior: the same reference text as the slice report — either an
absolute placement operation, or a diagnostic when a placed pattern is placed
again. `.slice`'s re-basing and this doubling are the two ways a composer can
silently produce a pattern at the wrong time; stating both in the language
reference (and, where possible, naming the pattern in a warning) would cover the
family.
