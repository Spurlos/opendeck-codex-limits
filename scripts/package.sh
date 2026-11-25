#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $(uname -s) != Linux ]]; then
  echo "This package currently supports Linux only." >&2
  exit 1
fi
cargo build --release --locked
plugin_dir="dist/io.github.spurlos.codexlimits.sdPlugin"
mkdir -p "$plugin_dir/bin" "$plugin_dir/icons" "$plugin_dir/inspector"
cp -f assets/manifest.json "$plugin_dir/manifest.json"
cp -f LICENSE THIRD_PARTY_NOTICES.txt "$plugin_dir/"
cp -f assets/icons/*.svg "$plugin_dir/icons/"
cp -f assets/inspector/* "$plugin_dir/inspector/"
cp -f target/release/opendeck-codex-limits "$plugin_dir/bin/"
chmod 755 "$plugin_dir/bin/opendeck-codex-limits"
echo "Built $plugin_dir for $(uname -m). Copy this directory into OpenDeck's plugins directory."
