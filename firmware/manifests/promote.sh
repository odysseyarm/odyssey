#!/usr/bin/env bash
# Copy one release entry from the alpha manifest to the stable one.
#
#   promote.sh <device> <version>
#
# Stable only ever contains entries someone promoted here, so this is the one
# place customers' firmware changes. Entries are prepended, keeping the
# newest-first order hosts rely on.
set -euo pipefail

if [ $# -ne 2 ]; then
    echo "usage: $0 <device> <version>" >&2
    exit 2
fi
device=$1
version=$2
dir=$(cd "$(dirname "$0")" && pwd)
alpha="$dir/releases.alpha.json"
stable="$dir/releases.stable.json"

entry=$(jq -c --arg d "$device" --arg v "$version" \
    '.devices[$d].firmwares // [] | map(select(.version == $v)) | first // empty' "$alpha")
if [ -z "$entry" ]; then
    echo "error: $device $version is not in releases.alpha.json" >&2
    exit 1
fi
if jq -e --arg d "$device" --arg v "$version" \
    '(.devices[$d].firmwares // []) | any(.version == $v)' "$stable" >/dev/null; then
    echo "error: $device $version is already in releases.stable.json" >&2
    exit 1
fi

jq --arg d "$device" --argjson e "$entry" \
    '.devices[$d].firmwares = [$e] + (.devices[$d].firmwares // [])' "$stable" > "$stable.tmp"
mv "$stable.tmp" "$stable"
echo "Promoted $device $version to stable"
