"""Synthetic voice continuation and tuning; no piece is a regression fixture."""

import unittest

import numpy as np

from render import build_lanes, pitch_paths, tempo_clock


def note(start, duration, pitch, expression=()):
    return {
        "start_tick": start, "duration_ticks": duration,
        "key": int(pitch), "attack_velocity": 100,
        "performance": {"pitch": pitch, "expression": list(expression)},
    }


def track(notes):
    return {"track": "voice", "ppq": 100, "notes": notes,
        "tempos": [{"tick": 0, "micros_per_quarter": 500000}]}


def patch(mode="legato", glide=100):
    return {"track": "voice", "patch": {"voice_mode": mode},
        "controls": {"glide_ms": glide}}


def paths(notes, mode="legato", glide=100):
    tr = track(notes)
    return pitch_paths(tr, tempo_clock(tr["ppq"], tr["tempos"]), patch(mode, glide))


class PitchTests(unittest.TestCase):
    def test_overlap_moves_linearly_and_transfers_visual_ownership(self):
        old, new = paths([note(0, 200, 60), note(100, 100, 72)])
        self.assertEqual(old.end, 0.5)
        self.assertTrue(old.continued)
        self.assertEqual(new.voice_start, old.start)
        np.testing.assert_allclose(new.pitch([0.5, 0.55, 0.6, 0.8]), [60, 66, 72, 72])
        np.testing.assert_allclose(new.points()[:2], [[0.5, 60], [0.6, 72]])

    def test_interrupted_glide_starts_at_current_pitch(self):
        first, middle, last = paths([note(0, 200, 60), note(10, 200, 72), note(15, 100, 67)])
        self.assertAlmostEqual(last.initial, 63)
        self.assertAlmostEqual(last.pitch(0.125), 65)
        self.assertTrue(middle.continued)

    def test_released_owner_does_not_return_to_older_held_note(self):
        p = paths([note(0, 400, 60), note(100, 20, 72), note(140, 80, 67)])
        self.assertEqual(p[2].glide, 0)
        self.assertEqual(p[2].pitch(0.7), 67)

    def test_polyphonic_and_nonoverlapping_notes_do_not_glide(self):
        for mode in ("poly", "legato", "retrigger"):
            with self.subTest(mode=mode):
                p = paths([note(0, 100, 60), note(100, 100, 72)], mode)
                self.assertEqual(p[1].initial, 72)
        p = paths([note(0, 200, 60), note(100, 100, 72)], "poly")
        self.assertEqual(p[0].end, 1)
        self.assertFalse(p[0].continued)
        self.assertEqual(p[1].pitch(0.5), 72)

    def test_zero_glide_transfers_ownership_without_inventing_a_ramp(self):
        old, new = paths([note(0, 200, 60), note(100, 100, 72)], glide=0)
        self.assertTrue(old.continued)
        np.testing.assert_allclose(new.pitch([0.5, 0.6]), [72, 72])

    def test_retrigger_glides_but_restarts_visual_attack(self):
        old, new = paths([note(0, 200, 60), note(100, 100, 72)], "retrigger")
        self.assertEqual(new.voice_start, 0.5)
        self.assertAlmostEqual(new.pitch(0.55), 66)

    def test_note_tuning_does_not_leak_to_new_owner(self):
        old, new = paths([note(0, 200, 60, [{"kind": 2, "phase": 0, "value": 2}]),
            note(100, 100, 72)])
        self.assertEqual(old.pitch(0.4), 62)
        self.assertEqual(new.pitch(0.5), 60)

    def test_tuning_phase_uses_performed_gate_across_tempo_change(self):
        tr = track([note(90, 40, 62.25, [
            {"kind": 2, "phase": 0, "value": 0},
            {"kind": 2, "phase": 1, "value": 2}])])
        tr["tempos"].append({"tick": 100, "micros_per_quarter": 1000000})
        p = pitch_paths(tr, tempo_clock(tr["ppq"], tr["tempos"]))[0]
        self.assertAlmostEqual(p.pitch(0.625), 63.25)
        self.assertAlmostEqual(p.pitch(1.0), 64.25)

    def test_lane_sort_keeps_paths_with_their_notes_and_includes_bend_range(self):
        tr = track([note(100, 100, 65), note(0, 150, 60, [
            {"kind": 2, "phase": 0, "value": -3},
            {"kind": 2, "phase": 1, "value": 0}])])
        lane = build_lanes([tr], {}, [patch()])[0]
        self.assertEqual(lane["lo"], 57)
        self.assertEqual(lane["paths"][0].target, 60)
        self.assertEqual(lane["paths"][1].target, 65)
        self.assertEqual(lane["notes"][0, 1], 0.5)


if __name__ == "__main__":
    unittest.main()
