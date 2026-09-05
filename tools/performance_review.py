"""Plot a muz performance inspection's key-down intervals and sustain controls.

Feed `muz inspect SONG --view performance --section NAME --json` to this developer aid.
Piano resonance under pedal is deliberately not drawn as a finger-held note.
"""
import argparse
import json
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.collections import PatchCollection
from matplotlib.patches import Rectangle
import numpy as np

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("inspection", type=Path)
p.add_argument("--track", default="piano")
p.add_argument("--output", type=Path, required=True)
a = p.parse_args()
t = next(t for t in json.loads(a.inspection.read_text()) if t["track"] == a.track)
notes = t["notes"]
ppq = t["ppq"]
start = min(n["start_tick"] for n in notes) / ppq
end = max(n["start_tick"] + n["duration_ticks"] for n in notes) / ppq
plt.style.use("dark_background")
fig, (keys, pedal) = plt.subplots(2, 1, figsize=(14, 6), layout="constrained", sharex=True, height_ratios=[4, 1])
rectangles, velocities = [], []
for n in notes:
    x = n["start_tick"] / ppq - start
    duration = n["duration_ticks"] / ppq
    key = n.get("performance", {}).get("pitch", n["key"])
    rectangles.append(Rectangle((x, key - .38), duration, .76))
    velocities.append(n.get("performance", {}).get("velocity", n["attack_velocity"] / 127))
    if duration > .5 and "finger" in n.get("annotations", {}):
        keys.text(x + .08, key, str(n["annotations"]["finger"]), fontsize=7, color="white", va="center")
pc = PatchCollection(rectangles, cmap="viridis", edgecolors="none")
pc.set_array(np.array(velocities)); pc.set_clim(0, 1); keys.add_collection(pc)
keys.set(ylim=(min(n["key"] for n in notes) - 2, max(n["key"] for n in notes) + 2), ylabel="MIDI pitch", title=f'{a.track}: performed key holds (labels = allocated fingers)')
fig.colorbar(pc, ax=keys, label="Attack intensity", fraction=.025, pad=.01)
controls = sorted((c["tick"] / ppq - start, c["value"] / 127) for c in t["controllers"] if c["controller"] == 64)
if controls:
    xs, ys = zip(*controls)
    pedal.step([*xs, end - start], [*ys, ys[-1]], where="post", color="#ffc174")
pedal.set(ylim=(-.05, 1.05), xlim=(0, end - start + .5), xlabel="Quarter-note beats from section start", ylabel="Pedal CC64")
for ax in (keys, pedal):
    for beat in np.arange(0, end - start, 4): ax.axvline(beat, color="white", alpha=.14, lw=.7)
    ax.grid(axis="y", alpha=.08)
a.output.parent.mkdir(parents=True, exist_ok=True)
fig.savefig(a.output, dpi=120)
