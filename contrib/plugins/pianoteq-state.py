#!/usr/bin/env python3
"""Capture an exact Pianoteq factory preset as project-owned VST3 component state.

Compatibility: Linux Pianoteq 9.2.4 only; see the Pianoteq section of README.md.
"""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile

MAX_STATE = 64 * 1024 * 1024
VERSION = 0x00090204
PREFS_NAME = 'Pianoteq92.prefs'


def config_root(env):
    return Path(env.get('XDG_CONFIG_HOME', str(Path(env.get('HOME', str(Path.home()))) / '.config')))


def resolve_plugin(value, class_id, env):
    """Existing paths take precedence over aliases; retain an alias's class."""
    explicit = Path(value).expanduser()
    config = None
    if explicit.exists():
        plugin = explicit.resolve()
    else:
        config = Path(env.get('MUZ_PLUGIN_CONFIG', str(config_root(env) / 'muz/plugins.json'))).resolve()
        aliases = json.loads(config.read_text())
        if not isinstance(aliases, dict):
            raise ValueError(f'plugin config must be a JSON object: {config}')
        alias = aliases.get(value)
        if not isinstance(alias, dict) or not isinstance(alias.get('path'), str) or not alias['path']:
            raise ValueError(f'plugin alias {value!r} needs an object with a string path in {config}')
        plugin = Path(alias['path']).expanduser()
        if not plugin.is_absolute():
            plugin = config.parent / plugin
        plugin = plugin.resolve()
        configured_class = alias.get('class')
        if configured_class is not None and not isinstance(configured_class, str):
            raise ValueError(f'plugin alias {value!r} class must be a string in {config}')
        if class_id is None:
            class_id = configured_class
    if not plugin.exists() or plugin.suffix != '.vst3' or not (plugin.is_file() or plugin.is_dir()):
        raise ValueError(f'expected an installed Pianoteq VST3 file or bundle: {plugin}')
    return plugin, class_id, config


def executable(value):
    path = Path(value).expanduser()
    found = str(path.resolve()) if path.exists() else shutil.which(str(path))
    if found is None:
        raise ValueError(f'executable unavailable: {value}')
    result = Path(found).resolve()
    if not result.is_file() or not os.access(result, os.X_OK):
        raise ValueError(f'expected executable file with execute permission: {result}')
    return result


def resolve_inputs(args, env):
    if sys.platform != 'linux':
        raise ValueError('unsupported platform; capture requires Linux Pianoteq 9.2.4')
    if not args.preset or not args.preset.strip():
        raise ValueError('--preset must be a nonempty exact factory name')
    plugin, class_id, config = resolve_plugin(args.plugin, args.class_id, env)
    standalone = executable(args.pianoteq or plugin.with_suffix(''))
    muz = executable(args.muz)
    prefs = (args.prefs or config_root(env) / 'Modartt' / PREFS_NAME).expanduser().resolve()
    if not prefs.is_file():
        raise ValueError(f'missing installed activation preferences: {prefs}; use --prefs PATH')
    output = args.output.expanduser().resolve()
    for source in (plugin, standalone, muz, prefs, config):
        if source is not None and (output == source or (output.exists() and os.path.samefile(output, source))):
            raise ValueError(f'output must not overwrite capture input: {source}')
    if output.exists() and not output.is_file():
        raise ValueError(f'output must be a component-state file: {output}')
    return argparse.Namespace(preset=args.preset, output=output, plugin=plugin,
                              class_id=class_id, pianoteq=standalone, prefs=prefs, muz=muz)


@contextmanager
def isolated_environment(inputs, env):
    inputs.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.pianoteq-state-', dir=inputs.output.parent) as directory:
        work = Path(directory)
        copied = work / 'config/Modartt' / PREFS_NAME
        copied.parent.mkdir(parents=True)
        shutil.copyfile(inputs.prefs, copied)
        copied.chmod(0o600)
        child_env = dict(env, XDG_CONFIG_HOME=str(work / 'config'))
        yield work, copied, child_env


def check_version(standalone, copied, env):
    result = subprocess.run([str(standalone), '--headless', '--prefs', str(copied), '--version'],
                            env=env, check=True, capture_output=True, text=True)
    match = re.search(r'\bPianoteq version ([^\s/]+)', result.stdout)
    if match is None or match.group(1) != '9.2.4':
        version = match.group(1) if match else 'unrecognized'
        raise ValueError(f'unsupported Pianoteq version {version}; use matching Linux 9.2.4 standalone/VST3')


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument('--preset', required=True, help='exact, case-sensitive factory preset name')
    result.add_argument('--output', '-o', required=True, type=Path, help='project-owned component-state destination')
    result.add_argument('--plugin', default='default', help='installed VST3 path or muz alias (default: default)')
    result.add_argument('--class', dest='class_id', help='VST3 class ID; overrides the alias class')
    result.add_argument('--pianoteq', type=Path, help='matching standalone executable (default: VST3 sibling)')
    result.add_argument('--prefs', type=Path, help='activation preferences (default: original config root/Modartt/Pianoteq92.prefs)')
    result.add_argument('--muz', default='muz', help='muz executable (default: muz on PATH)')
    return result


def component_preset(state: bytes) -> bytes:
    """Validate the 9.2.4 component bank and return only its selected PrVK payload."""
    if len(state) < 208 or len(state) > MAX_STATE:
        raise ValueError('invalid component size: expected 208 bytes to 64 MiB')
    for offset, magic in ((0, b'VstW'), (16, b'CcnK'), (24, b'FBCh'), (32, b'Pt9q')):
        if state[offset:offset + 4] != magic:
            raise ValueError(f'foreign or malformed Pianoteq component header at {offset}')
    version = struct.unpack_from('>I', state, 36)[0]
    if version != VERSION:
        raise ValueError(f'unsupported Pianoteq component version 0x{version:08x}; expected Linux 9.2.4')
    if struct.unpack_from('>I', state, 20)[0] != len(state) - 24:
        raise ValueError('inconsistent component wrapper length')
    bank = state[176:]
    if struct.unpack_from('>I', state, 172)[0] != len(bank):
        raise ValueError('inconsistent component bank length')
    if bank[:8] != bytes.fromhex('a09813fe02000000'):
        raise ValueError('unsupported Pianoteq component bank framing')
    n = struct.unpack_from('<I', bank, 24)[0]
    if not 4 <= n <= len(bank) - 28:
        raise ValueError('invalid component preset extent')
    if struct.unpack_from('<I', bank, 12)[0] != n + 12:
        raise ValueError('inconsistent component preset length')
    if bank[28:32] != b'PrVK':
        raise ValueError('component preset is not PrVK')
    return bank[28:28 + n]


def has_preset_identity(payload: bytes, name: str) -> bool:
    """Version-specific length-prefixed UTF-8 identity, not a general preset parser."""
    encoded = name.encode('utf-8')
    return struct.pack('<I', len(encoded)) + encoded in payload


def vst3_component(preset: bytes) -> bytes:
    """Extract exactly one bounded Comp chunk from a standard VST3 preset."""
    if len(preset) < 48 or len(preset) > MAX_STATE or preset[:4] != b'VST3':
        raise ValueError('invalid VST3 preset header or size (maximum 64 MiB)')
    offset = struct.unpack_from('<Q', preset, 40)[0]
    if offset < 48 or offset + 8 > len(preset) or preset[offset:offset + 4] != b'List':
        raise ValueError('invalid VST3 chunk-list offset or marker')
    count = struct.unpack_from('<I', preset, offset + 4)[0]
    if count > (len(preset) - offset - 8) // 20:
        raise ValueError('truncated VST3 chunk table')
    component = None
    for index in range(count):
        at = offset + 8 + index * 20
        start, size = struct.unpack_from('<QQ', preset, at + 4)
        if start < 48 or start > len(preset) or size > len(preset) - start:
            raise ValueError('invalid VST3 chunk extent')
        if preset[at:at + 4] == b'Comp':
            if component is not None:
                raise ValueError('duplicate VST3 Comp chunks')
            component = preset[start:start + size]
    if component is None:
        raise ValueError('VST3 preset has no Comp chunk')
    return component


def read_state(path):
    with path.open('rb') as stream:
        data = stream.read(MAX_STATE + 1)
    if len(data) > MAX_STATE:
        raise ValueError(f'state exceeds 64 MiB: {path}')
    return data


def select_factory(export, name):
    matches = []
    for path in sorted(export.rglob('*.vstpreset')):
        try:
            payload = component_preset(vst3_component(read_state(path)))
        except ValueError as error:
            raise ValueError(f'malformed factory preset {path}: {error}') from error
        if has_preset_identity(payload, name):
            matches.append(path)
    if not matches:
        raise ValueError(f'factory preset {name!r} not found in installed builtin export')
    if len(matches) != 1:
        raise ValueError(f'ambiguous factory preset {name!r}: ' + ', '.join(map(str, matches)))
    return matches[0]


def capture(args):
    inputs = resolve_inputs(args, os.environ)
    with isolated_environment(inputs, os.environ) as (work, copied, env):
        check_version(inputs.pianoteq, copied, env)
        export = work / 'export'
        subprocess.run([str(inputs.pianoteq), '--headless', '--prefs', str(copied),
                        '--export-vst3-presets', str(export), '--export-presets-filter', 'builtin'],
                       env=env, check=True)
        selected = select_factory(export, inputs.preset)
        checked = work / 'checked.state'
        command = [str(inputs.muz), 'devices', 'state', str(inputs.plugin)]
        if inputs.class_id is not None:
            command += ['--class', inputs.class_id]
        command += ['--load', str(selected), '-o', str(checked)]
        subprocess.run(command, env=env, check=True)
        payload = component_preset(read_state(checked))
        if not has_preset_identity(payload, inputs.preset):
            raise ValueError(f'plugin did not retain requested factory preset {inputs.preset!r}')
        # Publish only the plugin's accepted/resaved state, on the same filesystem.
        os.replace(checked, inputs.output)
    print(f'{inputs.preset}: {inputs.output}')
    print(f'Plugin: {inputs.plugin}')
    return 0


def main():
    return capture(parser().parse_args())


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'pianoteq-state.py: {error}', file=sys.stderr)
        sys.exit(1)
