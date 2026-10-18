# Firmware manifests and releases

Hosts (Odyssey Desktop) learn what firmware exists from one JSON manifest per
channel and download the images from GitHub releases.

| File | Who follows it | How it changes |
|---|---|---|
| `releases.alpha.json` | Users who chose Alpha in Settings | `just gen-release` adds an entry |
| `releases.stable.json` | Everyone else (the default) | `promote.sh` copies an entry from alpha |

Hosts parse a manifest whole or not at all, and take the **first** entry they can
install, so entries are newest-first and a malformed entry hides every update.
`cargo test -p odyssey-hub-server --lib firmware` parses both files.

## Components and tags

Each image is released under its own tag. A manifest entry for an application
release pins the exact component releases it was tested with.

| Component | Tag | Version is | Assets |
|---|---|---|---|
| Application | `<device>-v<semver>` | Cargo version | `app-a.bin`, `app-b.bin` |
| Stage2 bootloader | `<device>-bootloader-v<N>` | counter `N` in `bootloader/src/main.rs` | `stage2-bootloader.{bin,hex,elf}` |
| Net core | `<device>-netcore-v<N>` | counter `N` in `net-core/src/main.rs` | `net-core.{bin,hex,elf}` |

`<device>` is the manifest device key: `atslite1` or `legacy-atslite1`.

The bootloader counter is the `release!` literal in its `main.rs`. It is
published as `bcdDevice = 0xF000 | N` and as the DFU label `BOOTLOADER N`, and
recorded in the manifest, all from that one number. Raise it whenever the image
changes. Do not reuse a number.

## Releasing a bootloader

1. Raise the counter in `bootloader/src/main.rs` and merge to `master`.
2. Publish it, either way:
   - **Locally:** from the board directory, `just release-bootloader`
     (`DRY_RUN=1` runs the checks and stops). It builds, refuses a dirty tree,
     a commit not on `origin/master`, or an existing tag, then creates the
     release as a draft and publishes it last.
   - **CI:** push the tag `<device>-bootloader-v<N>`. The
     *Bootloader Release* workflow checks the tag against the source, builds with
     the same `just dist-bootloader` and publishes. It does nothing if a release
     for the tag already exists, so the two routes cannot overwrite each other.
3. Add it to a firmware release (below). `gen-release` hashes the **published**
   asset, not a local build, because a build made elsewhere is not bit-identical
   and the manifest must describe the bytes devices download.

Do not delete a published bootloader release; manifest entries point at it.

## Releasing a net core

The net core (the nRF5340 BLE controller) is released on its own under
`<device>-netcore-v<N>`, with assets `net-core.{bin,hex,elf}`.

1. Raise the counter in `net-core/src/main.rs` and merge to `master`. The
   counter is the literal in `macro_rules! release { () => { N }; }`; release
   tooling reads it through `firmware/release-counter`, which ignores layout,
   so rustfmt may format the macro as it likes. It is `1..=65535`; `0` means
   "unknown" and is never released.
2. Publish it, either way:
   - **Locally:** from the board directory, `just release-netcore`
     (`DRY_RUN=1` runs the checks and stops). It has the same checks and
     draft-then-publish order as `release-bootloader`.
   - **CI:** push the tag `<device>-netcore-v<N>`. The *Net Core Release*
     workflow checks the tag against the source, builds with the same
     `just dist-netcore` and publishes, standing down if a release for the tag
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

From the board directory:

```
just build
just gen-release <requires_bootloader> <requires_netcore> <min_desktop>
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

None has a default. Leaving one out would silently mean "any", so the recipe
and the manifest parser both refuse.

`gen-release` also refuses a bootloader or net-core image that changed without
a higher counter, a counter reused for a different image, and an image
published under two counters. Entries that predate a counter have no `release`
and count as 0.

Then tag and publish the application release, and promote it once it has run on
alpha units:

```
firmware/manifests/promote.sh <device> <version>
```

`promote.sh` refuses, before writing anything, a version that is not newer than
stable's current newest for the device, and an entry whose `requires_netcore`
is above the net core stable's newest entry carries. Stable units install
stable's head, so promoting the entry that needs a net core before the bridge
that delivers it would publish a head none of them can install; promote the
bridge first, then the entry that requires it.

`test-promote.sh` runs these refusals against synthetic manifests in a scratch
directory and leaves the committed manifests alone.

## Schema rules

- Hosts that predate a field ignore it. Adding a field never stops them parsing,
  but they also do not act on it: they still take the newest entry.
- A field new hosts must not default (`requires_bootloader`, `requires_netcore`,
  `min_desktop`) is required in the host's parser, and every existing entry is
  backfilled.
- A change old hosts must not act on at all needs a bump of the manifest's
  top-level `version`, which hosts built after that check refuse to read.
