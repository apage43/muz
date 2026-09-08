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

### A Turn Too Soon: local meter cannot reach section metadata or MIDI

Origin: the Hookpad-derived trio piece `projects/a-turn-too-soon`, whose middle
contains sixteen 3/4 measures between 4/4 passages (quarter-beat positions
152–200). Explicit three-beat patterns, shared tempo and source performance
functions express and render the music correctly. However, `song` accepts only
one global `meter`; section inspection reports 4/4 throughout, and song MIDI
export writes only that initial time signature. A `meter` field added to a
section is silently ignored:

```muz
song({
    tempo: 120,
    meter: [4, 4],
    sections: [
        section("four", 4b),
        merge(section("three", 3b), {meter: [3, 4]})
    ],
    tracks: [track("tone", note(60, 7b), synth("glass-lead"))]
})
```

`muz inspect ... --view sections --json` reports `[4,4]` for `three` as well.
This prevents the authored metric change from reaching section labels, transport
context and song MIDI metadata. The piece's workaround uses explicit `3b` measure
spans and passes that width to its performance functions; its README records
the intended meter timeline. The musical audio is unaffected.

Desired behavior: a general, beat-positioned meter representation that can reach
those consumers, with arrangement placement and local measure recipes remaining
source policy. Reject unsupported section fields rather than silently discarding
an apparent meter override. Verify with small mixed-meter synthetic cases; do not
use the piece as a regression fixture.
