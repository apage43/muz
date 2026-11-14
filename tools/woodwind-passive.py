# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "soundfile"]
# ///
"""Measure passive bore reflection and normalized input impedance.

    uv run tools/woodwind-passive.py examples/woodwind-prototypes/passive-cylinder.muz

Use a single sustained note on a passive patch with no driven reed or jet.
Send a short broadband pulse down the bore. Expose two raw voice-patch probes
at the same reference plane, `incident` for the pulse and `reflected` for the
returning pressure wave. Optionally expose `radiated` for the pressure at a
radiation output. The default track is "bore". Render long enough for the
return to die away and select an analysis band excited by the pulse.

The tool divides the return spectrum by the incident spectrum to obtain
pressure reflection R. It reports peaks and half-power widths of
Z/Z0 = (1+R)/(1-R), and |R| and returned energy as passivity screens. A finite
trace and excited frequency band give a screen for the held fingering, not
a proof of passivity under key motion. Reported peak widths use the -3 dB
crossings in |Z/Z0|, including the local spectral background. The result is
dimensionless until a characteristic impedance Z0 is supplied. Use --json
to save the measurements
and --radiated to include the radiation transfer in the same render.
"""

import argparse
import json
import math
import os
import subprocess
import tempfile
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parent.parent
MUZ = os.environ.get("MUZ", str(ROOT / "target/release/muz"))


def read_probe(directory, track, name, rate):
    samples, actual_rate = sf.read(directory / f"{track}@{name}.wav", always_2d=True)
    if actual_rate != rate or not np.isfinite(samples).all():
        raise ValueError(f"{name} probe has invalid samples or sample rate")
    if samples.shape[1] > 1 and not np.allclose(
        samples, samples[:, :1], rtol=1e-6, atol=1e-12
    ):
        raise ValueError(f"{name} must be a mono pressure probe")
    return samples[:, 0]


def calculate_transfer(incident, reflected, rate, limits):
    """Return a contiguous FFT band with adequate signal in the reference pulse."""
    if incident.size != reflected.size or incident.size < 3:
        raise ValueError("Probe renders must have the same duration")
    bins = 1 << (2 * incident.size - 1).bit_length()
    freq = np.fft.rfftfreq(bins, 1 / rate)
    band = slice(
        np.searchsorted(freq, limits[0]), np.searchsorted(freq, limits[1], side="right")
    )
    drive = np.fft.rfft(incident, bins)
    amplitude = np.abs(drive)
    reference = amplitude.max()
    if (
        freq[band].size < 3
        or reference == 0
        or np.any(amplitude[band] < reference * 1e-4)
    ):
        raise ValueError(
            "The incident pulse must excite the entire analysis band within 80 dB"
        )
    transfer = np.fft.rfft(reflected, bins)[band] / drive[band]
    return freq[band], transfer


def peak_width(hz, magnitude, i):
    """Interpolate the two crossings at 3 dB below an impedance peak."""
    half_power = magnitude[i] / math.sqrt(2)
    lower = np.flatnonzero(magnitude[:i] <= half_power)
    upper = np.flatnonzero(magnitude[i + 1 :] <= half_power)
    if not lower.size or not upper.size:
        return None
    left = lower[-1]
    right = i + 1 + upper[0]
    lo = np.interp(half_power, magnitude[left : left + 2], hz[left : left + 2])
    hi = np.interp(
        half_power,
        magnitude[right - 1 : right + 1][::-1],
        hz[right - 1 : right + 1][::-1],
    )
    return float(hi - lo)


def characterize(incident, reflected, rate, limits, radiated=None):
    hz, reflection = calculate_transfer(incident, reflected, rate, limits)
    # An exactly unit, zero-phase reflection yields a singular impedance.
    # Leave the ratio undefined there instead of implying a finite peak.
    denominator = 1 - reflection
    impedance = np.full(hz.shape, np.nan, dtype=complex)
    np.divide(
        1 + reflection, denominator, out=impedance, where=abs(denominator) > 1e-12
    )
    magnitude = abs(impedance)
    peaks = (
        np.flatnonzero(
            (magnitude[1:-1] > magnitude[:-2]) & (magnitude[1:-1] >= magnitude[2:])
        )
        + 1
    )
    resonances = []
    for i in peaks:
        row = {
            "hz": float(hz[i]),
            "normalized_impedance": float(magnitude[i]),
            "width_hz": peak_width(hz, magnitude, i),
        }
        if radiated is not None:
            row["radiation_transfer"] = float(abs(radiated[i]))
        resonances.append(row)

    peak = int(np.argmax(abs(reflection)))
    segment = max(1, round(0.05 * reflected.size))
    reflected_energy = float(np.dot(reflected, reflected))
    return {
        "sample_rate": rate,
        "fft_bin_hz": float(hz[1] - hz[0]),
        "band_hz": list(limits),
        "peak_reflection": float(abs(reflection[peak])),
        "peak_reflection_hz": float(hz[peak]),
        "reflected_energy_fraction": reflected_energy
        / float(np.dot(incident, incident)),
        "last_5_percent_energy_fraction": (
            float(np.dot(reflected[-segment:], reflected[-segment:])) / reflected_energy
            if reflected_energy
            else 0.0
        ),
        "resonances": resonances,
    }


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("source", type=Path)
    parser.add_argument("--track", default="bore")
    parser.add_argument("--rate", type=int, default=48_000)
    parser.add_argument("--seconds", type=float, default=0.5)
    parser.add_argument("--min-hz", type=float, default=50)
    parser.add_argument("--max-hz", type=float, default=5_000)
    parser.add_argument("--radiated", action="store_true")
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()
    if not 8_000 <= args.rate <= 192_000:
        parser.error("--rate must be between 8000 and 192000 Hz")
    if not 0.01 <= args.seconds <= 10:
        parser.error("--seconds must be between 0.01 and 10")
    if not 0 <= args.min_hz < args.max_hz < args.rate / 2:
        parser.error("analysis band must lie between zero and Nyquist")
    if not args.track or any(c in args.track for c in "@/\\"):
        parser.error("--track must be a single track ID")

    probes = (
        ("incident", "reflected", "radiated")
        if args.radiated
        else ("incident", "reflected")
    )
    limits = (args.min_hz, args.max_hz)
    with tempfile.TemporaryDirectory(prefix="muz-woodwind-passive-") as scratch:
        directory = Path(scratch)
        command = [
            MUZ,
            "render",
            str(args.source),
            "--output",
            str(directory),
            "--sample-rate",
            str(args.rate),
            "--seconds",
            str(args.seconds),
        ]
        for name in probes:
            command.extend(("--tap", f"{args.track}@{name}"))
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL)
        incident, reflected, *radiation = (
            read_probe(directory, args.track, name, args.rate) for name in probes
        )
        radiated = (
            calculate_transfer(incident, radiation[0], args.rate, limits)[1]
            if radiation
            else None
        )
        report = characterize(incident, reflected, args.rate, limits, radiated)

    print(
        f"Peak |R| {report['peak_reflection']:.4f} at {report['peak_reflection_hz']:.1f} Hz"
    )
    print(
        f"Returned energy {report['reflected_energy_fraction']:.2%}, "
        f"final 5% contains {report['last_5_percent_energy_fraction']:.2%} of return"
    )
    print(
        "  Peak Hz     |Z/Z0|     Width Hz"
        + ("       |radiation|" if args.radiated else "")
    )
    for row in report["resonances"]:
        width = "—" if row["width_hz"] is None else f"{row['width_hz']:.1f}"
        radiation = f"  {row['radiation_transfer']:16.4f}" if args.radiated else ""
        print(
            f"{row['hz']:9.1f}  {row['normalized_impedance']:10.2f}  {width:>11}{radiation}"
        )
    if args.json:
        args.json.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from None
