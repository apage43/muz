#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Regenerate the constant-gain sustain derivatives used by calibrated.muz.

Optional: install.py alone only fetches the upstream recordings. This script
reads the original zone tables through calibrated.muz and writes one mono-gain
float WAV per recording to assets/calibrated/{id}-{index}.wav, targeting a
mono body RMS of -24 dBFS measured over 0.4-2.8 s. A new note therefore never
amplifies an older note's release. Needs ffmpeg and ffprobe on PATH.

    contrib/vsco-2-ce/calibrate.py
    contrib/vsco-2-ce/calibrate.py --jobs 6

Re-running verifies existing derivatives against derivation.json and only
recomputes what changed. The pack's own modules are resolved through
MUZ_CONTRIB_DIR, which defaults to the contrib library holding this pack.
"""

import argparse
import concurrent.futures
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

PACK = Path(__file__).resolve().parent
OUTPUT = PACK / "assets/calibrated"
DERIVATION = OUTPUT / "derivation.json"
TARGET_DB = -24.0
BODY_WINDOW = (0.4, 2.8)


def muz_binary():
    override = os.environ.get("MUZ")
    if override:
        return override
    found = shutil.which("muz")
    if found:
        return found
    checkout = PACK.parent.parent
    candidate = checkout / "target/release/muz"
    if candidate.is_file():
        return str(candidate)
    raise SystemExit("cannot find the muz executable; set MUZ or put muz on PATH")


def run(*command, data=None):
    return subprocess.run(
        [str(part) for part in command],
        input=data,
        check=True,
        capture_output=True,
    ).stdout


def source_zones():
    """Ask the engine for the original zone tables behind calibrated.muz."""
    module = f"contrib/{PACK.name}/calibrated"
    probe = f'use "{module}" as c;\nc.sources\n'
    with tempfile.TemporaryDirectory(prefix="vsco-calibrate-") as directory:
        script = Path(directory) / "probe.muz"
        script.write_text(probe)
        environment = dict(os.environ, MUZ_CONTRIB_DIR=str(PACK.parent))
        result = subprocess.run(
            [muz_binary(), "eval", str(script)],
            cwd=directory,
            env=environment,
            capture_output=True,
            text=True,
        )
    if result.returncode != 0:
        raise SystemExit(f"muz could not read {module}:\n{result.stderr.strip()}")
    groups = json.loads(result.stdout)
    if not groups:
        raise SystemExit(f"{module} exposes no zone tables")
    return groups


def sha256(path):
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1 << 20):
            hasher.update(block)
    return hasher.hexdigest()


def derivative(instrument, index, zone):
    source = PACK / zone["path"]
    output = OUTPUT / f"{instrument}-{index}.wav"
    if not source.is_file():
        raise RuntimeError(f"{source} is missing; run install.py first")
    metadata = json.loads(
        run("ffprobe", "-v", "error", "-show_streams", "-of", "json", source)
    )["streams"][0]
    rate, channels = int(metadata["sample_rate"]), int(metadata["channels"])
    audio = np.frombuffer(
        run("ffmpeg", "-v", "error", "-i", source, "-f", "f32le", "-"), dtype="<f4"
    ).reshape(-1, channels)
    first, last = (round(edge * rate) for edge in BODY_WINDOW)
    body = audio[first : min(len(audio), last)].astype(np.float64).mean(axis=1)
    level = float(20 * np.log10(np.sqrt(np.mean(body * body))))
    gain_db = TARGET_DB - level
    scaled = audio * np.float32(10 ** (gain_db / 20))
    if not np.isfinite(scaled).all():
        raise RuntimeError(f"derived recording is not finite: {source}")
    peak = float(np.max(np.abs(scaled)))
    if peak >= 1.0:
        raise RuntimeError(f"derived recording needs headroom: {source}")
    run(
        "ffmpeg", "-y", "-v", "error", "-f", "f32le", "-ar", rate, "-ac", channels,
        "-i", "-", "-c:a", "pcm_f32le", output, data=scaled.astype("<f4").tobytes(),
    )
    # Verify the encoded derivative's data rather than assuming the encoder
    # preserved frame count, channels and the requested constant gain.
    decoded = np.frombuffer(
        run("ffmpeg", "-v", "error", "-i", output, "-f", "f32le", "-"), dtype="<f4"
    ).reshape(-1, channels)
    if decoded.shape != audio.shape or not np.array_equal(decoded, scaled):
        raise RuntimeError(f"encoder changed the audio written to {output}")
    return {
        "source": zone["path"],
        "output": str(output.relative_to(PACK)),
        "source_sha256": sha256(source),
        "output_sha256": sha256(output),
        "sample_rate": rate,
        "channels": channels,
        "frames": len(audio),
        "source_body_rms_dbfs": level,
        "constant_gain_db": gain_db,
        "target_body_rms_dbfs": TARGET_DB,
        "output_peak_dbfs": float(20 * np.log10(peak)),
    }


def already_derived(instrument, index, zone, previous):
    """Reuse a derivative whose source and gain already match the record."""
    record = previous.get(f"{instrument}-{index}.wav")
    if not record or record["source"] != zone["path"]:
        return None
    if record.get("target_body_rms_dbfs") != TARGET_DB:
        return None
    output = PACK / record["output"]
    source = PACK / zone["path"]
    if not output.is_file() or not source.is_file():
        return None
    if sha256(source) != record["source_sha256"]:
        return None
    if sha256(output) != record["output_sha256"]:
        return None
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--jobs", type=int, default=4, help="parallel jobs (default 4)")
    arguments = parser.parse_args()
    for tool in ("ffmpeg", "ffprobe"):
        if shutil.which(tool) is None:
            raise SystemExit(f"{tool} is required on PATH")
    previous = {}
    if DERIVATION.is_file():
        try:
            previous = {
                Path(record["output"]).name: record
                for record in json.loads(DERIVATION.read_text()).get("files", [])
            }
        except (json.JSONDecodeError, KeyError, TypeError):
            previous = {}

    groups = source_zones()
    jobs = [
        (group["id"], index, zone)
        for group in groups
        for index, zone in enumerate(group["zones"])
    ]
    OUTPUT.mkdir(parents=True, exist_ok=True)
    results = []
    reused = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=arguments.jobs) as pool:
        futures = {}
        for instrument, index, zone in jobs:
            cached = already_derived(instrument, index, zone, previous)
            if cached is not None:
                results.append(cached)
                reused += 1
                continue
            futures[
                pool.submit(derivative, instrument, index, zone)
            ] = instrument, index
        for future in concurrent.futures.as_completed(futures):
            instrument, index = futures[future]
            try:
                results.append(future.result())
            except subprocess.CalledProcessError as error:
                raise SystemExit(
                    f"{instrument}-{index}: {error.stderr.decode(errors='replace')}"
                ) from error
    order = {f"{i}-{n}.wav": p for p, (i, n, _) in enumerate(jobs)}
    results.sort(key=lambda record: order[Path(record["output"]).name])
    DERIVATION.write_text(
        json.dumps(
            {
                "method": "constant gain; mono body RMS 0.4-2.8s",
                "target_dbfs": TARGET_DB,
                "files": results,
            },
            indent=2,
        )
        + "\n"
    )
    print(
        f"{len(results)} derivatives in {OUTPUT.relative_to(PACK)} "
        f"({reused} verified, {len(results) - reused} written)"
    )


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
