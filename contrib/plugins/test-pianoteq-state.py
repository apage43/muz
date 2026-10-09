#!/usr/bin/env python3
"""Synthetic consumer checks for native Pianoteq factory-state capture."""
import importlib.util
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch

PACK = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('pianoteq_state', PACK / 'pianoteq-state.py')
state = importlib.util.module_from_spec(spec)
spec.loader.exec_module(state)


def identity(name):
    encoded = name.encode('utf-8')
    return struct.pack('<I', len(encoded)) + encoded


def component(name='Example Warm', suffix=b'opaque suffix'):
    payload = b'PrVK' + identity(name) + b'\0synthetic parameters'
    header = bytearray(176)
    for at, magic in ((0, b'VstW'), (16, b'CcnK'), (24, b'FBCh'), (32, b'Pt9q')):
        header[at:at + 4] = magic
    struct.pack_into('>I', header, 36, state.VERSION)
    bank = bytearray(28)
    bank[:8] = bytes.fromhex('a09813fe02000000')
    struct.pack_into('<I', bank, 12, len(payload) + 12)
    struct.pack_into('<I', bank, 24, len(payload))
    bank += payload + suffix
    struct.pack_into('>I', header, 20, 152 + len(bank))
    struct.pack_into('>I', header, 172, len(bank))
    return bytes(header + bank), payload


def vst3(chunks):
    header = bytearray(48)
    header[:4] = b'VST3'
    body = bytearray()
    entries = bytearray()
    for kind, contents in chunks:
        entries += kind + struct.pack('<QQ', 48 + len(body), len(contents))
        body += contents
    struct.pack_into('<Q', header, 40, 48 + len(body))
    return bytes(header + body + b'List' + struct.pack('<I', len(chunks)) + entries)


def changed(data, offset, fmt, value):
    result = bytearray(data)
    struct.pack_into(fmt, result, offset, value)
    return bytes(result)


class BinaryTests(unittest.TestCase):
    def test_identity_is_exact_and_counts_utf8_bytes(self):
        for name in ('Example Warm', 'Exemple Résonant 🎹'):
            payload = b'PrVK' + identity(name)
            self.assertTrue(state.has_preset_identity(payload, name))
            self.assertFalse(state.has_preset_identity(b'PrVK' + identity(name + ' Extra'), name))
            self.assertFalse(state.has_preset_identity(b'PrVK' + name.encode(), name))
        name = 'Résonant'
        wrong_count = struct.pack('<I', len(name)) + name.encode()
        self.assertFalse(state.has_preset_identity(wrong_count, name))

    def test_identity_outside_selected_payload_is_ignored(self):
        data, payload = component('Other', suffix=identity('Example Warm'))
        self.assertEqual(state.component_preset(data), payload)
        self.assertFalse(state.has_preset_identity(state.component_preset(data), 'Example Warm'))
        self.assertEqual(state.component_preset(component(suffix=b'')[0]), component(suffix=b'')[1])

    def test_invalid_component_framing_is_rejected(self):
        data, payload = component()
        broken = [data[:207], b'xxxx' + data[4:],
                  changed(data, 36, '>I', 0x00090300),
                  changed(data, 20, '>I', len(data) - 25),
                  changed(data, 172, '>I', len(data) - 177),
                  changed(data, 176, '<I', 0),
                  changed(data, 188, '<I', len(payload) + 13),
                  changed(data, 200, '<I', 3),
                  changed(data, 200, '<I', len(data)),
                  data[:204] + b'xxxx' + data[208:]]
        for value in broken:
            with self.subTest(value=value[:40]), self.assertRaises(ValueError):
                state.component_preset(value)

    def test_native_chunks_extract_selected_component_only(self):
        data, payload = component('Other')
        preset = vst3([(b'Info', identity('Example Warm')), (b'Comp', data), (b'Cont', b'controller')])
        self.assertEqual(state.vst3_component(preset), data)
        self.assertEqual(state.component_preset(state.vst3_component(preset)), payload)
        self.assertFalse(state.has_preset_identity(state.component_preset(state.vst3_component(preset)), 'Example Warm'))

    def test_invalid_native_containers_are_rejected(self):
        data, _ = component()
        preset = vst3([(b'Comp', data)])
        offset = struct.unpack_from('<Q', preset, 40)[0]
        broken = [preset[:47], b'xxxx' + preset[4:],
                  changed(preset, 40, '<Q', 0), changed(preset, 40, '<Q', len(preset)),
                  preset[:-1], changed(preset, offset + 4, '<I', 2),
                  changed(preset, offset + 12, '<Q', len(preset) + 1),
                  changed(preset, offset + 20, '<Q', len(preset)),
                  preset[:offset] + b'xxxx' + preset[offset + 4:],
                  vst3([(b'Info', b'no component')]),
                  vst3([(b'Comp', data), (b'Comp', data)])]
        for value in broken:
            with self.subTest(size=len(value)), self.assertRaises(ValueError):
                state.vst3_component(value)

    def test_state_size_limit(self):
        with patch.object(state, 'MAX_STATE', 250):
            with self.assertRaises(ValueError):
                state.component_preset(component(suffix=b'x' * 100)[0])
            with self.assertRaises(ValueError):
                state.vst3_component(vst3([(b'Comp', component()[0])]))
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / 'oversize.state'
                path.write_bytes(b'x' * 251)
                with self.assertRaises(ValueError):
                    state.read_state(path)

    def test_missing_duplicate_and_malformed_factory_presets(self):
        with tempfile.TemporaryDirectory() as directory:
            export = Path(directory)
            first = export / 'arbitrary filename.vstpreset'
            first.write_bytes(vst3([(b'Comp', component('Example Warm Extra')[0])]))
            with self.assertRaisesRegex(ValueError, 'Example Warm.*not found'):
                state.select_factory(export, 'Example Warm')
            first.write_bytes(vst3([(b'Comp', component()[0])]))
            self.assertEqual(state.select_factory(export, 'Example Warm'), first)
            second = export / 'duplicate.vstpreset'
            second.write_bytes(first.read_bytes())
            with self.assertRaisesRegex(ValueError, 'ambiguous.*arbitrary filename.*duplicate'):
                state.select_factory(export, 'Example Warm')
            second.write_bytes(b'malformed')
            with self.assertRaisesRegex(ValueError, 'malformed.*duplicate'):
                state.select_factory(export, 'Example Warm')


class InputTests(unittest.TestCase):
    def fixture(self, root):
        plugin = root / 'Instrument.vst3'
        plugin.write_bytes(b'synthetic plugin')
        standalone = root / 'Instrument'
        standalone.write_text('#!/bin/sh\nexit 1\n')
        standalone.chmod(0o700)
        prefs = root / 'activation.prefs'
        prefs.write_bytes(b'synthetic preference input')
        config = root / 'custom/plugins.json'
        config.parent.mkdir()
        config.write_text(json.dumps({'default': {'path': '../Instrument.vst3', 'class': 'alias-class'}}))
        env = dict(os.environ, MUZ_PLUGIN_CONFIG=str(config))
        args = state.parser().parse_args(['--preset', 'Example Warm', '-o', str(root / 'output.state'),
                                         '--prefs', str(prefs), '--muz', str(standalone)])
        return args, env, config

    def test_relative_alias_and_explicit_class_precedence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args, env, config = self.fixture(root)
            plugin, class_id, _ = state.resolve_plugin('default', None, env)
            self.assertEqual(plugin, root / 'Instrument.vst3')
            self.assertEqual(class_id, 'alias-class')
            self.assertEqual(state.resolve_plugin('default', 'explicit-class', env)[1], 'explicit-class')
            config.write_text('invalid JSON')
            self.assertEqual(state.resolve_plugin(str(plugin), None, env), (plugin, None, None))
            with self.assertRaises(ValueError):
                state.resolve_plugin('default', None, env)

    def test_missing_alias_and_invalid_config_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args, env, config = self.fixture(root)
            for value in ({}, [], {'default': 'path'}, {'default': {'path': 7}},
                          {'default': {'path': '../Instrument.vst3', 'class': 7}}):
                config.write_text(json.dumps(value))
                with self.subTest(value=value), self.assertRaises(ValueError):
                    state.resolve_inputs(args, env)

    def test_output_cannot_overwrite_inputs_or_hardlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args, env, config = self.fixture(root)
            linked = root / 'linked-output'
            os.link(args.prefs, linked)
            for path in (args.prefs, root / 'Instrument', root / 'Instrument.vst3', config, linked):
                args.output = path
                with self.subTest(path=path), self.assertRaisesRegex(ValueError, 'overwrite capture input'):
                    state.resolve_inputs(args, env)

    def test_unsupported_standalone_version_fails(self):
        for text in ('Pianoteq version 9.3.0/test', 'Pianoteq version 9.2.40/test', 'unrecognized'):
            with patch.object(state.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, text, '')):
                with self.assertRaisesRegex(ValueError, 'unsupported Pianoteq version'):
                    state.check_version('/standalone', Path('/copied.prefs'), {})

    def test_isolation_preserves_original_preferences_and_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args, env, _ = self.fixture(root)
            env['XDG_DATA_HOME'] = '/user/instrument/assets'
            original_env = dict(env)
            before = args.prefs.read_bytes(), args.prefs.stat()
            inputs = state.resolve_inputs(args, env)
            with state.isolated_environment(inputs, env) as (work, copied, child_env):
                self.assertEqual(copied.stat().st_mode & 0o777, 0o600)
                self.assertEqual(child_env['XDG_DATA_HOME'], env['XDG_DATA_HOME'])
                copied.write_bytes(b'child changed its preferences')
                self.assertNotEqual(child_env['XDG_CONFIG_HOME'], env.get('XDG_CONFIG_HOME'))
            after = args.prefs.stat()
            self.assertEqual(args.prefs.read_bytes(), before[0])
            self.assertEqual((after.st_size, after.st_mode, after.st_mtime_ns),
                             (before[1].st_size, before[1].st_mode, before[1].st_mtime_ns))
            self.assertEqual(env, original_env)
            self.assertFalse(work.exists())

    def test_capture_failures_preserve_existing_destination(self):
        for failure in ('export', 'load', 'wrong identity', 'malformed resave', 'publish'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                args, env, _ = self.fixture(root)
                args.output.write_bytes(b'previous project state')

                def run(command, **kwargs):
                    if '--version' in command:
                        return subprocess.CompletedProcess(command, 0, 'Pianoteq version 9.2.4/test', '')
                    if '--export-vst3-presets' in command:
                        if failure == 'export':
                            raise subprocess.CalledProcessError(1, command)
                        export = Path(command[command.index('--export-vst3-presets') + 1])
                        export.mkdir()
                        (export / 'factory.vstpreset').write_bytes(vst3([(b'Comp', component()[0])]))
                    else:
                        if failure == 'load':
                            raise subprocess.CalledProcessError(1, command)
                        checked = Path(command[command.index('-o') + 1])
                        if failure == 'malformed resave':
                            checked.write_bytes(b'invalid component')
                        else:
                            checked.write_bytes(component('Other' if failure == 'wrong identity' else 'Example Warm',
                                                          suffix=identity('Example Warm'))[0])
                    return subprocess.CompletedProcess(command, 0)

                with patch.dict(os.environ, env, clear=True), patch.object(state.subprocess, 'run', side_effect=run):
                    if failure == 'publish':
                        with patch.object(state.os, 'replace', side_effect=OSError('publication failed')):
                            with self.assertRaisesRegex(OSError, 'publication failed'):
                                state.capture(args)
                    else:
                        with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                            state.capture(args)
                self.assertEqual(args.output.read_bytes(), b'previous project state')
                self.assertFalse(list(root.glob('.pianoteq-state-*')))


if __name__ == '__main__':
    unittest.main()
