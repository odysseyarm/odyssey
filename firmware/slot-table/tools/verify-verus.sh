#!/bin/sh
set -eu

command -v verus >/dev/null 2>&1 || {
    printf '%s\n' 'error: verus not found on PATH' >&2
    exit 127
}

grep -Eq '^pub fn slot_for<' src/lib.rs || {
    printf '%s\n' 'error: src/lib.rs has no slot_for to verify' >&2
    exit 1
}

exec verus --crate-type=lib src/lib.rs --no-external-by-default --no-cheating --triggers-mode silent
