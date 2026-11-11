#!/usr/bin/env bash
set -euo pipefail
if [ "$#" -ne 1 ]; then
  echo 'usage: register.sh FLAKE_URI' >&2
  exit 2
fi
export ODYSSEY_CACHE_FLAKE=$1 ODYSSEY_CACHE_REPO
repo=$(jj root)
revision=$(jj log --no-graph -r @ -T commit_id)
ODYSSEY_CACHE_REPO="git+file://${repo}?rev=${revision}"
if [ "$ODYSSEY_CACHE_FLAKE" = . ]; then ODYSSEY_CACHE_FLAKE=$ODYSSEY_CACHE_REPO; fi
# shellcheck disable=SC2016
nix eval --impure --raw --expr '
  let
    inherit (builtins) attrNames concatMap concatStringsSep currentSystem filter getAttr getEnv getFlake hasAttr map;
    source = getFlake (getEnv "ODYSSEY_CACHE_FLAKE");
    current = getFlake (getEnv "ODYSSEY_CACHE_REPO");
    exported = source.legacyPackages.${currentSystem}.publicCache or (source.publicCache or null);
    allowlist = current.legacyPackages.${currentSystem}.publicCache;
    packages = source.packages.${currentSystem} or {};
    checks = source.checks.${currentSystem} or {};
    legacy = name:
      if name == "host-artifacts" && packages ? host then packages.host.cargoArtifacts
      else if hasAttr name packages then getAttr name packages
      else if hasAttr name checks then getAttr name checks
      else null;
    entries = policy:
      let
        names = attrNames (if exported != null then getAttr policy exported else getAttr policy allowlist);
        drvFor = name: if exported != null then getAttr name (getAttr policy exported) else legacy name;
      in map (name:
        let drv = drvFor name; in concatStringsSep " "
          ([ policy name drv.drvPath ] ++ map (output: (getAttr output drv).outPath) drv.outputs)
      ) (filter (name: drvFor name != null) names);
  in concatStringsSep "\n" ([ "private-source ${source.inputs.odyssey-cv.outPath}" ] ++ concatMap entries [ "closures" "outputs" ])
'
