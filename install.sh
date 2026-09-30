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

# Match the resolver's XDG convention on both supported desktop platforms.
case "${XDG_DATA_HOME:-}" in
  /*) data_dir=$XDG_DATA_HOME ;;
  *) data_dir="$HOME/.local/share" ;;
esac
contrib_dir="$data_dir/muz/contrib"
mkdir -p "$contrib_dir"
# Refuse copying onto/into the source tree (including paths reached by symlinks).
source_contrib=$(cd contrib && pwd -P)
installed_contrib=$(cd "$contrib_dir" && pwd -P)
case "$installed_contrib/" in
  "$source_contrib/"*)
    printf '%s\n' "Installed contrib directory must be outside the checkout's contrib tree." >&2
    exit 1
    ;;
esac

cargo install --path .
cargo build --release --package muz-clap --no-default-features

mkdir -p "$clap_dir"
cp "$plugin" "$clap_dir/Muz.clap"
printf 'Installed Muz.clap to %s\n' "$clap_dir"

# Merge rather than replace: retain downloads already in the installed tree.
# Existing checkout assets are copied too, keeping module-relative paths intact.
cp -R contrib/. "$contrib_dir/"
printf 'Installed contrib packs to %s\n' "$contrib_dir"
