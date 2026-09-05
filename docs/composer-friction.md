# Composer friction

This is the canonical log of live, unresolved composer-facing friction across muz.
Read it before composition and engine work. Record concrete problems as they are
encountered: distorted musical choices, repetitive editing, unclear semantics or
diagnostics, missing abstractions, and workflow failures. A valid workaround does
not close a report.

Commit a report in the same commit as the piece or revision that exposed it. For
work without a piece, name the task and commit the report with that work. Each
entry names its origin, observed behavior, affected decision/work, workaround,
and desired behavior. Include a small reproduction when useful; do not freeze a
whole composition as a test.

Remove an entry in the same commit that fixes the language, engine, documentation,
or workflow responsible, after verifying the reported use case. If only part is
fixed, keep the remaining problem explicit. Put usage in reference docs and
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

- **Kernel-boundary audit — scalar arithmetic blocks source recipes:** The small
  expression `0.5-0.5*cos(6.28318*33/64)` fails with `exact number overflow`.
  A 129-point source-written cosine envelope consequently fails, while native
  `lfo` evaluates its intermediate arithmetic in floating point. This prevents
  straightforward movement of envelope policy into source. Workaround: use the
  native recipe or alter/quantize the arithmetic. Desired: a general numerical
  policy that lets ordinary finite control calculations compose reliably while
  retaining exact musical time, rather than adding more native envelope shapes.
  Evidence: `src/lang/eval.rs` checked rational arithmetic and
  `src/lang/builtins.rs` numeric conversion/LFO implementation; reproduced with
  `muz eval` during this audit.

- **Kernel-boundary audit — numeric helpers discard units:**
  `max(1b,2b)+1b` and `abs(-10ms)+1ms` fail because these helpers return scalars;
  `min(1b,1s)` silently succeeds despite incompatible dimensions. These are
  obstacles to ordinary source-written timing and envelope functions. Workaround:
  write conditionals that return an original quantity. Desired: compatible-unit
  preservation and dimensional validation in general numeric helpers. Evidence:
  `src/lang/builtins.rs` numeric helper branches; all three cases reproduced with
  `muz eval` during this audit.

- **Kernel-boundary audit — musical data lacks a general transformation path:**
  `.notes` exposes note records, but `.refine("all",fn(n)=>{...})` rejects a
  callback, refinements cannot set duration or release offset, and pattern control
  events have no comparable source view. This makes custom articulation, ornaments
  and coherent note/pedal time warps depend on special Rust implementations.
  Workaround: reconstruct notes with metadata bookkeeping, or repeatedly refine
  individual identities. A source `fold` of per-identity refinements matched
  `scale_gate` including controls in a small probe, but rescans/clones the pattern
  for every note. Desired: a bounded, general transformation interface preserving
  untouched identities, annotations and controls, with suitable primitives for
  coherent event timing and deterministic variation. Then move musical policies
  such as dynamics, groove/humanize weights, rubato shape, flams and rolls into
  source where practical. This includes reconsidering the recent `scale_gate`
  builtin. Evidence: `src/lang/eval.rs` pattern field access and
  `src/lang/builtins.rs` note construction, refinement and performance branches;
  callback/duration/release-offset/control-access limitations reproduced in probes.

- **Kernel-boundary audit — drum grid vocabulary is closed:**
  `drums({brush:"x...x..."})` fails with `unknown kit voice 'brush'`, although
  manually authored notes can carry custom voices into the kit mapping. MIDI
  pitches and strike-symbol velocities are also fixed inside the grid parser.
  Workaround: construct custom-voice notes outside the compact grid syntax.
  Desired: general grid decoding with source-supplied voice/articulation data,
  keeping familiar mappings in stdlib. Parsing, event budgets and actual sample
  choke processing can remain engine responsibilities. Evidence:
  `src/lang/builtins.rs` `drums` and `src/compile.rs` kit expansion; the custom-lane
  rejection was reproduced with `muz eval`.

- **Kernel-boundary audit — recipes and catalogs still require kernel edits:**
  Euclidean rhythm construction, scale-mode tables, degree/chord recipes, named
  synth presets and the default Pianoteq path/class are implemented in Rust.
  The public helpers' named vocabulary therefore grows through kernel edits even
  where ordinary arrays, records and functions suffice. Workaround: bypass those
  catalogs with local source functions and explicit instrument settings. Audit
  probes successfully expressed Euclidean notes/span, a custom scale consumed by
  the tonal helpers, and a checked synth preset in source without new primitives.
  Desired: move these reusable recipes/default catalogs to stdlib or appropriate
  user configuration, retaining generic constructors, parsing, validation and
  device hosting. Move `lfo`/curve-placement recipes once the numerical issue above
  is resolved; do not hide it behind another native recipe. Evidence:
  `src/lang/builtins.rs`, `src/tonal.rs::scale`, and
  `src/compile.rs::device`/`preset`. These are extension/ownership findings, not
  reports that the existing pieces fail to compile.
