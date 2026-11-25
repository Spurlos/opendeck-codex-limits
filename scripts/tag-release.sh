#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if (( $# > 1 )) || [[ -n ${1:-} && $1 != --push ]]; then
  echo "Usage: ./scripts/tag-release.sh [--push]" >&2
  exit 2
fi

if ! git diff --quiet HEAD -- Cargo.toml Cargo.lock; then
  echo "Commit the Cargo.toml and Cargo.lock changes before tagging a release." >&2
  exit 1
fi

version=$(./scripts/release-version.sh)
tag="v$version"
ref="refs/tags/$tag"

if [[ ${1:-} == --push ]]; then
  if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "The release workflow requires a stable version (MAJOR.MINOR.PATCH)." >&2
    exit 1
  fi
  git remote get-url origin > /dev/null

  if git show-ref --verify --quiet "$ref"; then
    if [[ $(git cat-file -t "$ref") != tag || $(git rev-parse "$ref^{commit}") != "$(git rev-parse HEAD)" ]]; then
      echo "The existing tag $tag must be annotated and point to HEAD." >&2
      exit 1
    fi
  else
    git tag -a "$tag" -m "Release $tag" HEAD
  fi
  git push --no-follow-tags origin "$ref:$ref"
else
  git tag -a "$tag" -m "Release $tag" HEAD
  printf 'Created local release tag %s.\n' "$tag"
fi
