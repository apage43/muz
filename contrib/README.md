# contrib/

Ready-to-use instrument setups for third-party sample libraries and plugins.
Each pack owns a native mapping in source, an installer that fetches and
verifies the recordings it needs, and notes on upstream, version and license.

A pack never ships third-party content. Recordings land in the gitignored
`<pack>/assets/` directory, which is where the pack's own mapping resolves its
relative `assets/...` paths.

```sh
python3 contrib/virtuosity-drums/install.py     # install or verify one pack
```

```muz
use "contrib/virtuosity-drums/kit" as vd;
```

`use "contrib/<pack>/<module>"` resolves in this directory, or in
`$MUZ_CONTRIB_DIR` when it is set. Installation is idempotent: run the installer
again at any time to verify the content already on disk.

## Packs

| Pack | Upstream | License | Size | Content |
| --- | --- | --- | --- | --- |
| `virtuosity-drums` | Virtuosity Drums v0.925 (Versilian Studios with Karoryfer, performed by Austin McMahon) | CC0 | ~1.2 GB | Drum recordings and the SFZ programs they ship with |
| `vsco-2-ce` | VSCO 2 Community Edition (Sam Gossner, Simon Dalzell) | CC0 | ~460 MB | Strings, winds, brass, harp, timpani, cymbal and spiccato short bows |
| `karoryfer` | Karoryfer Black And Blue Basses and Shinyguitar | CC0 | ~280 MB | Articulated electric guitar and bass plucks |
| `unreal/standard-guitar` | Unreal Instruments Standard Guitar | Publisher terms: commercial use, no credit, no redistribution | ~720 MB | Chromatic sustain and mute takes |
| `unreal/metal-gtx` | Unreal Instruments METAL-GTX | Publisher terms: free use, no credit, no redistribution | ~1.6 GB | Recorded high-gain DI takes plus the derived mono set |
| `plugins` | OrbitCab, CHOWTapeModel, Surge XT, Pianoteq 9 | Plugin-specific | — | No content: alias conventions, device recipes and JUCE state helpers |

Packs whose recordings may not be redistributed ship only the installer, the
mapping and the license summary; download and install stays with the composer.

## Conventions

- A pack's functions evaluate in the pack's own directory, so a pack builds its
  devices itself (`hit`, `instrument`, `voice`, `kit`) and callers pass plain
  musical options. A caller that called `sample(zones, ...)` directly would
  resolve the pack's paths against the caller's directory instead.
- A device carrying a `state:` or `path:` value stays at the call site, because
  the value is relative to the module that declares it.
- Installers are Python with at most a documented external tool (for example
  `unrar` or `ffmpeg`); they verify size and SHA-256 against a pinned manifest
  and never overwrite a differing file silently.
- Content stays out of git; the engine's formatter keeps pack sources stable.
