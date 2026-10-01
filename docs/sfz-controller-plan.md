# SFZ constructor controller validation resolution

## Scope

Resolve the published Pocket Customs controller-constructor report without a new language primitive, instrument option format, or workaround. Preserve current main's timed group-choke correction.

## Implementation

1. Route SFZ constructor parameters past the song compiler's static-table-only check and through the existing `description::validate_device` / `validate_control` path. That path already owns controller spelling and finite 0–127 MIDI-unit ranges, including fractional defaults. Keep validation before audio preparation and retain device/source diagnostics.
2. Verify constructor overrides survive compilation and supersede authored `set_cc` defaults through the existing device control-values path. Existing preparation/reset tests cover the runtime default lifecycle; no audio engine behavior needs changing.
3. Extend native SFZ documentation with an actual constructor controller example and explicit valid name/range behavior.

## Verification and invariant

Add one focused case in the existing SFZ integration suite. The likely-to-break invariant is that the source constructor and the shared device validator agree on physical SFZ controls; the current failure is caused by those paths diverging. Cover boundary controllers, fractional override values, and invalid names/ranges in that case. Do not introduce a piece-specific regression fixture.

Run the original minimal `muz check` reproduction, existing SFZ integration/description/default-reset checks, relevant compiler/diagnostic checks, and repository formatting/build checks. Review the diff and remove the canonical report and this completed plan in the fixing commit. Push the fix only after checks pass.

## Piece follow-through

Use current `muz-projects/main`. Move initial CC1/7/11 and bass/drum setup into `sfz(..., options)` via the palette generator; remove initial score-start controller messages. Keep only genuine later section dynamics as score automation. Retain previews and earlier full render. Rebuild with the fixed current engine, check/render/encode the unchanged approved arrangement, update engine/source metadata and documentation, and push the music source. Replace the existing full-piece Library artifacts with guarded versions; leave preview identities untouched.
