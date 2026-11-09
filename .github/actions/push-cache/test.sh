#!/usr/bin/env bash
set -euo pipefail
selector=$(realpath "$(dirname "$0")/paths.sh")
shell=$(realpath "$(command -v bash)")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
mkdir source
printf 'private fixture\n' > source/private.txt
private=$(nix-store --add source)
hash=$(nix hash path source)
jq -n --arg hash "$hash" '{root:"root",nodes:{root:{inputs:{"odyssey-cv":"private"}},private:{locked:{narHash:$hash}}}}' > flake.lock
cat > fixture.nix <<'NIX'
{ shell, private }:
let
  mk = name: attrs: builtins.derivation ({
    inherit name;
    system = builtins.currentSystem;
    builder = builtins.storePath shell;
    args = [ "-c" "printf '%s' fixture > $out" ];
  } // attrs);
  dependency = mk "cache-build-only" {};
in {
  public = mk "cache-public-output" { buildOnly = dependency; };
  privateBuild = mk "cache-private-build" { source = builtins.storePath private; };
  privateRuntime = mk "cache-private-runtime" {
    source = builtins.storePath private;
    args = [ "-c" "printf '%s' \"$source\" > $out" ];
  };
}
NIX
public_drv=$(nix-instantiate fixture.nix --argstr shell "$shell" --argstr private "$private" -A public)
private_drv=$(nix-instantiate fixture.nix --argstr shell "$shell" --argstr private "$private" -A privateBuild)
runtime_drv=$(nix-instantiate fixture.nix --argstr shell "$shell" --argstr private "$private" -A privateRuntime)
public=$(nix-store --realise "$public_drv")
runtime=$(nix-store --realise "$runtime_drv")
dependency=$(nix-store --query --binding buildOnly "$public_drv")
PATHS="$public" PUBLIC_BUILDS='' bash "$selector" > runtime-paths
if grep -qxF "$dependency" runtime-paths; then
  echo 'Build-only output leaked into runtime closure' >&2
  exit 1
fi
PATHS='' PUBLIC_BUILDS="$public_drv" bash "$selector" > build-paths
grep -qxF "$public" build-paths
grep -qxF "$dependency" build-paths
if PATHS="$runtime" PUBLIC_BUILDS='' bash "$selector"; then
  echo 'Private runtime closure accepted' >&2
  exit 1
fi
if PATHS='' PUBLIC_BUILDS="$private_drv" bash "$selector"; then
  echo 'Private build closure accepted' >&2
  exit 1
fi
if PATHS='' PUBLIC_BUILDS='' bash "$selector"; then
  echo 'Empty cache selection accepted' >&2
  exit 1
fi
# shellcheck disable=SC2046
nix copy --to "file://$work/cache" $(cat build-paths)
store_hash=${dependency#/nix/store/}
test -s "cache/${store_hash%%-*}.narinfo"
printf 'Cache self-check passed: build-only layer copied; private runtime/build closures and empty selection rejected.\n'
