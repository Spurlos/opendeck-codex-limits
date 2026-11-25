#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if ! git diff --quiet HEAD -- Cargo.toml Cargo.lock; then
  echo "Commit the Cargo.toml and Cargo.lock changes before tagging a release." >&2
  exit 1
fi

version=$(bash scripts/release-version.sh)
tag="v$version"
git tag -a "$tag" -m "Release $tag" HEAD
printf 'Created local release tag %s.\n' "$tag"
