#!/usr/bin/env python3
"""Derive the METAL-GTX mono take bank that metal-gtx.muz plays.

The unpacked bank must already be in place: install.py downloads the pinned 1.34 GB
archive, verifies it and unpacks it into assets/metal-gtx/.  Every recorded take is a
stereo double track, so this extracts channel 0 and channel 1 as separate mono
recordings -- lossless channel separation with no pitch, timing, filtering or level
change -- regenerates the pins in manifest.json, and checks metal-gtx.muz against the
derived takes.

    python3 derive.py               derive what is missing, verify pins and module
    python3 derive.py --write-muz   also replace metal-gtx.muz with the generated table

Nothing is overwritten silently: an existing take that does not match its pinned
sha256, or a module that differs from the generated table, is reported instead.
"""
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import re
import shutil
import subprocess
import sys

PACK = Path(__file__).resolve().parent
BANK = PACK / 'assets/metal-gtx/UI_METAL-GTX'
OUT = PACK / 'assets/gtx-mono'
ARCHIVE = PACK / 'assets/metal-gtx-download.rar'
MANIFEST = PACK / 'manifest.json'
MODULE = PACK / 'metal-gtx.muz'
WORKERS = 8

# Articulation tag, recorded folder in the bank, and the round robins we keep.
SPECS = [
    ('sustain_down', 'Sus_Down', 3),
    ('sustain_up', 'Sus_Up', 3),
    ('palm_down', 'Mute_Down', 3),
    ('palm_up', 'Mute_Up', 3),
    ('release', 'Release1', 4),
]
# The recorded range the pack plays, and the notes it covers.
LOW, HIGH = 30, 57


def key(name):
    """MIDI pitch of a bank filename such as a#1_Sus_Down1.flac."""
    match = re.match(r'([a-g])([#b]?)(\d)_', name)
    return 12 * (int(match[3]) + 1) + {'c': 0, 'd': 2, 'e': 4, 'f': 5, 'g': 7, 'a': 9, 'b': 11}[match[1]] + {'': 0, '#': 1, 'b': -1}[match[2]]


def settings(tag):
    """Sampler settings per articulation, in the module's written order."""
    text = 'attack_ms: 0.4, release_ms: 38, velocity_track: 0.45, gain_db: -10'
    if tag.startswith('palm'):
        text = 'attack_ms: 0.4, release_ms: 42, velocity_track: 0.65, gain_db: -7.5'
    if tag == 'release':
        text = 'attack_ms: 0.3, release_ms: 55, velocity_track: 1, gain_db: -28, one_shot: true'
    return [part.strip() for part in text.split(',')]


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def relative(path):
    return Path(path).relative_to(PACK).as_posix()


def plan():
    """One job per recorded take side: (tag, source, root, take, channel)."""
    jobs = []
    for tag, folder, round_robins in SPECS:
        takes = sorted((BANK / 'Samples' / folder).glob('*.flac'))
        if not takes:
            raise SystemExit(f'No recordings under {relative(BANK / "Samples" / folder)}; run install.py first.')
        for src in takes:
            if src.name.startswith('Voice'):
                continue
            root = key(src.name)
            take = int(re.search(r'(\d+)\.flac$', src.name)[1])
            if LOW <= root <= HIGH and take <= round_robins:
                for side in (0, 1):
                    jobs.append((tag, src, root, take, side))
    return jobs


def destination(job):
    tag, _src, root, take, side = job
    return OUT / f'{tag}-{root}-{take}-{side}.flac'


def derive(job, pinned):
    """Extract one recorded side, or verify the take already on disk."""
    tag, src, root, take, side = job
    dst = destination(job)
    if dst.exists():
        digest = sha256(dst)
        documented = pinned.get(relative(dst))
        if documented and documented['sha256'] != digest:
            raise SystemExit(f'{relative(dst)} differs from its pinned sha256 {documented["sha256"]}.\n'
                             'Nothing was overwritten; delete that file to derive it again, or remove\n'
                             'assets/gtx-mono to derive the whole bank again.')
    else:
        subprocess.run(['ffmpeg', '-y', '-v', 'error', '-i', str(src), '-af', f'pan=mono|c0=c{side}',
                        '-c:a', 'flac', '-sample_fmt', 's32', str(dst)], check=True)
    return {
        'articulation': tag,
        'source': relative(src),
        'path': relative(dst),
        'bytes': dst.stat().st_size,
        'sha256': sha256(dst),
        'root': root,
        'take': take,
        'source_channel': side,
    }


def banner(archive):
    return (
        '// Unreal Instruments METAL-GTX: real recorded metal guitar articulations.\n'
        '// Owned by contrib/unreal/metal-gtx/ together with install.py and manifest.json; the\n'
        '// recordings live in the gitignored assets/ directory and are never committed.\n'
        '// Zone order is the round-robin order within each articulation, so do not reorder the\n'
        '// entries or rename takes: samplers pick takes by index and key range.\n'
        '// Generated by derive.py from the bank pinned in manifest.json (archive sha256\n'
        f'// {archive["sha256"]}, {archive["bytes"]}\n'
        '// bytes): the double-tracked stereo recordings split into their recorded left and\n'
        '// right sides, with no other processing of pitch, timing, filtering or level.\n'
    )


BUILDERS = """
// Builders. sample() resolves zone paths against the module that calls it, so a caller
// building these zones in another file would look for them beside that file. Build here
// instead, which keeps the pack's gitignored assets/ as the root:
//     let rhythm = gtx.instrument(gtx.palm_down_1_zones, {gain_db: -4});
// The preconfigured samples above already use the recorded articulation settings.
fn instrument(zones, options = {}) = sample(zones, options);
"""


def module_text(files, archive):
    """The generated articulation table, in the formatter's own layout."""
    lines = [banner(archive).rstrip('\n')]
    for tag, _folder, _round_robins in SPECS:
        for side in (0, 1):
            group = [f for f in files if f['articulation'] == tag and f['source_channel'] == side]
            roots = sorted({f['root'] for f in group})
            lines.append(f'let {tag}_{side}_zones = [')
            for entry in group:
                i = roots.index(entry['root'])
                lo = LOW if i == 0 else (roots[i - 1] + roots[i]) // 2 + 1
                hi = HIGH if i == len(roots) - 1 else (roots[i] + roots[i + 1]) // 2
                lines.append('    {path: ' + json.dumps(entry['path']) + f', root: {entry["root"]}, keys: [{lo}, {hi}]' + '},')
            lines.append('];')
            lines.append(f'let {tag}_{side} = sample({tag}_{side}_zones, {{')
            written = settings(tag)
            for i, option in enumerate(written):
                lines.append(f'    {option}' + (',' if i < len(written) - 1 else ''))
            lines.append('});')
    return '\n'.join(lines) + '\n' + BUILDERS


def main():
    arguments = sys.argv[1:]
    unknown = [a for a in arguments if a != '--write-muz']
    if unknown:
        raise SystemExit(f'Unknown argument: {unknown[0]}. Usage: derive.py [--write-muz]')
    write_muz = '--write-muz' in arguments
    if shutil.which('ffmpeg') is None:
        raise SystemExit('ffmpeg is required to derive the mono takes but was not found.')
    if not (BANK / 'Samples').is_dir():
        raise SystemExit(f'{relative(BANK)} is missing; run install.py to restore the bank.')
    OUT.mkdir(parents=True, exist_ok=True)

    if MANIFEST.exists():
        previous = json.loads(MANIFEST.read_text())
    else:
        previous = {'archive': {'url': '', 'bytes': 0, 'sha256': ''}}
    pinned = {entry['path']: entry for entry in previous.get('files', [])}
    pinned_sources = {entry['path']: entry for entry in previous.get('sources', [])}

    jobs = plan()
    fresh = [job for job in jobs if not destination(job).exists()]
    with ThreadPoolExecutor(max_workers=WORKERS) as pool:
        files = list(pool.map(lambda job: derive(job, pinned), jobs))
    differing = [f['path'] for f in files if f['path'] in pinned and pinned[f['path']]['sha256'] != f['sha256']]
    print(f'Takes: {len(files)} mono recordings from {len({f["source"] for f in files})} recorded double tracks')
    print(f'Derived: {len(fresh)} new recordings; {len(files) - len(fresh)} already present')
    for path in differing:
        print(f'note: {path} was written by a different ffmpeg build; recorded its sha256 here')
    if pinned:
        print(f'Pins: {len(files) - len(differing)} of {len(files)} takes match manifest.json')

    sources = {}
    for entry in files:
        path = PACK / entry['source']
        sources.setdefault(entry['source'], {'path': entry['source'], 'bytes': path.stat().st_size, 'sha256': sha256(path)})
    for path, entry in sorted(sources.items()):
        documented = pinned_sources.get(path)
        if documented and documented['sha256'] != entry['sha256']:
            raise SystemExit(f'{path} differs from its pinned sha256 {documented["sha256"]}.\n'
                             'The unpacked bank is not the pinned one; remove assets/metal-gtx and run install.py again.')
    print(f'Sources: {len(sources)} bank recordings pinned ({sum(e["bytes"] for e in sources.values())} bytes)')

    archive = previous.get('archive', {})
    if ARCHIVE.exists():
        size, digest = ARCHIVE.stat().st_size, sha256(ARCHIVE)
        if not archive.get('sha256'):
            archive = dict(archive, bytes=size, sha256=digest)
            print(f'Archive: no pinned hash yet; recorded {size} bytes / sha256 {digest}')
        elif (size, digest) != (archive.get('bytes'), archive.get('sha256')):
            raise SystemExit(f'{relative(ARCHIVE)} is {size} bytes / {digest}, not the pinned '
                             f'{archive.get("bytes")} bytes / {archive.get("sha256")}. Nothing was changed.')
        else:
            print(f'Archive: {relative(ARCHIVE)} matches its pin')
    elif ARCHIVE.name:
        print(f'Archive: {relative(ARCHIVE)} is absent; kept the pinned archive hashes')

    manifest = dict(previous)
    manifest['sources'] = [sources[path] for path in sorted(sources)]
    manifest['files'] = files
    body = json.dumps(manifest, indent=2, ensure_ascii=False) + '\n'
    if MANIFEST.exists() and MANIFEST.read_text() == body:
        print('manifest.json: unchanged')
    else:
        MANIFEST.write_text(body)
        print(f'manifest.json: written ({len(files)} takes pinned)')

    generated = module_text(files, archive)
    unchanged = MODULE.exists() and MODULE.read_text() == generated
    if not unchanged:
        if not write_muz:
            raise SystemExit(f'{MODULE.name} differs from the generated table; it was not overwritten.\n'
                             'Inspect the difference and run derive.py --write-muz to accept the generated table.')
        MODULE.write_text(generated)
    print(f'Module: {MODULE.name} ' + ('matches the derived takes' if unchanged else 'regenerated from the derived takes'))


if __name__ == '__main__':
    main()
