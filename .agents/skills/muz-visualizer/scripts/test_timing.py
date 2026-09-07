"""Synthetic tempo-boundary checks; no musical piece serves as a fixture."""

import unittest
from render import tempo_clock


class TempoClockTests(unittest.TestCase):
    def test_note_crosses_tempo_boundary(self):
        seconds = tempo_clock(
            960,
            [
                {"tick": 0, "micros_per_quarter": 500000},
                {"tick": 1920, "micros_per_quarter": 1000000},
            ],
        )
        self.assertAlmostEqual(seconds(1920), 1.0)
        self.assertAlmostEqual(seconds(2880) - seconds(960), 1.5)

    def test_last_simultaneous_tempo_wins(self):
        seconds = tempo_clock(
            100,
            [
                {"tick": 100, "micros_per_quarter": 1000000, "source_order": 2},
                {"tick": 0, "micros_per_quarter": 500000, "source_order": 0},
                {"tick": 100, "micros_per_quarter": 250000, "source_order": 1},
            ],
        )
        self.assertAlmostEqual(seconds(200), 1.5)

    def test_incomplete_map_is_rejected(self):
        with self.assertRaises(ValueError):
            tempo_clock(100, [{"tick": 100, "micros_per_quarter": 500000}])


if __name__ == "__main__":
    unittest.main()
