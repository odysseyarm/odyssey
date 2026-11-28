#!/usr/bin/env python3
"""Print odyssey-cv dependency notices from one or more Cargo metadata JSON files.

Run cargo metadata --locked --format-version 1 in the desktop, firmware and
private workspaces. Pass the saved JSON files to this script. Cargo's source
cache must be available, because the notices come from the resolved sources.
"""

from collections.abc import Iterator, Sequence
import json
from pathlib import Path
import re
import sys
from typing import Any


SUPPLEMENTS = Path(__file__).with_name("license-notices")
COMMENTS = re.compile(r"/\*.*?\*/|(?:^[ \t]*//[^\n]*(?:\n|$))+", re.M | re.S)


def selected_license(expression: str) -> str:
    if expression == "(MIT OR Apache-2.0) AND Unicode-3.0":
        return "MIT AND Unicode-3.0"
    if "AND" in expression or "WITH" in expression and " OR " not in expression:
        raise ValueError(f"License expression needs review: {expression}")
    if "MIT" in [
        part.strip() for part in expression.replace("/", " OR ").split(" OR ")
    ]:
        return "MIT"
    if expression == "Apache-2.0 OR BSL-1.0":
        return "BSL-1.0"
    if expression in ("Apache-2.0", "BSD-3-Clause", "MPL-2.0", "Zlib"):
        return expression
    raise ValueError(f"License expression needs review: {expression}")


def dependencies(metadata: dict[str, Any]) -> Iterator[dict[str, Any]]:
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    pending = [p["id"] for p in packages.values() if p["name"] == "odyssey-cv"]
    if not pending:
        raise ValueError("Metadata does not contain odyssey-cv")
    seen = set()
    while pending:
        ident = pending.pop()
        if ident in seen:
            continue
        seen.add(ident)
        yield packages[ident]
        pending.extend(
            dep["pkg"]
            for dep in nodes[ident]["deps"]
            if any(kind["kind"] is None for kind in dep["dep_kinds"])
        )


def notices(package: dict[str, Any]) -> tuple[str, list[tuple[str, str]]]:
    root = Path(package["manifest_path"]).parent
    expression = package["license"]
    if package["name"] == "sqpnp" and expression is None:
        expression = "BSD-3-Clause"
        if not (root / "LICENSE").read_text().startswith("BSD 3-Clause License"):
            raise ValueError("SQPnP license needs review")
    chosen = selected_license(expression or "")
    files = sorted(p for p in root.iterdir() if p.is_file())
    if chosen.startswith("MIT"):
        texts = [p for p in files if p.name.upper().startswith("LICENSE-MIT")]
    elif chosen == "BSL-1.0":
        texts = [p for p in files if p.name == "LICENSE-BOOST"]
    else:
        texts = []
    if not texts:
        texts = [
            p for p in files if p.name.upper() in ("LICENSE", "LICENSE.TXT", "COPYING")
        ]
    if not texts:
        if package["name"] == "r-efi":
            texts = [root / "AUTHORS"]
        else:
            texts = [SUPPLEMENTS / f"{package['name']}-{chosen}.txt"]
    texts += [p for p in files if p.name.upper().startswith(("NOTICE", "COPYRIGHT"))]
    if "Unicode-3.0" in chosen:
        texts.append(root / "LICENSE-UNICODE")
    result = [(p.name, p.read_text()) for p in dict.fromkeys(texts)]
    if package["name"] == "libm":
        for source in sorted((root / "src").rglob("*.rs")):
            for comment in COMMENTS.findall(source.read_text()):
                if "copyright" in comment.lower():
                    result.append((str(source.relative_to(root)), comment))
    return chosen, result


def collect(metadata_files: Sequence[str]) -> str:
    packages: dict[tuple[str, str, str], dict[str, Any]] = {}
    for filename in metadata_files:
        for package in dependencies(json.loads(Path(filename).read_text())):
            if package["name"] in ("odyssey-cv", "odyssey-common"):
                continue
            key = (package["name"], package["version"], package["source"] or "")
            packages[key] = package
    sections: dict[str, list[str]] = {}
    for key, package in sorted(packages.items()):
        chosen, texts = notices(package)
        label = f"{package['name']} {package['version']} — {chosen}"
        authors = ", ".join(package["authors"])
        origin = (
            package["repository"]
            or package["homepage"]
            or "https://crates.io/crates/" + package["name"]
        )
        if (
            package["source"]
            and package["source"].startswith("git+")
            and package["name"] != "eskf"
        ):
            origin = package["source"].removeprefix("git+")
        attribution = f"{label}\nAuthors: {authors}\nSource: {origin}\n"
        if chosen == "MPL-2.0":
            if not (package["source"] or "").startswith("registry+"):
                raise ValueError(
                    "MPL source availability needs review for non-registry sources"
                )
            attribution += (
                "The MPL-covered source is available without charge at:\n"
                f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download\n"
                "The MPL rights in that source are not restricted by Odyssey's license.\n"
            )
        body = "\n\n".join(f"[{name}]\n{text.rstrip()}" for name, text in texts)
        sections.setdefault(body, []).append(attribution)
    header = (
        "Third-party notices for odyssey-cv dependencies\n"
        "=============================================\n\n"
        "This file covers the odyssey-cv dependency tree resolved for the desktop,\n"
        "firmware and private workspaces. It includes target-specific dependencies\n"
        "and procedural macros conservatively. It does not cover every dependency\n"
        "of Odyssey. Odyssey and odyssey-common are covered by Odyssey's LICENSE.\n\n"
        "Each entry identifies the license used for redistribution. An MIT option\n"
        "is selected where available. Additional Unicode and source notices apply\n"
        "where stated. Identical notice texts are shared by the entries above them.\n\n"
        "Regenerate this file with public Odyssey's tools/collect-cv-notices.py after\n"
        "changes, using locked Cargo metadata from each affected workspace.\n"
    )
    return (
        header
        + "\n\n"
        + "\n\n".join(
            "=" * 72 + "\n\n" + "\n".join(entries) + "\n" + body
            for body, entries in sections.items()
        )
        + "\n"
    )


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit("Usage: collect-cv-notices.py CARGO_METADATA.json [...]")
    print(collect(sys.argv[1:]), end="")
