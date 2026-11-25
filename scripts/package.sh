#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if (( $# > 1 )) || [[ -n ${1:-} && $1 != --zip ]]; then
  echo "Usage: ./scripts/package.sh [--zip]" >&2
  exit 2
fi
if [[ $(uname -s) != Linux ]]; then
  echo "This package currently supports Linux only." >&2
  exit 1
fi
version=$(./scripts/release-version.sh)
manifest=$(jq -e --arg version "$version" '.Version = $version' assets/manifest.json)
cargo build --release --locked
bundle_id="io.github.spurlos.codexlimits"
bundle="$bundle_id.sdPlugin"
output_dir=dist
if [[ ${1:-} == --zip ]]; then
  mkdir -p dist
  output_dir=$(mktemp -d dist/.package.XXXXXX)
  trap 'rm -rf "$output_dir"' EXIT
fi
plugin_dir="$output_dir/$bundle"
mkdir -p "$plugin_dir/bin" "$plugin_dir/icons" "$plugin_dir/inspector"
printf '%s\n' "$manifest" > "$plugin_dir/manifest.json"
cp -f LICENSE THIRD_PARTY_NOTICES.txt RUST_DEPENDENCY_LICENSES.txt "$plugin_dir/"
cp -f "$(rustc --print sysroot)/share/doc/rust/COPYRIGHT-library.html" \
  "$plugin_dir/RUST_STANDARD_LIBRARY_LICENSES.html"
cp -f assets/icons/*.svg "$plugin_dir/icons/"
cp -f assets/inspector/* "$plugin_dir/inspector/"
cp -f "${CARGO_TARGET_DIR:-target}/release/opendeck-codex-limits" "$plugin_dir/bin/"
chmod 755 "$plugin_dir/bin/opendeck-codex-limits"
if [[ ${1:-} == --zip ]]; then
  asset="$bundle_id-linux-$(uname -m).zip"
  (cd "$output_dir" && zip -qr "$asset" "$bundle")
  unzip -tq "$output_dir/$asset" >&2
  unzip -p "$output_dir/$asset" "$bundle/manifest.json" |
    jq -e --arg version "$version" '.Version == $version' > /dev/null
  mv -f "$output_dir/$asset" "dist/$asset"
  printf 'dist/%s\n' "$asset"
else
  echo "Built $plugin_dir for $(uname -m). Copy this directory into OpenDeck's plugins directory."
fi
