# Fixed-bore clarinet experiments

## Two fingerings

`fixed-bore.muz` uses one nonlinear reed with three bidirectional delay
segments and two junctions. The register vent is a filtered local pressure
release. The G tone hole blends the local scattering between bore
transmission and reflection from an open end. The experiment has two
fingerings, concert A4 and F4, and tests the register transition with a
fixed bore:

```sh
target/release/muz render examples/woodwind-prototypes/register-slurs.muz \
  -o out/register-slurs.wav
```

## Tone-hole lattice clarinet

`clarinet_bore.muz` is a fixed-geometry B-flat clarinet, written E3–F6
(sounding MIDI 50–87). `lattice-clarinet.muz` plays it with the
`std/woodwinds` player's breath and articulation and a player model of its
own. Its parts:

- **Bore.** Cylinder segments between 20 holes (17 tone holes, two ring
  vents and the register hole) and a bell, as cubic delay lines, with a
  loss shelf on the mouthpiece segment. Wall loss is lumped onto every
  fifth segment (each segment loses under 0.5%). Every segment stays live:
  an open hole does not cut off the bore below it. The bore may change
  radius: the top segment is the mouthpiece's chamber, the main bore steps
  at three holes (`t`, `cs`, `gsds`: the upper joint's reverse taper and
  the lower joint's widening, as a polycylinder). Each step scatters with
  `(S_above - S_below) / (S_above + S_below)` and a hole at a step sees
  the junction's mean area. The committed geometry steps from 7.25 mm
  below the thumb hole to 7.30 mm below C#/G# and 7.69 mm below G#/D#; the
  mouthpiece's chamber is held at the bore's radius (on the previous reed
  a narrower one started the soft upper clarion in the low register, see
  the limits).
- **Bell.** The lower joint and bell flare as a catenoid, `r = r_j
  cosh((x - x_s) / x0)`: a clarinet bell is nearly catenoidal (Nederveen,
  as Moers & Kergomard 2011 quote him, x0 = 85 mm). Below the last hole
  the lower joint runs on at 7.69 mm for 8 mm, and the horn is cut into
  four cylinders of its mean area (9.0, 12.2, 17.3 and 24.9 mm over 40, 32,
  30 and 29 mm), shorter where it flares faster (twelve move the first
  two peaks by under a cent on average and the third peaks by about ten
  cents; the fit is to four). (A step at the last hole itself would cost the renderer a
  junction of its own, 3-4 Mc/s.) Its mouth radiates as an unflanged
  pipe of its radius (Levine & Schwinger 1948): the reflection is
  `-|R| e^(-2jkl)`, which Silva, Guillemain, Kergomard, Mallaroni & Norris
  (2009, JSV 322:255) give in closed form and one state-variable lowpass
  matches, `R = -lowpass2(1.8898 c / a, q 0.4213)`, within 0.01 of |R| and
  0.02 a of the end correction up to ka = 1 (2 kHz on this mouth): lossless
  at low frequencies, as radiation is. The fitted horn: x_s 506 mm, above
  the last hole (518 mm), x0 74.5 mm, the mouth 29.5 mm in radius (a
  B-flat bell's is 28-33 mm) at 657 mm. (Until
  round 10 the bell was four fitted cylinders, stepping at once from 7.69
  to 12.0 mm, and a fitted reflection `-0.918 lowpass1(3.14 kHz)^2`, which
  lost 8% at every frequency.) The bell radiates its volume flow.
- **Holes.** Each hole is a shunt junction: its open path, an inertance
  with a viscous resistance (narrower holes are lossier), in parallel with
  its chimney's compliance (van Walstijn & Scavone 2000). Normalized, the
  shunt's admittance over two is `A / (rho + s) + s T`, for `A` the open
  path's admittance at the key's travel, `rho` its resistance over inertance
  and `T` half the chimney's volume over the bore's area times c, so the
  junction's pressure is `(s + rho) / (T s^2 + (1 + rho T) s + rho + A)` of
  the arriving waves: a second-order lowpass with a zero. It is computed as
  the open path's release, a state-variable lowpass of depth `A / (rho + A)`
  at `sqrt((rho + A) / T)` with q `sqrt((rho + A) / T) T / (1 + rho T)`
  (0.01-0.3), followed by the compliance's one-pole on what is left, which
  is exact. (Until this round the open path was a one-pole release cascaded
  with the compliance's highpass, which added a cross term, the product of
  the two admittances: at an open hole a conductance of about `A T`, 5-14%
  of the open path's admittance on the large holes, damping the peaks above
  the open-hole cutoff.) A closed hole's open path vanishes and its
  compliance stays exact. The open path's release is its flow, which
  radiates. A pad and a finger over its hole both act as a curtain,
  `g = (1 + q) x / (x + q)`, so most of the change happens near the seat.
  The open path's depth (its admittance's share) scales the pressure that
  drives it, as `du/dt = A p - rho u` has it, not its output: an opening
  key's flow starts from rest, and a seated hole's lowpass is driven by
  10^-12 of the pressure (-240 dB; driven by an exact zero, its state
  decays into subnormal floats, which cost this engine 5-7 Mc/s at the
  99th percentile, docs/composer-friction.md). As a pad or finger seats,
  the gap's viscous resistance grows
  without bound (`1 + (0.02 / (g + 0.002))^2` times the hole's: 1% more
  at a fifth of the admittance, 0.04% open), so a closing hole's flow is
  choked within a fraction of a millisecond. (Until round 9 the depth
  scaled the filter's output, whose state kept following the pressure
  behind a seated pad. Scaling the drive without the gap's resistance
  left a closing hole leaking for 7–30 ms, its open path's slow pole, and
  slowed the ascents across the break to 40 ms, mean 25.)
  The register hole also has a jet resistance growing with its flow (Atig,
  Dalmont & Gilbert 2004; Dalmont et al. 2002). The open path's flow `u`
  obeys `du/dt = A p - (rho + rho_jet) u`, so the jet's part is a pressure
  `2 C |u| u / g` against the junction's, read one sample late: it drives
  the open path's lowpass, whose coefficients stay the linear hole's. (Until
  round 8 the jet moved the lowpass's cutoff, q and depth with the flow
  every sample, which scaled the flow at once by the steady-state release
  and so let an inertance's flow jump; that also cost a coefficient refill,
  a square root and a reciprocal every sample.) The register hole is the real tube, 1.52 mm
  in radius and 13.9 mm long (YCL-250: 1.5 and 12.5; Buffet Festival 1.5
  and 13, Colinot et al. 2026), projecting 3.9 mm into the bore (an
  unflanged inner end, its wall displacing bore volume), with the
  literature's jet coefficient C = 0.6 (0.6–0.7).
- **Geometry.** `clarinet_geometry.muz` is generated by
  `tools/clarinet-lattice.py` from `clarinet-geometry.json`, which also
  carries the player settings the geometry was played with (`player`: the
  register jet's C, open G and G# leaving the little finger free, and the
  register key's lead on ascents). The tool fits hole positions, radii,
  chimneys, the bore's profile and the losses to the measured input impedance of a real B-flat
  clarinet, UNSW's `ImpedClarinet.xlsx` (Dickens, Smith & Wolfe). `peaks`
  reduces it to the peaks of `clarinet-reference.json` and `bands` to
  `clarinet-reference-bands.json`: log |Z| and phase within a −3 dB width
  of each fingering's first five peaks below 2.6 kHz. Its frequency-domain
  model is the discrete-time system the renderer runs (third-order
  Lagrange delays, prewarped trapezoidal one-poles at 96 kHz), so the
  rendered lattice matches the fit. Two ring vents follow a measured
  Yamaha YCL-250 (Moers & Kergomard 2011): `v`, open only with the thumb
  and LH1 both up (open G), and the sliver `sl` above LH2. The result is an
  effective geometry, not a drawing. `fit --stage geometry` fits the
  picked peaks' frequencies and widths and, for the clarion and
  altissimo, the first peak's height against the sounding one's (weight 2
  and 10); `fit --stage bands` fits the bands, smooth in every parameter,
  with their heights, a prior toward the start, and the profile, losses
  and the holes near the upper clarion's open holes free (`BAND_HOLES`;
  `--set BAND_HOLES='"all"'` frees every hole). Both weigh the
  alternates double and hold the throat notes as played (right hand down)
  near the measured peaks. How the committed geometry was reached is in
  the limits below.
- **Fingerings.** Each written note maps to its open holes, checked
  against UNSW's fingering diagrams and Boehm charts: chalumeau, the
  clarion a twelfth up with the register key, the altissimo on the fifth
  harmonic with LH1 vented (and the G#/D# key from D6 up). The throat notes
  are played with the right hand down, as players do to cross the break.
  Open G and G# do not need the little finger: it stays where the previous
  note left it (its keys' slews take a minute there), up when they are
  tongued or reached from D5, so a slur up to D5 or D#5 moves no
  little-finger key, and down when reached from B4 or C5.
  Alternates (`fingering` control): the forked Eb4/Bb5 and the throat Bb4
  with the A key and side trill key. Quarter tones shade the changing holes
  to the fraction `shade` that sounds between the semitones
  (`tools/clarinet-lattice.py quarters` folds the played error into it).
  Across the break and into the altissimo the fingering changes just below
  the quarter tone and the player bends the upper fingering down.
- **Keys.** A finger or pad leaves its stop at rest and lands on its seat
  or stop at speed: the travel is a slew toward a target past either end,
  read through curves that hold their end values, so each motion ends at a
  definite moment and its filters rest. A finger moves in 5 ms and a spring
  in 8 ms from the note's change (Almeida, Chow, Smith & Wolfe 2009
  measured about 10 and 16 ms whole motions on the flute; the note's
  change already includes the player's anticipation). Fingers arrive in a
  fixed order (the register key; the thumb's hole with the left hand; the
  right hand; the little fingers), `stagger` times the note's `key_time`
  (0–1, default 0.6) apart, so a change of several fingers spreads over
  up to 18 ms and by default over 11 ms (professional flutists' fingers
  arrive within 10–20 ms of each other: Almeida, Chow, Smith & Wolfe 2009;
  Almeida et al. 2010; it was 0.4, 7 ms, before round 8); a key may take a different place opening and
  closing. The thumb rolls off the register key before it leaves its hole:
  with the thumb's hole moving ahead of the left hand, a sloppy slur down
  from D5 (`key_time` 1) passed through the thumb-vented D5 fingering, and
  one in five landed on open G's twelfth. A note's `vent` expression (0–1)
  lifts the register key by that much of its travel on top of the
  fingering; `lc.vented(rule)` is a note
  rule for `w.perform` or `w.play` that writes it over the last
  `vent_lead` (35 ms) before a slur up across the break, as players lead
  with the thumb and register key (`lattice-slurs.muz` uses it). The
  geometry sets the lift (`vent_crack`) and the lead; `slurs` and the
  test's grid play ascents with them. On this bore and reed the lift is 0
  (below).
- **Reed and lip.** The reed is `reed_dynamics.lip_reed` played by
  `reed_dynamics.dynamic`: a mass, spring and damper under the player's lip,
  its flow solved in closed form, in fixed units (pressures over 5 kPa).
  The lip's force in newtons loads the reed toward the lay, so its static
  closing pressure falls along a line, p_M = 9.4 kPa − F / 3.5 cm², and its
  rest opening with it (a linear spring from the 1.05 mm tip opening of
  Almeida et al.'s mouthpiece; an effective channel width of 11 mm). The
  played extinction sits about 15% above p_M, so open G extinguishes along
  Almeida, George, Smith & Wolfe's (2013) measured line: 3.3 kPa per newton,
  6.2 kPa at 1.4 N (their G4: about 6). The lip also shortens the reed's
  vibrating length and damps it: the resonance rises from 1.5 kHz free to
  2.5 kHz at 1.4 N, and the damping (1/Q) grows with the contact area from
  0.4 to 0.7 (estimated). Mapped on the (P, F) plane as Almeida et al. did,
  open G plays below that line and above a threshold falling from 3 kPa at
  0.5 N to 1.5 kPa at 2.5 N (theirs falls from about 4 to 2.5: the model,
  like theory, starts more easily than their machine); its pitch rises with
  F and P over about 50 cents (theirs 60), along contours of slope
  −0.23 N/kPa (theirs −0.3); it is loudest at high P and low F; and it does
  not squeak. G5 plays over their whole region (0.4–0.9 N, 3.5–7 kPa).
- **Player.** The lip force is 1.4 N in the chalumeau and throat (Almeida
  et al.'s G3 and G4), 0.65 N in the clarion (their G5, 0.4–0.9 N) and
  1.4 N in the altissimo (estimated), at dynamic 0.5. Along a crescendo it
  falls 0.3 N per kPa of blowing pressure, along their contours of equal
  pitch, so a crescendo keeps its pitch and a soft note is played with a
  firmer lip. The lip moves with the fingers. The lip's force, the tuning
  that rides on it and the upper clarion's voicing follow the dynamic the
  player plans (the note's velocity and pressure curve), not the tongue's
  burst, flutter or a breath vibrato, which only the breath carries: a
  player sets the embouchure for a dynamic, not for each pulse of the
  tongue. (That also lets the reed's and the tract's coefficients rest
  between the player's events.) The tongue is std's
  (`std/physical.wind_player`): its tip on the reed holds the channel shut
  over the release (`opening` scales the reed's opening) and a back stop
  holds the breath until the mouth fills (`fill` scales the blowing
  pressure).
- **Breath.** The blowing pressure is in kPa, linear in the dynamic between
  values at dynamic 0 and 1 on each register's lowest note: 1.9–6.8 kPa in
  the chalumeau, 2.9–5.7 kPa in the clarion (raised 15% on B4–E5) and
  2.2–5.0 kPa in the altissimo (estimated). Up each register it falls a
  little at the soft end and 15–25% per octave at the loud end. These follow
  Fuks & Sundberg's (1999) players (2.3–3.0 kPa pp, 4.3–6.4 kPa ff, falling
  with pitch within a register, highest on the lowest clarion notes).
  Breath turbulence is a noise flow at the reed channel, so it radiates
  through the bore.
- **Vocal tract.** One resonance upstream of the reed, steady at 1.3 kHz and
  1.5 bore characteristic impedances for most of the range, as Fritz &
  Wolfe (2005) measured in normal playing (1.0–1.4 kHz, 1.3–4 Zc, one
  configuration over most of the range). Playing loud in the upper clarion
  (A#5–C6) the player strengthens it to 4 Zc, the top of their range, its
  resonance kept, as the breath rises from 3 to 4.5 kPa (players voice
  high notes played loud: Chen, Smith & Wolfe 2009; Pàmies-Vilà et al.
  2020); these notes then hold their register to 4.5–4.8 kPa, past the
  player's forte (4.2–4.4 kPa), where they fell to the first impedance
  peak at 3.3–4.5 kPa. Tuned onto the note or 120 Hz above it, the tract
  holds them less far (C6 to 3.3–3.6 kPa), and the strengthening starts at
  A#5, so D5–E5 keep the resting tract and do not squeak. In the
  altissimo the player tunes it 120 Hz above the note at 10 Zc (Chen,
  Smith & Wolfe 2009; estimated), and on a slur up into it or a tongued
  attack there voices the new note: for 40 ms the resonance moves onto it,
  10 stronger. The std player's `voicing` and `tract` note controls (0.5
  neutral) scale the strength (twice per 0.125) and the resonance (two
  octaves either way) of the tract so set. Perform it with `lc.spec`:
  std/woodwinds' clarinet with its resting tract at 1.3 kHz, so the
  `tract` it writes means what it says on the lattice, and without std's
  altissimo tuning, which the lattice's player already does
  (`phrase-line.muz`, shared with the std clarinet, keeps std's profile; it
  stays below the altissimo).
- **Tuning.** The player keeps the note in tune with a variable mouthpiece
  length, from the tuned frequency against the fingering's own pitch at
  the current dynamic (`tools/clarinet-lattice.py tune`, with three passes
  that fold the residual into `clarinet_tuning.muz`), between 70 cents down
  and 50 up.
- **Radiation.** Each open hole and the bell radiate as monopoles: the
  far-field pressure is the time derivative of their summed volume flow
  (an open hole's flow is twice its pressure release; the bell's the
  incident less the reflected wave). Their spacing and the bell's
  directivity are left out.
- **External interaction.** The openings also radiate into one another
  (Lefebvre, Scavone & Kergomard 2013, "External tonehole interactions in
  woodwind instruments", Acta Acustica 99:975, after Pritchard 1960): the
  pressure outside hole i is the field of the others' volume flows,
  monopoles near a baffle, `rho / (2 pi d) dU/dt (t - d / c)`, which is a
  mutual inertance (3–10% of a large hole's own at 1–4 cm) and, through
  the delay, a shared radiation resistance. It opposes the hole's own
  flow, so it enters the open path's drive beside the junction's
  pressure, as the jet does; at the bell it enters the reflection as
  `(1 - R) e / 2`. `clarinet_bore.muz` can couple any openings the
  geometry names (`external`), each to its nearest neighbours among them:
  each one's flow is differentiated once (a highpass far above the band),
  reaches its neighbours through a cubic line `d / c` long, and is
  weighted by `S / (2 pi d c)`. `tools/clarinet-lattice.py` solves the
  coupled lattice exactly at each frequency (`coupled_impedance`: every
  wave is affine in the incident wave and the outside pressures, closed by
  `e = M U`), and the rendered lattice matches it to 0.1% in |Z|. The
  committed geometry couples none: round 9 coupled RH1–RH3, and on the
  physical bell that still damps the altissimo's sounding peaks (D6–F6's
  first-to-third ratios 8–11% above UNSW's, against 1–4% without) and
  C6's third (130 cents off, against 7), and
  improves nothing else (first peaks 4.1 cents mean either way, the
  forked Eb4 +8/−9 against +8/−8); every opening and the bell coupled to
  their neighbours are worse still. Its 14 nodes and 7–8 Mc/s are saved.

Render the slurs, a phrase, the phrase on both clarinets, and the range
with alternates, quarter tones and a bend:

```sh
target/release/muz render examples/woodwind-prototypes/lattice-slurs.muz -o out/lattice-slurs.wav
target/release/muz render examples/woodwind-prototypes/lattice-phrase.muz -o out/lattice-phrase.wav
target/release/muz render examples/woodwind-prototypes/blend-phrase.muz -o out/blend-phrase.wav
target/release/muz render examples/woodwind-prototypes/lattice-fingerings.muz -o out/lattice-fingerings.wav
```

### Measurements (2026-11-14, round 10: the physical bell)

- **Passive impedance.** The model against the reference over the 38
  standard fingerings: first peak 4.1 cents mean error (round 9: 4.8;
  max 13.2; 95% within 10), second 2.6 (2.2; max 7.0), third 24.7 (30.5):
  F4's and C6's 12 and 7 cents (round 9: 125 and 129; round 8: 7 and 4),
  69–78 on G#4–A#4 (68–76) and F#4 and G4 149 and 222 (149 and 221, see
  the limits); widths 0.73–1.16 of the reference (10–90%). The
  alternates: forked Eb4 +8/−8 (+8/−9), side Bb4 −2/+3. The altissimo's
  first-to-third ratios stand within 5% of the reference's from C#6 to
  F6 (round 9: 9–12% above from D6 up; round 8: within 4%). E3's peaks
  stand 0.84, 0.97 and 1.00 of the reference's from the first and
  0.48–0.65 from the fourth to the seventh (round 9: 0.79, 0.90, 0.95 and
  0.23–0.59; see the limits for why the reference's high peaks stand
  higher).
  The bands are 65.0% off in rms log |Z| (63.5%) and their heights 37%
  (43%). The rendered lattice over 15 fingerings (`passive`): 3.6 cents
  mean (4.1), max 13.6, 93% within 10; the forked Eb4 +8.5/−8.2. The
  clarion's first-to-second ratio is from 2% above to 19% below the
  reference's (C5 −19%, G#5 −12%), so the clarion speaks a little more
  easily than the measured impedance alone would let it.
- **Untuned pitch.** The standard fingerings' own pitches, with the
  embouchure still, lie within −34 and +22 cents of their notes at
  pressures 0.3–0.6 (mean +5). The throat Bb4 is the outlier, 21–34 cents
  flat.
- **Tuning.** All 38 notes and the three alternates are within 0.3 cents
  at pressures 0.3, 0.45 and 0.6 (`check`; `tune --rounds 4`), measured
  with the player's pitch still. The quarter tones are within 0.1 cents
  after their last fold (`quarters`; 0.2 before it).
- **Stability.** From pressure 0.25 to 0.6 in steps of 0.05 (`stability`),
  no clarion or altissimo note (nor the side Bb4 and forked Bb5) jumps
  more than 15 cents between steps or within a note, and the twelfth
  below stays under −85 dB (round 9: −84). On round 8 the forked Bb5
  carried nothing off its harmonics above −56 dB in the mouthpiece from
  dynamic 0.15 to 0.9 (−61 dB on round 7; its multiphonic was 3–8 dB under
  the note).
- **Upper clarion.** Along the player's crescendo past its loud end
  (`window`), every note from G5 to C6 speaks from 1.9 to 6.7 kPa, as on
  rounds 8 and 9 (round 7: A#5 to 4.5, B5 and C6 to 4.8). Without the jet
  they fell to the first peak at 2.9–4.3 kPa.
- **Dynamic range.** On the cylindrical bore every fingering spoke in its
  own register, with the twelfth below at −78 dB or less, from pressure
  0.15 to 0.9; not re-measured on this bore.
- **Slurs.** The grid covers 9 slurs across and within the registers, at
  pressures 0.3 and 0.55 and `key_time` 0, 0.4 and 1.0 (`slurs`, measured
  as the test measures them: each 15 ms window by its start, the breath
  seeded as the test's), each played at four phases of the old note at
  the change (`--phases 4`): how soon a register change lands and how
  deep it dips depends on that phase, which a player does not choose
  (below). Over 24 breath seeds (`TRACK=name`; 1,152 ascents across the
  break, 4,032 other slurs): every slur keeps the twelfth below at −14.2
  dB or less (round 9: −14.0) and settles within 3.0 cents (3.2; the
  player's drift). Within a register and descending, slurs settle within
  10 ms with no dip over 2.4 dB (2.1). Ascents settle in 10–35 ms (mean
  20.9; round 9 20.8) and dip at most 4.0 dB (4.6). No slur shows a
  third-mode burst over 3.6 dB (5.0), measured against the larger of the
  partial's steady level and the old note's level at its frequency. No
  D5–G4 lands on open G's twelfth, but most slurs down from the clarion
  to the chalumeau do (limits). The test's grid at four phases
  (`out/slurs-grid.txt`) is summarized in the commit.
- **Radiation.** The bell's mouth now radiates as an unflanged pipe:
  lossless at low frequencies, where the fitted reflection lost 8%.
- **Level.** At a fixed pressure the registers' mean levels (rms) rose
  about 5 dB from the chalumeau to the clarion and 4 dB to the altissimo
  on the cylindrical bore; not re-measured.
- **Cost.** On the engine whose packs keep their lanes' state side by
  side (a6e3ec8), one voice costs 96–97 megacycles per second over the 4 s
  of `woodwind_callback` on `lattice-slurs.muz` and 160–161 at the 99th
  percentile, against 98 and 161–163 for main's lattice (round 8's model
  with the seat-free open path) and 109 and 180–182 for round 9, measured
  alternately (compiled with vector lanes, 64 frames, least of three
  renders): about 2.4% and 4.0% of a 4 GHz core. Leaving out round 9's
  coupling saves its 14 nodes and 7–8 Mc/s; scaling the open path's drive
  by its depth costs 1–2, and driven by an exact zero 5–7 at the 99th
  percentile (subnormal floats), so a seated hole keeps a −240 dB leak;
  folding the compliance's term into the outgoing sums, the mouthpiece's
  shelf into its gains and the bell's flow into its lowpass saves about
  20 nodes. A step in the bore at the last hole (where the fit first put the
  bell's flare) cost 3–4 more, so the bell begins 8 mm below it. It uses
  623 of its 1,024 nodes (main 646, round 9 660).

```sh
uv run tools/clarinet-lattice.py report    # fitted model vs reference
uv run tools/clarinet-lattice.py passive   # rendered lattice vs reference
uv run tools/clarinet-lattice.py tune      # each fingering's own pitch, refined
uv run tools/clarinet-lattice.py quarters  # quarter tones folded into the shading
uv run tools/clarinet-lattice.py check     # tuning at three dynamics
uv run tools/clarinet-lattice.py stability # clarion and altissimo, pressure 0.25-0.6
uv run tools/clarinet-lattice.py window    # the kPa over which the upper clarion speaks
uv run tools/clarinet-lattice.py slurs --phases 4  # the slur grid at four phases of the old note
uv run tools/clarinet-lattice.py descents         # slurs down from the clarion: do they land?
uv run tools/clarinet-lattice.py bands ImpedClarinet.xlsx   # the reference's bands
uv run tools/clarinet-lattice.py fit --stage bands          # fit the bands (CLARINET_DIR: a copy)
```

### Remaining limits

- **Slurs down from the clarion stay in the clarion.** Of 756 slurs from
  written B4–C6 down to C4–A#4 (`descents`: pressures 0.3 and 0.55,
  `key_time` 0, 0.6 and 1), 380 land on the new fingering's twelfth instead
  of its note (main's round-8 model: 373): every one to D4 from C5 up,
  most to E4–F#4, about half to G4–A4 from F5 up, none to C4 or A#4. The
  slur grid's descents across the break (B4–A4, B4–G4, D5–G4) all land,
  so no gate saw it until round 10. The old regime does not hang on: as
  the register key seats, the oscillation passes within 5 ms to the
  nearest resonance of the changing fingering, the new one's second (E5
  to E4: from 587 Hz straight to 881 Hz, the fundamental never above
  −55 dB), and with the register hole shut that second register is
  stable. On an artificial mouth the second register with the register
  hole shut did not appear in pressure ramps on a Buffet clarinet's Eb4
  (Szwarcberg, Colinot et al. 2025, arXiv:2506.03875), though a player
  can force it; players slur down from the clarion every day, so the
  model's is too stable. Tried on 50 of these slurs (37 stay up): reed
  damping twice the estimated 0.4–0.7 (14), the chalumeau's lip at the
  clarion's 0.65 N (23), 15% less breath in the clarion (27), the
  visco-thermal wall loss's frequency dependence (92 of 252 against 127,
  but the effective lattice does not refit to it, above); a lip that
  slews over 40 ms, a weaker or no vocal tract, a stiffer or softer reed
  and a wider channel change nothing. The reed's damping, its channel and
  the lip's force at 1.4 N in the chalumeau are estimates or one machine's
  settings (below), and Silva, Kergomard, Vergez & Gilbert (2008, JASA
  124:3284) find the second register's
  thresholds most sensitive to the reed's damping; which of them the
  instrument gets wrong is open.
- **Ascents across the break are a short new attack, as players' are.**
  Written A4–B4 and G4–D5 settle in 10–35 ms (mean 21; 8–30, mean 19,
  before an opening key's flow started from rest in round 9) and dip up
  to 4.0 dB, against 10 ms or less elsewhere. The cause is in the bore: as
  the left hand seals over the open throat key, the throat note's
  resonance does not turn into the clarion's. It slides down toward the
  long tube's first mode (A4: from 422 Hz with the register key open to
  396 Hz, then 308 Hz once the fingers seal), the reed follows it, and the
  clarion resonance comes down from above as a second branch, so the
  clarion grows anew. The spread is the phase of the old note's
  oscillation when the keys move: with the breath and the pitch still,
  every seed gives the same slur, and the old note started a fraction of
  its period earlier moves G4–D5 between 8 and 20 ms with the keys
  together and between 18 and 30 ms with them spread. What seemed a tail over breath
  seeds (one ascent in thirteen past 30 ms or 3 dB on round 7) was the
  player's pitch drift moving that phase. Szwarcberg et al. (2025,
  arXiv:2506.03875) found the same on an artificial mouth: whether and how
  soon a jump to the clarion lands depends, near its thresholds, on the
  oscillation's phase when the register hole opens, which opened over
  30–50 ms there.
  - *What players do.* No study we found measures the duration or the
    level dip of a slurred B-flat4–B4 or G4–D5 by players. The nearest:
    professional flutists' changes of three and four fingers spend
    13 ± 9 and 21 ± 11 ms in passing fingerings (amateurs 25 ± 16 and
    34 ± 14), keys move in about 10 ms pushed and 16 ms sprung, and
    professionals keep their fingers within 10–20 ms of each other
    (Almeida, Chow, Smith & Wolfe 2009, JASA 126:1521; Almeida et al.,
    ISMA 2010); a clarinettist's legato between registers shows a release
    and then an attack in the mouthpiece, as if tongued (Pàmies-Vilà,
    Hofmann & Chatziioannou, ISMA 2019, eleven players), whose attacks
    take about 25–30 ms (5–95% of the envelope); within the chalumeau
    each legato change of note shows "a small drop in the sound level"
    (Pàmies-Vilà et al. 2018, Front. Psychol. 9:617); MIPCAT's recordings
    find that a change of pitch "is usually accompanied by an amplitude
    dip" (Almeida et al. 2023, Front. Signal Process. 3:1089366); and a
    regime left unsupported decays at about 0.4 dB per ms (the
    tongue-stopped decay, Li, Almeida, Smith & Wolfe 2016, JASA 139:825),
    so 13 ± 9 ms in passing gives a few dB. A clarinet thesis gives
    slurred transitions of 60 ± 40 ms in mouthpiece pressure (Adkison
    2019, Rollins College; read only in a search summary, its intervals
    unknown).
  - *The gate.* From these, an ascent across the break should settle
    within 45 ms (a professional's passing time, 21 + 2 × 11 ms, with
    the keys' travel) and dip less than 6 dB, and the ascents should
    settle within 30 ms on average. The test plays the grid on one breath
    and its ascents at four phases on another, 60 ascents; the model
    settles at the fast, shallow end of the players' range. `key_time`
    0.6 (11 ms of stagger) is now the default, within professionals'
    10–20 ms.
  - Tried on round 7, over eight or 16 seeds: leading with the register
    key (10–35 ms, half or full) deepens the dips (up to 4.9–5.8 dB); the
    throat keys closing before the left hand removes the deep dips but
    slows a third of the sloppy ascents past 30 ms; the thumb covering its
    hole before or after the left hand slows them; a lip that relaxes over
    10–50 ms deepens the dips; std's voicing of a slur up (`VOICED=1`)
    settles them sooner but dips one in ten 3–4 dB; a firmer throat lip
    (1.7 N) removes the deep dips only by making the throat note quieter.
    The jet as a pressure (round 8) settles G4–D5 about 2 ms sooner on
    average and dips it at most 1.3 dB deeper (4.7 against 3.4 over 12
    phases).
- **Throat Bb4 is flat** by 19–33 cents untuned at pressures 0.3–0.6
  (39–47 when the jet moved the hole's coefficients; on the previous reed
  −7 at 0.3 and −41 from mf), and the lip corrects it. Its
  fingering matches UNSW's chart (A and G# keys, register key, thumb hole
  open, right hand down) and its first two impedance peaks are within
  1.5 cents of UNSW's. Two measured properties pull it down as it gets
  louder: UNSW's own second peak is 104 cents below three times the first
  (A4's is 48 below), so its third harmonic falls on the flank of that
  peak; and the register tube's jet resistance at the literature's
  strength reaches about 0.7 bore impedances at mf (0.61 at pressure 0.3,
  0.79 at 0.6, with 2.9–3.9 kPa of breath, read with the `register`
  probe), a quarter of the tube's reactance. On the previous reed, without
  the jet the pitch still fell 36 cents from pressure 0.3 to 0.6; the
  side-key
  Bb4, with the same first peak and a second peak where the third harmonic
  lands, holds within 6. A refit with the real tube (1.47 mm, 13 mm,
  projecting about 2 mm) matches the reference as well and plays the
  same. No geometry or fingering error was found, and players report the
  same tendency: the clarinet goes flat when played loud and sharp when
  soft, players compensate with lip pressure (Schatt, Woodwind Doubling
  Resource Project, "Intonation"), and the throat tones move most (they
  fall twice as far as the rest when the barrel is pulled, R Clarinet
  Products, "Tuning the clarinet for performance"). No source gives cents
  against dynamic; the lip corrects it here as a player does.
- **How the committed bore was reached.** Fits to picked peaks are not
  smooth in the geometry (a peak changes identity as it moves), so the
  tool also fits bands of the measured impedance (`--stage bands`), by
  least squares with a forward-difference Jacobian (scipy, its columns in
  parallel), from four starts: the cylindrical bore with a neutral
  profile, the round-3 profiled bore and its peak-height refit, and the
  measured YCL-250 (Moers & Kergomard 2011, appendix A: its hole radii and
  chimneys, positions mapped linearly onto the register and E/B holes).
  Every candidate was played before any slur grid: tuned at pressures
  0.25–0.6 for jumps over 15 cents, wandering within a note, and the
  twelfth below (`stability`). Learned on the previous reed:
  - The profiled bores' narrow mouthpiece chamber (6.3–6.75 mm) was what
    made the soft upper clarion start in the low register (F#5–G#5 at
    pressure 0.3–0.35, twelfth below −23 to −33 dB) and E6 and F6 jump
    50–70 cents. Reset to the bore's radius (the reed and tract were set
    on that chamber), the same holes play those notes cleanly. The band
    fit holds it (`BAND_MOUTHPIECE`). On the lip-loaded reed, whose units
    are fixed, the round-3 profiled bore with its narrow chamber plays
    every clarion and altissimo note stably too, but its ascents take up
    to 32.5 ms and 8 of 48 dip 3 dB or more.
  - The band fit alone matched the bands better but played worse: it
    gives up the first peaks' tuning (4–5 cents mean, 15–20 at worst)
    and the clarion's first-to-second ratios for the many peaks near the
    cutoff it cannot reach, and left the forked Eb4 30–60 cents off its
    tuning curve. The fork's first peak stayed about 35% above the
    reference's on every geometry tried.
  - The committed bore is the YCL-250 start, fitted to the peaks, its
    chamber reset, band-fitted with every hole free, then fitted to the
    peaks with the altissimo's first-to-third ratio weighted 10, and once
    more with the flare held from narrowing (its second section had
    narrowed by 1 mm). Holes moved 1–4 mm from the YCL-250's mapped
    positions above LH1 and 6–26 mm below, radii up to 2.8 mm. With the
    parallel shunt it was band-fitted once more from there (15
    evaluations, the holes from the register to RH3 free, `fit --stage
    bands`): the holes from the sliver to RH2 moved 1–5 mm, LH2's radius
    fell from 5.9 to 4.7 mm and their chimneys lengthened, most from
    their 4 mm bound to 5–8 mm. Peak fits from it moved E3 and B4 30 cents
    and were not adopted.
  - Round 8 fitted the peaks locally from there (`fit --stage local
    --third 0.3`: the thumb hole and the holes from the sliver to RH3
    free, a prior holding them near the start, the third peaks at 0.3 and
    the forked Eb4's first two peaks five times more): LH2 widened from
    4.69 to 4.85 mm and moved 2.2 mm down with the Eb key and LH3, RH1
    narrowed by 0.2 mm, RH2 widened by 0.2 mm. The fork's second peak came
    from 22 cents under the reference to 8, its first stayed at +8; the
    first peaks' mean error rose from 4.3 to 4.8 cents, the second's from
    2.0 to 2.3, and the throat's third peaks from 47–55 to 69–77 cents.
    Held to the third peaks at 1.0 (or 0.4–0.6) instead, the throat's stay
    at 41–58 cents but the first and second peaks fall to 5.3–7.0 and
    3.2–5.6 cents (max 14–22), so the lighter hold was adopted.
  - Round 10 replaced the bell (`horn` in the geometry) and fitted it
    locally from round 9's geometry without the coupling (`fit --stage
    local --third 0.3`, the horn's start, x0 and mouth free with the
    holes from the thumb's to the E/B key, 12 + 15 evaluations): the
    horn's origin moved 21 mm up, x0 from Nederveen's 85 mm to 74.5 and
    the mouth from 30 to 29.5 mm; the lower holes moved 1–4 mm. With the
    first peaks it brought F4's and C6's third back (12 and 7 cents) and
    the altissimo's ratios within 5%. The fit cut the horn from the last
    hole; cut from 8 mm below it instead (a step at the hole costs 3–4
    Mc/s), the same horn moves the first peaks' mean by 0.02 cents. A horn
    whose catenoid itself starts below the last hole fits E3 only with x0
    near 55 mm, and loses 0.5 cents of the first peaks' mean. A local fit aiming the open lattice's
    band edge at UNSW's (`LOCAL_EDGE` on F4–A#4 and C6) moved it 10–20
    cents down at the cost of the forked Eb4 (+6/−12) and the second
    peaks (2.6 to 3.7 cents mean), so it was not adopted; a fit with the
    visco-thermal wall loss of a cylinder (`3e-5 sqrt(f) / r`: the flat
    loss and shelf at 0.055, 0.16 and 900 Hz instead of 0.072, 0.062 and
    470 Hz) reached only 10 cents on the first peaks and 30% on the
    altissimo's ratios in 20 evaluations.
  - The forked Bb5 was a multiphonic on the previous junction and
    geometry: its first mode (316 Hz) sounded with the note (831 Hz) 3–8
    dB under it in the mouthpiece, with their sum and difference tones,
    because its third impedance peak (1161 Hz) sat on their sum (1147
    Hz). The reed's resonance and damping did not change it; a lip of
    1.1 N did. On the refit geometry the fork's third peak is at 1108 Hz,
    and the first mode stays under −62 dB at the clarion's lip; its first
    peak now stands 1.43 times its second (the standard Bb5's 1.26).
  - Aiming the fit at A5–C6 first-to-second ratios 15–35% under the
    reference's moved them 3–10% and widened the upper clarion's window
    (below) by at most 0.3 kPa; not adopted.
- **Open G's tone depends on the little finger.** Its third partial is
  9 dB weaker with the little finger up than down in the radiated sound,
  though the same in the mouthpiece: at 1 kHz the bell's flow, stronger
  with the little-finger keys open, is in antiphase with the upper holes',
  and the openings are summed as one source, as a listener broadside
  hears them. Landing from B4, whose little finger was down, the radiated
  third partial passed from the one level to the other as the keys opened,
  10–14 dB above its steady level for about 10 ms. The little finger now
  stays down there, as open G leaves it free. Giving the openings their
  spacing toward a listener 30–60 degrees off the axis moved those bursts
  only to about 10 dB and changed the ascents' dips by up to 3 dB, so the
  radiation stays a single source.
- **The upper clarion's window.** UNSW's own first peaks there stand
  1.3–1.5 times the second, and on round 7 the bore did not widen the
  window: every geometry measured, with first-to-second ratios from 1.30
  to 1.45 at C6, topped out within 0.3 kPa of the others (A#5 at 4.5 kPa,
  B5 and C6 at 4.8 with the player's stronger tract, above). The round-8
  geometry (LH2 wider, RH1 narrower) takes B5 and C6 to 6.7 kPa and A#5
  to 5.0 on round 7's jet, and with the jet as a pressure every note from
  G5 to C6 speaks to 6.7 kPa, the end of the sweep. Without the jet they
  fall to the first peak at 2.9–4.3 kPa: the jet holds the register (a
  stronger jet, C = 1.2, did so on round 6's junction). The tract's strengthening is
  kept: it was measured on players (Chen, Smith & Wolfe 2009).
- **No forked Bb3/F5.** Boehm charts list none; the only fork the lattice
  can finger is 40 cents flat, so it is left out.
- **What is still estimated.** Measured: the reed's extinction line and
  tip opening, the chalumeau's and clarion's lip forces and the crescendo
  slope (Almeida et al. 2013), the breath's range and its fall with pitch
  (Fuks & Sundberg 1999), and the steady tract (Fritz & Wolfe 2005; 1.3 kHz
  and 1.5 Zc within their 1.0–1.4 kHz and 1.3–4 Zc). Estimated and set by
  playing the slur grid and the range: the reed's effective channel width
  (11 mm), its resonance and damping against the lip (the damping's
  growth with the contact area especially: under q = 0.35 the clarion
  squeaks near 1.1 kHz below 1 N and a sloppy D5–G4 lands on open G's
  twelfth), the altissimo's force, breath and tract tuning, the upper
  clarion's tract strength against the breath (within Fritz & Wolfe's
  range), and the breath's exact ends per register. The model starts notes about 1 kPa
  below Almeida et al.'s machine (theory does too; Dalmont & Frappé's
  machine fell between them), so its soft end is softer than theirs.
- **Third impedance peaks** are 25 cents off on average. F#4's and G4's
  are 149 and 222 cents sharp and the throat's (G#4–A#4) 69–78; F4's and
  C6's are 12 and 7 (round 9: 125 and 129). UNSW's are the shoulders of a
  broad rise, about 2–5 Zc and 170–570 Hz wide from 1.3 to 1.45 kHz (to
  1.55 kHz once the throat A key opens), in every reference fingering with
  the right hand's holes open and in none with them shut, where the
  lattice's impedance stays at 0.1–0.5 Zc up to a narrow peak 50–200 cents
  higher. Round 8 found that more hole, wall, mouthpiece or bell loss, the
  register tube's own resonance and cross-fingering do not raise it;
  round 9 that the external interaction does not; round 10 replaced the
  bell, as round 9 proposed, and found two other causes:
  - *The reference plane.* UNSW's impedance rises with frequency in every
    fingering: its geometric mean over 1.3–2.6 kHz is 2.2–2.7 Zc and over
    3.2–3.7 kHz 3.5–4.5, where the model's stays near 1. Its peaks stand
    higher at high frequency because its minima do: from max/min ratios,
    which do not depend on the scale, the round trip of UNSW's E3 keeps
    less than the model's at high frequency (0.37 against 0.50 of the wave
    at 2.1 kHz), but its minima rise in proportion to frequency, 0.08 Zc at
    440 Hz to 0.94 at 1.64 kHz, as a series inertance of about 30 mm of
    bore would raise them, and its level climbs as a reference plane
    narrower than the bore, reached through a taper, would make it. UNSW
    measured through a head 7.8 mm across, standing for the reed's area
    (Dickens, France, Smith & Wolfe 2007, Acoustics Australia 35:17), and
    a mouthpiece narrows toward its tip; a 3.9 mm radius has 3.5 times the
    bore's characteristic impedance. Seen through a short neck of that
    radius (9 mm), the model's minima rise as UNSW's do and its rms log
    |Z| above 1.3 kHz falls from 1.10 to 0.51 (the mean from 1.0 to 1.5
    Zc against UNSW's 2.5), but E3's high peaks hardly rise (0.65 and 0.49
    of UNSW's fourth and fifth to 0.67 and 0.54) and the third peaks move
    by 5–15 cents. So E3's fourth to sixth peaks, a quarter to three fifths
    of UNSW's on round 9 and half to two thirds now, measure the input's
    taper as much as the termination; the played lattice keeps its reed
    plane at the fit's, since a narrower chamber changes what the reed
    sees (one started the soft upper clarion in the low register, above).
  - *The band edge.* With the physical bell, F4's edge resonance stands at
    1514 Hz against UNSW's 1479 (F#4 1533 against 1500, G4 1597–1613
    against 1551, the throat 1604–1626 against 1554–1557): the open
    lattice's cutoff is 40–80 cents high where it carries the throat and
    F#4–G4, and the weak peak UNSW shows at the rise's foot (F4 1413, G4
    1418) has no counterpart once the bell no longer loses 8% everywhere
    (round 9's F4 doublet without the lowpass loss was that). Aiming the
    edge at UNSW's moves it 10–20 cents and breaks the forked Eb4 (above):
    the effective lattice trades it against the first two peaks, which
    the reed plays.
  - *Exactly.* The frequency model with every opening (20 holes and the
    bell) radiating into every other, with the `e^(-j k d)` delays (the
    full transfer-matrix interaction of Lefebvre et al.), lowers the first
    peaks 2.5 cents on average (the throat notes' 7–11; Lefebvre et al.
    found 5–10 on a clarinet with many holes open) and the second 3.1,
    flattens the peaks above the cutoff toward UNSW's (F4's at 1.51 kHz
    from 8.7 to 6.3 Zc against UNSW's 4.4–6.6, G4's at 1.61 kHz from 4.1
    to 2.1 against 2.3–2.7) and brings the rms log |Z| over 1.3–2.1 kHz
    from 1.134 to 1.079. Below 1.45 kHz it changes nothing: F4 at 1.40 kHz
    stays at 1.3–1.4 Zc against UNSW's 4.7, even with the coupling ten
    times stronger. The interaction is not the rise.
  - *Economically.* Nearest neighbours only, with their delays, keep a
    third to a half of that (the right hand's six openings 1.117, with the
    bell 1.110; every opening 1.100, every opening to two neighbours
    1.082); without the delays (a mutual inertance alone) the shared
    radiation resistance is lost and a quarter is kept. That resistance's
    low-frequency limit, the second derivative of the openings' summed
    flow, grows without bound where the bell's own radiation already is
    and rings the rendered lattice unstable through the bell; it is left
    out. RH1–RH3, which round 9 committed, give 1.123 (round 10 removed
    them, above).
  - *What round 9 took the rise for.* Moers & Kergomard (2011) read a
    clarinet's global cutoff from measured impedance by this slope
    inversion (D4: 1450 Hz on a YCL-250), and round 9 put its height down
    to the fitted bell's termination. The physical bell answers that part
    (above); the fitted lattice's local cutoffs (Moers & Kergomard's
    cells) still scatter from 1.1 to over 2.5 kHz where a maker's are
    nearly even.
  - *Fitting it.* A band fit counting the weak peaks fully (`fit --stage
    bands --set WEAK_PEAK=0.05`, every hole free, 12 evaluations) brings
    the throat's third peaks to 30–38 cents and F4's to 9, with the first
    peaks at 5.0 cents mean and the bands 58% off (63.5%), but the forked
    Eb4's second peak falls 36 cents under UNSW's (its fork is then
    useless), and the slur grid on it (with the committed tuning table)
    shows third-mode bursts up to 16 dB. Local peak fits that hold the fork (`fit --stage local`,
    `LOCAL_THIRD` on the throat, from it or from the committed geometry)
    reach 41–62 cents on the throat only with the fork at +8 to +15/−7 to
    −15 and the first peaks at 5.6–8 cents mean. The trade is the
    effective lattice's, not the interaction's; the throat's, F#4's and
    G4's third peaks are left, documented.

`reed_dynamics.muz` also provides the earlier bandpass correction to a
quasi-static reed (`single`) and an optional upstream resonance
(`voicing`). Both reeds accept the vocal tract. `reed-voicing.muz`
demonstrates them.

## Passive measurements

The passive examples inject a broadband pulse at the reed plane with the
exciter removed. Measure reflection, normalized input impedance, and
radiation with the same tool:

```sh
uv run tools/woodwind-passive.py examples/woodwind-prototypes/passive-cylinder.muz --radiated
uv run tools/woodwind-passive.py examples/woodwind-prototypes/passive-clarinet.muz --track g4 --min-hz 120 --radiated
```

The cylinder is a reference case with a 2 ms round trip and a first
impedance peak near 250 Hz. `passive-clarinet.muz` holds 15 fingerings
of the lattice clarinet, one track per written note. It is small-signal,
with no jet resistance, as the reference was measured. These results
describe held fingerings at the requested sample rate.
