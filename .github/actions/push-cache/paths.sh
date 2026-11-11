#!/usr/bin/env bash
# shellcheck disable=SC2086
set -euo pipefail
set -f

paths=${PATHS:-}
public_closure=
if [ -n "${PUBLIC_BUILDS:-}" ]; then
  # Only public workspaces: private builds must use approved output paths instead.
  derivations=$(nix path-info --derivation $PUBLIC_BUILDS)
  public_paths=$(nix-store --query --requisites --include-outputs $derivations)
  public_closure=$(nix path-info -r $public_paths)
  paths="$paths $public_paths"
fi
if [ -z "$paths" ]; then
  echo "No cache paths supplied" >&2
  exit 1
fi
closure=$(nix path-info -r $paths)

# The explicit source set is persisted with queued immutable revisions. CI without
# it falls back to its checked-out lock file.
private_sources=${PRIVATE_SOURCES:-}
if [ -z "$private_sources" ]; then
  private_hash=$(jq -er '.nodes[.nodes[.root].inputs["odyssey-cv"]].locked.narHash' flake.lock)
  private_sources=$(nix-store --print-fixed-path --recursive sha256 "$private_hash" source)
fi
derivations=
outputs=
for path in $paths; do
  case "$path" in
    *.drv) derivations="$derivations $path" ;;
    *) outputs="$outputs $path" ;;
  esac
done
if [ -n "$outputs" ]; then
  derivations="$derivations $(nix-store --query --valid-derivers $outputs)"
fi
provenance=
if [ -n "$derivations" ]; then
  provenance=$(nix-store --query --requisites --include-outputs $derivations)
fi
cache_action_path=$(dirname "$(realpath "$0")")
printf '%s\0' "$closure" "$provenance" "$private_sources" "$public_closure" | python3 "$cache_action_path/safety.py"
printf '%s\n' "$closure"
