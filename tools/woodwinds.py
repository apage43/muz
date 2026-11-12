# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "soundfile"]
# ///
"""Measure, calibrate and compare modeled woodwinds against recorded ones.

Nobody can listen to every note of a model: this tool renders a std/woodwinds
instrument (or any instrument expression) note by note across its range and
measures what a listener would hear, then sets it beside recorded winds.

    uv run tools/woodwinds.py reference                 # measure the recordings once
    uv run tools/woodwinds.py check flute               # the model across its range and dynamics
    uv run tools/woodwinds.py compare flute             # beside the recordings, by register
    uv run tools/woodwinds.py summary                   # one line per instrument, all five
    uv run tools/woodwinds.py calibrate flute           # thresholds and tuning -> std tables

A derived instrument is a spec in your own file. Calibrate it and compare it with
its recordings (any folder of single-note WAV files):

    uv run tools/woodwinds.py calibrate alto_flute --use my/winds.muz:my \\
        --spec my.alto_flute --cal-out my/alto_flute_cal.muz
    uv run tools/woodwinds.py compare alto_flute --use my/winds.muz:my \\
        --spec my.alto_flute --recordings "path/to/Alto Flute"

`--spec EXPR` names a spec record; the tool plays `w.patch(SPEC, overrides)` from
std/woodwinds, so it can swap the calibration and silence the breath noise when
it needs to. `--model EXPR` plays any instrument expression instead (check,
compare, floor and summary only); `--drive pressure` then writes the dynamic as a
`pressure` curve rather than as velocity.

Measurements, per note (the same code measures recordings and models):
  cents     pitch error against equal temperament
  rms       level of the steady part, dBFS
  H2..H8    partials relative to the fundamental, dB
  cen/f0    spectral centroid over the fundamental: a brightness that does not
            depend on the note; errors are reported in octaves (log2 of ratios)
  floor     the level between partials in six bands, dB re H1 in a 100 Hz band:
            the breath; `slope` is 8-16 kHz less 1-2 kHz
  rise      10-90 % of the steady level, ms; `release` 70 % to 10 % after the
            note ends, ms
  vibrato   rate (Hz), pitch depth (cents, peak) and level depth (dB, peak)
  alias     energy away from the harmonics below 16 kHz with the breath noise
            off, dB re total (top of the range, ff): what folds back

Environment: MUZ (engine binary, default target/release/muz in this checkout),
MUZ_CONTRIB (the contrib folder holding sonatina/ and vsco-2-ce/ assets),
WOODWINDS_WORK (scratch folder for renders, default out/woodwinds/work).
"""
import argparse
import json
import math
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parent.parent
MUZ = os.environ.get("MUZ", str(ROOT / "target/release/muz"))
OUT = ROOT / "out" / "woodwinds"
WORK = Path(os.environ.get("WOODWINDS_WORK", OUT / "work"))
REFERENCE = OUT / "reference.json"
CAL_JSON = OUT / "calibration"
STD_TABLES = ROOT / "std" / "woodwind_calibration.muz"

# Each std instrument: range, and how its calibration measures a threshold.
#   reed   a falling breath: the pressure at which the tone dies (a single reed)
#   cone   a rising breath: where the first register speaks (a double reed)
#   flue   no threshold (the jet speaks from any breath); tuning only
INSTRUMENTS = {
    "flute": {"range": (60, 96), "threshold": "flue"},
    "oboe": {"range": (58, 91), "threshold": "cone"},
    "clarinet": {"range": (50, 91), "threshold": "reed"},
    "bass_clarinet": {"range": (34, 79), "threshold": "reed"},
    "bassoon": {"range": (34, 75), "threshold": "cone"},
}
ORDER = ["flute", "oboe", "clarinet", "bass_clarinet", "bassoon"]


def contrib():
    for c in [os.environ.get("MUZ_CONTRIB"), ROOT / "contrib", ROOT.parent.parent.parent / "contrib"]:
        if c and (Path(c) / "sonatina" / "assets").exists():
            return Path(c)
    raise SystemExit("recordings not found: set MUZ_CONTRIB to a contrib folder with sonatina/assets and vsco-2-ce/assets")


def sources():
    c = contrib()
    sso = c / "sonatina/assets/sso/Sonatina Symphonic Orchestra/Samples"
    vsco = c / "vsco-2-ce/assets/vsco/Woodwinds"
    return {
        "flute": [("sso", sso / "Flute"), ("vsco", vsco / "Flute/susvib")],
        "oboe": [("sso", sso / "Oboe"), ("vsco", vsco / "Oboe/Vib")],
        "clarinet": [("sso", sso / "Clarinet"), ("vsco", vsco / "Clarinet/susLong")],
        "bass_clarinet": [("sso", sso / "Bass Clarinet")],
        "bassoon": [("sso", sso / "Bassoon"), ("vsco", vsco / "Bassoon/sus")],
        "alto_flute": [("sso", sso / "Alto Flute")],
        "piccolo": [("sso", sso / "Piccolo")],
        "english_horn": [("sso", sso / "Cor Anglais")],
        "contrabassoon": [("sso", sso / "Contrabassoon")],
    }


# --- Signal measurements ----------------------------------------------------------


def hz(m):
    return 440.0 * 2 ** ((m - 69) / 12)


def db(x):
    return 20 * np.log10(max(float(x), 1e-12))


def f0_autocorr(x, sr, lo=40.0, hi=4500.0):
    x = x - x.mean()
    n = len(x)
    if n < 64:
        return 0.0
    spec = np.fft.rfft(x * np.hanning(n), 2 * n)
    ac = np.fft.irfft(np.abs(spec) ** 2)[:n]
    ac /= ac[0] + 1e-20
    lag_lo, lag_hi = int(sr / hi), min(int(sr / lo), n - 2)
    seg = ac[lag_lo:lag_hi]
    if len(seg) < 3:
        return 0.0
    best = seg.max()
    peaks = [i for i in range(1, len(seg) - 1) if seg[i] > seg[i - 1] and seg[i] >= seg[i + 1] and seg[i] > 0.9 * best]
    if not peaks:
        return 0.0
    i = peaks[0] + lag_lo
    a, b, c = ac[i - 1], ac[i], ac[i + 1]
    return sr / (i + 0.5 * (a - c) / (a - 2 * b + c + 1e-20))


def refine_f0(x, sr, f0, harmonics=6):
    n = len(x)
    pad = 8 * n
    spec = np.abs(np.fft.rfft(x * np.hanning(n), pad))
    est, wts = [], []
    for k in range(1, harmonics + 1):
        b = k * f0 * pad / sr
        lo, hi = int(b * 0.97), int(b * 1.03) + 2
        if k * f0 > sr / 2 - 200 or hi >= len(spec):
            break
        i = lo + int(np.argmax(spec[lo:hi]))
        if 0 < i < len(spec) - 1:
            a, bb, c = (np.log(spec[j] + 1e-20) for j in (i - 1, i, i + 1))
            est.append((i + 0.5 * (a - c) / (a - 2 * bb + c + 1e-20)) * sr / pad / k)
            wts.append(spec[i])
    return float(np.average(est, weights=wts)) if est else f0


BANDS = [(100, 500), (500, 1000), (1000, 2000), (2000, 4000), (4000, 8000), (8000, 16000)]


def spectrum(x, sr, f0, count=12):
    """Partials (amplitudes), centroid, the floor between partials by band, and the
    share of energy away from the harmonics (dB re total)."""
    n = len(x)
    w = np.hanning(n)
    pad = 4 * n
    spec = np.abs(np.fft.rfft(x * w, pad)) / (w.sum() / 2)
    freqs = np.arange(len(spec)) * sr / pad
    ps = []
    for k in range(1, count + 1):
        if k * f0 > sr / 2 - 100:
            ps.append(None)
            continue
        b = k * f0 * pad / sr
        half = 0.09 * f0 * pad / sr
        ps.append(float(spec[max(int(b - half) - 2, 0):int(b + half) + 3].max()))
    power = spec ** 2
    sel = freqs < 16000
    centroid = float((freqs[sel] * power[sel]).sum() / (power[sel].sum() + 1e-30))
    h1 = ps[0] or 1e-12
    dist = np.abs(freqs / f0 - np.round(freqs / f0)) * f0
    floor = []
    for lo, hi in BANDS:
        m = (freqs >= lo) & (freqs < hi) & (dist > 0.3 * f0) & (freqs > 0.5 * f0)
        if m.sum() < 4 or hi > sr / 2:
            floor.append(None)
            continue
        s2 = float(np.median(power[m])) * (w.sum() / 2) ** 2 / (w ** 2).sum()
        floor.append(round(db(np.sqrt(s2 * 100.0 / (sr / 2)) / (h1 / np.sqrt(2))), 1))
    away = (dist > 0.1 * f0) & (freqs > 0.5 * f0) & (freqs < min(16000, sr / 2 - 100))
    alias = db(np.sqrt(power[away].sum() / (power.sum() + 1e-30)))
    return ps, centroid, floor, alias


def envelope(x, sr, win_ms=10.0):
    w = max(1, int(sr * win_ms / 1000))
    return np.sqrt(np.convolve(x ** 2, np.ones(w) / w, mode="same"))


def track_pitch(x, sr, f0, hop=0.02, win=0.06):
    n, h = int(win * sr), int(hop * sr)
    cents, level = [], []
    for a in range(0, len(x) - n, h):
        seg = x[a:a + n]
        f = f0_autocorr(seg, sr, lo=f0 * 0.8, hi=f0 * 1.25)
        cents.append(1200 * math.log2(f / f0) if f > 0 else np.nan)
        level.append(db(np.sqrt(np.mean(seg ** 2))))
    return np.array(cents), np.array(level)


def vibrato(x, sr, f0, hop=0.02):
    cents, level = track_pitch(x, sr, f0, hop)
    ok = np.isfinite(cents)
    if ok.sum() < 30:
        return None
    c = cents[ok] - np.nanmean(cents[ok])
    lv = level[ok] - level[ok].mean()
    w = np.hanning(len(c))
    f = np.fft.rfftfreq(2048, hop)
    band = (f >= 3.5) & (f <= 8)
    sp = np.abs(np.fft.rfft(c * w, 2048))
    k = np.argmax(sp[band])
    spl = np.abs(np.fft.rfft(lv * w, 2048))
    return {"rate": round(float(f[band][k]), 2), "cents": round(float(sp[band][k] / (w.sum() / 2)), 1),
            "db": round(float(spl[band].max() / (w.sum() / 2)), 2), "wander": round(float(np.std(c)), 1)}


def measure(x, sr, start, end, expect=None, release_at=None, want_vibrato=False):
    """One note: x[start:end] holds it; `release_at` (s) is where it was let go."""
    a, b = int(start * sr), int(end * sr)
    seg = x[a:b]
    if len(seg) < sr // 20:
        return None
    env = envelope(x, sr)
    dur = (b - a) / sr
    s0 = a + int(min(0.35, dur * 0.3) * sr) if expect is None else a + int(dur * 0.45 * sr)
    s1 = min(s0 + int(0.8 * sr), a + int(dur * (0.8 if expect is None else 0.95) * sr))
    steady = x[s0:s1][:16384]
    if len(steady) < 1024:
        return None
    lo, hi = (45, 2600) if expect is None else (hz(expect) * 0.7, hz(expect) * 1.4)
    f0 = f0_autocorr(steady[:8192], sr, lo, hi)
    r = {"rms": round(db(np.sqrt(np.mean(steady ** 2))), 1)}
    if f0 <= 0:
        return r
    f0 = refine_f0(steady, sr, f0)
    midi = 69 + 12 * math.log2(f0 / 440)
    ref = expect if expect is not None else round(midi)
    ps, cen, floor, alias = spectrum(steady, sr, f0)
    h1 = ps[0] or 1e-12
    level = np.sqrt(np.mean(steady ** 2))
    r.update({
        "f0": round(f0, 2), "midi": round(midi), "cents": round(1200 * math.log2(f0 / hz(ref)), 1),
        "H": [None if p is None else round(db(p / h1), 1) for p in ps],
        "centroid": round(cen, 1), "cen_f0": round(cen / f0, 2), "floor": floor, "alias": round(alias, 1),
    })
    pre = env[max(a - int(0.05 * sr), 0):s0]
    t10, t90 = np.nonzero(pre > 0.1 * level)[0], np.nonzero(pre > 0.9 * level)[0]
    r["rise_ms"] = round((t90[0] - t10[0]) / sr * 1000, 1) if len(t10) and len(t90) else None
    tail_from = int(release_at * sr) if release_at is not None else s1
    tail = env[tail_from:tail_from + int(1.5 * sr)]
    above = np.nonzero(tail > 0.7 * level)[0]
    t0 = above[-1] if len(above) and release_at is None else (above[0] if len(above) else 0)
    below = np.nonzero(tail[t0:] < 0.1 * level)[0]
    r["release_ms"] = round(below[0] / sr * 1000, 1) if len(below) else None
    if want_vibrato and dur > 1.2:
        r["vibrato"] = vibrato(x[s0:a + int(dur * 0.85 * sr)], sr, f0)
    return r


# --- Recordings --------------------------------------------------------------------


def dynamic_of(lib, name):
    if lib == "vsco":
        return "p" if "_v1" in name else "f"
    return "mf"


def measure_recording(path, lib):
    x, sr = sf.read(path)
    if x.ndim > 1:
        x = x.mean(axis=1)
    env = envelope(x, sr)
    if env.max() < 1e-4:
        return None
    on = np.nonzero(env > 0.1 * env.max())[0]
    r = measure(x, sr, on[0] / sr, on[-1] / sr, want_vibrato=True)
    if not r or "f0" not in r:
        return None
    r.update({"file": path.name, "lib": lib, "dynamic": dynamic_of(lib, path.name)})
    # The recording's own floor: the room and the microphones before the note
    # (or after it), in the same units. Breath can only be judged above it.
    quiet = x[:max(on[0] - int(0.03 * sr), 0)]
    if len(quiet) < 4096:
        quiet = x[on[-1] + int(1.0 * sr):]
    if len(quiet) >= 4096:
        r["room"] = room_floor(quiet[-16384:], sr, r["f0"], r["H"], x, on, r)
    return r


def room_floor(quiet, sr, f0, H, x, on, r):
    n = len(quiet)
    w = np.hanning(n)
    power = (np.abs(np.fft.rfft(quiet * w)) / (w.sum() / 2)) ** 2
    freqs = np.fft.rfftfreq(n, 1 / sr)
    # H1's amplitude, as `spectrum` measured it, from the level of the steady part.
    h1 = 10 ** (r["rms"] / 20) * np.sqrt(2) / np.sqrt(sum(10 ** (h / 10) for h in H if h is not None))
    out = []
    for lo, hi in BANDS:
        m = (freqs >= lo) & (freqs < hi)
        s2 = float(np.median(power[m])) * (w.sum() / 2) ** 2 / (w ** 2).sum()
        out.append(round(db(np.sqrt(s2 * 100.0 / (sr / 2)) / (h1 / np.sqrt(2))), 1))
    return out


def cmd_reference(args):
    out = {}
    for inst, dirs in sources().items():
        rows = []
        for lib, d in dirs:
            for p in sorted(d.glob("*.wav")):
                r = measure_recording(p, lib)
                if r:
                    rows.append(r)
        for lib, _ in dirs:
            # A library folder with one layer is its instrument's plain dynamic.
            mine = [r for r in rows if r["lib"] == lib]
            if lib == "vsco" and mine and all(r["dynamic"] == "p" for r in mine):
                for r in mine:
                    r["dynamic"] = "mf"
        rows.sort(key=lambda r: (r["midi"], r["lib"], r["file"]))
        out[inst] = rows
        print(f"{inst}: {len(rows)} recordings")
    REFERENCE.parent.mkdir(parents=True, exist_ok=True)
    REFERENCE.write_text(json.dumps(out, indent=1))
    print(f"wrote {REFERENCE}")


def plausible(r):
    # A recording whose 'second partial' towers over the first was read an octave
    # low (some library file names are an octave off): leave it out.
    return r.get("H") and max(h for h in r["H"][1:3] if h is not None) < 15


def reference(name, args):
    if args.recordings:
        rows = [r for p in sorted(Path(args.recordings).glob("*.wav")) if (r := measure_recording(p, "user")) and plausible(r)]
        return sorted(rows, key=lambda r: r["midi"])
    if not REFERENCE.exists():
        raise SystemExit("no reference measurements yet: run `uv run tools/woodwinds.py reference`")
    data = json.loads(REFERENCE.read_text())
    if name not in data:
        raise SystemExit(f"no recordings of {name}: pass --recordings DIR")
    return [r for r in data[name] if plausible(r)]


# --- Rendering a model --------------------------------------------------------------


class Model:
    def __init__(self, name, args):
        self.name = name
        self.args = args
        spec = args.spec or (f"w.specs.{name}" if name in INSTRUMENTS else None)
        if args.model is None and spec is None:
            raise SystemExit(f"{name} is not a std instrument: pass --spec EXPR or --model EXPR")
        self.spec = None if args.model else spec
        lo, hi = INSTRUMENTS.get(name, {}).get("range", (None, None))
        if args.range:
            lo, hi = (int(v) for v in args.range.split(","))
        if lo is None:
            lo, hi = self.eval_range()
        self.range = (lo, hi)
        self.drive = args.drive or ("velocity" if self.spec else "pressure")

    def eval_range(self):
        r = self.eval(f"{self.spec}.range")
        return int(r[0]), int(r[1])

    def header(self):
        lines = [f'use "{os.environ.get("WOODWINDS_MODULE", "std/woodwinds")}" as w;']
        for u in self.args.use or []:
            path, alias = u.rsplit(":", 1)
            lines.append(f'use "{path if path.startswith("std/") else Path(path).resolve()}" as {alias};')
        return "\n".join(lines) + "\n"

    def eval(self, expr):
        WORK.mkdir(parents=True, exist_ok=True)
        src = WORK / f"_{self.name}_eval.muz"
        src.write_text(self.header())
        p = subprocess.run([MUZ, "eval", str(src), "--expr", expr], capture_output=True, text=True)
        if p.returncode:
            sys.stderr.write(p.stdout + p.stderr)
            raise SystemExit(1)
        return json.loads(p.stdout)

    def instrument(self, overrides="{}"):
        if self.spec:
            return f"w.patch({self.spec}, merge({overrides}, {self.args.extra or '{}'}))"
        return self.args.model

    def note(self, m, beats, gate, dynamic, express="{}"):
        if self.drive == "velocity":
            return f"note({m}, {beats}b, velocity={dynamic}).gate({gate}).express({express})"
        return f"note({m}, {beats}b, velocity=0.001).gate({gate}).express(merge({{pressure: {dynamic}}}, {express}))"

    def render(self, notes, overrides="{}", tag="model"):
        WORK.mkdir(parents=True, exist_ok=True)
        src = WORK / f"_{self.name}_{tag}.muz"
        wav = WORK / f"{self.name}_{tag}.wav"
        src.write_text(self.header() + f"let inst = {self.instrument(overrides)};\n"
                       f'song({{tempo: 60, tracks: [track("p", seq([{", ".join(notes)}]), inst)], tail: 1}})\n')
        p = subprocess.run([MUZ, "render", str(src), "-o", str(wav)], capture_output=True, text=True)
        if p.returncode:
            sys.stderr.write(p.stdout + p.stderr)
            raise SystemExit(1)
        x, sr = sf.read(wav)
        return (x.mean(axis=1) if x.ndim > 1 else x), sr

    def notes(self, pitches, dynamic, overrides="{}", beats=2, gate=0.75, tag="model"):
        """Each pitch held `gate` of `beats` seconds, measured."""
        x, sr = self.render([self.note(m, beats, gate, dynamic) for m in pitches], overrides, tag)
        hold = beats * gate
        return [measure(x, sr, k * beats, k * beats + hold, m, release_at=k * beats + hold) for k, m in enumerate(pitches)]

    def pitches(self, step=3):
        lo, hi = self.range
        ps = list(range(lo, hi + 1, step))
        return ps if ps[-1] == hi else ps + [hi]


QUIET = "{noise: 0, hiss: 0, finger: 0, drift_cents: 0}"


def fmt(v, w, d=1):
    return f"{'-':>{w}}" if v is None else f"{v:>{w}.{d}f}"


def cmd_check(args):
    model = Model(args.name, args)
    dyns = [float(v) for v in (args.dynamics or "0.2,0.5,0.8,1").split(",")]
    for d in dyns:
        rows = model.notes(model.pitches(args.step), d)
        print(f"--- {args.name} at dynamic {d}")
        print(f"{'midi':>4} {'cents':>6} {'rms':>6} " + " ".join(f"{'H' + str(k):>5}" for k in range(2, 9))
              + f" {'cen/f0':>6} " + " ".join(f"{lo // 1000 if lo >= 1000 else lo / 1000:>4}k" for lo, _ in BANDS) + f" {'rise':>5} {'rel':>5}")
        for m, r in zip(model.pitches(args.step), rows):
            if not r or "f0" not in r:
                print(f"{m:>4}  silent")
                continue
            print(f"{m:>4} {fmt(r['cents'], 6)} {fmt(r['rms'], 6)} " + " ".join(fmt(h, 5, 0) for h in r["H"][1:8])
                  + f" {fmt(r['cen_f0'], 6, 2)} " + " ".join(fmt(f, 5, 0) for f in r["floor"])
                  + f" {fmt(r['rise_ms'], 5, 0)} {fmt(r['release_ms'], 5, 0)}")


def registers(rows):
    third = max(1, len(rows) // 3)
    return {"low": rows[:third], "middle": rows[third:-third] or rows, "high": rows[-third:]}


def mean(vs):
    vs = [v for v in vs if v is not None]
    return float(np.mean(vs)) if vs else None


def compare(model, args, dynamic=None, lib=None, quiet=False):
    """The model at the recordings' pitches. Returns per-register errors."""
    refs = [r for r in reference(model.name, args) if model.range[0] <= r["midi"] <= model.range[1]
            and r["cen_f0"] > 1.02 and (lib is None or r["lib"] == lib)]
    if dynamic is None:
        dynamic = {"p": 0.3, "mf": 0.55, "f": 0.8}
    by_dyn = {}
    for r in refs:
        by_dyn.setdefault(r["dynamic"], {}).setdefault(r["midi"], r)
    out = []
    for dyn, rows in sorted(by_dyn.items()):
        ps = sorted(rows)
        got = model.notes(ps, dynamic[dyn], tag=f"cmp_{dyn}")
        for m, g in zip(ps, got):
            out.append((dyn, m, rows[m], g))
    if not quiet:
        print(f"--- {model.name}: recording / model, at the recordings' pitches")
        print(f"{'dyn':>3} {'midi':>4} " + " ".join(f"{'H' + str(k):>9}" for k in range(2, 7))
              + f" {'cen/f0':>11} {'floor 1-2k':>11} {'8-16k':>9} {'cents':>6}")
        for dyn, m, r, g in out:
            H = (g or {}).get("H") or [None] * 12
            cells = [f"{fmt(r['H'][j], 4, 0)}/{fmt(H[j], 4, 0).strip():<4}" for j in range(1, 6)]
            gf = (g or {}).get("floor") or [None] * 6
            print(f"{dyn:>3} {m:>4} " + " ".join(cells) + f" {r['cen_f0']:>5.1f}/{fmt((g or {}).get('cen_f0'), 5, 1).strip():<5}"
                  + f" {fmt(r['floor'][2], 5, 0)}/{fmt(gf[2], 4, 0).strip():<5} {fmt(r['floor'][5], 4, 0)}/{fmt(gf[5], 4, 0).strip():<4} {fmt((g or {}).get('cents'), 6)}")
    return summarize(out)


def summarize(out):
    ok = [(dyn, m, r, g) for dyn, m, r, g in out if g and g.get("cents") is not None and abs(g["cents"]) < 50]
    s = {"notes": len(out), "speaking": len(ok)}
    if not ok:
        return s
    bright = [math.log2(g["cen_f0"] / r["cen_f0"]) for _, _, r, g in ok]
    s["bright_rms"] = float(np.sqrt(np.mean(np.square(bright))))
    regs = registers(sorted(ok, key=lambda t: t[1]))
    s["bright"] = {k: mean([math.log2(g["cen_f0"] / r["cen_f0"]) for _, _, r, g in v]) for k, v in regs.items()}
    hs = [g["H"][j] - r["H"][j] for _, _, r, g in ok for j in (1, 2) if g["H"][j] is not None and r["H"][j] is not None]
    s["h23_rms"] = float(np.sqrt(np.mean(np.square(hs)))) if hs else None
    for k, name in ((2, "floor_mid"), (5, "floor_top")):
        d = [g["floor"][k] - r["floor"][k] for _, _, r, g in ok if g["floor"][k] is not None and r["floor"][k] is not None]
        s[name] = mean(d)
    sl = [(g["floor"][5] - g["floor"][2]) - (r["floor"][5] - r["floor"][2]) for _, _, r, g in ok
          if None not in (g["floor"][5], g["floor"][2], r["floor"][5], r["floor"][2])]
    s["slope_err"] = mean(sl)
    s["cents_max"] = float(max(abs(g["cents"]) for _, _, _, g in ok))
    return s


def cmd_compare(args):
    model = Model(args.name, args)
    s = compare(model, args)
    print(json.dumps(s, indent=1))


def cmd_floor(args):
    model = Model(args.name, args)
    refs = [r for r in reference(model.name, args) if model.range[0] <= r["midi"] <= model.range[1]]
    head = f"{'':>20} " + " ".join(f"{lo}-{hi}".rjust(10) for lo, hi in BANDS)
    print(f"--- {model.name}: floor between partials, dB re H1 in 100 Hz\n{head}")
    regs = registers(sorted(refs, key=lambda r: r["midi"]))
    for g, rs in regs.items():
        f = [mean([r["floor"][k] for r in rs]) for k in range(len(BANDS))]
        print(f"{'recorded ' + g:>20} " + " ".join(fmt(v, 10, 0) for v in f))
    ps = sorted({rs[len(rs) // 2]["midi"] for rs in regs.values()})
    for d in (0.3, 0.55, 0.8):
        for m, r in zip(ps, model.notes(ps, d, beats=3, gate=0.8, tag="floor")):
            print(f"{'model ' + str(m) + ' at ' + str(d):>20} " + " ".join(fmt(v, 10, 0) for v in (r or {}).get("floor", [None] * 6)))


def cmd_summary(args):
    names = [args.name] if args.name else ORDER
    rows = []
    for n in names:
        model = Model(n, args)
        s = compare(model, args, quiet=True)
        # Across the range at mf: worst tuning, level from bottom to top, and
        # aliasing at the top note at ff with the breath off.
        ps = model.pitches(4)
        mf = model.notes(ps, 0.5, tag="sum_mf")
        spoke = [r for r in mf if r and r.get("cents") is not None and abs(r["cents"]) < 50]
        s["range_speaking"] = f"{len(spoke)}/{len(ps)}"
        s["range_cents_max"] = max((abs(r["cents"]) for r in spoke), default=None)
        s["level_low_to_high"] = (mf[-1]["rms"] - mf[0]["rms"]) if mf[0] and mf[-1] else None
        if model.spec:
            top = model.notes(ps[-2:], 1.0, QUIET, tag="sum_alias")
            s["alias_top"] = max((r.get("alias", -200) for r in top if r), default=None)
        soft, loud = model.notes(ps[:1], 0.25, tag="sum_p")[0], model.notes(ps[:1], 0.9, tag="sum_f")[0]
        if soft and loud and soft.get("centroid") and loud.get("centroid"):
            s["low_brightening_oct"] = math.log2(loud["centroid"] / soft["centroid"])
            s["low_level_p_to_f"] = loud["rms"] - soft["rms"]
        rows.append((n, s))
    if args.json:
        Path(args.json).write_text(json.dumps(dict(rows), indent=1))
    keys = [("speaking", "spoke"), ("bright_rms", "bright"), ("h23_rms", "H2/3"), ("floor_mid", "fl1-2k"),
            ("floor_top", "fl8-16k"), ("slope_err", "slope"), ("cents_max", "cents"), ("range_speaking", "range"),
            ("range_cents_max", "rcents"), ("level_low_to_high", "lvl lo>hi"), ("low_brightening_oct", "p>f oct"),
            ("low_level_p_to_f", "p>f dB"), ("alias_top", "alias")]
    print(f"{'':>14} " + " ".join(f"{h:>9}" for _, h in keys))
    for n, s in rows:
        cells = []
        for k, _ in keys:
            v = s.get(k)
            cells.append(f"{v:>9}" if isinstance(v, str) else fmt(v, 9, 2))
        if s.get("speaking") is not None:
            cells[0] = f"{s['speaking']}/{s['notes']}".rjust(9)
        print(f"{n:>14} " + " ".join(cells))


def cmd_vibrato(args):
    """A long note through std's playing rules beside the recordings' vibrato."""
    model = Model(args.name, args)
    lo, hi = model.range
    ps = [lo + (hi - lo) // 4, lo + (hi - lo) // 2, lo + 3 * (hi - lo) // 4]
    WORK.mkdir(parents=True, exist_ok=True)
    src = WORK / f"_{model.name}_vib.muz"
    wav = WORK / f"{model.name}_vib.wav"
    notes = ", ".join(f"note({m}, 4b, velocity=0.55).at({k * 5}b)" for k, m in enumerate(ps))
    perf = args.spec or f"w.specs.{model.name}"
    src.write_text(model.header() + f"let line = w.perform(stack([{notes}]), {perf}, {{timing: {{tempo: 60}}}});\n"
                   f'song({{tempo: 60, tracks: [track("p", line, w.instrument({perf}))], tail: 1}})\n')
    subprocess.run([MUZ, "render", str(src), "-o", str(wav)], check=True, capture_output=True)
    x, sr = sf.read(wav)
    x = x.mean(axis=1)
    for k, m in enumerate(ps):
        r = measure(x, sr, k * 5, k * 5 + 4, m, want_vibrato=True)
        print(f"model {m}: {r.get('vibrato')}  rise {r.get('rise_ms')} ms")
    for r in reference(model.name, args):
        if r.get("vibrato") and r["vibrato"]["cents"] > 1.5:
            print(f"rec {r['midi']} {r['lib']} {r['dynamic']}: {r['vibrato']}  rise {r.get('rise_ms')} ms")


# --- Calibration ---------------------------------------------------------------------


def cal_path(name, args):
    return Path(args.cal_json) if args.cal_json else CAL_JSON / f"{name}.json"


def load_cal(name, args):
    p = cal_path(name, args)
    return json.loads(p.read_text()) if p.exists() else {"thr": [], "cents": []}


def save_cal(name, args, c):
    p = cal_path(name, args)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(c, indent=1) + "\n")


def table(points, default):
    vals = [v for _, v in points if v is not None]
    if vals and min(vals) > 0.1 and max(vals) < 1:
        # A threshold far under its neighbours is a missed detection.
        med = sorted(vals)[len(vals) // 2]
        fixed = []
        for k, (m, v) in enumerate(points):
            if v is not None and v < 0.6 * med:
                near = [w for _, w in points[k + 1:k + 3] + points[max(k - 2, 0):k] if w is not None and w >= 0.6 * med]
                v = near[0] if near else med
            fixed.append([m, v])
        points = fixed
    pts = [[round(math.log2(hz(m)), 5), v] for m, v in points if v is not None]
    return pts[:64] if len(pts) >= 2 else default


def literal(c):
    return "{thr: " + json.dumps(table(c.get("thr", []), [[4, 1], [12, 1]])) + ", cents: " + json.dumps(table(c.get("cents", []), [[4, 0], [12, 0]])) + "}"


def band_level(seg, sr, f0, n=2048, hop=256, first_only=False):
    w = np.hanning(n)
    freqs = np.fft.rfftfreq(n, 1 / sr)
    if first_only:
        sel = (freqs > f0 * 0.93) & (freqs < f0 * 1.07)
    else:
        sel = np.zeros_like(freqs, dtype=bool)
        for k in (1, 2, 3):
            sel |= (freqs > f0 * (k - 0.1)) & (freqs < f0 * (k + 0.1))
    return np.array([np.sqrt((np.abs(np.fft.rfft(seg[a:a + n] * w))[sel] ** 2).sum()) for a in range(0, len(seg) - n, hop)])


def cmd_thresholds(model, args):
    kind = INSTRUMENTS.get(model.name, {}).get("threshold") or args.threshold
    if kind == "flue":
        save_cal(model.name, args, {**load_cal(model.name, args), "thr": []})
        return
    ps = model.pitches(3)
    grid, hold = 6.0, 5.0
    if kind == "reed":
        hi, lo = 0.62, 0.3
        curve = f"[[0, {hi}], [0.06, {hi}], [1, {lo}]]"
    else:
        lo, hi = 0.2, 0.62
        curve = f"[[0, {lo}], [0.9, {hi}], [1, {hi}]]"
    raw = "{calibration: w.raw, raw: true, noise: 0, hiss: 0, finger: 0, drift_cents: 0}"
    saved, model.drive = model.drive, "pressure"
    x, sr = model.render([model.note(m, 6, round(hold / grid, 4), curve, "{tongue: 0}") for m in ps], raw, "thr")
    model.drive = saved
    out = []
    for k, m in enumerate(ps):
        seg = x[int(k * grid * sr):int((k * grid + hold) * sr)]
        n_fft = 4096 if hz(m) < 200 else 2048
        env = band_level(seg, sr, hz(m), n=n_fft, first_only=(kind == "cone"))
        t = np.arange(len(env)) * 256 / sr + n_fft / 2 / sr
        if kind == "cone":
            g = np.where(t > 0.9 * hold, hi, lo + (hi - lo) * t / (0.9 * hold))
            ref = np.median(env[t > 0.92 * hold])
            on = np.nonzero(env > 0.3 * ref)[0]
            thr = round(float(g[on[0]]), 4) if len(on) else None
        else:
            g = np.where(t < 0.06 * hold, hi, hi + (lo - hi) * (t - 0.06 * hold) / (0.94 * hold))
            below = np.nonzero((t > 0.1 * hold) & (env < 0.02 * env.max()))[0]
            thr = round(float(g[below[0]]), 4) if len(below) else None
        print(f"{m:>4} threshold {thr}")
        out.append([m, thr])
    c = load_cal(model.name, args)
    c["thr"] = out
    save_cal(model.name, args, c)


def cmd_tune(model, args, passes=3):
    for k in range(passes):
        c = load_cal(model.name, args)
        prev = dict((m, v) for m, v in c.get("cents", []))
        ps = model.pitches(2)
        rows = model.notes(ps, 0.5, "{calibration: " + literal(c) + ", drift_cents: 0}", tag="tune")
        out, worst = [], 0.0
        for m, r in zip(ps, rows):
            err = (r or {}).get("cents")
            if err is None or abs(err) > 80:
                print(f"{m:>4} did not speak at pitch ({err})")
                out.append([m, prev.get(m, 0.0)])
                continue
            worst = max(worst, abs(err))
            out.append([m, round(prev.get(m, 0.0) + err, 2)])
        c["cents"] = out
        save_cal(model.name, args, c)
        print(f"tuning pass {k + 1}: worst error {worst:.1f} cents")


def cmd_calibrate(args):
    model = Model(args.name, args)
    if not args.keep:
        save_cal(model.name, args, {"thr": [], "cents": []})
    if not args.tune_only:
        cmd_thresholds(model, args)
    cmd_tune(model, args)
    emit(args)


def emit(args):
    if args.cal_out:
        c = load_cal(args.name, args)
        Path(args.cal_out).write_text(f"// Calibration of {args.name}, written by muz-core tools/woodwinds.py.\n"
                                      f"let {args.name} = {literal(c)};\n")
        print(f"wrote {args.cal_out}")
        return
    lines = ["// Generated by tools/woodwinds.py calibrate. Do not edit by hand.",
             "// Each instrument's measured speaking threshold (`thr`: the mouth pressure at",
             "// which the raw model stops or starts speaking, over the reed's closing",
             "// pressure) and tuning correction (`cents`), both against log2 of the",
             "// sounding frequency. `raw` measures an uncalibrated model."
             , "let raw = {thr: [[4, 1], [12, 1]], cents: [[4, 0], [12, 0]]};"]
    for n in ORDER:
        p = CAL_JSON / f"{n}.json"
        c = json.loads(p.read_text()) if p.exists() else existing_table(n)
        lines.append(f"let {n} = {c if isinstance(c, str) else literal(c)};")
    STD_TABLES.write_text("\n".join(lines) + "\n")
    print(f"wrote {STD_TABLES}")


def existing_table(name):
    for line in STD_TABLES.read_text().splitlines() if STD_TABLES.exists() else []:
        if line.startswith(f"let {name} = "):
            return line[len(f"let {name} = "):].rstrip(";")
    return "{thr: [[4, 1], [12, 1]], cents: [[4, 0], [12, 0]]}"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("command", choices=["reference", "check", "compare", "floor", "summary", "vibrato", "calibrate", "emit"])
    ap.add_argument("name", nargs="?")
    ap.add_argument("--spec", help="a spec record expression, played as w.patch(SPEC, ...)")
    ap.add_argument("--model", help="any instrument expression (not for calibrate)")
    ap.add_argument("--use", action="append", help="PATH:ALIAS, a module the expressions may use")
    ap.add_argument("--extra", help="overrides merged into the spec's patch, a record literal")
    ap.add_argument("--drive", choices=["velocity", "pressure"])
    ap.add_argument("--range", help="LOW,HIGH MIDI pitches")
    ap.add_argument("--recordings", help="a folder of single-note WAV files to compare with")
    ap.add_argument("--dynamics", help="comma-separated dynamics for check")
    ap.add_argument("--step", type=int, default=3)
    ap.add_argument("--threshold", choices=["reed", "cone", "flue"], default="reed", help="threshold method for a derived instrument")
    ap.add_argument("--cal-json", help="where calibration measurements accumulate")
    ap.add_argument("--cal-out", help="write the calibration as a .muz file here instead of std's tables")
    ap.add_argument("--keep", action="store_true", help="refine the existing calibration")
    ap.add_argument("--tune-only", action="store_true")
    ap.add_argument("--json", help="summary: also write the numbers here")
    args = ap.parse_args()
    if args.command == "reference":
        return cmd_reference(args)
    if args.command == "emit":
        return emit(args)
    if args.command != "summary" and not args.name:
        ap.error("name the instrument")
    {"check": cmd_check, "compare": cmd_compare, "floor": cmd_floor, "summary": cmd_summary,
     "vibrato": cmd_vibrato, "calibrate": cmd_calibrate}[args.command](args)


if __name__ == "__main__":
    main()
