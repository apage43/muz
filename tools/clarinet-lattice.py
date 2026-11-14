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
examples/woodwind-prototypes/clarinet_bore.muz builds, element by element,
as the discrete-time system the renderer runs at RATE: each delay line is a
third-order Lagrange fractional delay and each one-pole and state-variable
filter trapezoidal, prewarped at its cutoff (`impedance(..., rate=None)` gives the continuous
elements):

  bore      cylinder segments of radius `BORE` between holes, as pure delays
            with a round-trip gain exp(-2 beta l) per segment, and a loss shelf
            (1-k) + k lowpass1(f_shelf) on the mouthpiece segment
  hole      a shunt junction between two segments: the open path (inertance
            and resistance) in parallel with the chimney's compliance, whose
            admittance over two is A / (rho + s) + s T. Its pressure is
            p = (s + rho) / (T s^2 + (1 + rho T) s + rho + A) (a + b), exactly
            p = (1 - H_open)(1 - H_closed)(a + b) for incoming waves a, b, with
            H_open = d lowpass2(w_0, q)   (the open path's release)
            H_closed = kappa highpass1(f_c) (closed chimney volume: compliance)
            A = g alpha / 2, d = A / (rho + A), w_0 = sqrt((rho + A) / T),
            q = w_0 T / (1 + rho T), T = tau_c / 2,
            alpha = (b/a)^2 c / t_e, tau_c = (b/a)^2 t / c, kappa = tau_c w_c / 2,
            w_c = min(2 / tau_c, the corner of CLOSED_HP), and f_c the cutoff
            whose prewarped one-pole is the bilinear transform at w_c
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
import os
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
# CLARINET_DIR plays and writes a copy of the prototype directory instead
# (a candidate geometry, its generated muz and tuning), so several
# candidates can be played side by side.
PROTO = Path(os.environ.get("CLARINET_DIR", ROOT / "examples/woodwind-prototypes")).resolve()
REFERENCE = ROOT / "examples/woodwind-prototypes/clarinet-reference.json"
# The reference's measured impedance in bands around its first peaks (`bands`).
REFERENCE_BANDS = ROOT / "examples/woodwind-prototypes/clarinet-reference-bands.json"
GEOMETRY_JSON = PROTO / "clarinet-geometry.json"
GEOMETRY_MUZ = PROTO / "clarinet_geometry.muz"

C = 345.0  # m/s, warm dry air; positions are effective, so c is a convention
BORE = 7.3e-3  # m, bore radius
# The lattice runs at 2x oversampling of a 48 kHz render. The model below is
# the discrete-time system at that rate: Lagrange delays and trapezoidal
# one-poles, as src/audio/device/patch.rs runs them.
RATE = 96_000.0
# Highest cutoff of the closed-hole compliance highpass, as the renderer's
# parameter. Higher corners make the compliance depend on the sample rate
# (the trapezoidal one-pole is prewarped at its cutoff): at 12 kHz it changes
# by about 1% between 88.2 and 96 kHz.
CLOSED_HP = 12_000.0
# Nonlinear (jet) resistance of a narrow hole: the flow separates into a jet,
# a pressure drop rho C v |v| for the velocity v in the hole, a resistance
# R = C rho |v| / S_h. Dalmont et al. (2002) measured K = 0.5 for the
# resistance against the velocity amplitude of a sharp-edged hole, which is
# C = 3 pi K / 8 = 0.59 for the instantaneous velocity; Szwarcberg, Colinot
# et al. (2025) take C = 0.70 for the register hole's sharp edges. In the
# model's units (pressure over the unit P_CLOSE, flow as a pressure wave) it
# adds 2 C P_CLOSE / (rho c^2) (S_bore / S_hole)^2 |leak|
# to the open path's resistance over alpha. Only the register hole, whose
# velocity is highest, carries it in the prototype. The resistance goes as
# S_hole^-2: the fitted register tube is an effective 1.32 mm in radius
# against the real tube's 1.47-1.5 mm, and C = 0.36 on it is the jet that
# C = 0.6 gives the real tube (a refit with the real tube, 13 mm long and
# projecting into the bore, lands on the same coefficient).
JET_K = 0.36
P_CLOSE = 5000.0  # Pa: the model's pressure unit (lattice-clarinet.muz's `unit_kpa`)
RHO = 1.2

NOTE = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]


def written_name(key):
    k = int(key)
    return f"{NAMES[k % 12]}{k // 12 - 1}" + ("+" if key != k else "")


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
# hole on a Boehm clarinet. The order follows a measured Yamaha YCL-250
# (Moers & Kergomard, "On the cutoff frequency of clarinet-like instruments",
# arXiv:1101.4742, appendix A): the side trill holes lie above the A key; the
# open G sounds through two holes, the thumb hole and a vent above it; and E4
# through two, the LH2 hole and the sliver vent above it.
#
# Ring vents are pads that two rings hold shut, so they open only with both
# fingers up (RING_VENTS). `v` opens for G4 and the throat notes above it.
# Without it the thumb hole alone has to serve F#4 (thumb open, LH1 down) and
# G4 (both open), and their interval comes out 40 cents instead of a
# semitone. `sl`, the sliver, opens from E4 up and stays shut in the forked
# Eb4/Bb5 (LH2 up, LH3 down), which is otherwise 15 cents sharp. The lattice
# holds only the holes in the geometry; RING_VENT adds or removes the vents.
HOLES = [
    ("reg", "register", "register (speaker) key, thumb"),
    ("tr", "closed", "upper side trill key, RH"),
    ("a", "closed", "throat A key, LH1"),
    ("gs", "closed", "throat G# key, LH1"),
    ("v", "open", "vent closed by the thumb and LH1 rings"),
    ("t", "finger", "thumb hole"),
    ("l1", "finger", "LH1 ring"),
    ("sl", "open", "sliver vent closed by the LH2 and LH3 rings"),
    ("l2", "finger", "LH2 ring"),
    ("eb", "closed", "Eb/Bb side key"),
    ("l3", "finger", "LH3"),
    ("cs", "closed", "C#/G# key"),
    ("r1", "finger", "RH1 ring"),
    ("bb", "closed", "B/F# sliver key, RH"),
    ("r2", "finger", "RH2 ring"),
    ("r3", "finger", "RH3"),
    ("gsds", "closed", "G#/D# key, RH little finger"),
    ("fscs", "open", "F#/C# key, little finger"),
    ("fc", "open", "F/C key, little finger"),
    ("eb_low", "open", "E/B key, little finger"),
]
HOLE_NAMES = [h[0] for h in HOLES]
RING_VENTS = {"v": ("t", "l1"), "sl": ("l2", "l3")}
# `fit` adds (True, or a set of names) or removes (False) the ring vents, or
# keeps the geometry's holes (None). clarinet_bore.muz moves only the holes
# in its `key_groups`, so a lattice with a vent needs its name there.
RING_VENT = None
FINGERS = ["t", "l1", "l2", "l3", "r1", "r2", "r3"]
OPEN_STANDING = ["fscs", "fc", "eb_low"]


def chalumeau(fingers, keys=(), pressed=()):
    """Open holes for `fingers` down, closed-standing `keys` opened, and
    open-standing `pressed` keys closed."""
    opened = set(keys)
    opened |= {f for f in FINGERS if f not in fingers}
    opened |= {k for k in OPEN_STANDING if k not in pressed}
    opened |= {v for v, rings in RING_VENTS.items() if not set(rings) & set(fingers)}
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
    58: chalumeau(["t", "l1", "l2", "l3", "r1"]),
    # UNSW measured B3 as the cross fingering T123|-2-. With RH1 up and RH2
    # down it sounds a semitone below C4 because the RH rings shut a vent
    # above RH1 (the YCL-250's hole 9), which the lattice does not have.
    # The chromatic B3, RH1 with the B/F# sliver key, needs no vent.
    59: chalumeau(["t", "l1", "l2", "l3", "r1"], keys=("bb",)),
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
# UNSW's alternate A#4 is the A key with the second side key and no
# register key (the A4 trill fingering). The forked Bb3/F5 closes RH1 and
# RH3 over the open RH2 hole; UNSW did not measure it.
ALTERNATES = {
    63: ("fork", chalumeau(["t", "l1", "l3", "r1"])),
    70: ("side", chalumeau([], keys=("gs", "a", "tr"))),
}
# Unmeasured alternates that `fit` holds to their standard fingering's peaks,
# with a weight.
ALTERNATE_TARGETS = {}


# The throat notes as played: the player keeps the right hand and little
# fingers down ("right hand down"), so a slur across the break moves only the
# left hand, the thumb and the throat keys. Below the open throat holes this
# hardly moves the pitch; the measured reference has the right hand up, so
# fitting uses the standard chart (and holds the played throat notes near
# it, `PLAYED_WEIGHT`) and the generated muz the played one. On a bore whose
# closed lower joint resonates near open G's second peak (the full refit
# with a flare tried in this round), the little finger splits that peak in
# two, and G4 would want the little finger up.
RIGHT_HAND_DOWN = ("r1", "r2", "r3", "bb", "gsds", "fscs", "fc", "eb_low")
THROAT_RH = (67, 70)


# Open G and G# leave the little finger up (its keys open), as D5 and D#5
# above them want: with it down, the closed lower joint resonates near open
# G's second peak and splits it in two. A and B-flat keep it down, as B4
# and C5 want.
LITTLE_FINGER = ("fscs", "fc", "eb_low")
# Off for the committed geometry, which has no split at open G.
LITTLE_UP = False
# The notes that leave the little finger free when LITTLE_UP: its keys stay
# as the previous note left them (clarinet_bore.muz's `little_hold`).
LITTLE_FREE = (67, 68)


# A geometry carries the player settings it was played with, under
# "player": the register tube's jet coefficient (`JET_K`), whether open G
# and G# leave the little finger up (`LITTLE_UP`), and how far the register
# key leads an ascent across the break, by how much of its travel
# (`vent_crack`) and how many milliseconds ahead (`vent_lead`), written to
# the generated muz for lattice-clarinet.muz's `vented` rule.
def apply_player(geom):
    player = geom.get("player", {})
    globals()["JET_K"] = player.get("jet_k", 0.36)
    globals()["LITTLE_UP"] = player.get("little_up", False)
    return geom


def played(key, variant=0):
    """The open holes the player fingers for written `key`."""
    holes = fingering(key, variant)
    if THROAT_RH[0] <= key <= THROAT_RH[1]:
        holes -= set(RIGHT_HAND_DOWN) - (set(LITTLE_FINGER) if key in LITTLE_FREE and LITTLE_UP else set())
    return holes


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
    if key <= 89:  # altissimo: the fifth harmonic, LH1 lifted as a vent;
        # from D6 up the G#/D# key vents the lower bore too (as UNSW fingers it)
        return chal(key - 28) | {"reg", "l1"} | ({"gsds"} if key >= 86 else set())
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

def inner_correction(b, bore=BORE):
    d = b / bore
    return b * (0.82 - 0.193 * d - 1.09 * d**2 + 1.27 * d**3 - 0.71 * d**4)


def series_correction(b, bore=BORE):
    d = b / bore
    return b * (-0.37 + 0.087 * d) * d**2


# The register hole is a metal tube through the wall that also projects into
# the bore (on a Buffet Festival about 13 mm long, 3.0 mm bore, Colinot et
# al. 2026; on a Yamaha YCL-250 12.5 mm and 1.5 mm radius, Moers & Kergomard
# 2011). Its inner end is an unflanged tube end in the bore (end correction
# 0.61 b, Levine & Schwinger) rather than a flush hole's, and the projecting
# tube's wall (TUBE_WALL thick) displaces air from the bore, which takes its
# volume off the closed chimney's compliance.
TUBE_WALL = 0.4e-3
UNFLANGED = 0.6133


def lumped(hole):
    """Lumped parameters of one hole record {position, radius, chimney, pad},
    relative to the local bore (`bore`, the junction's mean radius)."""
    b, t = hole["radius"], hole["chimney"]
    bore = hole.get("bore", BORE)
    area = (b / bore) ** 2
    projecting = hole.get("protrusion", 0.0)
    if projecting > 0:
        te = t + UNFLANGED * b + 0.75 * b
        volume = b * b * t - ((b + TUBE_WALL) ** 2 - b * b) * projecting
    else:
        te = t + inner_correction(b, bore) + 0.75 * b
        volume = b * b * t
    alpha = area * C / te
    tau = volume / bore**2 / C
    # The compliance's exact release is a highpass at 2 / tau of gain 1; where
    # that corner is above the highest one (small holes) it is lowered there
    # with the gain that keeps its low-frequency slope, tau / 2. The gain is
    # then below 1 and the element stays passive. The renderer's one-pole is
    # trapezoidal and prewarped at its cutoff, so the cutoff it is given,
    # `closed_hz`, is the one whose filter is the plain bilinear transform of
    # the analog corner `closed_w`: exact at low frequencies, at RATE.
    wc = min(2 * RATE * math.tan(math.pi * CLOSED_HP / RATE), 2 / tau)
    return {
        "alpha": alpha,
        # The chimney's compliance as a time constant: its release alone is
        # 1 / (1 + s compliance_t).
        "compliance_t": tau / 2,
        "closed_w": wc,
        "closed_hz": RATE / math.pi * math.atan(wc / (2 * RATE)),
        "kappa": tau * wc / 2,
        "nonlinear": 2 * JET_K * P_CLOSE / (RHO * C**2) / area**2 * (BORE / bore) ** 2
        if hole.get("kind") == "register" else 0.0,
        "series": series_correction(b, bore),
        "effective_length": te,
    }


def opening(x, q):
    """Open-path admittance fraction at key travel x (0 shut .. 1 open)."""
    x = np.clip(x, 0.0, 1.0)
    if q is None:
        return x
    return (1 + q) * x / (x + q)


# The viscous resistance of a hole's open path over its inertance, R / M =
# w delta / b, at VISCOUS_HZ (delta the viscous boundary layer): w delta.
VISCOUS_HZ = 400.0
VISCOUS = math.sqrt(2 * 1.5e-5 * 2 * math.pi * VISCOUS_HZ)  # m/s


def hole_resistance(hole, loss):
    """The open path's resistance over its inertance, per alpha. A tone
    hole's is viscous, R / M = `hole` w delta / b, so a narrow hole is the
    lossier; the register tube has its own."""
    if hole["kind"] == "register":
        return max(loss["register"], 0.001)
    return max(loss["hole"] * VISCOUS / (hole["radius"] * lumped(hole)["alpha"]), 0.001)


# The bore's profile. Segment 0 is the mouthpiece's chamber, `geom["mouthpiece"]`
# long: under the baffle its section is narrower than the bore's. Segment 1
# runs on to the register hole at radius BORE, which normalizes every
# impedance. The main bore is cylindrical between steps: below each hole
# named in STEP_HOLES its radius changes to `geom["steps"][name]` (the upper
# joint's slight reverse taper and the lower joint's widening, as a
# polycylinder). Below the main bore the bell flares through `geom["flare"]`,
# cylinders of growing radius. A step scatters with the reflection
# (S_above - S_below) / (S_above + S_below), and a hole at a step sees the
# junction's mean area. Junction j joins segments j - 1 and j: junction 1
# is the mouthpiece's step, junctions 2 .. n + 1 the holes, then the flare's.
STEP_HOLES = ("t", "cs", "gsds")


def segment_radii(geom):
    """Radius of every segment, reed plane to bell: the mouthpiece, n + 1
    main segments, then the flare."""
    steps = geom.get("steps", {})
    r = BORE
    out = [geom.get("mouthpiece", {}).get("radius", BORE), r]
    for h in geom["holes"]:
        r = steps.get(h["name"], r)
        out.append(r)
    return out + [f["radius"] for f in geom.get("flare", [])]


def annotate(geom):
    """Give each hole its local bore radius (the junction's mean area)."""
    radii = segment_radii(geom)
    for k, h in enumerate(geom["holes"]):
        h["bore"] = math.sqrt((radii[k + 1] ** 2 + radii[k + 2] ** 2) / 2)
    return geom


def junction_hole(geom, j):
    """The hole at junction j (between segments j - 1 and j), or None."""
    k = j - 2
    return geom["holes"][k] if 0 <= k < len(geom["holes"]) else None


def step_reflection(above, below):
    """Reflection of a pressure wave arriving from radius `above` at a step to `below`."""
    return (above**2 - below**2) / (above**2 + below**2)


def segments(geom):
    """Segment lengths in metres, reed plane to bell, after series
    corrections: the mouthpiece, the main bore's n + 1, then the flare's."""
    annotate(geom)
    holes = geom["holes"]
    chamber = geom.get("mouthpiece", {}).get("length", 0.0)
    ends = [0.0, chamber] + [h["position"] for h in holes] + [geom["bell"]]
    lengths = np.diff(ends)
    corrections = [lumped(h)["series"] for h in holes]
    out = lengths.copy()
    for k, ta in enumerate(corrections):
        out[k + 1] += ta / 2  # ta is negative: the hole shortens the bore
        out[k + 2] += ta / 2
    return np.concatenate([out, [f["length"] for f in geom.get("flare", [])]])


def lowpass1(cutoff_hz, freqs, rate=RATE):
    """Response of muz's one-pole lowpass: trapezoidal, prewarped at its
    cutoff, which the renderer clamps to 10 Hz .. 0.45 rate. `rate` None is
    the analog filter."""
    if rate is None:
        return 1 / (1 + 1j * freqs / cutoff_hz)
    cutoff_hz = np.clip(cutoff_hz, 10.0, 0.45 * rate)
    return 1 / (1 + 1j * np.tan(np.pi * freqs / rate) / np.tan(np.pi * cutoff_hz / rate))


def svf_lowpass(cutoff_hz, q, freqs, rate=RATE):
    """Response of muz's state-variable lowpass (`s.filter`, "lowpass"):
    trapezoidal, prewarped at its cutoff, which the renderer clamps to
    10 Hz .. 0.45 rate, and its q to 0.01 .. 100. `rate` None is the analog
    filter."""
    if rate is None:
        u = 1j * freqs / cutoff_hz
    else:
        cutoff_hz = np.clip(cutoff_hz, 10.0, 0.45 * rate)
        u = 1j * np.tan(np.pi * freqs / rate) / np.tan(np.pi * cutoff_hz / rate)
    return 1 / (1 + u / np.clip(q, 0.01, 100.0) + u * u)


def open_path(alpha, rho, g, compliance_t):
    """The open path's filter: the cutoff (Hz), q and depth of the
    second-order lowpass whose output is the open path's pressure release
    in parallel with the chimney's compliance (see `impedance`)."""
    a = g * alpha / 2
    w0 = np.sqrt((rho + a) / compliance_t)
    return w0 / (2 * np.pi), w0 * compliance_t / (1 + rho * compliance_t), a / (rho + a)


def line_delay(seconds, freqs, rate=RATE):
    """Response of a muz cubic delay line (`s.line`): third-order Lagrange
    over the samples written whole-1 .. whole+2 frames ago, at least two
    samples."""
    if rate is None:
        return np.exp(-2j * np.pi * freqs * seconds)
    offset = max(seconds * rate, 2.0)
    whole = math.floor(offset)
    f = offset - whole
    weights = (-f * (f - 1) * (f - 2) / 6, (f + 1) * (f - 1) * (f - 2) / 2,
               -(f + 1) * f * (f - 2) / 2, (f + 1) * f * (f - 1) / 6)
    z = np.exp(-2j * np.pi * freqs / rate)
    return sum(wt * z ** (whole - 1 + i) for i, wt in enumerate(weights))


# Wall loss is lumped: the round-trip gain of each run of LOSS_GROUP segments
# below the mouthpiece is applied once, at the top segment of the run (the
# rest carry gain 1), which saves the renderer a multiply per segment. The
# loss per segment is under 0.5%, so this moves no peak measurably.
LOSS_GROUP = 5


def segment_gains(geom):
    """Round-trip wall-loss gain applied on each segment, lumped by LOSS_GROUP."""
    lengths = segments(geom)
    radii = segment_radii(geom)
    # Wall loss per length goes as 1 / radius.
    raw = [math.exp(-2 * geom["loss"]["beta"] * l * BORE / r) for l, r in zip(lengths, radii)]
    out = [raw[0]] + [1.0] * (len(raw) - 1)
    for start in range(1, len(raw), LOSS_GROUP):
        out[start] = float(np.prod(raw[start:start + LOSS_GROUP]))
    return out


def impedance(geom, states, freqs, rate=RATE):
    """Normalized input impedance at the reed plane for hole travels `states`:
    a dict name -> 0..1, or an array [fingering, hole] in HOLE_NAMES order,
    at `freqs` (one grid, or a row of frequencies per fingering).
    At `rate` it is the discrete-time lattice the renderer runs; `rate` None
    gives the continuous-time elements."""
    if isinstance(states, dict):
        states = np.array([[states.get(n, 0.0) for n in HOLE_NAMES]])
        return impedance(geom, states, freqs, rate)[0]
    states = np.asarray(states, dtype=float)
    loss = geom["loss"]
    lengths = segments(geom)
    holes = geom["holes"]
    # `freqs` is one grid for every fingering, or a row per fingering.
    R = np.broadcast_to(-loss["bell"] * lowpass1(loss["bell_hz"], freqs, rate) ** 2,
                        np.broadcast_shapes((len(states), 1), np.shape(freqs))).copy()
    gains = segment_gains(geom)
    radii = segment_radii(geom)
    for k in range(len(lengths) - 1, -1, -1):
        ell = lengths[k]
        R *= gains[k] * line_delay(ell / C, freqs, rate) ** 2
        if k == 0:
            R *= (1 - loss["shelf"]) + loss["shelf"] * lowpass1(loss["shelf_hz"], freqs, rate)
            continue
        # The junction above segment k: its pressure is
        # p = P (a + b + s (a - b)) for the wave a arriving from above and b
        # from below, s the step's reflection and P the hole's shunt.
        s = step_reflection(radii[k - 1], radii[k])
        h = junction_hole(geom, k)
        if h is not None:
            el = lumped(h)
            g = opening(states[:, HOLE_NAMES.index(h["name"])], h.get("pad"))[:, None]
            rho = hole_resistance(h, loss) * el["alpha"]
            hz, q, depth = open_path(el["alpha"], rho, g, el["compliance_t"])
            Ho = depth * svf_lowpass(hz, q, freqs, rate)
            if rate is None:
                Hc = el["kappa"] * (1 - lowpass1(el["closed_w"] / (2 * np.pi), freqs, None))
            else:
                Hc = el["kappa"] * (1 - lowpass1(el["closed_hz"], freqs, rate))
            P = (1 - Ho) * (1 - Hc)
        else:
            P = 1.0
        # With b = R (p - b): p / a = P (1 + s)(1 + R) / (1 + R - P (1 - s) R).
        R = P * (1 + s) * (1 + R) / (1 + R - P * (1 - s) * R) - 1
    return (1 + R) / (1 - R)


def states_for(key, variant=0):
    opened = fingering(key, variant)
    return {name: (1.0 if name in opened else 0.0) for name in HOLE_NAMES}


def travel_for(g, pad):
    """Key travel at which the open path carries admittance fraction g."""
    return g if pad is None else pad * g / (1 + pad - g)


def sounding_peak(geom, states, near):
    """The impedance peak nearest `near` Hz for hole travels `states`."""
    freqs = np.arange(near * 0.8, near * 1.25, near * 2e-4)
    z = np.abs(impedance(geom, np.array([[states.get(n, 0.0) for n in HOLE_NAMES]]), freqs))[0]
    peaks = find_peaks(freqs, z, count=8, floor=0.02)
    return min(peaks, key=lambda p: abs(math.log(p["hz"] / near)))["hz"] if peaks else float("nan")


def quarter_shade(geom, key):
    """Shading s for the quarter tone between written key-1 and key: every
    hole that opens in that change at admittance fraction s, every hole that
    closes at 1 - s, places the sounding peak at the geometric mean of the
    two fingerings' sounding peaks. Returns (s, cents at s, cents at s = 0.5),
    or None across a change of mode."""
    if key - 1 not in RANGE or mode(key - 1) != mode(key):
        return None
    pads = {h["name"]: h.get("pad") for h in geom["holes"]}
    a, b = played(key - 1), played(key)
    m = mode(key) - 1
    near = [model_peaks(geom, k, count=6)[m]["hz"] for k in (key - 1, key)]
    fa = sounding_peak(geom, {n: travel_for(1.0 if n in a else 0.0, pads[n]) for n in pads}, near[0])
    fb = sounding_peak(geom, {n: travel_for(1.0 if n in b else 0.0, pads[n]) for n in pads}, near[1])
    # `quarter_bias`: how sharp the played quarter tone came out of this
    # shading (measured by `quarters`); aim that much lower.
    target = math.sqrt(fa * fb) * 2 ** (-geom.get("quarter_bias", {}).get(str(key), 0.0) / 1200)

    def peak(s):
        states = {}
        for n in pads:
            g = (1.0 if n in a else 0.0) if (n in a) == (n in b) else (s if n in b else 1 - s)
            states[n] = travel_for(g, pads[n])
        return sounding_peak(geom, states, target)

    lo, hi = 0.0, 1.0
    f_lo, f_hi = peak(lo), peak(hi)
    rising = f_hi > f_lo
    for _ in range(30):
        mid = (lo + hi) / 2
        if (peak(mid) < target) == rising:
            lo = mid
        else:
            hi = mid
    s = (lo + hi) / 2
    return s, cents(peak(s), target), cents(peak(0.5), target)


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


def reference_curves(xlsx):
    """The spreadsheet's frequencies and, by fingering name, its complex
    input impedance (Pa s / m^3)."""
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
        out[name] = re + 1j * im
    return freqs, out


def reference_peaks(xlsx):
    freqs, curves = reference_curves(xlsx)
    out = {}
    for name, z in curves.items():
        key, variant = REFERENCE_NAMES[name]
        out[name] = {
            "written": key,
            "variant": variant,
            "peaks": find_peaks(freqs, np.abs(z), count=5, floor=0.05),
        }
    return out


# The measured impedance in bands around each fingering's first BAND_PEAKS
# peaks below BAND_MAX_HZ: within the larger of BAND_WIDTHS -3 dB widths and
# BAND_MIN_HZ of the peak, at most BAND_MAX_HALF. Stored as log |Z| (over the
# bore's characteristic impedance) and phase at the spreadsheet's own
# frequencies, every BAND_STRIDE-th bin. A fit to these is smooth in the
# geometry, unlike one to picked peaks, and covers the peaks' heights and
# widths as well as their frequencies.
BAND_PEAKS = 5
BAND_MAX_HZ = 2600.0
BAND_WIDTHS = 1.0
BAND_MIN_HZ = 12.0
BAND_MAX_HALF = 60.0
BAND_STRIDE = 1


def characteristic_impedance():
    return RHO * C / (math.pi * BORE**2)


def reference_bands(xlsx):
    freqs, curves = reference_curves(xlsx)
    step = float(np.median(np.diff(freqs)))
    if np.max(np.abs(np.diff(freqs) - step)) > 1e-6 * step:
        raise SystemExit("the spreadsheet's frequencies are not evenly spaced")
    zc = characteristic_impedance()
    out = {"hz0": float(freqs[0]), "step": step, "fingerings": {}}
    for name, z in curves.items():
        key, variant = REFERENCE_NAMES[name]
        peaks = [p for p in find_peaks(freqs, np.abs(z), count=BAND_PEAKS, floor=0.05) if p["hz"] < BAND_MAX_HZ]
        bands = []
        for k, p in enumerate(peaks, start=1):
            half = min(max(BAND_WIDTHS * (p["width"] or 2 * BAND_MAX_HALF), BAND_MIN_HZ), BAND_MAX_HALF)
            i0 = int(math.ceil((p["hz"] - half - freqs[0]) / step))
            i1 = int(math.floor((p["hz"] + half - freqs[0]) / step))
            idx = np.arange(max(i0, 0), min(i1, len(freqs) - 1) + 1, BAND_STRIDE)
            bands.append({"peak": k, "hz": round(p["hz"], 2), "start": int(idx[0]), "stride": BAND_STRIDE,
                          "log_z": [round(float(v), 4) for v in np.log(np.abs(z[idx]) / zc)],
                          "phase": [round(float(v), 4) for v in np.angle(z[idx])]})
        out["fingerings"][name] = {"written": key, "variant": variant, "bands": bands}
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


SEED = {
    # A seed from typical Boehm B-flat clarinet proportions, metres from the
    # reed tip. `fit` moves every value.
    "reg": (0.150, 0.0015, 0.013), "tr": (0.160, 0.0020, 0.007), "a": (0.172, 0.0025, 0.007),
    "gs": (0.182, 0.0025, 0.007), "v": (0.196, 0.0025, 0.007), "t": (0.206, 0.0035, 0.009),
    "l1": (0.230, 0.0030, 0.008), "sl": (0.243, 0.0024, 0.007),
    "l2":(0.255, 0.0035, 0.007), "eb": (0.268, 0.0025, 0.007), "l3": (0.284, 0.0040, 0.007),
    "cs": (0.303, 0.0035, 0.007), "r1": (0.325, 0.0040, 0.006), "r2": (0.350, 0.0040, 0.006),
    "bb": (0.338, 0.0030, 0.006), "r3": (0.385, 0.0045, 0.006), "gsds": (0.404, 0.0040, 0.006),
    "fscs": (0.430, 0.0050, 0.006), "fc": (0.458, 0.0055, 0.006), "eb_low": (0.490, 0.0060, 0.006),
}


def seed_hole(name):
    kind, label = next((k, l) for n, k, l in HOLES if n == name)
    pos, b, t = SEED[name]
    return {"name": name, "kind": kind, "label": label, "position": pos, "radius": b, "chimney": t,
            "pad": None if kind == "finger" else 0.2}


def vents_wanted(ring_vent):
    if ring_vent is None or ring_vent is False:
        return set()
    return set(RING_VENTS) if ring_vent is True else set(ring_vent)


def arrange(geom, ring_vent=None):
    """Put the geometry's holes in HOLES order, adding (True, or a set of
    names) or removing (False) ring vents; a new vent starts halfway between
    its neighbours."""
    holes = {h["name"]: h for h in geom["holes"]}
    wanted = vents_wanted(ring_vent)
    if ring_vent is not None:
        for v in set(RING_VENTS) - wanted:
            holes.pop(v, None)
    for v in wanted - set(holes):
        k = HOLE_NAMES.index(v)
        above = next(holes[n]["position"] for n in reversed(HOLE_NAMES[:k]) if n in holes)
        below = next(holes[n]["position"] for n in HOLE_NAMES[k + 1:] if n in holes)
        holes[v] = dict(seed_hole(v), position=(above + below) / 2)
    order = [n for n in HOLE_NAMES if n in holes]
    geom["holes"] = [dict(holes[n], kind=next(k for m, k, _ in HOLES if m == n),
                          label=next(l for m, _, l in HOLES if m == n)) for n in order]
    # A hole moved in the order goes halfway between its new neighbours; then
    # keep the minimum spacing.
    hs = geom["holes"]
    for k in range(1, len(hs) - 1):
        lo, hi = hs[k - 1]["position"], hs[k + 1]["position"]
        if not lo < hs[k]["position"] < hi and hi - lo > 2 * MIN_SEGMENT:
            hs[k]["position"] = (lo + hi) / 2
    for k in range(1, len(hs)):
        hs[k]["position"] = max(hs[k]["position"], hs[k - 1]["position"] + MIN_SEGMENT + 1e-4)
    geom["bell"] = max(geom["bell"], hs[-1]["position"] + 0.05)
    return geom


def initial_geometry(ring_vent=None):
    wanted = vents_wanted(RING_VENT if ring_vent is None else ring_vent)
    holes = [seed_hole(name) for name, _, _ in HOLES if name in wanted or name not in RING_VENTS]
    return {
        "holes": holes, "bell": 0.600,
        "loss": {"beta": 0.05, "shelf": 0.15, "shelf_hz": 1500.0, "hole": 1.0, "register": 0.3, "bell": 0.97, "bell_hz": 1800.0},
    }


LOSS_KEYS = ["beta", "shelf", "shelf_hz", "hole", "register", "bell", "bell_hz"]
LOSS_BOUNDS = {
    "beta": (0.0, 1.0), "shelf": (0.0, 0.6), "shelf_hz": (200.0, 6000.0), "hole": (0.0, 20.0), "register": (0.0, 4.0),
    "bell": (0.5, 1.0), "bell_hz": (300.0, 6000.0),
}
# m: clarinet_bore.muz needs every segment, after its series corrections, at
# least two samples at 88.2 kHz (7.82 mm); write_muz checks it.
MIN_SEGMENT = 0.008
MIN_SECONDS = 2 / 88_200


def hole_bounds(hole):
    """Plausible (radius, chimney) ranges in mm. The measured Yamaha YCL-250
    has tone holes of radius 1.5-6.2 mm and chimneys of 4.5-10 mm, and a
    register tube of radius 1.5 mm and 12.5 mm long that also projects into
    the bore; a Buffet Festival's tube is 13 mm long and 3.0 mm across."""
    if hole["kind"] == "register":
        return (1.45, 1.55), (12.0, 14.0)
    return (1.5, 6.5), (4.0, 12.0)


# The profile: the main bore's steps (radius in mm) and the flare's segments
# (length, radius). A Boehm B-flat bore is 14.6-15 mm across through the
# joints and its bell opens to about 60 mm over 120 mm.
STEP_BOUNDS = (6.6, 8.0)
FLARE = 4
FLARE_LENGTH = (0.008, 0.06)
FLARE_RADIUS = (6.6, 30.0)
PROTRUSION = (1.0, 4.5)  # mm, the register tube inside the bore
# The mouthpiece's chamber: from the tip to where its bore reaches the
# barrel's, narrower under the baffle (a clarinet mouthpiece is about 90 mm
# long, its window about 35 mm).
MOUTHPIECE_LENGTH = (0.010, 0.060)
MOUTHPIECE_RADIUS = (4.5, 7.6)


def with_profile(geom, flare=False):
    """Give a geometry without one a neutral profile: no steps, a mouthpiece
    chamber of the bore's radius and no flare. With `flare`, a geometry
    without a flare gets FLARE short segments at the bore's radius, carved
    out of the bell segment, for a fit to shape."""
    geom.setdefault("steps", {n: BORE for n in STEP_HOLES})
    geom.setdefault("mouthpiece", {"length": 0.02, "radius": BORE})
    geom.setdefault("flare", [])
    if flare and not geom["flare"]:
        geom["flare"] = [{"length": 0.0085, "radius": BORE} for _ in range(FLARE)]
        geom["bell"] -= 0.0085 * FLARE
    return geom


def pack(geom, what):
    x, lo, hi = [], [], []
    holes = geom["holes"]
    if "geometry" in what:
        prev = 0.0
        for h in holes:
            x.append(h["position"] - prev); lo.append(MIN_SEGMENT); hi.append(0.2)
            prev = h["position"]
        x.append(geom["bell"] - prev); lo.append(MIN_SEGMENT); hi.append(0.3)
        for h in holes:
            (a, b), _ = hole_bounds(h)
            x.append(h["radius"] * 1000); lo.append(a); hi.append(b)
        for h in holes:
            _, (a, b) = hole_bounds(h)
            x.append(h["chimney"] * 1000); lo.append(a); hi.append(b)
        for h in holes:
            if h["kind"] == "register":
                x.append(h.get("protrusion", 0.003) * 1000); lo.append(PROTRUSION[0]); hi.append(PROTRUSION[1])
    if "profile" in what:
        x.append(geom["mouthpiece"]["length"]); lo.append(MOUTHPIECE_LENGTH[0]); hi.append(MOUTHPIECE_LENGTH[1])
        x.append(geom["mouthpiece"]["radius"] * 1000); lo.append(MOUTHPIECE_RADIUS[0]); hi.append(MOUTHPIECE_RADIUS[1])
        for n in STEP_HOLES:
            x.append(geom["steps"][n] * 1000); lo.append(STEP_BOUNDS[0]); hi.append(STEP_BOUNDS[1])
        for f in geom["flare"]:
            x.append(f["length"]); lo.append(FLARE_LENGTH[0]); hi.append(FLARE_LENGTH[1])
            x.append(f["radius"] * 1000); lo.append(FLARE_RADIUS[0]); hi.append(FLARE_RADIUS[1])
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
        for h in holes:
            if h["kind"] == "register":
                h["protrusion"] = float(x[i]) / 1000; i += 1
    if "profile" in what:
        g["mouthpiece"] = {"length": float(x[i]), "radius": float(x[i + 1]) / 1000}; i += 2
        for name in STEP_HOLES:
            g["steps"][name] = float(x[i]) / 1000; i += 1
        for f in g["flare"]:
            f["length"] = float(x[i]); f["radius"] = float(x[i + 1]) / 1000; i += 2
    if "loss" in what:
        for k in LOSS_KEYS:
            g["loss"][k] = float(x[i]); i += 1
    return g


THIRD_PEAK = 0.0
# Weight of the first peak's height against the sounding one's, per register
# (clarion, altissimo): it decides whether the register speaks.
RATIO_WEIGHT = {1: 2.0, 2: 10.0}
# Weight of the throat fingerings as played (right hand down) against the
# measured peaks.
PLAYED_WEIGHT = 1.0
# Weight of the clarion peak heights against the reference, and the model's
# overall height scale against the reference's (the committed geometry's
# peaks stand at about 0.9 of the measured ones).
HEIGHT_WEIGHT = 0.0
HEIGHT_SCALE = 0.9


def residuals(geom, ref, widths=False, peaks=2):
    """Cents errors of the first `peaks` (and the sounding) impedance peaks of
    every reference fingering, in units of 10 cents; optionally widths (in
    units of 10%); and, for register fingerings, the height of the first peak
    against the sounding one, which decides whether the register speaks."""
    # Alternates count double: a fork is a resonance players choose for its tone.
    items = [(name, r, 1.0 if r["variant"] == 0 else 2.0) for name, r in sorted(ref.items())]
    # Unmeasured alternates aim at their standard fingering's peaks.
    items += [(written_name(k) + "*", dict(ref[written_name(k)], variant=1), w)
              for k, w in sorted(ALTERNATE_TARGETS.items())]
    fingerings = [(r["written"], r["variant"]) for _, r, _ in items]
    model = all_model_peaks(geom, fingerings)
    out = []
    for (name, r, weight), got_all in zip(items, model):
        key, variant = r["written"], r["variant"]
        n = 3 if THIRD_PEAK else max(peaks, mode(key))
        want = r["peaks"][:n]
        got = match(got_all, want)
        for k, (w, m) in enumerate(zip(want, got)):
            # The sounding peak counts most; a third peak that does not sound
            # (near the open-hole cutoff) counts THIRD_PEAK.
            wk = weight * (2.0 if k + 1 == mode(key) else THIRD_PEAK if k == 2 else 1.0)
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
        # In every stage, since it decides whether the register speaks.
        s = mode(key) - 1
        ok = s > 0 and all(g is not None for g in got[: s + 1])
        out.append(RATIO_WEIGHT[s] * math.log((got[0]["z"] / got[s]["z"]) / (want[0]["z"] / want[s]["z"])) / 0.1 if ok else 0.0)
    # Heights, which decide whether a soft note speaks: each clarion note's
    # sounding peak against the reference's (in bore characteristic
    # impedances, less the model's overall scale), and the fourth peaks of
    # the upper clarion, which sit near twice the note and must not stand
    # above the reference's (a strong one draws the reed off its note).
    if HEIGHT_WEIGHT:
        zc = RHO * C / (math.pi * BORE**2)
        for (name, r, weight), got_all in zip(items, model):
            key = r["written"]
            if r["variant"] or not 71 <= key <= 84:
                continue
            want = r["peaks"][:4]
            got = match(got_all, want)
            if got[1] is not None:
                out.append(HEIGHT_WEIGHT * math.log(got[1]["z"] / (want[1]["z"] / zc) / HEIGHT_SCALE) / 0.1)
            if key >= 76 and len(want) > 3 and got[3] is not None:
                out.append(HEIGHT_WEIGHT * max(0.0, math.log(got[3]["z"] / (want[3]["z"] / zc) / HEIGHT_SCALE)) / 0.1)
    # The throat notes as played, right hand down, stay near the measured
    # (right hand up) peaks: players find the hand changes them little.
    throat = [(k, v) for k in range(THROAT_RH[0], THROAT_RH[1] + 1) for v in (0, 1) if v == 0 or has_alternate(k)]
    states = np.array([[1.0 if n in played(k, v) else 0.0 for n in HOLE_NAMES] for k, v in throat])
    z = np.abs(impedance(geom, states, FREQS))
    names = {(r["written"], r["variant"]): r for r in ref.values()}
    for (k, v), row in zip(throat, z):
        want = names[(k, v)]["peaks"][:2]
        for w, m in zip(want, match(find_peaks(FREQS, row, count=6, floor=0.05), want)):
            out.append(PLAYED_WEIGHT * (cents(m["hz"], w["hz"]) / 10 if m else 3.0))
    out.extend(penalties(geom))
    return np.array(out)


def penalties(geom):
    """Bounds the parameters' boxes do not keep."""
    # Series corrections shorten segments below MIN_SEGMENT: keep each above
    # two samples at 88.2 kHz, with 0.1 mm to spare (a penalty per 0.01 mm).
    floor = MIN_SECONDS * C + 1e-4
    out = list(np.maximum(floor - segments(geom), 0.0) / 1e-5)
    # A bell only widens: a penalty per 0.02 mm of narrowing along the flare.
    radii = [geom["holes"] and segment_radii(geom)[len(geom["holes"]) + 1]] + [f["radius"] for f in geom.get("flare", [])]
    out.extend(np.maximum(np.array(radii[:-1]) - np.array(radii[1:]), 0.0) / 2e-5)
    return out


# --- The band fit ------------------------------------------------------------------------
#
# Each point of the reference's bands (`bands`) gives two residuals: the
# model's log |Z| less the reference's, less log HEIGHT_SCALE (the model's
# peaks stand at about 0.9 of the measured ones: the reference's
# calibration against the bore's characteristic impedance), in units of
# BAND_UNITS[0]; and the phase difference in units of BAND_UNITS[1]. A
# band's points share its weight (over the square root of their number), so
# a band 5% off throughout costs about 1. The first two peaks, which set
# the tuning, weigh PEAK_WEIGHT; alternates weigh double, as in the peak
# fit; the upper clarion's peaks 2-4 (the sounding one and those near
# twice the note, where its open holes' lattice sits) and the altissimo's
# 3-5 weigh UPPER_WEIGHT. The throat notes as played (right hand down) are
# held to the measured bands of their first two peaks at PLAYED_WEIGHT.
# Residuals beyond BAND_ROBUST units count linearly (scipy's soft_l1), so
# a band the effective lattice cannot reach does not rule the fit.
BAND_UNITS = (0.05, 0.05)
PEAK_WEIGHT = {1: 3.0, 2: 2.0, 3: 1.0, 4: 1.0, 5: 0.7}
UPPER_CLARION = (76, 84)
UPPER_WEIGHT = 2.0
BAND_ROBUST = 3.0
# Each band's height, a soft maximum of its log |Z| (at BAND_SOFTMAX), against
# the reference's, in units of HEIGHT_UNIT: it holds the balance of the
# peaks even where a peak's frequency is a little off. The first peak and
# the sounding one (whose ratio decides whether the register speaks) weigh
# BAND_HEIGHT, the others half, and the upper clarion's and altissimo's
# others UPPER_WEIGHT more.
BAND_SOFTMAX = 0.03
HEIGHT_UNIT = 0.03
BAND_HEIGHT = 3.0
WEAK_PEAK = 0.4
_BAND_TABLES = {}


def upper_peak(key, peak):
    """The upper clarion's peaks 2-4 and the altissimo's 3-5."""
    return (UPPER_CLARION[0] <= key <= UPPER_CLARION[1] and 2 <= peak <= 4) or (key > UPPER_CLARION[1] and 3 <= peak <= 5)


def band_weight(key, variant, peak):
    return (1.0 if variant == 0 else 2.0) * PEAK_WEIGHT[peak] * (UPPER_WEIGHT if upper_peak(key, peak) else 1.0)


def height_weight(key, variant, peak):
    w = 1.0 if variant == 0 else 2.0
    if peak in (1, mode(key)):
        return w * BAND_HEIGHT
    return w * 0.5 * (UPPER_WEIGHT if upper_peak(key, peak) else 1.0)


def soft_max(x, axis=-1):
    """BAND_SOFTMAX log-sum-exp of x, less its value for a flat x."""
    top = np.max(x, axis=axis, keepdims=True)
    return (top + BAND_SOFTMAX * np.log(np.mean(np.exp((x - top) / BAND_SOFTMAX), axis=axis, keepdims=True))).squeeze(axis)


def band_table(bands):
    """Rows of frequencies, reference log |Z| and phase, weights and hole
    states: the reference fingerings, then the throat notes as played."""
    tag = (id(bands), LITTLE_UP, UPPER_WEIGHT, PLAYED_WEIGHT, BAND_HEIGHT, BAND_SOFTMAX, WEAK_PEAK)
    if tag in _BAND_TABLES:
        return _BAND_TABLES[tag]
    rows = []
    for name, r in sorted(bands["fingerings"].items(), key=lambda kv: (kv[1]["written"], kv[1]["variant"])):
        k, v = r["written"], r["variant"]
        rows.append((name, fingering(k, v), r["bands"], lambda peak, k=k, v=v: band_weight(k, v, peak),
                     lambda peak, k=k, v=v: height_weight(k, v, peak)))
        if THROAT_RH[0] <= k <= THROAT_RH[1] and played(k, v) != fingering(k, v):
            rows.append((name + " played", played(k, v), [b for b in r["bands"] if b["peak"] <= 2],
                         lambda peak: PLAYED_WEIGHT, lambda peak: 0.0))
    n = max(sum(len(b["log_z"]) for b in bs) for _, _, bs, _, _ in rows)
    F = np.full((len(rows), n), 1000.0)
    L, P, W = np.zeros_like(F), np.zeros_like(F), np.zeros_like(F)
    heights = []  # (row, start, end, reference height, weight)
    for i, (_, _, bs, weight, hweight) in enumerate(rows):
        j = 0
        top = max(max(b["log_z"]) for b in bs)
        for b in bs:
            m = len(b["log_z"])
            # A weak resonance (near the open holes' cutoff) hardly acts on
            # the reed: it weighs in proportion to its height below WEAK_PEAK
            # of the fingering's highest.
            weak = min(1.0, math.exp(max(b["log_z"]) - top) / WEAK_PEAK)
            F[i, j:j + m] = bands["hz0"] + bands["step"] * (b["start"] + b["stride"] * np.arange(m))
            L[i, j:j + m], P[i, j:j + m] = b["log_z"], b["phase"]
            W[i, j:j + m] = weak * weight(b["peak"]) / math.sqrt(m)
            heights.append((i, j, j + m, float(soft_max(np.array(b["log_z"]))), weak * hweight(b["peak"])))
            j += m
    states = np.array([[1.0 if n in opened else 0.0 for n in HOLE_NAMES] for _, opened, _, _, _ in rows])
    table = ([r[0] for r in rows], [r[2] for r in rows], F, L, P, W, states, heights)
    _BAND_TABLES[tag] = table
    return table


def band_errors(geom, bands):
    """Per-point log |Z| and phase errors and weights, by table row, and each
    band's height error and weight."""
    names, row_bands, F, L, P, W, states, heights = band_table(bands)
    z = impedance(geom, states, F)
    logz = np.log(np.abs(z)) - math.log(HEIGHT_SCALE)
    dh = np.array([[soft_max(logz[i, a:b]) - h, w] for i, a, b, h, w in heights])
    return names, row_bands, logz - L, np.angle(z * np.exp(-1j * P)), W, dh


def robust(r, scale):
    """Residuals whose squares are scipy's soft_l1 loss at `scale`: about r
    when small, growing as sqrt(2 scale |r|) when large."""
    u = r / scale
    return np.sign(r) * scale * np.sqrt(2 * (np.sqrt(1 + u * u) - 1))


def band_residuals(geom, bands):
    _, _, dl, dp, W, dh = band_errors(geom, bands)
    r = np.concatenate([(W * dl / BAND_UNITS[0]).ravel(), (W * dp / BAND_UNITS[1]).ravel(),
                        dh[:, 1] * dh[:, 0] / HEIGHT_UNIT])
    return np.concatenate([robust(r, BAND_ROBUST) if BAND_ROBUST else r, penalties(geom)])


def band_report(geom, bands, worst=12):
    """Each band's rms log |Z| error (%), phase error (rad) and height error
    (%), the worst first."""
    names, row_bands, dl, dp, W, dh = band_errors(geom, bands)
    heights = iter(dh)
    rows = []
    total = 0.0
    for i, (name, bs) in enumerate(zip(names, row_bands)):
        j = 0
        for b in bs:
            m = len(b["log_z"])
            l, ph, w = dl[i, j:j + m], dp[i, j:j + m], W[i, j]
            h, hw = next(heights)
            rows.append((name, b["peak"], b["hz"], 100 * math.sqrt(np.mean(l**2)), math.sqrt(np.mean(ph**2)),
                         100 * h, w * math.sqrt(m), hw))
            total += w * w * m * (np.mean(l**2) / BAND_UNITS[0] ** 2 + np.mean(ph**2) / BAND_UNITS[1] ** 2)
            total += (hw * h / HEIGHT_UNIT) ** 2
            j += m
    print(f"band cost {total / 2:.1f} over {len(rows)} bands; "
          f"rms log|Z| {np.sqrt(np.mean([r[3] ** 2 for r in rows])):.1f}%, "
          f"phase {np.sqrt(np.mean([r[4] ** 2 for r in rows])):.3f} rad, "
          f"height {np.sqrt(np.mean([r[5] ** 2 for r in rows])):.1f}%")
    rows.sort(key=lambda r: -((r[6] * r[3] / 5) ** 2 + (r[6] * r[4] / 0.05) ** 2 + (r[7] * r[5] / 3) ** 2))
    print(f"{'fingering':14s} {'peak':>4s} {'hz':>7s} {'|Z| rms%':>8s} {'phase':>6s} {'height%':>7s} {'weights':>9s}")
    for name, k, hz, l, ph, h, w, hw in rows[:worst]:
        print(f"{name:14s} {k:4d} {hz:7.1f} {l:8.1f} {ph:6.3f} {h:+7.1f} {w:4.1f} {hw:4.1f}")


# Parameters the band fit frees, each with its box and its prior scale: a
# change of one scale costs as much as a band 5% off, over BAND_PRIOR. Free
# are the profile, the losses, the bell's length and, in BAND_HOLES, the
# holes near the upper clarion's open holes: positions within BAND_SHIFT of
# the start, radii and chimneys within hole_bounds.
BAND_HOLES = ("reg", "v", "t", "l1", "sl", "l2", "eb", "l3", "cs", "r1", "bb", "r2", "r3")
BAND_SHIFT = 0.012
BAND_PRIOR = 1.0
# Whether the mouthpiece's chamber is free. On the previous reed, set in
# units of the chamber's characteristic impedance on a chamber of the
# bore's radius, a narrower one (as the peak fit's profile found,
# 6.3-6.75 mm) left the soft upper clarion (F#5-G#5) starting in the low
# register and E6 and F6 jumping between regimes, so it is held (README).
BAND_MOUTHPIECE = False
PRIOR_SCALE = {
    "position": 0.002, "radius": 0.0003, "chimney": 0.0015, "protrusion": 0.0005,
    "mouthpiece_length": 0.005, "mouthpiece_radius": 0.0003, "step": 0.0002,
    "flare_length": 0.005, "flare_radius": 0.003, "bell": 0.005,
    "beta": 0.01, "shelf": 0.03, "shelf_hz": 100.0, "hole": 0.2, "register": 0.05, "bell_loss": 0.03,
    "bell_hz": 400.0,
}


def band_parameters(geom, holes=None):
    """(path, lower, upper, prior scale) for each free parameter."""
    holes = BAND_HOLES if holes is None else holes
    holes = HOLE_NAMES if holes == "all" else holes
    out = []
    for k, h in enumerate(geom["holes"]):
        if h["name"] not in holes:
            continue
        (rlo, rhi), (tlo, thi) = hole_bounds(h)
        out.append((("holes", k, "position"), h["position"] - BAND_SHIFT, h["position"] + BAND_SHIFT,
                    PRIOR_SCALE["position"]))
        out.append((("holes", k, "radius"), rlo / 1000, rhi / 1000, PRIOR_SCALE["radius"]))
        out.append((("holes", k, "chimney"), tlo / 1000, thi / 1000, PRIOR_SCALE["chimney"]))
        if h.get("protrusion"):
            out.append((("holes", k, "protrusion"), PROTRUSION[0] / 1000, PROTRUSION[1] / 1000,
                        PRIOR_SCALE["protrusion"]))
    out.append((("bell",), geom["bell"] - 0.02, geom["bell"] + 0.02, PRIOR_SCALE["bell"]))
    if BAND_MOUTHPIECE:
        out.append((("mouthpiece", "length"), *MOUTHPIECE_LENGTH, PRIOR_SCALE["mouthpiece_length"]))
        out.append((("mouthpiece", "radius"), MOUTHPIECE_RADIUS[0] / 1000, MOUTHPIECE_RADIUS[1] / 1000,
                    PRIOR_SCALE["mouthpiece_radius"]))
    for n in STEP_HOLES:
        out.append((("steps", n), STEP_BOUNDS[0] / 1000, STEP_BOUNDS[1] / 1000, PRIOR_SCALE["step"]))
    for k in range(len(geom["flare"])):
        out.append((("flare", k, "length"), *FLARE_LENGTH, PRIOR_SCALE["flare_length"]))
        out.append((("flare", k, "radius"), FLARE_RADIUS[0] / 1000, FLARE_RADIUS[1] / 1000,
                    PRIOR_SCALE["flare_radius"]))
    for key in LOSS_KEYS:
        out.append((("loss", key), *LOSS_BOUNDS[key], PRIOR_SCALE["bell_loss" if key == "bell" else key]))
    return out


def _get(geom, path):
    for p in path:
        geom = geom[p]
    return geom


def _set(geom, path, value):
    for p in path[:-1]:
        geom = geom[p]
    geom[path[-1]] = float(value)


def band_unpack(geom, entries, x0, y):
    g = json.loads(json.dumps(geom))
    for (path, _, _, scale), a, b in zip(entries, x0, y):
        _set(g, path, a + scale * b)
    return g


def _band_column(args):
    y, i, h = args
    w = _WORK
    y = y.copy()
    y[i] += h
    return band_residuals(band_unpack(w["geom"], w["entries"], w["x0"], y), w["bands"])


def _band_init(geom, bands, entries, x0, settings):
    apply_player(geom)
    globals().update(settings)
    _WORK.update(geom=geom, bands=bands, entries=entries, x0=x0)


def fit_bands(geom, bands, max_nfev, prior=None, anchor=None, workers=32):
    """Least squares on the bands, with a prior toward `anchor` (the start by
    default) in units of PRIOR_SCALE over `prior` (BAND_PRIOR). The
    Jacobian is by forward differences, its columns in parallel."""
    from concurrent.futures import ProcessPoolExecutor
    from scipy.optimize import least_squares

    prior = BAND_PRIOR if prior is None else prior
    geom = arrange(with_profile(geom, flare=True), RING_VENT)
    entries = band_parameters(geom)
    x0 = np.array([_get(geom, e[0]) for e in entries])
    scales = np.array([e[3] for e in entries])
    lo = (np.array([e[1] for e in entries]) - x0) / scales
    hi = (np.array([e[2] for e in entries]) - x0) / scales
    lo, hi = np.minimum(lo, -1e-6), np.maximum(hi, 1e-6)
    if anchor is None:
        home = np.zeros_like(x0)
    else:
        anchor = arrange(with_profile(json.loads(json.dumps(anchor)), flare=True), RING_VENT)
        home = np.clip((np.array([_get(anchor, e[0]) for e in entries]) - x0) / scales, lo, hi)

    def f(y):
        return np.concatenate([band_residuals(band_unpack(geom, entries, x0, y), bands), prior * (y - home)])

    settings = {k: globals()[k] for k in (
        "UPPER_WEIGHT", "PLAYED_WEIGHT", "HEIGHT_SCALE", "BAND_UNITS", "PEAK_WEIGHT", "BAND_ROBUST", "BAND_HOLES",
        "BAND_SOFTMAX", "HEIGHT_UNIT", "BAND_HEIGHT", "WEAK_PEAK", "BAND_MOUTHPIECE")}
    with ProcessPoolExecutor(workers, initializer=_band_init, initargs=(geom, bands, entries, x0, settings)) as pool:
        def jac(y):
            f0 = band_residuals(band_unpack(geom, entries, x0, y), bands)
            h = 1e-3
            steps = np.where(y + h > hi, -h, h)
            cols = list(pool.map(_band_column, [(y, i, steps[i]) for i in range(len(y))], chunksize=2))
            J = np.stack([(c - f0) / d for c, d in zip(cols, steps)], axis=1)
            return np.vstack([J, prior * np.eye(len(y))])

        result = least_squares(f, np.zeros_like(x0), jac=jac, bounds=(lo, hi), max_nfev=max_nfev, verbose=1,
                               xtol=1e-10, ftol=1e-8)
    moved = sorted(zip(np.abs(result.x - home), [e[0] for e in entries]), reverse=True)[:8]
    print(f"cost {result.cost:.1f} (prior {0.5 * np.sum((prior * (result.x - home)) ** 2):.1f}), "
          f"{result.nfev} evaluations; most moved (scales): "
          + ", ".join(f"{'.'.join(map(str, p))} {m:.1f}" for m, p in moved))
    return band_unpack(geom, entries, x0, result.x)


_WORK = {}


def _init(geom, ref, what, widths, third=0.0):
    globals()["THIRD_PEAK"] = third
    _WORK.update(geom=geom, ref=ref, what=what, widths=widths)


def _column(args):
    x, i, h = args
    y = x.copy()
    y[i] += h
    return residuals(unpack(_WORK["geom"], y, _WORK["what"]), _WORK["ref"], widths=_WORK["widths"])


def fit(geom, ref, what, widths, max_nfev, workers=24):
    """Least squares on the reference peaks; the Jacobian's columns are
    computed in parallel."""
    from concurrent.futures import ProcessPoolExecutor
    from scipy.optimize import least_squares

    geom = arrange(geom, RING_VENT)
    x0, bounds = pack(geom, what)

    def f(x):
        return residuals(unpack(geom, x, what), ref, widths=widths)

    with ProcessPoolExecutor(workers, initializer=_init, initargs=(geom, ref, what, widths, THIRD_PEAK)) as pool:
        def jac(x):
            f0 = f(x)
            steps = 1e-4 * np.maximum(np.abs(x), 1e-3)
            # Step inward at an upper bound.
            steps = np.where(x + steps > bounds[1], -steps, steps)
            cols = list(pool.map(_column, [(x, i, steps[i]) for i in range(len(x))]))
            return np.stack([(c - f0) / h for c, h in zip(cols, steps)], axis=1)

        result = least_squares(f, x0, jac=jac, bounds=bounds, x_scale="jac", max_nfev=max_nfev, verbose=1,
                               xtol=1e-12, ftol=1e-10)
    print(f"cost {result.cost:.1f}, {result.nfev} evaluations")
    return unpack(geom, result.x, what)


# A local peak fit: the holes LOCAL_HOLES (position, radius, chimney) free
# about the start, with a prior in units of LOCAL_SCALE over LOCAL_PRIOR,
# the third peaks at THIRD_PEAK (`--third`), and the first two peaks of the
# fingerings in LOCAL_EXTRA counted that much more (in units of 10 cents).
# Round 8 brought the forked Eb4's second peak from 22 cents under the
# reference to 8 with it (`fit --stage local --third 0.3`).
LOCAL_HOLES = ("t", "sl", "l2", "eb", "l3", "cs", "r1", "bb", "r2", "r3")
LOCAL_SCALE = {"position": 0.002, "radius": 0.0003, "chimney": 0.0015}
LOCAL_PRIOR = 0.5
LOCAL_EXTRA = {"D#4alt2": 5.0}


def fit_local(geom, ref, max_nfev):
    from scipy.optimize import least_squares

    entries = [(k, what) for k, h in enumerate(geom["holes"]) if h["name"] in LOCAL_HOLES
               for what in ("position", "radius", "chimney")]
    x0 = np.array([geom["holes"][k][what] for k, what in entries])
    scales = np.array([LOCAL_SCALE[what] for _, what in entries])

    def build(y):
        g = json.loads(json.dumps(geom))
        for (k, what), v in zip(entries, x0 + scales * y):
            g["holes"][k][what] = float(v)
        return g

    def extra(g):
        out = []
        for name, weight in LOCAL_EXTRA.items():
            r = ref[name]
            want = r["peaks"][:2]
            got = match(model_peaks(g, r["written"], r["variant"], count=6), want)
            out += [weight * cents(m["hz"], w["hz"]) / 10 for w, m in zip(want, got)]
        return out

    def f(y):
        g = build(y)
        return np.concatenate([residuals(g, ref), extra(g), LOCAL_PRIOR * y])

    result = least_squares(f, np.zeros(len(x0)), diff_step=1e-3, max_nfev=max_nfev, verbose=1)
    print(f"cost {result.cost:.1f}, {result.nfev} evaluations")
    return build(result.x)


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
    # Whether the register speaks: the first peak's height over the sounding one's.
    for m in (2, 3):
        cells = [f"{name} {got[0]['z'] / got[m - 1]['z']:.2f}/{want[0]['z'] / want[m - 1]['z']:.2f}"
                 for name, key, variant, want, got in rows
                 if mode(key) == m and variant == 0 and got[0] and got[m - 1]]
        print(f"z1/z{m} model/ref: " + ", ".join(cells))


def muz_number(x):
    # Six significant digits, positional: a positional literal is exact in muz
    # (one with an exponent is inexact).
    return np.format_float_positional(float(f"{x:.6g}"), trim="-")


def write_muz(geom, path=None):
    holes = geom["holes"]
    lengths = segments(geom)
    radii = segment_radii(geom)
    n = len(holes)
    names = ["mouthpiece", "reg"] + [h["name"] for h in holes[1:]] + ["bell"] + [f"flare {k}" for k in range(len(geom.get("flare", [])))]
    short = [name for name, l in zip(names, lengths) if l / C < MIN_SECONDS]
    if short:
        raise SystemExit(f"segments above {short} are shorter than two samples at 88.2 kHz")
    present = {h["name"] for h in holes}
    loss = geom["loss"]
    lines = [
        "// Generated by tools/clarinet-lattice.py fit from clarinet-geometry.json.",
        "// An effective B-flat clarinet geometry fitted to the UNSW impedance",
        "// measurements (see README.md). Edit the JSON and regenerate.",
        "//",
        "// holes: from the mouthpiece down. `seconds` is the bore segment above the",
        "// hole (after series inertance corrections), `alpha_hz` the open path's",
        "// inertance corner alpha / 2pi, `kappa` the closed chimney's compliance as a",
        "// highpass gain at `closed_hz`, `compliance_t` that compliance as a time",
        "// constant (half the chimney's volume over the bore's area times c, the",
        "// release 1 / (1 + s compliance_t)), `pad` the key's curtain parameter (finger",
        "// holes have none). `closed_hz` is the trapezoidal one-pole's cutoff at",
        f"// {RATE / 1000:g} kHz (2x oversampling of 48 kHz), where the filter is the",
        "// bilinear transform of the compliance; at 88.2 kHz the compliance is",
        "// within about 1% of it. `step` is the reflection of a wave arriving from",
        "// above at a change of the bore's radius at the hole (0 for none).",
        f"let closed_hz = {muz_number(CLOSED_HP)};",
        f"let speed = {muz_number(C)};",
        f"let bore_radius = {muz_number(BORE)};",
        "let holes = [",
    ]
    for k, h in enumerate(holes):
        el = lumped(h)
        pad = "null" if h.get("pad") is None else muz_number(h["pad"])
        lines.append(
            f"    {{name: \"{h['name']}\", kind: \"{h['kind']}\", seconds: {muz_number(lengths[k + 1] / C)}, "
            f"alpha_hz: {muz_number(el['alpha'] / (2 * math.pi))}, resistance: {muz_number(hole_resistance(h, loss))}, "
            f"kappa: {muz_number(el['kappa'])}, compliance_t: {muz_number(el['compliance_t'])}, nonlinear: {muz_number(el['nonlinear'])}, "
            f"closed_hz: {muz_number(el['closed_hz'])}, pad: {pad}, step: {muz_number(step_reflection(radii[k + 1], radii[k + 2]))}, "
            f"position_mm: {h['position'] * 1000:.2f}, radius_mm: {h['radius'] * 1000:.3f}, chimney_mm: {h['chimney'] * 1000:.2f}"
            + (f", protrusion_mm: {h['protrusion'] * 1000:.2f}" if h.get("protrusion") else "") + "},"
        )
    lines.append("];")
    lines.append(f"let bell_seconds = {muz_number(lengths[n + 1] / C)};")
    lines.append("// The bell's flare below the main bore: cylinders of growing radius, each")
    lines.append("// with the step at its top. `bell_admittance` is the last one's area over")
    lines.append("// the bore's.")
    lines.append("let flare = [" + ", ".join(
        f"{{seconds: {muz_number(lengths[n + 2 + k] / C)}, step: {muz_number(step_reflection(radii[n + 1 + k], radii[n + 2 + k]))}, "
        f"radius_mm: {radii[n + 2 + k] * 1000:.2f}}}" for k in range(len(lengths) - n - 2)) + "];")
    lines.append(f"let bell_admittance = {muz_number((radii[-1] / BORE) ** 2)};")
    lines.append("// The mouthpiece's chamber above the bore: its round trip, the step into the")
    lines.append("// bore below it, and its characteristic impedance over the bore's.")
    lines.append(f"let mouthpiece = {{seconds: {muz_number(lengths[0] / C)}, step: {muz_number(step_reflection(radii[0], radii[1]))}, "
                 f"impedance: {muz_number((BORE / radii[0]) ** 2)}, radius_mm: {radii[0] * 1000:.2f}}};")
    lines.append("// How far the register key leads an ascent across the break (0..1 of its")
    lines.append("// travel) and how many milliseconds ahead (lattice-clarinet.muz's `vented`).")
    lines.append(f"let vent_crack = {muz_number(geom.get('player', {}).get('vent_crack', 0))};")
    lines.append(f"let vent_lead = {muz_number(geom.get('player', {}).get('vent_lead', 35))};")
    lines.append("// The written notes that leave the little finger free: open G and G#, played")
    lines.append("// little finger up, where it stays as the previous note left it.")
    lines.append("let little_free = [" + (", ".join(str(k) for k in LITTLE_FREE) if LITTLE_UP else "") + "];")
    lines.append(f"let loss = {{beta: {muz_number(loss['beta'])}, shelf: {muz_number(loss['shelf'])}, "
                 f"shelf_hz: {muz_number(loss['shelf_hz'])}, hole: {muz_number(loss['hole'])}, "
                 f"bell: {muz_number(loss['bell'])}, bell_hz: {muz_number(loss['bell_hz'])}}};")
    lines.append("// Round-trip wall-loss gain of each segment, mouthpiece first, then the bell")
    lines.append("// segment and the flare, lumped onto the top segment of each run of five.")
    lines.append("let segment_gain = [" + ", ".join(muz_number(g) for g in segment_gains(geom)) + "];")
    lines.append("")
    # Fingerings: for each written note, standard and alternate, the open holes.
    lines.append("// Fingerings by written note: the holes open, standard and alternate.")
    lines.append("// `shade`: for the quarter tone up from the previous written note, the")
    lines.append("// admittance fraction of each hole that opens (a hole that closes keeps")
    lines.append("// 1 - shade) that sounds the geometric mean of the two notes; 0.5 at the")
    lines.append("// first note and across a change of register.")
    lines.append("let fingerings = [")
    for key in RANGE:
        std = sorted(played(key, 0) & present, key=HOLE_NAMES.index)
        alt = sorted(played(key, 1) & present, key=HOLE_NAMES.index) if has_alternate(key) else None
        alt_s = "null" if alt is None else "[" + ", ".join(f'"{n}"' for n in alt) + "]"
        shade = quarter_shade(geom, key)
        lines.append(f"    {{written: {key}, mode: {mode(key)}, open: [" + ", ".join(f'"{n}"' for n in std) + f"], alternate: {alt_s}, "
                     f"shade: {muz_number(round(shade[0], 4)) if shade else '0.5'}}},")
    lines.append("];")
    (path or GEOMETRY_MUZ).write_text("\n".join(lines) + "\n")


# --- Playing: the fingering's own pitch, and the tuned result ---------------------

MUZ = ROOT / "target/release/muz"
DYNAMICS = (0.3, 0.45, 0.6)
NOTE_SECONDS = 0.6
TUNING_MUZ = PROTO / "clarinet_tuning.muz"


def played_notes(quarter_tones=False):
    """(sounding key, written key, variant) for every fingering in range, and
    with `quarters` the quarter tones between semitones of one mode (shaded
    holes, written k + 0.5)."""
    out = [(k - 2, k, 0) for k in RANGE]
    out += [(k - 2, k, 1) for k in RANGE if has_alternate(k)]
    if quarter_tones:
        out += [(k - 1.5, k + 0.5, 0) for k in RANGE if k + 1 in RANGE and mode(k) == mode(k + 1)]
    return out


# The instrument is measured with the player's pitch still: its drift
# (std/woodwinds' `drift_cents`, a slow wander of a few cents) would
# otherwise enter the tuning table at the times this song plays each note,
# and `check` would find it again there, while every other passage meets
# another wander (a post-slur B4 up to 4.9 cents off its tuned 0.3).
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
        f'song({{tempo: 60, tracks: [track("cl", line, lc.clarinet({{velocity_breath: 0, drift_cents: 0, lattice: {lattice}}}))]}})\n'
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


def render_dynamics(notes, lattice, rate=48_000, dynamics=None, windows=((0.3, 0.55),)):
    """Render `notes` at each of `dynamics` (DYNAMICS) in parallel; return
    {dynamic: rows}, a row the measure of each of `windows` (seconds into
    the note), or of the one window."""
    import subprocess
    import tempfile

    import soundfile as sf

    results = {}
    with tempfile.TemporaryDirectory(prefix="muz-clarinet-tune-") as scratch:
        procs = []
        for d in dynamics or DYNAMICS:
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
                got = [measure_pitch(x[int((start + a) * actual):int((start + b) * actual)], actual,
                                     440 * 2 ** ((s - 69) / 12)) for a, b in windows]
                rows.append(got[0] if len(windows) == 1 else got)
            results[d] = rows
    return results


# Playing stability: the clarion and altissimo (with their alternates and
# the throat B-flat) tuned at STABILITY_DYNAMICS. A note is bistable where
# its pitch jumps more than JUMP_CENTS between adjacent dynamics, wanders
# where it moves more than JUMP_CENTS within the note (0.15-0.33 s against
# 0.37-0.55 s), and speaks in the wrong register where the twelfth below
# stands above UNDER_DB against it.
STABILITY_DYNAMICS = (0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6)
JUMP_CENTS = 15.0
UNDER_DB = -40.0


def stability(tuned=True, only=None):
    notes = [n for n in played_notes() if n[1] >= 70 and (only is None or written_name(n[1]) + ("*" if n[2] else "") in only)]
    measured = render_dynamics(notes, "{}" if tuned else "{tuned: false}", dynamics=STABILITY_DYNAMICS, windows=((0.3, 0.55), (0.15, 0.33), (0.37, 0.55)))
    print(f"{'note':8s} " + " ".join(f"{d:>13}" for d in STABILITY_DYNAMICS) + "  flags")
    levels = np.array([[measured[d][i][0][1] for d in STABILITY_DYNAMICS] for i in range(len(notes))])
    bad = []
    for i, (s, w, v) in enumerate(notes):
        rows = [measured[d][i] for d in STABILITY_DYNAMICS]
        pitch = [r[0][0] for r in rows]
        flags = []
        jumps = [abs(b - a) for a, b in zip(pitch, pitch[1:])]
        if max(jumps) > JUMP_CENTS:
            flags.append(f"jump {max(jumps):.0f}c at {STABILITY_DYNAMICS[int(np.argmax(jumps)) + 1]}")
        wander = [abs(r[1][0] - r[2][0]) for r in rows]
        if max(wander) > JUMP_CENTS:
            flags.append(f"wanders {max(wander):.0f}c at {STABILITY_DYNAMICS[int(np.argmax(wander))]}")
        under = max(r[0][2] for r in rows)
        if under > UNDER_DB:
            flags.append(f"twelfth below {under:.0f} dB")
        quiet = levels[i] < np.median(levels, axis=0) - 20
        if quiet.any():
            flags.append(f"silent at {STABILITY_DYNAMICS[int(np.argmax(quiet))]}")
        if flags:
            bad.append(written_name(w) + ("*" if v else ""))
        print(f"{written_name(w) + ('*' if v else ''):8s} "
              + " ".join(f"{r[0][0]:+6.1f}({r[0][2]:+4.0f})" for r in rows) + "  " + "; ".join(flags))
    print(f"unstable: {len(bad)} ({', '.join(bad) or 'none'})")
    return bad


# The clarion's window: a crescendo on each upper clarion note, its breath
# WINDOW_KPA[0] + WINDOW_KPA[1] d kPa at dynamic d (1.9-6.7 kPa on every
# clarion note, past the player's 2.9-5.7 and without its fall up the
# register, which keeps the player inside the window), its lip falling
# 0.3 N per kPa from the clarion's 0.65 N at 4.3 kPa, untuned. A note speaks where its own pitch
# is within 60 cents and the twelfth below stays under -20 dB against it.
WINDOW_KPA = (1.9, 4.8)
WINDOW_DYNAMICS = tuple(round(0.05 * k, 2) for k in range(21))


# LATTICE_EXTRA adds settings to the lattice record (", upper_peak: 3").
def window(first=79):
    notes = [n for n in played_notes() if first <= n[1] <= 84 and n[2] == 0]
    lo, span = WINDOW_KPA
    lattice = (f"{{tuned: false, breath_kpa: [[1.9, 6.8], [{lo}, {lo + span}], [2.2, 5.0]], "
               "pressure_octave: [[-0.03, -0.15], [0, 0], [-0.18, -0.18]], overblow: 0"
               + os.environ.get("LATTICE_EXTRA", "") + "}")
    out = render_dynamics(notes, lattice, dynamics=WINDOW_DYNAMICS)
    kpa = [lo + span * d for d in WINDOW_DYNAMICS]
    print(f"{'note':5s} {'speaks (kPa)':>12s}   " + " ".join(f"{p:5.1f}" for p in kpa))
    for i, (s, w, v) in enumerate(notes):
        ok = [abs(out[d][i][0]) < 59 and out[d][i][2] < -20 for d in WINDOW_DYNAMICS]
        ps = [p for p, k in zip(kpa, ok) if k]
        # The window: the longest run of dynamics where it speaks.
        best, run = (None, None), []
        for p, k in zip(kpa, ok):
            run = run + [p] if k else []
            if run and (best[0] is None or run[-1] - run[0] > best[1] - best[0]):
                best = (run[0], run[-1])
        text = f"{best[0]:.1f}-{best[1]:.1f}" if ps else "never"
        print(f"{written_name(w):5s} {text:>12s}   " + " ".join(f"{out[d][i][2]:+5.0f}" if k else "    x"
                                                             for d, k in zip(WINDOW_DYNAMICS, ok)))


def tune(scale, reed_mm, rounds=3):
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
    # A length change does not move a fingering's pitch by exactly its share
    # of the tube's quarter waves (the lower bore and the holes respond too):
    # play it tuned and fold what is left into the table, `rounds` times.
    x = np.array(DYNAMICS) - DYNAMICS[1]
    for _ in range(rounds):
        tuned = render_dynamics(notes, "{}")
        natural = [(w, v, coef + np.polyfit(x, [tuned[d][i][0] for d in DYNAMICS], 2)[::-1])
                   for i, (w, v, coef) in enumerate(natural)]
        left = np.array([[tuned[d][i][0] for d in DYNAMICS] for i in range(len(notes))])
        print(f"tuned residual before refinement: mean |err| {np.abs(left).mean():.1f}, max {np.abs(left).max():.1f}")
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
        lines.append(f"    {{written: {muz_number(w)}, variant: {v}, cents: [{', '.join(muz_number(round(c, 3)) for c in coef)}]}},")
    lines.append("];")
    TUNING_MUZ.write_text("\n".join(lines) + "\n")


def quarters(rounds=2):
    """Play every quarter tone tuned and fold how sharp it sounds into the
    shading's target (`quarter_bias` in the geometry JSON): the player shades
    a hole until the quarter tone is in tune."""
    geom = apply_player(json.loads(GEOMETRY_JSON.read_text()))
    keys = [k for k in RANGE if k - 1 in RANGE and mode(k - 1) == mode(k)]
    notes = [(k - 2.5, k - 0.5, 0) for k in keys]
    for _ in range(rounds):
        measured = render_dynamics(notes, "{}")
        bias = geom.setdefault("quarter_bias", {})
        cents_by = [np.mean([measured[d][i][0] for d in DYNAMICS]) for i in range(len(keys))]
        print(" ".join(f"{written_name(k - 1)}+:{c:+.1f}" for k, c in zip(keys, cents_by)))
        print(f"quarter tones: mean {np.mean(cents_by):+.1f}, max |err| {np.max(np.abs(cents_by)):.1f}")
        for k, c in zip(keys, cents_by):
            bias[str(k)] = bias.get(str(k), 0.0) + float(c)
        GEOMETRY_JSON.write_text(json.dumps(geom, indent=1) + "\n")
        write_muz(geom)


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
# Extra slurs for --only (not in the default grid).
EXTRA_SLURS = [("E3-B4", 52, 71), ("G3-D5", 55, 74), ("F4-B4", 65, 71), ("G4-B4", 67, 71), ("A#4-B4", 70, 71),
               ("B4-E3", 71, 52), ("A4-C5", 69, 72), ("G4-C5", 67, 72), ("F4-C5", 65, 72), ("E4-B4", 64, 71)]
SLUR_DYNAMICS = (0.3, 0.55)
KEY_TIMES = (0.0, 0.4, 1.0)
SLUR_GAP = 1.2
# Each slur is played PHASES times, the old note started k / PHASES of its
# period earlier for k = 0 .. PHASES - 1, so its oscillation stands at
# another phase when the keys move: how soon a register change lands, and how
# deep it dips, depends on that phase (Szwarcberg et al. 2025 found the same
# on an artificial mouth), which a player does not choose. `--phases` sets it.
PHASES = 1
# Whether ascents across the break lead with the register key (the `vent`
# rule of lattice-clarinet.muz); the environment's VENT=0 turns it off and
# VENT=LEAD_MS,CRACK tries another lead.
VENT = os.environ.get("VENT", "1")
VOICED = os.environ.get("VOICED", "0")
# std/woodwinds' player on a slur across a break: the tongue raised (voicing
# 0.875, eight times the resting strength) with the tract tuned to 1.05 times
# the new note (on its resting 1200 Hz, two octaves either way), relaxing
# over 150 ms; on a slur down, voicing 0.375.
def VOICE_UP(written):
    t = max(0.0, min(1.0, 0.5 + math.log2(440 * 2 ** ((written - 2 - 69) / 12) * 1.05 / 1200) / 4))
    # VOICE_UP sets the raised voicing, VOICE_TUNE=0 leaves the tract's resonance.
    v = float(os.environ.get("VOICE_UP", "0.875"))
    t = t if os.environ.get("VOICE_TUNE", "1") == "1" else 0.5
    return f", voicing: [[0ms, {v}], [150ms, 0.5]], tract: [[0ms, {t:.4f}], [150ms, 0.5]]"
VOICE_DOWN = ", voicing: [[0ms, 0.375], [150ms, 0.5]]"


def slur_cells(slurs):
    """(name, a, b, dynamic, key_time, phase) for every slur of the grid."""
    return [(name, a, b, d, kt, k / PHASES) for d in SLUR_DYNAMICS for kt in KEY_TIMES
            for name, a, b in slurs for k in range(PHASES)]


def transitions_source(model, slurs):
    use = (PROTO / "lattice-clarinet.muz").as_posix()
    notes = []
    for i, (_, a, b, d, kt, phase) in enumerate(slur_cells(slurs)):
        t = i * SLUR_GAP
        # The old note starts `phase` of its period early.
        early = phase / (440 * 2 ** ((a - 2 - 69) / 12))
        e = f"{{pressure: {d}, tongue: 0, key_time: {kt}}}"
        # An ascent across the break: the player leads with the
        # register key (lattice-clarinet.muz's `vent` rule).
        if "," in VENT:
            ms, crack = VENT.split(",")
            change = 500 + early * 1000
            curve = f"[[0ms, 0], [{change - float(ms):.4f}ms, 0], [{change:.4f}ms, {crack}]]"
        else:
            curve = f"lc.vent_curve({500 + early * 1000:.4f})"
        lead = f", vent: {curve}" if mode(a) == 1 and mode(b) > 1 and VENT != "0" else ""
        first = f"{{pressure: {d}, tongue: 0, key_time: {kt}{lead}}}"
        notes.append(f"note({a - 2}, {0.53 + early:.6f}b, at = {t - early:.6f}b).gate(1).express({first})")
        # The std player's voicing of a slur across the break
        # (std/woodwinds' `up` and `down`): VOICED=1 adds it.
        second = e
        if VOICED != "0" and mode(a) != mode(b):
            second = e[:-1] + (VOICE_UP(b) if b > a else VOICE_DOWN) + "}"
        notes.append(f"note({b - 2}, 0.5b, at = {t + 0.5:.2f}b).gate(1).express({second})")
    line = ",\n    ".join(notes)
    # The track is named as tests/clarinet_bore.rs names it, which seeds the
    # breath noise alike; TRACK draws another breath.
    return (f'use "{use}" as lc;\nuse "std/woodwinds" as w;\n'
            f"let line = stack([\n    {line}\n]);\n"
            f'song({{tempo: 60, tracks: [track("{os.environ.get("TRACK", "clarinet")}", line, {model})]}})\n')


def transitions(model, only=None):
    import subprocess
    import tempfile

    import soundfile as sf

    with tempfile.TemporaryDirectory(prefix="muz-clarinet-slurs-") as scratch:
        src = Path(scratch) / "slurs.muz"
        out = Path(scratch) / "slurs.wav"
        slurs = SLURS if only is None else [x for x in SLURS + EXTRA_SLURS if x[0] in only]
        src.write_text(transitions_source(model, slurs))
        # The block size the tests render with (tests/clarinet_bore.rs); BLOCK
        # tries another, which moves the change against the blocks.
        subprocess.run([str(MUZ), "render", str(src), "-o", str(out), "--block-size", os.environ.get("BLOCK", "128")],
                       check=True, stdout=subprocess.DEVNULL)
        audio, rate = sf.read(out, always_2d=True)
    x = audio[:, 0]
    win = int(0.015 * rate)
    hop = int(0.0025 * rate)
    print(f"{'slur':7s} {'dyn':>4s} {'keys':>4s} {'settle ms':>9s} {'dip dB':>7s} {'f/3 dB':>7s} {'3f dB':>6s} {'cents':>6s} {'phase':>5s}")
    rows = []
    for i, (name, a, b, d, kt, phase) in enumerate(slur_cells(slurs)):
        tc = i * SLUR_GAP + 0.5
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
            # Each window by its start, as tests/clarinet_bore.rs measures.
            times.append(s0 / rate - tc)
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
        # third partial against the larger of that partial's steady
        # level and the old note's level at its frequency (a burst of
        # the third mode, not the old note's own partial there, as
        # D5's second is open G's third).
        after = times >= 0.0
        low_db = 20 * math.log10(low[after].max() / a_ss)
        high_db = 20 * math.log10(high[after].max() / max(h3_ss, dft_amplitude(before, rate, 3 * fb)))
        rows.append((name, d, kt, settle, dip, low_db, high_db, pitch, phase))
        s_ms = "  never" if settle is None else f"{settle * 1000:9.1f}"
        print(f"{name:7s} {d:4.2f} {kt:4.1f} {s_ms:>9s} {dip:7.1f} {low_db:7.1f} {high_db:6.1f} {pitch:+6.1f} {phase:5.2f}")
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
    p = sub.add_parser("bands", help="the reference's impedance around its peaks -> clarinet-reference-bands.json")
    p.add_argument("xlsx", type=Path)
    p = sub.add_parser("fit")
    p.add_argument("--fresh", action="store_true", help="start from the built-in seed")
    p.add_argument("--stage", choices=["geometry", "loss", "both", "joint", "profile", "all", "bands", "local"], default="both",
                   help="geometry, then loss (both); everything at once (joint); with the bore profile (profile, all); "
                        "the profile, losses and the holes near the upper clarion's to the measured bands (bands); "
                        "the holes of LOCAL_HOLES near the start, the forked Eb4 weighted (local)")
    p.add_argument("--start", type=Path, help="start from this geometry JSON (bands)")
    p.add_argument("--anchor", type=Path, help="the prior's geometry JSON (bands; default the start)")
    p.add_argument("--prior", type=float, help=f"the prior's weight (bands; default {BAND_PRIOR})")
    p.add_argument("--set", action="append", default=[], metavar="NAME=VALUE",
                   help="override one of the tool's settings, a Python literal (e.g. UPPER_WEIGHT=3)")
    p.add_argument("--third", type=float, default=0.0, help="weight of the third impedance peaks")
    p.add_argument("--max-nfev", type=int, default=60)
    sub.add_parser("report", help="the fitted model's peaks against the reference")
    sub.add_parser("passive", help="the rendered lattice's peaks against the reference")
    sub.add_parser("muz")
    p = sub.add_parser("tune", help="measure each fingering's own pitch -> clarinet_tuning.muz")
    p.add_argument("--scale", type=float, default=1.0)
    p.add_argument("--reed-mm", type=float, default=0.0)
    p.add_argument("--rounds", type=int, default=3, help="refinement passes")
    sub.add_parser("check", help="the tuned instrument's pitch at three dynamics")
    p = sub.add_parser("stability", help="the clarion and altissimo's pitch and register from pressure 0.25 to 0.6")
    p.add_argument("--untuned", action="store_true", help="with the mouthpiece length still")
    p.add_argument("--only", nargs="+", help="written notes, e.g. G5 A#5* (an alternate)")
    p = sub.add_parser("window", help="the kPa over which each upper clarion note speaks")
    p.add_argument("--first", type=int, default=79, help="the lowest written note (default G5)")
    sub.add_parser("quarters", help="play the quarter tones and fold their error into the shading")
    p = sub.add_parser("slurs", help="settle time, dip and stray modes over a grid of slurs")
    p.add_argument("--model", default="lc.clarinet({velocity_breath: 0})",
                   help="instrument expression (lc: the lattice clarinet, w: std/woodwinds)")
    p.add_argument("--only", nargs="+", help="slurs by name, e.g. A4-B4")
    p.add_argument("--key-times", nargs="+", type=float, help="key_time values (default 0 0.4 1.0)")
    p.add_argument("--phases", type=int, default=1, help="phases of the old note at the change, per slur")
    args = parser.parse_args()
    if args.command == "slurs":
        if args.key_times:
            globals()["KEY_TIMES"] = tuple(args.key_times)
        globals()["PHASES"] = args.phases
        transitions(args.model, args.only)
        return
    if args.command == "tune":
        tune(args.scale, args.reed_mm, args.rounds)
        return
    if args.command == "check":
        check_tuning()
        return
    if args.command == "stability":
        stability(not args.untuned, args.only)
        return
    if args.command == "window":
        window(args.first)
        return
    if args.command == "quarters":
        quarters()
        return

    if args.command == "bands":
        bands = reference_bands(args.xlsx)
        REFERENCE_BANDS.write_text(json.dumps(bands, separators=(",", ":")) + "\n")
        print(f"wrote {len(bands['fingerings'])} fingerings to {REFERENCE_BANDS}")
        return
    if args.command == "peaks":
        ref = reference_peaks(args.xlsx)
        REFERENCE.write_text(json.dumps(ref, indent=1) + "\n")
        print(f"wrote {len(ref)} fingerings to {REFERENCE}")
        return
    ref = json.loads(REFERENCE.read_text())
    start = getattr(args, "start", None) or GEOMETRY_JSON
    geom = initial_geometry() if (args.command == "fit" and args.fresh) or not start.exists() else json.loads(start.read_text())
    geom["loss"].setdefault("register", geom["loss"]["hole"])
    apply_player(geom)
    with_profile(geom, flare=args.command == "fit" and args.stage in ("profile", "all"))
    if args.command == "fit":
        import ast
        for item in args.set:
            name, value = item.split("=", 1)
            if name not in globals():
                raise SystemExit(f"no setting {name}")
            globals()[name] = ast.literal_eval(value)
        globals()["THIRD_PEAK"] = args.third
        if args.stage == "profile":
            geom = fit(geom, ref, {"geometry", "profile"}, widths=False, max_nfev=args.max_nfev)
            geom = fit(geom, ref, {"loss"}, widths=True, max_nfev=args.max_nfev)
        if args.stage == "all":
            geom = fit(geom, ref, {"geometry", "profile", "loss"}, widths=True, max_nfev=args.max_nfev)
        if args.stage in ("geometry", "both"):
            geom = fit(geom, ref, {"geometry"}, widths=False, max_nfev=args.max_nfev)
        if args.stage in ("loss", "both"):
            geom = fit(geom, ref, {"loss"}, widths=True, max_nfev=args.max_nfev)
        if args.stage == "joint":
            geom = fit(geom, ref, {"geometry", "loss"}, widths=True, max_nfev=args.max_nfev)
        if args.stage == "local":
            geom = fit_local(geom, ref, args.max_nfev)
        if args.stage == "bands":
            anchor = json.loads(args.anchor.read_text()) if args.anchor else None
            geom = fit_bands(geom, json.loads(REFERENCE_BANDS.read_text()), args.max_nfev, args.prior, anchor)
        GEOMETRY_JSON.write_text(json.dumps(geom, indent=1) + "\n")
        write_muz(geom)
        report(geom, ref)
        if REFERENCE_BANDS.exists():
            band_report(geom, json.loads(REFERENCE_BANDS.read_text()))
    elif args.command == "report":
        report(geom, ref)
        if REFERENCE_BANDS.exists():
            band_report(geom, json.loads(REFERENCE_BANDS.read_text()))
    elif args.command == "passive":
        passive(ref)
    elif args.command == "muz":
        write_muz(geom)


if __name__ == "__main__":
    main()
