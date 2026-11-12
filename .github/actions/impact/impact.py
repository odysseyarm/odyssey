#!/usr/bin/env python3
"""Decide which CI suites a change can affect, as `suite=true|false` lines.

Pull requests, merge groups and manual runs on a feature branch compare against
their base. Every other event runs every suite, as does any failure here: a
selector that guesses wrong must cost time, never coverage.

A suite is affected by the Cargo path-dependency closure of its crates plus the
non-Cargo inputs listed in `build_rules`. Cargo.lock files are listed by hand
because the SDK workspaces carry their own and ignore the root one.
"""

from __future__ import annotations

import os
import posixpath
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from fnmatch import fnmatchcase
from pathlib import Path

SUITES = ("host", "xtask", "proofs", "cache", "tracking", "ffi", "uniffi", "format")

DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")

# Anything that changes every build: toolchain, Nix source filter, shared Nix
# code, Cargo configuration and this selector.
EVERYWHERE = (
    "rust-toolchain.toml",
    "flake.nix",
    "flake.lock",
    ".cargo",
    "nix/common.nix",
    "nix/default.nix",
    "nix/pkgs",
    ".github/actions/setup-nix",
    ".github/actions/impact",
)

# Files the host and tracking builds read without a Cargo dependency on them:
# the firmware manifests and memory maps embedded by odyssey-hub-server, and
# helper tools.
HOST_DATA = (
    "firmware/manifests",
    "firmware/memory",
    "firmware/dongle-fw/memory-dongle.kdl",
    "tools",
)


@dataclass
class Rule:
    paths: list[str]
    skip: tuple[str, ...] = ()
    match: object = None
    reasons: dict[str, str] = field(default_factory=dict)


def inside(path: str, prefix: str) -> bool:
    prefix = prefix.rstrip("/")
    return path == prefix or path.startswith(prefix + "/")


def dependency_paths(directory: str, data: dict, shared: dict[str, str]) -> set[str]:
    tables = [data.get(key, {}) for key in DEPENDENCY_TABLES]
    for target in data.get("target", {}).values():
        tables.extend(target.get(key, {}) for key in DEPENDENCY_TABLES)
    tables.extend(data.get("patch", {}).values())
    found = set()
    for table in tables:
        for name, dependency in table.items():
            if not isinstance(dependency, dict):
                continue
            if "path" in dependency:
                found.add(posixpath.normpath(posixpath.join(directory, dependency["path"])))
            elif dependency.get("workspace") and name in shared:
                found.add(posixpath.normpath(shared[name]))
    return found


def load_graph(root: Path, manifests: list[str]) -> tuple[dict[str, set[str]], dict]:
    parsed = {
        posixpath.dirname(manifest) or ".": tomllib.loads((root / manifest).read_text(encoding="utf-8-sig"))
        for manifest in manifests
    }
    workspace = parsed.get(".", {}).get("workspace", {})
    shared = {
        name: dependency["path"]
        for name, dependency in workspace.get("dependencies", {}).items()
        if isinstance(dependency, dict) and "path" in dependency
    }
    graph = {
        directory: dependency_paths(directory, data, shared)
        for directory, data in parsed.items()
        if directory != "."
    }
    return {d: {e for e in edges if e in graph} for d, edges in graph.items()}, workspace


def closure(graph: dict[str, set[str]], roots: list[str]) -> list[str]:
    missing = [root for root in roots if root not in graph]
    if missing:
        raise LookupError(f"no Cargo manifest in {missing}")
    seen: set[str] = set()
    todo = list(roots)
    while todo:
        directory = todo.pop()
        if directory not in seen:
            seen.add(directory)
            todo.extend(graph[directory])
    return sorted(seen)


def build_rules(graph: dict[str, set[str]], workspace: dict) -> dict[str, Rule]:
    members = [
        directory
        for directory in graph
        if any(fnmatchcase(directory, pattern) for pattern in workspace.get("members", []))
        and not any(inside(directory, excluded) for excluded in workspace.get("exclude", []))
    ]
    if not members:
        raise LookupError("the root workspace lists no members")
    workflow = ".github/workflows/"
    sdk_skip = ("samples", "Cross.toml")

    def sdk(name: str) -> Rule:
        directory = f"sdk/{name}"
        return Rule(
            closure(graph, [directory])
            + ["Cargo.toml", "LICENSE", "nix/sdk.nix", "nix/xwin.nix", ".github/actions/push-cache"],
            skip=tuple(f"{directory}/{entry}" for entry in sdk_skip),
        )

    ffi, uniffi = sdk("odyssey-hub-client-ffi"), sdk("odyssey-hub-client-uniffi")
    ffi.paths += [f"{workflow}ffi-build.yml", f"{workflow}ffi-ci.yml"]
    uniffi.paths += [f"{workflow}uniffi-build.yml", f"{workflow}uniffi-ci.yml"]

    def lockfile(path: str) -> bool:
        return posixpath.basename(path) in ("Cargo.toml", "Cargo.lock")

    def rust(path: str) -> bool:
        return path.endswith(".rs")

    return {
        "host": Rule(
            closure(graph, members)
            + [*HOST_DATA, "Cargo.toml", "Cargo.lock", "clippy.toml", "nix/host.nix", f"{workflow}host-ci.yml"]
        ),
        "tracking": Rule(
            closure(graph, ["crates/odyssey-hub-server", "crates/odyssey-sim"])
            + [
                *HOST_DATA,
                "Cargo.toml",
                "Cargo.lock",
                "nix/host.nix",
                f"{workflow}tracking-regression.yml",
            ]
        ),
        # The repository tests read firmware and SDK manifests directly, so any
        # Cargo manifest or lock file counts.
        "xtask": Rule(
            closure(graph, ["crates/xtask"])
            + [
                "firmware",
                "crates/odyssey-autocalib",
                "crates/odyssey-desktop/orbbec-sdk.json",
                "nix/host.nix",
                f"{workflow}host-ci.yml",
            ],
            match=lockfile,
        ),
        "proofs": Rule(
            closure(graph, ["crates/odyssey-protocol", "crates/odyssey-version"])
            + [
                "Cargo.toml",
                "Cargo.lock",
                "crates/xtask/Cargo.toml",
                "crates/xtask/Cargo.lock",
                f"{workflow}host-ci.yml",
            ]
        ),
        "cache": Rule([".github/actions/push-cache", f"{workflow}host-ci.yml"]),
        "format": Rule(["rustfmt.toml", "nix/host.nix", f"{workflow}fmt.yml"], match=rust),
        "ffi": ffi,
        "uniffi": uniffi,
    }


def affected(changed: list[str], rules: dict[str, Rule]) -> dict[str, str | None]:
    result: dict[str, str | None] = {}
    for suite, rule in rules.items():
        result[suite] = None
        for path in changed:
            if any(inside(path, skipped) for skipped in rule.skip):
                continue
            listed = any(inside(path, prefix) for prefix in (*rule.paths, *EVERYWHERE))
            if listed or (rule.match is not None and rule.match(path)):
                result[suite] = path
                break
    return result


def git(*arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments], check=True, capture_output=True, text=True
    ).stdout


def comparison(environment: dict[str, str]) -> tuple[str, str] | None:
    event = environment.get("EVENT", "")
    if event in ("pull_request", "merge_group"):
        return environment["BASE"], environment["HEAD"]
    default = environment.get("DEFAULT_BRANCH", "")
    if event == "workflow_dispatch" and default and environment.get("REF_NAME") != default:
        return f"origin/{default}", "HEAD"
    return None


def changed_files(base: str, head: str) -> list[str]:
    listing = git("diff", "--name-only", "--no-renames", "-z", f"{base}...{head}")
    return [path for path in listing.split("\0") if path]


def select(environment: dict[str, str]) -> tuple[dict[str, str | None], str]:
    everything = {suite: "run everything" for suite in SUITES}
    compared = comparison(environment)
    if compared is None:
        return everything, f"{environment.get('EVENT', 'unknown')} runs every suite"
    try:
        manifests = [p for p in git("ls-files", "-z", "--", "*Cargo.toml").split("\0") if p]
        graph, workspace = load_graph(Path("."), manifests)
        changed = changed_files(*compared)
        return affected(changed, build_rules(graph, workspace)), f"{len(changed)} changed files"
    except Exception as error:  # noqa: BLE001 - a selector failure must never skip tests
        return everything, f"selection failed ({type(error).__name__}: {error}); running every suite"


def render(result: dict[str, str | None]) -> dict[str, bool]:
    flags = {suite: result[suite] is not None for suite in SUITES}
    flags["host_ci"] = any(flags[suite] for suite in ("host", "xtask", "proofs", "cache"))
    return flags


def main() -> None:
    result, note = select(dict(os.environ))
    flags = render(result)
    for name, value in flags.items():
        print(f"{name}={'true' if value else 'false'}")
    lines = [f"Suite selection: {note}"]
    lines += [f"- {suite}: {result[suite] or 'skipped'}" for suite in SUITES]
    print("\n".join(lines), file=sys.stderr)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
