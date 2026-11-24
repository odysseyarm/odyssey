# Firmware manifests and releases

Hosts (Odyssey Desktop) learn what firmware exists from one JSON manifest per
channel and download the images from GitHub releases.

| File | Who follows it | How it changes |
|---|---|---|
| `releases.nightly.json` | Users who chose Nightly in Settings | `cargo xtask firmware <board> gen-release` adds an entry |
| `releases.stable.json` | Everyone else (the default) | `cargo xtask firmware promote` copies an entry from nightly |

Nightly used to be called alpha. A saved `alpha` choice reads as nightly, and
`--channel alpha` still works in xtask. Older Desktops request
`releases.alpha.json`, which no longer exists, so they see no firmware updates
until they are updated. There is no beta tier. An entry is nightly until
somebody promotes it to stable.

Hosts parse a manifest whole or not at all, and take the **first** entry they
can install, so entries are newest-first and one malformed entry hides every
update. `cargo nextest run -p odyssey-hub-server --lib -E 'test(firmware)'`
parses both files.

Both files use manifest schema 2. `requires_bootloader` is a SemVer string, and
`0.0.0` means no minimum. Stage2 bootloader file entries pin a SemVer `version`,
while net-core entries keep numeric `release` counters. Schema 2 requires an
updated Desktop, so publish that Desktop before these manifests. Hosts enforcing
schema 1 reject them. Offline packs also need schema 2 manifests.

## Version text the hub reads

Firmware reports its application version in USB string descriptor 4. The hub
accepts exactly two forms and turns them into `FirmwareVersion`
(`odyssey-hub-common`), a fixed-size value that crosses the C ABI without a
string buffer and orders by SemVer precedence:

- `X.Y.Z`, Stable.
- `X.Y.Z-nightly.YYYYMMDD.tHHMMSS.g<12 lowercase hex>`, Nightly, as
  `desktop-release.yml` stamps with `git rev-parse --short=12`.

Anything else reads as "no version reported". That includes build metadata
(`+...`) and other pre-release tags. A device that reports a version is compared
against a manifest entry only when the entry's version parses the same way.

## Releasing everything that changed

Three commands cover Desktop, the SDKs, the dongle, the marker board and both
ATS Lites with their net cores and bootloaders. The sections below describe the
steps they run.

```
cargo xtask release plan      # what changed since each tag; reads only
cargo xtask release bump      # prepare bootloader/app patch versions and net-core counters
cargo xtask release publish   # tag merged master commits and publish
```

**Changed** means a file differs between the component's newest tag and HEAD in
its crate, a path dependency it builds with, its lock file or its memory map.
That errs toward bumping. A comment in a shared crate counts.

`bump` uses `cargo-release` to patch-bump apps and bootloaders, and net-core
counters remain source macros. Commit the changes on a release branch, open a
pull request, get it reviewed, pass the required checks, and squash-merge before
publishing. A raised net core also raises the application that delivers it.
Bootloaders are raised only by name
(`cargo xtask release bump atslite1-bootloader`), because a bootloader release
reflashes every unit's bootloader. Name components to bump only those.

`publish` needs a clean tree and validates its commit against remote `master`
through GitHub. After the PR is squash-merged, fetch master and publish from its
merged SHA. Final release tags point to merged `master` SHAs, never to
pre-squash branch SHAs. It tags net cores and bootloaders and waits for their
workflows, then builds and publishes ATS Lite application assets and tags the
remaining selected components. It generates ATS Lite and dongle nightly entries
locally from the published assets. Commit each manifest change on a branch and
submit it as a separate PR. Name components to publish only those, and
`--dry-run` prints the tags.

`min_desktop` is the oldest Desktop release from which every later one speaks
the firmware's wire version (`DEVICE_PROTOCOL`, or `MUX_PROTOCOL` for the
dongle), and it never goes below the device's previous entry.
`requires_bootloader` and `requires_netcore` carry over from the previous entry.
Raise one with `gen-release` by hand.

Generated ATS Lite, dongle, and marker nightly entries need their own PRs after
the assets are published. Each one is reviewed, passes the required checks and
is squash-merged before units are offered its release. The marker workflow opens
its PR itself and explicitly dispatches the required checks.

The bump and publish decisions come from the
[`pure-release`](https://github.com/Abrahamh08/pure-release) dependency. Component
mapping, dependency discovery and publishing I/O remain in `crates/xtask/src/release.rs`.
Host CI runs Verus and Kani against the dependency revision in `crates/xtask/Cargo.lock`:

```
nix build .#checks.x86_64-linux.release-proofs .#checks.x86_64-linux.wire-proofs
```

Verus verifies the seven release properties for arbitrary component-table sizes
and compatibility-history lengths. Its strict gate rejects unproved assumptions
and skipped executable bodies. Kani independently checks every valid table and
state combination at 11 components,
and compatibility histories of zero through five releases. Both cover arbitrary
`u64` version triples:

- changed components are never current, and source-version regressions are refused
- outgoing parts force their carrier application to become releasable
- one automatic bump reaches a fixed point, and default publication accepts its
  ready components, ordered parts before applications, without duplicates
- named publication requires a ready carrier but does not publish it unless
  selected, while default publication selects every ready carrier
- net-core counter increments are monotonic and reject exhaustion at 65535
- the Desktop floor is the oldest compatible suffix

Wire proofs call production codecs against fixed byte layouts, which is a
stronger check than an encode/decode roundtrip. Coverage includes legacy object
records, object and marker reports, packet headers, current marker GATT writes
and device-version CBOR encoding plus fixed-width unsigned-16-bit CBOR decoding.
Legacy proofs use `std`. Marker proofs also run without default features. Marker
GATT input lengths are bounded to 0..=3, and other payloads have fixed protocol
sizes. The dependency-free version crate proves decimal BCD majors 0..=99 and
reversible legacy counter identities 1..=5, and it refuses all other u16
counters and u32 triples.

Fixed payload proofs use fixed buffers and do not model heap allocation. An
arbitrary checked position covers every byte or suffix position without
repeating the same property in an unrolled loop. The Desktop-floor proof uses
Kissat. Solver selection is per harness, based on measured verification time.

These checks do not infer whether an arbitrary source change needs a major
version bump. Uncovered packet payloads, transport behavior, semantic changes,
GitHub responses, dependency discovery and real hardware remain outside the
proofs. A successful proof does not guarantee that an entire release is safe.

## Components and tags

Each image is released under its own tag. A manifest entry for an application
release pins the exact component releases it was tested with.

| Component | Tag | Version is | Assets |
|---|---|---|---|
| Application | `<device>-v<semver>` | Cargo version | `app-a.bin`, `app-b.bin` |
| Stage2 bootloader | `<device>-bootloader-v<semver>` | bootloader Cargo version | `stage2-bootloader.{bin,hex,elf}` |
| Net core | `<device>-netcore-v<N>` | counter `N` in `firmware/<board>-net-core/src/main.rs` | `net-core.{bin,hex,elf}` |
| Marker application | `marker-control-board-v<semver>` | Cargo version | `app.{bin,hex,elf}`, `merged.hex`, factory `bootloader.{hex,elf}` |
| Marker factory bootloader | `marker-control-board-bootloader-v<semver>` | bootloader Cargo version | `bootloader.{bin,hex,elf}` (SWD only) |

`<device>` is the manifest device key: `atslite1` (board `lite1`) or
`legacy-atslite1` (board `legacy-atslite`). Run every command from the
repository root.

`marker-control-board` is both the board name and the manifest device key. Its
nRF52833 has one application image, no net core, and one USB DFU bootloader.
Manifest `files` contains only `app`. New releases require bootloader `"1.0.0"`
and `requires_netcore` is `0`. Older entries keep their historical floor. Never add `app-a`, `app-b`, `net-core`, or
`stage2-bootloader` to this device. Its bootloader cannot update itself through
DFU.

`dongle` entries are installed by Odyssey Desktop 0.7.5 and newer. The dongle
keeps its maker's bootloader (Fanstel or Laird, Nordic serial DFU), so `files`
contains both `dfu-fanstel` and `dfu-laird`, `requires_bootloader` is `"0.0.0"`
and `requires_netcore` is `0`. Assets are named
`dongle-fw-<semver>-dfu-<maker>.zip`. Desktop checks size and hash, tries the
signed init packets before writing an application, and selects the package the
bootloader accepts. It follows the same physical USB port through the bootloader
and the reconnect, and it confirms the full dongle identity and reported
firmware version before showing success. Offline packs must carry both ZIPs.
After publication, run `cargo xtask dongle gen-release <min_desktop>`.

A bootloader's firmware version comes from its Cargo package SemVer and is
reported through USB descriptor 4, and `bcdDevice` carries its decimal major
version. Application `ODYSSEY_BUILD_VERSION` overrides never change bootloader
identity. Versions `0.0.1` through `0.0.5` are reserved aliases for historical
counter releases.

Bootloader major versions mark breaking update-interface changes, minor versions
add compatible features and patch versions fix bugs. Manifest minimum versions
remain explicit compatibility claims. SemVer alone does not prove flash-layout
compatibility, or that a bootloader can install the next release.


Both ATS Lite stage2 bootloaders validate each bank's initial stack pointer and
Thumb reset vector before jumping. Pending banks get two boot attempts before
rollback. If neither bank is valid, bootloader enters USB DFU recovery; it never
jumps into invalid flash.

## Releasing a bootloader

1. Patch-bump `firmware/<board>-bootloader/Cargo.toml` and merge to `master`.
   `cargo xtask release bump <component>` does this for selected bootloaders.
2. Publish it, either way:
   - **Locally:** `cargo xtask firmware <board> release-bootloader`
     (`DRY_RUN=1` runs the checks and stops). It builds, and it refuses a dirty
     tree, a commit that is not on `origin/master`, or an existing tag. Then it
     creates the release as a draft and publishes it last.
   - **CI:** push `<device>-bootloader-v<semver>`. The workflow checks the
     complete tag version against Cargo.toml, builds with `cargo xtask firmware
     <board> dist-bootloader`, then publishes. An existing release is left
     untouched.
3. Add it to a firmware release (below). `gen-release` hashes the **published**
   asset. A local build will not do, because builds made elsewhere are not
   bit-identical.

Do not delete a published bootloader release. Manifest entries point at it.

## Releasing a net core

The net core (the nRF5340 BLE controller) is released on its own under
`<device>-netcore-v<N>`, with assets `net-core.{bin,hex,elf}`.

1. Raise the counter in `firmware/<board>-net-core/src/main.rs` and merge to
   `master`. The counter is the literal in
   `macro_rules! release { () => { N }; }`. Release tooling reads it through
   `cargo xtask firmware release-counter`, which ignores layout, so rustfmt may
   format the macro as it likes. The counter is `1..=65535`. `0` means "unknown"
   and is never released.
2. Publish it, either way:
   - **Locally:** `cargo xtask firmware <board> release-netcore`
     (`DRY_RUN=1` runs the checks and stops). It has the same checks and
     draft-then-publish order as `release-bootloader`.
   - **CI:** push the tag `<device>-netcore-v<N>`. The *Net Core Release*
     workflow checks the tag against the source, builds with the same
     `cargo xtask firmware <board> dist-netcore` and publishes. It stands down
     if a release for the tag already exists.
3. Add it to a firmware release (below). `gen-release` hashes the **published**
   `net-core.bin`, never a local build, and records `release` on the `net-core`
   file entry.

The device reports the counter itself, so a host reads it back to verify the
copy. A net core that fails to install simply does not answer. Nothing reports
the net core while the device is in the stage2 bootloader, and a net core update
leaves the device there until an application is written, so the read-back only
happens once the new application is running. Reading it needs device protocol
0.1.5 or newer. Older firmware cannot decode the request, so a host must not ask
it, and it treats such a device's net core as unknown.

Do not delete a published net-core release. Manifest entries point at it.

### Bridge entries

An entry's own `net-core` release never counts toward its own
`requires_netcore`. A net-core bump therefore ships through a *bridge*: an
application entry that carries the newer net core and whose `requires_netcore`
is only what is deployed today (`0` for the first migration).

To move to net core N+1:

1. Release net core N+1 (above).
2. Publish a bridge application entry carrying it, with `requires_netcore` set
   to the release currently deployed.
3. Only a later entry may set `requires_netcore` to N+1, and only while the
   bridge is still the newest entry. `gen-release` refuses a floor above the net
   core carried by the current newest entry.

The last rule exists because hosts are greedy. A unit installs the newest entry
it is allowed to. If a newer entry that needs no net core overtakes the bridge,
the bridge is never taken, the unit never receives N+1, and it is stranded below
the entry that requires it. The host tests walk every starting point through
both committed manifests and fail unless each unit reaches the newest entry.

Units reach later entries in two hops. First comes the bridge, which installs
N+1. The later application follows once N+1 is reported as running.

## Releasing application firmware

Build the application, publish its immutable assets, then generate the nightly
entry from those published bytes:

```
cargo xtask firmware <board> build
cargo xtask firmware <board> gen-release <requires_bootloader> <requires_netcore> <min_desktop>
```

- `requires_bootloader`: the minimum bootloader SemVer this application can run
  on. `"0.0.0"` means no floor. Hosts compare full SemVer precedence and require
  the reported bootloader version to meet the floor after any same-session
  update. A matching SemVer major does not guarantee compatibility with a later
  bootloader release, so set this floor from explicit compatibility knowledge.
  `"0.0.0"` does not promise compatibility with every future bootloader version.

  Newly generated modern Lite1 application entries require stage2 `2.0.0` or
  later for secure boot. Historical entries retain their original floors.
  Newly generated legacy ATS Lite entries retain the signed-DFU floor `1.0.0`.
  The modern-layout cutover requires paired SWD provisioning; do not ship its
  stage2 as a standalone upgrade to deployed units.
- `requires_netcore`: the net-core release the unit must already be **running**,
  as it reports, before this entry may be installed. `0` for none. The host
  checks it against the net core the unit runs now and never against the one
  this update installs, because nothing can verify a written net core until the
  new application runs. A unit that cannot report its net core counts as unknown
  and does not satisfy a floor above 0. The value must be an integer no higher
  than the highest net-core release carried by an entry already published for
  the device. `gen-release` refuses otherwise.
- `min_desktop`: the oldest Odyssey Desktop this firmware is required and tested
  with. It is not automatically the desktop shipping alongside. Coupling a
  hotfix to a desktop upgrade it does not need strands users. Older desktops are
  told a newer firmware exists and where to get the desktop.

None has a default. Leaving one out would silently mean "any", so the command
and the manifest parser both refuse.

`gen-release` refuses a changed bootloader image unless its Cargo SemVer is
above the newest version, and it refuses a version reused for another image.
Net-core images keep their counter checks: a changed image needs a higher
counter, counters cannot be reused, and one image cannot use multiple counters.
Historical bootloader versions `0.0.1` through `0.0.5` resolve to the existing
`-bootloader-v1` through `-bootloader-v5` tags. No historical release assets or
tags are rewritten.

After publishing application assets, generate the nightly entry from those
immutable bytes and submit a separate manifest PR. Require review, required
checks, and squash merge before units are offered the release. Promote it once
it has run on nightly units:

```
cargo xtask firmware promote <device> <version>
```

`promote` checks before it writes anything. A version that is not newer than
stable's current newest for the device is refused. So is an entry whose
`requires_netcore` is above the net core stable head carries, or whose
bootloader floor is above the bootloader stable head carries. Promote bridge
entries before the entries that require those components, because hosts install
stable's head.

The xtask unit tests (`cargo nextest run --manifest-path crates/xtask/Cargo.toml`)
run these refusals against synthetic manifests and leave committed manifests alone.

### Marker control board

Initial installation requires SWD, because old upstream firmware has no USB DFU
loader. `cargo xtask firmware marker-control-board build` creates
`build/merged.hex` for factory programming and `build/app.{bin,hex,elf}` for
application releases. `check` checks both crates, and `dist-bootloader` produces
independent factory artifacts. `flash`, `flash-chiperase`, `flash-recover`, and
`flash-app` use `nRF52833_xxAA`. Full sector flashing resets boot state from
`firmware/memory/memory-marker-control-board.kdl` and never touches nRF5340
state addresses. Probe commands write hardware. Invoke them only with the device
owner's approval.

Set `[package.metadata.firmware] min-desktop` in the application Cargo.toml to
the oldest tested Desktop version. Marker DFU-only discovery and single-image
updates require **0.7.3+**. Desktop lists markers without tracker controls and
verifies the application version from USB `bcdDevice` after updating. Submit the
application version and compatibility floor for review and squash-merge before
publishing. The final release tag must point to the merged `master` SHA, never
to a pre-squash branch SHA.

*Marker Control Board Release* builds the application plus factory images and
publishes immutable GitHub assets. It then downloads the published `app.bin`,
computes SHA256 and size, and opens a PR adding a newest-first nightly entry
onto latest master. The generated entry describes the immutable published bytes.
Existing releases are not overwritten. Reruns accept identical manifest entries,
reject changed hashes, and reuse an open matching PR without replacing branch
contents. A branch without an open PR causes refusal. The PR receives explicit
host-ci, fmt, and tracking-regression workflow dispatches, because token-created
PRs do not trigger `pull_request` checks.

A manual *Marker Control Board Release* dispatch builds and uploads artifacts
only. The factory bootloader release uses the existing *Bootloader Release*
workflow with `marker-control-board-bootloader-v1`. No separate component
release is required before application publication. Factory images install
through SWD only.

Local manifest generation runs **after** publishing application assets:

```
cargo xtask firmware marker-control-board gen-release 1.0.0 0 <min_desktop>
cargo xtask firmware promote marker-control-board <version>
```

Unlike the dual-bank recipes, marker `gen-release` hashes the published
`app.bin` directly, never a local build or unpublished components.
Offline packs accept either a single `app` or the complete `app-a`/`app-b`
layout, and they refuse mixed or incomplete layouts. Single-image entries cannot
carry net-core/bootloader files or a nonzero net-core requirement. Shared asset
paths must agree on SHA-256 and size across selected entries.
Checksums and relative paths are validated before packing.

## Signed releases

Modern ATS Lite, legacy ATS Lite and marker applications require Ed25519 signatures
before activating OTA updates. The message format, trusted keys and
verifier live in `firmware/release-signing`, shared by firmware and `xtask`.

**Message.** 67 bytes, little-endian integers:

| Bytes | Content |
|---|---|
| 18 | `ODYSSEY-FW-SIG-V1\0` |
| 1 | component id: 0 lite1 app, 1 lite1 net core, 2 legacy-atslite app, 3 legacy-atslite net core, 4 marker app, 5 dongle app, 6 lite1 stage2, 7 legacy-atslite stage2 |
| 4 | image size in bytes (`u32`) |
| 12 | version as three `u32`: the application `major, minor, patch`, or `release, 0, 0` for a net core |
| 32 | SHA-256 of the image |

The signature is the 64-byte pure Ed25519 signature of that message. The
component id keeps a signed image from being moved to another product or role,
and the version keeps an old signed image from passing as a new one. An
application's version is the manifest entry's `version` without pre-release
and build parts. Stage2 uses its own SemVer triple and separate component id.
The dongle's `dfu-*` ZIPs use Nordic DFU signatures rather than this envelope.

**Signing.** `cargo xtask firmware sign <component> <image> --version <v>
--signer <signer>` prints the signature as 128 hex characters. Components are
`lite1-app`, `lite1-netcore`, `legacy-atslite-app`, `legacy-atslite-netcore`,
`lite1-bootloader`, `legacy-atslite-bootloader`, `marker-app` and `dongle-app`;
a net core's version is its release counter.
`--signer dev` signs with the development seed. Anything else is a command,
split on whitespace without quoting, that reads the message on stdin and writes
the 64 raw signature bytes to stdout. `xtask` verifies each signature against
the keys in `release-signing` before it prints or stores it, so a signer whose
public key is not installed (below) is refused.

Set `ODYSSEY_SIGNER` to a production-key signer before `gen-release` (or
`release publish`) for either ATS Lite or marker. Missing signers, untrusted
signatures and development-key signatures fail before the manifest is written.
Signed file entries are `app`, `app-a`, `app-b`, `net-core` and `stage2-bootloader`.

**Modern Lite1 threshold policy.** Trust policy v2 has up to four active image
keys and four root slots. Empty root slots use 64 zero hex characters. Create
unsigned record, then pass it between root holders; each holder appends a
signature over the same payload. No root private key needs to be present on
the assembly machine:

```
cargo xtask firmware trust-list create --generation 1 \
  --key <image-key-hex> \
  --root <root-a-hex> --root <root-b-hex> --root <root-c-hex> \
  --root 0000000000000000000000000000000000000000000000000000000000000000 \
  --threshold 2 --output policy.payload
```
```
cargo xtask firmware trust-list sign --input policy.payload --output policy.record \
  --public-key <root-a-hex> --signer "python3 firmware/release-signing/openpgp-sign.py --device /dev/hidrawN"
cargo xtask firmware trust-list sign --input policy.record --output policy.record \
  --public-key <root-b-hex> --signer "<root-b signer command>"
cargo xtask firmware trust-list wrap --input policy.record --output policy.dfu \
  --public-key <root-a-hex> --signer "python3 firmware/release-signing/openpgp-sign.py --device /dev/hidrawN"
python3 firmware/release-signing/openpgp-sign.py --self-test
```

`sign` appends signer entry and verifies explicit public key. Feed output to
next signer; keep root private keys with their owners. `wrap` signs transport
envelope with one current root.

**OpenPGP signer.** `openpgp-sign.py` uses signature slot 1 on Ledger over
Linux HID (`--device /dev/hidrawN`) or a PC/SC card such as Token2
(`--reader INDEX`). Both need Python with `cryptography`; PC/SC additionally
needs `pyscard` and a running PC/SC service. List reader indices with that
interpreter:

```
python3 -c 'from smartcard.System import readers; print(*enumerate(readers()), sep="\n")'
ODYSSEY_SIGNER="python3 firmware/release-signing/openpgp-sign.py --reader 0" \
  cargo xtask firmware <board> gen-release ...
```

Reader indices can change when devices reconnect; check the list before signing.
Ledger keeps PIN entry on-device. PC/SC asks for the OpenPGP user PIN on the
terminal without echo and opens an exclusive, reset-on-disconnect session.
Neither transport retries failed PIN or signing commands. Configure the card's
touch policy during provisioning; the signer does not enable or enforce it.

The PC/SC path assumes a provisioned Ed25519 OpenPGP signing key and standard
OpenPGP APDUs; Token2 hardware has not been tested. Returned signatures must
verify as plain Ed25519 over the original message before anything is written to
stdout. `xtask` separately verifies authorization against trusted public keys.
`--self-test` checks HID framing, TLV parsing, PIN lengths, APDU errors and
signature verification without hardware.

Factory generation 1 must preserve stage1 immutable root slots and threshold
exactly. Production stage1 uses threshold 2. Assemble command accepts raw signed
policy record; it checks root authorization and exact factory baseline before
placing erased 132-byte transport prefix plus record in `trust_a`. Trust policy
record authorizes policy independently of outer transport header. Release
pipeline keeps `ODYSSEY_LITE1_TRUST_DFU` input path; point it at reviewed
generation-1 signed record.

```
cargo xtask firmware assemble-lite1 \
  --stage1 build/stage1-bootloader.hex \
  --stage1-netboot-hash build/stage1-netcore-bootloader.sha256 \
  --netboot build/netcore-bootloader.bin --trust policy.record \
  --stage2 stage2.dfu --app-a app-a.dfu --app-b app-b.dfu --net-core net-core.dfu \
  --output merged.hex --trace merged.trace
```

Ordinary policy updates require current root threshold signatures. Root
rotation requires both current-root threshold and new-root threshold signatures.
Stage1 anchors are immutable: changing initial baseline requires new factory
stage1 firmware and newly manufactured/provisioned units. After the first boot
stage1 keeps the accepted roots in locked flash, so root rotations are not limited.
A revoked root counts for nothing in any later policy.

`assemble-lite1` verifies production signatures, signed component identifiers,
exact payload lengths and SHA-256 before writing. App, stage2 and net-core slots
contain signed header, 380 erased bytes, then payload; net-core capacity excludes
its final 4 KiB state page. Netboot padded region remains checked against stage1
embedded hash. Trace records each component version and SHA-256.

**YubiKey signer.** Generate an Ed25519 key in PIV slot 9c (needs YubiKey
firmware 5.7 or newer; OpenPGP's `gpg --detach-sign` wraps the signature in a
PGP packet and is not raw Ed25519, so it does not work here), then sign with
`firmware/release-signing/piv-sign.py`. It needs Python with
`yubikey-manager`, asks for the PIV PIN on the terminal and prints only the 64
signature bytes:

```
ykman piv keys generate --algorithm ED25519 --pin-policy ALWAYS --touch-policy ALWAYS 9c pub.pem
ODYSSEY_SIGNER="python3 firmware/release-signing/piv-sign.py" \
  cargo xtask firmware <board> gen-release ...
```

The signer command is split on whitespace, so `python3` must be an interpreter
that can import `yubikit`. Every signature needs the PIN and a touch.

**Key slots.** `RELEASE_KEYS` in `firmware/release-signing/src/keys.rs` supplies
the initial root slots: 0 `matthew@ampotato`, 1 `yubikey@kybrdgmr`, 2 the Ledger
Flex key, and 3 empty (all zero), reserved for a second YubiKey. Stage1 embeds
that initial set, not every future set. A dual-threshold root rotation can fill
the empty slot or replace a root on already-provisioned units.

Everyday firmware needs one signature from an active image key; trust-policy
changes need the current root threshold. To obtain a raw YubiKey public key:

```
ykman piv keys export 9c - | python3 -c 'import sys,base64;print(base64.b64decode("".join(l for l in sys.stdin if "-----" not in l))[-32:].hex())'
```

The last 32 bytes of the DER encoding are the key. An all-zero slot never
matches. Builds with the `dev-signing-key` feature also trust a development key
whose seed is in `crates/xtask/src/firmware/signing.rs`; the seed is public, so
such builds accept signatures anyone can make and must never be released.
Optimized firmware builds with that feature require `ODYSSEY_DEV_SIGNING=1`;
use it only for firmware you flash yourself. `--signer dev` is for the explicit
development signing command, not release generation.

**Custody plan.** Production starts with three independent roots and threshold
2. Keep roots on separate devices and retain the Ledger seed backup offline.
One lost root can be replaced using the other two; losing two roots blocks
policy changes until destructive SWD recovery. Everyday image keys are managed
separately: first add a replacement through a threshold-signed policy, install
images signed by it, then remove the old image key through another policy.
Root replacement additionally requires the proposed set's threshold signatures.

**Enforcement.** Supported ATS Lite and marker applications and bootloaders
enable verification through dependency features; `--no-default-features` cannot
disable it. On nRF5340, the `oberon`
feature uses CryptoCell CC312/PKA for Ed25519 and Oberon for its SHA-512.
Image digests remain SHA-256 (Oberon on nRF5340, `sha2` on the marker); the
67-byte signed message and signature format are unchanged. Marker and host
Ed25519 verification still use `salty`.
Legacy ATS Lite and pre-secure-boot Lite1 bootloader 1.0.0 require signed DFU
envelopes for every exposed target. Modern Lite1 stage2 2.0.0 authenticates
fixed signed slots and trust records; earlier bootloaders retain raw-image
transfers.

**USB DFU.** The 132-byte prefix is the 67-byte message above, its 64-byte
signature and one zero byte. Original image bytes follow, with no transport
padding. Header authentication precedes flash writes; exact image size and hash
must match before activation. App vector commit is deferred until verification,
so interrupted or rejected ATS downloads cannot become bootable fallback banks.

Desktop wraps manifest signatures according to the running bootloader version.
For standalone recovery, use the production signer:

```sh
cargo xtask firmware sign lite1-app app-a.bin --version 0.1.24 --output app-a.dfu
dfu-util -d 1915:5221 -S <device-serial> -a 0 -D app-a.dfu
```

Select the image matching the bootloader's advertised APP_A/APP_B slot; slot
IDs can change after an update. Bootloader and net-core signing names are
`lite1-bootloader`, `legacy-atslite-bootloader`, `lite1-netcore` and
`legacy-atslite-netcore`; marker uses `marker-app`. `--signer dev` is only for
explicit development-key firmware and is never accepted by production builds.

This is authenticated installation, not secure boot. Installed banks are not
authenticated on every reset, SWD remains unrestricted, and general rollback
protection is not implemented. Marker bootloader upgrades require SWD; preserve
owner-bond pages and test signed recovery before locking debug access.

Build nRF5340 firmware inside `nix develop .#firmware`; it pins matching public
TF-PSA-Crypto headers and the ARM C compiler. The SDK's hash callbacks use a
separate Oberon context and a bounded 4 KiB scratch arena, not an application heap.
Only `Ok(true)` authenticates: this SDK also returns hardware error `-147` for
invalid signatures. Valid empty-message signatures return that error too, so
the bridge explicitly rejects empty messages; release messages are nonempty.

`firmware/cc312-verifier/test_vectors.rs` retains the hardware self-check:
include it in a target application and call `check()` before boot confirmation.
It covers three nonempty RFC 8032 vectors, explicit empty-message
rejection, malformed R, noncanonical S and changed messages, with a known-good
verification after each rejection. This matrix passed on ATS `19e3543e7b6e`;
that unit was restored to published application 0.1.23 afterward.

Hardware DFU checks on ATS `63db8ec382ad` accepted a production-key application,
rejected unsigned and tampered-signature images, and recovered after rejection.
An application transfer interrupted after 49,152 bytes left the confirmed app
bootable after reset; a matching signed image then installed successfully.
A signed stage2 1.0.0 self-update rebooted into the app and exposed working DFU
again. This was a same-version update, without a final flash read-back.
Signed net-core installation reported release 4 on its first boot after the
handoff fix; an unconfirmed application trial rolled back through the watchdog.
Legacy ATS Lite handoff was compiled but not hardware-tested; marker hardware
was not tested.

## Schema rules

- Hosts that predate a field ignore it. Adding a field never stops them parsing,
  but they also do not act on it: they still take the newest entry.
- `requires_bootloader`, `requires_netcore`, and `min_desktop` are required by shared manifest schema. Hosts reject incomplete manifests; xtask validates before writing and preserves unknown JSON fields.
- A change old hosts must not act on at all needs a bump of the manifest's
  top-level `version`, which hosts built after that check refuse to read.
