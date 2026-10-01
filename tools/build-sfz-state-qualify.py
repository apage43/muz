#!/usr/bin/env python3
"""Build the metadata-only public API probe from exact Cargo JSON artifacts."""
import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parent.parent
build = subprocess.run(
    ["cargo", "build", "--locked", "--release", "--lib", "--no-default-features", "--message-format=json"],
    cwd=root, text=True, capture_output=True,
)
sys.stderr.write(build.stderr)
if build.returncode:
    raise SystemExit(build.returncode)
artifacts = {}
for line in build.stdout.splitlines():
    message = json.loads(line)
    if message.get("reason") != "compiler-artifact":
        continue
    name = message["target"]["name"]
    if name in ("muz", "serde_json"):
        libraries = [path for path in message["filenames"] if path.endswith(".rlib")]
        if len(libraries) != 1:
            raise SystemExit(f"expected exactly one rlib for {name}: {libraries}")
        artifacts[name] = libraries[0]
if artifacts.keys() != {"muz", "serde_json"}:
    raise SystemExit(f"missing exact Cargo artifacts: {artifacts}")
binary = root / "target/release/sfz-state-qualify"
subprocess.run(
    ["rustc", "--edition=2024", str(root / "tools/sfz-state-qualify.rs"),
     "-L", f"dependency={root / 'target/release/deps'}",
     "--extern", f"muz={artifacts['muz']}",
     "--extern", f"serde_json={artifacts['serde_json']}", "-o", str(binary)],
    cwd=root, check=True,
)
print(binary)
