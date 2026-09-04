# muz

A headless composition and production studio, built in Rust. Work in progress; see [PLAN.md](PLAN.md) and [docs/status.md](docs/status.md).

Build with `cargo build --release`, then `cargo install --path .` to put `muz` on PATH. Linux is the current host platform. Playback uses PipeWire; `serve --headless` and offline work need no audio device. No Python, ffmpeg, GUI, or other music program is needed to compose or render.

```
muz new my-song
muz check my-song/song.muz
muz render my-song/song.muz -o mix.wav
muz analyze mix.wav
muz serve my-song/song.muz
muz audition opening
muz render --socket /tmp/muz.sock --section opening -o audition.wav
muz jobs
muz shutdown
```

`serve` watches source and imports. Invalid edits retain the last accepted graph. Named compatible devices retain their running state across reloads; structural changes are prepared off the audio thread. Headless operation has the same control protocol and clock. Background renders run another `muz` process against the accepted session, independent of live playback. `muz call '{"command":"status"}'` exposes the same newline JSON protocol used by agents. Socket permissions are 0600.

Use `muz docs language`, `muz docs production`, and `muz --help` for the bundled references. The two pieces in `projects/` are freely editable music, never fixtures. Dedicated synthetic tests cover tricky timing, routing and performance invariants.
