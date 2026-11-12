#!/usr/bin/env python3
import bz2
import gzip
import hashlib
import lzma
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
import zlib
from pathlib import Path

MAX_ARCHIVE_DEPTH = 8


def private_sources(paths, explicit_sources):
    fingerprints = set()
    roots = set(map(Path, explicit_sources))
    for root in paths:
        base = Path(root)
        for manifest in (base / "Cargo.toml", base / "crates/odyssey-cv/Cargo.toml"):
            if not manifest.is_file():
                continue
            with manifest.open("rb") as stream:
                if tomllib.load(stream).get("package", {}).get("name") == "odyssey-cv":
                    roots.add(base)
    for base in roots:
        if not base.is_dir():
            raise ValueError(f"Private source unavailable: {base}")
        # shortcut: hashes miss edited source, add format-specific checks before approving new private output families.
        for source in base.rglob("*"):
            if source.is_file() and source.suffix == ".rs":
                data = source.read_bytes()
                if data.strip():
                    fingerprints.add(hashlib.sha256(data).digest())
    return fingerprints


def scan_file(path, fingerprints, forbidden_paths=(), depth=0, public_graph=False):
    try:
        return _scan_file(path, fingerprints, forbidden_paths, depth, public_graph)
    except (ValueError, NotImplementedError, EOFError, OSError, tarfile.TarError, zipfile.BadZipFile, lzma.LZMAError, zlib.error) as error:
        if not public_graph or (isinstance(error, OSError) and error.errno is not None):
            raise
        with open(path, "rb") as stream:
            return hash_stream(stream, fingerprints, forbidden_paths)


def _scan_file(path, fingerprints, forbidden_paths, depth, public_graph):
    if depth >= MAX_ARCHIVE_DEPTH:
        raise ValueError(f"Archive nesting exceeds {MAX_ARCHIVE_DEPTH}: {path}")
    with open(path, "rb") as stream:
        magic = stream.read(8)
        stream.seek(257)
        tar_magic = stream.read(5)
    if magic.startswith((b"7z\xbc\xaf\x27\x1c", b"Rar!\x1a\x07", b"\x1f\x9d", b"LZIP", b"\x04\x22\x4d\x18")):
        raise ValueError(f"Unsupported archive format: {path}")
    if magic.startswith(b"\x28\xb5\x2f\xfd"):
        with tempfile.NamedTemporaryFile() as expanded:
            result = subprocess.run(["zstd", "-q", "-d", "-c", str(path)], stdout=expanded, stderr=subprocess.PIPE)
            if result.returncode:
                raise ValueError(f"Cannot decompress zstd payload {path}: {result.stderr.decode(errors='replace')}")
            return scan_file(expanded.name, fingerprints, forbidden_paths, depth + 1, public_graph)
    if magic.startswith((b"\x1f\x8b", b"BZh", b"\xfd7zXZ\x00")):
        opener = gzip.open if magic.startswith(b"\x1f\x8b") else bz2.open if magic.startswith(b"BZh") else lzma.open
        with opener(path, "rb") as source:
            return scan_stream(source, fingerprints, forbidden_paths, depth + 1, public_graph)
    if magic.startswith(b"PK\x03\x04"):
        with zipfile.ZipFile(path) as archive:
            for item in archive.infolist():
                if item.is_dir():
                    continue
                if item.flag_bits & 1:
                    raise ValueError(f"Encrypted ZIP payload: {path}")
                with archive.open(item) as source:
                    if scan_stream(source, fingerprints, forbidden_paths, depth + 1, public_graph):
                        return True
        return False
    try:
        with tarfile.open(path, mode="r:*") as archive:
            for item in archive.getmembers():
                if not item.isfile():
                    continue
                with archive.extractfile(item) as source:
                    if scan_stream(source, fingerprints, forbidden_paths, depth + 1, public_graph):
                        return True
        return False
    except tarfile.ReadError:
        if tar_magic == b"ustar":
            raise
        with open(path, "rb") as stream:
            return hash_stream(stream, fingerprints, forbidden_paths)


def hash_stream(stream, fingerprints, forbidden_paths):
    digest = hashlib.sha256()
    carry = b""
    carry_size = max((len(path) for path in forbidden_paths), default=1) - 1
    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(chunk)
        sample = carry + chunk
        if any(path in sample for path in forbidden_paths):
            return True
        carry = sample[-carry_size:] if carry_size else b""
    return digest.digest() in fingerprints


def scan_stream(stream, fingerprints, forbidden_paths=(), depth=0, public_graph=False):
    with tempfile.NamedTemporaryFile() as payload:
        shutil.copyfileobj(stream, payload)
        payload.flush()
        return scan_file(payload.name, fingerprints, forbidden_paths, depth, public_graph)



def main():
    payloads, provenance, explicit_source_text, public_text, _end = sys.stdin.read().split("\0")
    roots = [Path(item) for item in provenance.splitlines() if item]
    explicit_sources = [Path(item) for item in explicit_source_text.split()]
    public_paths = set(public_text.splitlines())
    if set(payloads.splitlines()).intersection(map(str, explicit_sources)):
        print("Private source path selected; refusing cache upload", file=sys.stderr)
        return 1
    try:
        fingerprints = private_sources(roots, [source for source in explicit_sources if source.exists()])
    except (OSError, ValueError) as error:
        print(f"Cannot inspect private source provenance: {error}", file=sys.stderr)
        return 1
    for item in payloads.splitlines():
        path = Path(item)
        try:
            if not fingerprints and item not in public_paths and not (path.is_file() and path.stat().st_size == 0):
                print(f"Private source fingerprints unavailable for non-public payload {path}; refusing cache upload", file=sys.stderr)
                return 1
            if path.is_file():
                files = [path]
            elif path.is_dir():
                files = (entry for entry in path.rglob("*") if entry.is_file())
            else:
                continue
            forbidden = tuple(str(source).encode() for source in explicit_sources)
            for file in files:
                if any(file == source for source in explicit_sources):
                    print(f"Private odyssey-cv source path selected for cache: {file}", file=sys.stderr)
                    return 1
                try:
                    if scan_file(file, fingerprints, forbidden, public_graph=item in public_paths):
                        print(f"Private odyssey-cv source payload found in {file}; refusing cache upload", file=sys.stderr)
                        return 1
                except Exception as error:
                    print(f"Cannot inspect cache payload {file}: {error}", file=sys.stderr)
                    return 1
        except OSError as error:
            print(f"Cannot inspect cache path {path}: {error}", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
