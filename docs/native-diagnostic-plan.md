# Remaining native diagnostic boundary work

Observed native preparation still converts engine errors into `NativeError` strings after typed engine validation. Compilation alone does not validate unknown automation targets.

1. Retain an optional typed `Diagnostic` source in `NativeError`, keeping existing codes, messages, clone behavior, and display. Migrate native error-wrapping boundaries to recover diagnostics from source chains rather than rendered text; update affected literal construction and wrapper sources.
2. Add a focused headless `NativeRuntime::prepare_revision` regression with synthetic source that compiles but fails graph automation validation. Recover the diagnostic from the actual returned native error chain and assert message/location/help and stable display.
3. Run only the new native regression and portable library compile check. Update embedding documentation, remove this completed plan, commit owned core changes, and send the final SHA/contract for conscious native repin and artifact rebuild. No deployment.
