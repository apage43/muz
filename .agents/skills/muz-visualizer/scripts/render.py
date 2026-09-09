# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=2,<3", "Pillow>=11,<13", "opencv-python-headless>=4.10,<6"]
# ///
"""Render instrument lanes from a fresh muz performance export and matching master."""

import argparse
import fnmatch
import hashlib
import json
import math
import os
import subprocess
import tempfile
import time
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont


def tempo_clock(ppq, tempos):
    """Integrate the exported tempo map, honoring the last event at equal ticks."""
    if ppq <= 0 or not tempos:
        raise ValueError("A positive PPQ and tempo map are required")
    ordered = sorted(tempos, key=lambda e: (e["tick"], e.get("source_order", 0)))
    ticks = np.array([e["tick"] for e in ordered])
    rates = np.array([e["micros_per_quarter"] / 1e6 / ppq for e in ordered])
    if ticks[0] != 0 or np.any(rates <= 0):
        raise ValueError("Tempo map must start at zero with positive tempos")
    secs = np.r_[0, np.cumsum(np.diff(ticks) * rates[:-1])]

    def seconds(t):
        i = max(0, np.searchsorted(ticks, t, side="right") - 1)
        return float(secs[i] + (t - ticks[i]) * rates[i])

    return seconds


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def build_lanes(raw, style):
    """Group performed sources explicitly; microphone copies can be excluded.

    Pitched lanes use key durations. Percussion uses illustrative strike decays,
    optionally capped by the next event in a named choke source. Neither changes
    the performed onset. Legacy flat ID-to-name/color styles still work.
    """
    palette = [
        "ecd4a0",
        "88cedc",
        "8ebad4",
        "a1b6e2",
        "bca6d4",
        "9298bd",
        "b7e8de",
        "b6d3a2",
        "e7b77e",
        "d39b79",
    ]
    source = {tr["track"]: tr for tr in raw}
    if len(source) != len(raw):
        raise ValueError("Duplicate physical track IDs")
    definitions = style.get("lanes")
    if definitions is None:
        definitions = [dict(style.get(name, {}), sources=[name]) for name in source]
    if not 1 <= len(definitions) <= 24:
        raise ValueError("The lane layout supports 1–24 visible lanes")
    excluded = {
        name
        for name in source
        if any(fnmatch.fnmatchcase(name, pat) for pat in style.get("exclude", []))
    }
    used = set()
    onsets = {}
    for name, tr in source.items():
        seconds = tempo_clock(tr["ppq"], tr["tempos"])
        onsets[name] = sorted(seconds(n["start_tick"]) for n in tr["notes"])
    lanes = []
    for i, definition in enumerate(definitions):
        ids = definition["sources"]
        if not ids or len(set(ids)) != len(ids):
            raise ValueError("A lane needs distinct source IDs")
        if any(name not in source or name in used or name in excluded for name in ids):
            raise ValueError(f"Unknown, duplicated or excluded lane source: {ids}")
        used.update(ids)
        kind = definition.get("kind", "pitched")
        if kind not in ("pitched", "percussion"):
            raise ValueError(f"Unknown lane kind: {kind}")
        voices, notes = [], []
        for j, name in enumerate(ids):
            voice = {
                "glyph": "hit",
                "decay": 0.2,
                "offset": 0,
                **definition.get("strike", {}),
                **style.get(name, {}),
            }
            if voice["decay"] <= 0 or not math.isfinite(voice["decay"]):
                raise ValueError("Strike decay must be positive and finite")
            if any(ch not in source for ch in voice.get("choked_by", [])):
                raise ValueError("Unknown choke source")
            voices.append(voice)
            tr = source[name]
            seconds = tempo_clock(tr["ppq"], tr["tempos"])
            chokes = np.array(
                sorted(t for ch in voice.get("choked_by", []) for t in onsets[ch])
            )
            for n in tr["notes"]:
                start = seconds(n["start_tick"])
                end = seconds(n["start_tick"] + n["duration_ticks"])
                if kind == "percussion":
                    end = start + voice["decay"] * 6
                    ix = np.searchsorted(chokes, start, side="right")
                    if ix < len(chokes):
                        end = min(end, chokes[ix])
                performed = n.get("performance", {})
                notes.append(
                    [
                        start,
                        end,
                        performed.get("pitch", n["key"]),
                        performed.get("velocity", n["attack_velocity"] / 127),
                        j,
                    ]
                )
        notes = np.array(sorted(notes), dtype=float).reshape(-1, 5)
        lo, hi = np.percentile(notes[:, 2], [0, 100]) if len(notes) else (48, 72)
        lanes.append(
            {
                "name": definition.get(
                    "name", ids[0].replace("-", " ").replace("_", " ").title()
                ),
                "color": np.array(
                    tuple(
                        bytes.fromhex(
                            definition.get("color", palette[i % len(palette)])
                        )
                    ),
                    dtype=float,
                ),
                "y": definition.get("y", np.linspace(96, 972, len(definitions))[i]),
                "pitch_height": definition.get("pitch_height", 30),
                "kind": kind,
                "notes": notes,
                "voices": voices,
                "sources": ids,
                "lo": lo,
                "span": max(hi - lo, 12),
            }
        )
    missing = set(source) - used - excluded
    if missing:
        raise ValueError(
            f"Unmapped sources (exclude explicitly if intentional): {sorted(missing)}"
        )
    return lanes


def strike_envelope(age, decay, end, now):
    """Immediate attack; no anticipatory light or glow beyond a choke."""
    return math.exp(-age / decay) if age >= 0 and now < end else 0.0


def glyph(im, kind, x, y, radius, color, width=1):
    x, y, r = int(x), int(y), max(2, int(radius))
    if kind in ("hat", "clap"):
        for dx in [0] if kind == "hat" else [-r // 2, r // 2]:
            cv2.line(
                im, (x - r + dx, y - r), (x + r + dx, y + r), color, width, cv2.LINE_AA
            )
            cv2.line(
                im, (x - r + dx, y + r), (x + r + dx, y - r), color, width, cv2.LINE_AA
            )
    elif kind in ("snare", "tom"):
        sides = 4 if kind == "snare" else 6
        angles = np.arange(sides) * 2 * np.pi / sides + np.pi / 2
        points = np.column_stack(
            [x + r * np.cos(angles), y + r * np.sin(angles)]
        ).astype(np.int32)
        cv2.polylines(im, [points], True, color, width, cv2.LINE_AA)
    elif kind == "crash":
        for a in np.arange(6) * np.pi / 3:
            cv2.line(
                im,
                (x, y),
                (int(x + r * np.cos(a)), int(y + r * np.sin(a))),
                color,
                width,
                cv2.LINE_AA,
            )
    elif kind == "ride":
        cv2.ellipse(
            im, (x, y), (r, max(2, r // 2)), -20, 0, 360, color, width, cv2.LINE_AA
        )
    else:
        cv2.circle(im, (x, y), r, color, -1 if kind == "kick" else width, cv2.LINE_AA)
        if kind == "open_hat":
            cv2.line(
                im, (x - r, y - r - 3), (x + r, y - r - 3), color, width, cv2.LINE_AA
            )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--source", type=Path, required=True)
    p.add_argument("--audio", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--work-dir", type=Path, required=True)
    p.add_argument("--muz", type=Path, default=Path("target/release/muz"))
    p.add_argument(
        "--style",
        type=Path,
        help="JSON track styles, or lanes with sources and explicit exclude patterns",
    )
    p.add_argument(
        "--font", type=Path, default=Path("/usr/share/fonts/noto/NotoSans-Light.ttf")
    )
    p.add_argument("--fps", type=int, choices=[30, 60], default=60)
    p.add_argument(
        "--encoder", choices=["auto", "h264_nvenc", "libx264"], default="auto"
    )
    p.add_argument(
        "--preview",
        type=float,
        nargs="+",
        help="Save frames at these seconds without encoding",
    )
    args = p.parse_args()
    cv2.setNumThreads(4)
    ROOT = args.work_dir.resolve()
    ROOT.mkdir(parents=True, exist_ok=True)
    MASTER = args.audio.resolve()
    DEST = args.output.expanduser().resolve()
    if DEST == MASTER:
        p.error("Output must differ from the audio source")
    W, H, FPS = 1920, 1080, args.fps
    DURATION = float(
        subprocess.check_output(
            [
                "ffprobe",
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
                str(MASTER),
            ]
        )
    )
    performance = subprocess.check_output(
        [str(args.muz), "inspect", str(args.source), "--view", "performance", "--json"]
    )
    (ROOT / "performance.json").write_bytes(performance)
    raw = json.loads(performance)
    style = json.loads(args.style.read_text()) if args.style else {}
    lanes = build_lanes(raw, style)
    names = [lane["name"] for lane in lanes]
    colors = [lane["color"] for lane in lanes]
    ys = [lane["y"] for lane in lanes]
    tracks = [(lane["notes"], lane["lo"], lane["span"]) for lane in lanes]
    font = ImageFont.truetype(str(args.font), min(23, int(700 / len(lanes))))
    while max(font.getlength(name) for name in names) > 220:
        font = ImageFont.truetype(str(args.font), font.size - 1)
    # Fine, intentionally low-contrast background. All text consists of instrument names.
    y, x = np.mgrid[:H, :W]
    radial = np.exp(-(((x - 1050) / 920) ** 2) - ((y - 490) / 760) ** 2)
    base = np.zeros((H, W, 3), np.float32)
    for k, (a, b) in enumerate([(5, 6), (9, 9), (17, 13)]):
        base[:, :, k] = a + b * radial
    rng = np.random.default_rng(812)
    base += rng.normal(0, 0.55, (H, W, 1))
    base = np.clip(base, 0, 255).astype(np.uint8)
    for i, yy in enumerate(ys):
        half_height = lanes[i]["pitch_height"] / 2 + 10
        cv2.line(
            base,
            (284, int(yy + half_height)),
            (1840, int(yy + half_height)),
            (18, 26, 37),
            1,
            cv2.LINE_AA,
        )
        for px in [606, 926, 1246, 1566, 1840]:
            cv2.line(
                base,
                (px, int(yy - half_height)),
                (px, int(yy + half_height)),
                (14, 21, 32),
                1,
                cv2.LINE_AA,
            )
    labels = Image.new("RGB", (W, H))
    d = ImageDraw.Draw(labels)
    for i, yy in enumerate(ys):
        d.text(
            (247, yy),
            names[i],
            font=font,
            fill=tuple((colors[i] * 0.78).astype(int)),
            anchor="rm",
        )
    labels = np.array(labels)
    stars = np.column_stack(
        [
            rng.uniform(275, 1855, 120),
            rng.uniform(48, 1025, 120),
            rng.uniform(0.15, 1, 120),
            rng.uniform(0, 6.28, 120),
        ]
    )
    PLAY = 650
    SPEED = 114

    def rgb(col, scale=1):
        return tuple(int(v) for v in np.clip(col * scale, 0, 255))

    def line(im, pts, color, width=1):
        cv2.polylines(im, [np.asarray(pts, np.int32)], False, color, width, cv2.LINE_AA)

    def frame(t):
        im = base.copy()
        strikes = np.zeros_like(im)
        light = np.zeros((H // 2, W // 2, 3), np.uint8)
        # Slow celestial dust gives the quiet passages a sense of space.
        for sx, sy, z, phase in stars:
            xx = int(sx + 5 * math.sin(t * 0.08 + phase))
            yy = int(sy + 3 * math.cos(t * 0.055 + phase))
            v = (0.3 + 0.2 * math.sin(t * 0.45 + phase)) * z
            cv2.circle(
                im,
                (xx, yy),
                1,
                (int(12 + 70 * v), int(19 + 105 * v), int(30 + 139 * v)),
                -1,
                cv2.LINE_AA,
            )
        ending = np.clip((t - DURATION * 0.86) / (DURATION * 0.09), 0, 1)
        cv2.line(im, (PLAY, 58), (PLAY, 1016), (69, 81, 96), 1, cv2.LINE_AA)
        cv2.circle(im, (PLAY, 54), 3, (187, 196, 208), -1, cv2.LINE_AA)
        cv2.circle(im, (PLAY, 1020), 3, (187, 196, 208), -1, cv2.LINE_AA)
        activities = []
        for i, ((notes, lo, span), yy, col) in enumerate(zip(tracks, ys, colors)):
            col = col * (1 - ending * 0.12) + np.array([171, 137, 217]) * ending * 0.12
            visible = notes[(notes[:, 0] < t + 10.6) & (notes[:, 1] > t - 3.2)]
            activity = 0.0
            lane = lanes[i]
            if lane["kind"] == "percussion":
                cv2.circle(im, (442, int(yy)), 25, rgb(col, 0.13), 1, cv2.LINE_AA)
            for j, (start, end, pitch, vel, voice_index) in enumerate(visible):
                if lane["kind"] == "percussion":
                    voice = lane["voices"][int(voice_index)]
                    y0 = yy + voice["offset"]
                    age = t - start
                    env = strike_envelope(age, voice["decay"], end, t)
                    activity = max(activity, env * vel)
                    x0 = PLAY + (start - t) * SPEED
                    if 292 <= x0 <= 1840:
                        fade = min(1, (1840 - x0) / 120)
                        strength = (
                            (0.22 + 0.3 * vel)
                            if age < 0
                            else (0.4 + 0.5 * vel) * max(0, 1 - age / 3.2)
                        )
                        glyph(
                            strikes,
                            voice["glyph"],
                            x0,
                            y0,
                            3 + 2 * vel,
                            rgb(col, strength * fade),
                        )
                    if env > 0.012:
                        strength = env * (0.35 + 0.65 * vel)
                        # A fixed strike face and an expanding ring respond at the
                        # performed onset. Unlike key lanes, there is no sustain bar.
                        glyph(
                            strikes,
                            voice["glyph"],
                            442,
                            y0,
                            10 + 7 * vel,
                            rgb(col, strength * 1.25),
                            2,
                        )
                        cv2.circle(
                            strikes,
                            (442, int(y0)),
                            int(18 + age * 72),
                            rgb(col, strength * 0.5),
                            1,
                            cv2.LINE_AA,
                        )
                        cv2.circle(
                            light,
                            (221, int(y0 / 2)),
                            int(10 + 6 * vel),
                            rgb(col, strength * 0.5),
                            -1,
                            cv2.LINE_AA,
                        )
                        glyph(
                            strikes,
                            voice["glyph"],
                            PLAY,
                            y0,
                            4 + 5 * vel,
                            rgb(col, strength * 1.5),
                            2,
                        )
                        cv2.circle(
                            light,
                            (PLAY // 2, int(y0 / 2)),
                            int(5 + 5 * vel),
                            rgb(col, strength * 0.7),
                            -1,
                            cv2.LINE_AA,
                        )
                    continue
                height = lane["pitch_height"]
                y0 = yy + height / 2 - (pitch - lo) / span * height
                age = t - start
                release = t - end
                active = age >= 0 and release < 0
                env = (
                    min(1, max(0, age) * 9) * math.exp(-max(0, release) * 2.8)
                    if age >= 0
                    else 0
                )
                activity = max(activity, env * vel)
                x1 = PLAY + (start - t) * SPEED
                x2 = PLAY + (end - t) * SPEED
                # Upcoming notes are polished, dim capsules; played notes dissolve to the left.
                left = max(292, x1)
                right = min(1840, x2)
                if right > left:
                    fade = min(1, max(0, (1840 - left) / 170))
                    bright = (
                        (0.40 + 0.28 * vel) * fade
                        if age < 0
                        else (0.56 + 0.40 * vel) * max(0.1, 1 - max(0, release) / 3.2)
                    )
                    if active:
                        bright = 0.86 + 0.3 * vel
                    cv2.line(
                        im,
                        (int(left), int(y0)),
                        (int(right), int(y0)),
                        rgb(col, bright),
                        3,
                        cv2.LINE_AA,
                    )
                    if active:
                        cv2.line(
                            light,
                            (int(max(left, PLAY) / 2), int(y0 / 2)),
                            (int(right / 2), int(y0 / 2)),
                            rgb(col, 0.52),
                            4,
                            cv2.LINE_AA,
                        )
                    cv2.circle(
                        im,
                        (int(left), int(y0)),
                        2,
                        rgb(col, bright * 1.18),
                        -1,
                        cv2.LINE_AA,
                    )
                if env > 0.012:
                    # A separate vibrating filament for each sounding pitch; released notes decay.
                    xs = np.linspace(300, PLAY, 95)
                    q = (PLAY - xs) / (PLAY - 300)
                    amp = (2.8 + 4.5 * vel) * env * np.sin(np.pi * q) ** 0.65
                    wave = np.sin(
                        q * (11 + pitch * 0.12) - t * (4 + pitch * 0.04)
                    ) + 0.27 * np.sin(q * 39 + t * 3)
                    yv = y0 + wave * amp
                    pts = np.column_stack([xs, yv])
                    line(im, pts, rgb(col, 0.62 * env), 1)
                    line(light, pts / 2, rgb(col, 0.32 * env), 2)
                    cv2.circle(
                        light,
                        (PLAY // 2, int(y0 / 2)),
                        int(5 + 5 * vel),
                        rgb(col, env * 0.9),
                        -1,
                        cv2.LINE_AA,
                    )
                    cv2.circle(
                        im,
                        (PLAY, int(y0)),
                        int(2 + vel * 2),
                        rgb(col, env * 1.3),
                        -1,
                        cv2.LINE_AA,
                    )
                    # Sparks depart precisely at note onset, never randomly trigger instruments.
                    if age < 1.7:
                        for k in range(3):
                            px = PLAY - age * (40 + k * 28)
                            py = y0 - math.sin(k * 2 + pitch) * age * 11
                            a = (1 - age / 1.7) * vel
                            cv2.circle(
                                im,
                                (int(px), int(py)),
                                1,
                                rgb(col, a * 1.4),
                                -1,
                                cv2.LINE_AA,
                            )
            activities.append(activity)
            cv2.circle(
                im, (270, int(yy)), 2, rgb(col, 0.22 + 0.8 * activity), -1, cv2.LINE_AA
            )
            if activity > 0.01:
                cv2.circle(
                    light,
                    (135, int(yy / 2)),
                    5,
                    rgb(col, activity * 0.6),
                    -1,
                    cv2.LINE_AA,
                )
        glow = cv2.GaussianBlur(light, (0, 0), 5)
        glow = cv2.resize(glow, (W, H), interpolation=cv2.INTER_LINEAR)
        im = cv2.add(cv2.add(im, strikes), glow)
        im = cv2.add(im, labels)
        # Restrained border arcs evoke a resonant hall without obstructing any part.
        energy = sum(activities) / len(lanes)
        xx = np.linspace(294, 1839, 260)
        for k in range(3):
            wav = np.sin(xx * 0.004 + t * 0.22 + k * 0.8) * (2 + energy * 9)
            line(
                im,
                np.column_stack([xx, 1038 + k * 5 + wav]),
                (25 + k * 3, 34 + k * 4, 46 + k * 5),
                1,
            )
        fade_in = max(0, float(style.get("fade_in", 1.4)))
        fade = min(1, t / fade_in if fade_in else 1, max(0, (DURATION - t) / 2.4))
        if fade < 1:
            im = (im * fade).astype(np.uint8)
        return im

    if args.preview:
        for t in args.preview:
            if not 0 <= t < DURATION:
                p.error(f"Preview time {t} is outside the audio duration")
            Image.fromarray(frame(t)).save(ROOT / f"preview-{t:g}.png")
        print(
            json.dumps(
                {
                    "duration": DURATION,
                    "tracks": len(raw),
                    "visible_lanes": len(lanes),
                    "notes": sum(len(t[0]) for t in tracks),
                }
            )
        )
        return
    encoder = args.encoder
    if encoder == "auto":
        check = subprocess.run(
            [
                "ffmpeg",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=s=64x64",
                "-frames:v",
                "1",
                "-c:v",
                "h264_nvenc",
                "-f",
                "null",
                "-",
            ],
            capture_output=True,
            check=False,
        )
        encoder = "h264_nvenc" if check.returncode == 0 else "libx264"
    enc = ["-c:v", encoder]
    enc += (
        ["-preset", "p7", "-tune", "hq", "-rc", "vbr", "-cq", "17", "-b:v", "0"]
        if encoder == "h264_nvenc"
        else ["-preset", "slow", "-crf", "17"]
    )
    DEST.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=f".{DEST.stem}-", suffix=".mp4", dir=DEST.parent)
    os.close(fd)
    proc = None
    try:
        cmd = [
            "ffmpeg",
            "-hide_banner",
            "-loglevel",
            "warning",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            "1920x1080",
            "-r",
            str(FPS),
            "-i",
            "-",
            "-i",
            str(MASTER),
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            *enc,
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "320k",
            "-ar",
            "48000",
            "-movflags",
            "+faststart",
            "-t",
            str(DURATION),
            tmp,
        ]
        proc = subprocess.Popen(cmd, stdin=subprocess.PIPE)
        started = time.monotonic()
        for f in range(math.ceil(DURATION * FPS)):
            proc.stdin.write(frame(f / FPS).tobytes())
            if f % (FPS * 10) == 0:
                print(
                    f"{f / FPS:.0f}/{DURATION:.1f}s; {f / max(0.1, time.monotonic() - started):.1f} fps",
                    flush=True,
                )
        proc.stdin.close()
        if proc.wait():
            raise RuntimeError("ffmpeg encoding failed")
        provenance = {
            "source": str(args.source.resolve()),
            "audio": str(MASTER),
            "audio_sha256": sha256(MASTER),
            "performance_sha256": sha256(ROOT / "performance.json"),
            "renderer_sha256": sha256(__file__),
            "style_sha256": sha256(args.style) if args.style else None,
            "lanes": [
                {
                    "name": lane["name"],
                    "kind": lane["kind"],
                    "sources": lane["sources"],
                    "notes": len(lane["notes"]),
                }
                for lane in lanes
            ],
            "output": str(DEST),
            "encoder": encoder,
            "tracks": len(raw),
            "visible_lanes": len(lanes),
            "notes": sum(len(t[0]) for t in tracks),
            "encoding": "completed",
        }
        os.replace(tmp, DEST)
        (ROOT / "verification.json").write_text(json.dumps(provenance, indent=2) + "\n")
        print(DEST, flush=True)
    finally:
        if proc is not None and proc.poll() is None:
            proc.terminate()
            proc.wait()
        Path(tmp).unlink(missing_ok=True)


if __name__ == "__main__":
    main()
