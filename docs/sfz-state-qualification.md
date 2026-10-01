# Linked SFZ state qualification

[contrib/sfz-state-qualification.json](../contrib/sfz-state-qualification.json)
records eight actual roots across the six SFZ families. Each probe loads and
normalizes its mapping, builds a linked `DeviceState`, encodes version 4, decodes
through an explicitly supplied resolver, and compares the restored `SfzConfig`
with the original. It hashes sample bytes for the dependency table but does not
decode PCM, prepare an audio processor, or render audio. No publisher contents,
encoded states or binaries are committed.

| Root | Regions | Normalized bytes | Encoded state bytes |
| --- | ---: | ---: | ---: |
| Sonatina Trumpet Sustain | 13 | 38,309 | 11,543 |
| Sonatina All Brass KS | 664 | 3,175,237 | 62,299 |
| Sonatina Organ Combinations | 1,590 | 6,242,719 | 69,910 |
| Virtuosity Full Kit | 8,902 | 29,677,434 | 1,186,438 |
| Standard Guitar KSOP | 12,427 | 138,537,458 | 689,215 |
| METAL-GTX Full | 14,235 | 129,172,503 | 754,037 |
| Shinyguitar main | 6,798 | 43,468,938 | 214,738 |
| Black and Blue Darkblack 01 | 8,448 | 112,481,180 | 405,066 |

Every recorded roundtrip preserved the normalized program, constructor defines,
source overlays, voice/sample budget, seed, and linked asset policy. Shinyguitar
requires `sample_dir="../Samples"`; the other recorded roots use no constructor
defines or source overlays. This representative host-state gate does not claim
that every upstream root or every opcode is audio-qualified.

The JSON identifies the exact executable SHA256. Provenance was captured before
the Darkblack run; the preceding seven successful rows used the same executable
and were captured retrospectively. That executable includes compact version 4
state and predates final resolver authorization hardening. Current authorization
is covered by permanent device-state, scoped asset, and CLAP tests. Byte counts
include machine-specific absolute paths and can change when installations move.

## Reproduce with current source

The standalone [public API helper](../tools/sfz-state-qualify.rs) accepts one JSON
request per line on stdin. Each request contains `path` (the absolute SFZ root),
`defines`, and `source_overlays`. The helper runs loading, state creation, and
restoration under the native `ScopedFileAssets::configured_sfz()` grant. Normal
installed contrib discovery authorizes the five SFZ asset directories; custom
library locations require `MUZ_SFZ_ASSET_ROOTS`. Serialized descriptors never
create filesystem grants. See [embedding](embedding.md#sfz-assets-and-saved-state).

Build the helper from exact Cargo JSON artifacts, with its executable inside the
checkout's `target/release` directory so ordinary contrib discovery works:

```sh
python3 tools/build-sfz-state-qualify.py
target/release/sfz-state-qualify < requests.jsonl
```

The builder uses `cargo build --locked --release --lib --no-default-features
--message-format=json` and selects the exact `muz` and `serde_json` rlib artifact
paths from Cargo messages. It does not choose between ambiguous filename globs.
The earlier retained observations used an explicitly granted `FileAssets`
resolver before final authorization hardening; their binary/source hashes remain
unchanged in the qualification JSON. The current reproduction helper exercises
the final native scoped workflow.

Generate requests from the retained qualification rows, using the local contrib
installation rather than copying this machine's absolute roots:

```python
import json
from pathlib import Path

contrib = Path("/your/authorized/muz/contrib").resolve()
proof = json.loads(Path("contrib/sfz-state-qualification.json").read_text())
for row in proof["rows"]:
    print(json.dumps({
        "path": str(contrib / row["root"].removeprefix("contrib/")),
        "defines": row["defines"],
        "source_overlays": row["source_overlays"],
    }))
```

Require every result's `roundtrip` to be true, `decode_error` to be null, and
`encoded_state_bytes` to remain below 67,108,864. The original source descriptor
limit is 33,554,432 bytes; inherited region size exceeding that is intentionally
handled by the compact linked descriptor rather than a larger cap. Hash and
metadata fences assume stable host-controlled library files during preparation;
canonical confinement is not an OS sandbox against hostile concurrent filesystem
mutation.

The current scoped-helper smoke check also records ordinary checkout contrib
discovery for Darkblack 11 without extra grants (216 regions; 19,597 state bytes),
default denial for a temporary custom library, and successful exact roundtrip
after explicitly setting `MUZ_SFZ_ASSET_ROOTS` for that directory. Its executable
identity and engine base/dirty-source provenance are separate from the historical
eight-root qualification and do not replace those observations.
