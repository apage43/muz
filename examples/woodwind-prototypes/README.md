# Fixed-bore clarinet experiment

`fixed-bore.muz` uses one nonlinear reed with three bidirectional delay
segments and two junctions. The register vent is a filtered local pressure
release. The G tone hole blends the local scattering between bore
transmission and reflection from an open end. This preserves the pressure
history in each segment. The hole is a simplified termination, with
effective travel times fitted to the two pitches.

Render the slurs in either direction from the muz-core checkout:

```sh
target/release/muz render examples/woodwind-prototypes/register-slurs.muz \
  -o out/register-slurs.wav
```

The experiment has two fingerings, concert A4 and F4. Pitches select between
those fingerings. This is a test of the register transition with a fixed
bore, before adding a tone-hole lattice and calibrating the full range.
It uses the existing patch graph at 2× oversampling.

## Full-range lattice

`clarinet_bore.muz` extends the experiment to MIDI 50–87, with 18 effective
tone-hole positions and two register vents. `lattice-clarinet.muz` plays it
with the `std/woodwinds` player. `lattice-slurs.muz` renders the same slurs
through it:

```sh
target/release/muz render examples/woodwind-prototypes/lattice-slurs.muz \
  -o out/lattice-slurs.wav
```

These are the model's known limits:

- An open hole stops transmission to the bore below it, and only the hole
  for the current note opens. The lattice behaves as a movable termination,
  with no open holes below and no cross or alternate fingerings.
- The per-note `clarion_cutoffs` table tunes the third mode through the
  reflection filters, so tuning and timbre are coupled.
- The patch does not use the player's tuned frequency. Calibration, drift,
  vibrato cents, and bends do not reach it.
- One voice costs about 70% of a 64-frame deadline at 48 kHz.

`reed_dynamics.muz` provides a bandpass correction to the reed opening and an
optional single upstream resonance (`voicing`). The correction approximates
reed inertia; it is not a mechanical reed. `reed-voicing.muz` demonstrates
both. `lattice-clarinet.muz` enables them through
`lattice: {reed_mode, tract}`.

## Passive measurements

The passive examples inject a broadband pulse at the reed plane with the
exciter removed. Measure reflection, normalized input impedance, and
radiation with the same tool:

```sh
uv run tools/woodwind-passive.py examples/woodwind-prototypes/passive-cylinder.muz --radiated
uv run tools/woodwind-passive.py examples/woodwind-prototypes/passive-clarinet.muz --track open_g --radiated
uv run tools/woodwind-passive.py examples/woodwind-prototypes/passive-clarinet.muz --track clarion_b --radiated
```

The cylinder is a reference case with a 2 ms round trip and a first
impedance peak near 250 Hz. The clarinet example exercises the full fixed
bore at the two fingerings from the register-slur example. These results
describe held fingerings at the requested sample rate.
