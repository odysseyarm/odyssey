# Firmware manifests and releases

Hosts (Odyssey Desktop) learn what firmware exists from one JSON manifest per
channel and download the images from GitHub releases.

| File | Who follows it | How it changes |
|---|---|---|
| `releases.alpha.json` | Users who chose Alpha in Settings | `cargo xtask firmware <board> gen-release` adds an entry |
| `releases.stable.json` | Everyone else (the default) | `cargo xtask firmware promote` copies an entry from alpha |

Hosts parse a manifest whole or not at all, and take the **first** entry they can
install, so entries are newest-first and a malformed entry hides every update.
`cargo test -p odyssey-hub-server --lib firmware` parses both files.

## Releasing everything that changed

Three commands cover Desktop, the SDKs, the dongle, the marker board and both
ATS Lites with their net cores and bootloaders. The sections below describe the
steps they run.

```
cargo xtask release plan      # what changed since each tag; reads only
cargo xtask release bump      # prepare patch versions; counters remain separate
cargo xtask release publish   # tag merged master commits and publish
```

- **Changed** means a file differs between the component's newest tag and HEAD
  in its crate, a path dependency it builds with, its lock file or its memory
  map. That errs toward bumping: a comment in a shared crate counts.
- `bump` prepares non-counter patch-version changes with `cargo-release`;
  release/push/tag actions are disabled in root `release.toml`. Counter changes
  remain separate until firmware version conversion. Commit changes on a release
  branch and open a pull request; review, pass required checks, and squash-merge
  before publishing.
  A raised net core also raises the application that delivers it. Bootloaders
  are raised only by name (`cargo xtask release bump atslite1-bootloader`): a
  bootloader release reflashes every unit's bootloader. Name components to bump
  only those.
- `publish` needs a clean tree and validates its commit against remote `master`
  through GitHub. After PR squash-merge, fetch master and publish from its merged
  SHA; final release tags point to merged `master` SHAs, not pre-squash branch
  SHAs. It tags net cores and bootloaders and waits for their workflows, then
  builds and publishes ATS Lite application assets and tags the remaining
  selected components.
  It generates ATS Lite and dongle alpha entries locally from published assets;
  commit each manifest change on a branch and submit a separate PR. Name
  components to publish only those; `--dry-run` prints tags.
- `min_desktop` is the oldest Desktop release from which every later one
  speaks the firmware's wire version (`DEVICE_PROTOCOL`, or `MUX_PROTOCOL` for
  the dongle), and never below the device's previous entry.
  `requires_bootloader` and `requires_netcore` carry over from the previous
  entry; raise one with `gen-release` by hand.
- Generated ATS Lite, dongle, and marker alpha entries require separate PRs
  after asset publication. Review, pass required checks, and squash-merge each
  before units are offered its release. Marker workflow opens its PR and
  explicitly dispatches required checks.

The bump and publish decisions come from the
[`pure-release`](https://github.com/Abrahamh08/pure-release) dependency. Component
mapping, dependency discovery and publishing I/O remain in `crates/xtask/src/release.rs`.
Host CI runs Kani against the dependency revision in `crates/xtask/Cargo.lock`:

```
nix build .#checks.x86_64-linux.release-proofs .#checks.x86_64-linux.wire-proofs
```

Release proofs cover every valid component table and combination of states at
the production table size (11 components):

- changed components are never current; source-version regressions are refused;
- outgoing parts force their carrier application to become releasable;
- one automatic bump reaches a fixed point; default publication accepts its
  ready components, ordered parts before applications, without duplicates;
- named publication requires a ready carrier, but does not publish it unless
  selected; default publication selects every ready carrier;
- numeric increments are monotonic and reject representation exhaustion
  (net-core counters: 65535; bootloader counters: 4095; other components: u64);
- the Desktop floor is the oldest compatible suffix, checked for histories of
  zero through five releases with arbitrary u64 version triples.

Wire proofs call production codecs against fixed byte layouts, not just
encode/decode roundtrips. Coverage includes legacy object records, object and
marker reports, packet headers, current marker GATT writes, USB application and
bootloader version fields, and device-version CBOR encoding plus fixed-width
unsigned-16-bit CBOR decoding. Legacy proofs use `std`;
marker and USB proofs also run without default features. Marker GATT input
lengths are bounded to 0..=3; other payloads have their fixed protocol sizes.
All bytes and version values within those bounds are symbolic.

Fixed payload proofs use fixed buffers instead of modeling heap allocation.
An arbitrary checked position covers every byte or suffix position without
repeating the same property in an unrolled loop. The Desktop-floor proof uses
Kissat; solver selection is per harness, based on measured verification time.

These checks do not infer whether an arbitrary source change needs a major
version bump. Uncovered packet payloads, transport behavior, semantic changes,
GitHub responses, dependency discovery, compatibility-floor selection beyond
the stated proof bound, and real hardware remain outside the proofs. Updating
both a codec and its frozen specification still requires review. A successful
proof is not a guarantee that an entire release is safe.

## Components and tags

Each image is released under its own tag. A manifest entry for an application
release pins the exact component releases it was tested with.

| Component | Tag | Version is | Assets |
|---|---|---|---|
| Application | `<device>-v<semver>` | Cargo version | `app-a.bin`, `app-b.bin` |
| Stage2 bootloader | `<device>-bootloader-v<N>` | counter `N` in `firmware/<board>-bootloader/src/main.rs` | `stage2-bootloader.{bin,hex,elf}` |
| Net core | `<device>-netcore-v<N>` | counter `N` in `firmware/<board>-net-core/src/main.rs` | `net-core.{bin,hex,elf}` |
| Marker application | `marker-control-board-v<semver>` | Cargo version | `app.{bin,hex,elf}`, `merged.hex`, factory `bootloader.{hex,elf}` |
| Marker factory bootloader | `marker-control-board-bootloader-v<N>` | release counter | `bootloader.{bin,hex,elf}` (SWD only) |

`<device>` is the manifest device key: `atslite1` (board `lite1`) or
`legacy-atslite1` (board `legacy-atslite`). Run every command from the
repository root.

`marker-control-board` is also both board name and manifest device key. Its
nRF52833 has one application image, no net core, and one USB DFU bootloader.
Manifest `files` contains only `app`; `requires_bootloader` is `1` and
`requires_netcore` is `0`. Never add `app-a`, `app-b`, `net-core`, or
`stage2-bootloader` to this device. Bootloader cannot self-update through DFU.

`dongle` entries are installed by Odyssey Desktop 0.7.5 and newer. The dongle
keeps its maker's bootloader (Fanstel or Laird, Nordic serial DFU), so `files`
contains both `dfu-fanstel` and `dfu-laird`; `requires_bootloader` and
`requires_netcore` are `0`. Assets are named
`dongle-fw-<semver>-dfu-<maker>.zip`. Desktop checks size/hash, tries the signed
init packets before writing an application, and selects the accepted package.
It targets the same physical USB port through bootloader/reconnect and confirms
the full dongle identity and reported firmware version before showing success.
Offline packs must carry both ZIPs. After publication, run
`cargo xtask dongle gen-release <min_desktop>`.

The bootloader counter is the `release!` literal in its `main.rs`. It is
published as `bcdDevice = 0xF000 | N` and as the DFU label `BOOTLOADER N`, and
recorded in the manifest, all from that one number. Raise it whenever the image
changes. Do not reuse a number.

## Releasing a bootloader

1. Raise the counter in `firmware/<board>-bootloader/src/main.rs` and merge to `master`.
2. Publish it, either way:
   - **Locally:** `cargo xtask firmware <board> release-bootloader`
     (`DRY_RUN=1` runs the checks and stops). It builds, refuses a dirty tree,
     a commit not on `origin/master`, or an existing tag, then creates the
     release as a draft and publishes it last.
   - **CI:** push the tag `<device>-bootloader-v<N>`. The
     *Bootloader Release* workflow checks the tag against the source, builds with
     the same `cargo xtask firmware <board> dist-bootloader` and publishes. It does nothing if a release
     for the tag already exists, so the two routes cannot overwrite each other.
3. Add it to a firmware release (below). `gen-release` hashes the **published**
   asset, not a local build, because a build made elsewhere is not bit-identical
   and the manifest must describe the bytes devices download.

Do not delete a published bootloader release; manifest entries point at it.

## Releasing a net core

The net core (the nRF5340 BLE controller) is released on its own under
`<device>-netcore-v<N>`, with assets `net-core.{bin,hex,elf}`.

1. Raise the counter in `firmware/<board>-net-core/src/main.rs` and merge to `master`. The
   counter is the literal in `macro_rules! release { () => { N }; }`; release
   tooling reads it through `cargo xtask firmware release-counter`, which ignores layout,
   so rustfmt may format the macro as it likes. It is `1..=65535`; `0` means
   "unknown" and is never released.
2. Publish it, either way:
   - **Locally:** `cargo xtask firmware <board> release-netcore`
     (`DRY_RUN=1` runs the checks and stops). It has the same checks and
     draft-then-publish order as `release-bootloader`.
   - **CI:** push the tag `<device>-netcore-v<N>`. The *Net Core Release*
     workflow checks the tag against the source, builds with the same
     `cargo xtask firmware <board> dist-netcore` and publishes, standing down if a release for the tag
     already exists.
3. Add it to a firmware release (below). `gen-release` hashes the **published**
   `net-core.bin`, not a local build, and records `release` on the `net-core`
   file entry.

The device reports the counter itself, so a host reads it back to verify the
copy; a net core that fails to install simply does not answer. Nothing reports
the net core while the device is in the stage2 bootloader, and a net core update
leaves the device there until an application is written, so the read-back only
happens once the new application is running. Reading it needs device protocol
0.1.5 or newer; older firmware cannot decode the request, so a host must not ask
it, and treats such a device's net core as unknown.

Do not delete a published net-core release; manifest entries point at it.

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
   bridge is still the newest entry: `gen-release` refuses a floor above the net
   core carried by the current newest entry.

The last rule exists because hosts are greedy. A unit installs the newest entry
it is allowed to, so a bridge that a newer entry overtakes (one that needs no
net core) is never taken, the unit never receives N+1, and it is stranded below
the entry that requires it. The host tests walk every starting point through
both committed manifests and fail unless each unit reaches the newest entry.

Units reach later entries in two hops: first the bridge, which installs N+1,
then the later application once N+1 is reported as running.

## Releasing application firmware

Build the application, publish its immutable assets, then generate the alpha
entry from those published bytes:

```
cargo xtask firmware <board> build
cargo xtask firmware <board> gen-release <requires_bootloader> <requires_netcore> <min_desktop>
```

- `requires_bootloader`: the lowest bootloader release this application can run
  on, `0` for none. The host refuses to install it unless the device's
  bootloader, after any update in the same session, reaches it. It must not be
  above the bootloader being published.
- `requires_netcore`: the net-core release the unit must already be **running**,
  as it reports, before this entry may be installed; `0` for none. The host
  checks it against the net core the unit runs now, never against the one this
  update installs: nothing can verify a written net core until the new
  application runs. A unit that cannot report its net core counts as unknown and
  does not satisfy a floor above 0. It must be an integer and not above the
  highest net-core release carried by an entry already published for the device;
  `gen-release` refuses otherwise.
- `min_desktop`: the oldest Odyssey Desktop this firmware is required and tested
  with. It is not automatically the desktop shipping alongside; coupling a
  hotfix to a desktop upgrade it does not need strands users. Older desktops
  are told a newer firmware exists and where to get the desktop.

None has a default. Leaving one out would silently mean "any", so the command
and the manifest parser both refuse.

`gen-release` also refuses a bootloader or net-core image that changed without
a higher counter, a counter reused for a different image, and an image
published under two counters. Entries that predate a counter have no `release`
and count as 0.

After publishing application assets, generate the alpha entry from those
immutable bytes and submit a separate manifest PR. Require review, required
checks, and squash merge before units are offered the release. Promote it once
it has run on alpha units:

```
cargo xtask firmware promote <device> <version>
```

`promote` refuses, before writing anything, a version that is not newer than
stable's current newest for the device, and an entry whose `requires_netcore`
is above the net core stable's newest entry carries. Stable units install
stable's head, so promoting the entry that needs a net core before the bridge
that delivers it would publish a head none of them can install; promote the
bridge first, then the entry that requires it.

The xtask unit tests (`cargo test --manifest-path crates/xtask/Cargo.toml`) run
these refusals against synthetic manifests and leave the committed manifests alone.

### Marker control board

Initial installation requires SWD: old upstream firmware has no USB DFU loader.
`cargo xtask firmware marker-control-board build` creates `build/merged.hex`
for factory programming and `build/app.{bin,hex,elf}` for application releases.
`check` checks both crates; `dist-bootloader` produces independent factory
artifacts. `flash`, `flash-chiperase`, `flash-recover`, and `flash-app` use
`nRF52833_xxAA`; full sector flashing resets boot state from
`firmware/memory/memory-marker-control-board.kdl`, never nRF5340 state addresses.
Probe commands write hardware; invoke only with device owner's approval.

Set `[package.metadata.firmware] min-desktop` in application Cargo.toml to the
oldest tested Desktop version; marker DFU-only discovery and single-image
updates require **0.7.3+**. Desktop lists markers without tracker controls and
verifies application version from USB `bcdDevice` after updating. Submit the
application version and compatibility floor for review and squash-merge before
publishing; final release tag must point to merged `master` SHA, not pre-squash
branch SHA. *Marker Control Board Release* builds application plus factory
images, publishes immutable GitHub assets, downloads published `app.bin`,
computes SHA256 and size, then opens a PR adding newest-first alpha entry onto
latest master. Generated entry describes immutable published bytes. Existing
releases are not overwritten; reruns accept identical manifest entries, reject
changed hashes, and reuse an open matching PR without replacing branch contents.
A branch without an open PR causes refusal. The PR receives explicit host-ci,
fmt, and tracking-regression workflow dispatches because token-created PRs do
not trigger `pull_request` checks.

Manual *Marker Control Board Release* dispatch builds/uploads artifacts only.
Factory bootloader release uses existing *Bootloader Release* workflow with
`marker-control-board-bootloader-v1`; no separate component release is required
before application publication. Factory images install through SWD only.

Local manifest generation runs **after** publishing application assets:

```
cargo xtask firmware marker-control-board gen-release 1 0 <min_desktop>
cargo xtask firmware promote marker-control-board <version>
```

Unlike dual-bank recipes, marker `gen-release` hashes published `app.bin` directly,
never local build or unpublished components. Offline packs accept either single
`app` or complete `app-a`/`app-b` layout, refusing mixed or incomplete layouts;
single-image entries cannot carry net-core/bootloader files or nonzero net-core
requirement. Existing checksum and relative-path checks apply unchanged.

## Schema rules

- Hosts that predate a field ignore it. Adding a field never stops them parsing,
  but they also do not act on it: they still take the newest entry.
- A field new hosts must not default (`requires_bootloader`, `requires_netcore`,
  `min_desktop`) is required in the host's parser, and every existing entry is
  backfilled.
- A change old hosts must not act on at all needs a bump of the manifest's
  top-level `version`, which hosts built after that check refuse to read.
