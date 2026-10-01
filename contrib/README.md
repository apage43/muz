# Optional instrument packs

These packs provide source mappings and setup instructions for third-party sample
libraries and plugins. Choose a pack below, install its external content, and
import its module. Native muz presets need none of these packs.

[Documentation index](../docs/README.md) · [Instrument guide](../docs/instruments.md)

## Install and import

After `./install.sh`, packs live in `$XDG_DATA_HOME/muz/contrib`, defaulting to
`~/.local/share/muz/contrib`. The installer copies the checkout's pack files and
any downloaded assets there. Reinstalling refreshes files without deleting assets
already in the installed tree. No recordings are downloaded by `install.sh`.

For the installed CLI, download content into that installed copy. Run a pack's
installer by its installed path, for example (with the default data location):

```sh
python3 "$HOME/.local/share/muz/contrib/virtuosity-drums/install.py"
```

If you set an absolute `XDG_DATA_HOME`, substitute that directory for
`$HOME/.local/share` in these commands. Empty or relative `XDG_DATA_HOME` values
use the default. Pack README commands use checkout-relative paths; substitute
the installed pack path when using the installed copy. Checkout users can keep
running `python3 contrib/<pack>/install.py`; rerun `./install.sh` to copy newly
downloaded checkout assets into the installed tree.

Recordings go under the pack's `assets/` directory (ignored in the checkout).
Follow its README for requirements, download size, license, and any manual route. Re-running an installer
verifies the pinned content. Differing files are reported; packs with an explicit
replacement option document it individually.

In your project's source:

```muz
use "contrib/virtuosity-drums/kit" as vd;
```

The project can live wherever you choose. Imports select one library root in
this order:

1. `MUZ_CONTRIB_DIR`, when set. It must name an existing directory; an invalid
   override is an error, with no fallback.
2. The `contrib/` directory associated with a checkout-built executable.
3. The installed user data directory described above.

`./install.sh` needs no environment export or surviving checkout. Plain
`cargo install --path .` installs only the executable; use `./install.sh` for the
complete desktop install, or explicitly select a library:

```sh
export MUZ_CONTRIB_DIR="/absolute/path/to/your/contrib"
```

A selected library is not merged with the others. A missing module or asset must
be installed into that library. Keep the installed location stable: plugin state
may record absolute sample paths, and relocating those assets requires updating
that state.

Plugin recipes use separately installed binaries and aliases; their README
explains the setup and any available installer.

## Packs

| Pack | Upstream | License | Size | Content |
| --- | --- | --- | --- | --- |
| [virtuosity-drums](virtuosity-drums/README.md) | Virtuosity Drums v0.925 (Versilian Studios with Karoryfer, performed by Austin McMahon) | CC0 | ~1.2 GB | Drum recordings and the SFZ programs they ship with |
| [vsco-2-ce](vsco-2-ce/README.md) | VSCO 2 Community Edition (Sam Gossner, Simon Dalzell) | CC0 | ~460 MB | Strings, winds, brass, harp, timpani, cymbal and spiccato short bows |
| [sonatina](sonatina/README.md) | Sonatina Symphonic Orchestra 4.0 (Mattias Westlund, Peter Eastman and contributors) | CC Sampling Plus 1.0 | ~1.48 GB | All 557 upstream SFZ programs, including articulation and keyswitch variants; requires an SFZ plugin |
| [karoryfer](karoryfer/README.md) | Karoryfer Black And Blue Basses and Shinyguitar | CC0 | ~280 MB | Articulated electric guitar and bass plucks |
| [unreal/standard-guitar](unreal/standard-guitar/README.md) | Unreal Instruments Standard Guitar | Publisher terms: commercial use, no credit, no redistribution | ~720 MB | Chromatic sustain and mute takes |
| [unreal/metal-gtx](unreal/metal-gtx/README.md) | Unreal Instruments METAL-GTX | Publisher terms: free use, no credit, no redistribution | ~1.6 GB | Recorded high-gain DI takes plus the derived mono set |
| [plugins](plugins/README.md) | OrbitCab, CHOWTapeModel, Surge XT, Pianoteq 9 | Plugin-specific | — | No content: alias conventions, device recipes and JUCE state helpers |

Packs whose recordings may not be redistributed ship only the installer, the
mapping and the license summary; download and install stays with the composer.

## Asset paths and pack conventions

Use the instrument builder from the module that owns the zones: for example,
`guitar.acoustic(options)`. A function evaluates in its defining module, so that
builder resolves `assets/...` within the pack. Calling `sample(pack.zones, ...)`
in your own module would resolve those relative paths against your project.

For project-owned plugin state, keep the `plugin(...,{state:...})` call in the
project module. Plugin packs can return settings records that you pass at that
call site; see [shared plugin recipes](plugins/README.md).

Installers use Python and any external tools named in their README, such as
`unrar` or `ffmpeg`. Manifests pin sizes and SHA-256 hashes. External recordings,
plugin binaries, and generated media stay out of this engine repository.
Document chosen assets and versions with the project that uses them.

Original SFZ program catalogs and complete installation profiles are documented in
[SFZ.md](SFZ.md). VSCO retains its native mapping until a separate SFZ source is pinned.
