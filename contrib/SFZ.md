# Complete SFZ assets and public catalogs

`sfz-catalog.json` lists public programs, separately from include fragments. Catalog
paths are relative to each pack's `assets/`. Sonatina exposes 557 roots, Virtuosity
eight, Karoryfer one Shinyguitar root and eleven Black And Blue roots, Standard
Guitar six, and METAL-GTX five. This is original program/control coverage beyond
existing curated native presets. Existing `.muz` modules and their calibration
remain available. VSCO's pinned source has no SFZ mappings; its native presets
remain the supported interface.

Each pack's `sfz.muz` exports a named function for every catalog root, with
punctuation converted to underscores and numeric IDs prefixed with `program_`.
For example, Karoryfer exports `shinyguitar()` and `program_01_darkblack_keysw()`.
Sonatina's functions retain catalog IDs converted to underscore names. Builders
accept an options record and stamp the pack directory as their asset root.
Named builders supply a per-program `sample_budget_frames` default measured from
the pinned dependency headers, rounded up with one Mi-frame margin. The generic
constructor retains the conservative engine default. Inspect the audit report's
`estimated_decoded_stereo_frames` / bytes before loading large originals; invoking
a full-kit builder opts into its larger footprint. Caller options may lower the
budget to reject the load before decoding.
Each decoded frame uses eight bytes even for originally mono recordings.
The engine validates its hard resource ceiling before sample decoding.
`program(path, options)` imports another original program using an explicit
pack-relative path; unlike named Shinyguitar, it requires explicit macro context.

Install original mappings and recordings with:

```sh
python3 contrib/install-sfz.py karoryfer --profile sfz-complete
python3 contrib/install-sfz.py unreal/standard-guitar --profile sfz-complete
python3 contrib/install-sfz.py unreal/metal-gtx --profile sfz-complete
python3 contrib/install-sfz.py sonatina --profile sfz-complete
python3 contrib/install-sfz.py virtuosity-drums --profile sfz-complete
```

Use `--check` for read-only verification, `--source DIR` to reuse pinned Git-backed
files, `--source ARCHIVE` for archive packs, or `--assets DIR` to choose a separate
installation. An existing differing file is rejected. Archives are verified by size
and SHA-256 before extraction; archive members and destinations must stay inside
staging/assets directories. Every installed asset is checked against SHA-256 or its
immutable Git blob identity before atomic placement. Archive installs require space
for staging as well as final recordings; Unreal extraction requires `unrar`.

`native-subset` delegates to the previous pack installer. Run those installers
directly for their individual flags. METAL-GTX complete SFZ installation uses the
original stereo recordings and releases/noises; native presets continue to use their
separately derived mono takes. No mapping or recording is redistributed in git.

The complete Karoryfer manifest includes all 846 Shinyguitar and 2,208 bass WAVs,
original SFZs and bank descriptors at the existing pinned revisions. Git blob hashes
verify upstream bytes, while the existing native manifest retains SHA-256 pins.
Shinyguitar's catalog supplies `sample_dir=../Samples` from the original
`Shinyguitar.bank.xml`; fragments lacking parent/macro context are not public roots.
Archive manifests describe mappings and recordings from the immutable publisher
archives, including the original stereo Unreal sample corpus. Standard Guitar's
archive was independently downloaded and SHA-256 verified before manifest creation.
Restricted Unreal thin builders reject `embed_assets: true` and retain linked assets.
The generic engine API can still embed recordings owned or appropriately licensed
by its caller. Restricted Unreal licenses permit use but do not grant redistribution; preserve the
publisher notices, and do not embed these assets in public bundles.

`audit-sfz.py` drives the engine importer audit command against each public catalog.
It records importer errors or expanded region/opcode-value/dependency inventories,
hashing every resolved dependency. The importer remains the source of truth for
includes, macros, inheritance and sample resolution; a lexical scan cannot establish
support. Build `cargo build --release --bin sfz-audit --no-default-features`, then supply `--importer target/release/sfz-audit`.
The audit executable consumes JSON-lines requests containing each root path and
catalog-defined macro bindings, and emits JSON lines
with `path`, `regions`, `dependencies`, and `opcodes` (distinct effective values by
spelling), diagnostics, or `error`. Reports contain only relative asset paths and content hashes.
For example:

```sh
python3 contrib/audit-sfz.py contrib/karoryfer \
  --importer target/release/sfz-audit --output /tmp/karoryfer-audit.json
```

`--prepare` additionally decodes all dependencies and prepares the runtime under
its resource limits; it reports preparation failures separately. This can require
substantial memory and time for full library roots. Successful preparation alone
does not establish audible parity. Successful dependency auditing alone does not establish runtime DSP compatibility;
use the engine's behavioral and reference-player acceptance tests as well.

Sonatina descriptors for Cymbals & Tamtam and Violin Solo 2 KS/Tremolo carry
source overlays pinned to original file SHA-256. They remove exactly the malformed
comment and `volume-1` / `volume-2` lines ignored by the verified sfizz reference. Original
installed bytes remain unchanged; a different source hash rejects the overlay.
The malformed volume text is not rewritten as a gain decision.

`--exercise` implies preparation and additionally checks native predicate, physical
controller, overlapping-note, sustain/release and deterministic finite-output smoke
fixtures. It reports exercise failures separately. These probes cover every public
root, not every region or all combinations; they do not certify reference-player
audio parity. Use a release audit binary for full-corpus decoding and timing.
