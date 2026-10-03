#!/usr/bin/env python3
"""Synthetic checks for archive placement, corrected SFZs and sfizz state editing."""
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


class StateOverlayTests(unittest.TestCase):
    SOURCE = (
        b'// License: CC Sampling Plus 1.0\r\n'
        b'// Original mapping\n'
        b'\r\n'
        b'/        Cymbals & Tamtam\r\n'
        b'#include "../Fragments/release.sfz"\r\n'
        b'<region> sample=../Samples/cymbal.wav key=49 volume=-3\n'
    )
    REMOVAL = '/        Cymbals & Tamtam\r\n'

    def fixture(self, root, contents=None, removals=None):
        contents = self.SOURCE if contents is None else contents
        source = root / 'assets/sso/Programs/Cymbals & Tamtam.sfz'
        source.parent.mkdir(parents=True)
        source.write_bytes(contents)
        sample = source.parent / '../Samples/cymbal.wav'
        sample.parent.mkdir()
        sample.write_bytes(b'synthetic audio')
        fragment = source.parent / '../Fragments/release.sfz'
        fragment.parent.mkdir()
        fragment.write_bytes(b'<group> ampeg_release=2\n')
        patch_row = {'id': 'percussion-cymbals-tamtam', 'path': source.relative_to(root / 'assets').as_posix()}
        digest = hashlib.sha256(contents).hexdigest()
        manifest = {'files': [{**patch_row, 'bytes': len(contents), 'sha256': digest}]}
        overlay = {
            'path': source.name, 'sha256': digest,
            'removals': [self.REMOVAL] if removals is None else removals,
            'reason': 'Synthetic pinned malformed header',
        }
        descriptor = {'programs': [{**patch_row, 'source_overlays': [overlay]}]}
        return source, patch_row, manifest, descriptor

    def test_root_overlay_preserves_mapping_original_and_line_endings(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, patch_row, manifest, descriptor = self.fixture(root)
            with patch.object(state, 'PACK', root):
                derived = state.prepared_sfz(patch_row, manifest, descriptor)
                self.assertEqual(state.prepared_sfz(patch_row, manifest, descriptor), derived)
            expected = self.SOURCE.replace(self.REMOVAL.encode(), b' ' * (len(self.REMOVAL) - 2) + b'\r\n')
            self.assertEqual(derived.parent, source.parent)
            self.assertEqual(derived.read_bytes(), expected)
            self.assertEqual(source.read_bytes(), self.SOURCE)
            self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), manifest['files'][0]['sha256'])
            self.assertEqual(len(expected), len(self.SOURCE))
            self.assertEqual(expected.splitlines(keepends=True)[3], b' ' * (len(self.REMOVAL) - 2) + b'\r\n')
            self.assertIn(b'// License: CC Sampling Plus 1.0\r\n', expected)
            self.assertIn(b'#include "../Fragments/release.sfz"\r\n', expected)
            self.assertIn(b'<region> sample=../Samples/cymbal.wav key=49 volume=-3\n', expected)
            self.assertEqual((derived.parent / '../Samples/cymbal.wav').read_bytes(), b'synthetic audio')
            self.assertEqual((derived.parent / '../Fragments/release.sfz').read_bytes(), b'<group> ampeg_release=2\n')

    def test_bad_source_or_overlay_hash_leaves_existing_derivative_untouched(self):
        for mismatch in ('source', 'overlay'):
            with self.subTest(mismatch=mismatch), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source, patch_row, manifest, descriptor = self.fixture(root)
                derived = source.with_name(source.stem + '.muz-sfizz.sfz')
                derived.write_bytes(b'previous derivative')
                if mismatch == 'source':
                    source.write_bytes(self.SOURCE + b'changed')
                else:
                    descriptor['programs'][0]['source_overlays'][0]['sha256'] = '0' * 64
                original = source.read_bytes()
                with patch.object(state, 'PACK', root), self.assertRaisesRegex(ValueError, 'pin|hash mismatch'):
                    state.prepared_sfz(patch_row, manifest, descriptor)
                self.assertEqual(source.read_bytes(), original)
                self.assertEqual(derived.read_bytes(), b'previous derivative')
                self.assertFalse(list(source.parent.glob('*.part')))

    def test_removal_must_match_exactly_once(self):
        for contents, removals in (
            (self.SOURCE, [self.REMOVAL.replace('\r\n', '\n')]),
            (self.SOURCE + self.REMOVAL.encode(), [self.REMOVAL]),
            (self.SOURCE, ['']),
        ):
            with self.subTest(removals=removals), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source, patch_row, manifest, descriptor = self.fixture(root, contents, removals)
                with patch.object(state, 'PACK', root), self.assertRaisesRegex(ValueError, 'exactly once'):
                    state.prepared_sfz(patch_row, manifest, descriptor)
                self.assertEqual(source.read_bytes(), contents)
                self.assertEqual(list(source.parent.iterdir()), [source])

    def test_corrupt_derivative_is_atomically_regenerated(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, patch_row, manifest, descriptor = self.fixture(root)
            with patch.object(state, 'PACK', root):
                derived = state.prepared_sfz(patch_row, manifest, descriptor)
                expected = derived.read_bytes()
                derived.write_bytes(b'corrupt')
                self.assertEqual(state.prepared_sfz(patch_row, manifest, descriptor), derived)
            self.assertEqual(derived.read_bytes(), expected)
            self.assertEqual(source.read_bytes(), self.SOURCE)
            self.assertFalse(list(source.parent.glob('*.part')))

    def test_failed_publication_preserves_previous_derivative(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, patch_row, manifest, descriptor = self.fixture(root)
            derived = source.with_name(source.stem + '.muz-sfizz.sfz')
            derived.write_bytes(b'previous derivative')
            with patch.object(state, 'PACK', root), patch.object(state.os, 'replace', side_effect=OSError('cannot publish')):
                with self.assertRaisesRegex(OSError, 'cannot publish'):
                    state.prepared_sfz(patch_row, manifest, descriptor)
            self.assertEqual(derived.read_bytes(), b'previous derivative')
            self.assertEqual(source.read_bytes(), self.SOURCE)
            self.assertFalse(list(source.parent.glob('*.part')))

    def test_temporary_verification_failure_does_not_publish(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, patch_row, manifest, descriptor = self.fixture(root)
            original_read = Path.read_bytes

            def read_bytes(path):
                return b'corrupt' if path.suffix == '.part' else original_read(path)

            with patch.object(state, 'PACK', root), patch.object(Path, 'read_bytes', read_bytes):
                with patch.object(state.os, 'replace') as replace, self.assertRaisesRegex(ValueError, 'verification failed'):
                    state.prepared_sfz(patch_row, manifest, descriptor)
                replace.assert_not_called()
            self.assertEqual(list(source.parent.iterdir()), [source])
            self.assertEqual(source.read_bytes(), self.SOURCE)

    def test_included_fragment_overlay_is_not_claimed_or_applied(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, patch_row, manifest, descriptor = self.fixture(root)
            descriptor['programs'][0]['source_overlays'][0]['path'] = '../Fragments/release.sfz'
            fragment = source.parent / '../Fragments/release.sfz'
            original = fragment.read_bytes()
            with patch.object(state, 'PACK', root):
                self.assertEqual(state.prepared_sfz(patch_row, manifest, descriptor), source)
            self.assertEqual(source.read_bytes(), self.SOURCE)
            self.assertEqual(fragment.read_bytes(), original)
            self.assertEqual(list(source.parent.iterdir()), [source])



if __name__ == '__main__':
    unittest.main()
