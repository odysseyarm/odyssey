#!/usr/bin/env bash
# shellcheck disable=SC2086
set -euo pipefail
set -f

paths=${PATHS:-}
if [ -n "${PUBLIC_BUILDS:-}" ]; then
  # Only public workspaces: private builds must use approved output paths instead.
  nix build --no-link $PUBLIC_BUILDS >&2
  derivations=$(nix path-info --derivation $PUBLIC_BUILDS)
  paths="$paths $(nix-store --query --requisites --include-outputs $derivations)"
fi
if [ -z "$paths" ]; then
  echo "No cache paths supplied" >&2
  exit 1
fi
closure=$(nix path-info -r $paths)
private_hash=$(jq -er '.nodes[.nodes[.root].inputs["odyssey-cv"]].locked.narHash' flake.lock)
private=$(nix-store --print-fixed-path --recursive sha256 "$private_hash" source)
case $'\n'"$closure"$'\n' in
  *$'\n'"$private"$'\n'*)
    echo "closure references the odyssey-cv source; refusing to push" >&2
    exit 1
    ;;
esac
printf '%s\n' "$closure"
