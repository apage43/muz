#!/usr/bin/env python3
"""Install the Unreal Instruments METAL-GTX bank for contrib/unreal/metal-gtx.

Downloads the publisher's 1.34 GB archive, verifies its size and sha256, checks that
every archive member stays inside assets/metal-gtx/, unpacks it there, verifies the
recorded takes that metal-gtx.muz plays, and then derives the mono take bank.

    python3 contrib/unreal/metal-gtx/install.py

Re-running is safe: the archive is verified again, nothing is overwritten, and only
missing takes are derived.  Needs unrar and ffmpeg on PATH.  The library is free to use
with no credit required, but it must not be redistributed, so the recordings stay
outside git and only this mapping and these scripts are committed.
"""
from pathlib import Path, PurePosixPath
import hashlib
import json
import shutil
import subprocess
import sys
import urllib.request

PACK = Path(__file__).resolve().parent
ASSETS = PACK / 'assets'
ARCHIVE = ASSETS / 'metal-gtx-download.rar'
PARTIAL = ARCHIVE.with_suffix('.partial')
BANK = ASSETS / 'metal-gtx'
MANIFEST = PACK / 'manifest.json'
MIB = 1024 * 1024


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * MIB), b''):
            digest.update(chunk)
    return digest.hexdigest()


def gibibytes(count):
    return f'{count / (1024 * MIB):.2f} GiB'


def require(tool):
    if shutil.which(tool) is None:
        raise SystemExit(f'{tool} is required by install.py but was not found on PATH.')


def download(pin):
    """Fetch the archive into a partial file, then verify it before keeping it."""
    if PARTIAL.exists():
        raise SystemExit(f'{PARTIAL} is an unfinished download of {PARTIAL.stat().st_size} bytes.\n'
                         'It was not overwritten; delete it and run install.py again.')
    ASSETS.mkdir(exist_ok=True)
    print(f'Downloading {gibibytes(pin["bytes"])} from {pin["url"]}', flush=True)
    with urllib.request.urlopen(pin['url']) as response, PARTIAL.open('wb') as target:
        written, step = 0, 0
        while True:
            chunk = response.read(MIB)
            if not chunk:
                break
            target.write(chunk)
            written += len(chunk)
            if written // (64 * MIB) > step:
                step = written // (64 * MIB)
                print(f'  {gibibytes(written)} of {gibibytes(pin["bytes"])}', flush=True)
    if PARTIAL.stat().st_size != pin['bytes'] or sha256(PARTIAL) != pin['sha256']:
        raise SystemExit(f'Downloaded {PARTIAL.stat().st_size} bytes, but the pinned archive is '
                         f'{pin["bytes"]} bytes / sha256 {pin["sha256"]}.\n'
                         'The download was kept as a .partial file for inspection; nothing else changed.')
    PARTIAL.replace(ARCHIVE)
    print(f'Downloaded and verified {gibibytes(ARCHIVE.stat().st_size)} as sha256 {pin["sha256"]}')


def verify_archive(pin):
    size, digest = ARCHIVE.stat().st_size, sha256(ARCHIVE)
    if size != pin['bytes'] or digest != pin['sha256']:
        raise SystemExit(f'{ARCHIVE} is {size} bytes / sha256 {digest}, not the pinned '
                         f'{pin["bytes"]} bytes / {pin["sha256"]}.\n'
                         'Nothing was changed; move the file aside and run install.py again.')
    print(f'Archive: {gibibytes(size)} with sha256 {digest}')


def unsafe(name):
    """Reject members that would land outside assets/metal-gtx/."""
    path = PurePosixPath(name.replace('\\', '/'))
    return not path.parts or path.is_absolute() or '..' in path.parts


def unpack():
    """Validate every member path, then extract without overwriting existing files."""
    names = subprocess.check_output(['unrar', 'lb', str(ARCHIVE)], text=True).splitlines()
    for name in names:
        if unsafe(name):
            raise SystemExit(f'Unsafe archive member: {name}')
    BANK.mkdir(exist_ok=True)
    print(f'Unpacking {len(names)} members into {BANK.relative_to(PACK)}/ (existing files are kept)', flush=True)
    subprocess.run(['unrar', 'x', '-idq', '-o-', str(ARCHIVE), str(BANK) + '/'], check=True)


def verify_sources(manifest):
    """Every recorded take the module plays must match its pinned bytes and sha256."""
    print(f'Verifying {len(manifest["sources"])} bank recordings', flush=True)
    shown, wrong = [], 0
    for entry in manifest['sources']:
        path = PACK / entry['path']
        reason = None
        if not path.is_file():
            reason = 'is missing'
        elif path.stat().st_size != entry['bytes']:
            reason = f'is {path.stat().st_size} bytes, not {entry["bytes"]}'
        elif sha256(path) != entry['sha256']:
            reason = f'does not match sha256 {entry["sha256"]}'
        if reason:
            wrong += 1
            if len(shown) < 5:
                shown.append(f'{entry["path"]} {reason}')
    if wrong:
        extra = f'\nand {wrong - len(shown)} more' if wrong > len(shown) else ''
        detail = '\n'.join(shown)
        raise SystemExit(f'{detail}{extra}\n'
                         f'{wrong} recorded takes differ from the pins; move aside '
                         f'{BANK.relative_to(PACK)} and run install.py again.')
    total = sum(entry['bytes'] for entry in manifest['sources'])
    print(f'Verified: {len(manifest["sources"])} recorded takes ({gibibytes(total)}) match their pins')


def main():
    manifest = json.loads(MANIFEST.read_text())
    pin = manifest['archive']
    for tool in ('unrar', 'ffmpeg'):
        require(tool)
    if ARCHIVE.exists():
        verify_archive(pin)
    else:
        download(pin)
    unpack()
    verify_sources(manifest)
    print('Deriving the mono take bank', flush=True)
    result = subprocess.run([sys.executable, str(PACK / 'derive.py')])
    if result.returncode != 0:
        raise SystemExit('Derivation failed; the unpacked bank is in place, so derive.py can be re-run once the cause is fixed.')
    print('\nDone. This pack uses:')
    print(f'  {ARCHIVE.relative_to(PACK)}  the publisher archive, verified and not committed')
    print(f'  {BANK.relative_to(PACK)}/      the unpacked bank, not committed')
    print(f'  assets/gtx-mono/      {len(manifest["files"])} derived mono takes, not committed')
    print('Keep the archive to derive again without downloading, or delete it and re-run install.py.')
    print('METAL-GTX is free to use with no credit required; do not redistribute the recordings.')


if __name__ == '__main__':
    main()
