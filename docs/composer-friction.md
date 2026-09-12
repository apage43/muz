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

### Voice lifetime follows modulation envelopes rather than sounding tails

- **Origin:** sound/patch/instrument-design audit, 2026-09-11; confirmed by
  inspection of `VoicePatch::frame` and the documented retirement contract.
- **Observed:** all graph ADSRs contribute to retirement regardless of their role
  in the signal flow. An all-one-shot patch retires when these envelopes fall
  below the threshold, even if a downstream delay still contains audio. A
  modulation envelope can also keep an otherwise finished voice alive.
- **Affected decision/work:** a short exciter feeding a longer ringing body cannot
  define its excitation independently of the body's lifetime. Adding resonators
  without addressing this would retain the same limitation.
- **Workaround:** add an envelope solely to extend lifetime, or put tail effects
  on the track. Track processing shares state across notes and cannot supply an
  independent, pitch-tracked body for each voice.
- **Desired behavior:** an explicit, bounded voice-completion contract independent
  of incidental modulation ADSRs, preserving legacy behavior by default. An
  exciter may finish before its downstream body; resource reclamation must still
  be guaranteed. A small fixing test can use a one-shot ADSR with zero attack,
  `decay:0.01`, zero sustain feeding a delay with `seconds:0.03`: current envelope
  retirement precedes even the first delayed output.

### Programmable voice patches cannot retain internal stereo

- **Origin:** sound/patch/instrument-design audit and review of the prior
  sound-design proposal, 2026-09-11; confirmed by implementation inspection.
- **Observed:** a patch has one scalar output, panned only after graph evaluation.
  Graph sample readers average the recording's left and right channels. Native
  preset stereo unison and standalone stereo sample playback exist but cannot be
  retained inside an equivalent programmable graph.
- **Affected decision/work:** oscillator-level stereo placement, independent
  left/right voice processing, and stereo sample/synth hybrids require moving
  processing outside the patch or splitting instruments across tracks.
- **Workaround:** use stereo preset synths, track widening effects, or duplicated
  tracks. None exposes two independently constructed outputs of one patch voice.
- **Desired behavior:** accept paired scalar outputs and preserve access to sample
  channels with shared prepared asset storage. Stereo combinators and unison
  policies can remain source functions; scalar DSP nodes need not all change type.

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

### Native note attacks always create a fresh voice

- **Origin:** sound/patch/instrument-design audit, 2026-09-11; confirmed by inspecting
  note-on handling in the preset synth and voice-patch processors.
- **Observed:** each note-on allocates or steals a voice and initializes its state.
  There is no native mono/legato contract for transferring pitch to an existing
  voice while retaining oscillator/envelope state. Per-note tuning curves already
  bend a held voice, but separate score notes still produce separate attacks.
- **Affected decision/work:** mono bass/lead phrases cannot choose legato envelope
  retrigger behavior and glide through ordinary overlapping note events.
- **Workaround:** rewrite a phrase as a sustained note with a tuning curve and
  explicit expression, or use a plugin. The sustained-note recipe changes score
  note identity and makes articulation bookkeeping the composer's responsibility.
- **Desired behavior:** a reusable source performance recipe over sufficient voice
  continuation/retrigger semantics. First assess existing event operations; any
  new runtime contract should preserve note-off ownership and bounded allocation,
  while musical priority and glide policies remain in source where practical.

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

