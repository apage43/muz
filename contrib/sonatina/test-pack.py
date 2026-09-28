#!/usr/bin/env python3
"""Small synthetic checks for verified archive placement and sfizz state editing."""
import hashlib
import importlib.util
import io
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
from unittest.mock import patch

PACK = Path(__file__).resolve().parent


def module(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), PACK / (name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


installer = module('install')
state = module('make-state')
REPOSITORY = {'name': 'test/repo', 'revision': 'abc'}


def tar(entries):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode='w:gz') as archive:
        for name, contents in entries:
            info = tarfile.TarInfo(name)
            info.size = len(contents)
            archive.addfile(info, io.BytesIO(contents))
    stream.seek(0)
    return stream


def row(contents=b'audio'):
    return {'repo_path': 'sample.wav', 'path': 'sso/sample.wav', 'bytes': len(contents), 'sha256': hashlib.sha256(contents).hexdigest()}


class PackTests(unittest.TestCase):
    def test_verified_archive_publishes_only_pinned_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(installer, 'ASSETS', root), patch.object(installer, 'report'):
                installer.unpack_archive(tar([('repo-abc/unused.txt', b'ignored'), ('repo-abc/sample.wav', b'audio')]), REPOSITORY, [row()])
            self.assertEqual((root / 'sso/sample.wav').read_bytes(), b'audio')
            self.assertFalse((root / 'unused.txt').exists())

    def test_bad_hash_does_not_replace_existing_file_or_leave_part(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / 'sso/sample.wav'
            destination.parent.mkdir()
            destination.write_bytes(b'existing')
            with patch.object(installer, 'ASSETS', root), self.assertRaises(installer.InstallError):
                installer.unpack_archive(tar([('repo-abc/sample.wav', b'wrong')]), REPOSITORY, [row()])
            self.assertEqual(destination.read_bytes(), b'existing')
            self.assertEqual(list(destination.parent.iterdir()), [destination])

    def test_missing_and_traversal_members_fail(self):
        for entries in ([], [('repo-abc/../sample.wav', b'audio')]):
            with self.assertRaises(installer.InstallError):
                installer.unpack_archive(tar(entries), REPOSITORY, [row()])

    def test_state_path_preserves_plugin_configuration(self):
        suffix = bytes(range(80))
        original = struct.pack('<QI', 5, 1) + b'\0' + suffix
        selected = state.select_patch(original, Path('/tmp/Solo Violín/Sustain.sfz'))
        path, end = state.state_path(selected)
        self.assertEqual(path, '/tmp/Solo Violín/Sustain.sfz')
        self.assertEqual(selected[end:], suffix)
        with self.assertRaises(ValueError):
            state.select_patch(selected, Path('/tmp/other.sfz'))

    def test_unknown_or_malformed_state_fails(self):
        for data in (b'', struct.pack('<QI', 6, 1) + b'\0' * 80, struct.pack('<QI', 5, 9999) + b'\0' * 80):
            with self.assertRaises(ValueError):
                state.state_path(data)


if __name__ == '__main__':
    unittest.main()
