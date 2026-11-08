#!/usr/bin/env bash
# Regenerate nix/firmware-git-hashes.json: the fixed-output hash of every git
# dependency in the firmware Cargo.lock files, keyed by the lock's source string.
# Run it from the repository root after changing a git dependency or its revision.
set -euo pipefail

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT

cat firmware/*/Cargo.lock | grep -o '^source = "git+[^"]*"' | sed 's/^source = "//; s/"$//' | sort -u |
  while read -r source; do
    url=${source#git+}
    rev=${url##*#}
    url=${url%%#*}
    url=${url%%\?*}
    hash=$(nix run nixpkgs#nix-prefetch-git -- --url "$url" --rev "$rev" --fetch-submodules --quiet | jq -r .hash)
    printf '%s\t%s\n' "$source" "$hash"
  done |
  jq -Rn '[inputs | split("\t") | {key: .[0], value: .[1]}] | from_entries' > nix/firmware-git-hashes.json
