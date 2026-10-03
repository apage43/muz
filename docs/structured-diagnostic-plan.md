# Remaining structured-diagnostic work

Observed: `AudioEngine::validate_automation` converts source diagnostics to strings for latency-changing, duplicate-lane, and unknown targets. Native consumers cannot recover location/help from the error chain.

1. Retain boxed `Diagnostic` values in `EngineError::Source` and expose them through `std::error::Error::source`, retaining the existing terminal display and error equality/clone contract. Migrate all three constructors; use structured fields, never parsed terminal text.
2. Add synthetic consumer regression coverage for unknown targets, latency-changing targets, and duplicate lanes. Wrap engine errors in `anyhow` context, recover `Diagnostic` through the source chain, and assert location/help plus unchanged display. Check the portable no-default-features library build.
3. Run only these affected regressions and the portable compile check. Document the source-chain contract in the embedding reference, remove this implemented plan, and commit only owned core files (not composer-friction or unrelated plan edits).
4. Send the verified core SHA and exact source-chain contract to the native runtime owner and web verifier. The native owner must consciously update the pin and rebuild artifacts before integration verification; no deployment.
