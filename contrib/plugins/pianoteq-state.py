#!/usr/bin/env python3
"""Capture an exact Pianoteq factory or user preset as project-owned VST3 component state.

Compatibility: Linux Pianoteq 9.2.4 only; see the Pianoteq section of README.md.
"""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
from xml.sax.saxutils import escape

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
    fxp = None
    if args.fxp is not None:
        fxp = args.fxp.expanduser().resolve()
        if not fxp.is_file():
            raise ValueError(f'missing user preset file: {fxp}')
    elif not args.preset or not args.preset.strip():
        raise ValueError('--preset must be a nonempty exact factory name')
    plugin, class_id, config = resolve_plugin(args.plugin, args.class_id, env)
    # A user preset needs no export, so no standalone.
    standalone = None if fxp else executable(args.pianoteq or plugin.with_suffix(''))
    muz = executable(args.muz)
    prefs = (args.prefs or config_root(env) / 'Modartt' / PREFS_NAME).expanduser().resolve()
    if not prefs.is_file():
        raise ValueError(f'missing installed activation preferences: {prefs}; use --prefs PATH')
    output = args.output.expanduser().resolve()
    for source in (plugin, standalone, muz, prefs, config, fxp):
        if source is not None and (output == source or (output.exists() and os.path.samefile(output, source))):
            raise ValueError(f'output must not overwrite capture input: {source}')
    if output.exists() and not output.is_file():
        raise ValueError(f'output must be a component-state file: {output}')
    return argparse.Namespace(preset=args.preset, fxp=fxp, output=output, plugin=plugin,
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
    source = result.add_mutually_exclusive_group(required=True)
    source.add_argument('--preset', help='exact, case-sensitive factory preset name')
    source.add_argument('--fxp', type=Path, help='a user preset saved by Pianoteq (.fxp)')
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


LEAN = struct.pack('<I', len(b'LEAN-PRESET')) + b'LEAN-PRESET'


def reject_lean(payload: bytes, source):
    """A lean preset names its preset over a base; the plugin loads only the base."""
    if LEAN in payload:
        raise ValueError(f'{source} is a LEAN-PRESET reference, which the plugin loads as its base '
                         'preset; capture the saved user preset with --fxp PATH.fxp instead')


def fxp_program(fxp: bytes) -> tuple[bytes, str]:
    """Return a 9.2.4 opaque-chunk program's PrVK payload and its stored name."""
    if len(fxp) < 0x40 or len(fxp) > MAX_STATE or fxp[:4] != b'CcnK' or fxp[8:12] != b'FPCh':
        raise ValueError('not an opaque-chunk program (.fxp) file')
    if fxp[16:20] != b'Pt9q':
        raise ValueError('foreign .fxp: expected a Pianoteq 9 program')
    if struct.unpack_from('>I', fxp, 4)[0] not in (0, len(fxp) - 8):  # Pianoteq writes 0
        raise ValueError('inconsistent .fxp length')
    payload = fxp[0x3c:]
    if struct.unpack_from('>I', fxp, 0x38)[0] != len(payload) or payload[:4] != b'PrVK':
        raise ValueError('.fxp program chunk is not one PrVK payload')
    name = fxp[0x1c:0x38].split(b'\0', 1)[0].decode('utf-8', 'replace')
    return payload, name


def with_program(state: bytes, payload: bytes) -> bytes:
    """Replace a component's PrVK payload, rewriting its four lengths."""
    old = component_preset(state)
    delta = len(payload) - len(old)
    result = bytearray(state[:204]) + payload + state[204 + len(old):]
    for at, fmt in ((20, '>I'), (172, '>I'), (188, '<I'), (200, '<I')):
        struct.pack_into(fmt, result, at, struct.unpack_from(fmt, result, at)[0] + delta)
    if component_preset(bytes(result)) != payload:
        raise ValueError('component rebuild failed')
    return bytes(result)


def read_state(path):
    with path.open('rb') as stream:
        data = stream.read(MAX_STATE + 1)
    if len(data) > MAX_STATE:
        raise ValueError(f'state exceeds 64 MiB: {path}')
    return data


def private_user_root(prefs: Path, root: Path):
    """Point the copied preferences' user folder (presets, banks) at a private root."""
    text = prefs.read_text(encoding='utf-8')
    folder = escape(str(root) + "/", {'"': "&quot;"})
    value = f'<VALUE name="user_root_folder" val="{folder}"/>'
    text, count = re.subn(r'<VALUE name="user_root_folder" val="[^"]*"\s*/>', lambda _: value, text)
    if count == 0:
        if '</PROPERTIES>' not in text:
            raise ValueError(f'unrecognized Pianoteq preferences: {prefs}')
        text = text.replace('</PROPERTIES>', f'  {value}\n</PROPERTIES>', 1)
    prefs.write_text(text, encoding='utf-8')


def free_port():
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        return probe.getsockname()[1]


SERVE_TIMEOUT = 120
TEMP_BANK = 'muz-capture'


def rpc(port, method, params):
    body = json.dumps({'jsonrpc': '2.0', 'method': method, 'params': params, 'id': 1}).encode()
    with urllib.request.urlopen(f'http://127.0.0.1:{port}/jsonrpc', body, timeout=60) as reply:
        result = json.loads(reply.read())
    if 'error' in result:
        raise ValueError(f'Pianoteq {method}: {result["error"].get("message", result["error"])}')
    return result['result']


def saved_factory_fxp(inputs, copied, env, work):
    """Save a factory preset as a full .fxp through the standalone's JSON-RPC server.

    9.2.4 exports factory presets only as LEAN-PRESET references, but `savePreset`
    writes the whole program. The bank lands in a private user root, so the user's
    presets are neither read (only factory presets can match) nor written.
    """
    root = work / 'user'
    root.mkdir()
    private_user_root(copied, root)
    port = free_port()
    server = subprocess.Popen([str(inputs.pianoteq), '--headless', '--prefs', str(copied),
                               '--serve', f'127.0.0.1:{port}'],
                              env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.monotonic() + SERVE_TIMEOUT
        while True:
            try:
                info = rpc(port, 'getInfo', [])[0]
                break
            except OSError:
                if server.poll() is not None:
                    raise ValueError(f'Pianoteq standalone exited ({server.returncode}) before serving')
                if time.monotonic() > deadline:
                    raise ValueError('Pianoteq standalone did not start its JSON-RPC server')
                time.sleep(0.25)
        if info.get('version') != '9.2.4':
            raise ValueError(f'unsupported Pianoteq version {info.get("version")}; use matching Linux 9.2.4')
        try:
            rpc(port, 'loadPreset', {'name': inputs.preset})
        except ValueError as error:
            raise ValueError(f'factory preset {inputs.preset!r} not found or not licensed: {error}') from error
        current = rpc(port, 'getInfo', [])[0]['current_preset']
        if current.get('name') != inputs.preset or current.get('bank'):
            raise ValueError(f'Pianoteq loaded {current.get("name")!r} (bank {current.get("bank")!r}) '
                             f'for factory preset {inputs.preset!r}')
        rpc(port, 'savePreset', {'name': inputs.preset, 'bank': TEMP_BANK})
    finally:
        try:
            rpc(port, 'quit', [])
        except (OSError, ValueError):
            pass
        try:
            server.wait(timeout=30)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()
    saved = sorted((root / 'Presets' / TEMP_BANK).glob('*.fxp'))
    if len(saved) != 1:
        raise ValueError(f'Pianoteq did not save factory preset {inputs.preset!r} as one .fxp')
    return saved[0]


def device_state(inputs, env, output, load=None):
    command = [str(inputs.muz), 'devices', 'state', str(inputs.plugin)]
    if inputs.class_id is not None:
        command += ['--class', inputs.class_id]
    if load is not None:
        command += ['--load', str(load)]
    subprocess.run(command + ['-o', str(output)], env=env, check=True)


def wrap_program(inputs, env, work, payload, name):
    """Put a full program in the plugin's own component framing; publish the resaved state."""
    base = work / 'base.state'
    device_state(inputs, env, base)
    staged = work / 'staged.state'
    staged.write_bytes(with_program(read_state(base), payload))
    checked = work / 'checked.state'
    device_state(inputs, env, checked, load=staged)
    resaved = component_preset(read_state(checked))
    if not has_preset_identity(resaved, name) or LEAN in resaved:
        raise ValueError(f'plugin did not retain preset {name!r}')
    # Publish only the plugin's accepted/resaved state, on the same filesystem.
    os.replace(checked, inputs.output)


def capture_user(inputs):
    """Wrap a saved user preset's program in the plugin's own component framing."""
    payload, stored = fxp_program(read_state(inputs.fxp))
    reject_lean(payload, inputs.fxp)
    # The .fxp name field holds 28 bytes; the payload holds the whole name.
    name = inputs.fxp.stem
    if not has_preset_identity(payload, name):
        raise ValueError(f'{inputs.fxp} does not carry the preset name {name!r} (stored: {stored!r})')
    with isolated_environment(inputs, os.environ) as (work, _, env):
        wrap_program(inputs, env, work, payload, name)
    print(f'{name}: {inputs.output}')
    print(f'Plugin: {inputs.plugin}')
    return 0


def capture(args):
    inputs = resolve_inputs(args, os.environ)
    if inputs.fxp is not None:
        return capture_user(inputs)
    with isolated_environment(inputs, os.environ) as (work, copied, env):
        check_version(inputs.pianoteq, copied, env)
        payload, _ = fxp_program(read_state(saved_factory_fxp(inputs, copied, env, work)))
        reject_lean(payload, f'saved factory preset {inputs.preset!r}')
        if not has_preset_identity(payload, inputs.preset):
            raise ValueError(f'saved factory preset does not carry the name {inputs.preset!r}')
        wrap_program(inputs, env, work, payload, inputs.preset)
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
