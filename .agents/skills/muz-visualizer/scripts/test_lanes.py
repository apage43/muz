"""Synthetic grouping and strike timing; no composition is a regression fixture."""

import unittest
from unittest.mock import patch

from render import build_lanes, group_performance, inspect_all, strike_envelope


def track(name, tick=100, duration=10, ppq=100):
    return {
        "track": name,
        "ppq": ppq,
        "tempos": [
            {"tick": 0, "micros_per_quarter": 500000},
            {"tick": ppq, "micros_per_quarter": 1000000},
        ],
        "notes": [
            {
                "start_tick": tick,
                "duration_ticks": duration,
                "key": 46,
                "attack_velocity": 80,
                "performance": {"velocity": 0.631},
            }
        ],
    }


class LaneTests(unittest.TestCase):
    def test_inspection_follows_pagination(self):
        pages = [
            b'{"view":"performance","rows":[{"n":1}],"total":2,"next":1}',
            b'{"view":"performance","rows":[{"n":2}],"total":2,"next":null}',
        ]
        with patch("render.subprocess.check_output", side_effect=pages) as command:
            self.assertEqual(inspect_all("muz", "piece.muz", "performance"),
                             [{"n": 1}, {"n": 2}])
        self.assertIn("1", command.call_args_list[1].args[0])

    def test_current_inspection_rows_keep_track_clocks_and_events(self):
        graph = {"tracks": [
            {"id": "a", "source": {"kind": "midi", "summary": {"ppq": 960}}},
            {"id": "b", "source": {"kind": "midi", "summary": {"ppq": 480}}},
        ]}
        rows = [
            {"track": "a", "stream": "notes", "event": {"key": 60}},
            {"track": "a", "stream": "controllers", "event": {"value": 1}},
            {"track": "b", "stream": "tempos", "event": {"tick": 0}},
        ]
        self.assertEqual(group_performance(rows, graph), [
            {"track": "a", "ppq": 960, "notes": [{"key": 60}], "tempos": []},
            {"track": "b", "ppq": 480, "notes": [], "tempos": [{"tick": 0}]},
        ])

    def test_legacy_style_keeps_note_duration_and_fractional_velocity(self):
        lane = build_lanes(
            [track("piano", tick=90, duration=20)], {"piano": {"name": "Grand"}}
        )[0]
        self.assertEqual(lane["name"], "Grand")
        self.assertAlmostEqual(lane["notes"][0, 0], 0.45)
        self.assertAlmostEqual(lane["notes"][0, 1], 0.6)
        self.assertAlmostEqual(lane["notes"][0, 3], 0.631)

    def test_mic_copies_are_excluded_and_choke_uses_its_own_clock(self):
        raw = [track("open"), track("closed", tick=300, ppq=200), track("room.open")]
        style = {
            "exclude": ["room.*"],
            "lanes": [
                {"name": "Hats", "kind": "percussion", "sources": ["open", "closed"]}
            ],
            "open": {"decay": 0.4, "choked_by": ["closed"]},
        }
        lanes = build_lanes(raw, style)
        self.assertEqual(len(lanes), 1)
        self.assertEqual(len(lanes[0]["notes"]), 2)
        self.assertAlmostEqual(lanes[0]["notes"][0, 0], 0.5)
        self.assertAlmostEqual(lanes[0]["notes"][0, 1], 1.0)

    def test_no_missing_or_double_counted_sources(self):
        raw = [track("kick"), track("room.kick")]
        with self.assertRaisesRegex(ValueError, "Unmapped"):
            build_lanes(raw, {"lanes": [{"sources": ["kick"]}]})
        with self.assertRaisesRegex(ValueError, "duplicated"):
            build_lanes(raw, {"lanes": [{"sources": ["kick"]}, {"sources": ["kick"]}]})

    def test_strikes_light_immediately_and_stop_at_choke(self):
        self.assertEqual(strike_envelope(-0.01, 0.2, 1, 0), 0)
        self.assertEqual(strike_envelope(0, 0.2, 1, 0.5), 1)
        self.assertGreater(strike_envelope(0.1, 0.2, 1, 0.6), 0)
        self.assertEqual(strike_envelope(0.5, 0.2, 1, 1), 0)


if __name__ == "__main__":
    unittest.main()
