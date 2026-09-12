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

### Project-file structure errors cannot name a line

- **Origin:** diagnostics pass over `muz` error reporting.
- **Observed:** structural validation of a `project.json5` session (unknown
  device parameter, duplicate global id, track instrument kind, undeclared bus,
  schema version) reports the project file but no line or field, because
  `RawSession` deserializes through `json5::from_str` into plain values and the
  spans are gone before any check runs. The JSON5 *syntax* errors do name
  `path:line:column`.
- **Affected decision/work:** a project large enough that the offending id does
  not sit on the first screen forces a manual search for the name in the file
  before the check can be satisfied.
- **Workaround:** search the project text for the id or parameter named in the
  message.
- **Desired behavior:** name the location of the failing value — at minimum the
  JSON5 key path (`tracks[2].instrument.parameters.cutof_hz`), ideally the line
  and column. Resolving this needs a span-preserving project read (a custom
  deserializer that records key offsets) rather than another message rewrite.
