"""Development-only envelope/spectrum comparison of post-insert stem taps.

--layer NAME PATH FADER_DB is repeatable. Faders are explicit because dry taps precede them.
No audio is authored, mixed, or mastered by this tool.
"""
import argparse
import json
import subprocess
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--layer", nargs=3, action="append", required=True, metavar=("NAME", "PATH", "FADER_DB"))
p.add_argument("--start", type=float, default=0)
p.add_argument("--seconds", type=float, default=5)
p.add_argument("--output", type=Path, required=True)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
plt.style.use("dark_background")
fig, axes = plt.subplots(2, 1, figsize=(13, 8), layout="constrained")
report = {}
rate = 48000
for name, path, gain in a.layer:
    raw = subprocess.check_output(["ffmpeg", "-v", "error", "-ss", str(a.start), "-t", str(a.seconds), "-i", path, "-f", "f32le", "-ar", str(rate), "-ac", "2", "-"])
    x = np.frombuffer(raw, dtype="<f4").reshape(-1, 2).astype(float) * 10 ** (float(gain) / 20)
    block = 240
    chunks = x[:len(x) // block * block].reshape(-1, block, 2)
    rms = np.sqrt(np.mean(chunks ** 2, axis=(1, 2)))
    axes[0].plot(np.arange(len(rms)) * block / rate, 20 * np.log10(np.maximum(rms, 1e-7)), label=name)
    fft_size = min(16384, 2 ** int(np.log2(len(x))))
    frames = np.lib.stride_tricks.sliding_window_view(x.mean(axis=1), fft_size)[::fft_size // 2]
    power = np.mean(np.abs(np.fft.rfft(frames * np.hanning(fft_size))) ** 2, axis=0)
    hz = np.fft.rfftfreq(fft_size, 1 / rate)
    axes[1].plot(hz, 10 * np.log10(np.maximum(power, 1e-15)), label=name)
    report[name] = {"fader_db": float(gain), "rms_dbfs": float(10 * np.log10(max(np.mean(x ** 2), 1e-15))),
        "peak_dbfs": float(20 * np.log10(max(np.abs(x).max(), 1e-15))),
        "correlation": float(np.corrcoef(x.T)[0, 1]) if np.any(x) else None}
axes[0].set(xlabel="Seconds into excerpt", ylabel="5 ms RMS (dBFS)", ylim=(-65, 0), title="Envelopes at stated output faders")
axes[1].set(xlabel="Frequency (Hz)", ylabel="Windowed spectral power (relative dB)", xscale="log", xlim=(25, 16000))
for ax in axes:
    ax.legend(loc="upper right")
    ax.grid(alpha=.15)
fig.savefig(a.output / "layers.png", dpi=120)
(a.output / "layers.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
