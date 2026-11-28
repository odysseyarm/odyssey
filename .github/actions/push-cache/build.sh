#!/usr/bin/env bash
set -euo pipefail
flake=${1:-.}
exec nix build -L --no-link "$flake#host.cargoArtifacts" "$flake#xtask"
