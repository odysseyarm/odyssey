#!/usr/bin/env python3
"""Artifact contract for the Odyssey AppImage payload.

Checks an extracted AppDir (and optionally the AppImage file itself) with
readelf, not with the patchelf that produced it. Exits non-zero listing every
violation: wrong interpreter, RPATH/RUNPATH leakage, dependencies that are
neither bundled nor allowed from the host, bundled host libraries, symbol
versions above what the reference 0.7.4 AppImage required, /nix/store paths,
and a malformed .desktop file or icon.
"""
import argparse
import fnmatch
import json
import os
import re
import subprocess
import sys

LIVE_STORE_PATH = re.compile(rb"/nix/store/[0-9a-df-np-sv-z]{32}")
errors = []


def fail(msg):
    errors.append(msg)


def readelf(flag, path):
    return subprocess.run(["readelf", "-W", flag, path], check=True, capture_output=True, text=True).stdout


def is_elf(path):
    with open(path, "rb") as f:
        return f.read(4) == b"\x7fELF"


def dynamic(path):
    out = readelf("-d", path)
    return {
        "needed": re.findall(r"\(NEEDED\)\s+Shared library: \[(.*?)\]", out),
        "rpath": re.findall(r"\(RPATH\)\s+Library rpath: \[(.*?)\]", out),
        "runpath": re.findall(r"\(RUNPATH\)\s+Library runpath: \[(.*?)\]", out),
    }


def interpreter(path):
    m = re.search(r"Requesting program interpreter: (\S+)\]", readelf("-l", path))
    return m.group(1) if m else None


def needed_versions(path):
    out = readelf("-V", path)
    block = re.search(r"Version needs section.*?(?=\nVersion (?:definition|symbols) section|\Z)", out, re.S)
    return re.findall(r"Name: (\S+)\s+Flags", block.group(0)) if block else []


def parse_version(text):
    return tuple(int(p) for p in text.split("."))


def check_versions(path, rel, maxima):
    for name in needed_versions(path):
        prefix, _, ver = name.partition("_")
        if name == "GLIBC_PRIVATE":
            fail(f"{rel}: binds to glibc internals ({name})")
        elif prefix in maxima and re.fullmatch(r"\d+(\.\d+)*", ver) and parse_version(ver) > parse_version(maxima[prefix]):
            fail(f"{rel}: needs {name}, above the {prefix}_{maxima[prefix]} the host must provide")


def check_desktop(root, name, desktop_name, icon_name):
    for rel in (f"{name}.desktop", f"usr/share/applications/{name}.desktop"):
        path = os.path.join(root, rel)
        if not os.path.isfile(path):
            fail(f"missing {rel}")
            continue
        entry = dict(l.split("=", 1) for l in open(path).read().splitlines() if "=" in l)
        if entry.get("Type") != "Application" or entry.get("Name") != desktop_name:
            fail(f"{rel}: Type/Name wrong: {entry.get('Type')!r} {entry.get('Name')!r}")
        if entry.get("Exec", "").split()[:1] != [name] or entry.get("Icon") != icon_name:
            fail(f"{rel}: Exec/Icon do not name {name}/{icon_name}: {entry.get('Exec')!r} {entry.get('Icon')!r}")
    icon = os.path.join(root, f"usr/share/icons/hicolor/256x256/apps/{icon_name}.png")
    if not os.path.isfile(icon):
        fail("missing 256x256 icon")
    else:
        head = open(icon, "rb").read(24)
        if head[:8] != b"\x89PNG\r\n\x1a\n" or int.from_bytes(head[16:20], "big") != 256 or int.from_bytes(head[20:24], "big") != 256:
            fail("icon is not a 256x256 PNG")
        for rel in (f"{icon_name}.png", ".DirIcon"):
            top = os.path.join(root, rel)
            if not os.path.exists(top) or open(top, "rb").read() != open(icon, "rb").read():
                fail(f"{rel} does not resolve to the icon")


def check_image(path, expected_name):
    if os.path.basename(path) != expected_name:
        fail(f"AppImage is named {os.path.basename(path)}, expected {expected_name}")
    if not os.access(path, os.X_OK):
        fail("AppImage is not executable")
    with open(path, "rb") as f:
        head = f.read(64)
    if head[:4] != b"\x7fELF" or head[8:11] != b"AI\x02":
        fail("AppImage lacks the ELF header and type 2 magic")
    if int.from_bytes(head[18:20], "little") != 62:
        fail("AppImage runtime is not x86-64")
    if interpreter(path) is not None:
        fail("AppImage runtime needs a dynamic loader; it must be static")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("--policy", required=True)
    ap.add_argument("--appimage")
    ap.add_argument("--appimage-name")
    a = ap.parse_args()
    policy = json.load(open(a.policy))
    name = policy["binary"]
    exe = os.path.join(a.root, "usr/bin", name)
    lib_dir = os.path.join(a.root, "usr/lib")

    if a.appimage:
        check_image(a.appimage, a.appimage_name)

    apprun = os.path.join(a.root, "AppRun")
    if not (os.path.isfile(apprun) and os.access(apprun, os.X_OK) and open(apprun, "rb").read(9) == b"#!/bin/sh"):
        fail("AppRun must be an executable #!/bin/sh script")
    check_desktop(a.root, name, policy["desktopName"], policy["iconName"])

    if not os.path.isfile(exe):
        fail(f"missing usr/bin/{name}")
        return report()

    bundled = sorted(os.listdir(lib_dir)) if os.path.isdir(lib_dir) else []
    objects = [(os.path.join("usr/bin", name), exe)] + [(os.path.join("usr/lib", f), os.path.join(lib_dir, f)) for f in bundled]
    graph = {}
    for rel, path in objects:
        if not is_elf(path):
            fail(f"{rel}: not an ELF file")
            continue
        dyn = dynamic(path)
        is_exe = path == exe
        want_runpath = policy["exeRunpath"] if is_exe else policy["libRunpath"]
        if dyn["rpath"]:
            fail(f"{rel}: has DT_RPATH {dyn['rpath']}")
        if dyn["runpath"] != [want_runpath]:
            fail(f"{rel}: RUNPATH is {dyn['runpath']}, expected [{want_runpath!r}]")
        interp = interpreter(path)
        if is_exe and interp != policy["interpreter"]:
            fail(f"{rel}: interpreter is {interp!r}, expected {policy['interpreter']!r}")
        if not is_exe and interp:
            fail(f"{rel}: bundled library has an interpreter {interp!r}")
        graph[rel] = dyn["needed"]
        for soname in dyn["needed"]:
            bundled_here = soname in bundled
            host = any(fnmatch.fnmatchcase(soname, p) for p in policy["hostLibs"])
            if not (bundled_here or host):
                fail(f"{rel}: needs {soname}, neither bundled nor an allowed host library")
        check_versions(path, rel, policy["maxVersions"])

    for f in bundled:
        if any(fnmatch.fnmatchcase(f, p) for p in policy["hostLibs"]):
            fail(f"usr/lib/{f}: host library must not be bundled")

    reachable, stack = set(), [os.path.join("usr/bin", name)]
    while stack:
        for soname in graph.get(stack.pop(), []):
            rel = os.path.join("usr/lib", soname)
            if soname in bundled and rel not in reachable:
                reachable.add(rel)
                stack.append(rel)
    for f in bundled:
        if os.path.join("usr/lib", f) not in reachable:
            fail(f"usr/lib/{f}: bundled but not needed by anything")

    for root, _, files in os.walk(a.root):
        for f in files:
            p = os.path.join(root, f)
            if os.path.islink(p):
                continue
            hit = LIVE_STORE_PATH.search(open(p, "rb").read())
            if hit:
                fail(f"{os.path.relpath(p, a.root)}: live Nix store path {hit.group(0).decode()}")
    return report()


def report():
    if errors:
        print("AppImage contract violated:", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        sys.exit(1)
    print("AppImage contract holds")


if __name__ == "__main__":
    main()
