# Remaining architecture work

This is an active checklist, not a historical report. Remove completed items in
the same commit that implements and verifies them. Delete this file when empty.
Permanent behavior belongs in the language, synthesis, performance and embedding
reference docs. Git history preserves completed plans.

## Scheduler safety and preparation (S7/S9)

- Reject schedules that exceed fixed active-note or callback-event capacities
  before acceptance, including expression, releases, seek restoration and
  obligations retained across repeated revisions.
- Replace callback history scans with bounded controller/message restoration
  and indexed sounding-note lookup.
- Compile one coherent timeline per candidate and reuse it across tracks.
- Verify burst/overlap rejection, seek/reload restoration, tempo/extent/mode
  transitions, queued rejection/correction, and allocation-free application.

## Typed description pipeline (S11)

- Introduce a private-construction validated session boundary.
- Carry typed patches from checked conversion into runtime preparation, with
  resolved input indices, output/lifetime/resource policy and separate controls.
- Replace runtime JSON reparsing and JSON-stripping structural comparisons.
- Consolidate configuration-independent field/default/range checks while retaining
  resource, sample-rate and native-plugin validation during preparation.
- Verify source/DTO equivalence, invalid payloads and shared/independent graph state.

## Context and asset completion (S12)

- Check operation cancellation throughout expansion, preparation and decoding.
- Capture one per-instance preparation budget for validation and telemetry.
- Bind asset versions to opened immutable content, with bounded reads and distinct
  source-attributed missing/stale/unsupported errors.
- Route plugin-state content through the byte service where supported.
- Share decoded assets by identity/version/settings without sharing playback state.
- Verify isolation, stale-content rejection, cancellation and portable compilation.

## Provenance and semantic diffs (S13)

- Track interned occurrence context and the latest relevant edit, rather than
  merely the last call returning a pattern.
- Attribute imported MIDI to asset/order and retain truthful source-span fallbacks.
- Report note-field, occurrence and controller changes separately from metadata,
  with detailed processor/route/control/schedule consequences.
- Verify named insertion stability, nested placements, sparse edits, expansion,
  removal, rename and ambiguous identities; keep responses and sidecars bounded.

## Remaining boundary and inspection migration (S4/S8/S10)

- Preserve curve-value units during sampling and reject mixed dimensions.
- Add explicit units/effect classifications to shared parameter descriptors;
  retain conservative replacement for unproven/dynamic controls.
- Bound and paginate remaining detailed inspection surfaces and integrate bounded
  summaries into their consumers.
- Complete hostile transfer/summary validation and focused worker-transfer coverage
  for CC, tempo, expression, selected samples and unsupported data.

## Muzpad consumer integration

Work belongs in the sibling muzpad repository. Follow its AGENTS.md; do not deploy
without a new explicit publishing request.

- Replace its private snapshot wrapper with the shared checked transfer API.
- Use prepared revisions on the retained rendering engine instead of replacing
  the rendering worker for every accepted edit.
- Expose supported in-memory MIDI/WAV/FLAC assets through host contexts and imports.
- Consume bounded inspection/provenance APIs and surface revision consequences.
- Verify worker transfer, invalid-edit retention, held-note reloads, asset imports,
  navigation, cancellation and browser playback.

No rest/spread, macros/classes, static unit system, incremental evaluator,
alternate arrangement language, multi-crate migration or browser-native plugins
are scheduled by this checklist.
