# Embedding muz

The default `desktop` Cargo feature includes the CLI, PipeWire output, native
VST3/CLAP hosting, filesystem watching, control socket, and render jobs. Existing
desktop builds retain these facilities. A library consumer can disable defaults
to build the language, compiler, musical model, inspection, native synthesis,
effects, and audio engine without those platform dependencies:

```sh
cargo check --lib --no-default-features --target wasm32-unknown-unknown
```

The portable build rejects native plugin instruments explicitly. It does not
provide browser audio output itself: the embedding host owns scheduling and
output buffers. `AudioEngine::render_interleaved` produces PCM at the host's
configured sample rate, in blocks no larger than its configured capacity.

## Unsaved source and module loading

`lang::SourceLoader` supplies module identity resolution, source text, and the
contrib library root. `lang::load_with_loader` evaluates through that loader;
`compile::compile_with_loader` also lowers and checks the resulting session.
Pass an `Rc<dyn SourceLoader>`. A fresh compilation uses a fresh evaluator/cache,
so unsaved revisions are isolated. Imports, dependencies, and cycle detection
use the same host-provided identities. `FileSourceLoader` preserves ordinary
desktop behavior. Standard modules remain embedded; `lang::STANDARD_MODULES`
exposes their import names and source text for hosts that want to display them.

The host's `resolve` implementation must normalize equivalent paths consistently
and enforce its own filesystem/mount boundaries. Relative imports are passed
relative to the declaring module. `contrib_modules` optionally provides names
for missing-module suggestions.

This interface loads source text only. MIDI files, audio samples, plugin state,
and asset metadata still use the native filesystem APIs. A browser host must
advertise its supported asset capabilities; source-loader support does not imply
sample or MIDI-asset support.

## Editor inspection

`Compiled::locations` maps keys such as `track.lead`, `device.lead.instrument`,
and `route.lead.out` to the declaration locations already retained by lowering.
This is declaration attribution, not a complete expansion history of every
generated note. Locations serialize their path, one-based line and Unicode
character column, and source excerpt. Browser editors using UTF-16 positions
must convert character columns to their document offsets.

`lang::Diagnostic::to_json` exposes the same primary-location selection, help,
and caller locations as terminal diagnostics. Avoid parsing the terminal text
to recover locations.

Ordinary `Session` serialization deliberately omits the imported/performed event
arrays in `MidiTrackSource::imported` to keep control responses bounded. A host
transferring a playable session between isolated runtimes must transfer those
arrays explicitly and restore them before preparation. Inspection views provide
performed notes, controllers, tempo maps, automation, sections, and native patches.

Keep host UI and browser bridges in the consuming project. These interfaces are
general embedding facilities; they add no musical policies or language builtins.
