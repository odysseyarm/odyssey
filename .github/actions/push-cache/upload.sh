#!/usr/bin/env bash
set -euo pipefail
set -f

cache_action_path=$(dirname "$(realpath "$0")")
selection=$(bash "$cache_action_path/paths.sh")
mapfile -t paths <<< "$selection"
key=${NIX_SIGNING_KEY_FILE:-}
if [ -z "$key" ]; then
  : "${NIX_SIGNING_KEY:?Set NIX_SIGNING_KEY_FILE or NIX_SIGNING_KEY}"
  key=$(mktemp)
  trap 'rm -f "$key"' EXIT
  printf '%s' "$NIX_SIGNING_KEY" > "$key"
fi
nix store sign -r -k "$key" "${paths[@]}"
nix copy --to 's3://odyssey-cache?endpoint=a3499d8c0c6b0de52bda690ab62c8ded.r2.cloudflarestorage.com&region=auto&compression=zstd&parallel-compression=true' "${paths[@]}"
