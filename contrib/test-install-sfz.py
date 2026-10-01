#!/usr/bin/env python3
"""Synthetic integrity/path tests for complete SFZ installation."""
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import struct
import unittest

spec = importlib.util.spec_from_file_location('installer', Path(__file__).with_name('install-sfz.py'))
i = importlib.util.module_from_spec(spec)
spec.loader.exec_module(i)
spec_a = importlib.util.spec_from_file_location('audit', Path(__file__).with_name('audit-sfz.py'))
audit = importlib.util.module_from_spec(spec_a)
spec_a.loader.exec_module(audit)


class Integrity(unittest.TestCase):
    def test_paths_and_symlinks_cannot_escape(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)/'assets';root.mkdir()
            (root/'link').symlink_to(Path(t))
            for path in ('../sample.wav', '/sample.wav', 'link/sample.wav', '..\\sample.wav'):
                with self.assertRaises(ValueError): i.safe(root,path)

    def test_pin_checked_before_placement_and_existing_file_preserved(self):
        with tempfile.TemporaryDirectory() as t:
            root=Path(t);source=root/'source';source.mkdir();assets=root/'assets'
            (source/'a.sfz').write_bytes(b'<region>')
            row=dict(path='a.sfz',bytes=8,sha256=hashlib.sha256(b'<region>').hexdigest())
            i.place(row,assets,source,False)
            self.assertEqual((assets/'a.sfz').read_bytes(),b'<region>')
            (assets/'a.sfz').write_bytes(b'changed!')
            with self.assertRaises(ValueError):i.place(row,assets,source,False)
            self.assertEqual((assets/'a.sfz').read_bytes(),b'changed!')
            with self.assertRaises(ValueError):i.place(dict(row,path='missing.sfz'),assets,None,True)

    def test_git_blob_hash_includes_header(self):
        with tempfile.TemporaryDirectory() as t:
            p=Path(t)/'asset';p.write_bytes(b'abc')
            self.assertEqual(i.fingerprint(p,True),hashlib.sha1(b'blob 3\0abc').hexdigest())

    def test_container_inventory_frame_counts(self):
        with tempfile.TemporaryDirectory() as t:
            root=Path(t)
            fmt=struct.pack('<HHIIHH',1,2,48000,192000,4,16)
            chunks=b'fmt '+struct.pack('<I',len(fmt))+fmt+b'data'+struct.pack('<I',12)+bytes(12)
            p=root/'sample.wav';p.write_bytes(b'RIFF'+struct.pack('<I',len(chunks)+4)+b'WAVE'+chunks)
            self.assertEqual(audit.audio_metadata(p)['frames'],3)
            packed=(44100<<44)|(1<<41)|(15<<36)|1234
            stream=bytes(10)+packed.to_bytes(8,'big')+bytes(16)
            p=root/'sample.flac';p.write_bytes(b'fLaC'+bytes([128,0,0,34])+stream)
            self.assertEqual(audit.audio_metadata(p),dict(format='flac',sample_rate=44100,channels=2,bits_per_sample=16,frames=1234))


if __name__=='__main__':unittest.main()
