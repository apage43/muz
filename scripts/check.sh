#!/bin/sh
# Opt-in local checks (not CI). Run from anywhere: scripts/check.sh
# Runs every step and reports all failures; exit status is nonzero if any failed.
#
# Known state: `cargo fmt --check` currently reports about 35 files needing
# formatting. They are deliberately not reformatted in bulk, so that step is
# expected to fail until they are touched.
cd "$(dirname "$0")/.." || exit 1
status=0
run() {
    echo "==> $*"
    "$@" || { echo "FAILED: $*"; status=1; }
}
run cargo fmt --check
run cargo clippy
# Portable build (no CLI, audio device, or native hosting); see docs/embedding.md.
run cargo check --lib --no-default-features
if rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
    run cargo check --lib --no-default-features --target wasm32-unknown-unknown
else
    echo "==> skipping wasm32 check (target wasm32-unknown-unknown not installed)"
fi
if command -v ruff >/dev/null 2>&1; then
    run ruff check tools
else
    echo "==> skipping ruff (not installed)"
fi
for t in tools/test-*.py contrib/test-*.py; do
    run python3 "$t"
done
exit $status
