"""Subpixel motion through the actual rasterizer, independent of any piece."""

import contextlib
import io
import json
import tempfile
from pathlib import Path
from unittest.mock import patch
import unittest

import numpy as np
from PIL import Image, ImageFont

import render

from render import glyph


class MotionTests(unittest.TestCase):
    def test_constant_speed_does_not_snap_to_pixel_steps(self):
        # The score scrolls at 114px/s: 1.9px per frame at 60fps.
        for kind in ("hat", "snare", "ride", "kick"):
            with self.subTest(kind=kind):
                centers = []
                for frame in range(20):
                    image = np.zeros((48, 96, 3), dtype=np.uint8)
                    glyph(image, kind, 20.2 + frame * 1.9, 24.3, 4.6, (180, 180, 180))
                    weights = image[:, :, 0].sum(axis=0).astype(float)
                    centers.append(np.dot(weights, np.arange(96)) / weights.sum())
                error = np.max(np.abs(np.diff(centers) - 1.9))
                self.assertLess(error, 0.2, f"Uneven frame displacement: {error:.3f}px")

    def test_released_filament_fades_without_darkening_background(self):
        # Exercise the complete frame path, with only external score/probe I/O
        # substituted by a synthetic note. No font or instrument install needed.
        font = ImageFont.load_default(size=20)
        note = {"start_tick": 0, "duration_ticks": 20, "key": 64, "attack_velocity": 90}
        images = []
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            style = root / "style.json"
            style.write_text(
                json.dumps(
                    {
                        "fade_in": 0,
                        "lanes": [{"sources": ["piano"], "y": 100, "pitch_height": 30}],
                    }
                )
            )
            for notes in ([], [note]):
                raw = [
                    {
                        "track": "piano",
                        "ppq": 100,
                        "notes": notes,
                        "tempos": [
                            {"tick": 0, "micros_per_quarter": 500000},
                        ],
                    }
                ]
                argv = [
                    "render",
                    "--source",
                    "synthetic.muz",
                    "--audio",
                    "synthetic.wav",
                    "--output",
                    str(root / "unused.mp4"),
                    "--work-dir",
                    directory,
                    "--style",
                    str(style),
                    "--preview",
                    "1.5",
                ]
                with (
                    patch("sys.argv", argv),
                    patch.object(
                        render.subprocess,
                        "check_output",
                        side_effect=[
                            b"5",
                            json.dumps({"tracks": [{"id": "piano", "source": {
                                "kind": "midi", "summary": {"ppq": 100}}}]}).encode(),
                            json.dumps({"view": "performance", "rows": [
                                {"track": "piano", "stream": "notes", "event": n}
                                for n in notes
                            ] + [{"track": "piano", "stream": "tempos", "event": t}
                                 for t in raw[0]["tempos"]],
                                "total": len(notes) + 1, "next": None}).encode(),
                            b'{"view":"patches","rows":[],"total":0,"next":null}',
                        ],
                    ),
                    patch.object(render.ImageFont, "truetype", return_value=font),
                    contextlib.redirect_stdout(io.StringIO()),
                ):
                    render.main()
                with Image.open(root / "preview-1.5.png") as image:
                    images.append(np.array(image)[110:121, 320:400].astype(int))
        darker = np.count_nonzero(np.any(images[1] < images[0], axis=2))
        self.assertEqual(darker, 0, f"Released note painted {darker} dark pixels")


if __name__ == "__main__":
    unittest.main()
