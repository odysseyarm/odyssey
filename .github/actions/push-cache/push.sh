#!/usr/bin/env bash
set -euo pipefail
set -f

cache_action_path=$(dirname "$(realpath "$0")")
repo=$(jj root)
for arg in "$@"; do
  if [ "$arg" = --dry-run ]; then
    exec jj --color never git push "$@"
  fi
done
if ! push_plan=$(jj --color never git push --dry-run "$@" 2>&1); then
  printf '%s\n' "$push_plan" >&2
  exit 1
fi
printf '%s\n' "$push_plan"

if [[ $push_plan == *'Nothing changed.'* ]]; then
  exit 0
fi
if [[ $push_plan =~ Changes\ to\ push\ to\ ([^:]+): ]]; then
  remote=${BASH_REMATCH[1]}
else
  echo 'Could not determine remote from jj push dry-run; refusing push' >&2
  exit 1
fi

declare -A registered=()
saw_change=false
while IFS= read -r line; do
  [[ $line == *bookmark:* ]] || continue
  saw_change=true
  [[ $line == *'[delete'* ]] && continue
  if [[ $line =~ to\ ([[:xdigit:]]{8,})\] ]]; then
    prefix=${BASH_REMATCH[1]}
  else
    echo "Could not determine proposed target from jj dry-run: $line" >&2
    exit 1
  fi
  if ! commit=$(jj log --no-graph -r "$prefix" -T 'commit_id ++ "\n"'); then
    echo "Could not resolve proposed target $prefix" >&2
    exit 1
  fi
  if [[ ! $commit =~ ^[[:xdigit:]]{40,64}$ ]]; then
    echo "Unexpected commit id for proposed target $prefix: $commit" >&2
    exit 1
  fi
  remote_literal=$(jq -cn --arg remote "$remote" '$remote')
  outgoing=$(jj log --no-graph -r "(ancestors($commit) ~ ancestors(remote_bookmarks(remote=$remote_literal)) | $commit) ~ root()" -T 'commit_id ++ "\n"')
  while IFS= read -r revision; do
    [[ -n $revision ]] || continue
    [[ ${registered[$revision]+present} ]] && continue
    uri="git+file://${repo}?rev=${revision}"
    bash "$cache_action_path/local.sh" --register "$uri"
    registered[$revision]=1
  done <<< "$outgoing"
done <<< "$push_plan"
if ! "$saw_change"; then
  echo 'Unrecognized jj push plan; refusing unchecked push.' >&2
  exit 1
fi
bash "$cache_action_path/local.sh" --flush
confirmed=$(jj --color never git push --dry-run "$@" 2>&1)
if [ "$confirmed" != "$push_plan" ]; then
  echo 'Push targets changed during cache upload; retry push.' >&2
  exit 1
fi
exec jj git push "$@"
