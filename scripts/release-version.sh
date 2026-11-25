#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo metadata --locked --no-deps --format-version 1 |
  jq -er '.packages[] | select(.name == "opendeck-codex-limits") | .version'
