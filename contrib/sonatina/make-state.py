#!/usr/bin/env python3
"""Select any Sonatina catalog patch in a verified sfizz VST3 state file."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

PACK = Path(__file__).resolve().parent


def state_path(data):
    """Read the version-5 sfizz VST3 prefix, preserving the opaque remainder.

    Layout: sfizz-ui/plugins/vst/SfizzVstState.cpp and Steinberg IBStreamer.
    This is deliberately not a serializer for all plugin settings.
    """
    if len(data) < 13:
        raise ValueError('plugin returned a truncated state')
    version, size = struct.unpack_from('<QI', data)
    if version != 5:
        raise ValueError(f'unsupported sfizz VST3 state version {version}; expected 5')
    end = 12 + size
    if size < 1 or end + 16 > len(data) or data[end - 1] != 0:
        raise ValueError('plugin returned an invalid sfizz VST3 path field')
    encoded = data[12:end - 1]
    if b'\0' in encoded:
        raise ValueError('plugin state path contains an embedded NUL')
    return encoded.decode('utf-8'), end


def select_patch(template, path):
    current, end = state_path(template)
    if current:
        raise ValueError('fresh plugin state already selects an SFZ; refusing to reuse a configured preset')
    encoded = str(path).encode('utf-8') + b'\0'
    return template[:8] + struct.pack('<I', len(encoded)) + encoded + template[end:]


def run_state(muz, plugin, output, load=None):
    command = [muz, 'devices', 'state', plugin, '-o', str(output)]
    if load is not None:
        command += ['--load', str(load)]
    subprocess.run(command, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--patch', help='exact patch ID from CATALOG.md or --list')
    parser.add_argument('--output', '-o', type=Path, help='project-owned output state path')
    parser.add_argument('--plugin', default='sfizz', help='sfizz VST3 path or muz alias (default: sfizz)')
    parser.add_argument('--muz', default='muz', help='muz executable with sfizz hosting support')
    parser.add_argument('--list', nargs='?', const='', metavar='FILTER', help='list patch IDs, optionally filtered')
    args = parser.parse_args()
    catalog = json.loads((PACK / 'catalog.json').read_text())
    if args.list is not None:
        for row in catalog['patches']:
            if args.list.lower() in (row['id'] + ' ' + row['name']).lower():
                print(f"{row['id']}\t{row['name']}")
        return 0
    if not args.patch or args.output is None:
        parser.error('--patch and --output are required unless using --list')
    matches = [row for row in catalog['patches'] if row['id'] == args.patch]
    if not matches:
        raise ValueError(f'unknown patch {args.patch!r}; use --list to find its ID')
    row = matches[0]
    sfz = (PACK / 'assets' / row['path']).resolve()
    sfz.relative_to((PACK / 'assets').resolve())
    if not sfz.is_file():
        raise ValueError(f'missing {sfz}; run contrib/sonatina/install.py first')
    manifest = json.loads((PACK / 'manifest.json').read_text())
    pin = next(item for item in manifest['files'] if item['path'] == row['path'])
    data = sfz.read_bytes()
    if len(data) != pin['bytes'] or hashlib.sha256(data).hexdigest() != pin['sha256']:
        raise ValueError(f'SFZ differs from its pin: {sfz}; run install.py --check')
    output = args.output.expanduser().resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.sonatina-state-', dir=output.parent) as directory:
        work = Path(directory)
        template, generated, checked = (work / name for name in ('template.state', 'selected.state', 'checked.state'))
        run_state(args.muz, args.plugin, template)
        generated.write_bytes(select_patch(template.read_bytes(), sfz))
        run_state(args.muz, args.plugin, checked, load=generated)
        actual, _ = state_path(checked.read_bytes())
        if actual != str(sfz):
            raise ValueError(f'plugin did not preserve selected SFZ: {actual!r}')
        # Publish the plugin's own accepted/resaved state, atomically.
        os.replace(checked, output)
    print(f'{args.patch}: {output}')
    print(f'SFZ: {sfz}')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'make-state.py: {error}', file=sys.stderr)
        sys.exit(1)
