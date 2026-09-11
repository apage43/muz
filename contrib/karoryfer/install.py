#!/usr/bin/env python3
"""Fetch and verify the pinned Karoryfer recordings used by contrib/karoryfer/.

Samples are never committed. This places the subset named by manifest.json into
./assets (git-ignored) and checks every pinned byte count and SHA-256, so a
re-run either verifies what is already there or installs what is missing.

  python3 contrib/karoryfer/install.py
  python3 contrib/karoryfer/install.py --from /path/to/existing/assets
  python3 contrib/karoryfer/install.py --force
"""

import argparse
import concurrent.futures
import hashlib
import json
import os
import sys
import time
import urllib.request
from pathlib import Path, PurePosixPath

PACK = Path(__file__).resolve().parent
ASSETS = PACK / "assets"
MANIFEST = PACK / "manifest.json"
ATTEMPTS = 3
TIMEOUT_SECONDS = 120


def pinned_bytes(item):
    return item["bytes"]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def existing(path):
    """Return the byte count and SHA-256 of path, or None when it is absent."""
    if not path.is_file():
        return None
    data = path.read_bytes()
    return len(data), digest(data)


def matches(item, size, sha256):
    return size == pinned_bytes(item) and sha256 == item["sha256"]


def describe(item, size, sha256):
    return (
        f"{item['path']}: expected {pinned_bytes(item)} bytes / {item['sha256']}, "
        f"found {size} bytes / {sha256}"
    )


def download(url):
    last = None
    for attempt in range(1, ATTEMPTS + 1):
        try:
            with urllib.request.urlopen(url, timeout=TIMEOUT_SECONDS) as response:
                return response.read()
        except Exception as error:  # noqa: BLE001 - reported below with context
            last = error
            if attempt < ATTEMPTS:
                time.sleep(attempt)
    raise RuntimeError(f"could not download {url}: {last}")


def read_local(item, directory):
    """Return the pinned bytes of a local copy, or None when it is not there."""
    if directory is None:
        return None
    path = directory / item["path"]
    if not path.is_file():
        return None
    data = path.read_bytes()
    if not matches(item, len(data), digest(data)):
        raise RuntimeError(
            f"local copy does not match the pin; {describe(item, len(data), digest(data))}"
        )
    return data


def install(item, force, source):
    """Return (status, item); status is 'verified', 'installed' or 'replaced'."""
    destination = ASSETS / item["path"]
    found = existing(destination)
    if found is not None and matches(item, *found):
        return "verified", item
    if found is not None and not force:
        raise RuntimeError(
            "refusing to overwrite a differing file; "
            + describe(item, *found)
            + " (re-run with --force to replace it)"
        )
    data = read_local(item, source)
    if data is None:
        data = download(item["url"])
    if not matches(item, len(data), digest(data)):
        raise RuntimeError(
            "downloaded data does not match the pin; "
            + describe(item, len(data), digest(data))
        )
    destination.parent.mkdir(parents=True, exist_ok=True)
    staging = destination.with_name(destination.name + ".download")
    try:
        staging.write_bytes(data)
        os.replace(staging, destination)
    finally:
        if staging.exists():
            staging.unlink()
    return ("replaced" if found is not None else "installed"), item


def load_manifest():
    manifest = json.loads(MANIFEST.read_text())
    files = manifest["files"]
    if not isinstance(files, list) or not files:
        raise SystemExit(f"{MANIFEST} lists no files")
    for item in files:
        for key in ("path", "url", "bytes", "sha256"):
            if key not in item:
                raise SystemExit(f"{MANIFEST}: entry {item} lacks '{key}'")
        relative = PurePosixPath(item["path"])
        if relative.is_absolute() or ".." in relative.parts:
            raise SystemExit(f"{MANIFEST}: unsafe path '{item['path']}'")
        destination = ASSETS.joinpath(*relative.parts).resolve()
        if not destination.is_relative_to(ASSETS.resolve()):
            raise SystemExit(f"{MANIFEST}: path escapes assets/: {item['path']}")
        if len(item["sha256"]) != 64 or not all(
            c in "0123456789abcdef" for c in item["sha256"]
        ):
            raise SystemExit(f"{MANIFEST}: bad sha256 for {item['path']}")
    return manifest, files


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--from",
        dest="source",
        type=Path,
        default=None,
        metavar="DIR",
        help=(
            "prefer pinned files from an existing assets directory "
            "(one that contains guitar/ and bass/) instead of downloading them"
        ),
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="replace files that exist but differ from the pin",
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=6,
        metavar="N",
        help="parallel transfers (default 6)",
    )
    arguments = parser.parse_args()

    manifest, files = load_manifest()
    source = arguments.source.resolve() if arguments.source else None
    if source is not None and not source.is_dir():
        raise SystemExit(f"{source} is not a directory")
    total = sum(pinned_bytes(item) for item in files)
    print(
        f"Karoryfer pack: {len(files)} pinned files, {total / 1e6:.1f} MB, "
        f"into {ASSETS}"
    )
    for repository, pin in manifest["repositories"].items():
        print(f"  {repository}: {pin['name']} @ {pin['revision']} ({pin['license']})")

    statuses = {}
    failed = []
    with concurrent.futures.ThreadPoolExecutor(
        max_workers=max(1, arguments.jobs)
    ) as pool:
        futures = [pool.submit(install, item, arguments.force, source) for item in files]
        for future, item in zip(futures, files):
            try:
                status, done = future.result()
            except Exception as error:  # noqa: BLE001 - reported per file
                failed.append((item["path"], str(error)))
                print(f"FAILED {item['path']}: {error}", file=sys.stderr)
                continue
            statuses[status] = statuses.get(status, 0) + 1
            if status != "verified":
                print(f"{status} {done['path']}")

    verified = statuses.get("verified", 0)
    installed = statuses.get("installed", 0)
    replaced = statuses.get("replaced", 0)
    print(
        f"{verified} verified, {installed} installed, {replaced} replaced, "
        f"{len(failed)} failed ({total / 1e6:.1f} MB pinned)"
    )
    if failed:
        print(f"{len(failed)} file(s) failed; nothing was written for them", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
