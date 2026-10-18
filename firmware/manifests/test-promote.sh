#!/usr/bin/env bash
# Exercise promote.sh against synthetic manifests in a scratch directory.
#
#   test-promote.sh
#
# Nothing here touches the committed manifests: promote.sh finds the manifests
# next to itself, so the script is copied into the scratch directory first.
set -euo pipefail

src=$(cd "$(dirname "$0")" && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cp "$src/promote.sh" "$scratch/"

failures=0

# entry <version> <requires_netcore> [carried net-core release]
entry() {
    jq -n --arg v "$1" --argjson r "$2" --arg c "${3:-}" '{
        version: $v, requires_bootloader: 0, requires_netcore: $r, min_desktop: "0.6.0",
        files: ({"app-a": {sha256: "00", size: 1}}
            + (if $c == "" then {} else {"net-core": {sha256: "00", size: 1, release: ($c | tonumber)}} end))
    }'
}

# manifest <entry>... -> manifest with those entries, newest first
manifest() {
    jq -n --argjson e "$(printf '%s\n' "$@" | jq -s .)" '{version: 1, devices: {atslite1: {firmwares: $e}}}'
}

# reset <stable entries...>: alpha holds the bridge chain, stable holds the given entries
reset() {
    manifest "$(entry 0.3.0 2 2)" "$(entry 0.2.1 0 2)" "$(entry 0.2.0 0 1)" "$(entry 0.1.0 0)" > "$scratch/releases.alpha.json"
    manifest "$@" > "$scratch/releases.stable.json"
}

# expect <name> <pass|fail> <version> [message fragment]
expect() {
    local name=$1 want=$2 version=$3 fragment=${4:-}
    local before after out status=0
    before=$(sha256sum "$scratch/releases.stable.json")
    out=$("$scratch/promote.sh" atslite1 "$version" 2>&1) || status=$?
    after=$(sha256sum "$scratch/releases.stable.json")
    if [ "$want" = pass ] && [ "$status" -ne 0 ]; then
        echo "FAIL $name: expected success, got: $out"; failures=$((failures + 1)); return
    fi
    if [ "$want" = fail ]; then
        if [ "$status" -eq 0 ]; then
            echo "FAIL $name: expected a refusal"; failures=$((failures + 1)); return
        fi
        if [ -n "$fragment" ] && ! grep -q "$fragment" <<<"$out"; then
            echo "FAIL $name: refusal did not mention '$fragment': $out"; failures=$((failures + 1)); return
        fi
        if [ "$before" != "$after" ]; then
            echo "FAIL $name: stable changed despite the refusal"; failures=$((failures + 1)); return
        fi
    fi
    echo "ok   $name"
}

# Skipping the bridge would publish a head no stable unit can install.
reset "$(entry 0.1.0 0)"
expect "post-bridge entry before its bridge is refused" fail 0.3.0 "promote the bridge entry"

# In order: the bridge, then the entry that needs what it carries.
reset "$(entry 0.1.0 0)"
expect "the bridge promotes" pass 0.2.1
expect "the entry requiring the bridge's release then promotes" pass 0.3.0

# The floor is judged against stable's head, not against anything in alpha.
reset "$(entry 0.2.0 0 1)" "$(entry 0.1.0 0)"
expect "a floor above the head's net core is refused" fail 0.3.0 "carries 1"

# Version order is still enforced, and takes precedence by running first.
reset "$(entry 0.2.1 0 2)" "$(entry 0.1.0 0)"
expect "an older version than the head is refused" fail 0.2.0 "not newer"

if [ "$failures" -ne 0 ]; then
    echo "$failures failed" >&2
    exit 1
fi
echo "all passed"
