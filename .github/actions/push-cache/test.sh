#!/usr/bin/env bash
set -euo pipefail
selector=$(realpath "$(dirname "$0")/paths.sh")
local_selector=$(realpath "$(dirname "$0")/local.sh")
queue_check=$(realpath "$(dirname "$0")/queue_test.py")
# The fixture's builder has to be a store path. A runner's /usr/bin/bash is not one.
repo=$(realpath "$(dirname "$0")/../../..")
shell=$(nix build --no-link --print-out-paths --inputs-from "$repo" nixpkgs#bash.out)/bin/bash
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
mkdir source
printf 'fn private_fixture() { let secret = 73; }\n' > source/private.rs
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
  unbuilt = mk "cache-never-built" { buildOnly = dependency; };
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
missing_private=/nix/store/00000000000000000000000000000000-missing-private-source
PRIVATE_SOURCES="$missing_private" PATHS='' PUBLIC_BUILDS="$public_drv" bash "$selector" > public-without-private
grep -qxF "$public" public-without-private
if PRIVATE_SOURCES="$missing_private" PATHS="$public" PUBLIC_BUILDS='' bash "$selector"; then
  echo 'Non-public payload accepted without private-source fingerprints' >&2
  exit 1
fi
touch proof-marker
proof_marker=$(nix-store --add proof-marker)
PRIVATE_SOURCES="$missing_private" PATHS="$proof_marker" PUBLIC_BUILDS='' bash "$selector" > marker-without-private
grep -qxF "$proof_marker" marker-without-private
unbuilt_drv=$(nix-instantiate fixture.nix --argstr shell "$shell" --argstr private "$private" -A unbuilt)
unbuilt=$(nix-store --query --outputs "$unbuilt_drv")
ODYSSEY_CACHE_STATE="$work/local-output" PUBLIC_CACHE_PLAN="outputs public $public_drv $public
closures unbuilt $unbuilt_drv $unbuilt" bash "$local_selector" --dry-run > local-output-paths
grep -qxF "$public" local-output-paths
if grep -qxF -e "$dependency" -e "$unbuilt" -e "$unbuilt_drv" local-output-paths; then
  echo 'Local output policy selected a build dependency or an unbuilt derivation' >&2
  exit 1
fi
ODYSSEY_CACHE_STATE="$work/local-closure" PUBLIC_CACHE_PLAN="closures public $public_drv $public" bash "$local_selector" --dry-run > local-closure-paths
grep -qxF "$dependency" local-closure-paths
ODYSSEY_CACHE_STATE="$work/local-unbuilt" PUBLIC_CACHE_PLAN="closures unbuilt $unbuilt_drv $unbuilt" bash "$local_selector" --dry-run > local-unbuilt-paths
if [ -s local-unbuilt-paths ] || nix-store --check-validity "$unbuilt" 2>/dev/null; then
  echo 'Local selection uploaded or built an unbuilt derivation' >&2
  exit 1
fi
python3 "$queue_check" "$public_drv" "$public" "$dependency" "$unbuilt_drv" "$unbuilt"
# shellcheck disable=SC2046
nix copy --to "file://$work/cache" $(cat build-paths)
store_hash=${dependency#/nix/store/}
test -s "cache/${store_hash%%-*}.narinfo"
isolated="local?root=$work/isolated"
nix copy --no-check-sigs --from "file://$work/cache" --to "$isolated" "$public"
if NIX_REMOTE="$isolated" nix-store --check-validity "$public_drv" 2>/dev/null; then
  echo 'Isolated output copy unexpectedly includes its derivation' >&2
  exit 1
fi
NIX_REMOTE="$isolated" PATHS="$public" PUBLIC_BUILDS='' bash "$selector" > isolated-paths
grep -qxF "$public" isolated-paths
python3 "$repo/.github/actions/push-cache/safety_test.py"
python3 "$repo/.github/actions/push-cache/revision_gate_test.py" "$shell" "$private"
printf 'Cache self-check passed: build-only layer copied; private runtime/build closures and empty selection rejected; local selection keeps output-only and unbuilt derivations out.\n'
