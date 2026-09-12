# Composer friction

This is the canonical log of live, unresolved engine and language issues that we
can fix in muz. Record defects and limitations in language semantics, synthesis,
rendering, diagnostics and exposed primitives, including cases that distort
musical choices or force repetitive editing. A valid workaround does not close a
report.

External model quality, provider availability, skill tooling and development
process reports do not belong here.

Commit a report in the same commit as the piece or revision that exposed it. For
work without a piece, name the task and commit the report with that work. Each
entry names its origin, observed behavior, affected decision/work, workaround,
and desired behavior. Include a small reproduction when useful; do not freeze a
whole composition as a test.

Remove an entry in the same commit that resolves the engine or language issue,
after verifying the reported use case. If only part is fixed, keep the remaining
problem explicit. Put usage in reference docs and
regressions in focused tests. Git history retains reports and their fixes; this
file has no resolved section, remedy archive, or parallel per-piece log. A piece
commit that encounters no new friction needs no ceremonial report. Fixes found
and completed within the same commit need no artificial live entry.

## Choosing a resolution

For every friction item, seek a general solution that expands the builtins/kernel
as little as necessary while fully resolving the reported problem. Start by
asking whether existing language operations and exposed data can express the
solution clearly. Put reusable policies and recipes in `std/`; keep particular
musical choices in project source and demonstrate useful techniques in examples.

When source cannot express the solution well, identify the missing general
capability. Prefer a small, composable primitive or better access to musical data
that enables a family of source-level solutions over a builtin for the reported
special case. Keep policy choices such as shapes, selection rules and overlap
behavior in source wherever practical. Kernel changes remain appropriate for
engine defects, runtime guarantees, or capabilities that genuinely require them;
minimizing the kernel must not mean retaining awkward workarounds or merely moving
complexity into every composition.

In the fixing commit, explain why existing facilities suffice or why the added
primitive belongs in the kernel, and demonstrate the specific resolution in
source. Tag-derived automation is one example of this general rule: expose note
data and timing operations, then let composers write the automation recipes.

## Open reports

### Sample-zone selection cannot feed programmable per-voice processing

- **Origin:** sound/patch/instrument-design audit, 2026-09-11; confirmed against
  `SampleZone`, `Sampler`, graph sample preparation, and compiler expression checks.
- **Observed:** standalone samplers provide key/velocity zones, recording choices,
  stereo playback and bounded loops, but their voices have only the built-in
  amplitude/pan/tuning path. Brightness and pressure expression are rejected.
  Graph sample nodes permit per-voice filtering and modulation but accept only a
  single path/root and whole-file loop flag, without the zone-selection facilities.
- **Affected decision/work:** a multisampled instrument cannot combine its existing
  recording map with an independent filter/envelope per sounding note. Overlapping
  sampler zones select one recording rather than simultaneous dynamic layers.
- **Workaround:** filter the summed track, split notes/layers into separate tracks,
  construct single-recording graph patches, or use a plugin. Layer crossfades can
  be authored across tracks, but mapping and processing must then be coordinated.
- **Desired behavior:** compose prepared sample selection/playback with programmable
  voice processing and independently controlled layers. Reuse zone data and note
  identity; keep articulation selection and crossfade shapes in source where
  possible rather than introducing an instrument-specific synthesis mode.

### Project-file structure errors cannot name a line

- **Origin:** diagnostics pass over `muz` error reporting.
- **Observed:** structural validation of a `project.json5` session (unknown
  device parameter, duplicate global id, track instrument kind, undeclared bus,
  schema version) reports the project file but no line or field, because
  `RawSession` deserializes through `json5::from_str` into plain values and the
  spans are gone before any check runs. The JSON5 *syntax* errors do name
  `path:line:column`.
- **Affected decision/work:** a project large enough that the offending id does
  not sit on the first screen forces a manual search for the name in the file
  before the check can be satisfied.
- **Workaround:** search the project text for the id or parameter named in the
  message.
- **Desired behavior:** name the location of the failing value — at minimum the
  JSON5 key path (`tracks[2].instrument.parameters.cutof_hz`), ideally the line
  and column. Resolving this needs a span-preserving project read (a custom
  deserializer that records key offsets) rather than another message rewrite.

### Kit-track insert automation cannot name the logical track

- **Origin:** Ghost Light (`../muz-projects/ghost-light/`), a native drum & bass piece.
- **Observed:** a `lowpass` insert declared in a `kit()` track's `chain` cannot
  be automated through the logical track id. `automation("drums.tone.cutoff_hz",
  ...)` fails: the kit expands the logical `drums` track into physical
  `drums.kick`, `drums.snare`, … tracks, each carrying a copy of the track chain,
  so `drums.tone` never exists as a target. The failure now names the rejected
  target at its source line and lists the ids the expanded graph accepts
  (`targets under 'drums': drums.kick.tone.cutoff_hz, …`), but the lane still has
  to be written once per expanded voice. Naming one expanded voice
  (`drums.kick.tone.cutoff_hz`) checks, but sweeping a whole kit then needs one
  lane per voice.
- **Affected decision/work:** the intended arrangement used a single drum-track
  lowpass for a filtered intro and a closed breakdown. The workaround moved the
  filter to the shared `rhythm` bus and automated `rhythm.tone.cutoff_hz`. That
  also filters the percussion, and because kit-voice sends tap before the bus,
  the drum kit's own `room`/`crush` sends stay unfiltered through the intro.
- **Workaround:** filter a bus that receives the kit, or emit one automation lane
  per expanded voice.
- **Desired behavior:** accept the logical track device id and apply the lane to
  every expanded voice (the natural reading of a track-level insert). Broadcasting
  a logical insert lane is the smaller general expansion; a clearer error only
  narrows the search.
- **Reproduction:**
  ```muz
  song({
      tempo: 120,
      tracks: [
          track("drums", drums({kick: "X...", hat: "x.x."}), kit("default"), {
              chain: [fx("lowpass", {id: "tone", cutoff_hz: 16000})]
          })
      ],
      automation: [
          automation("drums.tone.cutoff_hz", curve([[0b, 500], [4b, 16000]]))
      ],
      tail: 1
  })
  ```
  The bus form (`rhythm.tone.cutoff_hz` on a `bus("rhythm", ...)`) passes.

