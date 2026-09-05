# Composer friction

This is the canonical log of live, unresolved composer-facing friction across muz.
Read it before composition and engine work. Record concrete problems as they are
encountered: distorted musical choices, repetitive editing, unclear semantics or
diagnostics, missing abstractions, and workflow failures. A valid workaround does
not close a report.

Commit a report in the same commit as the piece or revision that exposed it. For
work without a piece, name the task and commit the report with that work. Each
entry names its origin, observed behavior, affected decision/work, workaround,
and desired behavior. Include a small reproduction when useful; do not freeze a
whole composition as a test.

Remove an entry in the same commit that fixes the language, engine, documentation,
or workflow responsible, after verifying the reported use case. If only part is
fixed, keep the remaining problem explicit. Put usage in reference docs and
regressions in focused tests. Git history retains reports and their fixes; this
file has no resolved section, remedy archive, or parallel per-piece log. A piece
commit that encounters no new friction needs no ceremonial report. Fixes found
and completed within the same commit need no artificial live entry.

## Open reports

- **Borrowed Light / Iron and Ash — isolated note sends:** tag-driven throws open
  the send for the entire sounding track, so overlapping untagged notes also echo.
  Phrase answers were moved onto manually maintained tracks; splitting piano
  material also risks losing aggregate physical checks. Desired: compact routing
  of tagged notes to an effect bus from one authored logical track, retaining all
  notes and combined piano checks, with processing boundaries made explicit.

- **Borrowed Light / Iron and Ash — background render submission:** a third
  audition fails with `two renders already active`. Workaround: poll jobs and
  submit auditions in pairs. Desired: a bounded, inspectable, cancellable queue
  that keeps two workers active and retains the accepted revision at submission.
