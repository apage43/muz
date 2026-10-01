# Composer friction

This is the canonical protocol and live log for unresolved engine and language
issues that can be fixed in muz. Read it before composing, revising a piece, or
changing the engine. Repository boundaries and the verification budget are in
[AGENTS.md](../AGENTS.md).

[Documentation index](README.md)

## What belongs here

Record defects and limitations in language semantics, synthesis, rendering,
diagnostics, and exposed primitives. Include issues that distort musical choices
or force repetitive editing, even when a workaround lets the piece proceed.
A valid workaround does not close a report.

External model quality, provider availability, skill tooling, and development
process reports do not belong here.

## Recording a report

Commit a report in the same commit as the piece or revision that exposed it.
For work without a piece, name the task and commit the report with that work.
Each entry includes:

- **Origin:** the piece, revision, or task that exposed the issue.
- **Observed behavior:** what muz does and the relevant conditions.
- **Affected work:** the musical decision or work the issue obstructs.
- **Workaround:** how the work proceeded, if possible.
- **Desired behavior:** the capability or correction needed.

Include a small reproduction when useful. Do not freeze a whole composition as
a test. A piece commit with no new friction needs no report; a problem found and
fixed within the same commit needs no artificial live entry.

## Choosing a resolution

Seek a general solution that expands the builtins/kernel as little as necessary
while fully resolving the reported problem. First ask whether existing language
operations and exposed data can express the solution clearly. Put reusable
policies and recipes in `std/`, particular musical choices in project source,
and demonstrations of useful techniques in examples.

When source cannot express the solution well, identify the missing general
capability. Prefer a small, composable primitive or better access to musical data
that enables a family of solutions over a builtin for one special case. Keep
shapes, selection rules, and overlap policies in source wherever practical.

Kernel changes remain appropriate for engine defects, runtime guarantees, and
capabilities that require them. Minimizing the kernel must not retain awkward
workarounds or move complexity into every composition.

In the fixing commit, explain why existing facilities suffice or why the new
primitive belongs in the kernel. Demonstrate the specific resolution in source.
For example, tag-derived automation uses exposed note data and timing operations
so composers can write their own automation recipes. This principle applies to
all friction items.

## Closing a report

Verify the reported use case, document usage in the relevant reference, and add
focused regression tests. Remove the entry in the same commit that resolves the
engine or language issue. If only part is fixed, keep the remaining problem
explicit.

Git history retains reports and fixes. This file has no resolved section or
remedy archive; do not maintain parallel per-piece friction logs.

## Open reports

### Native SFZ timed group choke leaves excessive legato overlap

- **Origin:** The Quiet Between Suns, native SFZ migration; listener reported
  different cello note overlap in the first 15 seconds.
- **Observed behavior:** Sonatina 4.0 `Strings - Performance/Celli Legato.sfz`
  uses `group=1 off_by=1 off_mode=time off_time=1` for both first and legato
  layers. Native SFZ applies a linear outgoing-voice gain ramp in
  `src/audio/device/sfz.rs` (`Sfz::start` sets `choke_time`; the render loop
  subtracts `1 / (sample_rate * choke_time)` from `choke_gain`). Installed sfizz
  1.2.3 instead produces an approximately exponential timed fade. A unity-DC
  fixture, with the old key still down at the incoming attack, measured
  outgoing amplitude relative to its pre-choke level:

  | Time after choke | Native | sfizz 1.2.3 |
  | --- | ---: | ---: |
  | 100 ms | 0.89984 | 0.40654 |
  | 200 ms | 0.79971 | 0.16531 |
  | 500 ms | 0.49930 | 0.01111 |

  Minimal reproduction at 48 kHz: a mono constant-0.25 WAV (`dc.wav`),
  `<control> set_cc7=127 set_cc11=127`, then
  `<group> group=1 off_by=1 off_mode=time off_time=1 amp_veltrack=0 ampeg_release=1.5`,
  `<region> sample=dc.wav key=60 loop_mode=loop_continuous loop_start=0 loop_end=47999`,
  and `<region> sample=*silence key=62`.
  Note-on 60 at 0.125 s; note-on 62 at 0.625 s; note-off 60 at 0.675 s.
  Explicitly use `.gate(1)` when expressing those timings in muz.

  Paired 15-second dry renders of the actual opening used the original SFZ via
  native `sfz(...)` and the retained sfizz VST3 state. All 209 performed note/CC
  rows were identical. At the first E3→F#3 handoff (1.730769 s), integrated FFT
  power around the first four harmonics (±7 Hz) in the 100–400 ms post-handoff
  window gave an old/new pitch ratio of −2.80 dB native versus −17.95 dB sfizz:
  15.15 dB more outgoing-note prominence. Incoming F#3 band power was nearly
  equal (21039.87 versus 20978.53). This isolates a transition-tail defect,
  rather than a changed score, CC schedule, patch choice, or mix.
- **Affected work:** The intentionally overlapped `.gate(1.035)` cello melody
  becomes audibly more polyphonic instead of handing off like the original
  sfizz performance. Other long timed-choke programs may also be affected;
  they have not been assessed.
- **Workaround:** None applied. The piece remains native; shortening authored
  overlaps or editing the library would mask the engine discrepancy.
- **Desired behavior:** Preserve held-key/first-legato selection while matching
  the reference timed-choke envelope, including nonzero starting envelope levels
  and subsequent note-offs. Qualify the fade at multiple points, not merely
  whether the voice eventually becomes silent. Investigation only; no engine
  implementation was changed.
