#!/usr/bin/env python3
"""Standard-library tests; installed reference execution is opt-in."""
import importlib.util
import math
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('sfz_reference', Path(__file__).with_name('sfz-reference.py'))
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)


class ReferenceTests(unittest.TestCase):
    def test_midi_vlq_boundaries(self):
        self.assertEqual(reference.vlq(0), b'\x00')
        self.assertEqual(reference.vlq(127), b'\x7f')
        self.assertEqual(reference.vlq(128), b'\x81\x00')
        self.assertEqual(reference.vlq(16383), b'\xff\x7f')

    def test_synthetic_metrics_are_calibrated(self):
        with tempfile.TemporaryDirectory() as directory:
            wav = Path(directory)/'tone.wav'
            reference.sample(wav)
            metrics = reference.metrics(wav)
            self.assertEqual(metrics['frames'], 48000)
            self.assertEqual(metrics['channels'], 1)
            self.assertAlmostEqual(metrics['peak'], 8000/32768, places=4)
            self.assertAlmostEqual(metrics['sustain_rms'], 8000/32768/math.sqrt(2), places=4)

    def test_comparison_reports_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            wav = Path(directory)/'tone.wav'
            reference.sample(wav)
            result = reference.compare(wav, wav)
            self.assertEqual(result['max_absolute_error'], 0)
            self.assertEqual(result['relative_rms_error'], 0)
            self.assertEqual(result['compared_frames'], 48000)

    def test_midi_contains_exact_event_delays(self):
        with tempfile.TemporaryDirectory() as directory:
            midi = Path(directory)/'notes.mid'
            reference.midi(midi)
            data = midi.read_bytes()
            self.assertEqual(data[:4], b'MThd')
            self.assertIn(b'\x78\x90\x3c\x64', data)
            self.assertIn(b'\x83\x60\x80\x3c\x40', data)
            self.assertIn(b'\x82\x68\xff\x2f\x00', data)


if __name__ == '__main__':
    unittest.main()
