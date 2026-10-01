# Native SFZ performance qualification

The release acceptance target is unchanged: on declared reference workloads,
callback p99 must be below half the callback duration, with no callback deadline
overruns, dropped regions or nonfinite output. Allocation tests are separate.
Successful loading, bounded polyphony and deterministic output do not pass a
timing gate.

## Reproduce a workload

Build `cargo build --release --bin sfz-audit`. The command accepts JSON-lines
requests with `path`, optional `defines` and `source_overlays`, and optional
`profile_workload` (`four_held_notes` or `drum_pattern`), and optional
`profile_switch` (a specific MIDI keyswitch). Use the complete installed
mapping and bindings from its catalog, then run:

```sh
taskset -c 8 target/release/sfz-audit --profile < requests.jsonl > results.jsonl
```

Choose an available CPU for the host. The report records actual affinity, CPU,
compiler, executable SHA-256, source revision/status and selected keys, controls
and switch. Copy the executable before a long run so another build cannot change
the binary used by later requests. A failed timing gate returns nonzero after
writing its report; preserve that output.

Each root prepares once at 44.1, 48 and 96 kHz, then reuses/reset its processor for
64, 256 and 1024-frame callbacks. A held-note workload plays four simultaneous
owned notes for two seconds, followed by one second of release. The drum workload
plays 24 hits at eight steps per second, with short note gates and recorded
velocities. Keys map to the closest available source key. The derived predicate
selects an articulation and any necessary controller gate; inspect those settings
before interpreting the result as a particular factory or microphone preset.

Timing surrounds `DeviceProcessor::process`; event construction, inspection and
output hashing are outside it. Nearest-rank p50/p99/max include note-start and
release callbacks. Wall-clock overruns remain failures. Voice maxima are sampled
at callback ends and can miss shorter peaks. Decoded stereo bytes are exact
`frames * 8`; RSS includes allocator and other process state, and VmHWM is explicitly
cumulative across earlier requests. A shallow compiled-DSP byte count excludes
curve/route heap allocations. Normalized-program hashes include resolved absolute
paths and identify a run, rather than a relocatable publisher program.

## Recorded baseline

[Baseline measurements](../contrib/sfz-performance-baseline.json) contain the
immutable-executable results on an AMD Ryzen Threadripper 3990X, Linux x86_64,
rustc 1.98.1, pinned to CPU 8 with competing reference jobs paused.

The initial seven roots cover Trumpet, basic/full Virtuosity, dense All Brass,
Organ Combinations, Standard KSOP and METAL Full. **60 of 63 combinations pass.**
Dense All Brass's four-note chord reaches 54 voices. Its three 96 kHz combinations
fail the p99 target at roughly 1.8 times half the callback duration; 64 frames has
five actual overruns. An exact cutoff-conversion cache reduces that ratio to about
1.7 with two overruns, so the gate remains unmet.

The Virtuosity pattern reaches 33 voices and passes both program grids; its shared
drum keys produce identical audio in those settings. This does not measure every
extra instrument, microphone control or maximum stack. Full kit decoding is about
4.9 GiB and takes 35–36 seconds per preparation in this run. Organ uses a selected
registration. The METAL measurement selects switch 5, a slide-effects articulation;
a separate normal factory articulation profile explicitly selects switch 17,
reaches 16 voices and passes all nine combinations. Neither filename size
nor complete decoding establishes worst-case synthesis coverage.

Subsequent controller-generation and extended LFO/filter/EQ control caches
retain exact PCM. Pitch caches still miss the Brass 96 kHz half-callback target
at ratios 1.065/1.049/1.050. Exact masks that bypass disabled EQ and zero-depth
LFO destinations then make all nine targeted Brass cases pass, with zero
overruns/drops and 96 kHz ratios 0.981/0.968/0.971 for 64/256/1024-frame blocks.
The margin is narrow; the final complete workload grid remains pending. Immutable
binary hashes and timing distributions stay beside the initial failed baseline.

The fixed 256-layer synthetic stress benchmark also exceeds realtime. Keep the
remaining qualification plan and canonical friction report until the required
timing target is met or an explicit user decision changes the release scope.
