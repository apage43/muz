# Composer friction

This is the canonical protocol and live log for unresolved engine and language
issues that can be fixed in muz. Read it before composing, revising a piece, or
changing the engine. Repository boundaries and the verification budget are in
[AGENTS.md](../AGENTS.md).

[Documentation index](README.md)

## What belongs here

Record defects and limitations in language semantics, synthesis, rendering,
diagnostics, and exposed primitives. Include issues that distort musical choices
or force repetitive editing, even when a workaround lets the piece proceed.
A valid workaround does not close a report.

External model quality, provider availability, skill tooling, and development
process reports do not belong here.

## Recording a report

Commit a report in the same commit as the piece or revision that exposed it.
For work without a piece, name the task and commit the report with that work.
Each entry includes:

- **Origin:** the piece, revision, or task that exposed the issue.
- **Observed behavior:** what muz does and the relevant conditions.
- **Affected work:** the musical decision or work the issue obstructs.
- **Workaround:** how the work proceeded, if possible.
- **Desired behavior:** the capability or correction needed.

Include a small reproduction when useful. Do not freeze a whole composition as
a test. A piece commit with no new friction needs no report; a problem found and
fixed within the same commit needs no artificial live entry.

## Choosing a resolution

Seek a general solution that expands the builtins/kernel as little as necessary
while fully resolving the reported problem. First ask whether existing language
operations and exposed data can express the solution clearly. Put reusable
policies and recipes in `std/`, particular musical choices in project source,
and demonstrations of useful techniques in examples.

When source cannot express the solution well, identify the missing general
capability. Prefer a small, composable primitive or better access to musical data
that enables a family of solutions over a builtin for one special case. Keep
shapes, selection rules, and overlap policies in source wherever practical.

Kernel changes remain appropriate for engine defects, runtime guarantees, and
capabilities that require them. Minimizing the kernel must not retain awkward
workarounds or move complexity into every composition.

In the fixing commit, explain why existing facilities suffice or why the new
primitive belongs in the kernel. Demonstrate the specific resolution in source.
For example, tag-derived automation uses exposed note data and timing operations
so composers can write their own automation recipes. This principle applies to
all friction items.

## Closing a report

Verify the reported use case, document usage in the relevant reference, and add
focused regression tests. Remove the entry in the same commit that resolves the
engine or language issue. If only part is fixed, keep the remaining problem
explicit.

Git history retains reports and fixes. This file has no resolved section or
remedy archive; do not maintain parallel per-piece friction logs.

## Open reports

### Sonatina cymbal setup reports a parser error but succeeds

- **Origin:** `muz-projects/boss-battles/the-name-it-could-not-swallow-orchestral`,
  orchestral version preparation and rendering.
- **Observed behavior:** Generating state for `percussion-cymbals-tamtam` and
  checking/rendering the piece with sfizz 1.2.3 VST3 prints
  `Parse error in "Cymbals & Tamtam.sfz" at line 4: Expected opcode name.`
  The pinned upstream program's fourth line begins with `/`, not `//`.
  State preparation and `muz check` still succeed. A cymbal-only render produces
  audio, so the player recovers to the mapped sample regions in this case.
  The project's cymbal-only excerpt at 12.8 seconds also rendered
  recovered audio (sample peak -30.83 dBFS at its initial mix settings).
- **Affected work:** The first use of this pack required a separate playback
  check to distinguish a recovered malformed header from a failed percussion
  instrument; successful state preparation alone does not establish that.
- **Workaround:** Leave the verified upstream library unchanged, verify the
  actual cymbal output, and document the warning in the piece's `ASSETS.md`.
- **Desired behavior:** A corrected, verified cymbal program that loads without
  the parser error, preserving its original sample mapping and licensing.

### Explicit clock-time excerpt length overrides the release-tail option

- **Origin:** `muz-projects/boss-battles/the-name-it-could-not-swallow-orchestral`,
  cymbal-only render review.
- **Observed behavior:** Rendering with `--solo cymbals --start 12.8 --seconds 2
  --tail 2` reports both options but writes exactly 96,000 frames at 48 kHz
  (2 seconds), not a 2-second excerpt plus 2 seconds of release.
  `src/render.rs` selects `seconds.unwrap_or(end - start)`, so explicit seconds
  overrides the tail-inclusive duration. The workflow's option table describes
  `--tail` as additional release time without stating this precedence.
- **Affected work:** Clock-time percussion auditions cannot assume a requested
  tail extends the explicitly supplied excerpt length.
- **Workaround:** Treat `--seconds` as the entire output duration; the full
  delivery omits it and correctly includes the score's six-second tail.
- **Desired behavior:** Document the exact precedence in workflow and CLI help,
  or provide an unambiguous score-window-plus-release scope.
