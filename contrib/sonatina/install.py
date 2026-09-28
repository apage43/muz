#!/usr/bin/env python3
"""Fetch, verify and place the Sonatina 4.0 recordings listed in manifest.json.

Every file is pinned by upstream path, byte size and SHA-256. Existing files
that match their pin are left untouched; an existing file that differs is
reported instead of being overwritten. Run with --check to verify only.

    python3 contrib/sonatina/install.py
    python3 contrib/sonatina/install.py --check
    python3 contrib/sonatina/install.py --force

Manifest paths are relative to the assets/ directory that the muz modules
reference, and the content is never committed: install.py writes into this
pack's gitignored assets/ directory only.
"""

import argparse
import concurrent.futures
import hashlib
import json
import os
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path, PurePosixPath
from urllib.parse import quote

PACK = Path(__file__).resolve().parent
MANIFEST = PACK / "manifest.json"
ASSETS = PACK / "assets"
CHUNK = 1 << 20
RETRIES = 4
USER_AGENT = "muz-contrib-sonatina-installer/1"


def fail(message):
    raise InstallError(message)


class InstallError(Exception):
    """A pin, download or placement problem worth reporting to the user."""


def load_manifest():
    try:
        document = json.loads(MANIFEST.read_text())
    except FileNotFoundError:
        fail(f"missing {MANIFEST}")
    except json.JSONDecodeError as error:
        fail(f"{MANIFEST} is not valid JSON: {error}")
    repository = document.get("repositories", {}).get("sso")
    if not repository:
        fail(f"{MANIFEST} has no sso repository pin")
    revision = repository["revision"]
    base = f"https://raw.githubusercontent.com/{repository['name']}/{revision}/"
    files = document.get("files")
    if not files:
        fail(f"{MANIFEST} lists no files")
    for row in files:
        destination = row["path"]
        if (
            not destination
            or Path(destination).is_absolute()
            or ".." in Path(destination).parts
        ):
            fail(f"manifest path must stay inside assets/: {destination!r}")
        expected = base + quote(row["repo_path"])
        if row["url"] != expected:
            fail(
                f"manifest url for {destination} does not match the pinned "
                f"revision: {row['url']} != {expected}"
            )
        if row["bytes"] <= 0 or len(row["sha256"]) != 64:
            fail(f"manifest pin for {destination} is incomplete")
    return repository, files


def digest(path):
    """Stream a file to its (bytes, sha256)."""
    hasher = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        while block := stream.read(CHUNK):
            size += len(block)
            hasher.update(block)
    return size, hasher.hexdigest()


def matches(path, row):
    if not path.is_file():
        return False
    size, sha = digest(path)
    return size == row["bytes"] and sha == row["sha256"]


def fetch(row, destination):
    """Download one pinned file into a temporary sibling, then place it."""
    temporary = None
    for attempt in range(1, RETRIES + 1):
        try:
            request = urllib.request.Request(
                row["url"], headers={"User-Agent": USER_AGENT}
            )
            hasher = hashlib.sha256()
            size = 0
            with urllib.request.urlopen(request, timeout=60) as response:
                with tempfile.NamedTemporaryFile(
                    dir=destination.parent, prefix=destination.name, suffix=".part",
                    delete=False,
                ) as part:
                    temporary = Path(part.name)
                    while block := response.read(CHUNK):
                        part.write(block)
                        size += len(block)
                        hasher.update(block)
            break
        except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as error:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
                temporary = None
            if attempt == RETRIES:
                fail(f"{row['url']}: {error}")
            time.sleep(2 ** (attempt - 1))
    assert temporary is not None
    if size != row["bytes"]:
        temporary.unlink(missing_ok=True)
        fail(
            f"{destination}: downloaded {size} bytes, manifest pins "
            f"{row['bytes']}"
        )
    if hasher.hexdigest() != row["sha256"]:
        temporary.unlink(missing_ok=True)
        fail(
            f"{destination}: downloaded sha256 {hasher.hexdigest()} does not "
            f"match pinned {row['sha256']}"
        )
    os.replace(temporary, destination)


def install(row, force):
    destination = ASSETS / row["path"]
    if destination.is_file():
        if matches(destination, row):
            return "verified", destination, 0
        if not force:
            fail(
                f"{destination} exists but does not match its pin; move it aside "
                f"or re-run with --force to replace it"
            )
    destination.parent.mkdir(parents=True, exist_ok=True)
    fetch(row, destination)
    if not matches(destination, row):
        fail(f"{destination}: placed file failed its pin verification")
    return "installed", destination, row["bytes"]


def check(row):
    destination = ASSETS / row["path"]
    if matches(destination, row):
        return "verified", destination, row["bytes"]
    state = "differs" if destination.is_file() else "missing"
    return state, destination, 0


def report(state, destination, size):
    relative = destination.relative_to(PACK)
    suffix = f" ({size} bytes)" if size else ""
    print(f"{state:9} {relative}{suffix}", flush=True)


def unpack_archive(response, repository, needed):
    """Stream a GitHub tarball; publish only individually verified regular files."""
    prefix = repository['name'].split('/')[-1] + '-' + repository['revision']
    pending = {row['repo_path']: row for row in needed}
    with tarfile.open(fileobj=response, mode='r|gz') as archive:
        for member in archive:
            parts = PurePosixPath(member.name).parts
            if not parts or parts[0] != prefix or '..' in parts:
                fail(f'unexpected archive path: {member.name!r}')
            repo_path = '/'.join(parts[1:])
            if repo_path not in pending:
                continue
            row = pending[repo_path]
            if not member.isfile() or member.size != row['bytes']:
                fail(f'archive member differs from pin: {repo_path}')
            destination = ASSETS / row['path']
            destination.parent.mkdir(parents=True, exist_ok=True)
            temporary = None
            try:
                hasher = hashlib.sha256()
                size = 0
                source = archive.extractfile(member)
                with tempfile.NamedTemporaryFile(dir=destination.parent, suffix='.part', delete=False) as part:
                    temporary = Path(part.name)
                    while block := source.read(CHUNK):
                        size += len(block)
                        hasher.update(block)
                        part.write(block)
                if size != row['bytes'] or hasher.hexdigest() != row['sha256']:
                    fail(f'archive file failed SHA-256/size verification: {repo_path}')
                os.replace(temporary, destination)
                temporary = None
                del pending[repo_path]
                report('installed', destination, size)
            finally:
                if temporary is not None:
                    temporary.unlink(missing_ok=True)
    if pending:
        fail(f'archive is missing {len(pending)} pinned files')


def archive_install(repository, files, arguments):
    """Return None for small repairs, otherwise stream the full pinned archive."""
    needed = []
    for row in files:
        destination = ASSETS / row['path']
        if matches(destination, row):
            continue
        if destination.exists() and not arguments.force:
            fail(f'{destination} differs from its pin; use --force to replace it')
        needed.append(row)
    if arguments.transport == 'auto' and len(needed) <= 32:
        return None
    if not needed:
        print(f'0 installed, {len(files)} verified, 0 missing, 0 differing')
        return 0
    url = f"https://codeload.github.com/{repository['name']}/tar.gz/{repository['revision']}"
    print(f'Streaming pinned archive for {len(needed)} files (no archive retained)', flush=True)
    request = urllib.request.Request(url, headers={'User-Agent': USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            unpack_archive(response, repository, needed)
    except (urllib.error.URLError, OSError, tarfile.TarError) as error:
        fail(f'archive download failed: {error}; rerun to retain verified progress, or use --transport files')
    print(f'{len(needed)} installed, {len(files) - len(needed)} verified, 0 missing, 0 differing')
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--check", action="store_true", help="verify only; download nothing"
    )
    parser.add_argument(
        "--force", action="store_true", help="replace files that fail their pin"
    )
    parser.add_argument(
        "--jobs", type=int, default=4, help="parallel downloads (default 4)"
    )
    parser.add_argument(
        '--transport', choices=('auto', 'archive', 'files'), default='auto',
        help='auto streams an archive for >32 missing files, otherwise downloads individual files',
    )
    arguments = parser.parse_args()
    if arguments.jobs < 1:
        fail("--jobs must be at least 1")

    repository, files = load_manifest()
    pinned = sum(row["bytes"] for row in files)
    print(
        f"{repository['name']} @ {repository['revision']}: {len(files)} files, "
        f"{pinned} bytes"
    )
    if not arguments.check and arguments.transport != 'files':
        result = archive_install(repository, files, arguments)
        if result is not None:
            return result
    action = check if arguments.check else lambda row: install(row, arguments.force)
    counts = {"verified": 0, "installed": 0, "missing": 0, "differs": 0}
    failures = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=arguments.jobs) as pool:
        futures = {pool.submit(action, row): row for row in files}
        for future in concurrent.futures.as_completed(futures):
            row = futures[future]
            try:
                state, destination, size = future.result()
            except InstallError as error:
                failures.append(str(error))
                print(f"FAILED    {row['path']}", flush=True)
                continue
            counts[state] += 1
            report(state, destination, size)
    print(
        f"{counts['installed']} installed, {counts['verified']} verified, "
        f"{counts['missing']} missing, {counts['differs']} differing"
    )
    if failures:
        for message in failures:
            print(message, file=sys.stderr)
        return 1
    if arguments.check and (counts["missing"] or counts["differs"]):
        print("run install.py without --check to fetch the missing files")
        return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except InstallError as error:
        print(f"install.py: {error}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        sys.exit(130)
