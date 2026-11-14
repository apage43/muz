# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "openpyxl", "soundfile"]
# ///
"""Fit, measure and generate the tone-hole lattice clarinet prototype.

    uv run tools/clarinet-lattice.py peaks ImpedClarinet.xlsx   # reference peaks -> JSON
    uv run tools/clarinet-lattice.py fit                        # fit the geometry, write the muz data
    uv run tools/clarinet-lattice.py report                     # model peaks against the reference
    uv run tools/clarinet-lattice.py passive                    # rendered lattice against the reference
    uv run tools/clarinet-lattice.py tune                       # each fingering's own pitch -> tuning table
    uv run tools/clarinet-lattice.py check                      # tuned pitch at three dynamics
    uv run tools/clarinet-lattice.py slurs                      # settle, dip and stray modes of a slur grid

`fit` uses scipy and the spreadsheet only through the committed reference
JSON; `tune`, `check`, `slurs` and `passive` render with target/release/muz.

The reference is the measured input impedance of a B-flat clarinet for each
standard fingering, published by UNSW (Dickens, Smith & Wolfe, "Clarinet
acoustics", https://www.phys.unsw.edu.au/music/clarinet/, spreadsheet
ImpedClarinet.xlsx). It is measured at the mouthpiece without the reed's
compliance. `peaks` reduces it to the peak frequencies and -3 dB widths of
|Z| and writes them to examples/woodwind-prototypes/clarinet-reference.json;
the spreadsheet itself is not part of the repository.

The model here is the frequency response of the lattice that
examples/woodwind-prototypes/clarinet_bore.muz builds, element by element:

  bore      cylinder segments of radius `BORE` between holes, as pure delays
            with a round-trip gain exp(-2 beta l) per segment, and a loss shelf
            (1-k) + k lowpass1(f_shelf) on the mouthpiece segment
  hole      a shunt junction between two segments. Its pressure is
            p = (1 - H_open)(1 - H_closed)(a + b) for incoming waves a, b, with
            H_open = d lowpass1(w_open)   (open path: inertance and resistance)
            H_closed = kappa highpass1(f_c) (closed chimney volume: compliance)
            w_open = rho + g alpha / 2, d = g alpha / (2 rho + g alpha)
            alpha = (b/a)^2 c / t_e, tau_c = (b/a)^2 t / c, kappa = tau_c w_c / 2
            g(x) = (1 + q) x / (x + q) is the open path's admittance at key travel x
            (a pad's curtain inertance; finger holes use g = x)
  series    the negative series inertance t_a of each hole shortens the bore
  bell      reflection -r_bell lowpass1(f_bell)^2 after the last segment

`fit` adjusts hole positions, radii and chimney heights (an effective geometry,
not a measured drawing) and the loss parameters to the reference peaks, and
writes examples/woodwind-prototypes/clarinet_geometry.muz.
"""

import argparse
import json
import math
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
PROTO = ROOT / "examples/woodwind-prototypes"
REFERENCE = PROTO / "clarinet-reference.json"
GEOMETRY_JSON = PROTO / "clarinet-geometry.json"
GEOMETRY_MUZ = PROTO / "clarinet_geometry.muz"

C = 345.0  # m/s, warm dry air; positions are effective, so c is a convention
BORE = 7.3e-3  # m, bore radius
CLOSED_HP = 25_000.0  # Hz, corner of the closed-hole compliance highpass
# Nonlinear (jet) resistance of a narrow hole, R = K rho |v| / S_h (Atig,
# Dalmont & Gilbert 2004, K about 0.5-1.2). In the model's units (pressure
# over the reed's closing pressure P_CLOSE, flow as a pressure wave) it adds
# 2 K P_CLOSE / (rho c^2) (S_bore / S_hole)^2 |leak| to the open path's
# resistance over alpha. Only the register hole, whose velocity is highest,
# carries it in the prototype.
JET_K = 0.36
P_CLOSE = 5000.0  # Pa
RHO = 1.2

NOTE = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]


def written_name(key):
    return f"{NAMES[key % 12]}{key // 12 - 1}"


def written_key(name):
    i, acc = 1, 0
    if name[1] == "#":
        i, acc = 2, 1
    return 12 * (int(name[i]) + 1) + NOTE[name[0]] + acc


# --- Holes and fingerings -------------------------------------------------------
#
# Holes from the mouthpiece down. `kind`: finger (a ring or plain hole under a
# finger), closed (a key whose spring holds its pad shut), open (a key whose
# spring holds its pad open), register. The key named is the one that moves the
# hole on a Boehm clarinet; `note` is the written chalumeau note the hole gives
# as the first open hole.
HOLES = [
    ("reg", "register", "register (speaker) key, thumb"),
    ("a", "closed", "throat A key, LH1"),
    ("gs", "closed", "throat G# key, LH1"),
    ("tr", "closed", "upper side trill key, RH"),
    ("t", "finger", "thumb hole"),
    ("l1", "finger", "LH1 ring"),
    ("l2", "finger", "LH2 ring"),
    ("eb", "closed", "Eb/Bb key (sliver/side key)"),
    ("l3", "finger", "LH3"),
    ("cs", "closed", "C#/G# key"),
    ("r1", "finger", "RH1 ring"),
    ("r2", "finger", "RH2 ring"),
    ("bb", "closed", "Bb/F side key"),
    ("r3", "finger", "RH3"),
    ("gsds", "closed", "G#/D# key, RH little finger"),
    ("fscs", "open", "F#/C# key, little finger"),
    ("fc", "open", "F/C key, little finger"),
    ("eb_low", "open", "E/B key, little finger"),
]
HOLE_NAMES = [h[0] for h in HOLES]
FINGERS = ["t", "l1", "l2", "l3", "r1", "r2", "r3"]
OPEN_STANDING = ["fscs", "fc", "eb_low"]


def chalumeau(fingers, keys=(), pressed=()):
    """Open holes for `fingers` down, closed-standing `keys` opened, and
    open-standing `pressed` keys closed."""
    opened = set(keys)
    opened |= {f for f in FINGERS if f not in fingers}
    opened |= {k for k in OPEN_STANDING if k not in pressed}
    return opened


ALL = FINGERS
LOW_KEYS = ("fscs", "fc", "eb_low")
CHALUMEAU = {
    52: chalumeau(ALL, pressed=LOW_KEYS),
    53: chalumeau(ALL, pressed=("fscs", "fc")),
    54: chalumeau(ALL, pressed=("fscs",)),
    55: chalumeau(ALL),
    56: chalumeau(ALL, keys=("gsds",)),
    57: chalumeau(["t", "l1", "l2", "l3", "r1", "r2"]),
    58: chalumeau(["t", "l1", "l2", "l3", "r1", "r2"], keys=("bb",)),
    59: chalumeau(["t", "l1", "l2", "l3", "r1"]),
    60: chalumeau(["t", "l1", "l2", "l3"]),
    61: chalumeau(["t", "l1", "l2", "l3"], keys=("cs",)),
    62: chalumeau(["t", "l1", "l2"]),
    63: chalumeau(["t", "l1", "l2"], keys=("eb",)),
    64: chalumeau(["t", "l1"]),
    65: chalumeau(["t"]),
    66: chalumeau(["l1"]),  # cross fingering: thumb open, LH1 down
    67: chalumeau([]),
    68: chalumeau([], keys=("gs",)),
    69: chalumeau([], keys=("gs", "a")),  # the A key lifts the G# pad too
    70: chalumeau([], keys=("gs", "a", "reg")),
}
# Forks and resonance fingerings, by written note.
ALTERNATES = {
    63: ("fork", chalumeau(["t", "l1", "l3", "r1"])),
    70: ("side", chalumeau([], keys=("gs", "a", "reg", "tr"))),
}


def fingering(key, variant=0):
    """The set of open holes for written `key`, standard (0) or alternate (1)."""
    def chal(n):
        if variant == 1 and n in ALTERNATES:
            return set(ALTERNATES[n][1])
        return set(CHALUMEAU[n])
    if key <= 70:
        return chal(key)
    if key <= 84:  # clarion: the chalumeau a twelfth down, register key open
        return chal(key - 19) | {"reg"}
    if key <= 89:  # altissimo: the fifth harmonic, LH1 lifted as a vent
        return chal(key - 28) | {"reg", "l1"}
    raise ValueError(f"written {key} is outside the prototype's range")


def has_alternate(key):
    if key <= 70:
        return key in ALTERNATES
    if key <= 84:
        return key - 19 in ALTERNATES
    return key - 28 in ALTERNATES


# Which impedance peak sounds the note: 1 chalumeau and throat, 2 clarion
# (the twelfth), 3 altissimo (the fifth harmonic).
def mode(key):
    return 1 if key <= 70 else 2 if key <= 84 else 3


RANGE = range(52, 90)  # written E3..F6 = sounding D3..D#6

# Reference fingerings by UNSW name: (written key, variant).
REFERENCE_NAMES = {written_name(k): (k, 0) for k in RANGE}
REFERENCE_NAMES.update({"A#4alt": (70, 1), "D#4alt2": (63, 1)})


# --- Geometry -> lumped elements --------------------------------------------------

def inner_correction(b):
    d = b / BORE
    return b * (0.82 - 0.193 * d - 1.09 * d**2 + 1.27 * d**3 - 0.71 * d**4)


def series_correction(b):
    d = b / BORE
    return b * (-0.37 + 0.087 * d) * d**2


def lumped(hole):
    """Lumped parameters of one hole record {position, radius, chimney, pad}."""
    b, t = hole["radius"], hole["chimney"]
    area = (b / BORE) ** 2
    te = t + inner_correction(b) + 0.75 * b
    alpha = area * C / te
    tau = area * t / C
    # The compliance's exact release is a highpass at 2 / tau of gain 1; where
    # that corner is above CLOSED_HP (small holes) it is lowered to CLOSED_HP
    # with the gain that keeps its low-frequency slope, tau / 2. The gain is
    # then below 1 and the element stays passive.
    wc = min(2 * math.pi * CLOSED_HP, 2 / tau)
    return {
        "alpha": alpha,
        "closed_w": wc,
        "kappa": tau * wc / 2,
        "nonlinear": 2 * JET_K * P_CLOSE / (RHO * C**2) / area**2 if hole.get("kind") == "register" else 0.0,
        "series": series_correction(b),
        "effective_length": te,
    }


def opening(x, q):
    """Open-path admittance fraction at key travel x (0 shut .. 1 open)."""
    x = np.clip(x, 0.0, 1.0)
    if q is None:
        return x
    return (1 + q) * x / (x + q)


def hole_resistance(hole, loss):
    """The open path's resistance over its inertance, per alpha: the narrow
    register tube has its own."""
    key = "register" if hole["kind"] == "register" else "hole"
    return max(loss.get(key, loss["hole"]), 0.001)


def segments(geom):
    """Segment lengths in metres, reed plane to bell, after series corrections."""
    holes = geom["holes"]
    ends = [0.0] + [h["position"] for h in holes] + [geom["bell"]]
    lengths = np.diff(ends)
    corrections = [lumped(h)["series"] for h in holes]
    out = lengths.copy()
    for k, ta in enumerate(corrections):
        out[k] += ta / 2  # ta is negative: the hole shortens the bore
        out[k + 1] += ta / 2
    return out


def impedance(geom, states, freqs):
    """Normalized input impedance at the reed plane for hole travels `states`:
    a dict name -> 0..1, or an array [fingering, hole] in HOLE_NAMES order."""
    if isinstance(states, dict):
        states = np.array([[states.get(n, 0.0) for n in HOLE_NAMES]])
        return impedance(geom, states, freqs)[0]
    states = np.asarray(states, dtype=float)
    loss = geom["loss"]
    w = 2 * np.pi * freqs
    jw = 1j * w
    lengths = segments(geom)
    holes = geom["holes"]
    wb = 2 * np.pi * loss["bell_hz"]
    R = np.broadcast_to(-loss["bell"] / (1 + jw / wb) ** 2, (len(states), len(freqs))).copy()
    for k in range(len(lengths) - 1, -1, -1):
        ell = lengths[k]
        R *= np.exp(-2 * loss["beta"] * ell) * np.exp(-jw * 2 * ell / C)
        if k == 0:
            ws = 2 * np.pi * loss["shelf_hz"]
            R *= (1 - loss["shelf"]) + loss["shelf"] / (1 + jw / ws)
        else:
            h = holes[k - 1]
            el = lumped(h)
            g = opening(states[:, HOLE_NAMES.index(h["name"])], h.get("pad"))[:, None]
            rho = hole_resistance(h, loss) * el["alpha"]
            wo = rho + g * el["alpha"] / 2
            d = g * el["alpha"] / (2 * rho + g * el["alpha"])
            Ho = d / (1 + jw / wo)
            wc = el["closed_w"]
            Hc = el["kappa"] * (jw / wc) / (1 + jw / wc)
            P = (1 - Ho) * (1 - Hc)
            ratio = R * P / (1 + R * (1 - P))
            R = P * (1 + ratio) - 1
    return (1 + R) / (1 - R)


def states_for(key, variant=0):
    opened = fingering(key, variant)
    return {name: (1.0 if name in opened else 0.0) for name in HOLE_NAMES}


# --- Peaks ---------------------------------------------------------------------------

def find_peaks(freqs, mag, count=4, floor=0.08):
    """Local maxima of |Z| above `floor` of the largest, parabolic in log |Z|,
    with -3 dB widths."""
    m = np.asarray(mag)
    top = np.nanmax(m)
    idx = np.flatnonzero((m[1:-1] > m[:-2]) & (m[1:-1] >= m[2:]) & (m[1:-1] > floor * top)) + 1
    out = []
    step = freqs[1] - freqs[0]
    for i in idx:
        a, b, c = np.log(m[i - 1]), np.log(m[i]), np.log(m[i + 1])
        denom = a - 2 * b + c
        p = 0.5 * (a - c) / denom if denom != 0 else 0.0
        f = freqs[i] + p * step
        half = m[i] / math.sqrt(2)
        lo = i
        while lo > 0 and m[lo] > half:
            lo -= 1
        hi = i
        while hi < len(m) - 1 and m[hi] > half:
            hi += 1
        if m[lo] > half or m[hi] > half:
            width = None
        else:
            flo = np.interp(half, [m[lo], m[lo + 1]], [freqs[lo], freqs[lo + 1]])
            fhi = np.interp(half, [m[hi], m[hi - 1]], [freqs[hi], freqs[hi - 1]])
            width = float(fhi - flo)
        out.append({"hz": float(f), "z": float(m[i]), "width": width})
        if len(out) >= count:
            break
    return out


def reference_peaks(xlsx):
    import openpyxl

    wb = openpyxl.load_workbook(xlsx, read_only=True, data_only=True)
    sheets = []
    for ws in wb.worksheets[2:4]:  # real and imaginary parts
        rows = [r for r in ws.iter_rows(values_only=True)]
        sheets.append(rows)
    head = sheets[0][0]
    freqs = np.array([float(r[0]) for r in sheets[0][1:] if r[0] is not None])
    out = {}
    for col, name in enumerate(head):
        if col == 0 or name not in REFERENCE_NAMES:
            continue
        re = np.array([float(r[col]) for r in sheets[0][1 : len(freqs) + 1]])
        im = np.array([float(r[col]) for r in sheets[1][1 : len(freqs) + 1]])
        key, variant = REFERENCE_NAMES[name]
        out[name] = {
            "written": key,
            "variant": variant,
            "peaks": find_peaks(freqs, np.abs(re + 1j * im), count=5, floor=0.05),
        }
    return out


# --- Fitting ---------------------------------------------------------------------------

FREQS = np.arange(100.0, 2200.0, 1.0)


def model_peaks(geom, key, variant=0, count=4):
    z = impedance(geom, states_for(key, variant), FREQS)
    return find_peaks(FREQS, np.abs(z), count=count, floor=0.05)


def all_model_peaks(geom, fingerings, count=6):
    """Peaks for a list of (key, variant), in one vectorized pass."""
    states = np.array([[states_for(k, v)[n] for n in HOLE_NAMES] for k, v in fingerings])
    z = np.abs(impedance(geom, states, FREQS))
    return [find_peaks(FREQS, row, count=count, floor=0.05) for row in z]


def cents(a, b):
    return 1200 * math.log2(a / b)


def match(model, measured):
    """Model peak nearest each measured peak (frequency order is not trusted)."""
    out = []
    for p in measured:
        best = min(model, key=lambda q: abs(math.log(q["hz"] / p["hz"])), default=None)
        out.append(best)
    return out


def compared(geom, ref, peaks=3):
    items = sorted(ref.items(), key=lambda kv: (kv[1]["written"], kv[1]["variant"]))
    model = all_model_peaks(geom, [(r["written"], r["variant"]) for _, r in items])
    rows = []
    for (name, r), got_all in zip(items, model):
        want = r["peaks"][:peaks]
        rows.append((name, r["written"], r["variant"], want, match(got_all, want)))
    return rows


def initial_geometry():
    # A seed from typical Boehm B-flat clarinet proportions, metres from the
    # reed tip. `fit` moves every value.
    seed = {
        "reg": (0.150, 0.0015, 0.013), "a": (0.163, 0.0035, 0.008), "gs": (0.176, 0.0030, 0.008),
        "tr": (0.192, 0.0020, 0.008), "t": (0.205, 0.0035, 0.009), "l1": (0.230, 0.0035, 0.008),
        "l2": (0.255, 0.0035, 0.007), "eb": (0.268, 0.0025, 0.007), "l3": (0.284, 0.0040, 0.007),
        "cs": (0.303, 0.0035, 0.007), "r1": (0.325, 0.0040, 0.006), "r2": (0.350, 0.0040, 0.006),
        "bb": (0.366, 0.0030, 0.006), "r3": (0.385, 0.0045, 0.006), "gsds": (0.404, 0.0040, 0.006),
        "fscs": (0.430, 0.0050, 0.006), "fc": (0.458, 0.0055, 0.006), "eb_low": (0.490, 0.0060, 0.006),
    }
    holes = []
    for name, kind, label in HOLES:
        pos, b, t = seed[name]
        holes.append({
            "name": name, "kind": kind, "label": label, "position": pos, "radius": b, "chimney": t,
            "pad": None if kind == "finger" else 0.2,
        })
    return {
        "holes": holes, "bell": 0.600,
        "loss": {"beta": 0.05, "shelf": 0.15, "shelf_hz": 1500.0, "hole": 0.05, "register": 0.3, "bell": 0.97, "bell_hz": 1800.0},
    }


LOSS_KEYS = ["beta", "shelf", "shelf_hz", "hole", "register", "bell", "bell_hz"]
LOSS_BOUNDS = {
    "beta": (0.0, 1.0), "shelf": (0.0, 0.6), "shelf_hz": (200.0, 6000.0), "hole": (0.0, 1.0), "register": (0.0, 4.0),
    "bell": (0.5, 1.0), "bell_hz": (300.0, 6000.0),
}
MIN_SEGMENT = 0.009  # m: two samples at 88.2 kHz is 7.8 mm


def pack(geom, what):
    x, lo, hi = [], [], []
    holes = geom["holes"]
    if "geometry" in what:
        prev = 0.0
        for h in holes:
            x.append(h["position"] - prev); lo.append(MIN_SEGMENT); hi.append(0.2)
            prev = h["position"]
        x.append(geom["bell"] - prev); lo.append(0.05); hi.append(0.3)
        # Plausible clarinet holes: radius 1.5-6 mm (register 1.2-1.7), chimney
        # 2.5-12 mm (the register tube 10-20).
        for h in holes:
            reg = h["kind"] == "register"
            x.append(h["radius"] * 1000); lo.append(1.2 if reg else 1.5); hi.append(1.7 if reg else 6.0)
        for h in holes:
            reg = h["kind"] == "register"
            x.append(h["chimney"] * 1000); lo.append(10.0 if reg else 2.5); hi.append(20.0 if reg else 12.0)
    if "loss" in what:
        for k in LOSS_KEYS:
            x.append(geom["loss"][k]); lo.append(LOSS_BOUNDS[k][0]); hi.append(LOSS_BOUNDS[k][1])
    x = np.clip(np.array(x), np.array(lo) + 1e-9, np.array(hi) - 1e-9)
    return x, (np.array(lo), np.array(hi))


def unpack(geom, x, what):
    g = json.loads(json.dumps(geom))
    i = 0
    holes = g["holes"]
    n = len(holes)
    if "geometry" in what:
        pos = np.cumsum(x[: n + 1])
        for k, h in enumerate(holes):
            h["position"] = float(pos[k])
        g["bell"] = float(pos[n])
        i = n + 1
        for k, h in enumerate(holes):
            h["radius"] = float(x[i + k]) / 1000
        i += n
        for k, h in enumerate(holes):
            h["chimney"] = float(x[i + k]) / 1000
        i += n
    if "loss" in what:
        for k in LOSS_KEYS:
            g["loss"][k] = float(x[i]); i += 1
    return g


def residuals(geom, ref, widths=False, peaks=2):
    """Cents errors of the first `peaks` (and the sounding) impedance peaks of
    every reference fingering, in units of 10 cents; optionally widths (in
    units of 10%) and, for register fingerings, the height of the first peak
    against the sounding one, which decides whether the register speaks."""
    items = sorted(ref.items())
    fingerings = [(r["written"], r["variant"]) for _, r in items]
    model = all_model_peaks(geom, fingerings)
    out = []
    for (name, r), got_all in zip(items, model):
        key, variant = r["written"], r["variant"]
        n = max(peaks, mode(key))
        want = r["peaks"][:n]
        got = match(got_all, want)
        weight = 1.0 if variant == 0 else 0.5
        for k, (w, m) in enumerate(zip(want, got)):
            # The sounding peak counts most.
            wk = weight * (2.0 if k + 1 == mode(key) else 1.0)
            if m is None:
                out.append(wk * 3.0)
                if widths:
                    out.append(0.0)
                continue
            out.append(wk * cents(m["hz"], w["hz"]) / 10)
            if widths:
                if m["width"] is None or w["width"] is None:
                    out.append(0.0)
                else:
                    out.append(0.5 * wk * math.log(m["width"] / w["width"]) / 0.1)
        if widths:
            s = mode(key) - 1
            ok = s > 0 and all(g is not None for g in got[: s + 1])
            out.append(2 * math.log((got[0]["z"] / got[s]["z"]) / (want[0]["z"] / want[s]["z"])) / 0.1 if ok else 0.0)
    return np.array(out)


_WORK = {}


def _init(geom, ref, what, widths):
    _WORK.update(geom=geom, ref=ref, what=what, widths=widths)


def _column(args):
    x, i, h = args
    y = x.copy()
    y[i] += h
    return residuals(unpack(_WORK["geom"], y, _WORK["what"]), _WORK["ref"], widths=_WORK["widths"])


def fit(geom, ref, what, widths, max_nfev, workers=48):
    """Least squares on the reference peaks; the Jacobian's columns are
    computed in parallel."""
    from concurrent.futures import ProcessPoolExecutor
    from scipy.optimize import least_squares

    x0, bounds = pack(geom, what)

    def f(x):
        return residuals(unpack(geom, x, what), ref, widths=widths)

    with ProcessPoolExecutor(workers, initializer=_init, initargs=(geom, ref, what, widths)) as pool:
        def jac(x):
            f0 = f(x)
            steps = 1e-4 * np.maximum(np.abs(x), 1e-3)
            # Step inward at an upper bound.
            steps = np.where(x + steps > bounds[1], -steps, steps)
            cols = list(pool.map(_column, [(x, i, steps[i]) for i in range(len(x))]))
            return np.stack([(c - f0) / h for c, h in zip(cols, steps)], axis=1)

        result = least_squares(f, x0, jac=jac, bounds=bounds, x_scale="jac", max_nfev=max_nfev, verbose=1)
    print(f"cost {result.cost:.1f}, {result.nfev} evaluations")
    return unpack(geom, result.x, what)


# --- Output ------------------------------------------------------------------------------

def report(geom, ref):
    rows = compared(geom, ref, peaks=3)
    errs = {1: [], 2: [], 3: []}
    werr = []
    print(f"{'name':10s} {'mode':>4s}   " + "   ".join(f"{'peak'+str(k):>8s} {'ref':>7s} {'cents':>6s} {'width/ref':>9s}" for k in (1, 2, 3)))
    for name, key, variant, want, got in rows:
        cells = []
        for k, (w, m) in enumerate(zip(want, got), start=1):
            if m is None:
                cells.append(f"{'-':>8s} {w['hz']:7.1f} {'-':>6s} {'-':>9s}")
                continue
            c = cents(m["hz"], w["hz"])
            ratio = (m["width"] / w["width"]) if (m["width"] and w["width"]) else float("nan")
            if variant == 0:
                errs[k].append(c)
                if not math.isnan(ratio):
                    werr.append(ratio)
            cells.append(f"{m['hz']:8.1f} {w['hz']:7.1f} {c:+6.1f} {ratio:9.2f}")
        print(f"{name:10s} {mode(key):4d}   " + "   ".join(cells))
    for k in (1, 2, 3):
        e = np.abs(errs[k])
        if len(e):
            print(f"peak {k}: mean |err| {e.mean():.1f} cents, max {e.max():.1f}, within 10 cents {np.mean(e <= 10):.0%}")
    w = np.array(werr)
    print(f"width ratio model/ref: median {np.median(w):.2f}, 10-90% {np.percentile(w, 10):.2f}-{np.percentile(w, 90):.2f}")


def muz_number(x):
    # muz numbers have no exponent: six significant digits, positional.
    return np.format_float_positional(float(f"{x:.6g}"), trim="-")


def write_muz(geom):
    holes = geom["holes"]
    lengths = segments(geom)
    loss = geom["loss"]
    lines = [
        "// Generated by tools/clarinet-lattice.py fit from clarinet-geometry.json.",
        "// An effective B-flat clarinet geometry fitted to the UNSW impedance",
        "// measurements (see README.md). Edit the JSON and regenerate.",
        "//",
        "// holes: from the mouthpiece down. `seconds` is the bore segment above the",
        "// hole (after series inertance corrections), `alpha_hz` the open path's",
        "// inertance corner alpha / 2pi, `kappa` the closed chimney's compliance as a",
        "// highpass gain at `closed_hz`, `pad` the key's curtain parameter (finger",
        "// holes have none).",
        f"let closed_hz = {muz_number(CLOSED_HP)};",
        f"let speed = {muz_number(C)};",
        f"let bore_radius = {muz_number(BORE)};",
        "let holes = [",
    ]
    for k, h in enumerate(holes):
        el = lumped(h)
        pad = "null" if h.get("pad") is None else muz_number(h["pad"])
        lines.append(
            f"    {{name: \"{h['name']}\", kind: \"{h['kind']}\", seconds: {muz_number(lengths[k] / C)}, "
            f"alpha_hz: {muz_number(el['alpha'] / (2 * math.pi))}, resistance: {muz_number(hole_resistance(h, loss))}, "
            f"kappa: {muz_number(el['kappa'])}, nonlinear: {muz_number(el['nonlinear'])}, "
            f"closed_hz: {muz_number(el['closed_w'] / (2 * math.pi))}, pad: {pad}, "
            f"position_mm: {h['position'] * 1000:.2f}, radius_mm: {h['radius'] * 1000:.3f}, chimney_mm: {h['chimney'] * 1000:.2f}}},"
        )
    lines.append("];")
    lines.append(f"let bell_seconds = {muz_number(lengths[-1] / C)};")
    lines.append(f"let loss = {{beta: {muz_number(loss['beta'])}, shelf: {muz_number(loss['shelf'])}, "
                 f"shelf_hz: {muz_number(loss['shelf_hz'])}, hole: {muz_number(loss['hole'])}, "
                 f"bell: {muz_number(loss['bell'])}, bell_hz: {muz_number(loss['bell_hz'])}}};")
    lines.append("// Round-trip gain of each segment, mouthpiece first, then the bell segment.")
    lines.append("let segment_gain = [" + ", ".join(muz_number(math.exp(-2 * loss["beta"] * l)) for l in lengths) + "];")
    lines.append("")
    # Fingerings: for each written note, standard and alternate, the open holes.
    lines.append("// Fingerings by written note: the holes open, standard and alternate.")
    lines.append("let fingerings = [")
    for key in RANGE:
        std = sorted(fingering(key, 0), key=HOLE_NAMES.index)
        alt = sorted(fingering(key, 1), key=HOLE_NAMES.index) if has_alternate(key) else None
        alt_s = "null" if alt is None else "[" + ", ".join(f'"{n}"' for n in alt) + "]"
        lines.append(f"    {{written: {key}, mode: {mode(key)}, open: [" + ", ".join(f'"{n}"' for n in std) + f"], alternate: {alt_s}}},")
    lines.append("];")
    GEOMETRY_MUZ.write_text("\n".join(lines) + "\n")


# --- Playing: the fingering's own pitch, and the tuned result ---------------------

MUZ = ROOT / "target/release/muz"
DYNAMICS = (0.3, 0.45, 0.6)
NOTE_SECONDS = 0.6
TUNING_MUZ = PROTO / "clarinet_tuning.muz"


def played_notes():
    """(sounding key, written key, variant) for every fingering in range."""
    out = [(k - 2, k, 0) for k in RANGE]
    out += [(k - 2, k, 1) for k in RANGE if has_alternate(k)]
    return out


def song_source(notes, dynamic, lattice):
    use = (PROTO / "lattice-clarinet.muz").as_posix()
    body = ",\n    ".join(
        f"note({s}, {NOTE_SECONDS}b, at = {i * (NOTE_SECONDS + 0.15):.2f}b).gate(1)"
        f".express({{pressure: {dynamic}, tongue: 0, fingering: {v}}})"
        for i, (s, w, v) in enumerate(notes)
    )
    return (
        f'use "{use}" as lc;\nuse "std/signal" as s;\n'
        f"let line = stack([\n    {body}\n]);\n"
        f'song({{tempo: 60, tracks: [track("cl", line, lc.clarinet({{velocity_breath: 0, lattice: {lattice}}}))]}})\n'
    )


def dft_amplitude(x, rate, f):
    n = np.arange(len(x))
    w = np.hanning(len(x))
    return abs(np.sum(x * w * np.exp(-2j * np.pi * f * n / rate))) / len(x)


def measure_pitch(x, rate, nominal, span=150):
    """Cents from `nominal` of the strongest partial within `span` cents,
    and the level of the twelfth below relative to it (a wrong register)."""
    coarse = np.arange(-span, span + 1, 2.0)
    a = [dft_amplitude(x, rate, nominal * 2 ** (c / 1200)) for c in coarse]
    c0 = coarse[int(np.argmax(a))]
    fine = np.arange(c0 - 2, c0 + 2.01, 0.1)
    b = [dft_amplitude(x, rate, nominal * 2 ** (c / 1200)) for c in fine]
    c = float(fine[int(np.argmax(b))])
    level = max(b)
    under = dft_amplitude(x, rate, nominal / 3)
    return c, 20 * math.log10(level + 1e-12), 20 * math.log10((under + 1e-12) / (level + 1e-12))


def render_dynamics(notes, lattice, rate=48_000):
    """Render `notes` at each of DYNAMICS in parallel; return {dynamic: rows}."""
    import subprocess
    import tempfile

    import soundfile as sf

    results = {}
    with tempfile.TemporaryDirectory(prefix="muz-clarinet-tune-") as scratch:
        procs = []
        for d in DYNAMICS:
            src = Path(scratch) / f"tune-{d}.muz"
            out = Path(scratch) / f"tune-{d}.wav"
            src.write_text(song_source(notes, d, lattice))
            procs.append((d, out, subprocess.Popen(
                [str(MUZ), "render", str(src), "-o", str(out), "--sample-rate", str(rate)],
                stdout=subprocess.DEVNULL)))
        for d, out, p in procs:
            if p.wait() != 0:
                raise SystemExit(f"render failed at dynamic {d}")
            audio, actual = sf.read(out, always_2d=True)
            x = audio[:, 0]
            rows = []
            for i, (s, w, v) in enumerate(notes):
                start = i * (NOTE_SECONDS + 0.15)
                seg = x[int((start + 0.3) * actual):int((start + 0.55) * actual)]
                rows.append(measure_pitch(seg, actual, 440 * 2 ** ((s - 69) / 12)))
            results[d] = rows
    return results


def tune(scale, reed_mm):
    notes = played_notes()
    write_tuning(scale, reed_mm, [])
    measured = render_dynamics(notes, "{tuned: false}")
    natural = []
    print(f"{'note':8s} " + " ".join(f"{d:>14}" for d in DYNAMICS))
    for i, (s, w, v) in enumerate(notes):
        c = [measured[d][i][0] for d in DYNAMICS]
        x = np.array(DYNAMICS) - DYNAMICS[1]
        coef = np.polyfit(x, c, 2)[::-1]
        natural.append((w, v, coef))
        flags = " ".join(f"{m[0]:+7.1f} ({m[2]:+5.0f})" for m in (measured[d][i] for d in DYNAMICS))
        print(f"{written_name(w) + ('*' if v else ''):8s} {flags}")
    all_c = np.array([[measured[d][i][0] for d in DYNAMICS] for i in range(len(notes))])
    print(f"natural pitch: mean {all_c.mean():+.1f} cents, spread {all_c.min():+.1f}..{all_c.max():+.1f}")
    write_tuning(scale, reed_mm, natural)


def write_tuning(scale, reed_mm, natural):
    lines = [
        "// Generated by tools/clarinet-lattice.py tune: the pitch each fingering",
        "// sounds with the embouchure still, in cents from its written note, as",
        "// cents[0] + cents[1] x + cents[2] x^2 at dynamic `reference` + x.",
        "// `scale` stretches the bore's delays and `reed_mm` lengthens the",
        "// mouthpiece by the reed's equivalent volume when it is played.",
        f"let reference = {DYNAMICS[1]};",
        f"let scale = {muz_number(scale)};",
        f"let reed_mm = {muz_number(reed_mm)};",
        "let natural = [",
    ]
    for w, v, coef in natural:
        lines.append(f"    {{written: {w}, variant: {v}, cents: [{', '.join(muz_number(round(c, 3)) for c in coef)}]}},")
    lines.append("];")
    TUNING_MUZ.write_text("\n".join(lines) + "\n")


def check_tuning():
    notes = played_notes()
    measured = render_dynamics(notes, "{}")
    worst = []
    print(f"{'note':8s} " + " ".join(f"{d:>14}" for d in DYNAMICS))
    for i, (s, w, v) in enumerate(notes):
        row = [measured[d][i] for d in DYNAMICS]
        worst.extend(abs(m[0]) for m in row)
        print(f"{written_name(w) + ('*' if v else ''):8s} " + " ".join(f"{m[0]:+7.1f} ({m[2]:+5.0f})" for m in row))
    worst = np.array(worst)
    print(f"tuned: mean |err| {worst.mean():.1f} cents, max {worst.max():.1f}, within 5 cents {np.mean(worst <= 5):.0%}")


# Slurs by written note: across the break both ways, and within each register.
SLURS = [
    ("A4-B4", 69, 71), ("B4-A4", 71, 69), ("G4-D5", 67, 74), ("D5-G4", 74, 67),
    ("B4-G4", 71, 67), ("C4-E4", 60, 64), ("E4-C4", 64, 60), ("D5-F5", 74, 77), ("F5-D5", 77, 74),
]
SLUR_DYNAMICS = (0.3, 0.55)
KEY_TIMES = (0.4, 1.0)
SLUR_GAP = 1.2


def transitions_source(model):
    use = (PROTO / "lattice-clarinet.muz").as_posix()
    notes = []
    i = 0
    for d in SLUR_DYNAMICS:
        for kt in KEY_TIMES:
            for _, a, b in SLURS:
                t = i * SLUR_GAP
                e = f"{{pressure: {d}, tongue: 0, key_time: {kt}}}"
                notes.append(f"note({a - 2}, 0.53b, at = {t:.2f}b).gate(1).express({e})")
                notes.append(f"note({b - 2}, 0.5b, at = {t + 0.5:.2f}b).gate(1).express({e})")
                i += 1
    line = ",\n    ".join(notes)
    return (f'use "{use}" as lc;\nuse "std/woodwinds" as w;\n'
            f"let line = stack([\n    {line}\n]);\n"
            f'song({{tempo: 60, tracks: [track("cl", line, {model})]}})\n')


def transitions(model):
    import subprocess
    import tempfile

    import soundfile as sf

    with tempfile.TemporaryDirectory(prefix="muz-clarinet-slurs-") as scratch:
        src = Path(scratch) / "slurs.muz"
        out = Path(scratch) / "slurs.wav"
        src.write_text(transitions_source(model))
        subprocess.run([str(MUZ), "render", str(src), "-o", str(out)], check=True, stdout=subprocess.DEVNULL)
        audio, rate = sf.read(out, always_2d=True)
    x = audio[:, 0]
    win = int(0.015 * rate)
    hop = int(0.0025 * rate)
    i = 0
    print(f"{'slur':7s} {'dyn':>4s} {'keys':>4s} {'settle ms':>9s} {'dip dB':>7s} {'f/3 dB':>7s} {'3f dB':>6s} {'cents':>6s}")
    rows = []
    for d in SLUR_DYNAMICS:
        for kt in KEY_TIMES:
            for name, a, b in SLURS:
                tc = i * SLUR_GAP + 0.5
                i += 1
                fb = 440 * 2 ** ((b - 2 - 69) / 12)
                steady = x[int((tc + 0.25) * rate):int((tc + 0.45) * rate)]
                a_ss = dft_amplitude(steady, rate, fb)
                h3_ss = dft_amplitude(steady, rate, 3 * fb)
                rms_after = math.sqrt(np.mean(steady ** 2))
                before = x[int((tc - 0.2) * rate):int((tc - 0.02) * rate)]
                rms_before = math.sqrt(np.mean(before ** 2))
                pitch = measure_pitch(steady, rate, fb, span=60)[0]
                times, fund, rms, low, high = [], [], [], [], []
                for s0 in range(int((tc - 0.01) * rate), int((tc + 0.2) * rate), hop):
                    seg = x[s0:s0 + win]
                    times.append((s0 + win / 2) / rate - tc)
                    fund.append(dft_amplitude(seg, rate, fb))
                    rms.append(math.sqrt(np.mean(seg ** 2)))
                    low.append(dft_amplitude(seg, rate, fb / 3))
                    high.append(dft_amplitude(seg, rate, fb * 3))
                times, fund, rms, low, high = map(np.array, (times, fund, rms, low, high))
                ok = fund >= 0.8 * a_ss
                settle = None
                for k in range(len(ok)):
                    if ok[k:].all():
                        settle = max(times[k], 0.0)
                        break
                end = settle if settle is not None else 0.2
                span = (times >= 0) & (times <= max(end, 0.01))
                dip = 20 * math.log10(rms[span].min() / min(rms_before, rms_after)) if span.any() else 0.0
                # Stray modes after the change: the twelfth below the new note
                # against its steady fundamental (the wrong register), and its
                # third partial against that partial's steady level (a burst of
                # the third mode).
                after = times >= 0.0
                low_db = 20 * math.log10(low[after].max() / a_ss)
                high_db = 20 * math.log10(high[after].max() / h3_ss)
                rows.append((name, d, kt, settle, dip, low_db, high_db, pitch))
                s_ms = "  never" if settle is None else f"{settle * 1000:9.1f}"
                print(f"{name:7s} {d:4.2f} {kt:4.1f} {s_ms:>9s} {dip:7.1f} {low_db:7.1f} {high_db:6.1f} {pitch:+6.1f}")
    return rows


# The rendered lattice against the reference: examples/woodwind-prototypes/
# passive-clarinet.muz probed by tools/woodwind-passive.py.
PASSIVE_TRACKS = {
    "e3": "E3", "g3": "G3", "c4": "C4", "e4": "E4", "fs4": "F#4", "g4": "G4", "as4": "A#4",
    "b4": "B4", "d5": "D5", "g5": "G5", "c6": "C6", "cs6": "C#6", "f6": "F6",
    "ds4_alt": "D#4alt2", "as4_alt": "A#4alt",
}


def passive(ref):
    import subprocess
    import tempfile
    from concurrent.futures import ThreadPoolExecutor

    source = PROTO / "passive-clarinet.muz"
    with tempfile.TemporaryDirectory(prefix="muz-clarinet-passive-") as scratch:
        def run(track):
            out = Path(scratch) / f"{track}.json"
            subprocess.run(["uv", "run", "-q", str(ROOT / "tools/woodwind-passive.py"), str(source),
                            "--track", track, "--min-hz", "120", "--max-hz", "2000", "--seconds", "1",
                            "--json", str(out)], check=True, stdout=subprocess.DEVNULL)
            return track, json.loads(out.read_text())

        with ThreadPoolExecutor(8) as pool:
            measured = dict(pool.map(run, PASSIVE_TRACKS))
    errors, widths = [], []
    print(f"{'fingering':10s} " + "   ".join(f"{'peak' + str(k):>7s} {'ref':>7s} {'cents':>6s} {'width':>5s}" for k in (1, 2)))
    for track, name in PASSIVE_TRACKS.items():
        want = ref[name]["peaks"][:2]
        # Keep resonances, not the ripple of the pulse's spectrum.
        got_all = [{"hz": r["hz"], "width": r["width_hz"], "z": r["normalized_impedance"]}
                   for r in measured[track]["resonances"] if r["normalized_impedance"] > 3]
        got = match(got_all, want)
        cells = []
        for w, m in zip(want, got):
            c = cents(m["hz"], w["hz"])
            ratio = m["width"] / w["width"] if m["width"] and w["width"] else float("nan")
            errors.append(abs(c))
            if not math.isnan(ratio):
                widths.append(ratio)
            cells.append(f"{m['hz']:7.1f} {w['hz']:7.1f} {c:+6.1f} {ratio:5.2f}")
        print(f"{name:10s} " + "   ".join(cells))
    e = np.array(errors)
    w = np.array(widths)
    print(f"first two peaks: mean |err| {e.mean():.1f} cents, max {e.max():.1f}, within 10 cents {np.mean(e <= 10):.0%}")
    print(f"width ratio rendered/reference: median {np.median(w):.2f}, range {w.min():.2f}-{w.max():.2f}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("peaks")
    p.add_argument("xlsx", type=Path)
    p = sub.add_parser("fit")
    p.add_argument("--fresh", action="store_true", help="start from the built-in seed")
    p.add_argument("--stage", choices=["geometry", "loss", "both", "joint"], default="both")
    p.add_argument("--max-nfev", type=int, default=60)
    sub.add_parser("report", help="the fitted model's peaks against the reference")
    sub.add_parser("passive", help="the rendered lattice's peaks against the reference")
    sub.add_parser("muz")
    p = sub.add_parser("tune", help="measure each fingering's own pitch -> clarinet_tuning.muz")
    p.add_argument("--scale", type=float, default=1.0)
    p.add_argument("--reed-mm", type=float, default=0.0)
    sub.add_parser("check", help="the tuned instrument's pitch at three dynamics")
    p = sub.add_parser("slurs", help="settle time, dip and stray modes over a grid of slurs")
    p.add_argument("--model", default="lc.clarinet({velocity_breath: 0})",
                   help="instrument expression (lc: the lattice clarinet, w: std/woodwinds)")
    args = parser.parse_args()
    if args.command == "slurs":
        transitions(args.model)
        return
    if args.command == "tune":
        tune(args.scale, args.reed_mm)
        return
    if args.command == "check":
        check_tuning()
        return

    if args.command == "peaks":
        ref = reference_peaks(args.xlsx)
        REFERENCE.write_text(json.dumps(ref, indent=1) + "\n")
        print(f"wrote {len(ref)} fingerings to {REFERENCE}")
        return
    ref = json.loads(REFERENCE.read_text())
    geom = initial_geometry() if (args.command == "fit" and args.fresh) or not GEOMETRY_JSON.exists() else json.loads(GEOMETRY_JSON.read_text())
    geom["loss"].setdefault("register", geom["loss"]["hole"])
    if args.command == "fit":
        if args.stage in ("geometry", "both"):
            geom = fit(geom, ref, {"geometry"}, widths=False, max_nfev=args.max_nfev)
        if args.stage in ("loss", "both"):
            geom = fit(geom, ref, {"loss"}, widths=True, max_nfev=args.max_nfev)
        if args.stage == "joint":
            geom = fit(geom, ref, {"geometry", "loss"}, widths=True, max_nfev=args.max_nfev)
        GEOMETRY_JSON.write_text(json.dumps(geom, indent=1) + "\n")
        write_muz(geom)
        report(geom, ref)
    elif args.command == "report":
        report(geom, ref)
    elif args.command == "passive":
        passive(ref)
    elif args.command == "muz":
        write_muz(geom)


if __name__ == "__main__":
    main()
