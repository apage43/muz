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

### Internal clip timing still shares user metadata

Origin: architecture planning task at `ea95e537` (2026-09-13). Lowering now
checks malformed numeric clock markers and reports source-attributed errors,
but the internal `clock_start`/`clock_duration`/`clock_span` triplet still
shares ordinary annotations. Workaround: construct clips with `clip` and avoid
those metadata keys. Desired: dedicated validated timing that ordinary user
metadata cannot activate or overwrite.

### Numeric boundaries accept incompatible units and truncate integers

Origin: architecture planning task at `ea95e537` (2026-09-13), synthetic songs
and evaluator probes. Meter `[3.5,4]` becomes `[3,4]`, `[256,4]` saturates to
`[255,4]`, and `[3s,4Hz]` is accepted. `tempo:120Hz`, `tail:1b`,
`note(60Hz,1b)`, `note(60).repeat(2s)` and `cc(64Hz,1)` are accepted;
`[1,2][0.5]` returns the first element. These silently reinterpret musical/control
intent and hide mistakes. Workaround: supply correctly dimensioned values and
explicit integral counts/indices. Desired: preserve documented scalar defaults,
reject incompatible dimensions, and validate integrality/ranges before casts.

### Repeat expands controller and raw streams beyond their limits

Origin: architecture planning task at `ea95e537` (2026-09-13), small evaluator
probes. `control(64,0).repeat(21).repeat(10000)` and
`cc(64,0).repeat(21).repeat(10000)` each materialize 210,000 events; only subsequent
pattern validation rejects them. Repeat preflights notes but not these streams,
so larger inputs can consume excessive memory before rejection. Workaround:
bound expansion manually. Desired: preflight every expanded stream and variable
payload cost before cloning, including adjacent fit/overlay operations; do not
confuse composition memory limits with realtime event capacity.

### Tempo revision flushes a compatible held note

Origin: architecture planning task at `ea95e537` (2026-09-13), four-beat constant
voice and prepared-transaction probe. After playback begins, changing only tempo
from 120 to 100 BPM drops output from `0.14142136` to zero. The scheduler adopts
the active note, then the changed timeline's discontinuity flushes it; reload
suppresses seek catch-up. Delivered counters show one note-on and no note-off.
Ordinary note-removal retention tests still pass. This interrupts sustained
material during tempo revision. Workaround: stop/restart when changing tempo.
Desired: compatible source tempo edits preserve the original held-note release
and expression obligations; explicit seek/restart/panic retain their intentional
flush behavior.
