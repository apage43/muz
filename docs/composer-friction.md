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

### A song that imports its players' files runs out of the evaluation budget

- **Origin:** The Long Debt v1, A2 integration (`song.muz` importing `drums.muz`,
  `bass.muz`, `guitar.muz`, `keys.muz`, `strings.muz` and `synth.muz`).
- **Observed behavior:** every module imported into one evaluation shares a
  single budget of 5,000,000 operations (`host.rs`), and nothing on the command
  line raises it. Each player file evaluates alone within the budget, and each
  player checked `song.muz` with only their own part wired in, so the overrun
  first appeared at integration: `muz check song.muz` failed with `evaluation
  budget exceeded` inside `std/performance`'s binary search, called from
  `guitar.muz` (or `synth.muz`, whichever happened to evaluate last). Measured by
  burning a known number of operations after each import: bass about 2.1 M (its
  kick/bass lock self-test indexes every note by bar), strings about 1.05 M,
  drums, keys and guitar about 0.5–0.6 M each, synth 0.3 M. Each file's
  self-test runs on import, because a module's top-level `let`s and final
  expression are always evaluated.
- **Affected work:** the band workflow, where each player's file asserts its own
  self-checks at evaluation (`muz eval part.muz` fails if one breaks). Those
  assertions are worth keeping, but they are paid again by every importer, and
  the sum decides whether the song evaluates at all. The error names whichever
  file ran last, not the one that spent the budget.
- **Workaround:** each player file's self-test section is now inside
  `fn self_test() { ... }` and is run with `muz eval part.muz --expr
  'self_test()'`. The exported parts were proved identical before and after
  (hashes of every part's notes and controls).
- **Desired behavior:** either a budget per module (or one that scales with the
  number of modules), a way for a module to mark checks that run only when it
  is the entry point (`muz eval part.muz`) and not when it is imported, or at
  least a budget error that reports the operations spent per module.

### An import alias silently shadows a builtin

- **Origin:** The Long Debt v1, A2 integration.
- **Observed behavior:** `use "keys.muz" as keys;` is accepted and replaces the
  builtin `keys()` for the whole module. The failure surfaced only in a song
  check far from the `use` line: `song check 'one_guitar' (checks[1]) ... record
  is not callable`, pointing at the check's registration, not at the call to
  `keys(...)` inside the check function or at the alias.
- **Affected work:** naming a module after its file (`keys.muz` for the keyboard
  player) is natural; the error took a bisection to explain.
- **Workaround:** the alias is `theo`.
- **Desired behavior:** a warning (or error) when a `use ... as NAME` or `let`
  shadows a builtin that the module also calls, or a "not callable" message
  that names the shadowing binding and where it was defined.
