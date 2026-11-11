#!/usr/bin/env bash
set -euo pipefail
scripts=$(dirname "$(realpath "$0")")
if [ "${1:-}" = --push ]; then
  shift
  exec bash "$scripts/push.sh" "$@"
fi
exec python3 "$scripts/local.py" "$@"
