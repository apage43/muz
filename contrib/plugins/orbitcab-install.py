#!/usr/bin/env python3
"""Install the pinned OrbitCab CLAP build into the user plugin directory.

Verifies the published archive and the extracted binary against
orbitcab-manifest.json, refuses to replace a different build, and never runs
two installs at once. Idempotent: an already-installed matching build is
verified and left alone.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import sys
import tarfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def verify(dest: Path, spec: dict) -> bool:
    if not dest.is_file():
        return False
    if digest(dest.read_bytes()) != spec["binary_sha256"]:
        raise SystemExit(
            f"{dest} contains a different build; keep it and install this "
            "version elsewhere, then point the device at that path"
        )
    return True


def install(dest: Path, spec: dict) -> None:
    request = urllib.request.Request(
        spec["archive_url"], headers={"User-Agent": "muz-composer"}
    )
    archive = urllib.request.urlopen(request, timeout=90).read()
    if digest(archive) != spec["archive_sha256"]:
        raise SystemExit("plugin archive checksum mismatch")
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as bundle:
        member = bundle.getmember(spec["member"])
        if not member.isfile():
            raise SystemExit("plugin archive member is not a regular file")
        binary = bundle.extractfile(member).read()
    if digest(binary) != spec["binary_sha256"]:
        raise SystemExit("plugin binary checksum mismatch")
    dest.parent.mkdir(parents=True, exist_ok=True)
    # Exclusive creation protects a plugin installed concurrently.
    with dest.open("xb") as out:
        out.write(binary)
    dest.chmod(0o755)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dest",
        type=Path,
        help="plugin directory (default: ~/.clap)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify only; download nothing",
    )
    args = parser.parse_args()
    spec = json.loads((HERE / "orbitcab-manifest.json").read_text())
    dest = (args.dest or Path.home() / ".clap").expanduser() / spec["member"]
    if verify(dest, spec):
        print(f"verified  {spec['name']} {spec['version']}: {dest}")
        return 0
    if args.check:
        print(f"missing   {spec['name']} {spec['version']}: {dest}", file=sys.stderr)
        return 1
    install(dest, spec)
    print(f"installed {spec['name']} {spec['version']}: {dest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
