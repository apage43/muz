#!/usr/bin/env python3
"""Install complete pinned SFZ assets, preserving native-subset installers.

No mappings or recordings are bundled in git. --source reuses an archive or
an existing assets directory. --check reads only. Existing differing files
are rejected; downloads and copies are verified before atomic placement.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tempfile
import urllib.request
import zipfile

CONTRIB = Path(__file__).resolve().parent
PACKS = ('sonatina', 'virtuosity-drums', 'karoryfer', 'unreal/standard-guitar', 'unreal/metal-gtx')


def fingerprint(path, git=False):
    h = hashlib.sha1() if git else hashlib.sha256()
    if git:
        h.update(f'blob {path.stat().st_size}\0'.encode())
    with path.open('rb') as f:
        while chunk := f.read(1 << 20):
            h.update(chunk)
    return h.hexdigest()


def safe(root, relative):
    p = PurePosixPath(relative.replace('\\', '/'))
    if p.is_absolute() or '..' in p.parts:
        raise ValueError(f'unsafe asset path: {relative}')
    destination = root.joinpath(*p.parts)
    if not destination.resolve().is_relative_to(root.resolve()):
        raise ValueError(f'asset path escapes destination: {relative}')
    return destination


def matches(path, row):
    return path.is_file() and path.stat().st_size == row['bytes'] and fingerprint(path, 'git_blob' in row) == row.get('git_blob', row.get('sha256'))


def place(row, assets, source, check):
    destination = safe(assets, row['path'])
    if matches(destination, row):
        return
    if destination.exists():
        raise ValueError(f'existing file differs from immutable pin: {destination}')
    if check:
        raise ValueError(f'missing asset: {destination}')
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix='.sfz-install-', dir=destination.parent)
    os.close(fd)
    temporary = Path(temporary)
    try:
        local = safe(source, row['path']) if source else None
        if local and local.is_file():
            shutil.copyfile(local, temporary)
        else:
            with urllib.request.urlopen(row['url'], timeout=120) as response, temporary.open('wb') as out:
                shutil.copyfileobj(response, out, 1 << 20)
        if not matches(temporary, row):
            raise ValueError(f'asset does not match immutable pin: {row["path"]}')
        os.replace(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('pack', choices=PACKS)
    p.add_argument('--profile', choices=('native-subset', 'sfz-complete'), default='sfz-complete')
    p.add_argument('--source', type=Path)
    p.add_argument('--assets', type=Path, help='override destination assets directory')
    p.add_argument('--check', action='store_true')
    p.add_argument('--jobs', type=int, default=6)
    a = p.parse_args()
    pack = CONTRIB / a.pack
    if a.profile == 'native-subset':
        if a.assets or a.source or a.check:
            p.error('native-subset uses pack install.py; invoke it directly for pack-specific options')
        subprocess.run([__import__('sys').executable, str(pack / 'install.py')], check=True)
        return
    manifest_path = pack / ('sfz-manifest.json' if a.pack != 'sonatina' else 'manifest.json')
    m = json.loads(manifest_path.read_text())
    assets = (a.assets or pack / 'assets').resolve()
    source = a.source.resolve() if a.source else None
    if 'archive' in m and a.check:
        if source and source.is_file() and not matches(source, m['archive']):
            raise ValueError(f'archive differs from immutable pin: {source}')
        for row in m['files']:
            place(row, assets, None, True)
        print(f'{a.pack}: {len(m["files"])} immutable assets verified ({a.profile})')
        return
    if 'archive' in m:
        pin = m['archive']
        archive = source if source and source.is_file() else safe(assets, pin['cache'])
        place(dict(path=pin['cache'], bytes=pin['bytes'], sha256=pin['sha256'], url=pin['url']), assets, None, a.check) if not source or not source.is_file() else None
        if not matches(archive, pin):
            raise ValueError(f'archive differs from immutable pin: {archive}')
        if not a.check:
            # Stage extraction, then hash every declared asset before placing it.
            with tempfile.TemporaryDirectory(prefix='muz-sfz-') as directory:
                stage = Path(directory)
                if archive.suffix == '.zip':
                    with zipfile.ZipFile(archive) as bundle:
                        for info in bundle.infolist():
                            safe(stage, info.filename)
                        bundle.extractall(stage)
                else:
                    names = subprocess.check_output(['unrar', 'lb', str(archive)], text=True).splitlines()
                    for name in names:
                        safe(stage, name)
                    subprocess.run(['unrar', 'x', '-idq', '-o-', '-p-', str(archive), str(stage)+'/'], check=True)
                prefix = m.get('extract_prefix', '')
                if prefix:
                    relocated = stage / prefix
                    relocated.mkdir(parents=True, exist_ok=True)
                    for child in list(stage.iterdir()):
                        if child != relocated:
                            shutil.move(str(child), relocated / child.name)
                source = stage
                for row in m['files']:
                    place(row, assets, source, False)
        else:
            for row in m['files']:
                place(row, assets, None, True)
    else:
        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, a.jobs)) as pool:
            list(pool.map(lambda row: place(row, assets, source, a.check), m['files']))
    print(f'{a.pack}: {len(m["files"])} immutable assets verified ({a.profile})')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as e:
        raise SystemExit(str(e))
