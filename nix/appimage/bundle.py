#!/usr/bin/env python3
"""Turn the Nix-built Odyssey desktop binary into a host-portable AppDir.

The Nix binary is linked against the Nix glibc loader and RPATHs into /nix/store.
This rewrites it so that, on a machine without Nix, it behaves like the binary
cargo-packager used to produce on a stock distro:

  * PT_INTERP is the distro loader (/lib64/ld-linux-x86-64.so.2), so glibc,
    libm, libgcc_s and the loader come from the host and host graphics drivers
    loaded by GPUI resolve against the same libc they were built for.
  * Every other shared library in the closure is copied to usr/lib and found
    through RUNPATH ($ORIGIN/../lib for the binary, $ORIGIN for the libraries).
  * Libraries matching policy.hostLibs (glibc family, GPUI's Wayland/xkbcommon/
    Vulkan/EGL/GL stack, ...) are never bundled and never traversed.
  * Any remaining /nix/store reference in the payload is cut, so nothing points
    at a path that does not exist on the consumer machine.

verify.py is the independent check of the result.
"""
import argparse
import fnmatch
import json
import os
import re
import shutil
import subprocess
import sys

# Nix store hashes use the base32 alphabet without e, o, t, u. Crane's own
# reference stripping writes 32 'e', which therefore never matches this.
LIVE_STORE_PATH = re.compile(rb"/nix/store/([0-9a-df-np-sv-z]{32})(?=-)")
# OPENSSLDIR is compiled into libcrypto; the Nix one is meaningless elsewhere,
# so point it at the conventional location (openssl-probe still overrides it).
OPENSSL_DIR = re.compile(rb"/nix/store/[0-9a-df-np-sv-z]{32}-openssl[^/\x00\"\s]*/etc/ssl")


def run(*cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def is_elf(path):
    with open(path, "rb") as f:
        return f.read(4) == b"\x7fELF"


def needed(path):
    return run("patchelf", "--print-needed", path).split()


def rpath_dirs(path):
    origin = os.path.dirname(os.path.realpath(path))
    raw = run("patchelf", "--print-rpath", path).strip()
    return [d.replace("${ORIGIN}", origin).replace("$ORIGIN", origin) for d in raw.split(":") if d]


def is_host(soname, patterns):
    return any(fnmatch.fnmatchcase(soname, p) for p in patterns)


def resolve(soname, requester, root_dirs, search):
    for d in rpath_dirs(requester) + root_dirs + search:
        candidate = os.path.join(d, soname)
        if os.path.isfile(candidate):
            return os.path.realpath(candidate)
    sys.exit(f"bundle: cannot resolve {soname} needed by {requester}")


def sanitize(path):
    with open(path, "rb") as f:
        data = f.read()

    def openssl(m):
        return b"/etc/ssl".ljust(len(m.group(0)), b"/")

    out = OPENSSL_DIR.sub(openssl, data)
    seen = {m.group(0) for m in LIVE_STORE_PATH.finditer(out)}
    out = LIVE_STORE_PATH.sub(lambda m: b"/nix/store/" + b"e" * 32, out)
    if out != data:
        mode = os.stat(path).st_mode
        with open(path, "wb") as f:
            f.write(out)
        os.chmod(path, mode)
        print(f"bundle: cut {len(seen)} store reference(s) in {os.path.relpath(path)}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--appdir", required=True)
    ap.add_argument("--policy", required=True)
    ap.add_argument("--search", default="")
    a = ap.parse_args()

    policy = json.load(open(a.policy))
    name = policy["binary"]
    search = [d for d in a.search.split(":") if d]
    bin_dir = os.path.join(a.appdir, "usr", "bin")
    lib_dir = os.path.join(a.appdir, "usr", "lib")
    os.makedirs(bin_dir, exist_ok=True)
    os.makedirs(lib_dir, exist_ok=True)

    exe = os.path.join(bin_dir, name)
    shutil.copyfile(a.exe, exe)
    os.chmod(exe, 0o755)
    root_dirs = rpath_dirs(a.exe)

    # Walk the closure over the original (Nix) objects: their RUNPATHs are what
    # locate each dependency, and they are rewritten only after the walk.
    bundled = {}
    queue = [a.exe]
    while queue:
        obj = queue.pop()
        for soname in needed(obj):
            if is_host(soname, policy["hostLibs"]) or soname in bundled:
                continue
            real = resolve(soname, obj, root_dirs, search)
            dest = os.path.join(lib_dir, soname)
            shutil.copyfile(real, dest)
            os.chmod(dest, 0o644)
            bundled[soname] = real
            queue.append(real)

    for soname in bundled:
        run("patchelf", "--set-rpath", "$ORIGIN", os.path.join(lib_dir, soname))
    run("patchelf", "--set-interpreter", policy["interpreter"], "--set-rpath", "$ORIGIN/../lib", exe)

    for root, _, files in os.walk(os.path.join(a.appdir, "usr")):
        for f in files:
            p = os.path.join(root, f)
            if not os.path.islink(p) and is_elf(p):
                sanitize(p)

    print(f"bundle: {len(bundled)} librar{'y' if len(bundled) == 1 else 'ies'} bundled: {' '.join(sorted(bundled))}")


if __name__ == "__main__":
    main()
