#!/usr/bin/env python3
"""Build an offline firmware pack: everything an Odyssey in offline mode needs to update
devices, in one folder, laid out like GitHub so nothing has to be named by hand.

    build-pack.py OUT_DIR [--channel stable|alpha ...] [--device NAME ...] [--latest N]
                          [--manifests DIR] [--repo OWNER/NAME]

For each channel it writes OUT_DIR/releases.<channel>.json, holding the newest N
releases (default 2) of each device from this directory's manifests, and downloads every
file those releases list into OUT_DIR/<tag>/<name>.bin, checking each against the size
and sha256 in the manifest. Both application slots (app-a and app-b) are always
downloaded, because an update writes whichever slot the unit is not running.

The result goes wherever the administrator points Odyssey's `firmware_pack_dir` policy,
by default %ProgramData%\\OdysseyArm\\Odyssey\\firmware-pack. Odyssey itself leaves out
any release whose files are not all there (see crates/odyssey-hub-server/src/firmware/
pack.rs), and this script runs the same completeness check before it finishes.
"""

import argparse
import hashlib
import json
import pathlib
import sys
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
REQUIRED = ("app-a", "app-b")


def tag_for(device: str, entry: dict, key: str, file: dict):
    """Which GitHub release holds a manifest file. Mirrors ReleaseSource::tag.

    None for a net core or bootloader listed without a release number: older entries
    list those for the record only, and the updater never downloads them."""
    if key in ("net-core", "stage2-bootloader"):
        if "release" not in file:
            return None
        kind = "netcore" if key == "net-core" else "bootloader"
        return f"{device}-{kind}-v{file['release']}"
    return f"{device}-v{entry['version']}"


def download(repo: str, tag: str, key: str) -> bytes:
    url = f"https://github.com/{repo}/releases/download/{tag}/{key}.bin"
    request = urllib.request.Request(url, headers={"User-Agent": "odyssey-firmware-pack"})
    with urllib.request.urlopen(request, timeout=120) as response:
        return response.read()


def build(out: pathlib.Path, channels, devices, latest: int, manifests: pathlib.Path, repo: str) -> None:
    out.mkdir(parents=True, exist_ok=True)
    have = {}  # relative path -> sha256, so a file shared by releases is fetched once
    for channel in channels:
        source = json.loads((manifests / f"releases.{channel}.json").read_text(encoding="utf-8"))
        for device in list(source["devices"]):
            if devices and device not in devices:
                del source["devices"][device]
        if not source["devices"]:
            sys.exit(f"no device in the {channel} manifest matches {devices}")
        for device, info in source["devices"].items():
            info["firmwares"] = info["firmwares"][:latest]  # newest first
            for entry in info["firmwares"]:
                missing = [k for k in REQUIRED if k not in entry["files"]]
                if missing:
                    sys.exit(f"{device} {entry['version']} lists no {', '.join(missing)}")
                for key, file in sorted(entry["files"].items()):
                    tag = tag_for(device, entry, key, file)
                    if tag is None:
                        continue
                    relative = f"{tag}/{key}.bin"
                    if relative in have:
                        continue
                    data = download(repo, tag, key)
                    digest = hashlib.sha256(data).hexdigest()
                    if digest != file["sha256"] or len(data) != file["size"]:
                        sys.exit(f"{relative}: expected {file['size']} bytes / {file['sha256']}, "
                                 f"got {len(data)} bytes / {digest}")
                    target = out / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(data)
                    have[relative] = digest
                    print(f"{relative}  ({len(data)} bytes)")
        (out / f"releases.{channel}.json").write_text(json.dumps(source, indent=2) + "\n", encoding="utf-8")

        # The same check Odyssey does when it loads the pack.
        for device, info in source["devices"].items():
            for entry in info["firmwares"]:
                for key, file in entry["files"].items():
                    tag = tag_for(device, entry, key, file)
                    if tag is None:
                        continue
                    path = out / tag / f"{key}.bin"
                    if not path.is_file() or path.stat().st_size != file["size"]:
                        sys.exit(f"incomplete pack: {path} is missing or the wrong size")
    (out / "SHA256SUMS").write_text(
        "".join(f"{digest}  {relative}\n" for relative, digest in sorted(have.items())), encoding="utf-8"
    )
    (out / "README.txt").write_text(
        "Odyssey offline firmware pack.\n\n"
        "Put this folder where the 'firmware_pack_dir' policy points (by default\n"
        "%ProgramData%\\OdysseyArm\\Odyssey\\firmware-pack) and restart Odyssey. Keep the layout\n"
        "as it is; Odyssey only offers a firmware release when every file it lists is here.\n"
        "SHA256SUMS lists the checksum of every firmware file.\n",
        encoding="utf-8",
    )
    print(f"pack written to {out}: {len(have)} firmware files")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out", type=pathlib.Path)
    parser.add_argument("--channel", action="append", choices=["stable", "alpha"], help="default: stable")
    parser.add_argument("--device", action="append", help="only these devices (default: all)")
    parser.add_argument("--latest", type=int, default=2, help="newest releases per device (default 2)")
    parser.add_argument("--manifests", type=pathlib.Path, default=HERE, help="directory holding releases.*.json")
    parser.add_argument("--repo", default="odysseyarm/odyssey")
    args = parser.parse_args()
    build(args.out, args.channel or ["stable"], set(args.device or []), args.latest, args.manifests, args.repo)


if __name__ == "__main__":
    main()
