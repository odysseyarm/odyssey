#!/usr/bin/env python3
"""Prepare what the MSI needs for one build variant.

    prepare_orbbec.py online|offline [--stage DIR] [--archive ZIP] [--eula TXT]

Writes DIR/orbbec.wxs, the WiX fragment `Dioxus.toml` points at, and for the offline
variant also the files it installs. `dx bundle` is run afterwards with these set:

    ODYSSEY_MSI_VARIANT=<variant>
    ODYSSEY_ORBBEC_STAGE=<DIR>      (absolute path; the template reads EULA.rtf from it)

online   A fragment with a hidden, empty feature. The installer carries nothing; the app
         downloads the SDK itself (see src/sdk_install.rs).
offline  Downloads the SDK release in ../orbbec-sdk.json, checks its sha256, unpacks the
         listed files into DIR/orbbec-sdk, fetches the EULA (checked too), turns it into
         the installer's licence page, and writes a fragment that installs it all as an
         optional feature on by default.

The version, URLs, hashes and file list come only from ../orbbec-sdk.json, which the app
reads too. --archive and --eula use local copies instead of downloading, for testing.
"""

import argparse
import hashlib
import json
import pathlib
import re
import shutil
import sys
import urllib.request
import uuid
import zipfile
from xml.sax.saxutils import quoteattr

HERE = pathlib.Path(__file__).resolve().parent
PIN_FILE = HERE.parent / "orbbec-sdk.json"

# Component GUIDs must stay the same from one release to the next or an upgrade treats
# every file as new; deriving them from the install path (inside this fixed namespace)
# makes them stable. Never change this value.
GUID_NAMESPACE = uuid.UUID("5a1c0f4e-2f0f-4a52-9d57-6b3d2e0c2b91")

FEATURE_TITLE = "Orbbec camera support"
FEATURE_DESCRIPTION = (
    "Orbbec's camera software, needed to calibrate a screen with an Orbbec depth camera. "
    "Licensed by Orbbec under the agreement shown during setup."
)


def sha256_of(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fetch(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "odyssey-msi-build"})
    with urllib.request.urlopen(request, timeout=300) as response:
        return response.read()


def verified(data: bytes, expected: str, what: str) -> bytes:
    actual = sha256_of(data)
    if actual.lower() != expected.lower():
        sys.exit(f"{what}: expected sha256 {expected}, got {actual}")
    return data


def install_path(archive_path: str) -> str:
    """Where an archive file lands inside the SDK directory: bin/ is dropped."""
    return archive_path[len("bin/"):] if archive_path.startswith("bin/") else archive_path


def extract(archive: bytes, files: list[str], dest: pathlib.Path) -> list[str]:
    """Writes the listed files under dest and returns their install paths."""
    import io

    zf = zipfile.ZipFile(io.BytesIO(archive))
    # Archives made on Windows store `\` separators.
    index = {name.replace("\\", "/"): name for name in zf.namelist()}
    missing = [f for f in files if f not in index]
    if missing:
        sys.exit("the SDK archive does not contain: " + ", ".join(missing))
    installed = []
    for f in files:
        target = dest / install_path(f)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(zf.read(index[f]))
        installed.append(install_path(f))
    return installed


def rtf_escape(text: str) -> str:
    out = []
    for ch in text.replace("\r\n", "\n"):
        if ch in "\\{}":
            out.append("\\" + ch)
        elif ch == "\n":
            out.append("\\par\n")
        elif ord(ch) < 128:
            out.append(ch)
        else:
            # RTF takes UTF-16 code units as signed 16-bit numbers, `?` as the fallback.
            data = ch.encode("utf-16-le")
            for i in range(0, len(data), 2):
                unit = int.from_bytes(data[i:i + 2], "little")
                out.append("\\u%d?" % (unit - 65536 if unit > 32767 else unit))
    return "".join(out)


def eula_rtf(eula: str) -> str:
    preface = (
        "Orbbec camera support is Orbbec's software, not OdysseyArm's. It is installed only "
        "if you keep \"Orbbec camera support\" ticked on the next page, and it is yours to "
        "use under Orbbec's End User License Agreement, which follows. You can leave the "
        "feature out and add it later from Windows Settings > Apps > Odyssey > Modify."
    )
    return (
        "{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0\\fswiss Segoe UI;}}\\f0\\fs18 "
        + rtf_escape(preface)
        + "\\par\\par\n"
        + rtf_escape(eula)
        + "}"
    )


def wix_id(prefix: str, path: str) -> str:
    """A WiX identifier: letters, digits, `_` and `.`, at most 72 characters."""
    cleaned = re.sub(r"[^A-Za-z0-9_]", "_", path)
    candidate = f"{prefix}_{cleaned}"
    if len(candidate) <= 72:
        return candidate
    return f"{prefix}_{cleaned[:40]}_{hashlib.sha1(path.encode()).hexdigest()[:20]}"


def offline_fragment(stage: pathlib.Path, installed: list[str]) -> str:
    sdk = stage / "orbbec-sdk"
    ids = [wix_id("C", p) for p in installed] + [wix_id("F", p) for p in installed]
    if len(set(ids)) != len(ids):
        sys.exit("two SDK files map to the same WiX id; make wix_id tell them apart")
    # The directory tree, from the install paths.
    tree: dict = {}
    for p in installed:
        node = tree
        for part in p.split("/")[:-1]:
            node = node.setdefault(part, {})

    def directory_xml(name: str, subtree: dict, rel: str, indent: int) -> str:
        pad = "    " * indent
        children = "".join(
            directory_xml(child, sub, f"{rel}/{child}", indent + 1) for child, sub in sorted(subtree.items())
        )
        files_here = [p for p in installed if "/".join(p.split("/")[:-1]) == rel.partition("/")[2]]
        components = "".join(component_xml(p, indent + 1) for p in sorted(files_here))
        return (
            f"{pad}<Directory Id={quoteattr(wix_id('D', rel))} Name={quoteattr(name)}>\n"
            f"{components}{children}{pad}</Directory>\n"
        )

    def component_xml(p: str, indent: int) -> str:
        pad = "    " * indent
        guid = str(uuid.uuid5(GUID_NAMESPACE, p)).upper()
        source = str((sdk / p).resolve())
        return (
            f"{pad}<Component Id={quoteattr(wix_id('C', p))} Guid=\"{guid}\">\n"
            f"{pad}    <File Id={quoteattr(wix_id('F', p))} Name={quoteattr(p.split('/')[-1])} "
            f"Source={quoteattr(source)} KeyPath=\"yes\" />\n"
            f"{pad}</Component>\n"
        )

    root = directory_xml("orbbec-sdk", tree, "orbbec-sdk", 3)
    refs = "".join(
        f"            <ComponentRef Id={quoteattr(wix_id('C', p))} />\n" for p in sorted(installed)
    )
    return f"""<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by prepare_orbbec.py (offline variant). Do not edit. -->
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
    <Fragment>
        <DirectoryRef Id="INSTALLDIR">
{root}        </DirectoryRef>
    </Fragment>
    <Fragment>
        <ComponentGroup Id="OrbbecSdkFiles">
{refs}        </ComponentGroup>
    </Fragment>
    <Fragment>
        <!-- On by default (Level 1): an upgrade offers it again, ticked, so someone who has
             it keeps it. Untick it, or use Modify later, to leave it out. -->
        <Feature
            Id="OrbbecSdk"
            Title={quoteattr(FEATURE_TITLE)}
            Description={quoteattr(FEATURE_DESCRIPTION)}
            Level="1"
            AllowAdvertise="no">
            <ComponentGroupRef Id="OrbbecSdkFiles" />
        </Feature>
    </Fragment>
</Wix>
"""


ONLINE_FRAGMENT = """<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by prepare_orbbec.py (online variant). Do not edit. -->
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
    <Fragment>
        <!-- The template always references this feature. Online installers carry no SDK, so
             it is empty and never offered; the app downloads the SDK itself. -->
        <Feature Id="OrbbecSdk" Title="Orbbec camera support" Level="0" Display="hidden" />
    </Fragment>
</Wix>
"""


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("variant", choices=["online", "offline"])
    parser.add_argument("--stage", type=pathlib.Path, default=HERE.parent / "target" / "orbbec-stage")
    parser.add_argument("--archive", type=pathlib.Path, help="use this SDK zip instead of downloading it")
    parser.add_argument("--eula", type=pathlib.Path, help="use this EULA text instead of downloading it")
    args = parser.parse_args()

    stage = args.stage.resolve()
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)

    if args.variant == "online":
        (stage / "orbbec.wxs").write_text(ONLINE_FRAGMENT, encoding="utf-8")
        print(f"online: wrote {stage / 'orbbec.wxs'}")
        return

    pin = json.loads(PIN_FILE.read_text(encoding="utf-8"))
    archive_info = pin["windows_x64"]
    archive = args.archive.read_bytes() if args.archive else fetch(archive_info["url"])
    verified(archive, archive_info["sha256"], "the SDK archive")
    eula = args.eula.read_bytes() if args.eula else fetch(pin["eula"]["url"])
    verified(eula, pin["eula"]["sha256"], "the Orbbec EULA")

    installed = extract(archive, archive_info["files"], stage / "orbbec-sdk")
    # The EULA travels with the SDK, and its text is the installer's licence page.
    (stage / "orbbec-sdk" / "EULA.txt").write_bytes(eula)
    installed.append("EULA.txt")
    text = eula.decode("utf-8")
    (stage / "EULA.rtf").write_text(eula_rtf(text), encoding="ascii")
    (stage / "orbbec.wxs").write_text(offline_fragment(stage, installed), encoding="utf-8")
    print(f"offline: SDK {pin['version']}, {len(installed)} files, staged in {stage}")


if __name__ == "__main__":
    main()
