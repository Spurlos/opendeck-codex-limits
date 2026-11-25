#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

shellcheck_version=0.11.0
actionlint_version=1.7.12
platform="$(uname -s)-$(uname -m)"

case "$platform" in
  Linux-x86_64)
    shellcheck_arch=x86_64
    shellcheck_sha=b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6
    actionlint_arch=amd64
    actionlint_sha=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8
    ;;
  Linux-aarch64)
    shellcheck_arch=aarch64
    shellcheck_sha=68a8133197a50beb8803f8d42f9908d1af1c5540d4bb05fdfca8c1fa47decefc
    actionlint_arch=arm64
    actionlint_sha=325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6
    ;;
  *)
    printf 'Pinned lint tools are available for Linux x86-64 and ARM64. Detected %s.\n' "$platform" >&2
    exit 1
    ;;
esac

tool_dir="target/tools/$platform"
shellcheck_bin="$tool_dir/shellcheck-$shellcheck_version"
actionlint_bin="$tool_dir/actionlint-$actionlint_version"

install_linter() (
  local destination=$1 url=$2 checksum=$3 member=$4 temp_dir
  if [[ -x "$destination" ]]; then
    return
  fi

  mkdir -p "$tool_dir"
  temp_dir=$(mktemp -d "$tool_dir/.install.XXXXXX")
  trap 'rm -rf "$temp_dir"' EXIT
  curl --fail --silent --show-error --location --retry 3 "$url" --output "$temp_dir/tool.tar.gz"
  if ! printf '%s  %s\n' "$checksum" "$temp_dir/tool.tar.gz" | sha256sum --check --status; then
    printf 'Checksum verification failed for %s.\n' "$url" >&2
    exit 1
  fi
  tar -xOzf "$temp_dir/tool.tar.gz" "$member" > "$temp_dir/tool"
  chmod 755 "$temp_dir/tool"
  mv -f "$temp_dir/tool" "$destination"
)

install_linter "$shellcheck_bin" \
  "https://github.com/koalaman/shellcheck/releases/download/v$shellcheck_version/shellcheck-v$shellcheck_version.linux.$shellcheck_arch.tar.gz" \
  "$shellcheck_sha" "shellcheck-v$shellcheck_version/shellcheck"
install_linter "$actionlint_bin" \
  "https://github.com/rhysd/actionlint/releases/download/v$actionlint_version/actionlint_${actionlint_version}_linux_$actionlint_arch.tar.gz" \
  "$actionlint_sha" actionlint

cargo fmt --all --check
"$shellcheck_bin" scripts/*.sh
"$actionlint_bin" -shellcheck "$shellcheck_bin" -pyflakes=
cargo clippy --locked --all-targets -- -D warnings
