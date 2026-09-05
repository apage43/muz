"""Make constant-gain, level-matched MP3 excerpts for manual comparison.

The delivered masters remain unchanged. Uses ffmpeg only for excerpt/codec convenience,
and the public muz analyzer for loudness and true peak. This is not a mastering stage.
"""
import argparse
import json
import subprocess
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--excerpt", nargs=4, action="append", required=True, metavar=("NAME", "PATH", "START", "SECONDS"))
p.add_argument("--target-lufs", type=float, default=-18)
p.add_argument("--ceiling", type=float, default=-2)
p.add_argument("--muz", type=Path, default=Path("target/release/muz"))
p.add_argument("--output", type=Path, required=True)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
report = {}
for name, source, start, seconds in a.excerpt:
    excerpt = a.output / f"{name}-excerpt.wav"
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-ss", start, "-t", seconds, "-i", source, "-c:a", "pcm_f32le", str(excerpt)], check=True)
    measurements = json.loads(subprocess.check_output([str(a.muz), "analyze", str(excerpt), "--json"]))
    report[name] = dict(source=str(Path(source).resolve()), start=float(start), seconds=float(seconds), measurements=measurements)
# One common loudness keeps all comparisons fair while respecting the most dynamic excerpt.
level = min([a.target_lufs] + [v["measurements"]["integrated_lufs"] + a.ceiling - v["measurements"]["true_peak_dbtp"] for v in report.values()])
for name, entry in report.items():
    gain = level - entry["measurements"]["integrated_lufs"]
    entry.update(matched_lufs=level, gain_db=gain)
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(a.output / f"{name}-excerpt.wav"), "-af", f"volume={gain}dB", "-c:a", "libmp3lame", "-b:a", "320k", str(a.output / f"{name}-matched.mp3")], check=True)
(a.output / "comparison.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps({"matched_lufs": level, "excerpts": list(report)}, indent=2))
