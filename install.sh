#!/bin/sh
set -eu

cd "$(dirname "$0")"

case "$(uname -s)" in
  Linux)
    plugin=target/release/libmuz_clap.so
    clap_dir=${CLAP_DIR:-"$HOME/.clap"}
    ;;
  Darwin)
    plugin=target/release/libmuz_clap.dylib
    clap_dir=${CLAP_DIR:-"$HOME/Library/Audio/Plug-Ins/CLAP"}
    ;;
  *)
    printf '%s\n' "install.sh supports Linux and macOS; install the CLAP plugin manually on this platform." >&2
    exit 1
    ;;
esac

cargo install --path .
cargo build --release --package muz-clap --no-default-features

mkdir -p "$clap_dir"
cp "$plugin" "$clap_dir/Muz.clap"
printf 'Installed Muz.clap to %s\n' "$clap_dir"
