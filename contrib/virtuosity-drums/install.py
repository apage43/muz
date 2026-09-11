#!/usr/bin/env python3
"""Install the Virtuosity Drums v0.925 recordings into this pack's assets/.

Downloads the publisher's ZIP, checks it against the pinned size and sha256,
then extracts only Samples/, Programs/, LICENSE and notes.txt into ./assets so
that every path declared in kit.muz resolves. Nothing is committed to git.

Re-running is safe: a complete install is verified and left alone, missing
files are filled in, matching files are not rewritten, and a file that differs
from the pinned release stops the run instead of being overwritten.

    python3 contrib/virtuosity-drums/install.py
    python3 contrib/virtuosity-drums/install.py --source /path/to/Virtuosity_Drums_v0.925.zip
    python3 contrib/virtuosity-drums/install.py --check
"""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import sys
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ASSETS = HERE / "assets"
ARCHIVE = ASSETS / "virtuosity.zip"

URL = "https://versilian-studios.com/Distro/Virtuosity_Drums_v0.925.zip"
ARCHIVE_BYTES = 1227151376
ARCHIVE_SHA256 = "c6c5d0fe11a394e94be3146a950c3377ec102cb57d189d5a23cec26183d1963a"

# Extracted members and, for each, the number of files and total bytes the
# pinned release produces. Everything else in the ZIP (GUI/, the PDFs and the
# .bank.xml) is an SFZ-player front end that this native mapping does not use.
MEMBERS = ("Samples/", "Programs/", "LICENSE", "notes.txt")
EXPECTED = {
    "Samples/": (4858, 1468997687),
    "Programs/": (419, 663828),
    "LICENSE": (1, 7048),
    "notes.txt": (1, 5997),
}

CHUNK = 1 << 20
REPORT_EVERY = 128 << 20


class InstallError(Exception):
    """A condition the operator has to resolve before installing can succeed."""


def human(size: float) -> str:
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if size < 1024 or unit == "TiB":
            return f"{size:.0f} {unit}" if unit == "B" else f"{size:.1f} {unit}"
        size /= 1024
    raise AssertionError("unreachable")


def shown(path: Path) -> str:
    """Path relative to the pack, for messages that stay readable anywhere."""
    try:
        return str(path.relative_to(HERE))
    except ValueError:
        return str(path)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(CHUNK):
            digest.update(chunk)
    return digest.hexdigest()


def member_of(name: str) -> str | None:
    for member in MEMBERS:
        if name == member or (member.endswith("/") and name.startswith(member)):
            return member
    return None


def tree_stats(path: Path) -> tuple[int, int]:
    files = bytes_ = 0
    for child in path.rglob("*"):
        if child.is_file():
            files += 1
            bytes_ += child.stat().st_size
    return files, bytes_


def verify_archive(path: Path, label: str) -> None:
    size = path.stat().st_size
    if size != ARCHIVE_BYTES:
        raise InstallError(
            f"{label} is {size} bytes, but the pinned Virtuosity Drums v0.925 "
            f"archive is {ARCHIVE_BYTES} bytes ({human(ARCHIVE_BYTES)})."
        )
    digest = sha256_file(path)
    if digest != ARCHIVE_SHA256:
        raise InstallError(
            f"{label} has sha256 {digest}, but the pinned archive has "
            f"{ARCHIVE_SHA256}. Move the file aside and download the release again."
        )


def download(destination: Path) -> None:
    part = destination.with_name(destination.name + ".part")
    destination.parent.mkdir(parents=True, exist_ok=True)
    print(f"downloading {URL}")
    print(f"  -> {shown(destination)} ({human(ARCHIVE_BYTES)})")
    digest = hashlib.sha256()
    total = 0
    report = REPORT_EVERY
    request = urllib.request.Request(URL, headers={"User-Agent": "muz-virtuosity-install/1"})
    try:
        with urllib.request.urlopen(request, timeout=60) as response, part.open("wb") as out:
            while chunk := response.read(CHUNK):
                out.write(chunk)
                digest.update(chunk)
                total += len(chunk)
                if total >= report:
                    print(f"  {human(total)} of {human(ARCHIVE_BYTES)}", file=sys.stderr)
                    report += REPORT_EVERY
    except (urllib.error.URLError, OSError) as exc:
        part.unlink(missing_ok=True)
        raise InstallError(
            f"download failed: {exc}\n"
            "Download the ZIP with a browser or curl and pass it as "
            "--source PATH; the installer verifies it before extracting."
        ) from exc
    if total != ARCHIVE_BYTES or digest.hexdigest() != ARCHIVE_SHA256:
        part.unlink(missing_ok=True)
        raise InstallError(
            f"download does not match the pinned release: got {total} bytes with "
            f"sha256 {digest.hexdigest()} (partial file removed). Retry, or pass "
            "a good copy with --source PATH."
        )
    os.replace(part, destination)
    print(f"  verified sha256 {ARCHIVE_SHA256}")


def resolve_archive(source: Path | None) -> Path:
    if source is not None:
        if not source.is_file():
            raise InstallError(f"--source {source} is not a file")
        verify_archive(source, f"--source {source}")
        print(f"using verified archive {source} (not copied into {shown(ASSETS)})")
        return source
    if ARCHIVE.is_file():
        try:
            verify_archive(ARCHIVE, shown(ARCHIVE))
        except InstallError as exc:
            raise InstallError(f"{exc}\nThe existing archive was left untouched.") from exc
        print(f"using verified archive {shown(ARCHIVE)}")
        return ARCHIVE
    download(ARCHIVE)
    return ARCHIVE


def extract(archive: Path) -> tuple[int, int]:
    installed = kept = 0
    with zipfile.ZipFile(archive) as bundle:
        for info in bundle.infolist():
            if member_of(info.filename) is None or info.is_dir():
                continue
            destination = (ASSETS / info.filename).resolve()
            if not destination.is_relative_to(ASSETS.resolve()):
                raise InstallError(f"archive member escapes the install: {info.filename}")
            if destination.exists():
                size = destination.stat().st_size
                if size == info.file_size:
                    kept += 1
                    continue
                raise InstallError(
                    f"{shown(destination)} is {size} bytes, but the pinned release "
                    f"has {info.file_size} there. Move or delete the file and re-run; "
                    "differing files are never overwritten."
                )
            destination.parent.mkdir(parents=True, exist_ok=True)
            part = destination.with_name(destination.name + ".part")
            try:
                with bundle.open(info) as source, part.open("wb") as out:
                    shutil.copyfileobj(source, out, CHUNK)
                if part.stat().st_size != info.file_size:
                    raise InstallError(
                        f"extracted {shown(part)} has the wrong size; the archive may be damaged"
                    )
                os.replace(part, destination)
            except BaseException:
                part.unlink(missing_ok=True)
                raise
            installed += 1
    return installed, kept


def problems() -> list[str]:
    found: list[str] = []
    for member, (files, size) in EXPECTED.items():
        target = ASSETS / member.rstrip("/")
        if not target.exists():
            found.append(f"missing {shown(target)}")
        elif target.is_file():
            actual = target.stat().st_size
            if actual != size:
                found.append(f"{shown(target)}: {actual} bytes, expected {size}")
        else:
            actual_files, actual_bytes = tree_stats(target)
            if actual_files != files or actual_bytes != size:
                found.append(
                    f"{member}: {actual_files} files / {human(actual_bytes)}, "
                    f"expected {files} files / {human(size)}"
                )
    return found


def report() -> list[str]:
    found = problems()
    if found:
        print(f"{shown(ASSETS)} is incomplete:")
        for problem in found:
            print(f"  {problem}")
    else:
        files = sum(files for files, _ in EXPECTED.values())
        total = sum(size for _, size in EXPECTED.values())
        print(
            f"verified {shown(ASSETS)}: {files} files, {human(total)} "
            f"(Virtuosity Drums v0.925, sha256 {ARCHIVE_SHA256})"
        )
        print(f"sample paths resolve as {shown(ASSETS)}/Samples/{{mic}}/{{drum}}/...")
    return found


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--source",
        type=Path,
        help="already-downloaded Virtuosity_Drums_v0.925.zip to verify and extract in place",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the extracted content and exit without downloading or writing",
    )
    args = parser.parse_args(argv)

    if not report():
        return 0
    if args.check:
        return 1

    archive = resolve_archive(args.source)
    installed, kept = extract(archive)
    print(f"extracted {installed} files, {kept} files already matched the pinned release")
    if report():
        print("installation did not complete; re-run to retry", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except InstallError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(2)
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        sys.exit(130)
