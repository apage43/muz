#!/usr/bin/env python3
"""Install the Unreal Instruments Standard Guitar recordings for this pack.

Downloads the publisher archive, verifies it against the pinned SHA-256 in
manifest.json, validates the archive member paths, extracts it into assets/ and
verifies all 400 mapped recordings. Re-running only re-verifies. A file that
differs from its pinned checksum aborts the run: the supplied recordings are
never overwritten.

The publisher permits commercial use and forbids redistribution, so this script
fetches the archive from the publisher instead of shipping it.

System requirement: the `unrar` command on PATH (Arch: unrar, Debian/Ubuntu:
unrar-free, macOS: brew install unrar).
"""

from pathlib import Path, PurePosixPath
import hashlib
import json
import shutil
import subprocess
import urllib.error
import urllib.request

PACK = Path(__file__).resolve().parent
CHUNK = 1024 * 1024
PROGRESS_EVERY = 64 * CHUNK
USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64)"


def fail(message):
    raise SystemExit(message)


def mib(count):
    return f"{count / (1024 * 1024):.0f} MiB"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def pack_path(relative):
    path = PurePosixPath(relative)
    if path.is_absolute() or ".." in path.parts:
        fail(f"refusing a path outside the pack: {relative}")
    return PACK.joinpath(*path.parts)


def load_manifest():
    path = PACK / "manifest.json"
    if not path.exists():
        fail(f"missing pack manifest: {path}")
    return json.loads(path.read_text())


def verify(manifest):
    """Return the missing recordings and the mapped byte total.

    An existing recording whose size or checksum differs from the pin aborts
    the run so that supplied data is never silently replaced.
    """
    missing = []
    total = 0
    for record in manifest["files"]:
        path = pack_path(record["path"])
        total += record["bytes"]
        if not path.exists():
            missing.append(path)
            continue
        size = path.stat().st_size
        if size != record["bytes"]:
            fail(
                f"refusing to overwrite a supplied recording: {path}\n"
                f"expected {record['bytes']} bytes, found {size}. "
                "Move it aside and re-run."
            )
        if digest(path) != record["sha256"]:
            fail(
                f"refusing to overwrite a supplied recording: {path}\n"
                "its SHA-256 differs from manifest.json. Move it aside and re-run."
            )
    return missing, total


def cached_archive(manifest):
    """Return the verified publisher archive, downloading it when absent."""
    spec = manifest["download"]
    archive = pack_path(spec["cache"])
    if archive.exists():
        if archive.stat().st_size != spec["bytes"] or digest(archive) != spec["sha256"]:
            fail(
                f"the cached archive does not match the pin: {archive}\n"
                "Move it aside and re-run to download the pinned archive."
            )
        print(f"Using the verified archive {archive}.")
        return archive

    archive.parent.mkdir(parents=True, exist_ok=True)
    temporary = archive.with_name(archive.name + ".part")
    if temporary.exists():
        print(f"Removing the incomplete download {temporary}.")
        temporary.unlink()
    request = urllib.request.Request(spec["url"], headers={"User-Agent": USER_AGENT})
    print(f"Downloading {mib(spec['bytes'])} from the publisher...")
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            if "text/html" in response.headers.get("Content-Type", ""):
                fail(
                    "the public download returned a web page instead of the archive; "
                    f"download it manually from {manifest['homepage']} and place it at {archive}"
                )
            received = 0
            marked = 0
            with temporary.open("wb") as output:
                while chunk := response.read(CHUNK):
                    output.write(chunk)
                    received += len(chunk)
                    if received - marked >= PROGRESS_EVERY:
                        marked = received
                        print(f"  {mib(received)}")
    except OSError as error:
        fail(f"download failed: {error}")
    print(f"Downloaded {mib(received)}.")
    if digest(temporary) != spec["sha256"]:
        fail(
            f"the downloaded archive does not match the pinned SHA-256 ({spec['sha256']}); "
            "no recordings were extracted"
        )
    temporary.replace(archive)
    print(f"Verified the archive SHA-256 and kept it at {archive}.")
    return archive


def extract(archive, destination):
    if shutil.which("unrar") is None:
        fail(
            "the `unrar` command is required to extract the publisher archive; "
            "install it (Arch: unrar, Debian/Ubuntu: unrar-free, macOS: brew install unrar)"
        )
    try:
        listing = subprocess.check_output(["unrar", "lb", str(archive)], text=True)
    except (subprocess.CalledProcessError, OSError) as error:
        fail(f"could not read the archive with unrar: {error}")
    for name in listing.splitlines():
        entry = PurePosixPath(name.replace("\\", "/"))
        if entry.is_absolute() or ".." in entry.parts:
            fail(f"unsafe archive path: {name}")
    destination.mkdir(parents=True, exist_ok=True)
    try:
        subprocess.run(
            ["unrar", "x", "-idq", "-o-", "-p-", str(archive), str(destination) + "/"],
            check=True,
        )
    except (subprocess.CalledProcessError, OSError) as error:
        fail(f"unrar failed to extract {archive}: {error}")


def main():
    manifest = load_manifest()
    count = len(manifest["files"])
    missing, total = verify(manifest)
    if not missing:
        print(f"Verified {count} recordings ({mib(total)}) for {manifest['library']}.")
        return
    print(f"{len(missing)} of {count} mapped recordings are missing.")
    archive = cached_archive(manifest)
    destination = pack_path(manifest["download"]["extract_to"])
    extract(archive, destination)
    missing, total = verify(manifest)
    if missing:
        fail(
            "the archive did not provide all mapped recordings; still missing:\n  "
            + "\n  ".join(str(path) for path in missing[:10])
        )
    print(f"Installed and verified {count} recordings ({mib(total)}) under {destination}.")
    print("The supplied recordings are unmodified; they stay out of git.")


if __name__ == "__main__":
    main()
