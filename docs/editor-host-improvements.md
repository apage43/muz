# Pending editor-host improvements

## Conditional inspection indexes and stable metadata

This is a profile-gated extension, **not** a prerequisite or unconditional compiler
rewrite. The [workbench viewport owner](../../muzpad/docs/workbench-improvements.md)
first fixes bounded caching/request coalescing, loaded coverage, scheduling and
coordinator isolation where measured. Only if those measurements still identify
core range scans/overview work or missing producer metadata as material blockers
should an owning core commit select the smallest necessary item here.

Use existing [`inspect::PageRequest` / page envelopes](../src/inspect.rs) and
[inspection contracts](embedding.md#paged-inspection). Preserve retained revision
identity, exact ticks, row/byte limits, totals, first-omitted-row continuation,
stream ordering and explicit errors. Do not restore full unbounded performance
snapshots or increase budgets as a speed fix.

Eligible measured changes are revision-owned off-RT indexes for sorted point-event
ranges and interval-aware note overlap, bounded precomputed overview/LOD data, or
compact stable whole-track metadata such as pitch bounds. A start-only binary
search is incorrect for sustained notes entering from the left. Index construction,
retained bytes, retirement and query CPU must fit explicit measured budgets;
indexes must not leak across revisions or add callback work. Do not clone a second
accepted session/compiler merely to service inspection.

Keep semantic distinctions: current pitched overview is duration occupancy,
including unique active pitches; kit hit density is onset count, already available
as the additive `onsets` aggregate. Stable pitch bounds come from the complete
accepted track, not the current tile; a host can instead choose a stable scale
policy without any core API change. Complete track-catalog paging already exists;
no catalog endpoint is added just because a caller ignores continuations.

If the profile gate opens, extend [`inspection_pages`](../tests/inspection_pages.rs)
with synthetic long notes crossing left/right boundaries, exact boundary/empty
ranges, controller/message/tempo pressure, offset/byte continuations, pitch-bound
stability and density/LOD consistency. Compare indexed results to existing precise
range semantics and record construction/query time and retained bytes on the same
bounded synthetic corpus. No implementation or speedup is assumed here. Update
live inspection documentation only for changes actually shipped.
